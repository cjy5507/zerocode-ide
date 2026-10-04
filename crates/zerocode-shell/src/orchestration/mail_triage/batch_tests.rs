//! The mail triage asks a run's fresh letters in ONE request (t-32796): the
//! synthetic inbox the numbers are taken on, the stand-in model that answers
//! it, and the cases that hold the road — a batch is one request, its account
//! is counted once, one letter's broken answer is its own, a burst is cut and
//! asked side by side, and a Jev that is not there leaves every letter with
//! the word it would have had.
//!
//! Nothing here is a real letter. A day is a SHAPE: how many letters came in
//! how many batches, and in what kinds. The letters' words are never written —
//! the question reads none — and every id, address and path is made up here.

use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Instant;

use serde_json::Map;
use zerocode_core::jev::batch::{Answers, Judgment, Request};
use zerocode_core::jev::summary::{AGREED, BASELINE_AGREED, LABEL, NOT_COMPARED};
use zerocode_core::jev::{Cap, JevUse, MAIL_TRIAGE_BATCH_CAP, SMART_SETTINGS_KEY, Sent};
use zerocode_core::mail_triage::{
    MailRead, MailRefusal, MailTriage, NO_NEED_AFTER_ACTS, Situation,
};
use zerocode_core::orchestration::{
    Draft, LEDGER_ITSELF, MessageKind, Priority, ServedAnswer, ServedRow, Text, worker_address,
};

use super::*;
use crate::systemone::Asked;
use crate::systemone::TOGETHER_LANES;
use crate::systemone::tests::{ANSWERING_VERSION, Endpoint};

/// The coordinator's own pane, as the window numbers it: a term no other test
/// of this binary uses, so the pane-turn table has nothing to say of it.
const TERM: u32 = 3_279_601;

/// The coordinator's session, as the ledger's receipts name it.
const COORDINATOR: &str = "actor-v1:coordinator";

/// The shape of a day, as `(letters in a batch, batches that carry that
/// many)`: 172 letters in 107 batches — the day on which the label was first
/// measured (2026-09-26, `zerocode_core::mail_triage`), as two counts and
/// nothing else. Most of a day's batches are one letter; a few are bursts.
const DAY: [(usize, usize); 6] = [(1, 66), (2, 28), (3, 8), (4, 3), (6, 1), (8, 1)];

/// The step through the day's batch sizes that spreads the big batches over
/// the day rather than ending it with them: coprime with the number of
/// batches (107, which is prime), so it visits every one once.
const STRIDE: usize = 37;

/// The kinds a day's letters come in, round and round — 8 in 20 a worker's
/// news, 4 a silence told again, 3 a finished task, 2 a question, and one each
/// of a handover, a model switched and a quota wall. A mix, not a count of any
/// real day.
const KINDS: [MessageKind; 20] = [
    MessageKind::Status,
    MessageKind::WentQuiet,
    MessageKind::Status,
    MessageKind::WorkerDone,
    MessageKind::Question,
    MessageKind::Status,
    MessageKind::WentQuiet,
    MessageKind::Handover,
    MessageKind::Status,
    MessageKind::WorkerDone,
    MessageKind::Status,
    MessageKind::Question,
    MessageKind::WentQuiet,
    MessageKind::ModelDeviated,
    MessageKind::Status,
    MessageKind::WorkerDone,
    MessageKind::Status,
    MessageKind::QuotaWalled,
    MessageKind::WentQuiet,
    MessageKind::Status,
];

/// Where the fixture's clock starts, in epoch milliseconds: an arbitrary day.
const DAY_START_MS: i64 = 1_790_000_000_000;

/// The gap between two batches of the day.
const MINUTE_MS: i64 = 60_000;

/// How long after its batch arrived a sweep sees it: a beat and a half.
const SWEEP_AFTER_MS: i64 = 1_500;

/// The gap between two letters of one batch.
const LETTER_GAP_MS: i64 = 100;

/// How long after a batch arrived the coordinator begins on it.
const COORDINATOR_AFTER_MS: i64 = 5_000;

/// The gap between two of the coordinator's acts.
const ACT_GAP_MS: i64 = 100;

/// The workers the ledger's notices are about in a day: a few.
const QUIET_HANDS: usize = 6;

/// A text this long is the product's words and not a letter's fact: the
/// longest value a letter's entry carries is 19 bytes (a kind), the shortest
/// of the words a question says is 31.
const SHARED_WORDS_MIN_BYTES: usize = 24;

/// The words the batch's state opens with — every request, whichever road
/// built it, says them: counting them counts the requests that carried the
/// rubric.
const RUBRIC_OPENING: &str = "A coordinator agent runs a team of worker agents";

/// The most bytes of the product's words one request may repeat for each
/// letter beyond its first: the option and yes/no lines every question must
/// carry — about 300 B — and not the rubric, ten times that, which is said
/// once.
const OPTION_LINES_BYTES_MAX: usize = 512;

/* ---- the stand-in model --------------------------------------------------- */

/// The position a question names its letter by — `letters[3]` — when it names
/// one: the batch road's way. A question that names none is the per-letter
/// road's, whose state is the one letter.
fn letters_index(said: &str) -> Option<usize> {
    let (_, rest) = said.split_once("letters[")?;
    rest.split_once(']')?.0.parse().ok()
}

/// The kind of the letter `question` asks about, read as the model would
/// read it: off the entry its words name, or off the state itself.
fn kind_asked_about<'a>(body: &'a Value, question: &Value) -> Option<&'a str> {
    let said = question["instructions"].as_str()?;
    match letters_index(said) {
        Some(at) => body["state"]["letters"][at]["kind"].as_str(),
        None => body["state"]["kind"].as_str(),
    }
}

/// What the stand-in answers a choice: the word the kind rule gives the
/// letter's kind, at 0.8 and the other two options at 0.1 apiece.
fn choice_by_the_kind_rule(question: &Value, kind: Option<&str>) -> Value {
    let options: Vec<&str> = question["criteria"]
        .as_object()
        .map(|criteria| criteria.keys().map(String::as_str).collect())
        .unwrap_or_default();
    let ruled = kind
        .and_then(|kind| serde_json::from_value::<MessageKind>(json!(kind)).ok())
        .map(|kind| kind_rule(kind).word());
    let chosen = ruled
        .filter(|word| options.contains(word))
        .or_else(|| options.first().copied())
        .unwrap_or("none");
    let probabilities: Map<String, Value> = options
        .iter()
        .map(|word| {
            let share = if *word == chosen { 0.8 } else { 0.1 };
            ((*word).to_string(), json!(share))
        })
        .collect();
    json!({
        "type": "choice",
        "choice": chosen,
        "probabilities": probabilities,
        "confidence": 0.8,
    })
}

/// How the stand-in answers one request. `spoil` names the kind whose letters
/// it gives an answer that breaks the closed choice's rules — a model that
/// is wrong about some letters and in shape about the rest.
fn stand_in_answer(request: &str, spoil: Option<MessageKind>) -> String {
    let body: Value = request
        .split_once("\r\n\r\n")
        .and_then(|(_, body)| serde_json::from_str(body).ok())
        .unwrap_or(Value::Null);
    let mut answers = Map::new();
    if let Some(questions) = body["questions"].as_object() {
        for (name, question) in questions {
            let kind = kind_asked_about(&body, question);
            let answer = if question["type"] == "noul" {
                let stopped = matches!(kind, Some("question" | "worker_done"));
                let yes = if stopped { 0.9 } else { 0.1 };
                json!({ "type": "noul", "noul": yes })
            } else if spoil.is_some_and(|spoiled| Some(spoiled.as_str()) == kind) {
                json!({ "type": "choice", "choice": "later" })
            } else {
                choice_by_the_kind_rule(question, kind)
            };
            answers.insert(name.clone(), answer);
        }
    }
    json!({
        "model": ANSWERING_VERSION,
        "usage": { "input_tokens": request.len() / 4, "output_tokens": 0 },
        "answers": answers,
    })
    .to_string()
}

/// The stand-in as an endpoint, holding each request `hold_ms` before it
/// answers: the model's round trip, as a number.
fn stand_in(hold_ms: u64) -> Endpoint {
    Endpoint::answering_each(
        "HTTP/1.1 200 OK",
        |request: &str| stand_in_answer(request, None),
        hold_ms,
    )
}

/* ---- the window ----------------------------------------------------------- */

/// A job the beat hands off, as the host is handed it.
type Job = Box<dyn FnOnce() + Send>;

/// A window with one coordinator pane: it hands out the wire and the
/// coordinator's checkout and nothing else, and runs a job where it is handed
/// it — the trait's own default — so a test sees what a sweep did the moment
/// it returns.
struct Window {
    wire: Wire,
    checkout: PathBuf,
    /// Jobs handed off the beat, kept while a case times the beat's own work
    /// apart from theirs; `None` runs them where they are handed over.
    held: Mutex<Option<Vec<Job>>>,
}

impl Window {
    fn new(wire: Wire, checkout: PathBuf) -> Self {
        Self {
            wire,
            checkout,
            held: Mutex::new(None),
        }
    }

    /// From now on a job is kept until [`Self::run_held`]: the beat returns
    /// as soon as it has gathered what it found.
    fn hold_jobs(&self) {
        *self.held.lock().unwrap_or_else(|held| held.into_inner()) = Some(Vec::new());
    }

    /// Run every job kept so far, and keep holding the next ones.
    fn run_held(&self) {
        let jobs = self
            .held
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .replace(Vec::new());
        for job in jobs.unwrap_or_default() {
            job();
        }
    }
}

impl Host for Window {
    fn split(
        &self,
        _team: &str,
        _leader_term: u32,
        _from_term: u32,
        _pane: &str,
        _direction: zerocode_core::agent_teams::Direction,
        _command: &str,
        _token: &str,
    ) -> Option<u32> {
        None
    }
    fn send(&self, _term: u32, _text: &str) -> bool {
        false
    }
    fn capture(&self, _term: u32) -> Option<String> {
        None
    }
    fn focus(&self, _term: u32) -> bool {
        false
    }
    fn close(&self, _term: u32) {}
    fn jev_wire(&self) -> Option<Wire> {
        Some(self.wire.clone())
    }
    fn worktree_of(&self, _term: u32) -> Option<PathBuf> {
        Some(self.checkout.clone())
    }
    fn off_the_beat(&self, job: Job) {
        let mut held = self.held.lock().unwrap_or_else(|held| held.into_inner());
        match held.as_mut() {
            Some(jobs) => jobs.push(job),
            None => {
                drop(held);
                job();
            }
        }
    }
}

/// A zo home of the case's own: the mail triage at `mode`, consenting to
/// `consented`, and `budget` requests a day when one is named. The settings
/// file's path.
fn settings(
    home: &tempfile::TempDir,
    mode: &str,
    consented: &Path,
    budget: Option<u64>,
) -> PathBuf {
    let path = home.path().join("settings.json");
    let mut jev = json!({ "workspaces": [consented.display().to_string()] });
    if let Some(budget) = budget {
        jev["dailyRequests"] = json!(budget);
    }
    std::fs::write(
        &path,
        json!({ SMART_SETTINGS_KEY: { MAIL_TRIAGE.setting: mode, "jev": jev } }).to_string(),
    )
    .expect("zo's settings");
    path
}

/// Every row the ledger holds, oldest first.
fn rows(ledger: &Path) -> Vec<Value> {
    std::fs::read_to_string(ledger)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// The body of a request the endpoint heard, as the model reads it.
fn body_of(request: &str) -> Value {
    request
        .split_once("\r\n\r\n")
        .and_then(|(_, body)| serde_json::from_str(body).ok())
        .unwrap_or(Value::Null)
}

/// A book that has read its ledger's tail, as the beat's first sweep leaves it.
fn open_book(ledger: &Path) -> Arc<Mutex<MailBook>> {
    let book = Arc::new(Mutex::new(MailBook::default()));
    kept(&book).waiting = Some((ledger.to_path_buf(), Vec::new()));
    book
}

/* ---- the synthetic inbox -------------------------------------------------- */

/// One worker, the task it carries and the attempt it is on: what a letter
/// is about.
struct Hand {
    worker: String,
    task: String,
    dispatch: Option<String>,
}

/// One letter as the inbox posted it.
struct Posted {
    id: String,
    kind: MessageKind,
    worker: String,
    task: String,
}

/// A run with its coordinator seated, the workers its letters are about, and
/// the letters, as they arrive in batches. The letters' words are never
/// written.
struct Inbox {
    ledger: Ledger,
    run: String,
    /// Workers that report and ask: letters of their own.
    talkers: Vec<Hand>,
    /// Workers the ledger notices about.
    quiet: Vec<Hand>,
    /// Workers that report done, once each.
    finishing: Vec<Hand>,
    done: usize,
    letters: Vec<Posted>,
    batches: Vec<Range<usize>>,
}

impl Inbox {
    /// An inbox that will be handed `letters` letters in the mix [`KINDS`]
    /// names.
    fn new(letters: usize) -> Self {
        Self::with_quiet_hands(letters, QUIET_HANDS)
    }

    /// The same, with `quiet` workers for the ledger's notices to be about:
    /// each on an attempt of its own, so a long ledger holds that many.
    fn with_quiet_hands(letters: usize, quiet: usize) -> Self {
        let mut ledger = Ledger::new();
        let run = ledger.create_run("synthetic day", 1);
        ledger
            .seat_coordinator(&run, "team/%1", Some(COORDINATOR), 2)
            .expect("seated");
        let done_in_a_round = KINDS
            .iter()
            .filter(|kind| **kind == MessageKind::WorkerDone)
            .count();
        let finishing = letters.div_ceil(KINDS.len()) * done_in_a_round;
        let talkers = Self::hands(&mut ledger, &run, 2, 6);
        let quiet = Self::hands(&mut ledger, &run, 8, quiet);
        let finishing = Self::hands(&mut ledger, &run, 8 + quiet.len(), finishing);
        Self {
            ledger,
            run,
            talkers,
            quiet,
            finishing,
            done: 0,
            letters: Vec::new(),
            batches: Vec::new(),
        }
    }

    /// `count` workers, each on a task of its own, in panes from `first`.
    fn hands(ledger: &mut Ledger, run: &str, first: usize, count: usize) -> Vec<Hand> {
        (first..first + count)
            .map(|pane| {
                let task = ledger
                    .create_task(run, "spec".into(), format!("task {pane}"), vec![], None, 3)
                    .expect("a task");
                let seat = format!("%{pane}");
                let started = ledger
                    .start_worker(run, "claude", ("team", seat.as_str()), Some(&task), 4)
                    .expect("a worker");
                Hand {
                    worker: started.worker,
                    task,
                    dispatch: started.dispatch,
                }
            })
            .collect()
    }

    /// One letter of `kind` at `at`: from the worker it is about when a worker
    /// says it, from the ledger when the ledger notices it.
    fn post(&mut self, kind: MessageKind, at: i64) {
        let hand = match kind {
            MessageKind::WorkerDone => {
                let hand = &self.finishing[self.done];
                self.done += 1;
                hand
            }
            MessageKind::Status | MessageKind::Question => {
                &self.talkers[self.letters.len() % self.talkers.len()]
            }
            _ => &self.quiet[self.letters.len() % self.quiet.len()],
        };
        let by_a_worker = matches!(
            kind,
            MessageKind::Status | MessageKind::Question | MessageKind::WorkerDone
        );
        let from = if by_a_worker {
            worker_address(&hand.worker)
        } else {
            LEDGER_ITSELF.to_string()
        };
        let body = if kind == MessageKind::WorkerDone {
            r#"{"ok":true,"summary":"synthetic"}"#
        } else {
            "words nobody reads"
        };
        let id = self
            .ledger
            .post(
                &self.run,
                Draft {
                    from,
                    to: format!("run:{}", self.run),
                    kind,
                    body: Text::from(body),
                    subject: Text::default(),
                    priority: Priority::Normal,
                    payload: Text::default(),
                    thread: None,
                    task: Some(hand.task.clone()),
                    dispatch: hand.dispatch.clone(),
                },
                at,
            )
            .expect("a letter");
        self.letters.push(Posted {
            id,
            kind,
            worker: hand.worker.clone(),
            task: hand.task.clone(),
        });
    }

    /// A batch of letters arriving together at `at`, a hundred milliseconds
    /// apart.
    fn arrive(&mut self, kinds: &[MessageKind], at: i64) {
        let first = self.letters.len();
        for (n, kind) in kinds.iter().enumerate() {
            self.post(*kind, at + LETTER_GAP_MS * i64::try_from(n).unwrap_or(0));
        }
        self.batches.push(first..self.letters.len());
    }

    /// The run, as the ledger holds it now.
    fn run(&self) -> &Run {
        self.ledger.run(&self.run).expect("the run")
    }

    /// How many letters the coordinator's inbox holds.
    fn letters_held(&self) -> usize {
        let none = Vec::new();
        Mailroom::of_run(self.run(), &none).letters().count()
    }
}

/// The kinds of each batch of a day: [`DAY`]'s sizes in the order [`STRIDE`]
/// steps through them, the kinds in the mix [`KINDS`] names.
fn the_day() -> Vec<Vec<MessageKind>> {
    let sizes: Vec<usize> = DAY
        .iter()
        .flat_map(|(size, batches)| std::iter::repeat_n(*size, *batches))
        .collect();
    let mut next = 0;
    (0..sizes.len())
        .map(|n| {
            let size = sizes[(n * STRIDE) % sizes.len()];
            let kinds: Vec<MessageKind> = (next..next + size)
                .map(|k| KINDS[k % KINDS.len()])
                .collect();
            next += size;
            kinds
        })
        .collect()
}

/// One sweep of a day: how many letters it was handed, how long it took to
/// triage them — the time to a triaged batch — and which of the model's
/// requests were its.
struct Sweep {
    letters: usize,
    took: Duration,
    requests: Range<usize>,
}

/// A day's sweeps: each batch arrives, and one sweep of the beat sees it.
/// `heard` says how many requests the model has heard so far, so a sweep can
/// name its own.
fn run_the_day(
    window: &Window,
    book: &Arc<Mutex<MailBook>>,
    ledger: &Path,
    inbox: &mut Inbox,
    batches: &[Vec<MessageKind>],
    heard: &dyn Fn() -> usize,
) -> Vec<Sweep> {
    let mut sweeps = Vec::new();
    for (n, kinds) in batches.iter().enumerate() {
        let at = DAY_START_MS + MINUTE_MS * i64::try_from(n).unwrap_or(0);
        inbox.arrive(kinds, at);
        let seated = [(inbox.run(), TERM)];
        let first = heard();
        let began = Instant::now();
        ask_about(
            window,
            &window.wire,
            book,
            ledger,
            &seated,
            at + SWEEP_AFTER_MS,
        );
        sweeps.push(Sweep {
            letters: kinds.len(),
            took: began.elapsed(),
            requests: first..heard(),
        });
    }
    sweeps
}

/// Every string of `value` that is long enough to be the product's words.
fn words_in(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::String(text) if text.len() >= SHARED_WORDS_MIN_BYTES => found.push(text.clone()),
        Value::Array(items) => items.iter().for_each(|item| words_in(item, found)),
        Value::Object(fields) => fields.values().for_each(|item| words_in(item, found)),
        _ => {}
    }
}

/// The bytes of the product's words that left more than once: every copy of a
/// text after its first.
fn repeated_shared_bytes(bodies: &[Value]) -> usize {
    let mut found = Vec::new();
    bodies.iter().for_each(|body| words_in(body, &mut found));
    let mut copies: HashMap<&str, usize> = HashMap::new();
    for text in &found {
        *copies.entry(text.as_str()).or_default() += 1;
    }
    copies
        .iter()
        .map(|(text, copies)| text.len() * (copies - 1))
        .sum()
}

/// What a day came to: read off the rows the seat wrote and, where something
/// listened, the requests the model heard.
struct Account {
    letters: usize,
    batches: usize,
    requests: u64,
    request_bytes: u64,
    input_tokens: u64,
    /// The bytes of the product's words that left more than once among the
    /// requests of ONE delivered batch, summed over the day — what asking a
    /// batch letter by letter pays and asking it together does not. `None`
    /// where nothing listened (the real model).
    repeated_in_a_batch: Option<usize>,
    /// The same, among every request of the day: what no road avoids, a
    /// request carrying its own context.
    repeated_in_a_day: Option<usize>,
    /// How many requests carried the rubric.
    rubrics: Option<usize>,
    outcomes: BTreeMap<String, usize>,
}

fn account(rows: &[Value], listened: Option<&Endpoint>, sweeps: &[Sweep]) -> Account {
    let sum = |key: &str| -> u64 { rows.iter().map(|row| row[key].as_u64().unwrap_or(0)).sum() };
    let heard = listened.map(Endpoint::asked);
    let bodies: Option<Vec<Value>> = heard
        .as_ref()
        .map(|heard| heard.iter().map(|request| body_of(request)).collect());
    let mut outcomes: BTreeMap<String, usize> = BTreeMap::new();
    for row in rows {
        *outcomes
            .entry(row["outcome"].as_str().unwrap_or("none").to_string())
            .or_default() += 1;
    }
    Account {
        letters: rows.len(),
        batches: sweeps.len(),
        requests: sum("requests"),
        request_bytes: sum("requestBytes"),
        input_tokens: sum("inputTokens"),
        repeated_in_a_batch: bodies.as_ref().map(|bodies| {
            sweeps
                .iter()
                .map(|sweep| repeated_shared_bytes(&bodies[sweep.requests.clone()]))
                .sum()
        }),
        repeated_in_a_day: bodies.as_ref().map(|bodies| repeated_shared_bytes(bodies)),
        rubrics: heard.as_ref().map(|heard| {
            heard
                .iter()
                .map(|request| request.matches(RUBRIC_OPENING).count())
                .sum()
        }),
        outcomes,
    }
}

/* ---- the coordinator's day ------------------------------------------------ */

/// What decides how a synthetic coordinator behaves: a dice that rolls the
/// same every run (a linear congruential generator; the multiplier and the
/// increment are Knuth's MMIX).
struct Dice(u64);

impl Dice {
    const MULTIPLIER: u64 = 6_364_136_223_846_793_005;
    const INCREMENT: u64 = 1_442_695_040_888_963_407;
    /// The bits the generator's low end is too regular to use.
    const DISCARDED_BITS: u32 = 33;

    /// A roll under `outof`.
    fn roll(&mut self, outof: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(Self::MULTIPLIER)
            .wrapping_add(Self::INCREMENT);
        (self.0 >> Self::DISCARDED_BITS) % outof
    }
}

/// How often the synthetic coordinator handles a letter of a kind, in
/// percent: what somebody waits on nearly always, a worker's news about half
/// the time, a silence told again rarely.
fn handled_percent(kind: MessageKind) -> u64 {
    match kind {
        MessageKind::Question => 95,
        MessageKind::WorkerDone | MessageKind::QuotaWalled => 90,
        MessageKind::Status => 50,
        MessageKind::WentQuiet => 25,
        MessageKind::Handover => 20,
        _ => 10,
    }
}

/// The order a coordinator takes a batch in: what somebody waits on, then the
/// rest, each in the order it came.
fn rank(kind: MessageKind) -> u8 {
    match kind {
        MessageKind::Question => 0,
        MessageKind::WorkerDone | MessageKind::QuotaWalled => 1,
        _ => 2,
    }
}

/// Acts that touch nothing a letter is about, between two handled ones: none,
/// one or two.
const FILLER_ACTS: u64 = 3;

/// The seeds the synthetic coordinator is run under: the same letters, three
/// different days of behaviour, so a difference between two roads has to hold
/// under each of them.
const SEEDS: [u64; 3] = [7, 11, 13];

/// When the labels are read: long after the day and its closing acts.
const LABEL_NOW_MS: i64 = DAY_START_MS + MINUTE_MS * 200;

impl Inbox {
    /// One receipt of the coordinator's, filed at `at`.
    fn receipt(verb: &str, answer: &Value, at: i64, n: usize) -> ServedRow {
        ServedRow {
            caller: Some(Text::from(COORDINATOR)),
            request: Text::from(format!("r-{n}")),
            answer: ServedAnswer::Inline(answer.to_string()),
            fingerprint: None,
            verb: Some(verb.to_string()),
            filed_ms: Some(at),
            expired: false,
        }
    }

    /// The act by which the coordinator handles letter `n`, filed at `at` in
    /// `ledger`: a question is answered in its thread, a finished task is
    /// reviewed, and any other letter is dealt with by a word to the worker it
    /// is about.
    fn handling(&self, ledger: &mut Ledger, n: usize, at: i64, filed: usize) -> ServedRow {
        let letter = &self.letters[n];
        match letter.kind {
            MessageKind::Question => {
                let reply = ledger
                    .post(
                        &self.run,
                        Draft {
                            from: format!("run:{}", self.run),
                            to: worker_address(&letter.worker),
                            kind: MessageKind::Question,
                            body: Text::from("yes"),
                            subject: Text::default(),
                            priority: Priority::Normal,
                            payload: Text::default(),
                            thread: Some(letter.id.clone()),
                            task: None,
                            dispatch: None,
                        },
                        at,
                    )
                    .expect("an answer");
                Self::receipt("reply", &json!({ "messageId": reply }), at, filed)
            }
            MessageKind::WorkerDone => {
                Self::receipt("task-update", &json!({ "taskId": letter.task }), at, filed)
            }
            _ => Self::receipt("send", &json!({ "workerId": letter.worker }), at, filed),
        }
    }

    /// The ledger once a synthetic coordinator, rolling under `seed`, has dealt
    /// with the day: each batch taken in the order [`rank`] gives, each letter
    /// handled as often as [`handled_percent`] says, and enough acts at the
    /// end that a letter nothing named is given up. Every letter has been
    /// handed over. The inbox itself is left as it was, to be run under
    /// another seed.
    fn coordinated(&self, seed: u64) -> Ledger {
        let mut dice = Dice(seed);
        let mut ledger = Ledger::rebuild(self.ledger.export()).expect("a copy");
        let mut receipts = Vec::new();
        for (b, batch) in self.batches.iter().enumerate() {
            let mut at =
                DAY_START_MS + MINUTE_MS * i64::try_from(b).unwrap_or(0) + COORDINATOR_AFTER_MS;
            let mut order: Vec<usize> = batch.clone().collect();
            order.sort_by_key(|n| rank(self.letters[*n].kind));
            for n in order {
                if dice.roll(100) >= handled_percent(self.letters[n].kind) {
                    continue;
                }
                for _ in 0..dice.roll(FILLER_ACTS) {
                    let filed = receipts.len();
                    receipts.push(Self::receipt(
                        "task-create",
                        &json!({ "taskId": format!("t-new-{filed}") }),
                        at,
                        filed,
                    ));
                    at += ACT_GAP_MS;
                }
                let filed = receipts.len();
                receipts.push(self.handling(&mut ledger, n, at, filed));
                at += ACT_GAP_MS;
            }
        }
        let mut at = DAY_START_MS + MINUTE_MS * i64::try_from(self.batches.len()).unwrap_or(0);
        for _ in 0..=NO_NEED_AFTER_ACTS {
            let filed = receipts.len();
            receipts.push(Self::receipt(
                "task-create",
                &json!({ "taskId": format!("t-new-{filed}") }),
                at,
                filed,
            ));
            at += ACT_GAP_MS;
        }
        let mut projected = ledger.export();
        projected.served.extend(receipts);
        for inbox in projected
            .inboxes
            .iter_mut()
            .filter(|row| row.run == self.run && row.address == format!("run:{}", self.run))
        {
            inbox.pending.clear();
        }
        Ledger::rebuild(projected).expect("the ledger with the coordinator's day")
    }
}

/// The labels every seed's coordinator wrote for the answers `book` holds
/// waiting, one list a seed.
fn labelled(book: &Arc<Mutex<MailBook>>, inbox: &Inbox) -> Vec<(u64, Vec<Value>)> {
    let (path, waiting) = kept(book).waiting.clone().unwrap_or_default();
    SEEDS
        .into_iter()
        .map(|seed| {
            let mut copy = MailBook {
                waiting: Some((path.clone(), waiting.clone())),
                ..MailBook::default()
            };
            let ledger = inbox.coordinated(seed);
            (seed, label_waiting(&mut copy, &ledger, LABEL_NOW_MS))
        })
        .collect()
}

/// What each seed's labels say of the answers, with the kind rule beside them.
fn agreements(by_seed: &[(u64, Vec<Value>)]) -> Vec<(u64, Agreement)> {
    by_seed
        .iter()
        .map(|(seed, labels)| (*seed, agreement(labels)))
        .collect()
}

/// What the labels said of the answers: how many letters were compared, how
/// many the seat's answer agreed with what the coordinator did, and how many
/// the kind rule agreed with, on the same letters.
#[derive(Debug, Default, PartialEq)]
struct Agreement {
    compared: usize,
    answered: usize,
    rule: usize,
    not_compared: usize,
}

impl Agreement {
    /// Another seed's tally, pooled in.
    fn pool(&mut self, other: &Self) {
        self.compared += other.compared;
        self.answered += other.answered;
        self.rule += other.rule;
        self.not_compared += other.not_compared;
    }

    fn json(&self) -> Value {
        json!({
            "compared": self.compared,
            "answered": self.answered,
            "rule": self.rule,
            "notCompared": self.not_compared,
        })
    }
}

fn agreement(labels: &[Value]) -> Agreement {
    let mut tally = Agreement::default();
    for label in labels {
        if label.get(NOT_COMPARED.canonical).is_some() {
            tally.not_compared += 1;
        } else if let Some(agreed) = label[AGREED.canonical].as_bool() {
            tally.compared += 1;
            tally.answered += usize::from(agreed);
            tally.rule += usize::from(label[BASELINE_AGREED.canonical].as_bool() == Some(true));
        }
    }
    tally
}

/* ---- the cases ------------------------------------------------------------ */

/// The case's own checkout, home, ledger and window on `endpoint`.
struct Stand {
    _home: tempfile::TempDir,
    _work: tempfile::TempDir,
    ledger: PathBuf,
    window: Window,
    book: Arc<Mutex<MailBook>>,
}

fn stand_on(endpoint: &Endpoint, mode: &str, budget: Option<u64>, key: &str) -> Stand {
    stand_at(&endpoint.base(), mode, budget, key)
}

/// The same, on the origin `base` names.
fn stand_at(base: &str, mode: &str, budget: Option<u64>, key: &str) -> Stand {
    let home = tempfile::tempdir().expect("a zo home");
    let work = tempfile::tempdir().expect("a checkout");
    let settings = settings(&home, mode, work.path(), budget);
    let ledger = home.path().join(MAIL_TRIAGE.ledger);
    let wire = Wire::at(base, key, Some(settings));
    let book = open_book(&ledger);
    Stand {
        window: Window::new(wire, work.path().to_path_buf()),
        _home: home,
        _work: work,
        ledger,
        book,
    }
}

impl Stand {
    /// Hand `kinds` to the coordinator at once and let one sweep see them.
    fn sweep(&self, inbox: &mut Inbox, kinds: &[MessageKind]) -> Vec<Value> {
        inbox.arrive(kinds, DAY_START_MS);
        let seated = [(inbox.run(), TERM)];
        ask_about(
            &self.window,
            &self.window.wire,
            &self.book,
            &self.ledger,
            &seated,
            DAY_START_MS + SWEEP_AFTER_MS,
        );
        rows(&self.ledger)
    }
}

/// A batch of letters the coordinator is handed is ONE request: every letter
/// has a row of its own, each answered by the question that named it, and the
/// shared words travelled once.
#[test]
fn a_batch_of_letters_is_one_request_and_every_letter_is_answered_by_its_own_question() {
    let endpoint = stand_in(0);
    let stand = stand_on(&endpoint, "shadow", None, "test-key");
    let mut inbox = Inbox::new(5);
    let kinds = [
        MessageKind::Question,
        MessageKind::Status,
        MessageKind::WentQuiet,
        MessageKind::WorkerDone,
        MessageKind::Handover,
    ];
    let written = stand.sweep(&mut inbox, &kinds);

    let heard = endpoint.asked();
    assert_eq!(
        heard.len(),
        1,
        "five letters handed over together are one request"
    );
    assert_eq!(written.len(), 5, "and every letter has its own row");
    for (row, kind) in written.iter().zip(kinds) {
        assert_eq!(row["outcome"], ANSWERED, "{row}");
        assert_eq!(
            row[TRIAGE],
            kind_rule(kind).word(),
            "{}: the letter was answered by another letter's question",
            kind.as_str()
        );
        assert_eq!(row["kind"], kind.as_str());
    }
    assert_eq!(
        heard[0].matches(RUBRIC_OPENING).count(),
        1,
        "the rubric travels once"
    );
    let ids: Vec<&str> = written.iter().filter_map(|row| row[KEY].as_str()).collect();
    let posted: Vec<&str> = inbox.letters.iter().map(|one| one.id.as_str()).collect();
    assert_eq!(ids, posted, "the rows are the letters, oldest first");
    assert_eq!(
        kept(&stand.book)
            .waiting
            .as_ref()
            .map(|(_, waiting)| waiting.len()),
        Some(5),
        "every answer waits for its label"
    );
}

/// A request that judged five letters is counted once: the rows add up to one
/// request, its bytes and its tokens, however many rows it answered.
#[test]
fn the_rows_of_a_batch_count_its_request_once() {
    let endpoint = stand_in(0);
    let stand = stand_on(&endpoint, "shadow", None, "test-key");
    let mut inbox = Inbox::new(5);
    let written = stand.sweep(&mut inbox, &KINDS[..5]);

    let heard = endpoint.asked();
    assert_eq!(heard.len(), 1, "one request for the batch");
    let body_bytes = heard[0]
        .split_once("\r\n\r\n")
        .map_or(0, |(_, body)| body.len());
    let sum = |key: &str| -> u64 {
        written
            .iter()
            .map(|row| row[key].as_u64().unwrap_or(0))
            .sum()
    };
    assert_eq!(sum("requests"), 1, "one request, however many rows");
    assert_eq!(
        sum("requestBytes"),
        u64::try_from(body_bytes).unwrap_or(0),
        "its bytes once"
    );
    assert_eq!(
        sum("inputTokens"),
        u64::try_from(heard[0].len() / 4).unwrap_or(0),
        "its tokens once"
    );
    assert_eq!(
        written[0]["requests"], 1,
        "the first row carries the account"
    );
    for row in &written[1..] {
        assert_eq!(row["requests"], 0, "the rows beside it ride its request");
        assert_eq!(row["requestBytes"], 0);
        assert_eq!(row["model"], ANSWERING_VERSION, "and name who answered");
    }
    let named: Vec<&str> = written
        .iter()
        .filter_map(|row| row["batch"].as_str())
        .collect();
    assert_eq!(named.len(), 5, "every row names the batch it was asked in");
    assert!(named.iter().all(|batch| *batch == named[0]), "{named:?}");
    assert!(written.iter().all(|row| row["batchSize"] == 5));
}

/// One letter's broken answer is that letter's own: the letters beside it in
/// the same request are answered and wait for their labels.
#[test]
fn one_letters_broken_answer_leaves_the_letters_beside_it_answered() {
    let endpoint = Endpoint::answering_each(
        "HTTP/1.1 200 OK",
        |request: &str| stand_in_answer(request, Some(MessageKind::WentQuiet)),
        0,
    );
    let stand = stand_on(&endpoint, "shadow", None, "test-key");
    let mut inbox = Inbox::new(4);
    let kinds = [
        MessageKind::Question,
        MessageKind::WentQuiet,
        MessageKind::Status,
        MessageKind::WorkerDone,
    ];
    let written = stand.sweep(&mut inbox, &kinds);

    assert_eq!(endpoint.asked().len(), 1, "four letters are one request");
    assert_eq!(written.len(), 4);
    assert_eq!(written[1]["outcome"], "schema_unknown_option");
    assert!(written[1].get(TRIAGE).is_none(), "no verdict is claimed");
    for answered in [0, 2, 3] {
        assert_eq!(
            written[answered]["outcome"], ANSWERED,
            "{}",
            written[answered]
        );
    }
    assert_eq!(
        kept(&stand.book)
            .waiting
            .as_ref()
            .map(|(_, waiting)| waiting.len()),
        Some(3),
        "the broken one waits for no label"
    );
}

/// A burst above the cap is cut evenly and asked side by side: the wait is
/// the slowest request's, not the sum of them.
#[test]
fn a_burst_above_the_cap_is_cut_evenly_and_asked_side_by_side() {
    // Held long enough that a machine at full load, which can park a thread
    // for a hundred milliseconds, is still far from the three holds in a row
    // (1,200 ms) a one-after-another road would take.
    const HOLD_MS: u64 = 400;
    let endpoint = stand_in(HOLD_MS);
    let stand = stand_on(&endpoint, "shadow", None, "test-key");
    let letters = MAIL_TRIAGE_BATCH_CAP * 2 + 1;
    let mut inbox = Inbox::new(letters);
    let kinds: Vec<MessageKind> = (0..letters).map(|n| KINDS[n % KINDS.len()]).collect();
    let began = Instant::now();
    let written = stand.sweep(&mut inbox, &kinds);
    let took = began.elapsed();

    let heard = endpoint.asked();
    assert_eq!(
        heard.len(),
        3,
        "{letters} letters at a cap of {MAIL_TRIAGE_BATCH_CAP}"
    );
    let sizes: Vec<usize> = heard
        .iter()
        .map(|request| {
            body_of(request)["state"]["letters"]
                .as_array()
                .map_or(0, Vec::len)
        })
        .collect();
    assert_eq!(sizes.iter().sum::<usize>(), letters, "{sizes:?}");
    let widest = sizes.iter().max().copied().unwrap_or(0);
    let narrowest = sizes.iter().min().copied().unwrap_or(0);
    assert!(widest <= MAIL_TRIAGE_BATCH_CAP, "{sizes:?}");
    assert!(widest - narrowest <= 1, "{sizes:?}");
    assert!(
        took < Duration::from_millis(HOLD_MS * 2),
        "three requests held {HOLD_MS} ms each came back after {took:?}: asked one after another"
    );
    assert_eq!(written.len(), letters);
    assert!(written.iter().all(|row| row["outcome"] == ANSWERED));
    assert_eq!(
        written.iter().filter(|row| row["requests"] == 1).count(),
        3,
        "each of the three requests is counted once, on one row"
    );
}

/// A day of 172 letters in 107 batches: a request for each batch, every
/// letter once, the rubric said once per request, and the account of the
/// requests the rows add up to the requests the endpoint heard.
#[test]
fn a_day_costs_a_request_a_batch_and_every_letter_is_asked_once() {
    let endpoint = stand_in(0);
    let stand = stand_on(&endpoint, "shadow", None, "test-key");
    let day = the_day();
    let letters: usize = day.iter().map(Vec::len).sum();
    assert_eq!(letters, 172, "the shape of the day");
    assert_eq!(day.len(), 107);
    let mut inbox = Inbox::new(letters);
    let sweeps = run_the_day(
        &stand.window,
        &stand.book,
        &stand.ledger,
        &mut inbox,
        &day,
        &|| endpoint.count(),
    );

    let written = rows(&stand.ledger);
    let held = inbox.letters_held();
    let account = account(&written, Some(&endpoint), &sweeps);
    let heard = endpoint.count();
    assert_eq!(
        heard, account.batches,
        "a request for every batch, not for every letter ({} letters)",
        account.letters
    );
    assert_eq!(account.letters, held, "every letter has a row");
    let mut keys: Vec<&str> = written.iter().filter_map(|row| row[KEY].as_str()).collect();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), held, "and none has two");
    assert_eq!(
        account.requests,
        u64::try_from(heard).unwrap_or(0),
        "the rows add up to the requests that left"
    );
    assert_eq!(
        account.rubrics,
        Some(heard),
        "the rubric travelled once in each of them"
    );
    assert!(
        sweeps
            .iter()
            .all(|sweep| sweep.requests.len() == 1 && sweep.letters > 0),
        "every sweep asked its batch in one request"
    );
    let extra_letters = account.letters - account.batches;
    assert!(
        account
            .repeated_in_a_batch
            .is_some_and(|repeated| repeated <= extra_letters * OPTION_LINES_BYTES_MAX),
        "the product's words left more than once inside a batch: {:?} bytes for {extra_letters} letters beyond each batch's first",
        account.repeated_in_a_batch
    );
    assert!(
        account.outcomes.keys().all(|outcome| outcome == ANSWERED),
        "{:?}",
        account.outcomes
    );
}

/// What a letter nobody could judge keeps: the word the refusal gave it, no
/// verdict, no label to wait for — and so the kind rule, which reads the kind
/// alone and which every label is marked beside (`kind_rule`), is the only
/// reader the letter has: exactly as before the batch road, whichever way Jev
/// was out of reach.
#[test]
fn a_jev_that_is_not_there_leaves_every_letter_the_word_it_would_have_had() {
    let kinds = [
        MessageKind::Question,
        MessageKind::Status,
        MessageKind::WentQuiet,
    ];
    for token in [
        "no_key",
        "budget",
        "http_503",
        "rate_limited",
        "schema",
        "transport",
        "not_consented",
    ] {
        let endpoint = match token {
            "http_503" => {
                Endpoint::serving("HTTP/1.1 503 Service Unavailable", "{}".to_string(), 0)
            }
            "rate_limited" => {
                Endpoint::serving("HTTP/1.1 429 Too Many Requests", "{}".to_string(), 0)
            }
            "schema" => Endpoint::serving("HTTP/1.1 200 OK", "{\"nothing\":1}".to_string(), 0),
            _ => stand_in(0),
        };
        // Nothing listens at the last address: the connection is refused.
        let base = if token == "transport" {
            "http://127.0.0.1:1".to_string()
        } else {
            endpoint.base()
        };
        let key = if token == "no_key" { "" } else { "test-key" };
        let budget = (token == "budget").then_some(0);
        let mut stand = stand_at(&base, "shadow", budget, key);
        // The words come from a folder nobody consented to.
        let elsewhere = tempfile::tempdir().expect("another folder");
        if token == "not_consented" {
            stand.window.checkout = elsewhere.path().to_path_buf();
        }
        let mut inbox = Inbox::new(3);
        let written = stand.sweep(&mut inbox, &kinds);
        assert_nothing_judged(token, &written, &stand, &inbox);
    }
}

/// Every letter of a batch nothing judged has a row that says why, no verdict
/// and no wait for a label, and is asked about no more.
fn assert_nothing_judged(token: &str, written: &[Value], stand: &Stand, inbox: &Inbox) {
    let what = token;
    assert_eq!(written.len(), 3, "{what}: a row for every letter");
    for row in written {
        assert_eq!(row["outcome"], token, "{what}: {row}");
        assert!(row.get(TRIAGE).is_none(), "{what}: a verdict nobody gave");
    }
    let book = kept(&stand.book);
    assert_eq!(
        book.waiting.as_ref().map(|(_, waiting)| waiting.len()),
        Some(0),
        "{what}: nothing waits for a label"
    );
    for letter in &inbox.letters {
        assert!(book.asked.contains(&letter.id), "{what}: asked about again");
    }
}

/// A seat switched off asks nothing, writes nothing and leaves every letter
/// to be asked about when it is switched on.
#[test]
fn a_seat_that_is_off_asks_nothing_and_leaves_the_letters_unasked() {
    let endpoint = stand_in(0);
    let stand = stand_on(&endpoint, "off", None, "test-key");
    let mut inbox = Inbox::new(3);
    let written = stand.sweep(&mut inbox, &KINDS[..3]);
    assert!(written.is_empty());
    assert!(endpoint.asked().is_empty());
    assert!(kept(&stand.book).asked.is_empty());
}

/// A letter that was asked about is not asked about again — not by the next
/// sweep, and not by a window that restarted and read its ledger's tail.
#[test]
fn a_letter_is_asked_about_once_and_a_restart_asks_none_again() {
    let endpoint = stand_in(0);
    let stand = stand_on(&endpoint, "shadow", None, "test-key");
    let mut inbox = Inbox::new(4);
    stand.sweep(&mut inbox, &KINDS[..4]);
    assert_eq!(endpoint.asked().len(), 1);

    let seated = [(inbox.run(), TERM)];
    ask_about(
        &stand.window,
        &stand.window.wire,
        &stand.book,
        &stand.ledger,
        &seated,
        DAY_START_MS + 2 * SWEEP_AFTER_MS,
    );
    assert_eq!(endpoint.asked().len(), 1, "the next sweep asks nothing");

    let (asked, waiting) = read_tail(&stand.ledger);
    assert_eq!(asked.len(), 4, "the tail names every letter asked about");
    assert_eq!(waiting.len(), 4, "and every answer still waiting");
    let restarted = Arc::new(Mutex::new(MailBook::default()));
    kept(&restarted).asked.extend(asked);
    ask_about(
        &stand.window,
        &stand.window.wire,
        &restarted,
        &stand.ledger,
        &seated,
        DAY_START_MS + 3 * SWEEP_AFTER_MS,
    );
    assert_eq!(endpoint.asked().len(), 1, "a restart asks none again");
}

/// What the coordinator did next labels a batch's letters as it labels any:
/// the seat's answer and the kind rule, each marked against the same truth —
/// under every seed the synthetic coordinator is run under.
#[test]
fn the_labels_of_a_batch_carry_both_marks_as_ever() {
    let endpoint = stand_in(0);
    let stand = stand_on(&endpoint, "shadow", None, "test-key");
    let day = the_day();
    let letters: usize = day.iter().map(Vec::len).sum();
    let mut inbox = Inbox::new(letters);
    run_the_day(
        &stand.window,
        &stand.book,
        &stand.ledger,
        &mut inbox,
        &day,
        &|| endpoint.count(),
    );
    let answered = rows(&stand.ledger).len();
    for (seed, labels) in labelled(&stand.book, &inbox) {
        assert!(
            !labels.is_empty(),
            "seed {seed}: {answered} answers, no label"
        );
        let tally = agreement(&labels);
        assert!(tally.compared > 0, "seed {seed}: {tally:?}");
        for label in labels
            .iter()
            .filter(|label| label.get(NOT_COMPARED.canonical).is_none())
        {
            assert!(label[AGREED.canonical].is_boolean(), "{label}");
            assert!(label[BASELINE_AGREED.canonical].is_boolean(), "{label}");
        }
    }
}

/* ---- the rows of a sweep that asks in waves ------------------------------- */

/// How long the request of the last wave is held before it answers, in
/// milliseconds: the model slow on that one request and quick on the rest.
/// Long enough that a machine at full load, which can park a thread for a
/// hundred milliseconds, still tells the quick requests from the slow one.
const LAST_WAVE_HOLD_MS: u64 = 800;

/// How long a case waits for the last wave's request to leave before it gives
/// up and looks at the ledger anyway.
const LAST_WAVE_WAIT: Duration = Duration::from_secs(10);

/// How often that wait looks.
const LAST_WAVE_POLL: Duration = Duration::from_millis(5);

/// One sweep over one letter more than the lanes carry in a wave, with the
/// request of the last wave held: the rows the ledger held while it was held,
/// and the rows it held when the sweep was over.
fn sweep_with_the_last_request_held() -> (Vec<Value>, Vec<Value>) {
    let letters = TOGETHER_LANES * MAIL_TRIAGE_BATCH_CAP + 1;
    let arrived = Arc::new(AtomicUsize::new(0));
    let counting = Arc::clone(&arrived);
    let endpoint = Endpoint::answering_each(
        "HTTP/1.1 200 OK",
        move |request: &str| {
            if counting.fetch_add(1, Ordering::SeqCst) >= TOGETHER_LANES {
                thread::sleep(Duration::from_millis(LAST_WAVE_HOLD_MS));
            }
            stand_in_answer(request, None)
        },
        0,
    );
    let stand = stand_on(&endpoint, "shadow", None, "test-key");
    let mut inbox = Inbox::new(letters);
    let kinds: Vec<MessageKind> = (0..letters).map(|n| KINDS[n % KINDS.len()]).collect();
    thread::scope(|scope| {
        let sweeping = scope.spawn(|| stand.sweep(&mut inbox, &kinds));
        let gave_up = Instant::now() + LAST_WAVE_WAIT;
        while arrived.load(Ordering::SeqCst) <= TOGETHER_LANES && Instant::now() < gave_up {
            thread::sleep(LAST_WAVE_POLL);
        }
        let while_held = rows(&stand.ledger);
        (while_held, sweeping.join().expect("the sweep"))
    })
}

/// The rows of the request asked last, and the rows of the requests before
/// it, in that order.
fn split_by_the_last_request(written: &[Value]) -> (Vec<&Value>, Vec<&Value>) {
    let last = written.last().map(|row| &row[BATCH]);
    written.iter().partition(|row| Some(&row[BATCH]) == last)
}

/// A row says how long ITS request waited for its answer, not how long the
/// whole sweep took: the letters of the quick requests are not told they
/// waited for the slow one that left behind them.
#[test]
fn a_row_says_how_long_its_own_request_waited() {
    let (_, written) = sweep_with_the_last_request_held();
    let (slow, quick) = split_by_the_last_request(&written);
    assert!(
        !quick.is_empty() && !slow.is_empty(),
        "{} rows",
        written.len()
    );
    for row in quick {
        let waited = row[ELAPSED_MS.canonical].as_u64().unwrap_or(u64::MAX);
        assert!(
            waited < LAST_WAVE_HOLD_MS / 2,
            "a request answered at once waited {waited} ms: {row}"
        );
    }
    for row in slow {
        let waited = row[ELAPSED_MS.canonical].as_u64().unwrap_or(0);
        assert!(
            waited >= LAST_WAVE_HOLD_MS,
            "the held request waited {waited} ms: {row}"
        );
    }
}

/// The rows of the requests that came back are in the ledger before the next
/// wave is asked: a window closed while the last request waits loses that
/// request's letters — which have no row, and are asked again — and not the
/// whole sweep's, whose requests the day's count already holds as paid for.
#[test]
fn the_rows_of_a_wave_are_written_before_the_next_wave_is_asked() {
    let (while_held, at_the_end) = sweep_with_the_last_request_held();
    let (slow, quick) = split_by_the_last_request(&at_the_end);
    assert!(
        !quick.is_empty() && !slow.is_empty(),
        "{} rows",
        at_the_end.len()
    );
    assert_eq!(
        while_held.len(),
        quick.len(),
        "the letters of the requests that had come back had {} rows while the last waited",
        while_held.len()
    );
}

/// A request's account is written on a letter that names a task when the
/// batch has one, so that what a task cost in Jev (`task_cost`) reads the
/// request as some task's and not as nobody's: the first letter of this batch,
/// a notice of the ledger's, names none.
#[test]
fn a_request_is_booked_to_a_letter_that_names_a_task() {
    let endpoint = stand_in(0);
    let stand = stand_on(&endpoint, "shadow", None, "test-key");
    let mut inbox = Inbox::new(1);
    inbox
        .ledger
        .post(
            &inbox.run,
            Draft {
                from: LEDGER_ITSELF.to_string(),
                to: format!("run:{}", inbox.run),
                kind: MessageKind::WentQuiet,
                body: Text::from("words nobody reads"),
                subject: Text::default(),
                priority: Priority::Normal,
                payload: Text::default(),
                thread: None,
                task: None,
                dispatch: None,
            },
            DAY_START_MS,
        )
        .expect("a notice that names no task");
    inbox.post(MessageKind::WorkerDone, DAY_START_MS + LETTER_GAP_MS);
    let seated = [(inbox.run(), TERM)];
    ask_about(
        &stand.window,
        &stand.window.wire,
        &stand.book,
        &stand.ledger,
        &seated,
        DAY_START_MS + SWEEP_AFTER_MS,
    );

    let written = rows(&stand.ledger);
    assert_eq!(written.len(), 2);
    assert!(
        written[0][TASK_STAMP].is_null(),
        "the first letter names no task: {}",
        written[0]
    );
    let carrying: Vec<&Value> = written.iter().filter(|row| row["requests"] == 1).collect();
    assert_eq!(carrying.len(), 1, "one request");
    assert!(
        carrying[0][TASK_STAMP].is_string(),
        "the request is booked to no task: {}",
        carrying[0]
    );
    assert_eq!(
        carrying[0][BATCH], carrying[0][KEY],
        "and the batch is named by the row that carries it"
    );
}

/* ---- measurements --------------------------------------------------------- */

/// The profile a measurement ran under, as the job names it.
const PROFILE_ENV: &str = "MAIL_MEASURE_PROFILE";

/// Which road the measured build carries — `per-letter` before the batch road
/// and `batch` after it: a name the job gives, since both builds run these
/// very tests.
const ROAD_ENV: &str = "MAIL_MEASURE_ROAD";

/// How long the stand-in holds each request before it answers, in
/// milliseconds, when the job does not say: the model's measured round trip at
/// p50 (280 ms, rounds 2 to 10 of 2026-09-21,
/// `docs/design/jev-seats-accuracy-wave-20260921.md` §2).
const ROUND_TRIP_MS: u64 = 280;

/// The key a job names the stand-in's round trip by, in milliseconds.
const ROUND_TRIP_ENV: &str = "MAIL_MEASURE_ROUND_TRIP_MS";

/// The most requests a real-wire measurement may spend.
const MAX_REAL_REQUESTS: usize = 400;

/// The key of how many days the stand-in measurement asks, one after another
/// in one process: what a long run holds is read off the process's peak
/// resident set at one day and at many.
const DAYS_ENV: &str = "MAIL_MEASURE_DAYS";

/// The key of how many times a burst is asked.
const BURST_REPS_ENV: &str = "MAIL_MEASURE_BURST_REPS";

/// How many letters the long ledger of the burst measurement already holds,
/// asked about and put away.
const HISTORY_LETTERS: usize = 2_000;

/// How many attempts the notices of the long ledger are spread over: what a
/// run that has summoned workers for weeks holds.
const HISTORY_ATTEMPTS: usize = 300;

/// How many fresh letters arrive at once in a burst.
const BURST_LETTERS: usize = 100;

/// How many times the burst is asked, for a median, when the job does not say.
const BURST_REPEATS: usize = 7;

/// What every measurement line opens with. libtest prints `test <name> ... `
/// without a newline before a test's own output, so a measurement printed
/// plainly lands at the end of that line: it opens a line of its own, behind
/// this mark, and a job finds it by the mark.
const MEASURE_MARK: &str = "MAIL-MEASURE ";

/// Print one measurement as the job collects it: on a line of its own.
fn publish(measurement: &Value) {
    println!("\n{MEASURE_MARK}{measurement}");
}

/// A number a job names in its environment, or `default`.
fn env_number<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|said| said.parse().ok())
        .unwrap_or(default)
}

/// The value at `percent` of a sorted list, by the nearest rank.
fn percentile(sorted: &[u64], percent: usize) -> u64 {
    let Some(last) = sorted.len().checked_sub(1) else {
        return 0;
    };
    sorted[((last * percent + 50) / 100).min(last)]
}

/// Microseconds since `began`.
fn micros(began: Instant) -> u64 {
    u64::try_from(began.elapsed().as_micros()).unwrap_or(u64::MAX)
}

/// Milliseconds a duration took.
fn millis(took: Duration) -> u64 {
    u64::try_from(took.as_millis()).unwrap_or(u64::MAX)
}

/// One measured day, as a line the job collects: requests, bytes, the words
/// that left more than once, the time each batch took to be triaged, and what
/// the labels said under each seed.
fn report(
    label: &str,
    account: &Account,
    sweeps: &[Sweep],
    labels: Option<&[(u64, Agreement)]>,
) -> Value {
    let mut all: Vec<u64> = sweeps.iter().map(|sweep| millis(sweep.took)).collect();
    all.sort_unstable();
    let mut by_size: BTreeMap<usize, Vec<u64>> = BTreeMap::new();
    for sweep in sweeps {
        by_size
            .entry(sweep.letters)
            .or_default()
            .push(millis(sweep.took));
    }
    let by_size: Map<String, Value> = by_size
        .into_iter()
        .map(|(size, mut taken)| {
            taken.sort_unstable();
            (
                size.to_string(),
                json!({ "batches": taken.len(), "p50": percentile(&taken, 50), "max": taken.last() }),
            )
        })
        .collect();
    let agreement = labels.map(|by_seed| {
        let mut seeds = Map::new();
        let mut pooled = Agreement::default();
        for (seed, tally) in by_seed {
            seeds.insert(seed.to_string(), tally.json());
            pooled.pool(tally);
        }
        seeds.insert("pooled".to_string(), pooled.json());
        Value::Object(seeds)
    });
    json!({
        "label": label,
        "road": std::env::var(ROAD_ENV).unwrap_or_default(),
        "profile": std::env::var(PROFILE_ENV).unwrap_or_default(),
        "letters": account.letters,
        "batches": account.batches,
        "requests": account.requests,
        "requestBytes": account.request_bytes,
        "inputTokens": account.input_tokens,
        "repeatedInABatchBytes": account.repeated_in_a_batch,
        "repeatedInADayBytes": account.repeated_in_a_day,
        "requestsCarryingTheRubric": account.rubrics,
        "outcomes": account.outcomes,
        "timeToTriagedBatchMs": {
            "p50": percentile(&all, 50),
            "p95": percentile(&all, 95),
            "max": all.last(),
            "total": all.iter().sum::<u64>(),
        },
        "bySize": by_size,
        "agreement": agreement,
    })
}

/// What each letter's row says: its id, its kind, the word the seat gave (or
/// the word it was refused with), the confidence and the probability that it
/// is urgent — a line a letter, for comparing two roads' answers offline.
fn verdicts(written: &[Value]) -> Vec<Value> {
    written
        .iter()
        .map(|row| {
            let round = |key: &str| {
                row[key]
                    .as_f64()
                    .map(|share| (share * 100.0).round() / 100.0)
            };
            let word = row[TRIAGE]
                .as_str()
                .or_else(|| row[OUTCOME.canonical].as_str());
            json!([
                row[KEY],
                row["kind"],
                word,
                round("confidence"),
                round("urgent")
            ])
        })
        .collect()
}

/// The synthetic day asked of a stand-in model that holds each request for a
/// round trip: the numbers a batch road and a per-letter road differ by,
/// without the network's noise. With `MAIL_MEASURE_DAYS` above one the day is
/// asked again and again — each with an inbox, a book and a ledger of its own
/// — so a seat that kept something from every sweep would show it as a peak
/// that climbs with the days.
///
/// **Not a check.** It prints one JSON line. Run it under the build line, on
/// both roads and both profiles (`taskpolicy -b` is the low-spec one).
#[test]
#[ignore = "a measurement, printed; not a check"]
fn measure_the_synthetic_day_on_a_stand_in_model() {
    let hold_ms = env_number(ROUND_TRIP_ENV, ROUND_TRIP_MS);
    let days = env_number::<usize>(DAYS_ENV, 1).max(1);
    let day = the_day();
    let letters: usize = day.iter().map(Vec::len).sum();
    for _ in 0..days {
        let endpoint = stand_in(hold_ms);
        let stand = stand_on(&endpoint, "shadow", None, "test-key");
        let mut inbox = Inbox::new(letters);
        let sweeps = run_the_day(
            &stand.window,
            &stand.book,
            &stand.ledger,
            &mut inbox,
            &day,
            &|| endpoint.count(),
        );
        let written = rows(&stand.ledger);
        let account = account(&written, Some(&endpoint), &sweeps);
        if days == 1 {
            publish(&report("stand-in", &account, &sweeps, None));
        }
    }
    if days > 1 {
        publish(&json!({ "label": "stand-in", "days": days, "letters": letters }));
    }
}

/// A burst of `fresh` letters arriving at once on a ledger that already holds
/// `history` letters asked about and put away, asked `reps` times: what the
/// beat itself pays to gather them, and what the job off the beat pays to turn
/// them into requests, ask and write the rows — each apart, as a median over
/// the repeats, with the whole burst's wait to be triaged beside them. The beat
/// is the part a person's screen waits on; the wait is what a coordinator
/// handed a burst of parallel results waits on.
fn burst(stand: &Stand, history: usize, fresh: usize, reps: usize) -> Value {
    let letters = history + fresh;
    let attempts = if history == 0 {
        QUIET_HANDS
    } else {
        HISTORY_ATTEMPTS
    };
    let mut inbox = Inbox::with_quiet_hands(letters, attempts);
    let kinds: Vec<MessageKind> = (0..letters).map(|n| KINDS[n % KINDS.len()]).collect();
    inbox.arrive(&kinds, DAY_START_MS);
    let asked_already: Vec<String> = inbox.letters[..history]
        .iter()
        .map(|letter| letter.id.clone())
        .collect();
    let messages = inbox.run().messages().len();
    let attempts = inbox.run().dispatches.len();
    stand.window.hold_jobs();
    let (mut beat, mut job, mut triaged): (Vec<u64>, Vec<u64>, Vec<u64>) =
        (Vec::new(), Vec::new(), Vec::new());
    let mut requests = 0;
    for rep in 0..reps {
        // A book of this repeat's own: the history is asked about already, the
        // burst is not.
        let ledger = stand._home.path().join(format!("burst-{rep}.jsonl"));
        let book = open_book(&ledger);
        kept(&book).asked.extend(asked_already.iter().cloned());
        let seated = [(inbox.run(), TERM)];
        let began = Instant::now();
        ask_about(
            &stand.window,
            &stand.window.wire,
            &book,
            &ledger,
            &seated,
            DAY_START_MS + SWEEP_AFTER_MS,
        );
        let on_the_beat = micros(began);
        let began = Instant::now();
        stand.window.run_held();
        let off_the_beat = micros(began);
        beat.push(on_the_beat);
        job.push(off_the_beat);
        triaged.push(on_the_beat + off_the_beat);
        let written = rows(&ledger);
        assert_eq!(written.len(), fresh, "every fresh letter has a row");
        requests = written
            .iter()
            .map(|row| row["requests"].as_u64().unwrap_or(0))
            .sum::<u64>();
    }
    beat.sort_unstable();
    job.sort_unstable();
    triaged.sort_unstable();
    json!({
        "label": "burst",
        "road": std::env::var(ROAD_ENV).unwrap_or_default(),
        "profile": std::env::var(PROFILE_ENV).unwrap_or_default(),
        "messages": messages,
        "attempts": attempts,
        "fresh": fresh,
        "reps": reps,
        "requests": requests,
        "beatMicros": { "p50": percentile(&beat, 50), "max": beat.last() },
        "jobMicros": { "p50": percentile(&job, 50), "max": job.last() },
        "timeToTriagedMicros": { "p50": percentile(&triaged, 50), "max": triaged.last() },
    })
}

/// A burst of fresh letters on a long ledger, asked of a stand-in model
/// (`MAIL_MEASURE_ROUND_TRIP_MS`: none by default, the CPU the road itself
/// costs; a round trip, the wait it adds).
///
/// **Not a check.** It prints one JSON line. Run it under the build line on
/// both roads and both profiles (`taskpolicy -b` is the low-spec one).
#[test]
#[ignore = "a measurement, printed; not a check"]
fn measure_a_burst_on_a_long_ledger() {
    let endpoint = stand_in(env_number(ROUND_TRIP_ENV, 0));
    let stand = stand_on(&endpoint, "shadow", None, "test-key");
    let reps = env_number(BURST_REPS_ENV, BURST_REPEATS).max(1);
    publish(&burst(&stand, HISTORY_LETTERS, BURST_LETTERS, reps));
}

/// A stand on the real endpoint, under a zo home of its own: the person's day
/// count and ledgers are untouched. The key comes from the environment and not
/// from the keychain: inside a test build `accounts::security_command` is a
/// fixture holding a map. The job lifts it out of the real keychain into this
/// one command's environment, where no file and no log ever sees it.
fn the_real_wire() -> Stand {
    let key = std::env::var(zerocode_harness::TYPESAFE_API_KEY_ENV).unwrap_or_else(|_| {
        panic!(
            "{} carries this machine's TypeSafe key",
            zerocode_harness::TYPESAFE_API_KEY_ENV
        )
    });
    stand_at(crate::systemone::SYSTEMONE_BASE_URL, "shadow", None, &key)
}

/// A burst of fresh letters asked of the real model: the wait a coordinator
/// handed a burst of parallel results actually has before they are triaged.
///
/// **Not a check.** It crosses a real socket and spends a request per letter
/// on the per-letter road and a few on the batch road; it prints one JSON
/// line.
#[test]
#[ignore = "a measurement against api.typesafe.ai, printed; not a check"]
fn measure_a_burst_on_the_real_wire() {
    let stand = the_real_wire();
    publish(&burst(&stand, 0, BURST_LETTERS, 1));
}

/// The sizes of request the experiment asks the day's letters in: one a
/// request, and up to the widest a request could carry.
const SIZES: [usize; 6] = [1, 4, 8, 16, 32, 48];

/// How many requests of the experiment leave at once: a wave. Few, so the
/// experiment's own load is not what slows the answers.
const WAVE: usize = 4;

/// The widest request the experiment asks: the table's cap read wide enough
/// for it, so the door does not cut a request the experiment means to ask whole.
const WIDEST: usize = 48;

/// The seat's row with a cap of the experiment's own on the list of letters:
/// the door cuts a list to the table's cap, and the experiment asks wider.
const WIDE: JevUse = JevUse {
    sends: &[
        Sent {
            at: "/state/letters",
            cap: Cap::Items(WIDEST),
        },
        Sent {
            at: "/state/letters/*/kind",
            cap: Cap::Uncut,
        },
        Sent {
            at: "/state/letters/*/from",
            cap: Cap::Uncut,
        },
        Sent {
            at: "/state/letters/*/worker",
            cap: Cap::Uncut,
        },
        Sent {
            at: "/state/letters/*/task",
            cap: Cap::Uncut,
        },
        Sent {
            at: "/state/letters/*/taskStatus",
            cap: Cap::Uncut,
        },
        Sent {
            at: "/state/letters/*/priority",
            cap: Cap::Uncut,
        },
    ],
    ..MAIL_TRIAGE
};

/// The seat's questions with a cap of the case's own on how many letters a
/// request carries.
struct Capped {
    inside: MailTriage,
    cap: usize,
}

impl Judgment for Capped {
    type Verdict = MailRead;
    type Refusal = MailRefusal;

    fn cap(&self) -> usize {
        self.cap
    }

    fn items_key(&self) -> &'static str {
        self.inside.items_key()
    }

    fn shared(&self) -> Map<String, Value> {
        self.inside.shared()
    }

    fn questions(&self, at: usize) -> Vec<(&'static str, Value)> {
        self.inside.questions(at)
    }

    fn read(&self, answers: &Answers<'_>) -> Result<MailRead, MailRefusal> {
        self.inside.read(answers)
    }
}

/// How the answers of a batch road at each size agree with what a synthetic
/// coordinator did next, on the real model: the day's letters, with the
/// facts each entry carried when it was asked, are asked again in requests of
/// every size in [`SIZES`] — the same letters, the same words — and each
/// answer is marked against the labels of every seed, beside the kind rule on
/// the same letters. What it says is where agreement starts to give way to
/// size, and so what the cap should be.
///
/// **Not a check.** It crosses a real socket and spends one request a batch
/// of each size — 258 for the day — so it is `#[ignore]`d and run under the
/// build line with the key in this one command's environment, asking under a
/// zo home of its own.
#[test]
#[ignore = "a measurement against api.typesafe.ai, printed; not a check"]
fn measure_agreement_by_batch_size_on_the_real_wire() {
    // The day on a stand-in: what each letter's entry said when it was asked,
    // and what the coordinator was in at the time, in the letters' order.
    let model = stand_in(0);
    let stand = stand_on(&model, "shadow", None, "test-key");
    let day = the_day();
    let letters: usize = day.iter().map(Vec::len).sum();
    let mut inbox = Inbox::new(letters);
    run_the_day(
        &stand.window,
        &stand.book,
        &stand.ledger,
        &mut inbox,
        &day,
        &|| model.count(),
    );
    let mut asked_with: Vec<(Value, Value)> = Vec::new();
    for request in model.asked() {
        let body = body_of(&request);
        let coordinator = body["state"]["coordinator"].clone();
        for entry in body["state"]["letters"].as_array().into_iter().flatten() {
            asked_with.push((entry.clone(), coordinator.clone()));
        }
    }
    assert_eq!(asked_with.len(), letters, "an entry for every letter");

    // What the coordinator did next, by letter, under every seed.
    let ids: Vec<String> = inbox.letters.iter().map(|one| one.id.clone()).collect();
    let truths: Vec<HashMap<String, String>> = labelled(&stand.book, &inbox)
        .iter()
        .map(|(_, labels)| {
            labels
                .iter()
                .filter_map(|label| {
                    Some((
                        label[LABEL.canonical].as_str()?.to_string(),
                        label["truth"].as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .collect();

    let real = the_real_wire();
    let mut sizes = Vec::new();
    for size in SIZES {
        let groups: Vec<&[(Value, Value)]> = asked_with.chunks(size).collect();
        let judged: Vec<(Capped, Vec<usize>)> = groups
            .iter()
            .enumerate()
            .map(|(n, group)| {
                let coordinator = &group[0].1;
                let situation = Situation {
                    coordinator_busy: coordinator["busy"].as_bool(),
                    open_questions: coordinator["openQuestions"]
                        .as_u64()
                        .and_then(|n| usize::try_from(n).ok())
                        .unwrap_or(0),
                };
                let first = n * size;
                (
                    Capped {
                        inside: MailTriage::new(situation),
                        cap: size,
                    },
                    (first..first + group.len()).collect(),
                )
            })
            .collect();
        let (mut compared, mut agreed, mut rule, mut refused, mut bytes) = (0, 0, 0, 0, 0);
        let mut waves: Vec<u64> = Vec::new();
        for (these, judging) in groups.chunks(WAVE).zip(judged.chunks(WAVE)) {
            let built: Vec<Vec<Request>> = these
                .iter()
                .zip(judging)
                .map(|(group, (judgment, _))| {
                    zerocode_core::jev::batch::requests(
                        judgment,
                        group.iter().map(|(entry, _)| entry.clone()).collect(),
                    )
                })
                .collect();
            let asks: Vec<(Option<&Path>, Value)> = built
                .iter()
                .map(|made| {
                    (
                        Some(real.window.checkout.as_path()),
                        request_body(&made[0].state, &made[0].questions),
                    )
                })
                .collect();
            let began = Instant::now();
            let answered = real
                .window
                .wire
                .ask_together(&WIDE, asks, MAIL_TRIAGE_DEADLINE);
            waves.push(millis(began.elapsed()));
            for ((made, (judgment, at)), asked) in built.iter().zip(judging).zip(&answered) {
                bytes += asked.request_bytes;
                for (n, reading) in at.iter().zip(read_reply(&judgment.inside, &made[0], asked)) {
                    match reading {
                        Ok(read) => {
                            let kind = asked_with[*n].0["kind"]
                                .as_str()
                                .and_then(|kind| {
                                    serde_json::from_value::<MessageKind>(json!(kind)).ok()
                                })
                                .map(|kind| kind_rule(kind).word());
                            for truth in &truths {
                                let Some(wanted) = truth.get(&ids[*n]) else {
                                    continue;
                                };
                                compared += 1;
                                agreed += usize::from(read.triage.word() == wanted);
                                rule += usize::from(kind == Some(wanted.as_str()));
                            }
                        }
                        Err(_) => refused += 1,
                    }
                }
            }
        }
        waves.sort_unstable();
        sizes.push(json!({
            "size": size,
            "requests": groups.len(),
            "comparedPooledOverSeeds": compared,
            "agreed": agreed,
            "rule": rule,
            "refused": refused,
            "avgRequestBytes": bytes / groups.len().max(1),
            "waveMs": { "p50": percentile(&waves, 50), "max": waves.last() },
        }));
    }
    publish(&json!({ "label": "by-size", "letters": letters, "seeds": SEEDS, "sizes": sizes }));
}

/// What a request's reply says about each of its letters, read through the
/// seat's own reader: a reply with no answers in it is every letter's refusal.
fn read_reply(
    judgment: &MailTriage,
    request: &Request,
    asked: &Asked,
) -> Vec<Result<MailRead, MailRefusal>> {
    let answers = asked
        .answer
        .as_ref()
        .ok()
        .and_then(|body| serde_json::from_str::<Value>(body).ok())
        .and_then(|mut parsed| parsed.get_mut("answers").map(Value::take))
        .unwrap_or(Value::Null);
    request.read(judgment, &answers)
}

/// The synthetic day asked of the real model, and graded by what a synthetic
/// coordinator did next under each seed: the answers' agreement with the
/// labels, beside the kind rule's on the same letters, and the verdict of every
/// letter on a line of its own for comparing two roads' answers.
///
/// **Not a check.** It crosses a real socket and spends a request per batch
/// (a request per letter on the per-letter road), so it is `#[ignore]`d and
/// run under the build line, with the key lifted out of the keychain into this
/// one command's environment. It asks under a zo home of its own: the
/// person's day count and ledgers are untouched.
#[test]
#[ignore = "a measurement against api.typesafe.ai, printed; not a check"]
fn measure_the_synthetic_day_on_the_real_wire() {
    let stand = the_real_wire();
    let day = the_day();
    let letters: usize = day.iter().map(Vec::len).sum();
    assert!(
        letters <= MAX_REAL_REQUESTS,
        "a request per letter must stay under {MAX_REAL_REQUESTS}"
    );
    let mut inbox = Inbox::new(letters);
    let sweeps = run_the_day(
        &stand.window,
        &stand.book,
        &stand.ledger,
        &mut inbox,
        &day,
        &|| 0,
    );
    let written = rows(&stand.ledger);
    let account = account(&written, None, &sweeps);
    let by_seed = agreements(&labelled(&stand.book, &inbox));
    publish(&report("real-wire", &account, &sweeps, Some(&by_seed)));
    publish(&json!({
        "label": "verdicts",
        "road": std::env::var(ROAD_ENV).unwrap_or_default(),
        "profile": std::env::var(PROFILE_ENV).unwrap_or_default(),
        "verdicts": verdicts(&written),
    }));
}
