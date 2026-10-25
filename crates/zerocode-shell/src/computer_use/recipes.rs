//! Recipes (docs/design/computer-use-full-operator.md §7.2): a procedure
//! that worked, kept as a document a person can read and edit, under the
//! window's data root and in the artifacts catalogue. `recipe-run` walks it
//! in one call and stops only where the person or a fresh look is needed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use zerocode_core::computer_flow::{
    EvidenceLevel, FLOW_HEADING, FLOW_HEADING_CHECKS, FLOW_HEADING_TRIGGER, FlowSpec, Policy,
    parse_flow,
};
use zerocode_core::computer_recipe::{
    RECIPE_HEADING_PARAMS, RECIPE_HEADING_STEPS, RECIPE_HOUSEKEEPING, RECIPE_MARK_FAILED,
    RECIPE_MARK_OPENS, RECIPE_MARK_PERSONS_LAST_STEP, RECIPE_MARK_PERSONS_TURN, RECIPE_PARAM_CLOSE,
    RECIPE_PARAM_OPEN, RECIPE_SKIPPED, RECIPE_TAB_ALIAS, RECIPE_TEXT_PARAM, RecipeStop, RecipeTool,
    recipe_line_text, recipe_placeholders, recipe_same_step, recipe_saved_step, recipe_saved_words,
    recipe_step_stop,
};
use zerocode_core::computer_use::{COMPUTER_CLI, RECIPE_STEPS, recipe_slug, verb_method};
use zerocode_core::computer_use_protocol::error_code;

use super::ComputerUseError;
use crate::run_evidence::{Step, captures};
use zerocode_core::computer_recipe::privacy;

/// Under the window's local data root.
pub const RECIPES_DIR: &str = "computer-use/recipes";
/// The most a listing answers.
pub const LIST_MAX: usize = 200;

/// The steps a recipe keeps, by the door they went through: the desktop's
/// and the browser's actions and checks that carried the work — a step
/// that left evidence (`captures`), not the housekeeping around it, and not
/// another surface's steps (a run folder's emulator lines are not these
/// doors'). Answers the door a kept step replays through.
fn kept_by(step: &Step) -> Option<RecipeTool> {
    let tool = RecipeTool::ALL
        .into_iter()
        .find(|tool| tool.as_str() == step.tool)?;
    let housekeeping = tool == RecipeTool::Computer
        && verb_method(&step.verb).is_some_and(|method| RECIPE_HOUSEKEEPING.contains(&method));
    (captures(&step.tool, &step.verb).is_some() && !housekeeping).then_some(tool)
}

/// One kept step as the recipe writes it: its door, its saved words, whether
/// the helper handed it back as the person's last step, and the logged step.
struct Saved<'a> {
    tool: RecipeTool,
    words: Vec<String>,
    handed_back: bool,
    step: &'a Step,
}

/// The kept steps as a recipe writes them (core's `recipe_saved_step`): the
/// walk's own flags dropped, a press the helper handed back — or the person's
/// step a walk stopped at — declared the person's; and when the lone command
/// that made it follows (looks between aside), one step (`recipe_same_step`).
fn saved_steps(steps: &[Step]) -> Vec<Saved<'_>> {
    let mut saved: Vec<Saved<'_>> = Vec::new();
    for (tool, step) in steps.iter().filter_map(|step| Some((kept_by(step)?, step))) {
        let refusal = step
            .error
            .as_deref()
            .and_then(|error| zerocode_core::computer_use_protocol::answer_envelope("", error))
            .and_then(|envelope| envelope.get("error").cloned());
        // A browser or emulator line has no flags a walk adds beyond `--json`
        // (dropped here) and no press the helper hands back: its words are
        // kept as they were, one line long.
        let (words, handed_back) = match tool {
            RecipeTool::Computer => recipe_saved_step(&step.argv, refusal.as_ref()),
            RecipeTool::Browser | RecipeTool::Emulator => (
                recipe_saved_words(
                    &privacy::normalized(tool.as_str(), &step.argv)
                        .unwrap_or_else(|_| step.argv.iter().take(2).cloned().collect()),
                ),
                false,
            ),
        };
        let looks = |saved: &Saved<'_>| {
            saved.tool == RecipeTool::Computer
                && zerocode_core::computer_use::verb_method(&saved.step.verb)
                    .is_some_and(|method| RECIPE_SKIPPED.contains(&method))
        };
        if step.ok
            && let Some(person) = saved.iter_mut().rev().find(|saved| !looks(saved))
            && person.handed_back
            && person.tool == tool
            && recipe_same_step(&person.words, &words)
        {
            // The person's own words win when they declared the step or said
            // what the turn is; a press keeps the kind it was handed back as.
            if words.iter().any(|word| word == "--confirming")
                || recipe_step_stop(&words) == Some(RecipeStop::PersonsTurn)
            {
                person.words = words;
            }
            person.step = step;
            continue;
        }
        saved.push(Saved {
            tool,
            words,
            handed_back,
            step,
        });
    }
    saved
}

/// A parameter name for typed text no other line already uses: `text-<step>`,
/// or `text-<step>-<k>` when a walked line kept that name.
fn free_param(n: usize, taken: &mut std::collections::BTreeSet<String>) -> String {
    let mut name = format!("{RECIPE_TEXT_PARAM}-{n}");
    let mut k = 2;
    while taken.contains(&name) {
        name = format!("{RECIPE_TEXT_PARAM}-{n}-{k}");
        k += 1;
    }
    taken.insert(name.clone());
    name
}

fn recorded_input(step: &Step) -> privacy::InputPolicy {
    let mut policy = privacy::input_policy(&step.tool, &step.argv, &[]);
    if let Some(recorded) = step
        .observation
        .as_ref()
        .and_then(|value| value.get(privacy::INPUT_KEY))
        .and_then(|value| serde_json::from_value::<privacy::InputPolicy>(value.clone()).ok())
    {
        policy.secret |= recorded.secret;
        policy.credential |= recorded.credential;
    }
    policy
}

/// A parameter name for an upload's file at step `n`: `file_<n>`, and
/// `file_<n>_<k>` for the step's later files.
fn free_file(n: usize, taken: &mut std::collections::BTreeSet<String>) -> String {
    let base = format!("file_{n}");
    let mut name = base.clone();
    let mut suffix = 2;
    while taken.contains(&name) {
        name = format!("{base}_{suffix}");
        suffix += 1;
    }
    taken.insert(name.clone());
    name
}

fn free_secret(n: usize, taken: &mut std::collections::BTreeSet<String>) -> String {
    let base = format!("secret_text_{n}");
    let mut name = base.clone();
    let mut suffix = 2;
    while taken.contains(&name) {
        name = format!("{base}_{suffix}");
        suffix += 1;
    }
    taken.insert(name.clone());
    name
}

/// Refuse before creating or replacing a file. The diagnostic contains only
/// source positions; no field, argument, URL or rejected value is echoed.
fn validate_saved_values(
    name: &str,
    note: Option<&str>,
    steps: &[Step],
) -> Result<(), ComputerUseError> {
    let refused = |line: usize, step: usize| {
        ComputerUseError::new(
            error_code::INVALID_ARGUMENT,
            format!("recipe_credentials: line {line}, step {step}"),
        )
    };
    if !privacy::is_reference_value(name) && zerocode_core::credential::may_carry_a_credential(name)
        || note.is_some_and(|value| {
            !privacy::is_reference_value(value)
                && zerocode_core::credential::may_carry_a_credential(value)
        })
    {
        return Err(refused(0, 0));
    }
    for (line, step) in steps.iter().enumerate() {
        let policy = recorded_input(step);
        if policy.credential {
            return Err(refused(line + 1, step.n));
        }
        let argv =
            privacy::normalized(&step.tool, &step.argv).map_err(|_| refused(line + 1, step.n))?;
        let inputs = privacy::value_positions(&step.tool, &argv);
        if argv.iter().enumerate().any(|(index, word)| {
            index > 0
                && !word.starts_with("--")
                && !inputs.contains(&index)
                && privacy::unsafe_stored_argument(&step.tool, &argv, index)
        }) {
            return Err(refused(line + 1, step.n));
        }
        if inputs.is_empty()
            && step
                .error
                .as_deref()
                .is_some_and(zerocode_core::credential::may_carry_a_credential)
        {
            return Err(refused(line + 1, step.n));
        }
    }
    Ok(())
}

/// The document: a title, the note, one command line per step — its words
/// in the command-line grammar `recipe-run` splits back, typed text a named
/// `{{parameter}}` — with the person's points called out, and the parameters
/// listed. The words a person edits and `recipe-run` walks.
#[must_use]
pub fn recipe_markdown(name: &str, note: Option<&str>, steps: &[Step], at_epoch_ms: i64) -> String {
    let kept = saved_steps(steps);
    let mut out = format!(
        "# {name}\n\n- saved: {at_epoch_ms} (epoch ms)\n- steps: {}\n",
        kept.len()
    );
    if let Some(note) = note.map(str::trim).filter(|note| !note.is_empty()) {
        out.push_str(&format!("- note: {note}\n"));
    }
    out.push_str(&format!("\n{RECIPE_HEADING_STEPS}\n\n"));
    let mut params: Vec<(String, String)> = Vec::new();
    let mut browser_tabs: BTreeMap<String, String> = BTreeMap::new();
    let mut opened_aliases = std::collections::BTreeSet::new();
    // Names a walked line already carries: typed text takes none of them.
    let mut taken: std::collections::BTreeSet<String> = kept
        .iter()
        .flat_map(|saved| {
            saved
                .words
                .iter()
                .flat_map(|word| recipe_placeholders(word))
        })
        .collect();
    for (at, saved) in kept.iter().enumerate() {
        let n = at + 1;
        let went = saved.step.ok || saved.handed_back;
        let input = recorded_input(saved.step);
        let value_positions = privacy::value_positions(saved.tool.as_str(), &saved.words);
        let file_positions = privacy::file_positions(saved.tool.as_str(), &saved.words);
        let mut words: Vec<String> = saved
            .words
            .iter()
            .enumerate()
            .map(|(index, word)| {
                if file_positions.contains(&index) && !privacy::is_parameter_word(word) {
                    // A path is where a file sat on one machine: the next run
                    // is given the file, and the document names no folder.
                    let param = free_file(n, &mut taken);
                    params.push((
                        param.clone(),
                        format!("the file chosen at step {n}; its path was not stored"),
                    ));
                    return format!("${param}");
                }
                match (
                    value_positions.contains(&index),
                    crate::run_evidence::redacted_chars(word),
                ) {
                    (true, _) if input.secret => {
                        let param = free_secret(n, &mut taken);
                        params.push((
                            param.clone(),
                            format!("person's input at step {n}; no value was stored"),
                        ));
                        format!("${param}")
                    }
                    (true, None) if !privacy::is_parameter_word(word) => {
                        let param = free_param(n, &mut taken);
                        params.push((
                            param.clone(),
                            format!("typed at step {n}; no value was stored"),
                        ));
                        format!("{RECIPE_PARAM_OPEN}{param}{RECIPE_PARAM_CLOSE}")
                    }
                    // What was typed, kept only as its length, becomes the value
                    // the next run is given — named for the step it was typed at.
                    (true, Some(chars)) => {
                        let param = free_param(n, &mut taken);
                        params.push((
                            param.clone(),
                            format!("typed at step {n} ({chars} characters when saved)"),
                        ));
                        format!("{RECIPE_PARAM_OPEN}{param}{RECIPE_PARAM_CLOSE}")
                    }
                    _ => {
                        // A walk logs its recipe's own placeholders: still values
                        // the next run is given.
                        for name in recipe_placeholders(word) {
                            if !params.iter().any(|(known, _)| *known == name) {
                                params.push((name, format!("given at step {n} when saved")));
                            }
                        }
                        word.clone()
                    }
                }
            })
            .collect();
        let mut marks: Vec<String> = Vec::new();
        if saved.tool == RecipeTool::Browser {
            let label = if words.first().is_some_and(|verb| verb == "open") && went {
                saved
                    .step
                    .observation
                    .as_ref()
                    .and_then(|value| {
                        value.get(zerocode_core::agent_browser::BROWSER_OPENED_TAB_KEY)
                    })
                    .and_then(Value::as_str)
            } else {
                zerocode_core::computer_recipe::browser_tab_target(&words)
            };
            if let Some(alias) =
                label.filter(|label| zerocode_core::computer_recipe::is_tab_alias(label))
                && !browser_tabs.values().any(|known| known == alias)
            {
                browser_tabs.insert(alias.to_string(), alias.to_string());
            }
            if let Some(label) =
                label.filter(|label| zerocode_core::agent_browser::valid_browser_label(label))
            {
                let next = zerocode_core::computer_recipe::tab_alias(browser_tabs.len() + 1);
                let alias = browser_tabs
                    .entry(label.to_string())
                    .or_insert(next)
                    .clone();
                if words[0] == "open" {
                    marks.push(format!("{RECIPE_MARK_OPENS}{alias}"));
                    opened_aliases.insert(alias);
                } else {
                    words[1] = alias;
                }
            }
        }
        if recipe_step_stop(&words) == Some(RecipeStop::PersonsLastStep) {
            marks.push(RECIPE_MARK_PERSONS_LAST_STEP.to_string());
        }
        if recipe_step_stop(&words) == Some(RecipeStop::PersonsTurn) {
            marks.push(RECIPE_MARK_PERSONS_TURN.to_string());
        }
        if !went {
            marks.push(format!(
                "{RECIPE_MARK_FAILED} {}",
                if value_positions.is_empty() {
                    saved.step.error.as_deref().unwrap_or("?")
                } else {
                    "input failed; values omitted"
                }
            ));
        }
        out.push_str(&format!(
            "{n}. {}\n",
            recipe_line_text(saved.tool, &words, &marks)
        ));
    }
    let external: Vec<_> = browser_tabs
        .values()
        .filter(|alias| !opened_aliases.contains(*alias))
        .collect();
    if !browser_tabs.is_empty() {
        out.push_str("\n## Browser tabs\n\n");
        for alias in browser_tabs.values() {
            if opened_aliases.contains(alias) {
                out.push_str(&format!("- `{alias}` — the tab this run opens.\n"));
            } else if alias == RECIPE_TAB_ALIAS {
                out.push_str(&format!(
                    "- `{alias}` — supply `--tab <browser-label>` for a tab you own.\n"
                ));
            } else {
                let name = alias.trim_start_matches('$');
                params.push((
                    name.into(),
                    format!("the browser label for {alias}, supplied explicitly"),
                ));
                out.push_str(&format!("- `{alias}` — supply `{name}` in `--params`.\n"));
            }
        }
    }
    if !params.is_empty() {
        out.push_str(&format!("\n{RECIPE_HEADING_PARAMS}\n\n"));
        for (param, said) in &params {
            out.push_str(&format!(
                "- `{RECIPE_PARAM_OPEN}{param}{RECIPE_PARAM_CLOSE}` — {said}\n"
            ));
        }
    }
    let example: String = params
        .iter()
        .filter(|(param, _)| !param.starts_with("secret_"))
        .map(|(param, ..)| format!("\"{param}\":\"…\""))
        .collect::<Vec<_>>()
        .join(",");
    let tab_arg = if external
        .iter()
        .any(|alias| alias.as_str() == RECIPE_TAB_ALIAS)
    {
        " --tab <browser-label>"
    } else {
        ""
    };
    let params_arg = if example.is_empty() {
        String::new()
    } else {
        format!(" --params '{{{example}}}'")
    };
    out.push_str(&format!(
        "\n## How to walk it\n\n`{COMPUTER_CLI} recipe-run --name {slug}{tab_arg}{params_arg}` walks every step in one call and answers where it stopped: the person's turn or last step, a step naming the saved screen's element, window or process, a check the screen fails, a step whose work did not happen, a press that changed nothing where it landed, a person's hand on the pointer, or a step that would outlast the call. Do that step with a lone command, then run again with `--start <next>`. Edit a line to change a step; delete it to drop one; keep each step on its own numbered line.\n",
        slug = recipe_slug(name),
    ));
    out
}

fn recipes_dir(root: &Path) -> PathBuf {
    root.join(RECIPES_DIR)
}

/// Save a recipe from the steps in `from` (an evidence folder). Answers the
/// file written.
pub fn save(
    root: &Path,
    name: &str,
    note: Option<&str>,
    from: &Path,
    last: Option<usize>,
    at_epoch_ms: i64,
) -> Result<PathBuf, ComputerUseError> {
    let slug = recipe_slug(name);
    if slug.is_empty() {
        return Err(ComputerUseError::new(
            error_code::INVALID_ARGUMENT,
            "--name needs at least one letter or digit",
        ));
    }
    let steps = crate::run_evidence::steps_in(from);
    if steps.is_empty() {
        return Err(ComputerUseError::new(
            error_code::INVALID_ARGUMENT,
            format!(
                "no steps to save in {} — walk something first",
                from.display()
            ),
        ));
    }
    let keep = last.unwrap_or(RECIPE_STEPS);
    let start = steps.len().saturating_sub(keep);
    validate_saved_values(name, note, &steps[start..])?;
    let dir = recipes_dir(root);
    std::fs::create_dir_all(&dir).map_err(|error| {
        ComputerUseError::new(error_code::ACCESSIBILITY_ERROR, error.to_string())
    })?;
    let file = dir.join(format!("{slug}.md"));
    std::fs::write(
        &file,
        recipe_markdown(name, note, &steps[start..], at_epoch_ms),
    )
    .map_err(|error| {
        ComputerUseError::new(
            error_code::ACCESSIBILITY_ERROR,
            format!("could not write {}: {error}", file.display()),
        )
    })?;
    Ok(file)
}

/// Every recipe on disk, newest first: `{name, file, modifiedMs}`.
#[must_use]
pub fn list(root: &Path) -> Vec<Value> {
    let Ok(entries) = std::fs::read_dir(recipes_dir(root)) else {
        return Vec::new();
    };
    let mut rows: Vec<(i64, Value)> = entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "md"))
        .filter_map(|entry| {
            let path = entry.path();
            let modified = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |age| i64::try_from(age.as_millis()).unwrap_or(i64::MAX));
            let name = path.file_stem()?.to_string_lossy().into_owned();
            Some((modified, serde_json::json!({ "name": name, "file": path.display().to_string(), "modifiedMs": modified })))
        })
        .collect();
    rows.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    rows.into_iter()
        .take(LIST_MAX)
        .map(|(_, row)| row)
        .collect()
}

/// A recipe's text, by the name a person gave it.
pub fn show(root: &Path, name: &str) -> Result<(PathBuf, String), ComputerUseError> {
    let file = recipes_dir(root).join(format!("{}.md", recipe_slug(name)));
    let text = std::fs::read_to_string(&file).map_err(|_| {
        ComputerUseError::new(
            error_code::INVALID_ARGUMENT,
            format!("no recipe named '{name}' ({})", file.display()),
        )
    })?;
    Ok((file, text))
}

// ---- Flow cards (t-4260, docs/design/flow-engine-operator-and-qa.md §2.7) --
//
// A Flow is a recipe document with two more sections (`## Flow`, `## Checks`,
// core `computer_flow`). The card is a thin surface over the document: it
// lists the recipes that are Flows, shows the last run's verdict, and rewrites
// the two sections when a person changes the policy or the evidence level. It
// keeps no store of its own — the recipe folder and the session folders are
// the truth — and it fixes no control value: `policy` and `evidence` are the
// core enums' `ALL`, so the card can never drift from the grammar.

/// A Flow as the card's roster reads it: the recipe document's own facts, and
/// the last run's verdict. Nothing here is the card's invention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FlowRow {
    pub slug: String,
    pub name: String,
    pub policy: &'static str,
    pub evidence: &'static str,
    pub fingerprint: FlowFingerprint,
    pub checks: FlowChecks,
    /// How the slug is re-run today, from the automation surface — the card
    /// owns no scheduler. Absent when nothing schedules it (it is run by hand).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    /// Present (`true`) only for a money Flow (`guarded`): the roster's one
    /// read of the policy, so the card need not re-encode which policy is
    /// money. The refusal a money Flow shows rides `FlowListing::policies`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub money: Option<bool>,
    /// The last run's verdict, or `null` when the slug has never run.
    pub last: Option<FlowLast>,
    /// The document itself, so the card's "문서 열기" opens the recipe.
    pub file: String,
}

/// The identity half a person reads on the card: the apps and hosts the
/// recipe is bound to (the protocol and assets stay in the document).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FlowFingerprint {
    pub apps: Vec<String>,
    pub hosts: Vec<String>,
}

/// How many oracle lines the Flow has, and how many a green verdict needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FlowChecks {
    pub n: usize,
    pub required: usize,
}

/// The last run of a slug: when, whether it passed (`null` when it left no
/// `qa-verdict.json`), the session folder, and its report when one was kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FlowLast {
    pub at: i64,
    pub pass: Option<bool>,
    pub dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
}

/// The card's whole answer: the Flows, and the two closed sets its controls
/// draw from — never a value the card fixed itself (docs §2.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FlowListing {
    pub flows: Vec<FlowRow>,
    pub policies: Vec<PolicyOption>,
    pub levels: Vec<&'static str>,
}

/// One policy segment the card draws: the enum's word, and whether a run may
/// walk under it — so the card says when `guarded` cannot run yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PolicyOption {
    pub value: &'static str,
    pub runnable: Runnable,
}

/// Whether this build runs a policy, as the card reads it: `ok`, and the
/// refusal code when it does not. Every policy runs since the money gate
/// landed (2nd wave A): a `guarded` document without its money line is
/// refused by the parser as a document, never here as a policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Runnable {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<&'static str>,
}

/// The policy segments the card draws — `Policy::ALL`, each runnable in this
/// build. The card fixes no value: this is the grammar's own set.
fn policy_options() -> Vec<PolicyOption> {
    Policy::ALL
        .into_iter()
        .map(|policy| PolicyOption {
            value: policy.as_str(),
            runnable: Runnable {
                ok: true,
                refusal: None,
            },
        })
        .collect()
}

/// The evidence segments the card draws — `EvidenceLevel::ALL`, in words.
fn evidence_levels() -> Vec<&'static str> {
    EvidenceLevel::ALL
        .into_iter()
        .map(EvidenceLevel::as_str)
        .collect()
}

/// A grammar table's words, comma-joined, for a refusal that names them.
fn words_of<T: Copy>(all: &[T], as_str: fn(T) -> &'static str) -> String {
    all.iter()
        .map(|row| as_str(*row))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The name a person gave a recipe — its `# title` line — when it has one.
fn recipe_title(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let title = line.trim().strip_prefix("# ")?.trim();
        (!title.is_empty()).then(|| title.to_string())
    })
}

/// Every recipe document that is a Flow, as the card's roster (newest first,
/// as `list` orders them): read from the recipe folder, with the last run's
/// verdict from the session folder that ran its slug. `trigger_of` answers how
/// a slug is re-run, from the automation surface — the card owns no scheduler.
#[must_use]
pub fn flow_list(root: &Path, trigger_of: impl Fn(&str) -> Option<String>) -> FlowListing {
    let last = last_runs(root);
    let flows = list(root)
        .into_iter()
        .filter_map(|entry| {
            let slug = entry.get("name")?.as_str()?.to_string();
            let file = entry.get("file")?.as_str()?.to_string();
            let text = std::fs::read_to_string(&file).ok()?;
            let spec = parse_flow(&text).ok()??;
            Some(flow_row(slug, file, &text, &spec, &last, &trigger_of))
        })
        .collect();
    FlowListing {
        flows,
        policies: policy_options(),
        levels: evidence_levels(),
    }
}

/// One Flow as a roster row.
fn flow_row(
    slug: String,
    file: String,
    text: &str,
    spec: &FlowSpec,
    last: &BTreeMap<String, FlowLast>,
    trigger_of: &impl Fn(&str) -> Option<String>,
) -> FlowRow {
    let required = spec.checks.iter().filter(|check| check.required).count();
    FlowRow {
        name: recipe_title(text).unwrap_or_else(|| slug.clone()),
        policy: spec.policy.as_str(),
        evidence: spec.evidence.as_str(),
        fingerprint: FlowFingerprint {
            apps: spec.fingerprint.apps.iter().cloned().collect(),
            hosts: spec.fingerprint.hosts.iter().cloned().collect(),
        },
        checks: FlowChecks {
            n: spec.checks.len(),
            required,
        },
        trigger: trigger_of(&slug),
        money: (spec.policy == Policy::Guarded).then_some(true),
        last: last.get(&slug).cloned(),
        file,
        slug,
    }
}

/// The last run of every slug: the newest session folder a walk named it in
/// (`walk-NNN.json` `name`, slugified the same way the file is), with that
/// folder's `qa-verdict.json` and its `report.html`. One pass over the
/// sessions, so a roster of many Flows does not reread the tree per Flow.
fn last_runs(root: &Path) -> BTreeMap<String, FlowLast> {
    let mut best: BTreeMap<String, FlowLast> = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(root.join(crate::computer_use::evidence::SESSIONS_DIR))
    else {
        return best;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        for (_, record) in crate::run_evidence::walks_in(&dir) {
            let Some(slug) = record
                .get("name")
                .and_then(Value::as_str)
                .map(recipe_slug)
                .filter(|slug| !slug.is_empty())
            else {
                continue;
            };
            let at = record
                .get("at_epoch_ms")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            if best.get(&slug).is_some_and(|held| held.at >= at) {
                continue;
            }
            let report = dir.join(crate::computer_use::report::REPORT_FILE);
            best.insert(
                slug,
                FlowLast {
                    at,
                    pass: super::evidence::read_verdict(&dir).map(|(pass, _)| pass),
                    dir: dir.display().to_string(),
                    report: report.is_file().then(|| report.display().to_string()),
                },
            );
        }
    }
    best
}

/// Read a Flow's document, change its policy and/or evidence, and rewrite the
/// two Flow sections with one atomic write — the recipe's own steps untouched.
/// A document that is not a Flow, or a word outside the core enums, is refused
/// by name. Answers the file written.
pub fn flow_set(
    root: &Path,
    slug: &str,
    policy: Option<&str>,
    evidence: Option<&str>,
) -> Result<PathBuf, ComputerUseError> {
    let (file, text) = show(root, slug)?;
    let mut spec = parse_flow(&text)
        .map_err(|why| ComputerUseError::new(error_code::INVALID_ARGUMENT, why))?
        .ok_or_else(|| {
            ComputerUseError::new(
                error_code::INVALID_ARGUMENT,
                format!(
                    "'{slug}' is a recipe, not a Flow — it has no {FLOW_HEADING} / {FLOW_HEADING_CHECKS}"
                ),
            )
        })?;
    if let Some(word) = policy {
        spec.policy = Policy::from_word(word).ok_or_else(|| {
            ComputerUseError::new(
                error_code::INVALID_ARGUMENT,
                format!(
                    "'{word}' is not a policy ({})",
                    words_of(&Policy::ALL, Policy::as_str)
                ),
            )
        })?;
    }
    if let Some(word) = evidence {
        spec.evidence = EvidenceLevel::from_word(word).ok_or_else(|| {
            ComputerUseError::new(
                error_code::INVALID_ARGUMENT,
                format!(
                    "'{word}' is not an evidence level ({})",
                    words_of(&EvidenceLevel::ALL, EvidenceLevel::as_str)
                ),
            )
        })?;
    }
    // Fail closed: the document as it would be written must read back as a
    // Flow under every contract the parser keeps (a `guarded` Flow needs its
    // money line, docs/design/flow-engine-guarded-money-path.md §1) — a card
    // cannot write a policy the document cannot carry.
    let next = splice_flow_sections(&text, &spec);
    parse_flow(&next).map_err(|why| {
        ComputerUseError::new(
            error_code::INVALID_ARGUMENT,
            format!("'{slug}' would not read back as a Flow: {why}"),
        )
    })?;
    crate::update_store::write_then_rename(&file, next.as_bytes()).map_err(|error| {
        ComputerUseError::new(
            error_code::ACCESSIBILITY_ERROR,
            format!("could not write {}: {error}", file.display()),
        )
    })?;
    Ok(file)
}

/// The document with its two Flow sections replaced by `spec.written()` and
/// everything else — the recipe's steps, its parameters, any prose — left
/// exactly as it was. The two sections' span runs from the first of the two
/// headings to the end of the section the later one opens; a trailing section
/// after them is kept. When the text has no Flow (it should, since `flow_set`
/// parsed one first), the sections are appended.
fn splice_flow_sections(text: &str, spec: &FlowSpec) -> String {
    let mut flow_at = None;
    let mut checks_at = None;
    let mut trigger_at = None;
    let mut heads: Vec<usize> = Vec::new();
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with("## ") {
            heads.push(offset);
            if trimmed == FLOW_HEADING {
                flow_at = Some(offset);
            } else if trimmed == FLOW_HEADING_CHECKS {
                checks_at = Some(offset);
            } else if trimmed == FLOW_HEADING_TRIGGER {
                trigger_at = Some(offset);
            }
        }
        offset += line.len();
    }
    let block = spec.written();
    let (Some(flow), Some(checks)) = (flow_at, checks_at) else {
        let mut out = text.trim_end().to_string();
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(block.trim_end());
        out.push('\n');
        return out;
    };
    let region_start = flow.min(checks).min(trigger_at.unwrap_or(flow));
    let later = flow.max(checks).max(trigger_at.unwrap_or(flow));
    let region_end = heads
        .iter()
        .copied()
        .find(|&head| head > later)
        .unwrap_or(text.len());
    let mut out = text[..region_start].trim_end().to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(block.trim_end());
    out.push('\n');
    let tail = text[region_end..].trim_start();
    if !tail.is_empty() {
        out.push('\n');
        out.push_str(tail);
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saving_a_whole_secret_reference_succeeds_while_a_literal_credential_refuses() {
        for (value, allowed) in [
            ("$secret_password", true),
            ("{{password}}", true),
            ("password=$secret_password", true),
            ("password=SYNTHETIC_INPUT_ONLY", false),
        ] {
            let root = tempfile::tempdir().unwrap();
            let evidence = root.path().join("evidence");
            std::fs::create_dir_all(&evidence).unwrap();
            let recorded = Step {
                tool: "browser".into(),
                ..step(1, &["type", "browser-1", "#reference", value], true)
            };
            std::fs::write(
                evidence.join("steps.jsonl"),
                format!("{}\n", serde_json::to_string(&recorded).unwrap()),
            )
            .unwrap();
            let result = save(root.path(), "References", None, &evidence, None, 1);
            assert_eq!(
                result.is_ok(),
                allowed,
                "references and literal values must be distinguished"
            );
            if let Ok(file) = result {
                assert!(
                    !std::fs::read_to_string(file)
                        .unwrap()
                        .contains("SYNTHETIC_INPUT_ONLY")
                );
            }
        }
    }

    #[test]
    fn a_secret_recipe_value_is_a_parameter_and_never_a_saved_literal() {
        let root = tempfile::tempdir().unwrap();
        let evidence = root.path().join("evidence");
        std::fs::create_dir_all(&evidence).unwrap();
        let payload = serde_json::json!({"#password":"SYNTHETIC_INPUT_ONLY"}).to_string();
        let recorded = Step {
            tool: "browser".into(),
            observation: Some(
                serde_json::json!({"recipeInput":{"secret":true,"credential":false}}),
            ),
            ..step(1, &["fill", "browser-1", &payload], true)
        };
        std::fs::write(
            evidence.join("steps.jsonl"),
            format!("{}\n", serde_json::to_string(&recorded).unwrap()),
        )
        .unwrap();
        let path = save(root.path(), "Login", None, &evidence, None, 1).unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        assert!(
            !text.contains("SYNTHETIC_INPUT_ONLY"),
            "a value escaped its parameter"
        );
        assert!(
            text.contains("$secret_"),
            "the missing value must be a person's step"
        );
    }

    fn save_browser_steps(name: &str, rows: &[&[&str]]) -> String {
        let root = tempfile::tempdir().unwrap();
        let evidence = root.path().join("evidence");
        std::fs::create_dir_all(&evidence).unwrap();
        let lines: Vec<String> = rows
            .iter()
            .enumerate()
            .map(|(at, argv)| {
                let recorded = Step {
                    tool: "browser".into(),
                    ..step(at + 1, argv, true)
                };
                serde_json::to_string(&recorded).unwrap()
            })
            .collect();
        std::fs::write(
            evidence.join("steps.jsonl"),
            format!("{}\n", lines.join("\n")),
        )
        .unwrap();
        let path = save(root.path(), name, None, &evidence, None, 1).unwrap();
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn a_saved_upload_names_its_files_by_parameter_and_never_by_path() {
        // An older log kept the whole path; a new one keeps the name only.
        for logged in ["/Users/dev/Documents/receipt.pdf", "receipt.pdf"] {
            let text = save_browser_steps(
                "Upload",
                &[
                    &["upload", "browser-1", "#receipt", logged],
                    &[
                        "upload",
                        "browser-1",
                        "#more",
                        "--drop",
                        logged,
                        "/Users/dev/Pictures/scan.png",
                    ],
                ],
            );
            assert!(!text.contains("/Users/dev"), "a folder was saved: {text}");
            assert!(!text.contains("receipt.pdf"), "a file was saved: {text}");
            assert!(
                text.contains("zerocode-browser upload $tab #receipt $file_1"),
                "{text}"
            );
            assert!(text.contains("#more --drop $file_2 $file_2_2"), "{text}");
            assert!(text.contains("- `{{file_1}}` — "), "{text}");
            let lines = zerocode_core::computer_recipe::recipe_lines(&text).unwrap();
            assert_eq!(
                zerocode_core::computer_recipe::params_of(&lines),
                ["file_1", "file_2", "file_2_2"]
            );
        }
    }

    #[test]
    fn a_saved_upload_keeps_a_parameter_it_was_walked_with() {
        let text = save_browser_steps(
            "Upload",
            &[&["upload", "browser-1", "#receipt", "$file_receipt"]],
        );
        assert!(text.contains("#receipt $file_receipt`"), "{text}");
        assert!(!text.contains("$file_1"), "{text}");
    }

    #[test]
    fn an_ordinary_recipe_field_cannot_save_a_card_shaped_value_or_a_credential_url() {
        for argv in [
            vec![
                "fill".to_string(),
                "browser-1".into(),
                serde_json::json!({"#reference": "4242".repeat(4)}).to_string(),
            ],
            vec![
                "goto".into(),
                "browser-1".into(),
                "https://example.test/?token=SYNTHETIC_INPUT_ONLY".into(),
            ],
        ] {
            let root = tempfile::tempdir().unwrap();
            let evidence = root.path().join("evidence");
            std::fs::create_dir_all(&evidence).unwrap();
            let refs: Vec<_> = argv.iter().map(String::as_str).collect();
            let recorded = Step {
                tool: "browser".into(),
                ..step(1, &refs, true)
            };
            std::fs::write(
                evidence.join("steps.jsonl"),
                format!("{}\n", serde_json::to_string(&recorded).unwrap()),
            )
            .unwrap();
            let refused = save(root.path(), "Entry", None, &evidence, None, 1)
                .expect_err("unsafe values must refuse the save");
            assert!(!refused.message.contains("SYNTHETIC_INPUT_ONLY"));
            assert!(!refused.message.contains(&"4242".repeat(4)));
            assert!(!root.path().join(RECIPES_DIR).join("entry.md").exists());
        }
    }

    #[test]
    fn browser_tab_alias_survives_save_without_the_recorded_label() {
        let opened = Step {
            tool: "browser".into(),
            observation: Some(serde_json::json!({ "browserOpenedTab": "browser-4" })),
            ..step(1, &["open", "https://example.test/work"], true)
        };
        let pressed = Step {
            tool: "browser".into(),
            ..step(2, &["click", "browser-4", "#continue"], true)
        };
        let text = recipe_markdown("Tabs", None, &[opened, pressed], 1);
        assert!(text.contains("opens $tab"), "{text}");
        assert!(text.contains("click $tab #continue"), "{text}");
        assert!(!text.contains("browser-4"), "{text}");
    }

    #[test]
    fn browser_tab_alias_resaving_a_tail_keeps_the_explicit_tab_instruction() {
        let pressed = Step {
            tool: "browser".into(),
            ..step(1, &["click", "$tab", "#continue"], true)
        };
        let text = recipe_markdown("Tail", None, &[pressed], 1);
        assert!(text.contains("--tab <browser-label>"), "{text}");
        assert!(text.contains("click $tab #continue"), "{text}");
    }

    /// The real recipe loop and saver, with its browser-script boundary in
    /// Chromium. Opt-in because the Rust gate need not install Node/Chromium.
    #[test]
    #[ignore = "offline benchmark: requires Node and Playwright Chromium"]
    fn expense_workflow_runs_and_replays_its_saved_recipe_in_one_call() {
        use super::super::recipe_run::{
            self,
            bench::{Bench, command, walk_of},
        };
        use std::io::{BufRead, BufReader, Write};
        use std::process::Stdio;

        struct Driver {
            child: std::process::Child,
            input: std::process::ChildStdin,
            output: BufReader<std::process::ChildStdout>,
        }
        impl Driver {
            fn read(&mut self) -> Value {
                let mut line = String::new();
                assert!(
                    self.output.read_line(&mut line).unwrap() > 0,
                    "browser driver ended"
                );
                serde_json::from_str(&line).expect("browser driver JSON")
            }
            fn ask(&mut self, argv: &[String]) -> zerocode_hookd::TeamAnswer {
                writeln!(self.input, "{}", serde_json::to_string(argv).unwrap()).unwrap();
                self.input.flush().unwrap();
                let reply = self.read();
                zerocode_hookd::TeamAnswer {
                    exit_code: i32::try_from(reply["exit_code"].as_i64().expect("exit code"))
                        .unwrap(),
                    stdout: reply["stdout"].as_str().expect("stdout").into(),
                    stderr: reply["stderr"].as_str().expect("stderr").into(),
                }
            }
        }
        impl Drop for Driver {
            fn drop(&mut self) {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut child = crate::proc::quiet_command("node")
            .arg(root.join("ui/tests/expense-workflow-driver.mjs"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("Node with Chromium");
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut driver = Driver {
            child,
            input,
            output,
        };
        let ready = driver.read();
        assert_eq!(ready["ready"], true);
        let period = r##"{"#period":"지난달"}"##;
        let details = r##"{"#date":"2027-02-03","#amount":"18500","#template":"지출결의 · 환급신청서","#approver":"팀 검토 → 재무 검토"}"##;
        let params = serde_json::json!({ "url": ready["url"], "receipt": ready["receipt"], "period": period, "details": details });
        let text = "# Synthetic expense\n\n## Steps\n\n\
1. `zerocode-browser open {{url}}`\n\
2. `zerocode-browser marks browser-801`\n\
3. `zerocode-browser click browser-801 #approval-menu`\n\
4. `zerocode-browser click browser-801 'a[href=\"#documents\"]'`\n\
5. `zerocode-browser wait browser-801 #period`\n\
6. `zerocode-browser fields browser-801`\n\
7. `zerocode-browser fill browser-801 {{period}}`\n\
8. `zerocode-browser click browser-801 #open-document`\n\
9. `zerocode-browser wait browser-801 #copy`\n\
10. `zerocode-browser click browser-801 #copy`\n\
11. `zerocode-browser wait browser-801 #expense`\n\
12. `zerocode-browser fields browser-801`\n\
13. `zerocode-browser fill browser-801 {{details}}`\n\
14. `zerocode-browser click browser-801 #attach`\n\
15. `zerocode-browser wait browser-801 '#uploader[open]'`\n\
16. `zerocode-browser fields browser-801`\n\
17. `zerocode-browser upload browser-801 #receipt {{receipt}}`\n\
18. `zerocode-browser click browser-801 #upload-done`\n\
19. `zerocode-browser click browser-801 #review`\n\
20. `zerocode-browser wait browser-801 #confirm`\n\
21. `zerocode-browser click browser-801 #confirm`\n\
22. `zerocode-browser wait browser-801 #complete`\n";
        let temp = tempfile::tempdir().unwrap();
        let evidence = temp.path().join("evidence");
        std::fs::create_dir_all(&evidence).unwrap();
        let mut records = Vec::new();
        let mut measurements = Vec::new();
        let mut opened_labels = Vec::new();
        for round in 0..2 {
            if round == 0 {
                assert_eq!(driver.ask(&["reset".into()]).exit_code, 0);
            } else {
                assert_eq!(
                    driver
                        .ask(&["close".into(), "browser-801".into()])
                        .exit_code,
                    0
                );
            }
            let (document, given) = if round == 0 {
                (text.to_string(), params.clone())
            } else {
                let (_, saved) = show(temp.path(), "Synthetic expense").unwrap();
                assert!(
                    saved.contains("zerocode-browser upload"),
                    "the attachment must survive save: {saved}"
                );
                let receipt = ready["receipt"].as_str().unwrap();
                assert!(
                    !saved.contains(receipt) && !saved.contains("receipt.txt"),
                    "the attachment's path must not survive save: {saved}"
                );
                let lines = zerocode_core::computer_recipe::recipe_lines(&saved).unwrap();
                let mut values = serde_json::Map::new();
                let mut fills = 0;
                for line in &lines {
                    if line.argv[0] == "upload" {
                        // The attachment is the replay's to be given.
                        let file = line.argv.last().unwrap();
                        assert!(
                            zerocode_core::computer_recipe::privacy::is_file_parameter(file),
                            "{file}"
                        );
                        values.insert(file[1..].to_string(), Value::String(receipt.into()));
                    }
                    if line.argv[0] == "fill" {
                        let name = zerocode_core::computer_recipe::recipe_placeholders(
                            line.argv.last().unwrap(),
                        );
                        values.insert(
                            name[0].clone(),
                            Value::String(if fills == 0 { period } else { details }.into()),
                        );
                        fills += 1;
                    }
                }
                (saved, Value::Object(values))
            };
            let given = given.to_string();
            let command = command(&["--params", &given]);
            let bench = Bench::new();
            let began = std::time::Instant::now();
            let mut count = 0;
            let report = recipe_run::run(
                &walk_of(&command, &document),
                |tool, argv, _| {
                    assert_eq!(tool, RecipeTool::Browser);
                    count += 1;
                    let answer = driver.ask(argv);
                    if argv[0] == "open" && answer.exit_code == 0 {
                        let label =
                            zerocode_core::agent_browser::opened_browser_label(&answer.stdout)
                                .unwrap();
                        opened_labels.push(label.to_string());
                        bench.show(&[(label, ready["url"].as_str().unwrap())]);
                    }
                    bench
                        .clock
                        .set(u64::try_from(began.elapsed().as_millis()).unwrap());
                    if round == 0 {
                        let words = crate::run_evidence::redacted("browser", argv);
                        let refs: Vec<_> = words.iter().map(String::as_str).collect();
                        records.push(Step {
                            tool: "browser".into(),
                            observation: crate::run_evidence::browser_open_observation(
                                argv, &answer, None,
                            ),
                            ..step(count, &refs, answer.exit_code == 0)
                        });
                    }
                    answer
                },
                |_, _, _| {},
                |_| {},
                bench.desk(false, None),
                || true,
            )
            .expect("recipe runner");
            assert_eq!(report["done"], true, "round {round}: {report}");
            let elapsed_ms = began.elapsed().as_secs_f64() * 1000.0;
            assert!(
                elapsed_ms < 30_000.0,
                "recipe exceeded 30 seconds: {elapsed_ms} ms"
            );
            let completed = driver.ask(&["verify".into()]);
            assert_eq!(completed.exit_code, 0);
            let state: Value = serde_json::from_str(&completed.stdout).unwrap();
            assert_eq!(state["submitted"], true);
            assert_eq!(state["amount"], "18500");
            assert_eq!(state["files"][0]["name"], "receipt.txt");
            measurements.push(serde_json::json!({ "round": round + 1, "recipeCalls": 1, "recipeLines": report["steps"], "browserSteps": count, "elapsedMs": elapsed_ms, "modelCalls": 0 }));
            if round == 0 {
                let jsonl = records
                    .iter()
                    .map(|step| serde_json::to_string(step).unwrap())
                    .collect::<Vec<_>>()
                    .join("\n");
                std::fs::write(evidence.join("steps.jsonl"), format!("{jsonl}\n")).unwrap();
                save(temp.path(), "Synthetic expense", None, &evidence, None, 1).unwrap();
            }
        }
        assert_eq!(
            opened_labels,
            ["browser-801", "browser-802"],
            "the replay must open a different tab"
        );
        println!("expense recipe bench: {}", serde_json::json!(measurements));
        if let Some(out) = std::env::var_os("ZC_EXPENSE_EVIDENCE") {
            let out = PathBuf::from(out);
            std::fs::create_dir_all(&out).unwrap();
            std::fs::write(
                out.join("recipe-measurements.json"),
                serde_json::to_string_pretty(&measurements).unwrap(),
            )
            .unwrap();
            let (_, saved) = show(temp.path(), "Synthetic expense").unwrap();
            std::fs::write(out.join("saved-recipe.md"), saved).unwrap();
            assert_eq!(
                driver
                    .ask(&["snapshot".into(), out.to_string_lossy().into_owned()])
                    .exit_code,
                0
            );
        }
        assert_eq!(driver.ask(&["finish".into()]).exit_code, 0);
        assert!(driver.child.wait().unwrap().success());
    }

    fn step(n: usize, argv: &[&str], ok: bool) -> Step {
        Step {
            n,
            at_epoch_ms: 1_000 + n as i64,
            tool: "computer".into(),
            verb: argv[0].to_string(),
            argv: argv.iter().map(|w| (*w).to_string()).collect(),
            ok,
            error: (!ok).then(|| "stopped".to_string()),
            code: None,
            acts: false,
            shot: None,
            frame: None,
            observation: None,
            frame_skipped: None,
        }
    }

    #[test]
    fn a_recipe_is_the_walked_steps_with_the_persons_points_called_out() {
        let steps = vec![
            step(1, &["launch", "--app", "Safari", "--json"], true),
            step(2, &["status", "--json"], true),
            step(
                3,
                &[
                    "mouse-click",
                    "--x",
                    "1",
                    "--y",
                    "2",
                    "--confirming",
                    "payment",
                    "--json",
                ],
                true,
            ),
            step(4, &["handoff", "--reason", "2FA", "--json"], true),
            step(5, &["type", "--text", "[6 chars]", "--json"], false),
        ];
        let text = recipe_markdown(
            "쿠팡 결제까지",
            Some("장바구니는 비어 있어야"),
            &steps,
            5_000,
        );
        assert!(text.starts_with("# 쿠팡 결제까지\n"));
        assert!(
            text.contains("- steps: 4\n"),
            "housekeeping is not a step: {text}"
        );
        assert!(text.contains("- note: 장바구니는 비어 있어야"));
        assert!(
            text.contains("1. `zerocode-computer launch --app Safari`"),
            "{text}"
        );
        assert!(text.contains("2. `zerocode-computer mouse-click --x 1 --y 2 --confirming payment` — the person's last step"));
        assert!(text.contains("3. `zerocode-computer handoff --reason 2FA` — the person's turn"));
        assert!(
            text.contains(
                "4. `zerocode-computer type --text {{text-4}}` — failed then: input failed; values omitted"
            ),
            "a failed input still keeps its value out of the document: {text}"
        );
    }

    /// A recipe is machine-exact: whole words (a name with a space is one),
    /// what was typed a named parameter, only the desktop's steps — and it
    /// reads back into the commands it was written from.
    #[test]
    fn a_recipe_keeps_whole_words_names_typed_text_and_reads_back() {
        let mut browser = step(4, &["goto", "--url", "https://x"], true);
        browser.tool = "browser".into();
        let steps = vec![
            step(1, &["launch", "--app", "Google Chrome", "--json"], true),
            step(2, &["verdict", "--pass", "--json"], true),
            step(3, &["type", "--text", "[6 chars]", "--json"], true),
            browser,
            step(
                5,
                &[
                    "wait-for",
                    "--app",
                    "Mail",
                    "--text",
                    "Sent to Kim",
                    "--timeout-ms",
                    "5000",
                    "--json",
                ],
                true,
            ),
        ];
        let text = recipe_markdown("Mail", None, &steps, 1);
        assert!(
            text.contains("- steps: 4\n"),
            "the verdict is not a step; the browser's is, through its own door: {text}"
        );
        assert!(
            text.contains("3. `zerocode-browser goto --url https://x`\n"),
            "{text}"
        );
        assert!(
            text.contains("1. `zerocode-computer launch --app \"Google Chrome\"`"),
            "{text}"
        );
        assert!(
            text.contains("2. `zerocode-computer type --text {{text-2}}`"),
            "{text}"
        );
        assert!(text.contains(&format!("{RECIPE_HEADING_PARAMS}\n\n- `{{{{text-2}}}}` — typed at step 2 (6 characters when saved)")), "{text}");
        assert!(
            text.contains("recipe-run --name mail --params '{\"text-2\":\"…\"}'"),
            "{text}"
        );
        let lines = zerocode_core::computer_recipe::recipe_lines(&text).expect("it reads back");
        assert_eq!(
            lines
                .iter()
                .map(|line| line.argv.clone())
                .collect::<Vec<_>>(),
            [
                vec!["launch".to_string(), "--app".into(), "Google Chrome".into()],
                vec!["type".into(), "--text".into(), "{{text-2}}".into()],
                vec!["goto".into(), "--url".into(), "https://x".into()],
                vec![
                    "wait-for".into(),
                    "--app".into(),
                    "Mail".into(),
                    "--text".into(),
                    "Sent to Kim".into(),
                    "--timeout-ms".into(),
                    "5000".into()
                ],
            ]
        );
    }

    /// Saving a session a recipe walked keeps the recipe's words, not the
    /// walk's: its flags dropped, its placeholders kept and listed, a press
    /// the helper handed back the person's last step (one step, when the
    /// person's own press of it follows), a line break never splitting a step.
    #[test]
    fn a_walked_session_saves_back_into_the_same_recipe() {
        let handed_back = |n| {
            Step {
            error: Some(
                serde_json::json!({ "ok": false, "error": { "code": "confirmation_required", "message": "payment: Place order" } })
                    .to_string(),
            ),
            ..step(n, &["mouse-click", "--x", "900", "--y", "600", "--json"], false)
        }
        };
        let steps = vec![
            step(1, &["type", "--text", "{{to}}", "--json"], true),
            step(
                2,
                &[
                    "click",
                    "--app",
                    "Mail",
                    "--x",
                    "1",
                    "--y",
                    "2",
                    "--json",
                    "--no-screenshot",
                ],
                true,
            ),
            handed_back(3),
            step(
                4,
                &[
                    "handoff",
                    "--reason",
                    "Enter the code.\nThen press Continue.",
                    "--json",
                ],
                true,
            ),
            handed_back(5),
            step(
                6,
                &["mouse-click", "--x", "900", "--y", "600", "--json"],
                true,
            ),
            step(
                7,
                &["key", "--key", "return", "--allow-self", "--json"],
                true,
            ),
        ];
        let text = recipe_markdown("Pay", None, &steps, 1);
        assert!(
            text.contains("- steps: 6\n"),
            "the handed-back press and the person's press are one: {text}"
        );
        assert!(
            text.contains("1. `zerocode-computer type --text {{to}}`"),
            "{text}"
        );
        assert!(
            text.contains("- `{{to}}` — given at step 1 when saved"),
            "{text}"
        );
        assert!(
            text.contains("2. `zerocode-computer click --app Mail --x 1 --y 2`\n"),
            "the walk's flags are not the recipe's: {text}"
        );
        assert!(
            text.contains("3. `zerocode-computer mouse-click --x 900 --y 600 --confirming payment` — the person's last step"),
            "a press handed back is the person's, never `failed then`: {text}"
        );
        assert!(text.contains("5. `zerocode-computer mouse-click --x 900 --y 600 --confirming payment` — the person's last step"), "{text}");
        assert!(
            text.contains("6. `zerocode-computer key --key return`\n"),
            "{text}"
        );
        assert!(!text.contains(RECIPE_MARK_FAILED), "{text}");
        let lines = zerocode_core::computer_recipe::recipe_lines(&text).expect("it reads back");
        assert_eq!(lines.len(), 6, "every step on its own line");
        assert_eq!(lines[3].argv[2], "Enter the code. Then press Continue.");
        let planned = zerocode_core::computer_recipe::preflight(&lines, 3, &serde_json::Map::new())
            .expect("planned");
        assert_eq!(
            planned.planned[0].1,
            zerocode_core::computer_recipe::Plan::Stop(RecipeStop::PersonsLastStep),
            "the replay stops before the person's press"
        );
    }

    /// The person's step a walk stopped at is kept whether they did it by
    /// hand or with a lone command (then one step, looks between aside), and
    /// typed text never takes a name a walked line already carries.
    #[test]
    fn the_persons_step_survives_a_re_save_once_and_typed_names_never_collide() {
        let stopped = |n, argv: &[&str], kind: &str| {
            Step {
            error: Some(
                serde_json::json!({ "ok": false, "error": { "code": "recipe_stopped", "message": kind } })
                    .to_string(),
            ),
            ..step(n, argv, false)
        }
        };
        let handed_back = |n| {
            Step {
            error: Some(
                serde_json::json!({ "ok": false, "error": { "code": "confirmation_required", "message": "payment: Pay" } })
                    .to_string(),
            ),
            ..step(n, &["mouse-click", "--x", "9", "--y", "6", "--json"], false)
        }
        };
        let steps = vec![
            step(1, &["type", "--text", "{{text-5}}", "--json"], true),
            // The walk stopped for the 2FA; the person typed it by hand.
            stopped(2, &["handoff", "--reason", "2FA", "--json"], "persons_turn"),
            // The walk stopped again; this time the model handed it over itself.
            stopped(3, &["handoff", "--reason", "2FA", "--json"], "persons_turn"),
            step(
                4,
                &[
                    "handoff",
                    "--reason",
                    "Enter the code from your phone",
                    "--json",
                ],
                true,
            ),
            handed_back(5),
            step(6, &["screenshot", "--json"], true),
            step(
                7,
                &[
                    "mouse-click",
                    "--x",
                    "9",
                    "--y",
                    "6",
                    "--confirming",
                    "payment",
                    "--json",
                ],
                true,
            ),
            step(8, &["type", "--text", "[6 chars]", "--json"], true),
        ];
        let text = recipe_markdown("Pay", None, &steps, 1);
        assert!(!text.contains(RECIPE_MARK_FAILED), "{text}");
        assert!(
            text.contains("2. `zerocode-computer handoff --reason 2FA` — the person's turn"),
            "done by hand, still the person's: {text}"
        );
        assert!(
            text.contains("3. `zerocode-computer handoff --reason \"Enter the code from your phone\"` — the person's turn"),
            "one turn, in the words last said: {text}"
        );
        assert!(
            text.contains("4. `zerocode-computer mouse-click --x 9 --y 6 --confirming payment` — the person's last step"),
            "one press across the look between: {text}"
        );
        assert!(text.contains("5. `zerocode-computer screenshot`"), "{text}");
        assert!(
            text.contains("6. `zerocode-computer type --text {{text-6}}`"),
            "{text}"
        );
        assert!(text.contains("- steps: 6\n"), "{text}");
        let steps = vec![
            step(1, &["type", "--text", "{{text-2}}", "--json"], true),
            step(2, &["type", "--text", "[3 chars]", "--json"], true),
        ];
        let text = recipe_markdown("Clash", None, &steps, 1);
        assert!(
            text.contains("2. `zerocode-computer type --text {{text-2-2}}`"),
            "a walked line's name is not taken twice: {text}"
        );
        assert_eq!(
            zerocode_core::computer_recipe::params_of(
                &zerocode_core::computer_recipe::recipe_lines(&text).expect("reads back")
            ),
            ["text-2", "text-2-2"]
        );
    }

    #[test]
    fn a_recipe_is_saved_listed_and_shown_by_its_slug() {
        let root = tempfile::tempdir().expect("tempdir");
        let evidence = root.path().join("evidence");
        std::fs::create_dir_all(&evidence).unwrap();
        for n in 0..3 {
            crate::run_evidence::record(
                &evidence,
                1_000 + n,
                "computer",
                &["mouse-move".to_string()],
                Ok(()),
                crate::run_evidence::Framing::None,
            );
        }
        assert!(
            save(root.path(), "---", None, &evidence, None, 1).is_err(),
            "a name with a letter"
        );
        assert!(
            save(
                root.path(),
                "Empty",
                None,
                &root.path().join("nowhere"),
                None,
                1
            )
            .is_err(),
            "no steps, no recipe"
        );
        let file = save(
            root.path(),
            "Coupang checkout",
            Some("note"),
            &evidence,
            Some(2),
            9_000,
        )
        .expect("saved");
        assert!(file.ends_with("coupang-checkout.md"));
        let listed = list(root.path());
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["name"], "coupang-checkout");
        let (shown, text) = show(root.path(), "Coupang Checkout").expect("shown by any spelling");
        assert_eq!(shown, file);
        assert!(text.contains("- steps: 2\n"), "the last two: {text}");
        assert!(show(root.path(), "nothing").is_err());
    }

    /// A run's browser steps are the recipe's too (review 20): every browser
    /// line that left evidence is kept, in the door's own word, a typed value
    /// a named parameter like the desktop's — and the walk sends each one
    /// down the browser road, filled, logging the recipe's own words.
    #[test]
    fn a_recipe_keeps_browser_steps_and_replays_them_through_the_browser_door() {
        use super::super::recipe_run::bench::{
            self, Bench, browser_refused, browser_said, command, ok, walk_of, words,
        };
        use zerocode_core::computer_recipe::RecipeTool;
        let browser = |n, argv: &[&str]| Step {
            tool: "browser".into(),
            ..step(n, argv, true)
        };
        let steps = vec![
            browser(1, &["goto", "browser-1", "https://stg.example/login"]),
            browser(2, &["tabs"]),
            browser(3, &["click", "browser-1", "#login"]),
            browser(4, &["type", "browser-1", "#pw", "[6 chars]"]),
            step(5, &["key", "--key", "return", "--json"], true),
        ];
        let text = recipe_markdown("Web login", None, &steps, 1);
        assert!(
            text.contains("- steps: 4\n"),
            "a listing leaves no evidence and is no step: {text}"
        );
        assert!(
            text.contains("1. `zerocode-browser goto $tab https://stg.example/login`\n"),
            "{text}"
        );
        assert!(
            text.contains("2. `zerocode-browser click $tab #login`\n"),
            "{text}"
        );
        assert!(
            text.contains("3. `zerocode-browser type $tab #pw {{text-3}}`\n"),
            "what was typed is the next run's to give: {text}"
        );
        assert!(
            text.contains("4. `zerocode-computer key --key return`\n"),
            "{text}"
        );
        let lines = zerocode_core::computer_recipe::recipe_lines(&text).expect("reads back");
        assert_eq!(
            lines.iter().map(|line| line.tool).collect::<Vec<_>>(),
            [
                RecipeTool::Browser,
                RecipeTool::Browser,
                RecipeTool::Browser,
                RecipeTool::Computer
            ]
        );
        let bench = Bench::new().showing(&[("browser-1", "https://stg.example/login")]);
        let command = command(&["--tab", "browser-1", "--params", r#"{"text-3":"hunter2"}"#]);
        let mut roads: Vec<(RecipeTool, Vec<String>, Vec<String>)> = Vec::new();
        let report = super::super::recipe_run::run(
            &walk_of(&command, &text),
            |tool, argv, logged| {
                roads.push((tool, argv.to_vec(), logged.to_vec()));
                match tool {
                    RecipeTool::Browser => browser_said("done"),
                    RecipeTool::Computer => ok(),
                    RecipeTool::Emulator => unreachable!("no emulator step"),
                }
            },
            |_, _, _| {},
            |_| {},
            bench.desk(false, None),
            || true,
        )
        .expect("walked");
        assert_eq!(report["done"], true, "{report}");
        assert_eq!(
            roads.len(),
            5,
            "a newly supplied tab is read before its first change"
        );
        assert_eq!(roads[0].1, words(&["read", "browser-1"]));
        assert_eq!(
            roads[1].1,
            words(&["goto", "browser-1", "https://stg.example/login"]),
            "a browser line carries no walk flags"
        );
        assert_eq!(
            (roads[3].0, roads[3].1.clone(), roads[3].2[3].as_str()),
            (
                RecipeTool::Browser,
                words(&["type", "browser-1", "#pw", "hunter2"]),
                "{{text-3}}"
            ),
            "the value goes down the road; the log keeps the recipe's word"
        );
        assert_eq!(roads[4].0, RecipeTool::Computer);
        assert_eq!(report["ran"][0]["tool"], "browser");
        assert_eq!(report["ran"][3]["tool"], "computer");

        // A browser `wait` among the steps is a visibility wait, not a check
        // the walk judges (review 20): refused by the door, the step failed.
        let waiting = "# Wait\n\n## Steps\n\n1. `zerocode-browser wait browser-1 #done`\n2. `zerocode-computer key --key a`\n";
        let plain = bench::command(&[]);
        let report = super::super::recipe_run::run(
            &walk_of(&plain, waiting),
            |_, _, _| browser_refused(crate::cmd::browser::WAIT_TIMED_OUT),
            |_, _, _| {},
            |_| {},
            bench.desk(false, None),
            || true,
        )
        .expect("walked");
        assert_eq!(report["stop"]["kind"], "step_failed", "{report}");
        assert!(report["ran"][0].get("check").is_none());
        assert_eq!(report["next"], 1);
    }

    /// A run's emulator steps are the recipe's too: every emulator line that
    /// left evidence is kept, in the door's own word, a typed value a named
    /// parameter like the desktop's — and the walk sends each one down the
    /// emulator road, filled, logging the recipe's own words.
    #[test]
    fn a_recipe_keeps_emulator_steps_and_replays_them_through_the_emulator_door() {
        use super::super::recipe_run::bench::{Bench, command, ok, said, walk_of, words};
        use zerocode_core::computer_recipe::RecipeTool;
        let emulator = |n, argv: &[&str]| Step {
            tool: "emulator".into(),
            ..step(n, argv, true)
        };
        let steps = vec![
            emulator(
                1,
                &[
                    "tap",
                    "--platform",
                    "ios",
                    "--device",
                    "phone",
                    "--x",
                    "0.5",
                    "--y",
                    "0.5",
                ],
            ),
            emulator(2, &["tree", "--platform", "ios", "--device", "phone"]),
            emulator(
                3,
                &[
                    "text",
                    "--platform",
                    "ios",
                    "--device",
                    "phone",
                    "--text",
                    "[6 chars]",
                ],
            ),
            step(4, &["key", "--key", "return", "--json"], true),
        ];
        let text = recipe_markdown("Phone login", None, &steps, 1);
        assert!(
            text.contains("- steps: 3\n"),
            "a tree reads and leaves no evidence — no step: {text}"
        );
        assert!(
            text.contains(
                "1. `zerocode-emulator tap --platform ios --device phone --x 0.5 --y 0.5`\n"
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "2. `zerocode-emulator text --platform ios --device phone --text {{text-2}}`\n"
            ),
            "what was typed is the next run's to give: {text}"
        );
        assert!(
            text.contains("3. `zerocode-computer key --key return`\n"),
            "{text}"
        );
        let lines = zerocode_core::computer_recipe::recipe_lines(&text).expect("reads back");
        assert_eq!(
            lines.iter().map(|line| line.tool).collect::<Vec<_>>(),
            [
                RecipeTool::Emulator,
                RecipeTool::Emulator,
                RecipeTool::Computer
            ]
        );
        let bench = Bench::new();
        let command = command(&["--params", r#"{"text-2":"hunter2"}"#]);
        let mut roads: Vec<(RecipeTool, Vec<String>, Vec<String>)> = Vec::new();
        let report = super::super::recipe_run::run(
            &walk_of(&command, &text),
            |tool, argv, logged| {
                roads.push((tool, argv.to_vec(), logged.to_vec()));
                match tool {
                    RecipeTool::Emulator => said(&serde_json::json!({ "performed": true })),
                    RecipeTool::Computer => ok(),
                    RecipeTool::Browser => unreachable!("no browser step"),
                }
            },
            |_, _, _| {},
            |_| {},
            bench.desk(false, None),
            || true,
        )
        .expect("walked");
        assert_eq!(report["done"], true, "{report}");
        assert_eq!(roads.len(), 3);
        assert_eq!(
            roads[0].1,
            words(&[
                "tap",
                "--platform",
                "ios",
                "--device",
                "phone",
                "--x",
                "0.5",
                "--y",
                "0.5",
                "--json"
            ]),
            "an emulator line is sent with --json so the door answers an envelope"
        );
        assert_eq!(
            (roads[1].0, roads[1].1.clone(), roads[1].2[6].as_str()),
            (
                RecipeTool::Emulator,
                words(&[
                    "text",
                    "--platform",
                    "ios",
                    "--device",
                    "phone",
                    "--text",
                    "hunter2",
                    "--json"
                ]),
                "{{text-2}}"
            ),
            "the value goes down the road; the log keeps the recipe's word"
        );
        assert_eq!(roads[2].0, RecipeTool::Computer);
        assert_eq!(report["ran"][0]["tool"], "emulator");
        assert_eq!(report["ran"][2]["tool"], "computer");
    }

    /// How long saving takes for a recipe of emulator steps — printed, not
    /// checked (plan §5 numbers).
    #[test]
    #[ignore = "a measurement, printed; not a check"]
    fn measure_emulator_recipe_save() {
        let steps: Vec<Step> = (1..=100)
            .map(|n| Step {
                tool: "emulator".into(),
                ..step(
                    n,
                    &[
                        "tap",
                        "--platform",
                        "android",
                        "--device",
                        "phone",
                        "--x",
                        "0.5",
                        "--y",
                        "0.5",
                        "--json",
                    ],
                    true,
                )
            })
            .collect();
        let began = std::time::Instant::now();
        let rounds: u32 = 200;
        let mut bytes = 0;
        for _ in 0..rounds {
            bytes += recipe_markdown("Measure", None, &steps, 1).len();
        }
        let per = began.elapsed().as_secs_f64() * 1_000.0 / f64::from(rounds);
        eprintln!(
            "recipe_markdown, 100 emulator steps: {per:.3} ms per save ({} bytes)",
            bytes / usize::try_from(rounds).unwrap_or(1)
        );
    }

    /// The Flow card's roster: only the recipe documents that are Flows, each
    /// with its own facts, and — from the session folder its slug ran in —
    /// the last run's verdict. A plain recipe is not on the roster; the
    /// policy and evidence controls are the core enums, never fixed here.
    #[test]
    fn flow_list_reads_only_documents_with_a_flow_and_their_last_verdict() {
        use zerocode_core::computer_use::COMPUTER_USE_PROTOCOL_VERSION;
        let root = tempfile::tempdir().expect("tempdir");
        let recipes = root.path().join(RECIPES_DIR);
        std::fs::create_dir_all(&recipes).unwrap();
        // A Flow: a recipe with the two extra sections.
        std::fs::write(
            recipes.join("wallet-transfer.md"),
            format!(
                "# Wallet 송금 QA\n\n{RECIPE_HEADING_STEPS}\n\n\
                 1. `zerocode-computer launch --app Safari`\n\n\
                 {FLOW_HEADING}\n\n\
                 - policy: dry\n\
                 - evidence: full\n\
                 - fingerprint: apps=com.apple.Safari hosts=stg-admin.example.internal protocol={COMPUTER_USE_PROTOCOL_VERSION}\n\n\
                 {FLOW_HEADING_CHECKS}\n\n\
                 1. `zerocode-computer wait-for --app Safari --text 완료` — state, required\n\
                 2. `zerocode-computer wait-for --app Safari --text 로그인` — state, optional\n"
            ),
        )
        .unwrap();
        // A plain recipe: steps only, no Flow — not on the roster.
        std::fs::write(
            recipes.join("open-mail.md"),
            format!(
                "# 메일 열기\n\n{RECIPE_HEADING_STEPS}\n\n1. `zerocode-computer key --key tab`\n"
            ),
        )
        .unwrap();

        // A session whose walk ran the Flow's slug, with a verdict and report.
        let dir = root
            .path()
            .join(crate::computer_use::evidence::SESSIONS_DIR)
            .join("20260914-020000-1");
        std::fs::create_dir_all(&dir).unwrap();
        crate::run_evidence::record_walk(
            &dir,
            &serde_json::json!({ "kind": "recipe-run", "name": "wallet-transfer", "at_epoch_ms": 5_000 }),
        )
        .unwrap();
        crate::computer_use::evidence::write_verdict(
            &dir,
            &["verdict".to_string(), "--pass".to_string()],
            true,
            None,
            5_000,
        )
        .unwrap();
        std::fs::write(
            dir.join(crate::computer_use::report::REPORT_FILE),
            b"<!doctype html>",
        )
        .unwrap();

        let listing = flow_list(root.path(), |_| None);
        assert_eq!(listing.flows.len(), 1, "only the Flow: {:?}", listing.flows);
        let flow = &listing.flows[0];
        assert_eq!(flow.slug, "wallet-transfer");
        assert_eq!(flow.name, "Wallet 송금 QA");
        assert_eq!((flow.policy, flow.evidence), ("dry", "full"));
        assert_eq!(flow.fingerprint.apps, ["com.apple.Safari"]);
        assert_eq!(flow.fingerprint.hosts, ["stg-admin.example.internal"]);
        assert_eq!((flow.checks.n, flow.checks.required), (2, 1));
        assert_eq!(flow.money, None, "a dry Flow is not money");
        let last = flow.last.as_ref().expect("a last run for the slug");
        assert_eq!((last.at, last.pass), (5_000, Some(true)));
        assert!(last.report.is_some(), "the report was kept: {last:?}");
        assert!(last.dir.ends_with("20260914-020000-1"), "{}", last.dir);
        // The controls are the core enums' words, drawn — not fixed here.
        assert_eq!(
            listing.levels,
            EvidenceLevel::ALL
                .iter()
                .map(|level| level.as_str())
                .collect::<Vec<_>>()
        );
        let guarded = listing
            .policies
            .iter()
            .find(|policy| policy.value == "guarded")
            .expect("guarded is one of the policies");
        assert!(
            guarded.runnable.ok && guarded.runnable.refusal.is_none(),
            "guarded runs in this build — its contracts landed with the money gate: {guarded:?}"
        );
    }

    /// `flow_set` reads the document, changes the policy and evidence, and
    /// rewrites only the two Flow sections — the recipe's own steps stay word
    /// for word — and refuses a word outside the enums, or a plain recipe.
    #[test]
    fn flow_set_rewrites_the_two_flow_sections_and_leaves_the_steps_alone() {
        use zerocode_core::computer_use::COMPUTER_USE_PROTOCOL_VERSION;
        let root = tempfile::tempdir().expect("tempdir");
        let recipes = root.path().join(RECIPES_DIR);
        std::fs::create_dir_all(&recipes).unwrap();
        let doc = format!(
            "# 송금\n\n{RECIPE_HEADING_STEPS}\n\n\
             1. `zerocode-computer launch --app \"Google Chrome\"`\n\
             2. `zerocode-computer type --text {{{{amount}}}}` — money\n\n\
             {FLOW_HEADING}\n\n\
             - policy: dry\n\
             - evidence: full\n\
             - fingerprint: apps=com.google.Chrome hosts=stg.example protocol={COMPUTER_USE_PROTOCOL_VERSION}\n\
             - money: id=txn amount=amount recipient=recipient\n\n\
             {FLOW_HEADING_CHECKS}\n\n\
             1. `zerocode-computer wait-for --app \"Google Chrome\" --text 완료` — event, required\n"
        );
        // A Flow without a money line cannot become `guarded`: refused by the
        // parser's contract, and the file is left exactly as it was.
        let plain = doc
            .replace(" — money\n", "\n")
            .replace("- money: id=txn amount=amount recipient=recipient\n", "");
        std::fs::write(recipes.join("plain.md"), &plain).unwrap();
        let refused = flow_set(root.path(), "plain", Some("guarded"), None).expect_err("refused");
        assert!(refused.message.contains("money"), "{refused:?}");
        assert_eq!(
            std::fs::read_to_string(recipes.join("plain.md")).unwrap(),
            plain
        );
        let file = recipes.join("song-geum.md");
        std::fs::write(&file, &doc).unwrap();
        let steps_before = zerocode_core::computer_recipe::recipe_lines(&doc).expect("a recipe");

        let written = flow_set(
            root.path(),
            "song-geum",
            Some("guarded"),
            Some("verdict-only"),
        )
        .expect("set");
        assert_eq!(written, file);
        let after = std::fs::read_to_string(&file).unwrap();
        let spec = parse_flow(&after)
            .expect("still readable")
            .expect("still a Flow");
        assert_eq!(spec.policy, Policy::Guarded);
        assert_eq!(spec.evidence, EvidenceLevel::VerdictOnly);
        // The recipe's own steps are untouched, word for word.
        let steps_after =
            zerocode_core::computer_recipe::recipe_lines(&after).expect("still a recipe");
        assert_eq!(
            steps_after
                .iter()
                .map(|line| line.argv.clone())
                .collect::<Vec<_>>(),
            steps_before
                .iter()
                .map(|line| line.argv.clone())
                .collect::<Vec<_>>(),
            "the steps stay: {after}"
        );
        assert!(after.contains("# 송금\n"), "the title stayed: {after}");
        // A word outside the enums, and a plain recipe, are refused by name.
        assert!(flow_set(root.path(), "song-geum", Some("wet"), None).is_err());
        std::fs::write(
            recipes.join("plain.md"),
            format!("# 평범\n\n{RECIPE_HEADING_STEPS}\n\n1. `zerocode-computer key --key a`\n"),
        )
        .unwrap();
        assert!(
            flow_set(root.path(), "plain", Some("dry"), None).is_err(),
            "a recipe with no Flow section is not a Flow"
        );
    }

    /// How long `flow_list` takes over fifty Flow documents, each with a
    /// session folder that ran its slug — printed, not checked (§5 numbers).
    /// Run by hand:
    /// `cargo test -p zerocode-shell --bin zerocode-shell -- --ignored --nocapture flow_list_over_fifty`.
    #[test]
    #[ignore = "a measurement, printed; not a check"]
    fn measure_flow_list_over_fifty_flows() {
        use zerocode_core::computer_use::COMPUTER_USE_PROTOCOL_VERSION;
        const FLOWS: usize = 50;
        let root = tempfile::tempdir().expect("tempdir");
        let recipes = root.path().join(RECIPES_DIR);
        std::fs::create_dir_all(&recipes).unwrap();
        for n in 0..FLOWS {
            std::fs::write(
                recipes.join(format!("flow-{n}.md")),
                format!(
                    "# Flow {n}\n\n{RECIPE_HEADING_STEPS}\n\n\
                     1. `zerocode-computer launch --app Safari`\n\n\
                     {FLOW_HEADING}\n\n\
                     - policy: {}\n\
                     - evidence: full\n\
                     - fingerprint: apps=com.example.app{n} hosts=host-{n}.example protocol={COMPUTER_USE_PROTOCOL_VERSION}\n\n\
                     {FLOW_HEADING_CHECKS}\n\n\
                     1. `zerocode-computer wait-for --app Safari --text done` — state, required\n",
                    if n % 2 == 0 { "dry" } else { "guarded" },
                ),
            )
            .unwrap();
            let dir = root
                .path()
                .join(crate::computer_use::evidence::SESSIONS_DIR)
                .join(format!("20260914-0000{n:02}-1"));
            std::fs::create_dir_all(&dir).unwrap();
            crate::run_evidence::record_walk(
                &dir,
                &serde_json::json!({ "kind": "recipe-run", "name": format!("flow-{n}"), "at_epoch_ms": 1_000 + n as i64 }),
            )
            .unwrap();
            crate::computer_use::evidence::write_verdict(
                &dir,
                &["verdict".to_string(), "--pass".to_string()],
                n % 2 == 0,
                None,
                1_000 + n as i64,
            )
            .unwrap();
        }
        let rounds = 20;
        let mut runs: Vec<u128> = (0..rounds)
            .map(|_| {
                let began = std::time::Instant::now();
                let listing = flow_list(root.path(), |_| None);
                assert_eq!(listing.flows.len(), FLOWS);
                began.elapsed().as_micros()
            })
            .collect();
        runs.sort_unstable();
        eprintln!(
            "flow_list over {FLOWS} Flows: median {:.3} ms ({})",
            runs[runs.len() / 2] as f64 / 1_000.0,
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
        );
    }

    /// How long saving takes with the browser's steps among the desktop's,
    /// against the desktop's alone — printed, not checked (plan §3 F2 numbers).
    #[test]
    #[ignore = "a measurement, printed; not a check"]
    fn measure_saving_a_recipe_with_and_without_browser_steps() {
        let desktop: Vec<Step> = (1..=100)
            .map(|n| {
                step(
                    n,
                    &["click", "--app", "Mail", "--x", "1", "--y", "2", "--json"],
                    true,
                )
            })
            .collect();
        let mixed: Vec<Step> = (1..=100)
            .map(|n| {
                if n % 2 == 0 {
                    Step {
                        tool: "browser".into(),
                        ..step(n, &["click", "browser-1", "#pay"], true)
                    }
                } else {
                    step(
                        n,
                        &["click", "--app", "Mail", "--x", "1", "--y", "2", "--json"],
                        true,
                    )
                }
            })
            .collect();
        for (name, steps) in [("desktop only", &desktop), ("half browser", &mixed)] {
            let began = std::time::Instant::now();
            let rounds: u32 = 200;
            let mut bytes = 0;
            for _ in 0..rounds {
                bytes += recipe_markdown("Measure", None, steps, 1).len();
            }
            let per = began.elapsed().as_secs_f64() * 1_000.0 / f64::from(rounds);
            eprintln!(
                "recipe_markdown, 100 steps, {name}: {per:.3} ms per save ({} bytes)",
                bytes / usize::try_from(rounds).unwrap_or(1)
            );
        }
    }
}
