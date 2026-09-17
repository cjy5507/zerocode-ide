//! stdin 단일 소유자 — cooked-mode 줄 읽기와 붙여넣기 봉투 누적.
//!
//! IDE 는 여러 줄 프롬프트를 `ESC[200~ … ESC[201~` 로 감싸 보낸다. tty 의
//! 정규 모드는 줄마다 따로 넘기므로, 시작 마커부터 끝 마커까지의 줄을 하나의
//! 제출로 다시 합쳐야 여러 줄 붙여넣기가 N 턴이 아니라 한 턴이 된다.
//! 사용자 텍스트 안의 ESC 는 IDE 가 `␛` 로 바꿔 보내므로 마커는 모호하지 않다.

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

pub const PASTE_START: &str = "\u{1b}[200~";
pub const PASTE_END: &str = "\u{1b}[201~";

/// stdin 에서 온 한 사건.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    /// 완성된 제출(단일 줄, 또는 붙여넣기 봉투를 합친 여러 줄).
    Submit(String),
    /// Ctrl-D / 파이프 종료.
    Eof,
}

/// 붙여넣기 봉투를 줄 단위 입력에서 되살리는 누적기.
#[derive(Debug, Default)]
pub struct PasteAccumulator {
    open: Option<String>,
}

impl PasteAccumulator {
    /// 개행이 벗겨진 줄 하나를 먹이고, 제출이 완성되면 돌려준다.
    pub fn push_line(&mut self, raw: &str) -> Option<String> {
        if let Some(buffer) = self.open.as_mut() {
            if let Some(end) = raw.find(PASTE_END) {
                buffer.push('\n');
                buffer.push_str(&raw[..end]);
                buffer.push_str(&raw[end + PASTE_END.len()..]);
                return self.open.take();
            }
            buffer.push('\n');
            buffer.push_str(raw);
            return None;
        }
        let Some(start) = raw.find(PASTE_START) else {
            return Some(raw.to_string());
        };
        let mut text = String::new();
        text.push_str(&raw[..start]);
        let rest = &raw[start + PASTE_START.len()..];
        if let Some(end) = rest.find(PASTE_END) {
            text.push_str(&rest[..end]);
            text.push_str(&rest[end + PASTE_END.len()..]);
            return Some(text);
        }
        text.push_str(rest);
        self.open = Some(text);
        None
    }

    /// EOF 시 열린 봉투가 있으면 그대로 제출로 만든다.
    pub fn flush(&mut self) -> Option<String> {
        self.open.take().filter(|text| !text.trim().is_empty())
    }
}

/// stdin 을 끝까지 읽어 `tx` 로 사건을 보낸다. 루프의 유일한 stdin 독자.
pub async fn pump_stdin(tx: mpsc::Sender<InputEvent>) {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut paste = PasteAccumulator::default();
    loop {
        let Ok(Some(line)) = lines.next_line().await else {
            if let Some(submit) = paste.flush() {
                let _ = tx.send(InputEvent::Submit(submit)).await;
            }
            let _ = tx.send(InputEvent::Eof).await;
            return;
        };
        let line = line.trim_end_matches('\r');
        if let Some(submit) = paste.push_line(line) {
            if tx.send(InputEvent::Submit(submit)).await.is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PasteAccumulator, PASTE_END, PASTE_START};

    #[test]
    fn plain_line_passes_through() {
        let mut acc = PasteAccumulator::default();
        assert_eq!(acc.push_line("hello"), Some("hello".to_string()));
    }

    #[test]
    fn single_line_paste_strips_markers() {
        let mut acc = PasteAccumulator::default();
        let line = format!("{PASTE_START}fix it{PASTE_END}");
        assert_eq!(acc.push_line(&line), Some("fix it".to_string()));
    }

    #[test]
    fn multi_line_paste_becomes_one_submission() {
        let mut acc = PasteAccumulator::default();
        assert_eq!(acc.push_line(&format!("{PASTE_START}line one")), None);
        assert_eq!(acc.push_line("line two"), None);
        assert_eq!(
            acc.push_line(&format!("line three{PASTE_END}")),
            Some("line one\nline two\nline three".to_string())
        );
        assert_eq!(acc.push_line("after"), Some("after".to_string()));
    }

    #[test]
    fn eof_flushes_an_open_envelope() {
        let mut acc = PasteAccumulator::default();
        assert_eq!(acc.push_line(&format!("{PASTE_START}dangling")), None);
        assert_eq!(acc.flush(), Some("dangling".to_string()));
        assert_eq!(acc.flush(), None);
    }
}
