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

use std::io;
#[cfg(unix)]
use std::os::fd::{AsFd as _, OwnedFd};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use crossterm::event::{Event, EventStream};
use futures_util::task::AtomicWaker;
use futures_util::Stream;
#[cfg(unix)]
use tokio::io::{unix::AsyncFd, Interest};

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

/// 판이 닫혔는지 지켜보는 것. 판 pty 는 누구의 제어 터미널도 아니라 닫혀도
/// SIGHUP 이 오지 않고, crossterm 의 읽기 스레드는 닫힌 pty 의 EOF 에 다시
/// poll 하며 영원히 돌아 안쪽 흐름은 스스로 끝나지 않는다(t-76434). 그래서
/// 터미널 fd 의 사본을 런타임의 반응기(kqueue·epoll)에 걸어 둔다: 판이 닫히면
/// 그 fd 가 끝(EOF·HUP)으로 깨어 흐름을 깨운다. 키가 올 때도 깨지만 살아 있는
/// 조용한 판에서는 아무것도 깨우지 않는다 — 유휴 깨움 목표(`IDLE_TEND`, 0.5/s 이하)에
/// 타이머 하나 보태지 않는다.
#[cfg(unix)]
enum Watch {
    /// 지켜볼 터미널이 없거나 반응기가 그 fd 를 받지 않았다 — 판이 닫혀도 이
    /// 흐름은 모른다.
    Off,
    /// 아직 걸지 않았다 — 반응기는 런타임 안에서만 쓸 수 있어 런타임 안의 첫
    /// 폴링에 건다.
    Unarmed(OwnedFd),
    Armed(AsyncFd<OwnedFd>),
}

#[cfg(unix)]
impl Watch {
    /// 판이 닫혔는가. 아니면 다음 변화(키·닫힘)에 `cx` 의 태스크를 깨우게 둔다.
    fn hung_up(&mut self, cx: &mut Context<'_>) -> bool {
        if matches!(self, Self::Unarmed(_)) {
            let Self::Unarmed(terminal) = std::mem::replace(self, Self::Off) else {
                return false;
            };
            // 걸기 전에 이미 닫힌 판은 반응기의 첫 소식을 기다리지 않고 여기서 끝난다 —
            // 기다리는 동안 닫힌 터미널을 읽게 된다.
            if super::tty::hung_up_on(terminal.as_fd()) {
                return true;
            }
            // `AsyncFd` 는 런타임 밖에서 걸면 오류가 아니라 패닉이다. 런타임 밖의
            // 폴링은 걸지 않고 두며, 닫힘은 다음 폴링이 다시 본다.
            if tokio::runtime::Handle::try_current().is_err() {
                *self = Self::Unarmed(terminal);
                return false;
            }
            if let Ok(armed) = AsyncFd::with_interest(terminal, Interest::READABLE) {
                *self = Self::Armed(armed);
            }
        }
        let Self::Armed(watch) = self else {
            return false;
        };
        loop {
            match watch.poll_read_ready(cx) {
                Poll::Pending => return false,
                Poll::Ready(Ok(mut ready)) => {
                    // 키로도 깨어난다 — 끊김은 반응기가 본 끝, 또는 poll 의 HUP 뿐이다.
                    if ready.ready().is_read_closed() || super::tty::hung_up_on(watch.get_ref().as_fd()) {
                        return true;
                    }
                    ready.clear_ready();
                }
                Poll::Ready(Err(_)) => {
                    *self = Self::Off;
                    return false;
                }
            }
        }
    }
}

/// 턴 루프와 유휴 루프가 기다리는 키·붙여넣기·리사이즈의 흐름.
pub struct TerminalEvents<S = EventStream> {
    inner: S,
    wakeup: Arc<Wakeup>,
    /// 안쪽이 마지막에 `Pending` 이었다 — 다음 새 입력은 깨움으로 온다.
    armed: bool,
    /// 판이 닫혔는지 지켜본다. 닫힌 판에서는 이 흐름이 끝나고, 루프들의 입력
    /// 끝 팔이 턴 취소·기록·종료를 한다.
    #[cfg(unix)]
    watch: Watch,
    /// 끊김을 보고 끝났다 — 다음 폴링도 끝을 말한다.
    ended: bool,
}

impl TerminalEvents {
    #[must_use]
    pub fn new() -> Self {
        #[cfg(unix)]
        if let Ok(terminal) = std::io::stdin().as_fd().try_clone_to_owned() {
            return Self::over(EventStream::new()).watching(terminal);
        }
        Self::over(EventStream::new())
    }
}

impl Default for TerminalEvents {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> TerminalEvents<S> {
    fn over(inner: S) -> Self {
        Self {
            inner,
            wakeup: Arc::new(Wakeup::default()),
            armed: false,
            #[cfg(unix)]
            watch: Watch::Off,
            ended: false,
        }
    }

    /// `terminal` 이 닫히면 흐름이 끝난다 — zo 에서는 판의 pty.
    #[cfg(unix)]
    fn watching(mut self, terminal: OwnedFd) -> Self {
        self.watch = Watch::Unarmed(terminal);
        self
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
        #[cfg(unix)]
        if this.watch.hung_up(cx) {
            this.ended = true;
            return Poll::Ready(None);
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
    use std::io::Write as _;
    #[cfg(unix)]
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;
    #[cfg(unix)]
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

    /// 늦은 닫힘을 기다리는 상한 — 통과는 닫힘이 보이는 즉시이고, 이 상한은 실패에만
    /// 쓴다. 닫힘이 늦는 것은 주 끝이 열린 동안 병렬 시험이 띄운 자식이 그 사본을
    /// 물려받아(`openpty` 의 두 끝은 상속된다) 제가 끝날 때까지 쥘 때다(l81d).
    #[cfg(unix)]
    const HANG_UP_BOUND: Duration = Duration::from_secs(60);

    /// 창이 판을 닫으면 키가 하나도 오지 않아도 흐름이 끝나고, 끝난 흐름은
    /// 끝난 채다. 닫힌 터미널에는 읽을 것이 없으니 끝을 본 폴링은 안쪽을 묻지 않는다.
    /// 닫힘은 한 번의 폴링이 아니라 반응기의 소식으로 기다린다(`HANG_UP_BOUND`).
    #[cfg(unix)]
    #[tokio::test]
    async fn a_terminal_that_hung_up_ends_the_stream() {
        let pty = open_pty();
        drop(pty.master);
        let terminal = Fake::default();
        let mut events = TerminalEvents::over(terminal.clone()).watching(pty.slave);
        let ended = tokio::time::timeout(
            HANG_UP_BOUND,
            std::future::poll_fn(|cx| {
                let asked = terminal.polls();
                Pin::new(&mut events).poll_next(cx).map(|item| (item.is_none(), terminal.polls() - asked))
            }),
        )
        .await;
        assert!(matches!(ended, Ok((true, 0))), "the hung-up terminal did not end the stream unread: {ended:?}");
        assert!(matches!(poll(&mut events), Poll::Ready(None)), "an ended stream came back");
    }

    /// 런타임 밖의 폴링도 패닉하지 않는다 — 반응기는 런타임 안에서만 걸 수 있으니
    /// 살아 있는 판은 걸지 않은 채 기다리고, 닫힘은 다음 폴링이 본다. 안쪽은 처음 한
    /// 번만 묻는다(l81d: 닫힘이 첫 폴링에 아직 안 보이자 런타임 밖에서 반응기에 걸려다
    /// 패닉했다).
    #[cfg(unix)]
    #[test]
    fn a_terminal_polled_outside_a_runtime_neither_panics_nor_misses_its_hang_up() {
        let pty = open_pty();
        let terminal = Fake::default();
        let mut events = TerminalEvents::over(terminal.clone()).watching(pty.slave);
        assert!(poll(&mut events).is_pending(), "a live terminal ended the stream");

        drop(pty.master);
        let until = std::time::Instant::now() + HANG_UP_BOUND;
        let ended = loop {
            match poll(&mut events) {
                Poll::Pending => {
                    assert!(std::time::Instant::now() < until, "no later poll saw the hang-up");
                    std::thread::sleep(Duration::from_millis(5));
                }
                ready @ Poll::Ready(_) => break ready,
            }
        };
        assert!(matches!(ended, Poll::Ready(None)), "the hang-up did not end the stream: {ended:?}");
        assert!(matches!(poll(&mut events), Poll::Ready(None)), "an ended stream came back");
        assert_eq!(terminal.polls(), 1, "a later poll asked the terminal again");
    }

    /// 흐름을 폴링하는 태스크가 깨워진 횟수.
    #[cfg(unix)]
    #[derive(Default)]
    struct Wakes(AtomicUsize);

    #[cfg(unix)]
    impl Wakes {
        fn count(&self) -> usize {
            self.0.load(Ordering::Acquire)
        }
    }

    #[cfg(unix)]
    impl Wake for Wakes {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// 원시 모드의 pty 한 쌍 — 주 끝을 닫는 것이 창이 판을 닫는 것이다.
    #[cfg(unix)]
    fn open_pty() -> nix::pty::OpenptyResult {
        use nix::sys::termios::{cfmakeraw, tcgetattr, tcsetattr, SetArg};

        let pty = crate::tui::test_pty::open_pty(None).expect("open a pty");
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
        let pty = open_pty();
        let terminal = Fake::default();
        let mut events = TerminalEvents::over(terminal.clone()).watching(pty.slave);
        let woke = Arc::new(Wakes::default());
        let waker = Waker::from(Arc::clone(&woke));
        let mut cx = Context::from_waker(&waker);
        assert!(Pin::new(&mut events).poll_next(&mut cx).is_pending());

        let closed = tokio::time::Instant::now();
        drop(pty.master);
        // 기다리지 않고 양보한다: 멈춘 시계는 같은 스레드의 깨움이 와도 다음 타이머까지
        // 스스로 가지만(tokio current_thread), 양보는 반응기만 한 번 돌린다. 닫힘이 늦으면
        // (`HANG_UP_BOUND`) 진짜 시계로 잠깐씩 쉬며 다시 양보한다 — 멈춘 시계는 그동안에도
        // 가지 않는다.
        let until = std::time::Instant::now() + HANG_UP_BOUND;
        loop {
            tokio::task::yield_now().await;
            if woke.count() > 0 {
                break;
            }
            assert!(std::time::Instant::now() < until, "the closed pane did not wake the loop");
            std::thread::sleep(Duration::from_millis(5));
        }
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
        let pty = open_pty();
        let mut events = TerminalEvents::over(Fake::default()).watching(pty.slave);
        let woke = Arc::new(Wakes::default());
        let waker = Waker::from(Arc::clone(&woke));
        assert!(Pin::new(&mut events).poll_next(&mut Context::from_waker(&waker)).is_pending());

        tokio::time::sleep(Duration::from_secs(60)).await;
        assert_eq!(woke.count(), 0, "a live, quiet pane woke the loop");
        drop(pty.master);
    }

    /// 키가 오면 감시도 깨지만 판이 닫힌 것은 아니다 — 흐름은 열린 채이고, 그
    /// 깨움으로 터미널을 다시 묻지도 않는다. 키는 crossterm 의 깨움으로 온다.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_key_on_a_live_pane_neither_ends_the_stream_nor_asks_again() {
        let pty = open_pty();
        let terminal = Fake::default();
        let mut events = TerminalEvents::over(terminal.clone()).watching(pty.slave);
        let woke = Arc::new(Wakes::default());
        let waker = Waker::from(Arc::clone(&woke));
        let mut cx = Context::from_waker(&waker);
        assert!(Pin::new(&mut events).poll_next(&mut cx).is_pending());

        let mut master = std::fs::File::from(pty.master);
        master.write_all(b"a").expect("type a key");
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while woke.count() == 0 {
            assert!(std::time::Instant::now() < until, "the key never reached the watch");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(Pin::new(&mut events).poll_next(&mut cx).is_pending(), "a key ended the stream");
        assert_eq!(terminal.polls(), 1, "the watch's wake asked the terminal again");
    }

    /// 틱이 서른 번, 블록이 백 번 깨워도 조용한 터미널은 한 번만 묻는다.
    #[tokio::test]
    async fn a_quiet_terminal_is_asked_once_however_often_the_loop_wakes() {
        let terminal = Fake::default();
        let mut events = TerminalEvents::over(terminal.clone());
        for _ in 0..100 {
            assert!(poll(&mut events).is_pending());
        }
        assert_eq!(terminal.polls(), 1, "every wake of the loop asked the terminal again");
    }

    /// 키가 깨우면 그 폴링이 키를 읽고, 다음 폴링이 다시 기다림을 건다.
    #[tokio::test]
    async fn a_key_is_read_on_the_wake_that_brings_it() {
        let terminal = Fake::default();
        let mut events = TerminalEvents::over(terminal.clone());
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
        let mut events = TerminalEvents::over(terminal.clone());
        assert!(poll(&mut events).is_pending());
        terminal.press(KeyCode::Char('x'));
        terminal.press(KeyCode::Enter);
        assert!(matches!(poll(&mut events), Poll::Ready(Some(Ok(_)))));
        assert!(matches!(poll(&mut events), Poll::Ready(Some(Ok(_)))));
        assert!(poll(&mut events).is_pending());
    }
}
