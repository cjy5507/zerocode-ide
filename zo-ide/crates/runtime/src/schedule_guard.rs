//! The schedule guard: a schedule zo asks for is approved by a person, one
//! schedule at a time.
//!
//! `CronCreate` writes work that later starts by itself. A prompt cron opens a
//! turn in whatever zo session sits idle in the folder; a workflow cron runs
//! its graph with no turn at all. Nobody watches either. The person decided
//! (2026-11-11) that each such schedule is theirs to approve, so the
//! permission policy puts the call to a person whatever the mode and the rules
//! would do with it ([`crate::PermissionPolicy::authorize_with_context`]): the
//! question offers no lasting allow, a run that decides in a person's absence
//! refuses it, and where nobody can be asked — a pipe, a sub-agent, a
//! workflow, a pane the ledger summoned as a worker — the call is refused with
//! where to ask instead.
//!
//! The card is the guard's own sentence over what the call gave, and what the
//! call gave is shown as data by the one rule every such card follows
//! (`zerocode_core::card_words`): what would run, when — the expression and
//! the next runs it names — and where.
//!
//! What this is not: a boundary. `zo cron ensure` run from a shell writes the
//! same store unasked, and so does an edit of the store's file. The guard
//! closes the asking, not the file.

use std::fmt::Write as _;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use core_types::tool_input::parse_tool_arguments;
use core_types::tool_names::tool_key;
use serde_json::Value;
use zerocode_core::card_words::{shown, EXCERPT_MAX_CHARS, WORD_MAX_CHARS};

use crate::team_cron_registry::{
    cron_zone, next_due_at_after_in_zone, validate_cron_schedule, CronZone,
};

/// The tool that registers a schedule.
pub const TOOL: &str = "CronCreate";

/// How many of the coming runs the card names.
const CARD_RUNS: usize = 3;

/// The card's first line for a prompt: the guard's own words.
const SAYS_OF_A_PROMPT: &str = "A schedule runs by itself: each time it comes due, zo opens a turn here on this prompt, with nobody watching.";

/// The card's first line for a workflow: the guard's own words.
const SAYS_OF_A_WORKFLOW: &str = "A schedule runs by itself: each time it comes due, zo runs this workflow's graph here, with nobody watching and no turn of this session.";

/// Said after every refusal, so the mode advice a denial otherwise carries
/// does not send the model after an escalation that changes nothing — nor to
/// another road into the same store.
const NOT_THE_MODE: &str = "Permission audit: a person approves each schedule in every permission mode, so a wider mode changes nothing; do not retry the identical call, and do not register the schedule another way.";

/// Where to ask instead, said in every refusal: a run with nobody at it gets
/// one or the other.
const SAYS_WHERE_TO_ASK: &str =
    "Where no person is at this run, ask for the schedule in a zo session a person is at.";

/// What a call would register, as it reads.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Registration {
    /// The five-field expression, as the registry reads it.
    schedule: String,
    /// What comes due.
    runs: Runs,
    /// The name the call gave the schedule.
    label: Option<String>,
}

/// What a schedule runs when it comes due.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Runs {
    /// A turn opened on this prompt.
    Prompt(String),
    /// A workflow's graph, under the name it gave when it gave one.
    Workflow(Option<String>),
}

/// The card for the schedule `tool_name` called with `input` would register.
/// `None` for any other tool, and for a call that registers nothing
/// ([`asks`]).
#[must_use]
pub(crate) fn asked(tool_name: &str, input: &str) -> Option<String> {
    let registration = registration(tool_name, input)?;
    let folder = std::env::current_dir().ok();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Some(card(&registration, cron_zone(), now, folder.as_deref()))
}

/// Whether `tool_name` called with `input` would register a schedule, which
/// is a person's to approve. Not so for any other tool, nor for a call the
/// tool itself refuses — an expression that does not read, a call with both a
/// prompt and a workflow or with neither — since that registers nothing.
#[must_use]
pub fn asks(tool_name: &str, input: &str) -> bool {
    registration(tool_name, input).is_some()
}

/// The refusal when no person said yes: the person said no, the question went
/// unanswered, or the run answers in a person's absence. A turn always has a
/// prompter to put the question to, so a headless run and a turn nobody opened
/// end here, not at [`nobody_to_ask`] — and it says where to ask, too.
#[must_use]
pub fn unanswered() -> String {
    format!(
        "refused, since no person said yes to it: a person approves each schedule zo asks for. \
         {SAYS_WHERE_TO_ASK} {NOT_THE_MODE}"
    )
}

/// The refusal where nobody can be asked — a pipe, a sub-agent, a workflow, a
/// pane the ledger summoned as a worker — with where to ask instead.
#[must_use]
pub fn nobody_to_ask() -> String {
    format!(
        "refused, since nobody here can be asked: a person approves each schedule zo asks for. \
         {SAYS_WHERE_TO_ASK} {NOT_THE_MODE}"
    )
}

/// What `tool_name` called with `input` would register. The input is read as
/// the executor reads it (`parse_tool_arguments`, a mended slip included), the
/// expression by the registry's own check, and the choice between a prompt and
/// a workflow as the tool makes it — so a call the tool would run is never one
/// the guard did not see.
fn registration(tool_name: &str, input: &str) -> Option<Registration> {
    if tool_key(tool_name) != tool_key(TOOL) {
        return None;
    }
    let call = parse_tool_arguments(input).ok()?;
    let schedule = call.get("schedule").and_then(Value::as_str)?;
    validate_cron_schedule(schedule).ok()?;
    let prompt = call
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let workflow = call.get("workflow").filter(|workflow| !workflow.is_null());
    let runs = match (workflow, prompt.is_empty()) {
        (Some(workflow), true) => Runs::Workflow(workflow_name(workflow)),
        (None, false) => Runs::Prompt(prompt.to_string()),
        _ => return None,
    };
    Some(Registration {
        schedule: schedule.to_string(),
        runs,
        label: call
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// The name a scheduled workflow gives: the library entry it names, or its own
/// graph's name.
fn workflow_name(workflow: &Value) -> Option<String> {
    workflow
        .get("library")
        .or_else(|| workflow.get("spec").and_then(|spec| spec.get("name")))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The card for `registration`: the guard's sentence, then what the call gave
/// — each word of it as `card_words` shows data — when it runs on the clock of
/// `zone`, counted from `now`, and the folder it runs in.
fn card(registration: &Registration, zone: CronZone, now: u64, folder: Option<&Path>) -> String {
    let mut card = String::from(match registration.runs {
        Runs::Prompt(_) => SAYS_OF_A_PROMPT,
        Runs::Workflow(_) => SAYS_OF_A_WORKFLOW,
    });
    let said = |words: &str| Some(shown(words, WORD_MAX_CHARS)).filter(|shown| !shown.is_empty());
    if let Some(label) = registration.label.as_deref().and_then(said) {
        let _ = write!(card, "\nCalled: {label}");
    }
    match &registration.runs {
        Runs::Prompt(prompt) => {
            let _ = write!(card, "\nRuns: {}", excerpt(prompt));
        }
        Runs::Workflow(name) => match name.as_deref().and_then(said) {
            Some(name) => {
                let _ = write!(card, "\nRuns: the workflow {name}");
            }
            None => card.push_str("\nRuns: a workflow that gave no name"),
        },
    }
    let _ = write!(
        card,
        "\nWhen: {} ({} time). Next: {}",
        shown(&registration.schedule, WORD_MAX_CHARS),
        zone.label,
        coming(&registration.schedule, zone, now),
    );
    if let Some(folder) = folder {
        let _ = write!(
            card,
            "\nWhere: {}",
            shown(&folder.to_string_lossy(), EXCERPT_MAX_CHARS)
        );
    }
    card
}

/// The head of a prompt as the card shows it, and how long the whole is when
/// the head is not all of it: a cut is never passed off as the end.
fn excerpt(prompt: &str) -> String {
    let head = shown(prompt, EXCERPT_MAX_CHARS);
    let whole = shown(prompt, usize::MAX);
    if head.chars().count() < whole.chars().count() {
        format!(
            "\"{head}\"… ({} characters in all)",
            prompt.chars().count()
        )
    } else {
        format!("\"{head}\"")
    }
}

/// The next [`CARD_RUNS`] minutes `schedule` names after `now`, on the clock
/// of `zone`.
fn coming(schedule: &str, zone: CronZone, now: u64) -> String {
    let mut runs = Vec::with_capacity(CARD_RUNS);
    let mut after = now;
    while runs.len() < CARD_RUNS {
        let Ok(Some(next)) = next_due_at_after_in_zone(schedule, after, zone.offset_secs) else {
            break;
        };
        runs.push(clock(next, zone.offset_secs));
        after = next;
    }
    if runs.is_empty() {
        "none in the five years ahead".to_string()
    } else {
        runs.join(" · ")
    }
}

/// `Thu 2026-11-12 09:00`: an instant on the clock `offset_secs` ahead of UTC.
fn clock(unix_secs: u64, offset_secs: i64) -> String {
    /// Sunday first, as the arithmetic below counts them.
    const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

    let wall = i64::try_from(unix_secs)
        .unwrap_or(i64::MAX)
        .saturating_add(offset_secs);
    let days = wall.div_euclid(86_400);
    let seconds = wall.rem_euclid(86_400);
    let (year, month, day) = core_types::date::civil_from_unix_days(days);
    // The Unix epoch was a Thursday.
    let weekday = WEEKDAYS[usize::try_from((days + 4).rem_euclid(7)).unwrap_or(0)];
    format!(
        "{weekday} {year:04}-{month:02}-{day:02} {:02}:{:02}",
        seconds / 3_600,
        (seconds % 3_600) / 60
    )
}

#[cfg(test)]
mod tests;
