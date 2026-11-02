//! The window ends what a test or a measurement left behind (t-89544), so a
//! person never has to.
//!
//! On 2026-11-02 at 10:32 a timer set outside the build turn ended the turn and
//! left five `node ui/tests/window.mjs` and twenty headless Chromes running: five
//! hours, no CPU, about 1.8 GiB. After a restart the kill guard and
//! `process-sweep` both refused them as "not this pane's", and a person typed
//! `kill`. This is the judgment that lets the window do it instead.
//!
//! The judgment is narrow on purpose. A process is ended only when every one of
//! these is known, and anything unknown leaves it alone:
//!
//! - it hangs under pid 1, with no terminal, and nothing outside its own family
//!   shares its process group;
//! - its program and arguments say it is a test ([`kind_of`]);
//! - it stands in a checkout of a repository the ledger knows, or in an agent's
//!   scratch folder ([`Places`]);
//! - it is at least [`MIN_AGE_SECS`] old and its CPU time has not moved for
//!   [`CPU_STILL_SECS`];
//! - its whole family qualifies: one busy or unknown child keeps them all.
//!
//! This module reads no system and sends no signal. The window reads the table
//! and the places, and the signals go through `process_sweep`'s own road
//! ([`end`]): the same `pid@mark`, read again right before its TERM, so a reused
//! pid is left alone by one piece of code rather than two.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use super::process_sweep::{self as sweep, Judge, Judgment, Outcome, ProcessRow, Signals, Target};

/// How often the window looks: one `ps` every five minutes. Measured 11-02 on
/// this machine (1,035 processes): 69 ms wall and 67 ms CPU a look, 0.02 % of one
/// core; 255 ms under `taskpolicy -c background`, 0.09 %.
pub const LOOK_EVERY_SECS: u64 = 5 * 60;
/// A process younger than this is never a stray: the longest window suite runs
/// well past ten minutes and a person's own run may pause.
pub const MIN_AGE_SECS: u64 = 30 * 60;
/// How long a process's CPU time must stand still before it counts as stuck. A
/// restart forgets the watch, so a process is watched this long again after one:
/// the safe side.
pub const CPU_STILL_SECS: u64 = 10 * 60;
/// How much CPU time may still move inside [`CPU_STILL_SECS`] and count as still,
/// in hundredths of a second: one second, 0.17 % of one core. An idle headless
/// Chrome's own timers tick (measured 11-02: its main process took 0.01 s a
/// minute doing nothing), so "not one tick" would never catch the very processes
/// this exists for; a test doing work moves far more than this in a minute.
pub const CPU_STILL_SLACK_CS: u64 = 100;
/// From the first TERM to the KILL for whatever still stands.
pub const TERM_GRACE_MS: u64 = 5_000;
/// How many one look may end. The incident was 25; a look that wants more than
/// this ends this many and takes the rest five minutes later, so a mistake in the
/// judgment is small and its line in the log comes first.
pub const ENDS_PER_LOOK_MAX: usize = 64;
/// How long a KILL is given to show before it is read: it does not wait on the
/// process.
const KILL_SETTLE_MS: u64 = 200;
/// How many different names one log line spells; the rest are counted.
const LINE_NAMES_MAX: usize = 6;

/// A word in a command line or a folder that marks a measurement. A measurement's
/// processes sit idle on purpose, and ending one spoils a number nobody can get
/// back; the build line's own runner reads the same words as "wants a quiet
/// machine".
const MEASUREMENT_WORDS: [&str; 6] = [
    "measure",
    "measurement",
    "harness",
    "측정",
    "실측",
    "하네스",
];

const HEADLESS_CHROME: &str = "chrome-headless-shell";
const PLAYWRIGHT_CACHE: &str = "ms-playwright";
const NODE: &str = "node";
/// Cargo's test binaries end in `-` and 16 hex digits.
const TEST_BINARY_HASH_HEX: usize = 16;

/// What kind of test a command line says it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    NodeTest,
    HeadlessChrome,
    TestBinary,
}

/// One process-table row, read once per look. A column the system did not answer
/// is `None`, and `None` never qualifies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub pid: u32,
    pub ppid: u32,
    pub pgid: u32,
    pub uid: u32,
    /// The controlling terminal; `None` for a process that has none.
    pub tty: Option<String>,
    /// CPU time so far, in hundredths of a second.
    pub cpu_cs: Option<u64>,
    /// Whether the system's recent CPU share reads exactly zero.
    pub cpu_idle_now: bool,
    pub age_secs: Option<u64>,
    /// The start identity half of the `pid@mark`.
    pub started: String,
    pub args: String,
    pub cwd: Option<String>,
}

/// What stands behind a place: a checkout of a known repository, or an agent's
/// scratch folder, with the folder that is its top.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    Checkout(PathBuf),
    Scratch(PathBuf),
}

impl Place {
    fn root(&self) -> &Path {
        match self {
            Self::Checkout(root) | Self::Scratch(root) => root,
        }
    }
}

/// Answers from the file system, from outside, so a test needs no real folder.
pub trait Places {
    /// Whose folder `path` stands in; `None` for anywhere else, and for a path
    /// that cannot be read.
    fn place_of(&self, path: &str) -> Option<Place>;
}

/// What is never touched, as the window reads it for one look.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Holders {
    /// This user. A row of anybody else qualifies for nothing.
    pub uid: u32,
    /// The window itself.
    pub window: u32,
    /// The processes the window's panes hold.
    pub panes: Vec<u32>,
    /// The build line's records: a pid in `run/`, a turn's lock, a lane's lock.
    /// A stray in one of their process groups is that work's own.
    pub recorded: Vec<u32>,
    /// Folders a running build-line job stands in.
    pub busy_folders: Vec<String>,
    /// A marker or a lane says something is measuring or running: nothing is
    /// ended this look.
    pub stand_down: bool,
}

/// Why a stray is believed to have stopped moving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuProof {
    /// The window watched it stand still for [`CPU_STILL_SECS`]: the road nobody
    /// is asked on.
    Watched,
    /// A coordinator read a list and named its `pid@mark`, which answers for what
    /// the window has not watched since it started: the system's recent share
    /// reads zero and the window has not seen it move lately. Everything else is
    /// judged the same.
    NamedByCoordinator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Still {
    cpu_cs: u64,
    since_secs: u64,
    /// Whether this watch has seen the CPU time move, as opposed to first sight.
    moved: bool,
}

/// What the window remembers between looks: when each suspect's CPU time last
/// moved. Held in memory only, so a restart starts each one over.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Watch {
    seen: HashMap<(u32, String), Still>,
}

impl Watch {
    /// Take in one table. A process leaves the watch when it leaves the table.
    pub fn observe(&mut self, rows: &[Row], now_secs: u64) {
        let mut standing = HashSet::new();
        for row in rows {
            if kind_of(&row.args).is_none() {
                continue;
            }
            let Some(cpu_cs) = row.cpu_cs else {
                continue;
            };
            let key = (row.pid, row.started.clone());
            standing.insert(key.clone());
            let still = self.seen.entry(key).or_insert(Still {
                cpu_cs,
                since_secs: now_secs,
                moved: false,
            });
            if cpu_cs.saturating_sub(still.cpu_cs) > CPU_STILL_SLACK_CS || cpu_cs < still.cpu_cs {
                *still = Still {
                    cpu_cs,
                    since_secs: now_secs,
                    moved: true,
                };
            }
        }
        self.seen.retain(|key, _| standing.contains(key));
    }

    fn proves_still(&self, row: &Row, now_secs: u64, proof: CpuProof) -> bool {
        let still = self.seen.get(&(row.pid, row.started.clone()));
        let watched =
            still.is_some_and(|still| now_secs.saturating_sub(still.since_secs) >= CPU_STILL_SECS);
        match proof {
            CpuProof::Watched => watched,
            CpuProof::NamedByCoordinator => {
                let moved_lately = still.is_some_and(|still| still.moved);
                watched || (row.cpu_idle_now && !moved_lately)
            }
        }
    }
}

/// What a command line alone says. The first word is the program, in a form with
/// no space in it: a path holding a space is not a test this names, and a process
/// this cannot read is left alone.
#[must_use]
pub fn kind_of(args: &str) -> Option<Kind> {
    let mut words = args.split_whitespace();
    let program = words.next()?;
    let name = Path::new(program).file_name()?.to_str()?;
    if name == NODE {
        let script = script_of(args)?;
        return (script.contains("tests/") || script.contains("computer-bench/"))
            .then_some(Kind::NodeTest);
    }
    if name == HEADLESS_CHROME {
        return in_playwright_cache(program).then_some(Kind::HeadlessChrome);
    }
    test_binary_root(program).map(|_| Kind::TestBinary)
}

fn in_playwright_cache(program: &str) -> bool {
    let path = Path::new(program);
    path.is_absolute()
        && path
            .components()
            .any(|part| part.as_os_str() == PLAYWRIGHT_CACHE)
}

/// The first word after `node` that is not an option. An option taking its value
/// as the next word makes that value the script, which names no test: left alone.
fn script_of(args: &str) -> Option<&str> {
    args.split_whitespace()
        .skip(1)
        .find(|word| !word.starts_with('-'))
}

/// The folder above `target/<profile>/deps/<name>-<hash>`, when `program` is
/// that; a binary anywhere else is not a test this names.
fn test_binary_root(program: &str) -> Option<&Path> {
    let path = Path::new(program);
    if !path.is_absolute() {
        return None;
    }
    let name = path.file_name()?.to_str()?;
    let (_, hash) = name.rsplit_once('-')?;
    if hash.len() != TEST_BINARY_HASH_HEX
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let deps = path.parent()?;
    let profile = deps.parent()?;
    let target = profile.parent()?;
    (deps.file_name()? == "deps" && target.file_name()? == "target").then_some(target.parent()?)
}

/// Where a program or script path names, with `.` taken out and a `..` refused.
fn resolved(cwd: &str, path: &str) -> Option<PathBuf> {
    let joined = Path::new(cwd).join(path);
    let mut plain = PathBuf::new();
    for part in joined.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => return None,
            other => plain.push(other),
        }
    }
    Some(plain)
}

/// Whether a path under a checkout or scratch folder is one of the repository's
/// tests: `ui/tests/<one file>.mjs`, or anything under `tools/computer-bench/`.
fn is_repository_test(inside: &Path) -> bool {
    let parts: Vec<&std::ffi::OsStr> = inside.iter().collect();
    match parts.as_slice() {
        [ui, tests, file]
            if *ui == "ui"
                && *tests == "tests"
                && Path::new(file).extension().is_some_and(|ext| ext == "mjs") =>
        {
            true
        }
        [tools, bench, _, ..] => *tools == "tools" && *bench == "computer-bench",
        _ => false,
    }
}

/// The part of `inside` that is the repository's own path: `ui/…` or `tools/…`.
fn repository_tail(inside: &Path) -> PathBuf {
    (0..inside.iter().count())
        .map(|skip| inside.iter().skip(skip).collect::<PathBuf>())
        .find(|tail| is_repository_test(tail))
        .unwrap_or_else(|| inside.to_path_buf())
}

/// What this process is, read with where it stands; `None` when any part cannot be
/// told. The word is what the log says of it: relative to the checkout.
fn said_of(row: &Row, places: &dyn Places) -> Option<String> {
    let cwd = row.cwd.as_deref()?;
    places.place_of(cwd)?;
    let program = row.args.split_whitespace().next()?;
    match kind_of(&row.args)? {
        Kind::NodeTest => {
            let script = resolved(cwd, script_of(&row.args)?)?;
            let place = places.place_of(script.to_str()?)?;
            let inside = script.strip_prefix(place.root()).ok()?;
            let test = match place {
                Place::Checkout(_) => is_repository_test(inside).then_some(inside),
                // A copy of the repository in a scratch folder, one suite run from
                // an archive of a commit, stands somewhere below the folder's top.
                Place::Scratch(_) => is_repository_test(&repository_tail(inside)).then_some(inside),
            }?;
            Some(format!("{NODE} {}", repository_tail(test).display()))
        }
        Kind::HeadlessChrome => Some(HEADLESS_CHROME.to_string()),
        Kind::TestBinary => {
            let root = test_binary_root(program)?;
            let Place::Checkout(checkout) = places.place_of(root.to_str()?)? else {
                return None;
            };
            let inside = Path::new(program).strip_prefix(&checkout).ok()?;
            (checkout == root).then(|| inside.display().to_string())
        }
    }
}

fn marked_as_measurement(row: &Row) -> bool {
    let holds = |text: &str| {
        let lower = text.to_lowercase();
        MEASUREMENT_WORDS.iter().any(|word| lower.contains(word))
    };
    holds(&row.args) || row.cwd.as_deref().is_some_and(holds)
}

/// One family that passed everything the table alone can say: the root under pid
/// 1 and everything below it. Their places are read next, and only theirs.
#[must_use]
pub fn suspects(
    rows: &[Row],
    holders: &Holders,
    watch: &Watch,
    now_secs: u64,
    proof: CpuProof,
) -> Vec<Vec<u32>> {
    if holders.stand_down {
        return Vec::new();
    }
    let by_pid: HashMap<u32, &Row> = rows.iter().map(|row| (row.pid, row)).collect();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for row in rows {
        children.entry(row.ppid).or_default().push(row.pid);
    }
    let mut groups_held: HashSet<u32> = holders.recorded.iter().copied().collect();
    for pid in &holders.recorded {
        if let Some(row) = by_pid.get(pid) {
            groups_held.insert(row.pgid);
        }
    }
    let member_ok = |row: &Row| {
        row.uid == holders.uid
            && row.pid > 1
            && row.pid != holders.window
            && !holders.panes.contains(&row.pid)
            && !holders.recorded.contains(&row.pid)
            && !groups_held.contains(&row.pgid)
            && row.tty.is_none()
            && kind_of(&row.args).is_some()
            && !marked_as_measurement(row)
            && row.age_secs.is_some_and(|age| age >= MIN_AGE_SECS)
            && watch.proves_still(row, now_secs, proof)
    };
    let mut families: Vec<Vec<u32>> = rows
        .iter()
        .filter(|row| row.ppid == 1)
        .filter_map(|root| {
            let family = family_of(root.pid, &children);
            family
                .iter()
                .all(|pid| by_pid.get(pid).is_some_and(|row| member_ok(row)))
                .then_some(family)
        })
        .collect();
    // A process group holding anything outside the families on the list is
    // someone's running work: a job's shell, a batch script with its next test
    // still to come. Taking one family off can uncover another, so round again.
    loop {
        let listed: HashSet<u32> = families.iter().flatten().copied().collect();
        let foreign_group = |group: u32| {
            rows.iter()
                .any(|row| row.pgid == group && !listed.contains(&row.pid))
        };
        let before = families.len();
        families.retain(|family| {
            !family
                .iter()
                .any(|pid| by_pid.get(pid).is_some_and(|row| foreign_group(row.pgid)))
        });
        if families.len() == before {
            break;
        }
    }
    families
}

fn family_of(root: u32, children: &HashMap<u32, Vec<u32>>) -> Vec<u32> {
    let mut family = vec![root];
    let mut at = 0;
    while at < family.len() {
        if let Some(below) = children.get(&family[at]) {
            for pid in below {
                if !family.contains(pid) {
                    family.push(*pid);
                }
            }
        }
        at += 1;
    }
    family
}

/// A process the window has decided to end, and what the log may say of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stray {
    pub pid: u32,
    pub target: Target,
    pub said: String,
}

/// The families whose every member stands in a place this can name, none of them
/// beside a running job; `rows` carries the places by now.
#[must_use]
pub fn strays_of(
    families: &[Vec<u32>],
    rows: &[Row],
    holders: &Holders,
    places: &dyn Places,
) -> Vec<Stray> {
    let by_pid: HashMap<u32, &Row> = rows.iter().map(|row| (row.pid, row)).collect();
    let mut found = Vec::new();
    for family in families {
        let judged: Option<Vec<Stray>> = family
            .iter()
            .map(|pid| {
                let row = by_pid.get(pid)?;
                if marked_as_measurement(row) || beside_a_job(row, holders) {
                    return None;
                }
                Some(Stray {
                    pid: *pid,
                    target: Target {
                        pid: *pid,
                        mark: sweep::mark(*pid, &row.started, &row.args),
                    },
                    said: said_of(row, places)?,
                })
            })
            .collect();
        if let Some(judged) = judged {
            found.extend(judged);
        }
    }
    found
}

fn beside_a_job(row: &Row, holders: &Holders) -> bool {
    row.cwd.as_deref().is_some_and(|cwd| {
        holders
            .busy_folders
            .iter()
            .any(|folder| Path::new(cwd).starts_with(folder))
    })
}

/// The stronger signal, from outside. `process_sweep`'s own road never sends one,
/// so it is not one of its [`Signals`].
pub trait Stronger {
    fn kill(&self, pid: u32) -> Result<(), String>;
}

/// What a look did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reaped {
    /// What the log may say of each process this look ended, and of nothing else.
    pub said: Vec<String>,
    pub by_term: usize,
    pub by_kill: usize,
    /// Still standing after the KILL, or failed.
    pub left: usize,
    /// Read again before the signal and found to be another process.
    pub changed: usize,
    /// Already gone when its signal was due: nothing was sent and nothing ended.
    pub gone_before: usize,
}

impl Reaped {
    /// Whether a signal went out or one was held back; a look that only found its
    /// strays already gone did nothing.
    #[must_use]
    pub const fn did_anything(&self) -> bool {
        self.by_term + self.by_kill + self.left + self.changed > 0
    }
}

/// The judgment was made a moment ago, over this same table, and the window is
/// the one asking: every row the end road reaches was judged.
struct Decided;

impl Judge for Decided {
    fn judge(&self, _row: &ProcessRow, _table: &[ProcessRow]) -> Judgment {
        Judgment::Allow
    }
}

/// TERM by `process_sweep`'s road, each pid read again against its mark right
/// before the signal; whatever still stands with the same mark after
/// [`TERM_GRACE_MS`] gets a KILL under the same reading.
pub fn end(
    strays: &[Stray],
    rows: &[Row],
    signals: &dyn Signals,
    stronger: &dyn Stronger,
) -> Reaped {
    let strays = &strays[..strays.len().min(ENDS_PER_LOOK_MAX)];
    let table: Vec<ProcessRow> = rows
        .iter()
        .map(|row| ProcessRow {
            pid: row.pid,
            ppid: row.ppid,
            started: row.started.clone(),
            args: row.args.clone(),
            cwd: row.cwd.clone(),
        })
        .collect();
    let targets: Vec<Target> = strays.iter().map(|stray| stray.target.clone()).collect();
    let said_of_pid: HashMap<u32, &str> = strays
        .iter()
        .map(|stray| (stray.target.pid, stray.said.as_str()))
        .collect();
    let said = |target: &Target| said_of_pid.get(&target.pid).map(|said| (*said).to_string());
    let began = signals.now_ms();
    let ended = sweep::end(&table, &targets, &Decided, signals);
    let mut reaped = Reaped::default();
    let mut standing: Vec<&Target> = Vec::new();
    for one in &ended {
        match &one.outcome {
            Outcome::Signaled { gone: true } => {
                reaped.by_term += 1;
                reaped.said.extend(said(&one.target));
            }
            Outcome::Gone => reaped.gone_before += 1,
            Outcome::Signaled { gone: false } => standing.push(&one.target),
            Outcome::Changed => reaped.changed += 1,
            Outcome::Refused { .. } | Outcome::Failed { .. } => reaped.left += 1,
        }
    }
    if standing.is_empty() {
        return reaped;
    }
    let spent = signals.now_ms().saturating_sub(began);
    signals.pause(TERM_GRACE_MS.saturating_sub(spent));
    let pids: Vec<u32> = standing.iter().map(|target| target.pid).collect();
    let Some(now) = signals.reread_many(&pids) else {
        // A read that failed says nothing about any of them, so no KILL goes out.
        reaped.left += standing.len();
        return reaped;
    };
    let mut killed = Vec::new();
    for target in standing {
        let stands = now
            .get(&target.pid)
            .is_some_and(|(started, args)| sweep::mark(target.pid, started, args) == target.mark);
        if !stands {
            reaped.by_term += 1;
            reaped.said.extend(said(target));
        } else if stronger.kill(target.pid).is_ok() {
            killed.push(target);
        } else {
            reaped.left += 1;
        }
    }
    if killed.is_empty() {
        return reaped;
    }
    signals.pause(KILL_SETTLE_MS);
    let pids: Vec<u32> = killed.iter().map(|target| target.pid).collect();
    let after = signals.reread_many(&pids);
    for target in killed {
        let stands = after.as_ref().is_none_or(|now| {
            now.get(&target.pid).is_some_and(|(started, args)| {
                sweep::mark(target.pid, started, args) == target.mark
            })
        });
        if stands {
            reaped.left += 1;
        } else {
            reaped.by_kill += 1;
            reaped.said.extend(said(target));
        }
    }
    reaped
}

/// The one line the window's log keeps of a look. It names what ended and how
/// many, relative to the checkout, and no folder of anybody's.
#[must_use]
pub fn line(reaped: &Reaped) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for said in &reaped.said {
        match counts.iter_mut().find(|(name, _)| name == said) {
            Some((_, count)) => *count += 1,
            None => counts.push((said, 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    let mut names: Vec<String> = counts
        .iter()
        .take(LINE_NAMES_MAX)
        .map(|(name, count)| format!("{count} {name}"))
        .collect();
    if counts.len() > LINE_NAMES_MAX {
        names.push(format!(
            "{} more kinds",
            counts.len().saturating_sub(LINE_NAMES_MAX)
        ));
    }
    format!(
        "stray-reap: ended {} left by tests ({}): {} by TERM, {} by KILL, {} left standing, {} \
         changed before the signal",
        reaped.by_term + reaped.by_kill,
        names.join(", "),
        reaped.by_term,
        reaped.by_kill,
        reaped.left,
        reaped.changed
    )
}

/// CPU time as `ps -o time=` prints it — `[[dd-]hh:]mm:ss[.cc]`, or `mmm:ss.cc` on
/// macOS — in hundredths of a second. `None` for anything else.
#[must_use]
pub fn parse_cpu_time(word: &str) -> Option<u64> {
    let (clock, hundredths) = match word.split_once('.') {
        Some((clock, part)) => {
            let digits: u64 = part.parse().ok()?;
            let scaled = match part.len() {
                1 => digits * 10,
                2 => digits,
                _ => return None,
            };
            (clock, scaled)
        }
        None => (word, 0),
    };
    Some(parse_clock(clock)? * 100 + hundredths)
}

/// Elapsed time as `ps -o etime=` prints it, `[[dd-]hh:]mm:ss`, in seconds.
#[must_use]
pub fn parse_elapsed(word: &str) -> Option<u64> {
    parse_clock(word)
}

fn parse_clock(word: &str) -> Option<u64> {
    let (days, clock) = match word.split_once('-') {
        Some((days, clock)) => (days.parse::<u64>().ok()?, clock),
        None => (0, word),
    };
    let mut seconds = 0u64;
    let parts: Vec<&str> = clock.split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    for part in parts {
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        seconds = seconds.checked_mul(60)?.checked_add(part.parse().ok()?)?;
    }
    days.checked_mul(86_400)?.checked_add(seconds)
}

/// The `ps` row this reads: `pid ppid pgid uid tty time etime pcpu` and then the
/// five words of `lstart` and the command line, split by the window, which owns
/// the command line's bounds.
///
/// `None` when a row's numbers do not read.
#[must_use]
pub fn parse_columns(fields: &[&str]) -> Option<Columns> {
    let [pid, ppid, pgid, uid, tty, cpu, elapsed, share] = fields else {
        return None;
    };
    Some(Columns {
        pid: pid.parse().ok()?,
        ppid: ppid.parse().ok()?,
        pgid: pgid.parse().ok()?,
        uid: uid.parse().ok()?,
        tty: terminal_of(tty),
        cpu_cs: parse_cpu_time(cpu),
        age_secs: parse_elapsed(elapsed),
        cpu_idle_now: share.parse::<f64>().is_ok_and(|share| share == 0.0),
    })
}

/// A row's numbers, before its start and command line are folded in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Columns {
    pub pid: u32,
    pub ppid: u32,
    pub pgid: u32,
    pub uid: u32,
    pub tty: Option<String>,
    pub cpu_cs: Option<u64>,
    pub age_secs: Option<u64>,
    pub cpu_idle_now: bool,
}

/// `ps` prints no terminal as `??` on macOS and `?` on Linux. Anything else is a
/// terminal, so a word this does not know keeps the process.
fn terminal_of(word: &str) -> Option<String> {
    (!matches!(word, "??" | "?" | "-")).then(|| word.to_string())
}

/// One `# <key>:` line of a build-line job's header, the comment lines it opens with.
fn job_header<'a>(job: &'a str, key: &str) -> Option<&'a str> {
    job.lines()
        .take_while(|line| line.starts_with('#'))
        .find_map(|line| {
            line.strip_prefix('#')?
                .trim_start()
                .strip_prefix(key)?
                .strip_prefix(':')
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
}

/// Whether the build line's `# cwd:` line names a folder.
#[must_use]
pub fn job_folder(job: &str) -> Option<String> {
    job_header(job, "cwd").map(str::to_string)
}

/// Whether a running job says it measures (`# what:` holds a measurement word): the
/// runner gives it the machine alone, and a look's `ps`, `lsof` and signals would
/// land in its numbers, so the look stands down while it runs.
#[must_use]
pub fn job_measures(job: &str) -> bool {
    job_header(job, "what").is_some_and(|what| {
        let lower = what.to_lowercase();
        MEASUREMENT_WORDS.iter().any(|word| lower.contains(word))
    })
}
