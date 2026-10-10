//! 착지 전 점검(t-34501 3단계) — 원장의 반쪽.
//!
//! 코디가 `land-check`로 묻는다: 이 과업이 넘긴 머리를 비교 ref에 합치면 어떻게 되는가.
//! 원장은 git도 프로세스도 다루지 않는다. 그래서 이 파일이 하는 일은 둘뿐이다. 계획에서
//! 어느 과업·머리·저장소인지를 정해 창에 건네고([`ask`]), 창이 본 결과를 그 과업의 증거로
//! 받아 적는다([`Ledger::land_checked`]). 합치기, 점검 명령, 임시 폴더 정리는 창의
//! `land_check.rs`가 한다.
//!
//! 증거는 과업 행의 새 칸이 아니라 원장 자신의 편지(`land_check`)다. 과업 행에 칸을
//! 더하면 저장소의 표가 바뀌고, 편지는 이미 과업 id를 달고 코디 우편함으로 가며 원장이
//! 지울 때까지 남는다 — 코디의 `check --wait`도 그 편지로 깬다. 편지는 원장의 목소리로만
//! 쓰인다: 일반 `send --type land_check`는 거절된다([`MessageKind::is_the_ledgers_own`]).

use super::*;

/// The verb, spelled once: the table, the plan arm and the window all say it.
pub const LAND_CHECK_VERB: &str = "land-check";

/// The prefix every check id starts with. The window names its folders, rows and logs after the
/// id, so the shape is checked here before any of those paths is built.
pub const CHECK_PREFIX: &str = "lc-";

/// Whether `text` is a check id this window could have minted: `lc-<digits>-<digits>`.
///
/// A path is built from an id only after this says yes, so a `/`, a `..` or a separator of
/// another platform never reaches one.
#[must_use]
pub fn is_check_id(_text: &str) -> bool {
    // RED STAGE (t-42447): not built yet.
    false
}

/// What the window is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LandCheckAsk {
    /// Merge `head` into the compare ref in a throwaway checkout of the repository `checkout`
    /// belongs to. With `prepare`, stop there and keep the folder for a check run elsewhere;
    /// without it, run the project's check command (when it has one) and write the evidence.
    Merge {
        run: String,
        task: String,
        head: String,
        /// The worker's checkout, which only names the repository: the window asks git for
        /// its common directory and never reads or writes the checkout's own index or files.
        checkout: String,
        prepare: bool,
    },
    /// The end of a check run elsewhere on a prepared folder: its exit code and log, written
    /// as the task's evidence, and the folder removed. The window holds `task` and `head`
    /// against what it prepared and refuses a mismatch.
    Record {
        run: String,
        task: String,
        head: String,
        check: String,
        rc: i32,
        log: String,
        took_ms: Option<i64>,
    },
}

/// One check's evidence, as the window hands it to the ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LandCheckReceipt {
    pub run: String,
    pub task: String,
    pub check: String,
    /// The window's evidence as a JSON object. It must say its `state`; the ledger writes the
    /// `check` and `task` keys itself, so the body cannot name another task than the row.
    pub evidence: String,
}

/// The ledger's half of `land-check`: which task, which head, which repository.
///
/// The coordinator's verb, like `gate-create`: a worker that wants its own work checked says so
/// in its report, and the coordinator decides. The head defaults to the newest commit an
/// attempt on the task handed in (`Dispatch::source`), and the repository to the checkout of
/// the newest attempt whose worker was seated in one.
pub(super) fn ask(
    _ledger: &Ledger,
    _run_id: &str,
    _team: &str,
    _pane: &str,
    _words: &Words,
) -> Result<LandCheckAsk, String> {
    // RED STAGE (t-42447): not built yet.
    Err("land-check is not built yet".to_string())
}

impl Ledger {
    /// One check's evidence, in the ledger's own voice, to the coordinator of the run that holds
    /// the task — once per check id, however many times the window asks. Answers the row's id,
    /// or `None` when the row already stands.
    pub fn land_checked(
        &mut self,
        _receipt: &LandCheckReceipt,
        _now_ms: i64,
    ) -> Result<Option<String>, String> {
        // RED STAGE (t-42447): not built yet.
        Ok(None)
    }
}
