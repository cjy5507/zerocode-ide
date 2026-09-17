//! Recipes replayed (docs/design/computer-use-full-operator.md §7.2): what
//! `recipe-run` decides without a model — which saved steps run, which stop
//! for the person or for a fresh look, how a `{{parameter}}` fills a flag's
//! value, how long one step may hold the desk, and where a walk may resume.
//! The window walks the steps down the lone command's road; every decision it
//! makes is read from here, so the document, zo and the tests share the words.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::agent_browser::{self, BROWSER_CLI};
use crate::agent_emulator::{self, EMULATOR_CLI};
use crate::computer_use::{
    COMPUTER_CLI, COMPUTER_SETTLE_MS, COMPUTER_USE_DEADLINE_SECONDS, ComputerMethod,
    ELEMENT_INDEX_FLAGS, PROGRAM_WORD_FLAGS, WALKED_STEP_REFUSED_FLAGS, hold_of, parse_command,
    verb_method, walk_budget_ms, walk_step_holds_ms, walked_step, walked_step_flags,
    walked_step_refusal,
};
use crate::computer_use_protocol::identity::AppQuery;

/// The headings a recipe document is read by: steps only under theirs, so
/// prose anywhere else never runs.
pub const RECIPE_HEADING_STEPS: &str = "## Steps";
pub const RECIPE_HEADING_PARAMS: &str = "## Parameters";
/// A step line's marks — written by `recipe-save`, read back by `recipe-run`.
pub const RECIPE_MARK_SEPARATOR: &str = " — ";
pub const RECIPE_MARK_PERSONS_LAST_STEP: &str = "the person's last step: the window asks";
pub const RECIPE_MARK_PERSONS_TURN: &str = "the person's turn";
pub const RECIPE_MARK_FAILED: &str = "failed then:";
/// The step that moves money (docs/design/flow-engine-guarded-money-path.md
/// §2) — written by the person in the document, never by `recipe-save`: a
/// `dry` Flow stops before it, a `guarded` one presses it past its gate.
/// One to a document: a transaction moves once.
pub const RECIPE_MARK_MONEY: &str = "money";
/// A placeholder a person reads and types: `{{name}}` — plain characters in
/// the command-line grammar, so a filled line splits back into its words.
pub const RECIPE_PARAM_OPEN: &str = "{{";
pub const RECIPE_PARAM_CLOSE: &str = "}}";
/// What `recipe-save` names the text the log kept only as its length:
/// `text-<step>`, which a person renames in the document.
pub const RECIPE_TEXT_PARAM: &str = "text";
/// How long a recorded check without its own budget waits in a replay: a
/// screen that never comes hands the walk back in 10 s, not the whole run.
pub const RECIPE_CHECK_MS: u64 = 10_000;
/// How far the pointer may sit from where the walk left it before the walk
/// takes it that a person moved it.
pub const RECIPE_CURSOR_SLOP_POINTS: f64 = 2.0;
/// Checks: their exit code is the assertion a walker can judge.
pub const RECIPE_CHECKS: &[ComputerMethod] = &[ComputerMethod::WaitFor, ComputerMethod::SoundWait];
/// Acts whose landing a walk judges: a pressed point repaints around it (the
/// pixels of a box about the point, on whichever display shows it), a typed
/// text reads back from the field it went into (the helper's verification).
pub const RECIPE_LANDS: &[ComputerMethod] = &[ComputerMethod::MouseClick, ComputerMethod::Type];
/// The side of the box a pressed point is judged in, in screen points: the
/// control under it and its neighbourhood — a menu it opens, a field it
/// focuses — but not a spinner, a caret or a clock across the screen.
pub const RECIPE_LANDING_BOX_POINTS: f64 = 240.0;
/// Looks taken for a model to read; a replay has nobody to show them to.
pub const RECIPE_SKIPPED: &[ComputerMethod] = &[ComputerMethod::Screenshot, ComputerMethod::Zoom];
/// Never kept on save, skipped on run: the housekeeping around the work.
pub const RECIPE_HOUSEKEEPING: &[ComputerMethod] = &[
    ComputerMethod::Status,
    ComputerMethod::Evidence,
    ComputerMethod::Stop,
    ComputerMethod::Resume,
    ComputerMethod::Verdict,
    ComputerMethod::RecipeSave,
    ComputerMethod::RecipeList,
    ComputerMethod::RecipeShow,
    ComputerMethod::RecipeRun,
];

/// Which door a recipe line goes through: the desktop helper's CLI, the
/// window's browser door, or the mobile emulator door. Read from the code
/// span's first word and written back as it — never guessed from the verb,
/// since the doors share verbs (every one has a `screenshot`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecipeTool {
    Computer,
    Browser,
    Emulator,
}

impl RecipeTool {
    pub const ALL: [Self; 3] = [Self::Computer, Self::Browser, Self::Emulator];

    /// The command word a line of this tool starts with — the shim on the
    /// pane's PATH.
    #[must_use]
    pub const fn command_word(self) -> &'static str {
        match self {
            Self::Computer => COMPUTER_CLI,
            Self::Browser => BROWSER_CLI,
            Self::Emulator => EMULATOR_CLI,
        }
    }

    /// The tool's JSON word, as `serde` writes it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Computer => "computer",
            Self::Browser => "browser",
            Self::Emulator => "emulator",
        }
    }

    /// The tool whose command word this is.
    #[must_use]
    pub fn of_word(word: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|tool| tool.command_word() == word)
    }
}

/// Which verbs a window flag names a saved screen's window on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Names {
    Every,
    /// A window verb's `--id` is a window; `permissions --id` is a name.
    WindowVerbs,
}

/// Flags that carry a window number the saved run's screen handed out — a
/// window id is per launch, a window index an ordinal of the app's windows
/// that day. With the element indexes (validated only against the latest
/// tree, `ELEMENT_INDEX_FLAGS`) and an `--app pid:N` (a process of that day)
/// they stop a walk for a fresh look.
pub const RECIPE_WINDOW_FLAGS: &[(&str, Names)] = &[
    ("window-id", Names::Every),
    ("window-index", Names::Every),
    ("id", Names::WindowVerbs),
];
/// A cursor a listener handed out — `sound-wait --after` counts the events of
/// one `listen-start`, which a replay starts afresh: dropped from a saved line
/// and from a walked one, so the wait listens from the replay's newest event.
pub const RECIPE_LAUNCH_CURSORS: &[(ComputerMethod, &str)] =
    &[(ComputerMethod::SoundWait, "after")];

/// Why a walk stopped — and whether it resumes at that step or after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecipeStop {
    PersonsTurn,
    PersonsLastStep,
    NeedsALook,
    StepFailed,
    CheckFailed,
    NothingChanged,
    PersonMoved,
    Budget,
    Stopped,
    /// The money step: a rehearsal ends before it, a guarded walk waits at
    /// it for the confirmation bound to its transaction.
    MoneyStep,
}

impl RecipeStop {
    pub const ALL: [Self; 10] = [
        Self::PersonsTurn,
        Self::PersonsLastStep,
        Self::NeedsALook,
        Self::StepFailed,
        Self::CheckFailed,
        Self::NothingChanged,
        Self::PersonMoved,
        Self::Budget,
        Self::Stopped,
        Self::MoneyStep,
    ];

    /// The stop a report's own word names ([`Self::as_str`] read backwards),
    /// so a caller reading a walk's report never keeps a table of its own.
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|stop| stop.as_str() == word)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PersonsTurn => "persons_turn",
            Self::PersonsLastStep => "persons_last_step",
            Self::NeedsALook => "needs_a_look",
            Self::StepFailed => "step_failed",
            Self::CheckFailed => "check_failed",
            Self::NothingChanged => "nothing_changed",
            Self::PersonMoved => "person_moved",
            Self::Budget => "budget",
            Self::Stopped => "stopped",
            Self::MoneyStep => "money_step",
        }
    }

    /// Whether the walk resumes after the step it stopped at: that step was
    /// held, handed over, or already ran — someone does it right, then the
    /// walk goes on. The rest did not happen as the recipe says: resume at it.
    #[must_use]
    pub const fn resumes_after(self) -> bool {
        matches!(
            self,
            Self::PersonsTurn
                | Self::PersonsLastStep
                | Self::NeedsALook
                | Self::NothingChanged
                | Self::PersonMoved
        )
    }

    /// What to do, in words every client can follow (no verb names — zo has
    /// no handoff of its own).
    #[must_use]
    pub const fn advice(self) -> &'static str {
        match self {
            Self::PersonsTurn => {
                "this step is the person's (a code, a CAPTCHA, a press the operator may not make): ask them to do it, then run again from next"
            }
            Self::PersonsLastStep => {
                "this press is the person's last step (money or deletion): it was not pressed — let the person decide it, then run again from next"
            }
            Self::NeedsALook => {
                "this step names an element or a window the saved run's screen handed out: look, do it on this screen, then run again from next"
            }
            Self::StepFailed => {
                "the step was refused: look, set the screen right, then run again from next"
            }
            Self::CheckFailed => {
                "the screen (or the sound) is not where the saved run was: look, get there, then run again from next"
            }
            Self::NothingChanged => {
                "the act left no mark where it landed: look — if it did land (a field that hides what is typed), go on; else do it where it belongs, never blindly again — then run again from next"
            }
            Self::PersonMoved => {
                "a person moved the pointer while this step ran: the desk is theirs — wait for them, look, then run again from next"
            }
            Self::Budget => "the run's time ran out before this step: run again from next",
            Self::Stopped => {
                "the operator was stopped: wait for the person's resume, then run again from next"
            }
            Self::MoneyStep => {
                "this step moves money and was not pressed: a dry Flow rehearses up to it; a guarded Flow presses it once the person confirms the transaction the card names — run again from this step with --confirm <txn>"
            }
        }
    }
}

/// A parameter's name: letters (any script), digits, `-` and `_`.
#[must_use]
pub fn recipe_param_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|glyph| glyph.is_alphanumeric() || matches!(glyph, '-' | '_'))
}

/// The values `--params` gives, as the walk fills them: a JSON object of
/// named strings (a number is kept as its decimal text).
pub fn recipe_params(raw: &str) -> Result<Map<String, Value>, String> {
    let Value::Object(given) = serde_json::from_str::<Value>(raw)
        .map_err(|error| format!("--params is a JSON object of named values: {error}"))?
    else {
        return Err("--params is a JSON object of named values, like '{\"to\":\"Kim\"}'".into());
    };
    let mut values = Map::new();
    for (name, value) in given {
        if !recipe_param_name(&name) {
            return Err(format!(
                "--params: `{name}` is not a parameter name (letters, digits, - and _)"
            ));
        }
        let text = match value {
            Value::String(text) => text,
            Value::Number(number) => number.to_string(),
            _ => return Err(format!("--params: `{name}` is a string or a number")),
        };
        values.insert(name, Value::String(text));
    }
    Ok(values)
}

/// Every `{{name}}` in one word, in order; `{{a b}}` is not one.
#[must_use]
pub fn recipe_placeholders(word: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = word;
    while let Some(open) = rest.find(RECIPE_PARAM_OPEN) {
        let after = &rest[open + RECIPE_PARAM_OPEN.len()..];
        let Some(close) = after.find(RECIPE_PARAM_CLOSE) else {
            break;
        };
        let name = &after[..close];
        if recipe_param_name(name) {
            found.push(name.to_string());
            rest = &after[close + RECIPE_PARAM_CLOSE.len()..];
        } else {
            rest = after;
        }
    }
    found
}

/// Why a line could not be filled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unfilled {
    /// A placeholder in the verb: a value never chooses what runs.
    Verb,
    /// A placeholder in a flag's name: a value never chooses a flag.
    FlagName,
    /// This name's value reads like a flag, where only a value may stand.
    FlagShaped(String),
    /// These names have no value.
    Missing(Vec<String>),
}

/// A line with its placeholders filled, each within its own word. A value
/// fills a value and nothing else: a placeholder may not stand in the verb or
/// in a flag's name, and a filled word may not read like a flag unless it is a
/// program's own words (`--args`) — so a value can never lift a guard or
/// name an element the saved screen handed out.
pub fn recipe_fill(argv: &[String], values: &Map<String, Value>) -> Result<Vec<String>, Unfilled> {
    if argv
        .first()
        .is_some_and(|verb| !recipe_placeholders(verb).is_empty())
    {
        return Err(Unfilled::Verb);
    }
    let mut missing: Vec<String> = Vec::new();
    let mut filled: Vec<String> = Vec::with_capacity(argv.len());
    for (at, word) in argv.iter().enumerate() {
        let names = recipe_placeholders(word);
        if names.is_empty() {
            filled.push(word.clone());
            continue;
        }
        if word.starts_with('-') {
            return Err(Unfilled::FlagName);
        }
        let mut out = word.clone();
        for name in names {
            match values.get(&name).and_then(Value::as_str) {
                Some(value) => {
                    out = out.replacen(
                        &format!("{RECIPE_PARAM_OPEN}{name}{RECIPE_PARAM_CLOSE}"),
                        value,
                        1,
                    );
                    let program_words = at
                        .checked_sub(1)
                        .and_then(|before| argv[before].strip_prefix("--"))
                        .is_some_and(|flag| PROGRAM_WORD_FLAGS.contains(&flag));
                    if out.starts_with("--") && !program_words {
                        return Err(Unfilled::FlagShaped(name));
                    }
                }
                None if !missing.contains(&name) => missing.push(name),
                None => {}
            }
        }
        filled.push(out);
    }
    if missing.is_empty() {
        Ok(filled)
    } else {
        Err(Unfilled::Missing(missing))
    }
}

/// Where a saved step stops the walk before it runs, whatever the screen:
/// the person's turn, a press they declared theirs, or a number the saved
/// screen handed out.
#[must_use]
pub fn recipe_step_stop(argv: &[String]) -> Option<RecipeStop> {
    let method = verb_method(argv.first()?)?;
    if method == ComputerMethod::Handoff {
        return Some(RecipeStop::PersonsTurn);
    }
    let flags = || {
        argv.iter()
            .skip(1)
            .filter_map(|word| word.strip_prefix("--"))
    };
    if flags().any(|flag| flag == "confirming") {
        return Some(RecipeStop::PersonsLastStep);
    }
    let looks = names_a_saved_number(argv)
        || flags().any(|flag| {
            RECIPE_WINDOW_FLAGS.iter().any(|(name, names)| {
                *name == flag
                    && match names {
                        Names::Every => true,
                        Names::WindowVerbs => method.window_action().is_some(),
                    }
            })
        });
    let pid = argv.windows(2).any(|pair| {
        pair[0] == "--app" && matches!(AppQuery::parse(&pair[1]), Ok(AppQuery::Pid(_)))
    });
    (looks || pid).then_some(RecipeStop::NeedsALook)
}

/// Whether a line names its control by a number the saved screen handed out —
/// an index into the tree or a mark on the look ([`ELEMENT_INDEX_FLAGS`]) —
/// which a replay must earn again with a fresh look before it trusts.
fn names_a_saved_number(argv: &[String]) -> bool {
    argv.iter()
        .skip(1)
        .filter_map(|word| word.strip_prefix("--"))
        .any(|flag| ELEMENT_INDEX_FLAGS.contains(&flag))
}

/// Where a saved browser line stops the walk before it runs: a
/// `zerocode-browser click <label> --mark <n>` names a number the saved
/// screen handed out, exactly like the desktop `--mark`, so the replay stands
/// for a fresh look rather than trusting a stale number. The door counts a
/// browser line's other words itself, so this is its only static stop.
#[must_use]
pub fn browser_step_stop(argv: &[String]) -> Option<RecipeStop> {
    names_a_saved_number(argv).then_some(RecipeStop::NeedsALook)
}

/// A line without the words a launch handed out (`RECIPE_LAUNCH_CURSORS`).
fn without_launch_cursors(argv: &[String]) -> Vec<String> {
    let method = argv.first().and_then(|verb| verb_method(verb));
    let mut kept = Vec::with_capacity(argv.len());
    let mut words = argv.iter();
    while let Some(word) = words.next() {
        let cursor = word.strip_prefix("--").is_some_and(|flag| {
            RECIPE_LAUNCH_CURSORS
                .iter()
                .any(|(verb, name)| Some(*verb) == method && *name == flag)
        });
        if cursor {
            words.next();
        } else {
            kept.push(word.clone());
        }
    }
    kept
}

/// A step as a walk sends it: a check without a budget given the table's, a
/// launch's cursor dropped, and the flags the walk adds — the picture's
/// refusal only when the walk keeps no frame of the step (`framed`, plan
/// D10). Nothing is cut to fit the run: a step that cannot end inside it
/// waits for the next call (`budget`), so a check never fails for a clock
/// the walk shortened.
#[must_use]
pub fn recipe_step_argv(argv: &[String], framed: bool) -> Vec<String> {
    let mut step = without_launch_cursors(argv);
    let method = step.first().and_then(|verb| verb_method(verb));
    if let Some(asked_by) = method
        .filter(|method| RECIPE_CHECKS.contains(method))
        .and_then(hold_of)
        .and_then(|hold| hold.asked_by)
    {
        let spelled = format!("--{}", asked_by.flag);
        if !step.contains(&spelled) {
            step.extend([spelled, RECIPE_CHECK_MS.to_string()]);
        }
    }
    walked_step(step, framed)
}

/// The box a pressed point's landing is judged in: `RECIPE_LANDING_BOX_POINTS`
/// square about it, in screen points — `[x, y, width, height]`.
#[must_use]
pub fn recipe_landing_box(x: f64, y: f64) -> [f64; 4] {
    let half = RECIPE_LANDING_BOX_POINTS / 2.0;
    [
        x - half,
        y - half,
        RECIPE_LANDING_BOX_POINTS,
        RECIPE_LANDING_BOX_POINTS,
    ]
}

/// A logged step as a recipe keeps it: its saved words, and whether the
/// helper handed it back as the person's last step. A press found guarded
/// (`confirmation_required`, message `<kind>: <label>`) is saved declared —
/// `--confirming <kind>` — so a replay stops before it, never skipping a
/// press that merely "failed then".
#[must_use]
pub fn recipe_saved_step(argv: &[String], refusal: Option<&Value>) -> (Vec<String>, bool) {
    let mut words = recipe_saved_words(argv);
    // A walk that stopped at the person's step leaves that line: it did not
    // fail — it was theirs.
    if refusal
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        == Some(crate::computer_use_protocol::error_code::RECIPE_STOPPED)
    {
        return (words, true);
    }
    let handed_back = refusal
        .filter(|error| {
            error.get("code").and_then(Value::as_str)
                == Some(crate::computer_use_protocol::error_code::CONFIRMATION_REQUIRED)
        })
        .and_then(|error| error.get("message").and_then(Value::as_str))
        .and_then(crate::computer_use::parse_confirmation_required);
    let Some((kind, _)) = handed_back else {
        return (words, false);
    };
    let presses = words
        .first()
        .and_then(|verb| verb_method(verb))
        .is_some_and(ComputerMethod::presses);
    if presses && !words.iter().any(|word| word == "--confirming") {
        words.extend(["--confirming".to_string(), kind.as_str().to_string()]);
    }
    (words, true)
}

/// Whether two saved lines are one step — the person's step a walk stopped
/// at (or a press it handed back) and the lone command that then made it:
/// the same verb, and for anything but the person's turn the same words once
/// a declared `--confirming <kind>` is set aside.
#[must_use]
pub fn recipe_same_step(person: &[String], then: &[String]) -> bool {
    let verb = |line: &[String]| line.first().and_then(|verb| verb_method(verb));
    let Some(method) = verb(person).filter(|method| verb(then) == Some(*method)) else {
        return false;
    };
    if method == ComputerMethod::Handoff {
        return true;
    }
    let undeclared = |line: &[String]| {
        let mut kept = Vec::with_capacity(line.len());
        let mut words = recipe_saved_words(line).into_iter();
        while let Some(word) = words.next() {
            if word == "--confirming" {
                words.next();
            } else {
                kept.push(word);
            }
        }
        kept
    };
    undeclared(person) == undeclared(then)
}

/// How long a walked recipe step may hold the desk: its verb's hold, and for
/// an act whose landing is judged, the settle the walk looks through after it.
#[must_use]
pub fn recipe_step_holds_ms(step: &[String]) -> u64 {
    let lands = step
        .first()
        .and_then(|verb| verb_method(verb))
        .is_some_and(|method| RECIPE_LANDS.contains(&method));
    walk_step_holds_ms(step, false) + if lands { COMPUTER_SETTLE_MS } else { 0 }
}

/// How long one recipe line may hold a walk, by its door: a desktop line
/// by its verb's table and its landing's settle (`recipe_step_holds_ms`), a
/// browser line by the browser table's row — the one answer the preflight
/// fits and the walk budgets.
#[must_use]
pub fn recipe_line_holds_ms(tool: RecipeTool, step: &[String]) -> u64 {
    match tool {
        RecipeTool::Computer => recipe_step_holds_ms(step),
        RecipeTool::Browser => step
            .first()
            .and_then(|verb| agent_browser::holds_ms(verb))
            .unwrap_or_default(),
        RecipeTool::Emulator => step
            .first()
            .and_then(|verb| agent_emulator::holds_ms(verb))
            .unwrap_or_default(),
    }
}

/// A recipe line as a walk sends it, by its door: a desktop line with the
/// flags the walk adds — by whether it keeps a frame of the step (`framed`)
/// — and a check's budget (`recipe_step_argv`), a browser line as it is —
/// the door counts its words, and a flag it does not know would be refused
/// there.
#[must_use]
pub fn recipe_line_argv(tool: RecipeTool, argv: &[String], framed: bool) -> Vec<String> {
    match tool {
        RecipeTool::Computer => recipe_step_argv(argv, framed),
        RecipeTool::Browser => argv.to_vec(),
        RecipeTool::Emulator => emulator_step_argv(argv),
    }
}

/// Whether a non-desktop line's words fit its door's table — the preflight's
/// one arity gate for the doors whose lines are counted, not parsed. A desktop
/// line is parsed instead, so it is never asked here.
fn door_arity_ok(tool: RecipeTool, argv: &[String]) -> Result<(), String> {
    match tool {
        RecipeTool::Computer => Ok(()),
        RecipeTool::Browser => agent_browser::arity_ok(argv),
        RecipeTool::Emulator => agent_emulator::arity_ok(argv),
    }
}

/// An emulator line as a walk sends it: its words with `--json`, so the door
/// answers an envelope the walk reads (its app identity, a pressed rect) —
/// the flag `recipe_saved_words` drops on save, added back for the walk, once.
#[must_use]
fn emulator_step_argv(argv: &[String]) -> Vec<String> {
    let mut step = argv.to_vec();
    if !step.iter().any(|word| word == "--json") {
        step.push("--json".to_string());
    }
    step
}

/// What a recipe keeps of a logged line: its words, one physical line long
/// (a line break in a word would split the step in the document), without
/// the flags a walk adds (`walked_step_flags` — an unframed walk's are all
/// of them), without a flag no walk may carry (`WALKED_STEP_REFUSED_FLAGS`),
/// and without a launch's cursor.
#[must_use]
pub fn recipe_saved_words(argv: &[String]) -> Vec<String> {
    without_launch_cursors(argv)
        .into_iter()
        .filter(|word| {
            !word.strip_prefix("--").is_some_and(|flag| {
                walked_step_flags(false).any(|added| added == flag)
                    || WALKED_STEP_REFUSED_FLAGS.contains(&flag)
            })
        })
        .map(|word| {
            if word.contains(['\n', '\r']) {
                word.split(['\r', '\n'])
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                word
            }
        })
        .collect()
}

/// A command in a Markdown code span that holds it whole: one backtick more
/// than the longest run inside, padded when it starts or ends with one.
#[must_use]
pub fn code_span(text: &str) -> String {
    let longest = text
        .split(|glyph| glyph != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest + 1);
    let pad = if text.starts_with('`') || text.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{text}{pad}{fence}")
}

/// The code span a line starts with, and what follows it.
fn read_code_span(line: &str) -> Option<(&str, &str)> {
    let fence_len = line.chars().take_while(|glyph| *glyph == '`').count();
    if fence_len == 0 {
        return None;
    }
    let fence = &line[..fence_len];
    let rest = &line[fence_len..];
    let mut from = 0;
    while let Some(at) = rest[from..].find(fence) {
        let end = from + at;
        let after = &rest[end + fence_len..];
        if !after.starts_with('`') {
            let inner = &rest[..end];
            let inner = if inner.len() >= 2 && inner.starts_with(' ') && inner.ends_with(' ') {
                &inner[1..inner.len() - 1]
            } else {
                inner
            };
            return Some((inner, after));
        }
        from = end + fence_len + after.chars().take_while(|glyph| *glyph == '`').count();
    }
    None
}

/// The lines under one `## heading` — every occurrence of it, trimmed — or
/// None when the document has no such heading. The one judgement of where a
/// section starts and ends, for `## Steps` and for a Flow's two sections
/// alike: any other `## ` line closes it, so prose elsewhere never runs.
pub(crate) fn section<'a>(text: &'a str, heading: &str) -> Option<Vec<&'a str>> {
    let mut lines: Option<Vec<&str>> = None;
    let mut inside = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("## ") {
            inside = trimmed == heading;
            if inside {
                lines.get_or_insert_with(Vec::new);
            }
            continue;
        }
        if inside {
            lines.get_or_insert_with(Vec::new).push(trimmed);
        }
    }
    lines
}

/// A numbered line that opens a code span — `N. ` or `N) `, the span, and
/// what follows it (the marks). Prose (no number, or a number without a
/// span) is None; a numbered span that does not close on its line is the
/// number and an Err, so the reader can refuse it by name.
pub(crate) struct NumberedLine<'a> {
    pub shown: usize,
    pub span: Result<(&'a str, &'a str), &'static str>,
}

pub(crate) fn numbered_code_span(trimmed: &str) -> Option<NumberedLine<'_>> {
    let digits: String = trimmed.chars().take_while(char::is_ascii_digit).collect();
    let after_number = trimmed[digits.len()..].strip_prefix(['.', ')'])?;
    let shown = digits.parse::<usize>().ok()?;
    let spanned = after_number.trim_start();
    if !spanned.starts_with('`') {
        return None;
    }
    Some(NumberedLine {
        shown,
        span: read_code_span(spanned).ok_or("its code span does not close on its line"),
    })
}

/// A code span's command as its tool and its words: the first word names the
/// door (`RecipeTool::command_word`), the rest split as a command line.
pub fn recipe_command_words(command: &str) -> Result<(RecipeTool, Vec<String>), String> {
    let doors = || {
        RecipeTool::ALL
            .map(|tool| format!("`{}`", tool.command_word()))
            .join(" or ")
    };
    let (tool, words) = command
        .split_once(' ')
        .and_then(|(word, rest)| Some((RecipeTool::of_word(word)?, rest)))
        .ok_or_else(|| format!("its code span is not a {} command", doors()))?;
    let argv = crate::launch::split_command_line(words)?;
    if argv.is_empty() {
        return Err(format!("its code span has no words after {}", doors()));
    }
    Ok((tool, argv))
}

/// One step or check line as a recipe writes it — the tool's word and the
/// line's words in one code span, each mark after the separator — and as
/// `recipe_lines` reads it back. The one writer of the line grammar.
#[must_use]
pub fn recipe_line_text(tool: RecipeTool, argv: &[String], marks: &[String]) -> String {
    let command = format!(
        "{} {}",
        tool.command_word(),
        crate::launch::join_command_line(argv)
    );
    let marks: String = marks
        .iter()
        .map(|mark| format!("{RECIPE_MARK_SEPARATOR}{mark}"))
        .collect();
    format!("{}{marks}", code_span(&command))
}

/// One step line of a recipe document, read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipeLine {
    /// Its position among the step lines, from 1 — what `--start` counts.
    pub step: usize,
    /// The number a person reads in front of it.
    pub shown: usize,
    /// The door its words go through.
    pub tool: RecipeTool,
    pub argv: Vec<String>,
    /// The saved run marked it failed: a replay skips it.
    pub failed_then: bool,
    /// The person marked it the money step (`RECIPE_MARK_MONEY`).
    pub money: bool,
}

/// A line's marks after its code span, each without the separator.
fn marks_of(marks: &str) -> impl Iterator<Item = &str> {
    marks
        .split(RECIPE_MARK_SEPARATOR)
        .map(str::trim)
        .filter(|mark| !mark.is_empty())
}

/// The one step that moves money, if a line carries the mark: its number
/// (counted from 1). Two are refused by number — a transaction moves once.
pub fn money_step(lines: &[RecipeLine]) -> Result<Option<usize>, String> {
    let marked: Vec<usize> = lines
        .iter()
        .filter(|line| line.money)
        .map(|line| line.step)
        .collect();
    match marked.as_slice() {
        [] => Ok(None),
        [step] => Ok(Some(*step)),
        many => Err(format!(
            "steps {} all carry the `{RECIPE_MARK_MONEY}` mark: one transaction moves once, so one step moves it",
            many.iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// The step lines under `## Steps`: `N. ` or `N) `, a code span holding
/// `zerocode-computer <words>` or `zerocode-browser <words>`, and marks after
/// it. Prose is ignored; a numbered line that opens a code span it does not
/// close, or whose span holds anything but a command, is refused — never a
/// step dropped unseen.
pub fn recipe_lines(text: &str) -> Result<Vec<RecipeLine>, String> {
    let mut lines = Vec::new();
    for trimmed in section(text, RECIPE_HEADING_STEPS).into_iter().flatten() {
        let Some(NumberedLine { shown, span }) = numbered_code_span(trimmed) else {
            continue;
        };
        let step = lines.len() + 1;
        let at = |why: &str| format!("step {step} ({shown}.): {why}");
        let (command, marks) = span.map_err(at)?;
        let (tool, argv) = recipe_command_words(command).map_err(|why| at(&why))?;
        lines.push(RecipeLine {
            step,
            shown,
            tool,
            argv,
            failed_then: marks_of(marks).any(|mark| mark.starts_with(RECIPE_MARK_FAILED)),
            money: marks_of(marks).any(|mark| mark == RECIPE_MARK_MONEY),
        });
    }
    Ok(lines)
}

/// Every placeholder the lines name, in order of first use.
#[must_use]
pub fn params_of(lines: &[RecipeLine]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for name in lines
        .iter()
        .flat_map(|line| line.argv.iter().flat_map(|word| recipe_placeholders(word)))
    {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// What a walk does with one line, decided before anything moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Filled and parsed: run it.
    Run(Vec<String>),
    /// Not walked, and why.
    Skip(&'static str),
    /// The walk ends here, whatever the screen.
    Stop(RecipeStop),
}

/// A walk decided before anything moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preflight {
    /// From `start` up to and including the first line that stops the walk.
    pub planned: Vec<(RecipeLine, Plan)>,
    /// Values given that no line from `start` on uses.
    pub unused: Vec<String>,
    /// Placeholders only lines past the stop use — values the person's turn
    /// may teach, needed when the walk resumes, never an error now.
    pub later: Vec<String>,
}

/// Decide the walk from `start` (counted from 1): each line's stop or skip,
/// and every line before the first stop filled and parsed — all missing
/// names in one refusal, so nothing moves on a recipe that cannot finish
/// its first stretch. A line is filled before its stop is judged (a value may
/// name a process of the saved day), and nothing past the first stop is
/// filled or asked for.
pub fn preflight(
    lines: &[RecipeLine],
    start: usize,
    values: &Map<String, Value>,
) -> Result<Preflight, String> {
    if start == 0 || start > lines.len() {
        return Err(format!(
            "--start {start} is not a step of this recipe (it has {})",
            lines.len()
        ));
    }
    let walked = &lines[start - 1..];
    let mut planned = Vec::with_capacity(walked.len());
    let mut missing: Vec<String> = Vec::new();
    let mut through = walked.len();
    let room = walk_budget_ms(COMPUTER_USE_DEADLINE_SECONDS * 1_000);
    for (at_line, line) in walked.iter().enumerate() {
        let desktop = line.tool == RecipeTool::Computer;
        // A browser verb is never a desktop method, whatever it is called.
        let method = desktop
            .then(|| line.argv.first())
            .flatten()
            .and_then(|verb| verb_method(verb));
        let at = |why: &str| format!("step {} ({}.): {why}", line.step, line.shown);
        let fits = |holds: u64| {
            if holds > room {
                return Err(at(&format!(
                    "it may hold the desk {holds} ms and one recipe-run has {room} ms — give it a shorter wait"
                )));
            }
            Ok(())
        };
        let plan = if let Some(kind) = desktop.then(|| recipe_step_stop(&line.argv)).flatten() {
            Plan::Stop(kind)
        } else if line.failed_then {
            Plan::Skip("failed then")
        } else if method.is_some_and(|method| RECIPE_SKIPPED.contains(&method)) {
            Plan::Skip("a look")
        } else if method.is_some_and(|method| RECIPE_HOUSEKEEPING.contains(&method)) {
            Plan::Skip("housekeeping")
        } else {
            match recipe_fill(&line.argv, values) {
                Ok(filled) if desktop => {
                    // Checked as an unframed walk sends it — every flag a
                    // walk may add; a framed walk's step carries fewer.
                    let walked = recipe_step_argv(&filled, false);
                    let command = parse_command(&walked).map_err(|why| at(&why))?;
                    if let Some(why) = walked_step_refusal(&walked, &command) {
                        return Err(at(&why));
                    }
                    fits(recipe_line_holds_ms(line.tool, &walked))?;
                    match recipe_step_stop(&filled) {
                        Some(kind) => Plan::Stop(kind),
                        None => Plan::Run(filled),
                    }
                }
                // A browser or emulator line's words are counted by its door's
                // table and it holds what the table says; the one static stop
                // is a browser `--mark`, a number the saved screen handed out.
                Ok(filled) => {
                    door_arity_ok(line.tool, &filled).map_err(|why| at(&why))?;
                    fits(recipe_line_holds_ms(line.tool, &filled))?;
                    match browser_step_stop(&filled) {
                        Some(kind) => Plan::Stop(kind),
                        None => Plan::Run(filled),
                    }
                }
                Err(Unfilled::Verb) => {
                    return Err(at("a parameter may not name the command"));
                }
                Err(Unfilled::FlagName) => {
                    return Err(at("a parameter may not name a flag"));
                }
                Err(Unfilled::FlagShaped(name)) => {
                    return Err(at(&format!(
                        "the value of {RECIPE_PARAM_OPEN}{name}{RECIPE_PARAM_CLOSE} reads like a flag; a parameter fills a value"
                    )));
                }
                Err(Unfilled::Missing(names)) => {
                    for name in names {
                        if !missing.contains(&name) {
                            missing.push(name);
                        }
                    }
                    Plan::Skip("unfilled")
                }
            }
        };
        let stops = matches!(plan, Plan::Stop(_));
        planned.push((line.clone(), plan));
        if stops {
            through = at_line + 1;
            break;
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "this recipe needs --params for {} (recipe-show lists them)",
            missing
                .iter()
                .map(|name| format!("{RECIPE_PARAM_OPEN}{name}{RECIPE_PARAM_CLOSE}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let (before, after) = walked.split_at(through);
    let named = params_of(walked);
    let early = params_of(before);
    Ok(Preflight {
        planned,
        unused: values
            .keys()
            .filter(|name| !named.contains(name))
            .cloned()
            .collect(),
        later: params_of(after)
            .into_iter()
            .filter(|name| !early.contains(name) && !values.contains_key(name))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computer_use::{
        COMPUTER_USE_DEADLINE_SECONDS, COMPUTER_WAIT_FOR_MAX_MS, allowed, parse_command, usage,
        walk_budget_ms,
    };
    use serde_json::json;

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|word| (*word).to_string()).collect()
    }

    fn values(pairs: &[(&str, &str)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), json!(value)))
            .collect()
    }

    fn line(step: usize, argv: &[&str]) -> RecipeLine {
        RecipeLine {
            step,
            shown: step,
            tool: RecipeTool::Computer,
            argv: words(argv),
            failed_then: false,
            money: false,
        }
    }

    #[test]
    fn a_recipe_run_is_named_filled_and_started_by_the_loop() {
        let run = parse_command(&words(&[
            "recipe-run",
            "--name",
            "x",
            "--params",
            r#"{"to":"Kim","n":3}"#,
            "--start",
            "2",
        ]))
        .expect("a recipe run");
        assert_eq!(run.method, ComputerMethod::RecipeRun);
        assert_eq!(run.params["name"], "x");
        assert_eq!(run.params["recipeParams"], json!({ "to": "Kim", "n": "3" }));
        assert_eq!(run.params["start"], 2);
        assert!(run.method.provider_name().is_none() && run.method.is_desktop());
        assert!(
            run.method.acts() && !run.method.presses(),
            "a stopped door refuses it whole; its presses are judged one by one"
        );
        assert!(usage().contains("recipe-run --name"));
        for refused in [
            &["recipe-run"][..],
            &["recipe-run", "--name", "x", "--params", "[1]"],
            &["recipe-run", "--name", "x", "--params", r#"{"a":{"b":1}}"#],
            &["recipe-run", "--name", "x", "--params", r#"{"a b":1}"#],
            &["recipe-run", "--name", "x", "--start", "0"],
        ] {
            assert!(parse_command(&words(refused)).is_err(), "{refused:?}");
        }
    }

    #[test]
    fn a_recipe_step_stops_where_the_person_or_the_saved_screen_is_needed() {
        let stop = |argv: &[&str]| recipe_step_stop(&words(argv));
        assert_eq!(
            stop(&["handoff", "--reason", "2FA"]),
            Some(RecipeStop::PersonsTurn)
        );
        assert_eq!(
            stop(&[
                "mouse-click",
                "--x",
                "1",
                "--y",
                "1",
                "--confirming",
                "payment"
            ]),
            Some(RecipeStop::PersonsLastStep)
        );
        for argv in [
            &["click", "--app", "X", "--element-index", "3"][..],
            &["click", "--mark", "3", "--look", "812.4"],
            &["window-close", "--id", "9"],
            &["wait-for", "--app", "X", "--text", "a", "--window-id", "3"],
            &["read", "--app", "X", "--window-index", "1"],
        ] {
            assert_eq!(stop(argv), Some(RecipeStop::NeedsALook), "{argv:?}");
        }
        assert_eq!(
            stop(&["permissions", "--id", "accessibility"]),
            None,
            "a permission's --id is a name, not a window"
        );
        assert_eq!(stop(&["mouse-click", "--x", "1", "--y", "1"]), None);
        assert_eq!(stop(&["type", "--text", "hi"]), None);
        assert_eq!(
            stop(&["activate", "--app", "pid:812"]),
            Some(RecipeStop::NeedsALook),
            "a pid is a process of the saved run's day"
        );
        assert_eq!(stop(&["activate", "--app", "Mail"]), None);
        assert_eq!(
            stop(&[
                "click",
                "--app",
                "X",
                "--element-index",
                "3",
                "--confirming",
                "delete"
            ]),
            Some(RecipeStop::PersonsLastStep),
            "the person's step outranks a fresh look"
        );
        // A browser `click --mark <n>` names a number the saved screen handed
        // out, so its replay stops for a fresh look — the same as the desktop
        // `--mark`. An ordinary browser line runs.
        assert_eq!(
            browser_step_stop(&words(&["click", "browser-1", "--mark", "3"])),
            Some(RecipeStop::NeedsALook)
        );
        assert_eq!(
            browser_step_stop(&words(&["click", "browser-1", "#save"])),
            None
        );
        assert_eq!(browser_step_stop(&words(&["read", "browser-1"])), None);
        let after: Vec<RecipeStop> = RecipeStop::ALL
            .into_iter()
            .filter(|kind| kind.resumes_after())
            .collect();
        assert_eq!(
            after,
            [
                RecipeStop::PersonsTurn,
                RecipeStop::PersonsLastStep,
                RecipeStop::NeedsALook,
                RecipeStop::NothingChanged,
                RecipeStop::PersonMoved
            ]
        );
        for (at, kind) in RecipeStop::ALL.iter().enumerate() {
            for other in &RecipeStop::ALL[at + 1..] {
                assert_ne!(kind.as_str(), other.as_str());
                assert_ne!(kind.advice(), other.advice());
            }
        }
    }

    #[test]
    fn a_recipe_parameter_fills_a_flags_value_and_nothing_else() {
        let given = values(&[("msg", "hi there"), ("q", "rust")]);
        assert_eq!(
            recipe_fill(&words(&["type", "--text", "{{msg}}"]), &given),
            Ok(words(&["type", "--text", "hi there"])),
            "one word, spaces and all"
        );
        assert_eq!(
            recipe_fill(
                &words(&["open", "--url", "https://x/?q={{q}}&r={{q}}"]),
                &given
            ),
            Ok(words(&["open", "--url", "https://x/?q=rust&r=rust"]))
        );
        assert_eq!(
            recipe_fill(
                &words(&["type", "--text", "{{a}}{{b}}", "--x", "{{a}}"]),
                &given
            ),
            Err(Unfilled::Missing(words(&["a", "b"]))),
            "every missing name, once"
        );
        assert_eq!(
            recipe_fill(&words(&["{{v}}", "--x", "1"]), &given),
            Err(Unfilled::Verb)
        );
        for (line, value) in [
            (
                &["mouse-click", "--x", "1", "--y", "1", "{{f}}"][..],
                "--allow-self",
            ),
            (&["type", "--text", "{{f}}"], "--allow-self"),
        ] {
            assert_eq!(
                recipe_fill(&words(line), &values(&[("f", value)])),
                Err(Unfilled::FlagShaped("f".into())),
                "a value never lifts a guard: {line:?}"
            );
        }
        assert_eq!(
            recipe_fill(
                &words(&["click", "--app", "X", "--{{w}}", "3"]),
                &values(&[("w", "element-index")])
            ),
            Err(Unfilled::FlagName),
            "a value never chooses a flag"
        );
        assert_eq!(
            recipe_fill(
                &words(&["run", "--program", "/bin/ls", "--args", "{{a}}"]),
                &values(&[("a", "--all")])
            ),
            Ok(words(&["run", "--program", "/bin/ls", "--args", "--all"])),
            "a program's own words may start with dashes"
        );
        assert_eq!(recipe_placeholders("{{a b}} {{ok}}"), ["ok"]);
        assert!(
            recipe_param_name("받는-사람") && !recipe_param_name("") && !recipe_param_name("a b")
        );
        assert_eq!(
            recipe_params(r#"{"받는-사람":"Kim"}"#).expect("a name in any script")["받는-사람"],
            "Kim"
        );
    }

    /// A walked step is the saved line as written — never cut to fit the
    /// run, so a check fails only for its own clock — with a check's default
    /// budget, a launch's cursor dropped and the walk's flags added.
    #[test]
    fn a_recipe_step_is_sent_as_written_and_held_as_its_table_says() {
        let argv = |line: &[&str]| recipe_step_argv(&words(line), false);
        let flag = |argv: &[String], name: &str| {
            let at = argv.iter().position(|word| word == name).expect(name);
            argv[at + 1].parse::<u64>().unwrap()
        };
        let check = argv(&["wait-for", "--app", "X", "--text", "Done"]);
        assert_eq!(
            flag(&check, "--timeout-ms"),
            RECIPE_CHECK_MS,
            "a check without a budget gets the table's"
        );
        let kept = argv(&[
            "wait-for",
            "--app",
            "X",
            "--text",
            "Done",
            "--timeout-ms",
            "20000",
        ]);
        assert_eq!(
            flag(&kept, "--timeout-ms"),
            20_000,
            "a check's own budget is its own"
        );
        assert_eq!(flag(&argv(&["wait", "--ms", "5000"]), "--ms"), 5_000);
        assert_eq!(
            argv(&["run", "--program", "/bin/echo"]),
            words(&["run", "--program", "/bin/echo", "--json"]),
            "a run keeps its table's budget"
        );
        assert_eq!(
            argv(&[
                "sound-wait",
                "--label",
                "siren",
                "--after",
                "7",
                "--timeout-ms",
                "5000"
            ]),
            words(&[
                "sound-wait",
                "--label",
                "siren",
                "--timeout-ms",
                "5000",
                "--json"
            ]),
            "a replay listens from its own newest event"
        );
        let added: Vec<String> = walked_step_flags(false)
            .map(|flag| format!("--{flag}"))
            .collect();
        let click = argv(&["click", "--app", "X", "--x", "1", "--y", "1"]);
        assert!(
            click.ends_with(&added),
            "an unframed walk's app step answers an envelope and takes no picture: {click:?}"
        );
        let framed = recipe_step_argv(
            &words(&["click", "--app", "X", "--x", "1", "--y", "1"]),
            true,
        );
        assert!(
            framed.ends_with(&words(&["--json"]))
                && !added.iter().all(|flag| framed.contains(flag)),
            "a framed walk's app step keeps its picture: {framed:?}"
        );
        let desktop = argv(&["mouse-click", "--x", "1", "--y", "1"]);
        assert!(
            desktop.ends_with(&words(&["--json"])) && !desktop.ends_with(&added),
            "no picture to skip: {desktop:?}"
        );
        let holds = |line: &[&str]| recipe_step_holds_ms(&argv(line));
        assert_eq!(
            holds(&["mouse-click", "--x", "1", "--y", "1"]),
            COMPUTER_SETTLE_MS,
            "a landing is looked at through the settle"
        );
        assert_eq!(
            holds(&["launch", "--app", "TextEdit"]),
            crate::computer_use::COMPUTER_LAUNCH_PROCESS_MS
                + crate::computer_use::COMPUTER_LAUNCH_WAIT_READY_MS
        );
        assert_eq!(
            holds(&["quit", "--app", "TextEdit"]),
            crate::computer_use::COMPUTER_APP_SETTLE_MS,
            "a quit waits the helper's settle"
        );
        assert_eq!(
            holds(&["wait-for", "--app", "X", "--text", "Done"]),
            RECIPE_CHECK_MS
        );
    }

    /// What a recipe keeps of a logged line: the words a person wrote, one
    /// physical line long — never the walk's own flags, a flag no walk may
    /// carry, or a listener's cursor.
    #[test]
    fn a_saved_line_keeps_the_persons_words_and_nothing_the_walk_added() {
        assert_eq!(
            recipe_saved_words(&words(&[
                "type",
                "--text",
                "[5 chars]",
                "--allow-self",
                "--json"
            ])),
            words(&["type", "--text", "[5 chars]"])
        );
        let pressed = words(&["click", "--app", "X", "--x", "1", "--y", "2"]);
        assert_eq!(
            recipe_saved_words(&recipe_step_argv(&pressed, false)),
            pressed,
            "whatever an unframed walk added is stripped"
        );
        assert_eq!(
            recipe_saved_words(&recipe_step_argv(&pressed, true)),
            pressed
        );
        assert_eq!(
            recipe_saved_words(&words(&["sound-wait", "--after", "7", "--label", "siren"])),
            words(&["sound-wait", "--label", "siren"])
        );
        let handed = |code: &str, message: &str| {
            recipe_saved_step(
                &words(&["mouse-click", "--x", "1", "--y", "2", "--json"]),
                Some(&serde_json::json!({ "code": code, "message": message })),
            )
        };
        assert_eq!(
            handed("confirmation_required", "payment: Place order"),
            (
                words(&[
                    "mouse-click",
                    "--x",
                    "1",
                    "--y",
                    "2",
                    "--confirming",
                    "payment"
                ]),
                true
            ),
            "a press handed back is saved as the person's last step"
        );
        assert_eq!(
            handed("element_not_found", "gone"),
            (words(&["mouse-click", "--x", "1", "--y", "2"]), false)
        );
        assert_eq!(
            recipe_saved_words(&words(&[
                "handoff",
                "--reason",
                "Enter the code.\r\nThen press Continue."
            ])),
            words(&[
                "handoff",
                "--reason",
                "Enter the code. Then press Continue."
            ]),
            "a line break would split the step in the document"
        );
    }

    /// A step line may carry the money mark (docs/design/flow-engine-guarded-
    /// money-path.md §2): read back as the line's `money`, written by the one
    /// line writer, and one to a document — a transaction moves once.
    #[test]
    fn a_step_line_may_carry_the_money_mark() {
        let marked = recipe_line_text(
            RecipeTool::Browser,
            &words(&["click", "browser-1", "#send"]),
            &[RECIPE_MARK_MONEY.to_string()],
        );
        assert_eq!(
            marked,
            format!(
                "`zerocode-browser click browser-1 #send`{RECIPE_MARK_SEPARATOR}{RECIPE_MARK_MONEY}"
            )
        );
        let text = format!(
            "{RECIPE_HEADING_STEPS}\n\n1. `zerocode-browser type browser-1 #amount {{{{amount}}}}`\n2. {marked}\n3. `zerocode-computer key --key return`{RECIPE_MARK_SEPARATOR}{RECIPE_MARK_FAILED} money was refused\n"
        );
        let lines = recipe_lines(&text).expect("reads back");
        assert_eq!(
            lines.iter().map(|line| line.money).collect::<Vec<_>>(),
            [false, true, false],
            "only the marked line moves money — a mark's word inside another mark is not the mark"
        );
        assert_eq!(
            lines
                .iter()
                .map(|line| line.failed_then)
                .collect::<Vec<_>>(),
            [false, false, true]
        );
        assert_eq!(money_step(&lines), Ok(Some(2)));
        assert_eq!(money_step(&lines[..1]), Ok(None));
        let twice = format!("{text}4. {marked}\n");
        let both = money_step(&recipe_lines(&twice).unwrap()).unwrap_err();
        assert!(both.contains('2') && both.contains('4'), "{both}");
        // The mark is read whole: a line marked failed and money carries both.
        let text = format!(
            "{RECIPE_HEADING_STEPS}\n\n1. {marked}{RECIPE_MARK_SEPARATOR}{RECIPE_MARK_FAILED} x\n"
        );
        let line = &recipe_lines(&text).unwrap()[0];
        assert!(line.money && line.failed_then);
    }

    #[test]
    fn a_recipe_document_reads_back_into_its_commands() {
        let ticked = code_span("zerocode-computer type --text \"a`b\"");
        assert!(
            ticked.starts_with("``") && !ticked.starts_with("```"),
            "{ticked}"
        );
        let text = format!(
            "# Checkout\n\nprose `zerocode-computer quit --app Safari` never runs\n\n{RECIPE_HEADING_STEPS}\n\n\
             1. `zerocode-computer launch --app \"Google Chrome\"`\n\
             3)  `zerocode-computer type --text {{{{message}}}}`\n\
             not a step\n\
             7. {ticked} — {RECIPE_MARK_FAILED} stopped\n\n\
             ## How to walk it\n\n1. `zerocode-computer quit --app Safari`\n"
        );
        let lines = recipe_lines(&text).expect("readable");
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert_eq!(lines[0].argv, words(&["launch", "--app", "Google Chrome"]));
        assert_eq!(
            (lines[1].step, lines[1].shown),
            (2, 3),
            "the position counts, the written number is shown"
        );
        assert_eq!(lines[1].argv, words(&["type", "--text", "{{message}}"]));
        assert_eq!(lines[2].argv, words(&["type", "--text", "a`b"]));
        assert!(lines[2].failed_then && !lines[0].failed_then);
        assert_eq!(params_of(&lines), ["message"]);
        let broken =
            format!("{RECIPE_HEADING_STEPS}\n\n1. `zerocode-computer type --text \"open`\n");
        assert!(recipe_lines(&broken).unwrap_err().starts_with("step 1"));
        for unreadable in [
            // A word's line break split the step: its span never closes.
            "1. `zerocode-computer handoff --reason \"Enter the code\n   Then press Continue.\"` — the person's turn\n",
            "1. `zerocode-computr key --key tab`\n",
        ] {
            let text = format!("{RECIPE_HEADING_STEPS}\n\n{unreadable}");
            assert!(
                recipe_lines(&text).unwrap_err().starts_with("step 1 (1.)"),
                "never a step dropped unseen: {unreadable}"
            );
        }
        let prose = format!(
            "{RECIPE_HEADING_STEPS}\n\n1. Open the app first\n2. `zerocode-computer key --key tab`\n"
        );
        assert_eq!(
            recipe_lines(&prose)
                .expect("a numbered note is prose")
                .len(),
            1
        );
    }

    /// A step line may be a `zerocode-browser` command: read with its tool,
    /// walked by the browser door's own table, written back with its tool
    /// word — and any other tool word is refused, never a step dropped.
    #[test]
    fn a_recipe_line_may_be_a_browser_command_and_keeps_its_tool() {
        let text = format!(
            "{RECIPE_HEADING_STEPS}\n\n\
             1. `zerocode-browser click browser-1 \"#login-btn\"`\n\
             2. `zerocode-computer key --key return`\n\
             3. `zerocode-browser type browser-1 \"#amount\" {{{{amount}}}}`\n"
        );
        let lines = recipe_lines(&text).expect("readable");
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert_eq!(lines[0].tool, RecipeTool::Browser);
        assert_eq!(lines[0].argv, words(&["click", "browser-1", "#login-btn"]));
        assert_eq!(lines[1].tool, RecipeTool::Computer);
        assert_eq!(lines[1].argv, words(&["key", "--key", "return"]));
        assert_eq!(params_of(&lines), ["amount"]);
        assert_eq!(
            RecipeTool::Browser.command_word(),
            "zerocode-browser",
            "the word the shim on the PATH answers to"
        );
        assert_eq!(RecipeTool::Computer.command_word(), COMPUTER_CLI);
        // Written back through the one writer, each line reads the same.
        for line in &lines {
            let written = recipe_line_text(line.tool, &line.argv, &[]);
            let read = recipe_lines(&format!("{RECIPE_HEADING_STEPS}\n\n1. {written}\n"))
                .expect("its own writing");
            assert_eq!(
                (read[0].tool, &read[0].argv),
                (line.tool, &line.argv),
                "{written}"
            );
        }
        let marked = recipe_line_text(
            RecipeTool::Computer,
            &words(&["key", "--key", "tab"]),
            &[format!("{RECIPE_MARK_FAILED} stopped")],
        );
        assert!(
            marked.ends_with(&format!(
                "{RECIPE_MARK_SEPARATOR}{RECIPE_MARK_FAILED} stopped"
            )),
            "{marked}"
        );
        let read = recipe_lines(&format!("{RECIPE_HEADING_STEPS}\n\n1. {marked}\n")).unwrap();
        assert!(read[0].failed_then);
        let unknown = format!("{RECIPE_HEADING_STEPS}\n\n1. `zerocode-nonsense tap 1 2`\n");
        assert!(
            recipe_lines(&unknown)
                .unwrap_err()
                .starts_with("step 1 (1.)"),
            "an unknown tool word is refused"
        );
        // A browser line is walked as written: no static stop, its words
        // counted by the browser table, its hold the table's.
        let planned = preflight(&lines, 1, &values(&[("amount", "10")])).expect("planned");
        assert_eq!(
            planned.planned[0].1,
            Plan::Run(words(&["click", "browser-1", "#login-btn"]))
        );
        assert_eq!(
            planned.planned[2].1,
            Plan::Run(words(&["type", "browser-1", "#amount", "10"]))
        );
        let short = vec![RecipeLine {
            tool: RecipeTool::Browser,
            ..line(1, &["find", "browser-1"])
        }];
        assert!(
            preflight(&short, 1, &Map::new())
                .unwrap_err()
                .starts_with("step 1"),
            "a browser line with the wrong number of words is refused before anything moves"
        );
        let waits = vec![RecipeLine {
            tool: RecipeTool::Browser,
            ..line(1, &["wait", "browser-1", "#done", "500"])
        }];
        assert_eq!(
            preflight(&waits, 1, &Map::new()).unwrap().planned[0].1,
            Plan::Run(words(&["wait", "browser-1", "#done", "500"]))
        );
    }

    /// A step line may be a `zerocode-emulator` command: read with its tool,
    /// walked by the emulator door's own table, written back with its tool
    /// word — the phone twin of the browser line.
    #[test]
    fn a_recipe_line_may_be_an_emulator_command_and_keeps_its_tool() {
        let text = format!(
            "{RECIPE_HEADING_STEPS}\n\n\
             1. `zerocode-emulator tap --platform ios --device phone --x 0.5 --y 0.5`\n\
             2. `zerocode-computer key --key return`\n\
             3. `zerocode-emulator text --platform ios --device phone --text {{{{code}}}}`\n"
        );
        let lines = recipe_lines(&text).expect("readable");
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert_eq!(lines[0].tool, RecipeTool::Emulator);
        assert_eq!(
            lines[0].argv,
            words(&[
                "tap",
                "--platform",
                "ios",
                "--device",
                "phone",
                "--x",
                "0.5",
                "--y",
                "0.5"
            ])
        );
        assert_eq!(lines[1].tool, RecipeTool::Computer);
        assert_eq!(params_of(&lines), ["code"]);
        assert_eq!(RecipeTool::Emulator.command_word(), "zerocode-emulator");
        assert_eq!(RecipeTool::Emulator.as_str(), "emulator");
        // Written back through the one writer, each line reads the same.
        for line in &lines {
            let written = recipe_line_text(line.tool, &line.argv, &[]);
            let read = recipe_lines(&format!("{RECIPE_HEADING_STEPS}\n\n1. {written}\n"))
                .expect("its own writing");
            assert_eq!(
                (read[0].tool, &read[0].argv),
                (line.tool, &line.argv),
                "{written}"
            );
        }
        // Walked as written: no static stop, its words counted by the emulator
        // table and filled from the run's values.
        let planned = preflight(&lines, 1, &values(&[("code", "1234")])).expect("planned");
        assert_eq!(
            planned.planned[0].1,
            Plan::Run(words(&[
                "tap",
                "--platform",
                "ios",
                "--device",
                "phone",
                "--x",
                "0.5",
                "--y",
                "0.5"
            ]))
        );
        assert_eq!(
            planned.planned[2].1,
            Plan::Run(words(&[
                "text",
                "--platform",
                "ios",
                "--device",
                "phone",
                "--text",
                "1234"
            ]))
        );
        let short = vec![RecipeLine {
            tool: RecipeTool::Emulator,
            ..line(1, &["tap", "--platform", "ios"])
        }];
        assert!(
            preflight(&short, 1, &Map::new())
                .unwrap_err()
                .starts_with("step 1"),
            "an emulator line with too few words is refused before anything moves"
        );
        let unknown = format!("{RECIPE_HEADING_STEPS}\n\n1. `zerocode-nonsense tap 1 2`\n");
        assert!(
            recipe_lines(&unknown)
                .unwrap_err()
                .starts_with("step 1 (1.)"),
            "an unknown tool word is refused"
        );
    }

    #[test]
    fn a_walk_is_decided_before_anything_moves() {
        let lines = vec![
            line(1, &["launch", "--app", "Mail"]),
            line(2, &["type", "--text", "{{to}}"]),
            line(3, &["screenshot"]),
            RecipeLine {
                failed_then: true,
                ..line(4, &["key", "--key", "tab"])
            },
            line(5, &["handoff", "--reason", "2FA"]),
            line(6, &["type", "--text", "{{code}}"]),
        ];
        let planned =
            preflight(&lines, 1, &values(&[("to", "Kim"), ("spare", "x")])).expect("planned");
        let plans: Vec<&Plan> = planned.planned.iter().map(|(_, plan)| plan).collect();
        assert_eq!(plans.len(), 5, "up to and including the first stop");
        assert_eq!(*plans[1], Plan::Run(words(&["type", "--text", "Kim"])));
        assert_eq!(*plans[2], Plan::Skip("a look"));
        assert_eq!(*plans[3], Plan::Skip("failed then"));
        assert_eq!(*plans[4], Plan::Stop(RecipeStop::PersonsTurn));
        assert_eq!(
            planned.later,
            ["code"],
            "a value learnt at the person's turn does not block the steps before it"
        );
        assert_eq!(planned.unused, ["spare"]);

        let missing = preflight(&lines, 1, &Map::new()).unwrap_err();
        assert!(
            missing.contains("{{to}}") && !missing.contains("{{code}}"),
            "{missing}"
        );
        let resumed = preflight(&lines, 6, &values(&[("code", "123456")])).expect("resumed");
        assert_eq!(
            resumed.planned[0].1,
            Plan::Run(words(&["type", "--text", "123456"]))
        );
        assert!(
            preflight(&lines, 7, &Map::new()).is_err()
                && preflight(&lines, 0, &Map::new()).is_err()
        );
        let failed_turn = vec![RecipeLine {
            failed_then: true,
            ..line(1, &["handoff", "--reason", "x"])
        }];
        assert_eq!(
            preflight(&failed_turn, 1, &Map::new()).unwrap().planned[0].1,
            Plan::Stop(RecipeStop::PersonsTurn),
            "a turn that failed then is still the person's"
        );
        let unreadable = vec![line(1, &["mouse-click", "--x", "one"])];
        assert!(
            preflight(&unreadable, 1, &Map::new())
                .unwrap_err()
                .starts_with("step 1")
        );
        let lifted = vec![line(1, &["key", "--key", "return", "--allow-self"])];
        assert!(
            preflight(&lifted, 1, &Map::new())
                .unwrap_err()
                .contains("allow-self"),
            "a saved screen never lifts the guard on ZeroCode's own window"
        );
        let glide = vec![line(
            1,
            &["mouse-move", "--x", "1", "--y", "1", "--steps", "500"],
        )];
        assert!(preflight(&glide, 1, &Map::new()).is_err());
        let endless = vec![line(1, &["launch", "--app", "X", "--wait-ready", "90000"])];
        assert!(
            preflight(&endless, 1, &Map::new())
                .unwrap_err()
                .contains("shorter wait"),
            "a step no one call can hold is refused, not budgeted forever"
        );
        let filled_pid = vec![
            line(1, &["activate", "--app", "{{app}}"]),
            line(2, &["type", "--text", "{{code}}"]),
        ];
        assert_eq!(
            preflight(&filled_pid, 1, &values(&[("app", "pid:812")]))
                .unwrap()
                .planned[0]
                .1,
            Plan::Stop(RecipeStop::NeedsALook),
            "a pid a value names is a process of that day too — and nothing past it is asked for"
        );
    }

    /// The person's step and the lone command that made it are one step.
    #[test]
    fn the_persons_step_and_the_command_that_made_it_are_one() {
        let press = words(&[
            "mouse-click",
            "--x",
            "9",
            "--y",
            "6",
            "--confirming",
            "payment",
        ]);
        assert!(recipe_same_step(
            &press,
            &words(&["mouse-click", "--x", "9", "--y", "6", "--json"])
        ));
        assert!(recipe_same_step(
            &press,
            &words(&[
                "mouse-click",
                "--x",
                "9",
                "--y",
                "6",
                "--confirming",
                "payment"
            ])
        ));
        assert!(!recipe_same_step(
            &press,
            &words(&["mouse-click", "--x", "9", "--y", "7"])
        ));
        assert!(recipe_same_step(
            &words(&["handoff", "--reason", "2FA"]),
            &words(&["handoff", "--reason", "Enter the code from your phone"])
        ));
        assert!(!recipe_same_step(
            &words(&["handoff", "--reason", "x"]),
            &words(&["key", "--key", "tab"])
        ));
        assert_eq!(
            recipe_saved_step(
                &words(&["handoff", "--reason", "2FA", "--json"]),
                Some(&serde_json::json!({ "code": "recipe_stopped", "message": "persons_turn" }))
            ),
            (words(&["handoff", "--reason", "2FA"]), true),
            "the person's step a walk stopped at is kept, never `failed then`"
        );
    }

    #[test]
    fn the_recipe_tables_fit_the_verbs_and_the_run() {
        for method in RECIPE_LANDS {
            assert!(method.acts(), "{method:?}");
        }
        for method in RECIPE_CHECKS {
            assert!(!method.acts(), "{method:?}");
        }
        for (flag, _) in RECIPE_WINDOW_FLAGS {
            assert!(
                ComputerMethod::ALL
                    .iter()
                    .any(|method| allowed(*method).contains(flag)),
                "{flag} is a real flag"
            );
        }
        for (method, flag) in RECIPE_LAUNCH_CURSORS {
            assert!(allowed(*method).contains(flag), "{method:?} --{flag}");
        }
        assert!(RECIPE_HOUSEKEEPING.contains(&ComputerMethod::RecipeRun));
        assert!(RECIPE_HOUSEKEEPING.contains(&ComputerMethod::Verdict));
        const {
            assert!(RECIPE_CHECK_MS <= COMPUTER_WAIT_FOR_MAX_MS);
            assert!(RECIPE_CHECK_MS < walk_budget_ms(COMPUTER_USE_DEADLINE_SECONDS * 1_000));
        }
    }
}
