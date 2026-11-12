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
//! A schedule zo asks for runs no closer together than the floor the window
//! holds an asked schedule to (`CronCreateInput::work`). A call that would
//! register nothing — a tighter schedule, an expression that does not read, a
//! prompt and a workflow at once or neither — is refused in the tool's own
//! words before anybody is asked (`registers_nothing`): a yes to it would
//! register nothing, so no mode puts that question to a person.
//!
//! The shell is not a second road to the store. `zo cron ensure` is the
//! person's own — typed by them, or sent by the window on their click — and a
//! shell command of the model's that spells it out is refused and sent to the
//! tool, where the question stands ([`refused_in_a_shell`]). It is refused
//! where the kill guard and the desk guard refuse: the permission policy, the
//! enforcer a sub-agent's commands and a command file's `!` lines go through,
//! and the `PowerShell` tool.
//!
//! What this is not: a boundary. The refusal reads the names a command spells
//! out, by the reading the kill guard uses, and these kinds get past it:
//! - a script file, another interpreter, or a program that runs a command for
//!   the shell (`find -exec`, a task runner, `zo line add`);
//! - zo under another name: an alias, a variable, `cargo run`;
//! - the word `cron` split by quotes or a backslash, which the first look for
//!   that word in the command's letters passes over;
//! - a verb filled in at run time inside a string handed to a shell
//!   (`bash -c "zo cron $VERB"`): the reading drops what is filled in and
//!   takes the next word for the verb, so the call passes when no word
//!   follows, or when the one that does is a verb that registers nothing. At
//!   the top of a command such a verb is refused, since the empty word keeps
//!   its place;
//! - an edit of the store's file.
//!
//! The guard closes the asking, not the file.

use std::fmt::Write as _;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use core_types::tool_input::parse_tool_arguments;
use serde_json::Value;
use zerocode_core::card_words::{hidden_said, shown, shown_counting, EXCERPT_MAX_CHARS, WORD_MAX_CHARS};
use zerocode_core::shell_rule::runs::{self, ProgramRun};

use crate::team_cron_registry::{cron_zone, next_due_at_after_in_zone, CronCreateInput, CronWork, CronZone};

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

/// The program that writes the schedule store from a shell, by the name in
/// command position (`.exe` taken off), and the word that opens its schedule
/// verbs.
const SHELL_PROGRAM: &str = "zo";
const SHELL_VERBS_UNDER: &str = "cron";

/// The verb that registers a schedule.
const SHELL_VERB_THAT_REGISTERS: &str = "ensure";

/// The verbs under [`SHELL_VERBS_UNDER`] that register nothing: one reads the
/// store, the other takes a record out. Any other word there is refused as
/// the one that registers — a verb the command does not spell out among them
/// — save the words that only print the usage ([`SHELL_USAGE_WORDS`]).
const SHELL_VERBS_THAT_REGISTER_NOTHING: &[&str] = &["show", "remove"];

/// The words under [`SHELL_VERBS_UNDER`] that print the usage and do nothing.
const SHELL_USAGE_WORDS: &[&str] = &["-h", "--help"];

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
/// ([`asks`]). `workspace` is the folder the session's schedules are kept
/// for, which is where this one would run; without it the card names the
/// folder this process stands in.
#[must_use]
pub(crate) fn asked(tool_name: &str, input: &str, workspace: Option<&Path>) -> Option<String> {
    let registration = registration(tool_name, input)?;
    let here = if workspace.is_none() { std::env::current_dir().ok() } else { None };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Some(card(&registration, cron_zone(), now, workspace.or(here.as_deref())))
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

/// The refusal for a shell `command` of the model's that would register a
/// schedule past the question — `zo cron ensure`, where the shell would run
/// it and the reading reaches it (the module says what it does not reach) —
/// with the tool to ask through instead. `None` for any other command:
/// `zo cron show` and `zo cron remove` register nothing, and `-h` prints the
/// usage. A word after `cron` that is none of these is refused as the verb
/// that registers; at the top of a command that includes a verb the shell
/// fills in when it runs. Asked by every layer that judges a model's command,
/// after the kill guard and the desk guard; no mode, rule or hook answers it,
/// since the question it sends the model to is one only a person answers.
#[must_use]
pub fn refused_in_a_shell(command: &str) -> Option<String> {
    let ran = runs::programs_run(command, |text| text.contains(SHELL_VERBS_UNDER));
    let verb = ran.iter().find_map(registering_verb)?;
    let spelled = if verb == SHELL_VERB_THAT_REGISTERS {
        format!("`{SHELL_PROGRAM} {SHELL_VERBS_UNDER} {SHELL_VERB_THAT_REGISTERS}`")
    } else {
        format!(
            "`{SHELL_PROGRAM} {SHELL_VERBS_UNDER}` with a verb the command does not spell out as `{}`",
            SHELL_VERBS_THAT_REGISTER_NOTHING.join("` or `")
        )
    };
    Some(format!(
        "refused to run: this command registers a schedule from a shell ({spelled}), past the \
         question a person answers. Ask for the schedule with the {TOOL} tool (`schedule`, \
         `prompt` or `workflow`, `description`): it puts the schedule to a person. {NOT_THE_MODE}"
    ))
}

/// The verb by which `run` would write the schedule store: the word after
/// [`SHELL_VERBS_UNDER`] when the program is [`SHELL_PROGRAM`] and that word
/// neither registers nothing nor only prints the usage. `zo cron` with no
/// word after it prints its usage too.
fn registering_verb(run: &ProgramRun) -> Option<&str> {
    let program = run.program.strip_suffix(".exe").unwrap_or(&run.program);
    let [under, verb, ..] = run.args.as_slice() else {
        return None;
    };
    let verb = verb.as_str();
    (program == SHELL_PROGRAM
        && under.as_str() == SHELL_VERBS_UNDER
        && !SHELL_VERBS_THAT_REGISTER_NOTHING.contains(&verb)
        && !SHELL_USAGE_WORDS.contains(&verb))
    .then_some(verb)
}

/// Why `tool_name` called with `input` would register nothing, when it is
/// the tool that registers a schedule and its input reads: an expression
/// that does not read, a prompt and a workflow at once or neither, runs
/// closer together than the floor ([`CronCreateInput::work`], the tool's own
/// judgment and words). The policy refuses such a call with this before the
/// mode and the rules are asked, so no person is put a question whose yes
/// registers nothing. `None` for any other tool, for a call that registers,
/// and for an input the tool cannot read at all, which is the tool's to
/// answer.
#[must_use]
pub(crate) fn registers_nothing(tool_name: &str, input: &str) -> Option<String> {
    let why = call(tool_name, input)?.work().err()?;
    Some(format!("refused, since it would register nothing: {why}. {NOT_THE_MODE}"))
}

/// Whether `tool_name` is the tool that registers a schedule, in any
/// spelling a rule would meet it by (`tool_key`: case aside — it has no
/// alias and no separator). Asked of every call, so it compares in place.
fn registers_schedules(tool_name: &str) -> bool {
    tool_name.eq_ignore_ascii_case(TOOL)
}

/// What `tool_name` called with `input` would register. The input is read as
/// the executor reads it (`parse_tool_arguments`, a mended slip included), into
/// the arguments the tool reads, and what they register is the tool's own
/// judgment ([`CronCreateInput::work`]) — so a call the tool would run is
/// never one the guard did not see.
fn registration(tool_name: &str, input: &str) -> Option<Registration> {
    let call = call(tool_name, input)?;
    let runs = match call.work().ok()? {
        CronWork::Workflow(workflow) => Runs::Workflow(workflow_name(workflow)),
        CronWork::Prompt(prompt) => Runs::Prompt(prompt.trim().to_string()),
    };
    Some(Registration {
        schedule: call.schedule,
        runs,
        label: call.description,
    })
}

/// The arguments `tool_name` called with `input` hands the tool that
/// registers a schedule, read as the executor reads them
/// (`parse_tool_arguments`, a mended slip included); `None` for any other
/// tool and for an input the tool cannot read.
fn call(tool_name: &str, input: &str) -> Option<CronCreateInput> {
    if !registers_schedules(tool_name) {
        return None;
    }
    serde_json::from_value(parse_tool_arguments(input).ok()?).ok()
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
    // Letters that draw nothing are left out of what is shown and counted,
    // and the card says how many: what would run holds no word a person
    // approving it cannot see.
    let mut hidden = 0usize;
    if let Some(label) = registration.label.as_deref().and_then(|label| said(label, &mut hidden)) {
        let _ = write!(card, "\nCalled: {label}");
    }
    match &registration.runs {
        Runs::Prompt(prompt) => {
            hidden += shown_counting(prompt, EXCERPT_MAX_CHARS).hidden;
            let _ = write!(card, "\nRuns: {}", excerpt(prompt));
        }
        Runs::Workflow(name) => match name.as_deref().and_then(|name| said(name, &mut hidden)) {
            Some(name) => {
                let _ = write!(card, "\nRuns: the workflow {name}");
            }
            None => card.push_str("\nRuns: a workflow that gave no name"),
        },
    }
    if let Some(left_out) = hidden_said(hidden) {
        let _ = write!(card, "\nHidden: {left_out}");
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

/// A name the call gave, as the card shows it — none when nothing of it
/// draws — with the letters left out of it added to `hidden`.
fn said(words: &str, hidden: &mut usize) -> Option<String> {
    let shown = shown_counting(words, WORD_MAX_CHARS);
    *hidden += shown.hidden;
    Some(shown.text).filter(|text| !text.is_empty())
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
    let at = core_types::date::wall_clock(i64::try_from(unix_secs).unwrap_or(i64::MAX), offset_secs);
    format!(
        "{} {:04}-{:02}-{:02} {:02}:{:02}",
        at.weekday, at.year, at.month, at.day, at.hour, at.minute
    )
}

#[cfg(test)]
mod tests;
