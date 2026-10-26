//! 터미널 입력 — crossterm `EventStream` 을 **자기 깨움에만** 묻는다.
//!
//! `EventStream::poll_next` 는 부를 때마다 crossterm 의 전역 입력 잠금을
//! `try_lock_for(0)` 으로 잡아 본다. 그런데 키를 기다리는 동안 그 잠금은
//! `EventStream` 자신의 배경 스레드가 쥐고 있다(`poll_internal(None)` 안에서
//! 블록). 그래서 키가 없는 매 폴링이 `parking_lot` 의 `lock_slow` 로 떨어지고,
//! 거기서 `sched_yield` 를 일곱 번 부른 뒤에야 `Pending` 을 돌려준다.
//!
//! 턴 루프의 `select!` 는 틱·블록·감시자 — 무엇이 깨우든 이 팔을 다시
//! 폴링하므로, 모델을 기다리는 동안 메인 스레드 표본의 43.5% 가 `swtch_pri`
//! 에 있었다(2026-09-28, `sample`, 스트리밍 중 25.5%). 붐비는 기계에서
//! `sched_yield` 한 번은 수 밀리초를 기다리고, 그동안 키도 프레임도 못 돈다.
//!
//! 새 입력은 배경 스레드가 우리 waker 를 깨울 때에만 생긴다. 그러니 깨움을
//! 받기 전의 폴링은 잠금을 건드리지 않고 `Pending` 을 돌려주면 된다.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

use crossterm::event::{Event, EventStream};
use futures_util::task::AtomicWaker;
use futures_util::Stream;
use tokio::time::{Instant, Sleep};

/// 터미널이 끊겼는지 묻는 간격. 흐름은 이 간격마다 스스로 깨어 묻는다 — 유휴
/// 루프는 `IDLE_TEND` 마다만 깨므로, 루프의 폴링에 얹혀서는 닫힌 판을 그만큼
/// 늦게 본다(t-76434). 깨어서는 묻기만 하고 안쪽 스트림은 건드리지 않는다.
const HANG_UP_LOOK: Duration = Duration::from_millis(500);

/// 안쪽 스트림에 건네는 waker 의 몫 — 깨움이 왔다는 표시와, 그 깨움을
/// 넘겨줄 지금의 태스크.
#[derive(Default)]
struct Wakeup {
    woken: AtomicBool,
    task: AtomicWaker,
}

impl Wake for Wakeup {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.woken.store(true, Ordering::Release);
        self.task.wake();
    }
}

/// 턴 루프와 유휴 루프가 기다리는 키·붙여넣기·리사이즈의 흐름.
pub struct TerminalEvents<S = EventStream> {
    inner: S,
    wakeup: Arc<Wakeup>,
    /// 안쪽이 마지막에 `Pending` 이었다 — 다음 새 입력은 깨움으로 온다.
    armed: bool,
    /// 안쪽 뒤의 터미널이 끊겼는가 — 창이 판을 닫았다. 판 pty 는 누구의 제어
    /// 터미널도 아니라 SIGHUP 이 오지 않고, crossterm 의 읽기 스레드는 닫힌
    /// pty 의 EOF 에 다시 poll 하며 영원히 돌아 안쪽은 스스로 끝나지 않는다
    /// (t-76434). 그래서 이 흐름이 끝내고, 루프들의 입력 끝 팔이 턴 취소·기록·
    /// 종료를 한다.
    hung_up: fn() -> bool,
    /// 다음에 끊김을 물을 때. 런타임 시계의 타이머라, 아무것도 루프를 깨우지
    /// 않아도 이 흐름이 깨운다. 첫 폴링에 건다 — 런타임 안에서만 걸 수 있다.
    look: Option<Pin<Box<Sleep>>>,
    /// 끊김을 보고 끝났다 — 다음 폴링도 끝을 말한다.
    ended: bool,
}

impl TerminalEvents {
    #[must_use]
    pub fn new() -> Self {
        Self::over(EventStream::new(), super::tty::hung_up)
    }
}

impl Default for TerminalEvents {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> TerminalEvents<S> {
    fn over(inner: S, hung_up: fn() -> bool) -> Self {
        Self {
            inner,
            wakeup: Arc::new(Wakeup::default()),
            armed: false,
            hung_up,
            look: None,
            ended: false,
        }
    }
}

impl<S> Stream for TerminalEvents<S>
where
    S: Stream<Item = io::Result<Event>> + Unpin,
{
    type Item = io::Result<Event>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.ended {
            return Poll::Ready(None);
        }
        if this.look.as_mut().is_none_or(|look| look.as_mut().poll(cx).is_ready()) {
            if (this.hung_up)() {
                this.ended = true;
                return Poll::Ready(None);
            }
            let next = Instant::now() + HANG_UP_LOOK;
            let look = this.look.get_or_insert_with(|| Box::pin(tokio::time::sleep_until(next)));
            look.as_mut().reset(next);
            // 한 번 폴링해 두어야 마감이 이 태스크를 깨운다.
            let _ = look.as_mut().poll(cx);
        }
        // 등록이 표시 확인보다 먼저다: 확인과 등록 사이에 온 깨움이 옛
        // 태스크로 새지 않는다.
        this.wakeup.task.register(cx.waker());
        let woken = this.wakeup.woken.swap(false, Ordering::AcqRel);
        if this.armed && !woken {
            return Poll::Pending;
        }
        // 안쪽에는 언제나 우리 waker 를 건넨다 — 배경 스레드의 깨움이 표시를
        // 세운 뒤에 태스크로 간다.
        let waker = Waker::from(Arc::clone(&this.wakeup));
        let polled = Pin::new(&mut this.inner).poll_next(&mut Context::from_waker(&waker));
        this.armed = polled.is_pending();
        polled
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    #[cfg(unix)]
    use std::os::fd::{AsFd as _, OwnedFd};
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;
    #[cfg(unix)]
    use std::sync::OnceLock;
    use std::time::Duration;

    use crossterm::event::{KeyCode, KeyEvent};

    use super::*;

    /// crossterm 의 스트림처럼 구는 가짜: 폴링 수를 세고, 받은 waker 를
    /// 쥐었다가 키가 오면 깨운다.
    #[derive(Default)]
    struct Terminal {
        polls: usize,
        keys: VecDeque<Event>,
        waker: Option<Waker>,
    }

    #[derive(Clone, Default)]
    struct Fake(Arc<Mutex<Terminal>>);

    impl Fake {
        fn polls(&self) -> usize {
            self.0.lock().unwrap().polls
        }

        fn press(&self, code: KeyCode) {
            let waker = {
                let mut terminal = self.0.lock().unwrap();
                terminal.keys.push_back(Event::Key(KeyEvent::from(code)));
                terminal.waker.take()
            };
            if let Some(waker) = waker {
                waker.wake();
            }
        }
    }

    impl Stream for Fake {
        type Item = io::Result<Event>;

        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            let mut terminal = self.0.lock().unwrap();
            terminal.polls += 1;
            if let Some(event) = terminal.keys.pop_front() {
                Poll::Ready(Some(Ok(event)))
            } else {
                terminal.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }

    fn poll(events: &mut TerminalEvents<Fake>) -> Poll<Option<io::Result<Event>>> {
        let mut cx = Context::from_waker(Waker::noop());
        Pin::new(events).poll_next(&mut cx)
    }

    fn never_hung_up() -> bool {
        false
    }

    /// 창이 판을 닫으면 키가 하나도 오지 않아도 흐름이 끝나고, 끝난 흐름은
    /// 끝난 채다. 닫힌 터미널에는 읽을 것이 없으니 안쪽은 묻지 않는다.
    #[test]
    fn a_terminal_that_hung_up_ends_the_stream() {
        let terminal = Fake::default();
        let mut events = TerminalEvents::over(terminal.clone(), || true);
        assert!(matches!(poll(&mut events), Poll::Ready(None)));
        assert!(matches!(poll(&mut events), Poll::Ready(None)), "an ended stream came back");
        assert_eq!(terminal.polls(), 0, "a hung-up terminal was read");
    }

    /// 흐름을 폴링하는 태스크가 깨워진 횟수.
    #[derive(Default)]
    struct Wakes(AtomicUsize);

    impl Wakes {
        fn count(&self) -> usize {
            self.0.load(Ordering::Acquire)
        }
    }

    impl Wake for Wakes {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// 판이 끊긴 것을 이 흐름만 알 때 — 키도 틱도 없고, 유휴 루프는 다음
    /// `IDLE_TEND` 까지 잔다 — 흐름이 스스로 깨어 한 간격 안에 끝을 말한다.
    /// 간격 전에는 깨우지도 다시 묻지도 않고, 묻는 데 터미널을 읽지 않는다.
    #[tokio::test(start_paused = true)]
    async fn a_quiet_terminal_that_hangs_up_wakes_its_loop_to_end() {
        static HUNG_UP: AtomicBool = AtomicBool::new(false);
        fn hung_up() -> bool {
            HUNG_UP.load(Ordering::Acquire)
        }

        let terminal = Fake::default();
        let mut events = TerminalEvents::over(terminal.clone(), hung_up);
        let woke = Arc::new(Wakes::default());
        let waker = Waker::from(Arc::clone(&woke));
        let mut cx = Context::from_waker(&waker);
        assert!(Pin::new(&mut events).poll_next(&mut cx).is_pending());

        HUNG_UP.store(true, Ordering::Release);
        tokio::time::advance(HANG_UP_LOOK / 2).await;
        assert_eq!(woke.count(), 0, "the stream woke its loop before a look was due");
        assert!(
            Pin::new(&mut events).poll_next(&mut cx).is_pending(),
            "the stream looked again before a look was due"
        );

        tokio::time::advance(HANG_UP_LOOK).await;
        assert!(woke.count() > 0, "nothing woke the loop to look at the closed pane");
        assert!(matches!(Pin::new(&mut events).poll_next(&mut cx), Poll::Ready(None)));
        assert_eq!(terminal.polls(), 1, "a look read the terminal");
    }

    /// 원시 모드의 pty 한 쌍 — 주 끝을 닫는 것이 창이 판을 닫는 것이다.
    #[cfg(unix)]
    fn open_pty() -> nix::pty::OpenptyResult {
        use nix::sys::termios::{cfmakeraw, tcgetattr, tcsetattr, SetArg};

        let pty = nix::pty::openpty(None::<&nix::pty::Winsize>, None::<&nix::sys::termios::Termios>)
            .expect("open a pty");
        let mut raw = tcgetattr(&pty.slave).expect("the pty's modes");
        cfmakeraw(&mut raw);
        tcsetattr(&pty.slave, SetArg::TCSANOW, &raw).expect("raw mode");
        pty
    }

    /// 창이 판을 닫으면 그 닫힘이 루프를 깨우고 흐름을 그 자리에서 끝낸다 — 가짜
    /// 시계는 한 틱도 가지 않는다. 간격마다 묻는 흐름이었다면 그 간격이 오기 전에는
    /// 깨우지도 끝나지도 않는다.
    #[cfg(unix)]
    #[tokio::test(start_paused = true)]
    async fn a_pane_that_closes_ends_its_stream_on_the_spot() {
        static SLAVE: OnceLock<OwnedFd> = OnceLock::new();
        fn hung_up() -> bool {
            super::super::tty::hung_up_on(SLAVE.get().expect("the pty").as_fd())
        }

        let pty = open_pty();
        SLAVE.set(pty.slave).expect("one pty");
        let terminal = Fake::default();
        let mut events = TerminalEvents::over(terminal.clone(), hung_up);
        let woke = Arc::new(Wakes::default());
        let waker = Waker::from(Arc::clone(&woke));
        let mut cx = Context::from_waker(&waker);
        assert!(Pin::new(&mut events).poll_next(&mut cx).is_pending());

        let closed = tokio::time::Instant::now();
        drop(pty.master);
        // 기다리지 않고 양보한다: 멈춘 시계는 같은 스레드의 깨움이 와도 다음 타이머까지
        // 스스로 가지만(tokio current_thread), 양보는 반응기만 한 번 돌린다.
        tokio::task::yield_now().await;
        assert!(woke.count() > 0, "the closed pane did not wake the loop");
        assert!(
            matches!(Pin::new(&mut events).poll_next(&mut cx), Poll::Ready(None)),
            "the closed pane did not end the stream"
        );
        assert_eq!(closed.elapsed(), Duration::ZERO, "the clock moved, so a timer may have ended the stream");
        assert_eq!(terminal.polls(), 1, "a closed pane was read");
    }

    /// 살아 있는 판이 조용하면 이 흐름은 루프를 한 번도 깨우지 않는다 — 유휴
    /// 깨움 목표(`IDLE_TEND`, 0.5/s 이하)에 타이머 하나 보태지 않는다.
    #[cfg(unix)]
    #[tokio::test(start_paused = true)]
    async fn a_live_quiet_pane_never_wakes_its_loop() {
        static SLAVE: OnceLock<OwnedFd> = OnceLock::new();
        fn hung_up() -> bool {
            super::super::tty::hung_up_on(SLAVE.get().expect("the pty").as_fd())
        }

        let pty = open_pty();
        SLAVE.set(pty.slave).expect("one pty");
        let mut events = TerminalEvents::over(Fake::default(), hung_up);
        let woke = Arc::new(Wakes::default());
        let waker = Waker::from(Arc::clone(&woke));
        assert!(Pin::new(&mut events).poll_next(&mut Context::from_waker(&waker)).is_pending());

        tokio::time::sleep(Duration::from_secs(60)).await;
        assert_eq!(woke.count(), 0, "a live, quiet pane woke the loop");
        drop(pty.master);
    }

    /// 틱이 서른 번, 블록이 백 번 깨워도 조용한 터미널은 한 번만 묻는다.
    #[tokio::test]
    async fn a_quiet_terminal_is_asked_once_however_often_the_loop_wakes() {
        let terminal = Fake::default();
        let mut events = TerminalEvents::over(terminal.clone(), never_hung_up);
        for _ in 0..100 {
            assert!(poll(&mut events).is_pending());
        }
        assert_eq!(terminal.polls(), 1, "every wake of the loop asked the terminal again");
    }

    /// 키가 깨우면 그 폴링이 키를 읽고, 다음 폴링이 다시 기다림을 건다.
    #[tokio::test]
    async fn a_key_is_read_on_the_wake_that_brings_it() {
        let terminal = Fake::default();
        let mut events = TerminalEvents::over(terminal.clone(), never_hung_up);
        assert!(poll(&mut events).is_pending());
        terminal.press(KeyCode::Char('a'));
        match poll(&mut events) {
            Poll::Ready(Some(Ok(Event::Key(key)))) => assert_eq!(key.code, KeyCode::Char('a')),
            other => panic!("the key did not arrive: {other:?}"),
        }
        assert!(poll(&mut events).is_pending());
        assert!(poll(&mut events).is_pending());
        assert_eq!(terminal.polls(), 3, "one ask per wake and one to re-arm");
    }

    /// 루프들은 터미널을 이 모듈로만 읽는다 — `select!` 에서 폴링하는 맨
    /// `EventStream` 이 곧 이 모듈이 없앤 스핀이다.
    #[test]
    fn no_loop_polls_crosstermss_stream_directly() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![root];
        let mut offenders = Vec::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("src is readable").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|ext| ext == "rs")
                    && !path.ends_with("tui/input.rs")
                    && std::fs::read_to_string(&path).is_ok_and(|text| {
                        text.lines()
                            .any(|line| !line.trim_start().starts_with("//") && line.contains("EventStream"))
                    })
                {
                    offenders.push(path);
                }
            }
        }
        assert!(offenders.is_empty(), "these read crossterm's stream around TerminalEvents: {offenders:?}");
    }

    /// 한 깨움에 여러 키가 쌓였으면 비울 때까지 묻는다 — 붙여넣기 뒤의 Enter 가
    /// 다음 틱까지 밀리면 안 된다.
    #[tokio::test]
    async fn keys_queued_behind_one_wake_are_all_read() {
        let terminal = Fake::default();
        let mut events = TerminalEvents::over(terminal.clone(), never_hung_up);
        assert!(poll(&mut events).is_pending());
        terminal.press(KeyCode::Char('x'));
        terminal.press(KeyCode::Enter);
        assert!(matches!(poll(&mut events), Poll::Ready(Some(Ok(_)))));
        assert!(matches!(poll(&mut events), Poll::Ready(Some(Ok(_)))));
        assert!(poll(&mut events).is_pending());
    }
}
