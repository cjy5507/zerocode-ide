//! `delegate` — 일을 적고, 그 일 위에 워커를 띄우고, 워커에게 제 id를 알리는 첫 편지까지
//! 한 번의 계획으로 한다 (t-34501 4단계).
//!
//! 코디네이터는 지금 `task-create`, `worker-start --task`, 그리고 워커에게 「당신 id」 편지를
//! 보내는 `send`를 손으로 세 번 한다. 워커는 제 workerId를 스스로 알 수 없어서 마지막 편지가
//! 빠지면 큐 일감 이름도 못 짓는다. 이 동사는 셋을 한 계획 안에서 한다.
//!
//! 새 권위를 만들지 않는다. 과업은 `create_task`, 워커와 시도는 `worker-start`의 길 그대로,
//! 편지는 `post_as` 그대로다 — 이 파일은 그 셋을 이어 붙이고 **중간에 멈추면 아무것도
//! 남기지 않는 것**만 맡는다. 좌석·사용 한도·`--on-quota-wall` 판정은 `worker-start`가 그대로
//! 한다.

use super::*;

/// 이 동사의 이름. 표(`VERBS`)와 창의 앞문이 같은 글자를 쓰도록 한 곳에 둔다.
pub const VERB: &str = "delegate";

/// `--wait`의 기다림 앞에 창이 더 쓸 수 있는 시간: 판을 열고 워커가 브리핑을 받는 준비.
///
/// 창은 준비가 끝난 *뒤에* 기다림 시계를 재므로(`strip_wait`가 안쪽 `worker-start`에서
/// `--timeout-ms`를 빼서 준비 기한은 기본값이다), `--wait --timeout-ms X`의 답은 늦어도
/// 준비 기본값 + X 뒤에 나온다. 다리와 두 심(POSIX·PowerShell)의 기한이 같은 수에 얹도록
/// 한 곳에 둔다.
pub const WAIT_HEADROOM_MS: u32 = READY_TIMEOUT_DEFAULT_MS;

/// 이 동사가 스스로 읽는 낱말. 나머지는 모두 `worker-start`에 그대로 넘어간다.
///
/// `--wait`는 창이 워커의 끝을 기다릴 때만 읽는다(계획은 읽지 않는다).
const OWN_FLAGS: &[&str] = &["--spec", "--title", "--deps", "--parent", "--wait"];

/// 새 일을 위임하는 동사에는 맞지 않는 낱말과 그 까닭.
const REFUSED: &[(&str, &str)] = &[
    (
        "--task",
        "delegate writes the task itself — use worker-start to put a task that already exists on a worker",
    ),
    (
        "--retry-of",
        "a replacement follows an ended attempt of an existing task — use worker-start",
    ),
    (
        "--inherit-checkout",
        "it belongs to a replacement for an ended attempt — use worker-start",
    ),
    ("--bare", "a bare pane carries no task — use worker-start"),
    (
        "--on",
        "a federated worker's ids are the home ledger's — use task-create and worker-start --on",
    ),
];

/// `--wait`와, 그때의 `--timeout-ms`(준비 기한이 아니라 기다림의 길이)는 요청의 내용이 아니다.
/// 재시도 지문에서 빼야 「기다리지 않고 불렀다가 기다리며 다시 부르는」 재시도가 다른 요청으로
/// 거절되지 않는다. `--wait` 없이 준 `--timeout-ms`는 준비 기한이므로 그대로 남는다.
pub(super) fn strip_wait(words: &mut Words) {
    if words.has("--wait") {
        words.flags.retain(|held| held != "--wait");
        words.values.retain(|(name, _)| name != "--timeout-ms");
    }
}

/// 워커가 첫 편지에서 읽는 글. 제 id와, 그 id로 무엇을 하는지만 적는다 — 일 자체는
/// 브리핑이 이미 말했다.
fn letter(worker: &str, dispatch: &str, task: &str, title: &str, pane: &str) -> String {
    let name = if title.is_empty() { task } else { title };
    format!(
        "You are worker {worker}, on dispatch {dispatch} for task {task} ({name}), in pane {pane}. \
         Use these ids when you name your own files, jobs and questions. \
         `zerocode-orc check` reads your mail and `zerocode-orc help` lists the verbs."
    )
}

/// 일을 적고 워커를 띄우고 편지를 쓴다. 중간에 거절되면 적은 일을 거둔다.
#[allow(clippy::too_many_arguments)]
pub(super) fn plan(
    ledger: &mut Ledger,
    team: &mut Team,
    launcher: &dyn Launcher,
    words: &Words,
    pane: &str,
    now_ms: i64,
    actor: &str,
    caller: &str,
    seat: &str,
) -> Result<Decided, String> {
    let run_id = bound(ledger, words, caller, seat)?;
    let spec = words
        .value("--spec")
        .map(str::trim)
        .filter(|held| !held.is_empty())
        .ok_or("delegate needs --spec — the work, written so a worker who has seen nothing else can do it")?;
    if words
        .value("--agent")
        .filter(|held| !held.is_empty())
        .is_none()
    {
        return Err("delegate needs --agent".to_string());
    }
    if !words.positional.is_empty() {
        return Err("delegate does not accept positional arguments".to_string());
    }
    for (flag, why) in REFUSED {
        if words.value(flag).is_some() || words.has(flag) {
            return Err(format!("{flag} does not go with delegate — {why}"));
        }
    }

    // 일을 먼저 적는다: `worker-start`가 이 과업을 집어 가야 하기 때문이다. 아래 어느 거절에서든
    // 이 줄은 거둔다.
    let title = words.value("--title").unwrap_or_default().to_string();
    let task = ledger.create_task(
        &run_id,
        spec.to_string(),
        title.clone(),
        words.list("--deps"),
        words.value("--parent").map(str::to_string),
        now_ms,
    )?;

    // 워커의 첫 말은 `--prompt`다. 따로 주지 않으면 일의 설명이 곧 첫 말이다 — 과업 번호만으로는
    // 빈 입력칸이 뜬다.
    let mut inner: Vec<String> = vec![
        "worker-start".to_string(),
        "--task".to_string(),
        task.clone(),
    ];
    if words.value("--prompt").is_none() {
        inner.push("--prompt".to_string());
        inner.push(spec.to_string());
    }
    for (name, value) in &words.values {
        if !OWN_FLAGS.contains(&name.as_str()) {
            inner.push(name.clone());
            inner.push(value.clone());
        }
    }
    for flag in &words.flags {
        if !OWN_FLAGS.contains(&flag.as_str()) {
            inner.push(flag.clone());
        }
    }

    let started = plan_inner(ledger, team, launcher, &inner, pane, now_ms, Some(actor));
    let mut decided = match started {
        Ok(decided) if decided.reply.exit_code == 0 => decided,
        Ok(refusal) => {
            withdraw(ledger, &run_id, &task);
            return Ok(refusal);
        }
        Err(why) => {
            withdraw(ledger, &run_id, &task);
            return Err(why);
        }
    };

    let mut reply: serde_json::Value = serde_json::from_str(decided.reply.stdout.trim())
        .map_err(|why| format!("worker-start answered something delegate cannot read: {why}"))?;
    let worker = reply["workerId"].as_str().unwrap_or_default().to_string();
    let dispatch = reply["dispatchId"].as_str().unwrap_or_default().to_string();
    let worker_pane = reply["pane"].as_str().unwrap_or_default().to_string();
    // 편지가 안 써져도 위임은 이미 섰다 — 거둘 수 있는 것은 위에서 끝났고, 워커는 떠 있다.
    // 실패를 숨기지 않고 답에 적어, 코디네이터가 손으로 보내게 한다.
    let draft = Draft {
        from: sender(ledger, &team.leader_pane, &run_id, (&team.id, pane)),
        to: worker_address(&worker),
        kind: MessageKind::Status,
        body: letter(&worker, &dispatch, &task, &title, &worker_pane).into(),
        subject: Text::default(),
        priority: Priority::Normal,
        payload: Text::default(),
        thread: None,
        task: Some(task.clone()),
        dispatch: Some(dispatch.clone()),
    };
    match ledger.post_as(&run_id, draft, Some(seat), now_ms) {
        Ok(letter_id) => reply["letterId"] = serde_json::json!(letter_id),
        Err(why) => {
            reply["letterId"] = serde_json::Value::Null;
            reply["letterError"] = serde_json::json!(why);
        }
    }
    reply["delegated"] = serde_json::json!(true);
    reply["taskId"] = serde_json::json!(task);
    decided.reply = Reply::ok(format!("{reply}\n"));
    decided.requires_durability = true;
    Ok(decided)
}

/// 방금 적은 일을 거둔다. 아직 아무도 집지 않은(시도가 없는) 마지막 줄일 때만 — 그 밖의 것은
/// 이 호출이 만든 것이 아니다. 번호는 다시 쓰이지 않는다(원장은 번호를 되돌리지 않는다).
fn withdraw(ledger: &mut Ledger, run_id: &str, task: &str) {
    let Some(run) = ledger.run_mut(run_id) else {
        return;
    };
    if run.dispatches.iter().any(|held| held.task == task) {
        return;
    }
    if run.tasks.last().is_some_and(|held| held.id == task) {
        run.tasks.pop();
    }
}

/// 위임한 워커가 어디까지 왔는가. `delegate --wait`가 기다리는 것이 이것이다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// 워커가 `worker_done`을 보냈다. 글은 보낸 그대로다 — 해석하지 않는다.
    Reported { message: String, body: String },
    /// 워커가 코디네이터에게 물었고 아직 답을 받지 못했다.
    Asking { message: String, body: String },
    /// 시도가 보고도 질문도 없이 끝났다(멈춤·죽음·거둠). 과업이 지금 어느 상태인지만 적는다.
    Ended { task_status: &'static str },
}

impl Outcome {
    /// 기다림이 끝나는 까닭을 한 낱말로.
    pub const fn word(&self) -> &'static str {
        match self {
            Self::Reported { .. } => "reported",
            Self::Asking { .. } => "asking",
            Self::Ended { .. } => "ended",
        }
    }
}

/// 한 워커의 한 시도가 지금 어디에 있는가. 아직 아무 말도 없고 시도가 열려 있으면 `None`.
///
/// 편지함을 건드리지 않는다 — 코디네이터의 우편을 대신 가져가거나 읽음 처리하지 않고, 원장의
/// 줄만 읽는다. 그래서 기다리는 동안에도 다른 워커의 편지는 코디네이터에게 그대로 간다.
/// 가장 새로운 것이 이긴다: 질문에 답이 간 뒤 워커가 보고하면 보고다.
#[must_use]
pub fn outcome(run: &Run, worker: &str, dispatch: &str) -> Option<Outcome> {
    let from = worker_address(worker);
    let mut latest: Option<&Message> = None;
    for held in run.messages() {
        if held.from != from || held.dispatch.as_deref() != Some(dispatch) {
            continue;
        }
        match held.kind {
            MessageKind::WorkerDone => latest = Some(held),
            MessageKind::Question if thread_answer(run, held).is_none() => latest = Some(held),
            _ => {}
        }
    }
    if let Some(held) = latest {
        let (message, body) = (held.id.clone(), held.body.as_str().to_string());
        return Some(match held.kind {
            MessageKind::WorkerDone => Outcome::Reported { message, body },
            _ => Outcome::Asking { message, body },
        });
    }
    let attempt = run.dispatch(dispatch)?;
    attempt.ended_ms.map(|_| Outcome::Ended {
        task_status: run
            .task(&attempt.task)
            .map_or("unknown", |held| held.status.as_str()),
    })
}

/// `--wait`가 말하는 기다림의 길이(밀리초). `--wait`가 없으면 `None`.
///
/// `--wait`에 `--timeout-ms`가 없거나 범위 밖이면 거절한다. 얼마나 기다리는지 모르는 위임은
/// 일을 적기 전에 되돌려야 하고, 창이 이 값으로 다리의 기한(`hookd`)과 같은 길이를 잡는다 —
/// `--wait`가 있으면 `--timeout-ms`는 준비 기한이 아니라 기다림의 길이다.
pub fn wait_budget(argv: &[String]) -> Result<Option<u32>, String> {
    let Some((_, rest)) = argv.split_first() else {
        return Ok(None);
    };
    let words = split_words(rest, BOOL_FLAGS);
    if !words.has("--wait") {
        return Ok(None);
    }
    let raw = words.value("--timeout-ms").ok_or(
        "delegate --wait needs --timeout-ms <ms> — how long to wait for the worker, up to ten \
         minutes",
    )?;
    budget_under(raw, WAIT_BUDGET_MAX_MS).map(Some)
}

/// 위임의 답에 기다림이 본 것을 덧붙인다. 처음 위임이 한 답은 그대로이고 `waited`만 는다 —
/// 같은 이름으로 다시 불러 받은 첫 답에도 같은 모양으로 붙는다.
#[must_use]
pub fn with_waited(
    answer: &str,
    outcome: Option<&Outcome>,
    waited_ms: u64,
    budget_ms: u32,
) -> String {
    let Ok(mut said) = serde_json::from_str::<serde_json::Value>(answer.trim()) else {
        return answer.to_string();
    };
    let mut waited = serde_json::json!({
        "outcome": outcome.map_or("timeout", Outcome::word),
        "waitedMs": waited_ms,
        "budgetMs": budget_ms,
    });
    match outcome {
        Some(Outcome::Reported { message, body }) => {
            waited["messageId"] = serde_json::json!(message);
            // 워커가 쓴 글 그대로가 JSON이면 열어 보여 주고, 아니면 글 그대로 둔다.
            let report = serde_json::from_str::<serde_json::Value>(body)
                .unwrap_or_else(|_| serde_json::json!(body));
            for key in ["ok", "summary", "head"] {
                if let Some(held) = report.get(key) {
                    waited[key] = held.clone();
                }
            }
            waited["report"] = report;
        }
        Some(Outcome::Asking { message, body }) => {
            waited["messageId"] = serde_json::json!(message);
            waited["question"] = serde_json::json!(body);
            waited["next"] = serde_json::json!(
                "answer it with `reply --to-message <messageId> --body <answer>`; the worker is waiting"
            );
        }
        Some(Outcome::Ended { task_status }) => {
            waited["taskStatus"] = serde_json::json!(task_status);
            waited["next"] = serde_json::json!(
                "the attempt ended without a report or a question — read `worker-show` for why"
            );
        }
        None => {
            waited["next"] = serde_json::json!(
                "nothing has been heard from the worker yet; it is still working. `check --wait` \
                 reads its report when it comes"
            );
        }
    }
    said["waited"] = waited;
    format!("{said}\n")
}
