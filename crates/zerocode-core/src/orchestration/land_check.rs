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
//! 편지의 본문은 에이전트가 읽는 한 줄이고, 증거 전체는 페이로드의 JSON이다.

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
pub fn is_check_id(text: &str) -> bool {
    let Some(rest) = text.strip_prefix(CHECK_PREFIX) else {
        return false;
    };
    let mut parts = rest.split('-');
    let digits = |part: Option<&str>| {
        part.is_some_and(|part| {
            !part.is_empty() && part.len() <= 20 && part.bytes().all(|b| b.is_ascii_digit())
        })
    };
    digits(parts.next()) && digits(parts.next()) && parts.next().is_none()
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
    ledger: &Ledger,
    run_id: &str,
    team: &str,
    pane: &str,
    words: &Words,
) -> Result<LandCheckAsk, String> {
    let run = ledger.run(run_id).ok_or_else(|| unknown_run(run_id))?;
    if let Some(worker) = run.worker_in_pane(team, pane) {
        return Err(format!(
            "{LAND_CHECK_VERB} is the coordinator's verb, and this pane is worker {} — say in \
             your report which head you want checked and let the coordinator run it",
            worker.id
        ));
    }
    let task_id = words
        .value("--task")
        .ok_or("land-check needs --task <id>")?;
    let task = run
        .task(task_id)
        .ok_or_else(|| format!("unknown task: {task_id}"))?;
    let named_head = match words.value("--head") {
        Some(named) => Some(commit_named(named).ok_or_else(|| {
            format!("--head names a commit, seven hex digits or more, and {named:?} is not one")
        })?),
        None => None,
    };
    if let Some(check) = words.value("--record") {
        if !is_check_id(check) {
            return Err(format!(
                "--record names a check this window prepared (lc-<ms>-<n>), and {check:?} is not one"
            ));
        }
        if words.has("--prepare") {
            return Err("--record ends a prepared check; it cannot prepare another".into());
        }
        let head = named_head.ok_or(
            "land-check --record needs --head: the head the check was prepared for, said back",
        )?;
        let rc = words
            .value("--rc")
            .ok_or("land-check --record needs --rc <exit code>")?;
        let rc = rc
            .parse::<i32>()
            .map_err(|_| format!("--rc is an exit code, and {rc:?} is not one"))?;
        let log = words
            .value("--log")
            .ok_or("land-check --record needs --log <the log the prepared check named>")?;
        if !std::path::Path::new(log).is_absolute() {
            return Err(format!("--log is an absolute path, and {log:?} is not one"));
        }
        let took_ms = match words.value("--took-ms") {
            Some(took) => Some(
                took.parse::<i64>()
                    .ok()
                    .filter(|took| *took >= 0)
                    .ok_or_else(|| format!("--took-ms is milliseconds, and {took:?} is not"))?,
            ),
            None => None,
        };
        return Ok(LandCheckAsk::Record {
            run: run.id.clone(),
            task: task.id.clone(),
            head,
            check: check.to_string(),
            rc,
            log: log.to_string(),
            took_ms,
        });
    }
    let attempts: Vec<&Dispatch> = run
        .dispatches
        .iter()
        .rev()
        .filter(|attempt| attempt.task == task.id)
        .collect();
    let head = match named_head {
        Some(head) => head,
        None => attempts
            .iter()
            .find_map(|attempt| attempt.source.as_deref().and_then(commit_named))
            .ok_or_else(|| {
                format!(
                    "task {task_id} has no handed-in commit — no attempt's worker_done named a \
                     head; say which with --head"
                )
            })?,
    };
    let checkout = attempts
        .iter()
        .find_map(|attempt| {
            run.worker(&attempt.worker)
                .and_then(|worker| worker.checkout.clone())
        })
        .ok_or_else(|| {
            format!(
                "task {task_id} has no checkout written down — no worker of it was seated in \
                 one, so there is no repository to merge in"
            )
        })?;
    Ok(LandCheckAsk::Merge {
        run: run.id.clone(),
        task: task.id.clone(),
        head,
        checkout,
        prepare: words.has("--prepare"),
    })
}

/// The one line an agent reads for a letter: `land-check t-1: passed · tree abc1234 · rc 0 ·
/// 1.2 s · log <path>`. Built from the evidence the window handed in, so the line and the payload
/// cannot disagree.
fn line_of(task: &str, evidence: &serde_json::Map<String, serde_json::Value>) -> String {
    let state = evidence
        .get("state")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let mut parts = vec![format!("{LAND_CHECK_VERB} {task}: {state}")];
    if let Some(conflicts) = evidence.get("conflicts") {
        let total = conflicts["total"].as_u64().unwrap_or(0);
        let files: Vec<&str> = conflicts["files"]
            .as_array()
            .map(|files| files.iter().filter_map(serde_json::Value::as_str).collect())
            .unwrap_or_default();
        let noun = if total == 1 { "file" } else { "files" };
        parts.push(format!("{total} clashing {noun}: {}", files.join(", ")));
    }
    if let Some(tree) = evidence.get("tree").and_then(serde_json::Value::as_str) {
        parts.push(format!("tree {}", tree.chars().take(7).collect::<String>()));
    }
    if let Some(rc) = evidence.get("rc").and_then(serde_json::Value::as_i64) {
        parts.push(format!("rc {rc}"));
    }
    if let Some(took) = evidence.get("tookMs").and_then(serde_json::Value::as_i64) {
        parts.push(format!("{}.{} s", took / 1000, (took % 1000) / 100));
    }
    if let Some(log) = evidence.get("log").and_then(serde_json::Value::as_str) {
        parts.push(format!("log {log}"));
    }
    parts.join(" · ")
}

impl Ledger {
    /// One check's evidence, in the ledger's own voice, to the coordinator of the run that holds
    /// the task — once per check id, however many times the window asks. Answers the row's id,
    /// or `None` when the row already stands.
    pub fn land_checked(
        &mut self,
        receipt: &LandCheckReceipt,
        now_ms: i64,
    ) -> Result<Option<String>, String> {
        if !is_check_id(&receipt.check) {
            return Err(format!("{:?} is not a check id", receipt.check));
        }
        let mut evidence = match serde_json::from_str::<serde_json::Value>(&receipt.evidence) {
            Ok(serde_json::Value::Object(evidence)) => evidence,
            _ => return Err("a check's evidence is one JSON object".to_string()),
        };
        if !evidence
            .get("state")
            .is_some_and(serde_json::Value::is_string)
        {
            return Err("a check's evidence says its state".to_string());
        }
        let run = self
            .run(&receipt.run)
            .ok_or_else(|| unknown_run(&receipt.run))?;
        if run.task(&receipt.task).is_none() {
            return Err(format!("unknown task: {}", receipt.task));
        }
        let stands = run.messages.iter().any(|row| {
            row.kind == MessageKind::LandCheck
                && serde_json::from_str::<serde_json::Value>(row.payload.as_str())
                    .ok()
                    .and_then(|body| body["check"].as_str().map(str::to_string))
                    .as_deref()
                    == Some(receipt.check.as_str())
        });
        if stands {
            return Ok(None);
        }
        evidence.insert("check".into(), receipt.check.clone().into());
        evidence.insert("task".into(), receipt.task.clone().into());
        let body = line_of(&receipt.task, &evidence);
        let draft = Draft {
            from: LEDGER_ITSELF.to_string(),
            to: run.address(),
            kind: MessageKind::LandCheck,
            body: body.into(),
            subject: Text::default(),
            priority: Priority::Normal,
            payload: serde_json::Value::Object(evidence).to_string().into(),
            thread: None,
            task: Some(receipt.task.clone()),
            dispatch: None,
        };
        let run_id = receipt.run.clone();
        self.post(&run_id, draft, now_ms).map(Some)
    }
}
