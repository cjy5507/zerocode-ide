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

use serde_json::Map;
use zerocode_core::jev::summary::{AGREED, BASELINE_AGREED, NOT_COMPARED};
use zerocode_core::jev::{MAIL_TRIAGE_BATCH_CAP, SMART_SETTINGS_KEY};
use zerocode_core::mail_triage::NO_NEED_AFTER_ACTS;
use zerocode_core::orchestration::{
    Draft, LEDGER_ITSELF, MessageKind, Priority, ServedAnswer, ServedRow, Text, worker_address,
};

use super::*;
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

/// A text this long is the product's words and not a letter's fact: the
/// longest value a letter's entry carries is 19 bytes (a kind), the shortest
/// of the words a question says is 31.
const SHARED_WORDS_MIN_BYTES: usize = 24;

/// The words the batch's state opens with — every request, whichever road
/// built it, says them: counting them counts the requests that carried the
/// rubric.
const RUBRIC_OPENING: &str = "A coordinator agent runs a team of worker agents";

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

/// A window with one coordinator pane: it hands out the wire and the
/// coordinator's checkout and nothing else, and runs a job where it is handed
/// it — the trait's own default — so a test sees what a sweep did the moment
/// it returns.
struct Window {
    wire: Wire,
    checkout: PathBuf,
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
        let quiet = Self::hands(&mut ledger, &run, 8, 6);
        let finishing = Self::hands(&mut ledger, &run, 14, finishing);
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

/// A day's sweeps: each batch arrives, and one sweep of the beat sees it.
/// Returns the letters in each sweep and how long the sweep took — the time
/// to a triaged batch.
fn run_the_day(
    window: &Window,
    book: &Arc<Mutex<MailBook>>,
    ledger: &Path,
    inbox: &mut Inbox,
    batches: &[Vec<MessageKind>],
) -> Vec<(usize, Duration)> {
    let mut took = Vec::new();
    for (n, kinds) in batches.iter().enumerate() {
        let at = DAY_START_MS + MINUTE_MS * i64::try_from(n).unwrap_or(0);
        inbox.arrive(kinds, at);
        let seated = [(inbox.run(), TERM)];
        let began = Instant::now();
        ask_about(
            window,
            &window.wire,
            book,
            ledger,
            &seated,
            at + SWEEP_AFTER_MS,
        );
        took.push((kinds.len(), began.elapsed()));
    }
    took
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

/// What a day came to: read off the rows the seat wrote and the requests the
/// endpoint heard.
struct Account {
    letters: usize,
    sweeps: usize,
    requests: u64,
    request_bytes: u64,
    input_tokens: u64,
    repeated_shared_bytes: usize,
    rubrics: usize,
    outcomes: BTreeMap<String, usize>,
}

fn account(rows: &[Value], endpoint: &Endpoint, sweeps: usize) -> Account {
    let sum = |key: &str| -> u64 { rows.iter().map(|row| row[key].as_u64().unwrap_or(0)).sum() };
    let heard = endpoint.asked();
    let bodies: Vec<Value> = heard.iter().map(|request| body_of(request)).collect();
    let mut outcomes: BTreeMap<String, usize> = BTreeMap::new();
    for row in rows {
        *outcomes
            .entry(row["outcome"].as_str().unwrap_or("none").to_string())
            .or_default() += 1;
    }
    Account {
        letters: rows.len(),
        sweeps,
        requests: sum("requests"),
        request_bytes: sum("requestBytes"),
        input_tokens: sum("inputTokens"),
        repeated_shared_bytes: repeated_shared_bytes(&bodies),
        rubrics: heard
            .iter()
            .map(|request| request.matches(RUBRIC_OPENING).count())
            .sum(),
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

    /// The act by which the coordinator handles letter `n`, filed at `at`: a
    /// question is answered in its thread, a finished task is reviewed, and
    /// any other letter is dealt with by a word to the worker it is about.
    fn handling(&mut self, n: usize, at: i64, filed: usize) -> ServedRow {
        let (id, kind, worker, task) = {
            let letter = &self.letters[n];
            (
                letter.id.clone(),
                letter.kind,
                letter.worker.clone(),
                letter.task.clone(),
            )
        };
        match kind {
            MessageKind::Question => {
                let reply = self
                    .ledger
                    .post(
                        &self.run,
                        Draft {
                            from: format!("run:{}", self.run),
                            to: worker_address(&worker),
                            kind: MessageKind::Question,
                            body: Text::from("yes"),
                            subject: Text::default(),
                            priority: Priority::Normal,
                            payload: Text::default(),
                            thread: Some(id),
                            task: None,
                            dispatch: None,
                        },
                        at,
                    )
                    .expect("an answer");
                Self::receipt("reply", &json!({ "messageId": reply }), at, filed)
            }
            MessageKind::WorkerDone => {
                Self::receipt("task-update", &json!({ "taskId": task }), at, filed)
            }
            _ => Self::receipt("send", &json!({ "workerId": worker }), at, filed),
        }
    }

    /// The ledger once the synthetic coordinator has dealt with the day: each
    /// batch taken in the order [`rank`] gives, each letter handled as often
    /// as [`handled_percent`] says, and enough acts at the end that a letter
    /// nothing named is given up. Every letter has been handed over.
    fn coordinated(mut self, dice: &mut Dice) -> Ledger {
        let mut receipts = Vec::new();
        let batches = self.batches.clone();
        for (b, batch) in batches.iter().enumerate() {
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
                receipts.push(self.handling(n, at, filed));
                at += ACT_GAP_MS;
            }
        }
        let mut at = DAY_START_MS + MINUTE_MS * i64::try_from(batches.len()).unwrap_or(0);
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
        let mut projected = self.ledger.export();
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
        window: Window {
            wire,
            checkout: work.path().to_path_buf(),
        },
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
    const HOLD_MS: u64 = 150;
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
    run_the_day(&stand.window, &stand.book, &stand.ledger, &mut inbox, &day);

    let written = rows(&stand.ledger);
    let held = inbox.letters_held();
    let account = account(&written, &endpoint, day.len());
    let heard = endpoint.asked().len();
    assert_eq!(
        heard, account.sweeps,
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
        account.rubrics, heard,
        "the rubric travelled once in each of them"
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
/// the seat's answer and the kind rule, each marked against the same truth.
#[test]
fn the_labels_of_a_batch_carry_both_marks_as_ever() {
    let endpoint = stand_in(0);
    let stand = stand_on(&endpoint, "shadow", None, "test-key");
    let day = the_day();
    let letters: usize = day.iter().map(Vec::len).sum();
    let mut inbox = Inbox::new(letters);
    run_the_day(&stand.window, &stand.book, &stand.ledger, &mut inbox, &day);
    let answered = rows(&stand.ledger).len();
    let ledger = inbox.coordinated(&mut Dice(7));
    let labels = label_waiting(
        &mut kept(&stand.book),
        &ledger,
        DAY_START_MS + MINUTE_MS * 200,
    );
    assert!(!labels.is_empty(), "{answered} answers, no label");
    let tally = agreement(&labels);
    assert!(tally.compared > 0, "{tally:?}");
    for label in labels
        .iter()
        .filter(|label| label.get(NOT_COMPARED.canonical).is_none())
    {
        assert!(label[AGREED.canonical].is_boolean(), "{label}");
        assert!(label[BASELINE_AGREED.canonical].is_boolean(), "{label}");
    }
}

/* ---- measurements --------------------------------------------------------- */

/// The profile a measurement ran under, as the job names it.
const PROFILE_ENV: &str = "MAIL_MEASURE_PROFILE";

/// Which road the measured build carries — `per-letter` before the batch road
/// and `batch` after it: a name the job gives, since both builds run this very
/// test.
const ROAD_ENV: &str = "MAIL_MEASURE_ROAD";

/// How long the stand-in holds each request before it answers, in
/// milliseconds: the model's measured round trip at p50 (280 ms, rounds 2 to
/// 10 of 2026-09-21, `docs/design/jev-seats-accuracy-wave-20260921.md` §2).
const ROUND_TRIP_MS: u64 = 280;

/// The most requests a real-wire measurement may spend.
const MAX_REAL_REQUESTS: usize = 400;

/// The value at `percent` of a sorted list, by the nearest rank.
fn percentile(sorted: &[u64], percent: usize) -> u64 {
    let Some(last) = sorted.len().checked_sub(1) else {
        return 0;
    };
    sorted[((last * percent + 50) / 100).min(last)]
}

/// One measured day, as a line the job collects: requests, bytes, the words
/// that left more than once, the time each batch took to be triaged, and what
/// the labels said.
fn report(
    label: &str,
    account: &Account,
    took: &[(usize, Duration)],
    labels: Option<&Agreement>,
) -> Value {
    let mut millis: Vec<u64> = took
        .iter()
        .map(|(_, took)| u64::try_from(took.as_millis()).unwrap_or(u64::MAX))
        .collect();
    millis.sort_unstable();
    let mut by_size: BTreeMap<usize, Vec<u64>> = BTreeMap::new();
    for (size, took) in took {
        by_size
            .entry(*size)
            .or_default()
            .push(u64::try_from(took.as_millis()).unwrap_or(u64::MAX));
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
    let agreement = labels.map(|tally| {
        json!({
            "compared": tally.compared,
            "answered": tally.answered,
            "rule": tally.rule,
            "notCompared": tally.not_compared,
        })
    });
    json!({
        "label": label,
        "road": std::env::var(ROAD_ENV).unwrap_or_default(),
        "profile": std::env::var(PROFILE_ENV).unwrap_or_default(),
        "letters": account.letters,
        "batches": account.sweeps,
        "requests": account.requests,
        "requestBytes": account.request_bytes,
        "inputTokens": account.input_tokens,
        "repeatedSharedBytes": account.repeated_shared_bytes,
        "rubricsSent": account.rubrics,
        "outcomes": account.outcomes,
        "timeToTriagedBatchMs": {
            "p50": percentile(&millis, 50),
            "p95": percentile(&millis, 95),
            "max": millis.last(),
            "total": millis.iter().sum::<u64>(),
        },
        "bySize": by_size,
        "agreement": agreement,
    })
}

/// The key of how many days the stand-in measurement asks, one after another
/// in one process: what a long run holds is read off the process's peak
/// resident set at one day and at many.
const DAYS_ENV: &str = "MAIL_MEASURE_DAYS";

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
    let hold_ms = std::env::var("MAIL_MEASURE_ROUND_TRIP_MS")
        .ok()
        .and_then(|said| said.parse().ok())
        .unwrap_or(ROUND_TRIP_MS);
    let days: usize = std::env::var(DAYS_ENV)
        .ok()
        .and_then(|said| said.parse().ok())
        .unwrap_or(1)
        .max(1);
    let day = the_day();
    let letters: usize = day.iter().map(Vec::len).sum();
    for _ in 0..days {
        let endpoint = stand_in(hold_ms);
        let stand = stand_on(&endpoint, "shadow", None, "test-key");
        let mut inbox = Inbox::new(letters);
        let took = run_the_day(&stand.window, &stand.book, &stand.ledger, &mut inbox, &day);
        let written = rows(&stand.ledger);
        let account = account(&written, &endpoint, day.len());
        if days == 1 {
            println!("{}", report("stand-in", &account, &took, None));
        }
    }
    if days > 1 {
        println!(
            "{}",
            json!({ "label": "stand-in", "days": days, "letters": letters })
        );
    }
}

/// The synthetic day asked of the real model, and graded by what a synthetic
/// coordinator did next: the answers' agreement with the labels, beside the
/// kind rule's on the same letters.
///
/// **Not a check.** It crosses a real socket and spends a request per batch
/// (a request per letter on the per-letter road), so it is `#[ignore]`d and
/// run under the build line, with the key lifted out of the keychain into this
/// one command's environment. It asks under a zo home of its own: the
/// person's day count and ledgers are untouched.
#[test]
#[ignore = "a measurement against api.typesafe.ai, printed; not a check"]
fn measure_the_synthetic_day_on_the_real_wire() {
    let key = std::env::var(zerocode_harness::TYPESAFE_API_KEY_ENV).unwrap_or_else(|_| {
        panic!(
            "{} carries this machine's TypeSafe key",
            zerocode_harness::TYPESAFE_API_KEY_ENV
        )
    });
    let home = tempfile::tempdir().expect("a zo home of this measurement's own");
    let work = tempfile::tempdir().expect("a checkout");
    let wire = Wire::at(
        crate::systemone::SYSTEMONE_BASE_URL,
        &key,
        Some(settings(&home, "shadow", work.path(), None)),
    );
    let ledger = home.path().join(MAIL_TRIAGE.ledger);
    let book = open_book(&ledger);
    let window = Window {
        wire,
        checkout: work.path().to_path_buf(),
    };
    let day = the_day();
    let letters: usize = day.iter().map(Vec::len).sum();
    assert!(
        letters <= MAX_REAL_REQUESTS,
        "a request per letter must stay under {MAX_REAL_REQUESTS}"
    );
    let mut inbox = Inbox::new(letters);
    let took = run_the_day(&window, &book, &ledger, &mut inbox, &day);
    let written = rows(&ledger);
    let sweeps = day.len();
    let account = Account {
        letters: written.len(),
        sweeps,
        requests: written
            .iter()
            .map(|row| row["requests"].as_u64().unwrap_or(0))
            .sum(),
        request_bytes: written
            .iter()
            .map(|row| row["requestBytes"].as_u64().unwrap_or(0))
            .sum(),
        input_tokens: written
            .iter()
            .map(|row| row["inputTokens"].as_u64().unwrap_or(0))
            .sum(),
        repeated_shared_bytes: 0,
        rubrics: 0,
        outcomes: written.iter().fold(BTreeMap::new(), |mut found, row| {
            *found
                .entry(row["outcome"].as_str().unwrap_or("none").to_string())
                .or_default() += 1;
            found
        }),
    };
    let ledger_after = inbox.coordinated(&mut Dice(7));
    let labels = label_waiting(
        &mut kept(&book),
        &ledger_after,
        DAY_START_MS + MINUTE_MS * 200,
    );
    println!(
        "{}",
        report("real-wire", &account, &took, Some(&agreement(&labels)))
    );
}
