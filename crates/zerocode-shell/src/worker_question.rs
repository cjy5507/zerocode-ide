//! A worker's own question goes to its run's coordinator, not to the person
//! (t-82296).
//!
//! A pane the ledger summoned is somebody's worker, and the somebody is the
//! coordinator of its run. When its CLI stops on an `AskUserQuestion`, the
//! person's 「당신 차례」 is the wrong door: nobody may be watching that
//! screen, and the coordinator who briefed the worker is the one who knows
//! the answer. So the question is filed in the ledger as the worker's own
//! `question` letter to its coordinator — the body and the choices word for
//! word, with the pane and the dispatch beside them — and the person's
//! screen does not draw it.
//!
//! The coordinator answers the way every question is answered, `reply
//! --to-message <id>`, with a choice's number or free words. The beat reads
//! that answer from the ledger and types it into the pane's picker through
//! the same answer door a card's answer takes
//! ([`crate::cmd::terminal::answer_ask`]) — no new verb, no new key road.
//!
//! The person is asked after all in three cases only: the run's coordinator
//! seat is empty, the person has taken the pane, or the coordinator has not
//! answered within [`COORDINATOR_ANSWER_WAIT_MS`]. The last two that arrive
//! while the question waits say so on the card (`ask:raised`).
//!
//! A question the pane moved past before the answer came is refused back to
//! the coordinator as 「지나간 질문」 and leaves nothing on the person's
//! screen: the moving past is the hook's word (a report that is no longer the
//! asking), or the answer door's, which reads the screen and refuses with
//! [`crate::answer_door::QUESTION_CHANGED`] — the judgement t-82287 made
//! for the person's own answers.
//!
//! A permission request is not routed: the automatic approval rules read it
//! first and what they leave is the person's, as before.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

use tauri::{AppHandle, Emitter};
use zerocode_core::ask::{AskPrompt, AskSelection};

use crate::agent_teams::Host;
use crate::hooks::PaneHookReport;

/// How long a coordinator is given to answer its worker's question before
/// the person is asked instead.
pub(crate) const COORDINATOR_ANSWER_WAIT_MS: i64 = 10 * 60 * 1000;

/// The window event that tells the page a worker's question reached the
/// person after all, and why: `{ term, why }` (t-82296).
pub(crate) const RAISED_EVENT: &str = "ask:raised";

/// Where the run's side of a worker's pane stands, read from the ledger
/// ([`crate::orchestration::worker_ask_seat`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AskSeat {
    pub(crate) run: String,
    pub(crate) worker: String,
    pub(crate) team: String,
    pub(crate) pane: String,
    /// The run's coordinator address, the one a worker's letter goes to.
    pub(crate) coordinator: String,
    /// Whether somebody sits in the run's coordinator seat now.
    pub(crate) coordinator_live: bool,
    /// Whether the person has taken the pane.
    pub(crate) taken: bool,
    /// The dispatch the worker is on, named in the letter.
    pub(crate) dispatch: Option<String>,
}

/// Why a worker's question is on the person's screen after all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Raised {
    /// The coordinator did not answer within [`COORDINATOR_ANSWER_WAIT_MS`].
    CoordinatorSilent,
    /// The run's coordinator seat emptied while the question waited.
    CoordinatorGone,
    /// The person took the pane while the question waited.
    TakenOver,
    /// The ledger would not file the letter.
    Unsent,
}

/// Where a question a pane just asked goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    /// To the run's coordinator, as the worker's `question` letter.
    Coordinator,
    /// To the person's 「당신 차례」, as before: no run's worker sits here,
    /// the run's coordinator seat is empty, or the person has the pane.
    Person,
}

/// The road of a question a pane just asked: a worker's goes to its
/// coordinator while somebody sits there and the person has not taken the
/// pane.
pub(crate) fn route(seat: Option<&AskSeat>) -> Route {
    match seat {
        Some(seat) if seat.coordinator_live && !seat.taken => Route::Coordinator,
        _ => Route::Person,
    }
}

/// What the ledger says of a question filed for a worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Standing {
    /// No answer yet.
    Waiting,
    /// The coordinator answered with these words.
    Answered(String),
    /// The question can no longer be answered, or is gone from the ledger.
    Closed,
}

/// What one beat does with one filed question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Step {
    /// Nothing yet.
    Wait,
    /// Type the coordinator's answer into the pane's picker.
    Answer(Vec<AskSelection>),
    /// Tell the coordinator the pane moved past the question, and forget it.
    RefusePassed,
    /// Put the question on the person's screen, saying why.
    Raise(Raised),
    /// Forget the question: there is nobody left to answer it or to tell.
    Forget,
}

/// The beat's judgement on one question already filed with the coordinator.
///
/// `passed` is the hook's word that the pane is no longer asking. `seat` is
/// the run as the ledger holds it now. `raised` is whether the person already
/// holds the question, so it is not raised twice — an answer the coordinator
/// gives after that is still typed, and the door decides whether the picker
/// still waits for it.
pub(crate) fn step(
    prompt: &AskPrompt,
    standing: &Standing,
    passed: bool,
    seat: Option<&AskSeat>,
    raised: bool,
    filed_ms: i64,
    now_ms: i64,
) -> Step {
    match standing {
        Standing::Closed => return Step::Forget,
        Standing::Answered(_) if passed => return Step::RefusePassed,
        Standing::Answered(words) => {
            return match selections_from_reply(prompt, words) {
                Some(selections) => Step::Answer(selections),
                None if raised => Step::Wait,
                None => Step::Raise(Raised::CoordinatorSilent),
            };
        }
        Standing::Waiting if passed => return Step::Wait,
        Standing::Waiting => {}
    }
    if raised {
        return Step::Wait;
    }
    match seat {
        None => Step::Raise(Raised::CoordinatorGone),
        Some(seat) if !seat.coordinator_live => Step::Raise(Raised::CoordinatorGone),
        Some(seat) if seat.taken => Step::Raise(Raised::TakenOver),
        Some(_) if now_ms.saturating_sub(filed_ms) >= COORDINATOR_ANSWER_WAIT_MS => {
            Step::Raise(Raised::CoordinatorSilent)
        }
        Some(_) => Step::Wait,
    }
}

/// The letter a worker's question is filed as: its subject and body.
///
/// The question and every choice travel word for word, numbered as the
/// answer names them, with the pane and the dispatch it is on, and one line
/// on how an answer is read ([`selections_from_reply`]).
pub(crate) fn letter(prompt: &AskPrompt, seat: &AskSeat, term: u32) -> (String, String) {
    let subject = prompt
        .questions
        .first()
        .map(|question| {
            question
                .header
                .clone()
                .filter(|header| !header.trim().is_empty())
                .unwrap_or_else(|| question.question.clone())
        })
        .unwrap_or_default();
    let mut body = format!(
        "워커 {}의 판(term-{term}{})이 묻습니다 — 사람 화면에는 올리지 않았습니다.\n",
        seat.worker,
        seat.dispatch
            .as_deref()
            .map(|id| format!(", {id}"))
            .unwrap_or_default(),
    );
    let several = prompt.questions.len() > 1;
    for (at, question) in prompt.questions.iter().enumerate() {
        body.push('\n');
        if several {
            body.push_str(&format!("{}) ", at + 1));
        }
        if let Some(header) = question
            .header
            .as_deref()
            .filter(|one| !one.trim().is_empty())
        {
            body.push_str(&format!("[{header}] "));
        }
        body.push_str(&question.question);
        if question.multi_select {
            body.push_str(" (여러 개 고를 수 있음)");
        }
        body.push('\n');
        for (row, option) in question.options.iter().enumerate() {
            body.push_str(&format!("  {}. {}", row + 1, option.label));
            if let Some(description) = option.description.as_deref() {
                body.push_str(&format!(" — {description}"));
            }
            body.push('\n');
        }
    }
    body.push_str(&format!(
        "\n답: reply --to-message 로 고른 번호(여러 개면 1,3) 또는 자유 글{}. {}분 안에 답이 없으면 사람에게 올립니다.",
        if several { "을 질문마다 한 줄에" } else { "" },
        COORDINATOR_ANSWER_WAIT_MS / 60_000,
    ));
    (subject, body)
}

/// The coordinator's reply as the picks a card would have sent.
///
/// One question takes the whole reply; several take one line each, in
/// order. A line that is only choice numbers inside the question's range —
/// one for a single-select, any for a multi-select — picks those rows;
/// anything else is the question's free answer. `None` when no question got
/// an answer at all.
pub(crate) fn selections_from_reply(prompt: &AskPrompt, reply: &str) -> Option<Vec<AskSelection>> {
    let reply = reply.trim();
    let lines: Vec<&str> = match prompt.questions.len() {
        0 => return None,
        1 => vec![reply],
        _ => reply.lines().map(str::trim).collect(),
    };
    let selections: Vec<AskSelection> = prompt
        .questions
        .iter()
        .enumerate()
        .map(|(at, question)| {
            let words = lines.get(at).copied().unwrap_or_default();
            match picked_rows(words, question.options.len(), question.multi_select) {
                Some(indices) => AskSelection {
                    indices,
                    other: String::new(),
                },
                None => AskSelection {
                    indices: Vec::new(),
                    other: words.to_string(),
                },
            }
        })
        .collect();
    zerocode_core::ask::has_answer(prompt, &selections).then_some(selections)
}

/// The rows `words` names when it is only choice numbers, 1-based, all in
/// `1..=rows`, and one of them unless `several` — as 0-based indices.
fn picked_rows(words: &str, rows: usize, several: bool) -> Option<Vec<usize>> {
    let numbers: Vec<&str> = words
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|one| !one.is_empty())
        .collect();
    if numbers.is_empty() || (!several && numbers.len() > 1) {
        return None;
    }
    let mut picked = Vec::with_capacity(numbers.len());
    for number in numbers {
        let row = number.parse::<usize>().ok()?;
        if row == 0 || row > rows {
            return None;
        }
        if !picked.contains(&(row - 1)) {
            picked.push(row - 1);
        }
    }
    Some(picked)
}

/// The words the coordinator is told when the pane moved past its question.
pub(crate) fn passed_letter(worker: &str, term: u32, question: &str) -> String {
    format!(
        "지나간 질문 — 워커 {worker}의 판(term-{term})은 질문 {question}을 이미 지나갔습니다. 답은 판에 넣지 않았습니다."
    )
}

/// The words the coordinator is told when its answer could not be typed.
pub(crate) fn unanswered_letter(worker: &str, term: u32, question: &str, why: &str) -> String {
    format!("질문 {question}의 답을 워커 {worker}의 판(term-{term})에 넣지 못했습니다 — {why}")
}

/// Where one question the window carries for a worker stands.
#[derive(Debug, Clone)]
enum Filing {
    /// Routed, not yet in the ledger.
    Unfiled,
    /// In the ledger as this message.
    Filed(String),
}

/// One worker's question the window carries to its coordinator.
#[derive(Debug, Clone)]
struct Asked {
    /// The question as the hook reported it, to compare the next report to.
    signature: String,
    prompt: AskPrompt,
    /// The asking report itself, sent to the page again when the person is
    /// asked after all.
    report: PaneHookReport,
    seat: AskSeat,
    filed: Filing,
    filed_ms: i64,
    passed: bool,
    raised: Option<Raised>,
}

fn asked() -> MutexGuard<'static, HashMap<u32, Asked>> {
    static ASKED: OnceLock<Mutex<HashMap<u32, Asked>>> = OnceLock::new();
    ASKED
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|held| held.into_inner())
}

fn signature(prompt: &AskPrompt) -> String {
    serde_json::to_string(prompt).unwrap_or_default()
}

/// Whether `report` is a pane asking an `AskUserQuestion` — not a permission,
/// and not a wait that described no question.
fn asking(report: &PaneHookReport) -> Option<&AskPrompt> {
    (report.state == zerocode_core::hook::HookState::NeedsAttention && report.approval.is_none())
        .then_some(report.ask_prompt.as_ref())
        .flatten()
}

/// Hear one hook report, and answer what the page is shown of it.
///
/// A worker's question routed to its coordinator is shown as the work it is
/// — the pane is mid-turn, waiting on its coordinator's answer — with no
/// question on it, so no card, no 「당신 차례」 row and no ring. Everything
/// else is shown as reported.
pub(crate) fn heard(report: &PaneHookReport, now_ms: i64) -> PaneHookReport {
    let term = report.term;
    if let Some(prompt) = asking(report) {
        let sig = signature(prompt);
        let mut held = asked();
        if let Some(standing) = held.get_mut(&term)
            && standing.signature == sig
        {
            standing.passed = false;
            return match standing.raised {
                Some(_) => report.clone(),
                None => quiet(report),
            };
        }
        held.remove(&term);
        drop(held);
        let seat = crate::orchestration::worker_ask_seat(term);
        if route(seat.as_ref()) == Route::Person {
            return report.clone();
        }
        let Some(seat) = seat else {
            return report.clone();
        };
        asked().insert(
            term,
            Asked {
                signature: sig,
                prompt: prompt.clone(),
                report: report.clone(),
                seat,
                filed: Filing::Unfiled,
                filed_ms: now_ms,
                passed: false,
                raised: None,
            },
        );
        return quiet(report);
    }
    let mut held = asked();
    if let Some(standing) = held.get_mut(&term) {
        // A wait that described no question (a notification while the picker
        // stands) is not the pane moving on; anything else is.
        if report.state == zerocode_core::hook::HookState::NeedsAttention
            && report.approval.is_none()
        {
            return match standing.raised {
                Some(_) => report.clone(),
                None => quiet(report),
            };
        }
        standing.passed = true;
        if matches!(standing.filed, Filing::Unfiled) {
            held.remove(&term);
        }
    }
    report.clone()
}

/// The report as the page is shown it while the coordinator holds the
/// question: the pane at work, asking nothing.
fn quiet(report: &PaneHookReport) -> PaneHookReport {
    let mut shown = report.clone();
    shown.state = zerocode_core::hook::HookState::Working;
    shown.ask = None;
    shown.ask_prompt = None;
    shown.submit_shape = zerocode_core::ask::SubmitShape::NotAQuestion;
    shown
}

/// Whether the person's screen keeps `term`'s question off: its worker's
/// coordinator holds it, and has not given it back to the person.
pub(crate) fn hidden(term: u32) -> bool {
    asked()
        .get(&term)
        .is_some_and(|standing| !standing.passed && standing.raised.is_none())
}

/// The pane is gone; whatever it asked goes with it.
pub(crate) fn forget(term: u32) {
    asked().remove(&term);
}

/// One beat over every question the window carries for a worker: file what
/// is not filed yet, type what the coordinator answered, refuse what the pane
/// moved past, and give the person what nobody answers.
pub(crate) fn sweep(app: &AppHandle, host: &dyn Host, now_ms: i64) {
    let terms: Vec<u32> = asked().keys().copied().collect();
    for term in terms {
        let Some(standing) = asked().get(&term).cloned() else {
            continue;
        };
        match &standing.filed {
            Filing::Unfiled => file(app, host, term, &standing, now_ms),
            Filing::Filed(question) => carry(app, term, &standing, question, now_ms),
        }
    }
}

/// File the worker's letter from the worker's own seat, through the one
/// road a typed `send` takes.
fn file(app: &AppHandle, host: &dyn Host, term: u32, standing: &Asked, now_ms: i64) {
    let seat = &standing.seat;
    let (subject, body) = letter(&standing.prompt, seat, term);
    let request = format!("worker-question-{}-{}", seat.worker, standing.filed_ms);
    let filed =
        crate::agent_teams::current_pane_capability(&seat.team, &seat.pane).and_then(|token| {
            let argv: Vec<String> = [
                "send",
                "--to",
                seat.coordinator.as_str(),
                "--type",
                "question",
                "--subject",
                subject.as_str(),
                "--body",
                body.as_str(),
                "--retry-request",
                request.as_str(),
            ]
            .iter()
            .map(|word| (*word).to_string())
            .collect();
            let answer = crate::orchestration::run(
                host,
                Vec::new(),
                &seat.team,
                &seat.pane,
                &token,
                &argv,
                now_ms,
            );
            (answer.exit_code == 0)
                .then(|| serde_json::from_str::<serde_json::Value>(answer.stdout.trim()).ok())
                .flatten()
                .and_then(|said| said["messageId"].as_str().map(str::to_string))
        });
    let mut held = asked();
    let Some(now) = held.get_mut(&term) else {
        return;
    };
    match filed {
        Some(question) => {
            now.filed = Filing::Filed(question);
            now.filed_ms = now_ms;
        }
        None => {
            let report = now.report.clone();
            let passed = now.passed;
            held.remove(&term);
            drop(held);
            if !passed {
                raise(app, term, &report, Raised::Unsent);
            }
        }
    }
}

/// One beat for a question in the ledger.
fn carry(app: &AppHandle, term: u32, standing: &Asked, question: &str, now_ms: i64) {
    let seat_now = crate::orchestration::worker_ask_seat(term);
    let ledger = crate::orchestration::question_standing(&standing.seat.run, question);
    let next = step(
        &standing.prompt,
        &ledger,
        standing.passed,
        seat_now.as_ref(),
        standing.raised.is_some(),
        standing.filed_ms,
        now_ms,
    );
    let seat = &standing.seat;
    match next {
        Step::Wait => {}
        Step::Forget => forget(term),
        Step::RefusePassed => {
            tell(
                seat,
                question,
                &passed_letter(&seat.worker, term, question),
                now_ms,
            );
            forget(term);
        }
        Step::Raise(why) => {
            if let Some(now) = asked().get_mut(&term) {
                now.raised = Some(why);
            }
            raise(app, term, &standing.report, why);
        }
        Step::Answer(selections) => {
            match crate::cmd::terminal::answer_ask(
                app.clone(),
                term,
                standing.report.agent.slug().to_string(),
                standing.prompt.clone(),
                selections,
            ) {
                Ok(()) => forget(term),
                Err(why) if why == crate::answer_door::ANSWER_IN_FLIGHT => {}
                Err(why) if why == crate::answer_door::QUESTION_CHANGED => {
                    tell(
                        seat,
                        question,
                        &passed_letter(&seat.worker, term, question),
                        now_ms,
                    );
                    forget(term);
                }
                Err(why) => {
                    tell(
                        seat,
                        question,
                        &unanswered_letter(&seat.worker, term, question, &why),
                        now_ms,
                    );
                    forget(term);
                }
            }
        }
    }
}

/// One letter from the ledger to the run's coordinator about `question`.
fn tell(seat: &AskSeat, question: &str, body: &str, now_ms: i64) {
    crate::orchestration::post_observation_once(
        Some(seat.run.as_str()),
        &seat.coordinator,
        body,
        &format!("worker-question-told-{question}"),
        now_ms,
    );
}

/// Give the person the question after all: why first, so the card that the
/// report draws says it, then the asking report itself.
fn raise(app: &AppHandle, term: u32, report: &PaneHookReport, why: Raised) {
    let _ = app.emit(
        RAISED_EVENT,
        serde_json::json!({ "term": term, "why": why }),
    );
    let _ = app.emit("hook:agent", report.clone());
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_core::ask::{AskOption, AskQuestion};

    const NOW: i64 = 1_800_000_000_000;

    fn prompt(multi: bool) -> AskPrompt {
        AskPrompt {
            questions: vec![AskQuestion {
                question: "cargo 시험 다음 단계로 넘어갈까요?".to_string(),
                header: Some("다음 단계".to_string()),
                multi_select: multi,
                options: vec![
                    AskOption {
                        label: "진행".to_string(),
                        description: Some("clippy까지 돌림".to_string()),
                    },
                    AskOption {
                        label: "멈춤".to_string(),
                        description: None,
                    },
                    AskOption {
                        label: "다시".to_string(),
                        description: None,
                    },
                ],
            }],
        }
    }

    fn seat(coordinator_live: bool, taken: bool) -> AskSeat {
        AskSeat {
            run: "run-1".to_string(),
            worker: "w-2".to_string(),
            team: "team-3".to_string(),
            pane: "%4".to_string(),
            coordinator: "run:run-1".to_string(),
            coordinator_live,
            taken,
            dispatch: Some("dp-9".to_string()),
        }
    }

    #[test]
    fn a_workers_question_goes_to_its_coordinator_while_somebody_sits_there() {
        assert_eq!(route(Some(&seat(true, false))), Route::Coordinator);
    }

    #[test]
    fn a_question_with_no_coordinator_or_no_worker_or_a_taken_pane_is_the_persons() {
        assert_eq!(route(Some(&seat(false, false))), Route::Person);
        assert_eq!(route(Some(&seat(true, true))), Route::Person);
        assert_eq!(route(None), Route::Person);
    }

    #[test]
    fn a_question_the_pane_moved_past_is_refused_back_to_the_coordinator_and_never_raised() {
        let asked = prompt(false);
        let answered = Standing::Answered("1".to_string());
        assert_eq!(
            step(
                &asked,
                &answered,
                true,
                Some(&seat(true, false)),
                false,
                NOW,
                NOW
            ),
            Step::RefusePassed
        );
        // Past the coordinator's time, a passed question still goes to nobody.
        let late = NOW + COORDINATOR_ANSWER_WAIT_MS * 3;
        assert_eq!(
            step(
                &asked,
                &Standing::Waiting,
                true,
                Some(&seat(true, false)),
                false,
                NOW,
                late
            ),
            Step::Wait
        );
        assert_eq!(
            step(
                &asked,
                &Standing::Waiting,
                true,
                Some(&seat(false, false)),
                false,
                NOW,
                late
            ),
            Step::Wait
        );
    }

    #[test]
    fn a_coordinator_silent_for_ten_minutes_gives_the_question_to_the_person() {
        let asked = prompt(false);
        let live = seat(true, false);
        let almost = NOW + COORDINATOR_ANSWER_WAIT_MS - 1;
        assert_eq!(
            step(
                &asked,
                &Standing::Waiting,
                false,
                Some(&live),
                false,
                NOW,
                almost
            ),
            Step::Wait
        );
        assert_eq!(
            step(
                &asked,
                &Standing::Waiting,
                false,
                Some(&live),
                false,
                NOW,
                NOW + COORDINATOR_ANSWER_WAIT_MS
            ),
            Step::Raise(Raised::CoordinatorSilent)
        );
        // Once, not on every beat after.
        assert_eq!(
            step(
                &asked,
                &Standing::Waiting,
                false,
                Some(&live),
                true,
                NOW,
                NOW + COORDINATOR_ANSWER_WAIT_MS * 2
            ),
            Step::Wait
        );
        assert_eq!(COORDINATOR_ANSWER_WAIT_MS, 600_000);
    }

    #[test]
    fn a_seat_that_empties_or_a_pane_the_person_takes_raises_the_question_at_once() {
        let asked = prompt(false);
        assert_eq!(
            step(
                &asked,
                &Standing::Waiting,
                false,
                Some(&seat(false, false)),
                false,
                NOW,
                NOW
            ),
            Step::Raise(Raised::CoordinatorGone)
        );
        assert_eq!(
            step(&asked, &Standing::Waiting, false, None, false, NOW, NOW),
            Step::Raise(Raised::CoordinatorGone)
        );
        assert_eq!(
            step(
                &asked,
                &Standing::Waiting,
                false,
                Some(&seat(true, true)),
                false,
                NOW,
                NOW
            ),
            Step::Raise(Raised::TakenOver)
        );
    }

    #[test]
    fn the_coordinators_answer_is_typed_as_the_picks_a_card_would_send() {
        let asked = prompt(false);
        assert_eq!(
            step(
                &asked,
                &Standing::Answered("2".to_string()),
                false,
                Some(&seat(true, false)),
                false,
                NOW,
                NOW
            ),
            Step::Answer(vec![AskSelection {
                indices: vec![1],
                other: String::new(),
            }])
        );
        // Raised to the person and then answered by the coordinator: the door
        // decides who was first.
        assert!(matches!(
            step(
                &asked,
                &Standing::Answered("멈추고 보고해".to_string()),
                false,
                Some(&seat(true, false)),
                true,
                NOW,
                NOW
            ),
            Step::Answer(_)
        ));
        assert_eq!(
            step(
                &asked,
                &Standing::Closed,
                false,
                Some(&seat(true, false)),
                false,
                NOW,
                NOW
            ),
            Step::Forget
        );
    }

    #[test]
    fn a_reply_reads_as_choice_numbers_or_as_free_words() {
        let single = prompt(false);
        let picked = |reply: &str, prompt: &AskPrompt| selections_from_reply(prompt, reply);
        assert_eq!(picked(" 3 ", &single).unwrap()[0].indices, vec![2]);
        // Two numbers on a single-select, a number out of range, or words are
        // the free answer, word for word.
        for words in ["1,2", "4", "0", "2번으로 하되 clippy 먼저"] {
            let selection = &picked(words, &single).unwrap()[0];
            assert!(selection.indices.is_empty(), "{words}");
            assert_eq!(selection.other, words);
        }
        let multi = prompt(true);
        assert_eq!(picked("1, 3", &multi).unwrap()[0].indices, vec![0, 2]);
        assert_eq!(picked("", &single), None);
    }

    #[test]
    fn several_questions_take_one_line_each() {
        let mut both = prompt(false);
        let mut second = both.questions[0].clone();
        second.question = "보고서는 어디에?".to_string();
        both.questions.push(second);
        let selections = selections_from_reply(&both, "1\nreviews 폴더").unwrap();
        assert_eq!(selections[0].indices, vec![0]);
        assert_eq!(selections[1].other, "reviews 폴더");
    }

    #[test]
    fn the_letter_carries_the_question_and_every_choice_word_for_word_with_the_pane() {
        let (subject, body) = letter(&prompt(false), &seat(true, false), 7);
        assert_eq!(subject, "다음 단계");
        assert!(body.contains("w-2"), "{body}");
        assert!(body.contains("term-7"), "{body}");
        assert!(body.contains("dp-9"), "{body}");
        assert!(
            body.contains("cargo 시험 다음 단계로 넘어갈까요?"),
            "{body}"
        );
        assert!(body.contains("  1. 진행 — clippy까지 돌림\n"), "{body}");
        assert!(body.contains("  2. 멈춤\n"), "{body}");
        assert!(body.contains("  3. 다시\n"), "{body}");
        assert!(body.contains("10분"), "{body}");
    }

    #[test]
    fn the_passed_letter_says_the_question_passed_and_nothing_was_typed() {
        let words = passed_letter("w-2", 7, "m-5");
        assert!(words.starts_with("지나간 질문"), "{words}");
        assert!(words.contains("m-5") && words.contains("term-7"), "{words}");
    }
}
