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
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use crossterm::event::{Event, EventStream};
use futures_util::task::AtomicWaker;
use futures_util::Stream;

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
}

impl TerminalEvents {
    #[must_use]
    pub fn new() -> Self {
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
    use std::sync::Mutex;

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

    /// 틱이 서른 번, 블록이 백 번 깨워도 조용한 터미널은 한 번만 묻는다.
    #[test]
    fn a_quiet_terminal_is_asked_once_however_often_the_loop_wakes() {
        let terminal = Fake::default();
        let mut events = TerminalEvents::over(terminal.clone());
        for _ in 0..100 {
            assert!(poll(&mut events).is_pending());
        }
        assert_eq!(terminal.polls(), 1, "every wake of the loop asked the terminal again");
    }

    /// 키가 깨우면 그 폴링이 키를 읽고, 다음 폴링이 다시 기다림을 건다.
    #[test]
    fn a_key_is_read_on_the_wake_that_brings_it() {
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
    #[test]
    fn keys_queued_behind_one_wake_are_all_read() {
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
