//! `zo build report` — a build says how it is going, in the records the
//! window's task board reads. `zo build` itself runs the builds of a project's
//! `.zo/build.toml` into the same records ([`crate::build_runner`]).
//!
//! Any build at all reports here — a Makefile, a CI step, a release script —
//! into its repository's build records (`zerocode_core::builds`): one record per
//! run under the shared git folder, the same for every worktree of the
//! repository. The verbs change the newest run of one build; each prints a
//! JSON receipt naming the record it wrote.
//!
//! ```text
//! zo build report begin   --build <id> [--name <text> | --name-<lang> <text> …] [--label <text>]
//!                         [--tool <text>] [--sha <sha>] [--plan <step,…> | --plan-file <path|->]
//!                         [--log <path>] [--cwd <dir>]
//! zo build report step    --build <id> [--end <step> --rc <code> [--secs <n>] [--skipped <why>]]
//!                         [--start <step>] [--label <text>] [--cwd <dir>]
//! zo build report stalled --build <id> (--step <step> --what <text> --quiet-since-ms <ms> [--cap-secs <n>] | --clear)
//!                         [--cwd <dir>]
//! zo build report end     --build <id> --outcome passed|failed|cancelled [--reason <text>] [--cwd <dir>]
//! ```

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use zerocode_core::builds::{self, Outcome, PlanStep, Run, Stalled, Text};

pub const USAGE: &str = "\
zo build [--build <id> ...] [--label <text>]
zo build --build <id> --step <step>
zo build --build <id> --from <step> [--label <text>]
zo build stop [--build <id> ...]
zo build approve [--build <id> ...]
zo build forget --build <id> --started <ms>
zo build restore --build <id> --started <ms>
zo build act --build <id> --act retry|stop
zo build report begin   --build <id> [--name <text> | --name-<lang> <text> ...] [--label <text>]
                        [--tool <text>] [--sha <sha>] [--plan <step,...> | --plan-file <path|->]
                        [--log <path>] [--cwd <dir>]
zo build report step    --build <id> [--end <step> --rc <code> [--secs <n>] [--skipped <why>]]
                        [--start <step>] [--label <text>] [--cwd <dir>]
zo build report stalled --build <id> (--step <step> --what <text> --quiet-since-ms <ms> [--cap-secs <n>] | --clear)
                        [--cwd <dir>]
zo build report end     --build <id> --outcome passed|failed|cancelled [--reason <text>] [--cwd <dir>]

  `zo build` runs the builds of the project's .zo/build.toml in its order,
  one runner a project at a time; --step goes on from a step that waits for
  a person, --from runs one build again from a step (the steps before it do
  nothing), `stop` stops them, and `approve` allows the commands no one has
  allowed to run here yet. `forget` moves an ended run to a trash it can be
  brought back from for a day (`restore`); `act` runs the retry or stop
  command a build another script reports names in .zo/build.toml. Any other build says how it is going with
  `report`: `begin` starts a run of the build <id>,
  `step` ends a step and starts the next, `stalled` says the running step has
  gone quiet, and `end` says how the run ended. The records live under the
  repository's shared git folder (zo/builds/<id>/), one per run, and the
  window's task board shows each build's newest run in the order the
  project's .zo/build.toml gives; --log names the file the run writes its
  output to, which the board's schedule opens. Receipts are JSON.
";

/// The flag every verb carries. An older `zo` that has no `build` verb reads
/// the words before it as a prompt and refuses this flag before any session
/// starts, so a script that reports to it fails fast and harmlessly.
const BUILD_FLAG: &str = "--build";
const NAME_BY_LANGUAGE: &str = "--name-";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Verb {
    Begin(Begin),
    Change(Change),
}

/// A new run of the build: everything said of it before its first step.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Begin {
    name: Option<Text>,
    label: String,
    tool: String,
    sha: String,
    plan: Option<PlanSource>,
    log: Option<PathBuf>,
}

/// A change to the build's newest run, which must still be running.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Change {
    Step {
        end: Option<EndStep>,
        start: Option<String>,
        label: Option<String>,
    },
    Stalled(Option<Stalled>),
    End {
        outcome: Outcome,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EndStep {
    step: String,
    rc: i32,
    secs: Option<u32>,
    skipped: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PlanSource {
    Names(Vec<String>),
    File(PathBuf),
    Stdin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Request {
    build: String,
    verb: Verb,
    cwd: Option<PathBuf>,
}

/// The flags given, by name; a name given twice keeps its last value.
#[derive(Default)]
struct Flags {
    values: BTreeMap<String, String>,
    names: BTreeMap<String, String>,
    clear: bool,
}

impl Flags {
    fn take(&mut self, flag: &str) -> Option<String> {
        self.values.remove(flag)
    }

    fn take_number<T: FromStr>(&mut self, flag: &str) -> Result<Option<T>, String> {
        self.take(flag)
            .map(|value| {
                value
                    .parse()
                    .map_err(|_| format!("{flag} needs a whole number, not `{value}`"))
            })
            .transpose()
    }

    fn require(&mut self, flag: &str, verb: &str) -> Result<String, String> {
        self.take(flag)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| format!("`zo build report {verb}` needs {flag}"))
    }

    /// Refuses whatever flag the verb did not take.
    fn finish(self, verb: &str) -> Result<(), String> {
        let left: Vec<String> = self
            .values
            .into_keys()
            .chain(
                self.names
                    .into_keys()
                    .map(|language| format!("{NAME_BY_LANGUAGE}{language}")),
            )
            .chain(self.clear.then(|| "--clear".to_owned()))
            .collect();
        if left.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "`zo build report {verb}` does not take {}\n\n{USAGE}",
                left.join(", ")
            ))
        }
    }
}

const VALUE_FLAGS: &[&str] = &[
    BUILD_FLAG,
    "--name",
    "--label",
    "--tool",
    "--sha",
    "--plan",
    "--plan-file",
    "--log",
    "--end",
    "--rc",
    "--secs",
    "--skipped",
    "--start",
    "--step",
    "--what",
    "--quiet-since-ms",
    "--cap-secs",
    "--outcome",
    "--reason",
    "--cwd",
];

fn flags_of(words: &[String]) -> Result<Flags, String> {
    let mut flags = Flags::default();
    let mut iter = words.iter();
    while let Some(flag) = iter.next() {
        if flag == "--clear" {
            flags.clear = true;
            continue;
        }
        let known = VALUE_FLAGS.contains(&flag.as_str());
        let language = flag
            .strip_prefix(NAME_BY_LANGUAGE)
            .filter(|language| !language.is_empty());
        if !known && language.is_none() {
            return Err(format!("unknown argument `{flag}`\n\n{USAGE}"));
        }
        let value = iter
            .next()
            .cloned()
            .ok_or_else(|| format!("{flag} requires a value"))?;
        match language {
            Some(language) => {
                flags.names.insert(language.to_owned(), value);
            }
            None => {
                flags.values.insert(flag.clone(), value);
            }
        }
    }
    Ok(flags)
}

fn parse(args: &[String]) -> Result<Request, String> {
    let (verb_word, words) = match args {
        [report, verb, words @ ..] if report == "report" => (verb.as_str(), words),
        [first, ..] if first != "-h" && first != "--help" && first != "report" => {
            return Err(format!("unknown `zo build` verb `{first}`\n\n{USAGE}"));
        }
        _ => return Err(USAGE.to_owned()),
    };
    let mut flags = flags_of(words)?;
    let build = flags.require(BUILD_FLAG, verb_word)?;
    let cwd = flags.take("--cwd").map(PathBuf::from);
    let verb = match verb_word {
        "begin" => Verb::Begin(begin_of(&mut flags)?),
        "step" => Verb::Change(step_of(&mut flags)?),
        "stalled" => Verb::Change(stalled_of(&mut flags)?),
        "end" => Verb::Change(end_of(&mut flags)?),
        other => return Err(format!("unknown `zo build report` verb `{other}`\n\n{USAGE}")),
    };
    flags.finish(verb_word)?;
    Ok(Request { build, verb, cwd })
}

fn begin_of(flags: &mut Flags) -> Result<Begin, String> {
    let one = flags.take("--name");
    let by_language = std::mem::take(&mut flags.names);
    let name = match (one, by_language.is_empty()) {
        (Some(_), false) => {
            return Err(format!(
                "give the build's name once with --name, or once per language with {NAME_BY_LANGUAGE}<lang>"
            ));
        }
        (Some(one), true) => Some(Text::One(one)),
        (None, false) => Some(Text::ByLanguage(by_language)),
        (None, true) => None,
    };
    let plan = match (flags.take("--plan"), flags.take("--plan-file")) {
        (Some(_), Some(_)) => return Err("choose --plan or --plan-file".to_owned()),
        (Some(names), None) => Some(PlanSource::Names(
            names
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .collect(),
        )),
        (None, Some(path)) if path == "-" => Some(PlanSource::Stdin),
        (None, Some(path)) => Some(PlanSource::File(PathBuf::from(path))),
        (None, None) => None,
    };
    Ok(Begin {
        name,
        label: flags.take("--label").unwrap_or_default(),
        tool: flags.take("--tool").unwrap_or_default(),
        sha: flags.take("--sha").unwrap_or_default(),
        plan,
        log: flags
            .take("--log")
            .filter(|path| !path.trim().is_empty())
            .map(PathBuf::from),
    })
}

fn step_of(flags: &mut Flags) -> Result<Change, String> {
    let end = match flags.take("--end") {
        Some(step) => Some(EndStep {
            step,
            rc: flags
                .take_number("--rc")?
                .ok_or("`zo build report step --end` needs --rc")?,
            secs: flags.take_number("--secs")?,
            skipped: flags.take("--skipped").filter(|why| !why.trim().is_empty()),
        }),
        None => None,
    };
    let start = flags.take("--start");
    let label = flags.take("--label");
    if end.is_none() && start.is_none() && label.is_none() {
        return Err("`zo build report step` needs --end, --start or --label".to_owned());
    }
    Ok(Change::Step { end, start, label })
}

fn stalled_of(flags: &mut Flags) -> Result<Change, String> {
    if std::mem::take(&mut flags.clear) {
        return Ok(Change::Stalled(None));
    }
    Ok(Change::Stalled(Some(Stalled {
        step: flags.require("--step", "stalled")?,
        what: flags.require("--what", "stalled")?,
        quiet_since_ms: flags
            .take_number("--quiet-since-ms")?
            .ok_or("`zo build report stalled` needs --quiet-since-ms")?,
        cap_secs: flags.take_number("--cap-secs")?.unwrap_or(0),
    })))
}

fn end_of(flags: &mut Flags) -> Result<Change, String> {
    let outcome = match flags.require("--outcome", "end")?.as_str() {
        "passed" => Outcome::Passed,
        "failed" => Outcome::Failed,
        "cancelled" => Outcome::Cancelled,
        other => {
            return Err(format!(
                "--outcome is passed, failed or cancelled, not `{other}`"
            ));
        }
    };
    Ok(Change::End {
        outcome,
        reason: flags.take("--reason").unwrap_or_default(),
    })
}

fn read_plan(source: &PlanSource) -> Result<Vec<PlanStep>, String> {
    let text = match source {
        PlanSource::Names(names) => {
            return Ok(names
                .iter()
                .map(|name| PlanStep {
                    name: name.clone(),
                    words: None,
                })
                .collect());
        }
        PlanSource::File(path) => std::fs::read_to_string(path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?,
        PlanSource::Stdin => {
            let mut held = String::new();
            std::io::stdin()
                .read_to_string(&mut held)
                .map_err(|error| format!("could not read the plan from stdin: {error}"))?;
            held
        }
    };
    serde_json::from_str(&text).map_err(|error| {
        format!("the plan is a JSON list of {{\"name\", \"words\"}} steps: {error}")
    })
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
}

impl Begin {
    /// The run as of `now_ms`; a log named by a relative path is the one under
    /// `cwd`, the folder the report was made from.
    fn into_run(self, now_ms: i64, cwd: &Path) -> Result<Run, String> {
        Ok(Run {
            plan: self
                .plan
                .as_ref()
                .map(read_plan)
                .transpose()?
                .unwrap_or_default(),
            name: self.name,
            label: self.label,
            tool: self.tool,
            sha: self.sha,
            started_ms: now_ms,
            log: self
                .log
                .map(|path| cwd.join(path).display().to_string())
                .unwrap_or_default(),
            ..Run::default()
        })
    }
}

impl Change {
    fn apply(self, run: &mut Run, now_ms: i64) {
        match self {
            Self::Step { end, start, label } => {
                if let Some(end) = end {
                    match end.skipped {
                        Some(why) => run.skip_step(&end.step, why),
                        None => run.end_step(&end.step, end.rc, end.secs, now_ms),
                    }
                }
                if let Some(step) = start {
                    run.start_step(&step, now_ms);
                }
                if let Some(label) = label {
                    run.label = label;
                }
            }
            Self::Stalled(stalled) => run.stalled = stalled,
            Self::End { outcome, reason } => run.end(outcome, reason),
        }
    }
}

/// Writes what `request` says into its build's records, under the repository
/// that holds `cwd`, as of `now_ms`: a new run, or a change to the newest one.
fn report(request: Request, cwd: &Path, now_ms: i64) -> Result<PathBuf, String> {
    let at = request.cwd.as_deref().unwrap_or(cwd);
    let shared = zerocode_core::git_dir::shared_of(at)
        .ok_or_else(|| format!("{} is not inside a git checkout", at.display()))?;
    let id = request.build.as_str();
    match request.verb {
        Verb::Begin(begin) => builds::begin(&shared, id, &begin.into_run(now_ms, at)?)
            .map_err(|error| error.to_string()),
        Verb::Change(change) => {
            let (path, mut run) =
                builds::running(&shared, id).map_err(|error| error.to_string())?;
            change.apply(&mut run, now_ms);
            builds::save(&path, &run).map_err(|error| error.to_string())?;
            Ok(path)
        }
    }
}

pub fn run(args: &[String], cwd: &Path) -> Result<String, String> {
    match args.first().map(String::as_str) {
        Some("report") => {
            let request = parse(args)?;
            let build = request.build.clone();
            let record = report(request, cwd, now_ms())?;
            serde_json::to_string(&json!({ "build": build, "record": record }))
                .map_err(|error| error.to_string())
        }
        Some("-h" | "--help") => Ok(USAGE.to_owned()),
        _ => crate::build_runner::run(args, cwd),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    fn checkout() -> tempfile::TempDir {
        let checkout = tempfile::tempdir().expect("a checkout");
        std::fs::create_dir_all(checkout.path().join(".git")).expect("its git folder");
        checkout
    }

    fn newest(checkout: &Path, id: &str) -> Run {
        let shared = zerocode_core::git_dir::shared_of(checkout).expect("a repository");
        let dir = builds::folder(&shared).join(id);
        let mut records: Vec<PathBuf> = std::fs::read_dir(dir)
            .expect("the build's folder")
            .map(|entry| entry.expect("an entry").path())
            .collect();
        records.sort();
        let text = std::fs::read_to_string(records.last().expect("a record")).expect("readable");
        serde_json::from_str(&text).expect("a run")
    }

    #[test]
    fn a_run_reports_its_steps_and_its_end() {
        let checkout = checkout();
        let at = checkout.path();
        for (words, now) in [
            (
                &[
                    "report", "begin", "--build", "app", "--name-ko", "앱 빌드", "--name-en",
                    "App", "--plan", "compile, test", "--tool", "gradle",
                ][..],
                1_000,
            ),
            (&["report", "step", "--build", "app", "--start", "compile"][..], 2_000),
            (
                &[
                    "report", "step", "--build", "app", "--end", "compile", "--rc", "0",
                    "--start", "test", "--label", "v1.2",
                ][..],
                9_000,
            ),
            (
                &["report", "end", "--build", "app", "--outcome", "failed", "--reason", "3 tests"][..],
                12_000,
            ),
        ] {
            report(parse(&args(words)).expect("a request"), at, now).expect("reported");
        }

        let run = newest(at, "app");

        assert_eq!(run.started_ms, 1_000);
        assert_eq!(run.label, "v1.2");
        assert_eq!(
            run.plan.iter().map(|step| step.name.as_str()).collect::<Vec<_>>(),
            ["compile", "test"]
        );
        assert_eq!((run.done[0].name.as_str(), run.done[0].secs), ("compile", 7));
        assert_eq!((run.outcome, run.reason.as_str()), (Some(Outcome::Failed), "3 tests"));
        assert_eq!(run.running, None);
        assert!(matches!(run.name, Some(Text::ByLanguage(ref words)) if words["ko"] == "앱 빌드"));
    }

    #[test]
    fn a_runs_log_is_named_by_an_absolute_path_a_relative_one_under_the_reports_folder() {
        let checkout = checkout();
        let at = checkout.path();
        let elsewhere = at.join("logs/lane.log").display().to_string();
        for (build, log, now) in [("lane", elsewhere.as_str(), 1), ("app", "out/app.log", 2)] {
            let words = ["report", "begin", "--build", build, "--log", log];
            report(parse(&args(&words)).expect("a request"), at, now).expect("begun");
        }

        assert_eq!(newest(at, "lane").log, elsewhere);
        assert_eq!(newest(at, "app").log, at.join("out/app.log").display().to_string());
    }

    #[test]
    fn a_report_to_an_ended_run_or_none_is_refused() {
        let checkout = checkout();
        let at = checkout.path();
        let step = parse(&args(&["report", "step", "--build", "app", "--start", "x"])).expect("a request");

        let none = report(step.clone(), at, 1).expect_err("no run has begun");
        for words in [
            &["report", "begin", "--build", "app"][..],
            &["report", "end", "--build", "app", "--outcome", "passed"][..],
        ] {
            report(parse(&args(words)).expect("a request"), at, 2).expect("reported");
        }
        let ended = report(step, at, 3).expect_err("the run has ended");

        assert!(none.contains("no run of `app`"), "{none}");
        assert!(ended.contains("has ended"), "{ended}");
    }

    #[test]
    fn a_stalled_step_is_said_and_cleared() {
        let checkout = checkout();
        let at = checkout.path();
        report(parse(&args(&["report", "begin", "--build", "lane"])).expect("a request"), at, 1).expect("begun");
        let quiet = &[
            "report", "stalled", "--build", "lane", "--step", "gate", "--what", "verify", "--quiet-since-ms", "5",
            "--cap-secs", "60",
        ];

        report(parse(&args(quiet)).expect("a request"), at, 6).expect("said");
        let said = newest(at, "lane").stalled;
        report(parse(&args(&["report", "stalled", "--build", "lane", "--clear"])).expect("a request"), at, 7)
            .expect("cleared");

        assert_eq!(said.map(|stalled| (stalled.step, stalled.cap_secs)), Some(("gate".to_owned(), 60)));
        assert_eq!(newest(at, "lane").stalled, None);
    }

    #[test]
    fn what_cannot_be_a_report_is_refused_before_anything_is_written() {
        for (words, said) in [
            (&["report", "begin"][..], "needs --build"),
            (&["report", "step", "--build", "app"][..], "needs --end, --start or --label"),
            (&["report", "step", "--build", "app", "--end", "x"][..], "needs --rc"),
            (&["report", "end", "--build", "app", "--outcome", "green"][..], "passed, failed or cancelled"),
            (&["report", "begin", "--build", "app", "--name", "A", "--name-ko", "에이"][..], "once per language"),
            (&["report", "end", "--build", "app", "--outcome", "passed", "--start", "x"][..], "does not take --start"),
            (&["report", "begin", "--build", "app", "--bogus", "1"][..], "unknown argument `--bogus`"),
            (&["deploy"][..], "unknown `zo build` verb"),
        ] {
            let refused = parse(&args(words)).expect_err("refused");
            assert!(refused.contains(said), "{words:?}: {refused}");
        }
        let checkout = checkout();
        let bad = report(
            parse(&args(&["report", "begin", "--build", "../out"])).expect("a request"),
            checkout.path(),
            1,
        )
        .expect_err("not a build id");
        assert!(bad.contains("cannot name a build"), "{bad}");
    }

    #[test]
    fn a_folder_outside_any_checkout_has_no_builds_to_report() {
        let plain = tempfile::tempdir().expect("a plain folder");

        let refused = report(
            parse(&args(&["report", "begin", "--build", "app"])).expect("a request"),
            plain.path(),
            1,
        )
        .expect_err("no repository");

        assert!(refused.contains("not inside a git checkout"), "{refused}");
    }
}
