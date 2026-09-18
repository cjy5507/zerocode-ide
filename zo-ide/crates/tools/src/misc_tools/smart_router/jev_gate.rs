//! zo's side of the Jev door (`zerocode_core::jev::door`): what the door is
//! told — a key, the person's own settings, the workspace the words come
//! from, the day's count — and the one place a System One request leaves zo.
//!
//! The door itself is shared with the window, which asks System One about a
//! stopped browser walk down a wire of its own. This file reads what the door
//! needs from zo's side of the machine and sends what it clears: the routing
//! judgment in both modes, the recall judgment and a key check all pass
//! [`JevDoor::pass`] or [`JevDoor::pass_key_check`] and leave through
//! [`send`], and a source contract holds that nothing else in the workspace
//! calls the wire.
//!
//! Consent is read from zo's config home — the settings file the window's
//! settings pane writes — and never from a project's `.zo/settings.json`,
//! which arrives with a clone. One migration runs here: a workspace whose
//! ledgers hold rows written before the door, and none since, is one where a
//! person had a judgment turned on, so it is consented once, in that file,
//! under zo's settings lock. A row the door writes carries its count of
//! withheld lines, so a consent the person later takes back is not written
//! again.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use api::{SystemOneCall, SystemOneClient, SystemOneRequest};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use zerocode_core::jev::door::{self, Cleared, JevSettings, Refused};
use zerocode_core::jev::{count, hedge, JevUse, RECALL, ROUTING, SMART_SETTINGS_KEY};

use super::shadow_ledger::{last_shadow_line, last_shadow_lines, shadow_ledger_dir, shadow_ledger_path};

/// The person's settings file under zo's config home.
const SETTINGS_FILE: &str = "settings.json";

/// Rows of a use's own ledger the hedge rule reads its sample of past
/// latencies from — the field's "window", counted in rows rather than in time
/// because this door answers tens of times a day and an hour of it can hold
/// nothing at all.
///
/// It has to hold [`hedge::MIN_SAMPLES`] single-request answers for the
/// ledger where those are rarest, and a tail of rows is lumpy: of the recall
/// ledger's 955 rows here on 2026-09-18, 816 were answered from the memo and
/// only 72 (7.5%) were one request's own latency. A hundred rows of that tail
/// held six samples and 128 held ten, either side of the eight the rule needs;
/// 256 held 20, and the routing ledger's whole 22 besides.
///
/// The cost is bounded the same way, and measured rather than assumed: 256
/// rows of the wider ledger is 355 KiB, and reading the tail of that real
/// 1.38 MB ledger and naming a delay from it took 495 us (least of 20, warm)
/// — 0.03% of the 1,500 ms wall it is spent to land inside.
const HEDGE_SAMPLE_ROWS: usize = 256;

/// The uses whose ledgers live under a zo project's state: the ones whose rows
/// say a person had a judgment turned on in a workspace.
const ZO_USES: [JevUse; 2] = [ROUTING, RECALL];

/// The door as it stands for one batch of requests.
#[derive(Debug, Clone)]
pub struct JevDoor {
    settings: JevSettings,
    workspace: Option<String>,
    requests: PathBuf,
    /// Where this project's shadow ledgers are — the past latencies a hedge's
    /// delay is read from. `None` for a door that stands in no project, which
    /// has no past to read and so plans no hedge.
    ledgers: Option<PathBuf>,
}

impl JevDoor {
    /// The door for words from `cwd`: the person's own settings, `cwd` spelled
    /// as the filesystem spells it, and today's count under zo's config home.
    /// A workspace whose ledgers predate the door is consented here, once.
    #[must_use]
    pub fn open(cwd: &Path) -> Self {
        let home = runtime::default_config_home();
        let settings_path = home.join(SETTINGS_FILE);
        let mut settings = read_settings(&settings_path);
        let workspace = door::resolved_path(cwd);
        if settings.enabled
            && !settings.consents(&workspace)
            && ledgers_predate_the_door(cwd)
            && consent(&settings_path, &workspace).is_ok()
        {
            settings.workspaces.push(workspace.clone());
        }
        Self {
            settings,
            workspace: Some(workspace),
            requests: todays_requests(&home),
            ledgers: Some(shadow_ledger_dir(cwd)),
        }
    }

    /// The door for a key check, which carries no workspace's words.
    #[must_use]
    pub fn for_key_check() -> Self {
        let home = runtime::default_config_home();
        Self {
            settings: read_settings(&home.join(SETTINGS_FILE)),
            workspace: None,
            requests: todays_requests(&home),
            ledgers: None,
        }
    }

    /// A door told exactly these facts, for a test that must not read the
    /// machine's settings.
    #[cfg(test)]
    pub(super) fn at(settings: JevSettings, workspace: &Path, config_home: &Path) -> Self {
        Self {
            settings,
            workspace: Some(door::resolved_path(workspace)),
            requests: count::requests_path(config_home, "2026-09-17"),
            ledgers: Some(config_home.to_path_buf()),
        }
    }

    /// Ask the door about one request of `row`'s whose body is `body`; cleared,
    /// the request is counted in the day.
    ///
    /// # Errors
    /// The door's refusal.
    pub fn pass(&self, row: &JevUse, key: bool, body: Value) -> Result<Cleared, Refused> {
        door::pass(
            |asking| door::may_send(row, asking, body),
            key,
            &self.settings,
            self.workspace.as_deref(),
            &self.requests,
        )
    }

    /// Ask the door about a key check; cleared, it is counted in the day.
    ///
    /// # Errors
    /// The door's refusal.
    pub fn pass_key_check(&self, key: bool, body: &Value) -> Result<Cleared, Refused> {
        door::pass(|asking| door::may_check_key(asking, body), key, &self.settings, None, &self.requests)
    }

    /// When a second request of `row`'s would leave, against the `wall` an
    /// answer of it is used inside — `None` for a judgment to ask once.
    ///
    /// The rule is [`hedge::plan`]'s and lives in core, measured rather than
    /// chosen. What lives here is the sample it reads and the money it costs.
    ///
    /// The sample is this use's own ledger and no other. The two are not one
    /// distribution: measured here 2026-09-18, recall's 72 single-request
    /// answers ran to a p50 of 301 ms and a p90 of 789 ms where routing's 22
    /// ran to 636 ms and 4,259 ms, so a delay read from the pair of them
    /// would be a delay neither use ever waited. Of that ledger's tail
    /// ([`HEDGE_SAMPLE_ROWS`]) only the rows that are one request's own
    /// latency are a sample of the wire ([`Timed::sample`]).
    ///
    /// `None` too when the day's budget could not carry a second request
    /// behind the first. That is the plan declining to spend; the place
    /// itself is taken by [`Self::count_a_hedge`] at the moment the hedge may
    /// leave, and that count is what binds.
    #[must_use]
    pub fn hedge_for(&self, row: &JevUse, wall: Duration) -> Option<Duration> {
        let ledgers = self.ledgers.as_deref()?;
        // Room for this judgment's own request and a second one behind it.
        if !door::within_budget(&self.settings, count::sent(&self.requests).saturating_add(1)) {
            return None;
        }
        let samples: Vec<u64> = last_shadow_lines(&ledgers.join(row.ledger), HEDGE_SAMPLE_ROWS)
            .iter()
            .filter_map(|line| serde_json::from_str::<Timed>(line).ok())
            .filter_map(Timed::sample)
            .collect();
        hedge::plan(&samples, wall).map(|plan| plan.delay)
    }

    /// Take the day's place for a second request, immediately before it may
    /// leave. A hedge this refuses must not be sent.
    ///
    /// # Errors
    /// [`Refused::Budget`] when the day has no place left for it.
    pub fn count_a_hedge(&self) -> Result<(), Refused> {
        door::count_a_hedge(&self.settings, &self.requests)
    }

    /// The delay a second request of `row`'s leaves at, with the day's place
    /// for it already taken — what a caller passes to [`send`].
    ///
    /// `None` when `waited` is false. A hedge buys exactly one thing: an
    /// answer inside a wall. Where the product is not waiting on the answer —
    /// the record-only road of either use, which is recorded beside what the
    /// turn did anyway — there is no wall to land inside and a second copy
    /// would be the person's money for nothing.
    ///
    /// `None` too when there is no plan, or when the day cannot carry the
    /// second request. The order is the one that matters: counted here,
    /// before it can leave, so a budget never learns of the spending too late
    /// to refuse it.
    #[must_use]
    pub fn hedge_now(&self, row: &JevUse, wall: Duration, waited: bool) -> Option<Duration> {
        waited
            .then(|| self.hedge_for(row, wall))
            .flatten()
            .filter(|_| self.count_a_hedge().is_ok())
    }
}

/// What a hedge's plan reads of a ledger row, in whichever of the two
/// ledgers wrote it.
///
/// A sample has to be one request's own latency, and `elapsedMs` is that on
/// fewer rows than it looks: on a timeout row it is the wall (this machine's
/// four are 8,001-8,002 ms against an 8,000 ms one), on a row the memo
/// answered it is zero, on a retried row it is an attempt plus its backoffs,
/// and on a hedged row it is the faster of two copies — which, left in, would
/// walk the delay down a sample of its own making.
///
/// The routing ledger spells that field in camel case and the recall ledger
/// in snake case, so both are read here. Silently reading one of them as no
/// rows at all is exactly the mistake that would turn a hedge off for a whole
/// use without saying so.
#[derive(Deserialize)]
struct Timed {
    outcome: String,
    #[serde(alias = "elapsedMs")]
    elapsed_ms: u64,
    #[serde(default)]
    cached: bool,
    #[serde(default)]
    retries: u32,
    #[serde(default, rename = "hedgeFired")]
    hedge_fired: bool,
}

impl Timed {
    /// The latency this row is a sample of, or `None` when it is not one.
    fn sample(self) -> Option<u64> {
        let one_request = !self.cached && self.retries == 0 && !self.hedge_fired;
        (self.outcome == door::ANSWERED_OUTCOME && one_request).then_some(self.elapsed_ms)
    }
}

/// A latency or a planned delay as the whole milliseconds a ledger column
/// holds — one reading of it, so no two ledgers round a call differently.
#[must_use]
pub fn millis(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

/// A typed request as the body the door reads; `None` for one that does not
/// serialize, which is sent nowhere.
#[must_use]
pub fn body_of<S: Serialize + ?Sized>(request: &SystemOneRequest<'_, S>) -> Option<Value> {
    serde_json::to_value(request).ok()
}

/// Send what the door cleared — the one place a System One request leaves zo.
///
/// `hedge` is what [`JevDoor::hedge_for`] planned and
/// [`JevDoor::count_a_hedge`] paid for: a second copy of these same bytes,
/// that long after the first, when the first has not answered.
pub async fn send(
    client: &SystemOneClient,
    cleared: Cleared,
    deadline: Duration,
    hedge: Option<Duration>,
) -> SystemOneCall {
    client.decide_body(cleared.into_bytes(), deadline, hedge).await
}

/// The door's settings in the person's settings file, each root spelled as the
/// filesystem spells it. A file that cannot be read or parsed consents to
/// nothing.
fn read_settings(path: &Path) -> JevSettings {
    let root = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or(Value::Null);
    JevSettings::from_root(&root).resolved()
}

/// Today's count file under `home`, the day read on this machine's clock.
fn todays_requests(home: &Path) -> PathBuf {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX));
    let offset_minutes = i32::try_from(core_types::date::local_utc_offset_secs() / 60).unwrap_or(0);
    count::requests_path(home, &count::day_of(now_ms, offset_minutes))
}

/// Whether `cwd`'s ledgers hold rows and every last one was written before the
/// door: a workspace where a person had a judgment on, and the door has not
/// spoken since.
fn ledgers_predate_the_door(cwd: &Path) -> bool {
    let last: Vec<String> =
        ZO_USES.iter().filter_map(|row| last_shadow_line(&shadow_ledger_path(cwd, row.ledger))).collect();
    !last.is_empty() && last.iter().all(|row| door::predates_the_door(row))
}

/// Add `workspace` to `smart.jev.workspaces` in the settings file at `path`,
/// under zo's settings lock, every other key as it stood. A file whose `smart`,
/// `jev` or `workspaces` is of another shape is refused rather than rewritten.
fn consent(path: &Path, workspace: &str) -> io::Result<()> {
    let _lock = runtime::SettingsFileLock::acquire(path)?;
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let mut root: Map<String, Value> =
        if text.trim().is_empty() { Map::new() } else { serde_json::from_str(&text).map_err(io::Error::other)? };
    let shape = || io::Error::new(io::ErrorKind::InvalidData, "the settings file holds another shape here");
    let roots = root
        .entry(SMART_SETTINGS_KEY)
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(shape)?
        .entry(door::JEV_SETTINGS_KEY)
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(shape)?
        .entry(door::WORKSPACES_SETTING)
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(shape)?;
    if !roots.iter().any(|root| root.as_str() == Some(workspace)) {
        roots.push(Value::String(workspace.to_string()));
    }
    let mut rendered = serde_json::to_string_pretty(&root).map_err(io::Error::other)?;
    rendered.push('\n');
    runtime::replace_file_atomic(path, rendered.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No file in the workspace but this one calls the wire, and the wire
    /// takes nothing but bytes: every request zo makes of System One has been
    /// through the door.
    #[test]
    fn the_door_is_the_only_road_to_the_wire() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut sources = Vec::new();
        let mut pending = vec![crates];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir).expect("a source directory").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|name| name != "target") {
                        pending.push(path);
                    }
                } else if path.extension().is_some_and(|extension| extension == "rs") {
                    sources.push(path);
                }
            }
        }
        let gate = Path::new(file!()).file_name().expect("this file");
        let wire = Path::new("api").join("src").join("systemone.rs");
        let callers: Vec<String> = sources
            .iter()
            .filter(|path| !path.ends_with(&wire))
            .filter(|path| {
                std::fs::read_to_string(path).is_ok_and(|text| text.contains(".decide_body("))
            })
            .map(|path| path.display().to_string())
            .collect();
        assert_eq!(callers.len(), 1, "the wire is called from {callers:?}");
        assert!(Path::new(&callers[0]).ends_with(Path::new("smart_router").join(gate)), "{callers:?}");
        assert!(sources.len() > 100, "the scan found the workspace: {} files", sources.len());
    }

    /// Both of zo's ledgers keep the door's counts under the door's own keys —
    /// the keys a migration reads a row's age by — and zo's wire names a
    /// missing key with the door's word.
    /// A routing row, as `decision_shadow` writes one.
    fn routing_row(outcome: &str, elapsed_ms: u64) -> super::super::decision_shadow::DecisionShadowRow {
        super::super::decision_shadow::DecisionShadowRow {
            at: 1,
            attempt: None,
            task: "0000000000000001".to_string(),
            rubric_version: 1,
            model: None,
            outcome: outcome.to_string(),
            elapsed_ms,
            retries: 0,
            cached: false,
            input_tokens: None,
            route_use: super::super::decision_shadow::DecisionRouteUse::RecordOnly,
            requests: Some(0),
            redacted_lines: Some(0),
            hedge_delay_ms: None,
            hedge_fired: None,
            hedge_won: None,
            loser_ms: None,
            probe: super::super::decision_shadow::ProbeCell::Failed("timeout".to_string()),
            jev: None,
        }
    }

    /// A recall row, as `rerank_shadow` writes one.
    fn recall_row(outcome: &str, elapsed_ms: u64) -> super::super::rerank_shadow::RerankShadowRow {
        super::super::rerank_shadow::RerankShadowRow {
            at: 1,
            query: 1,
            notes: 1,
            rubric_version: 1,
            outcome: outcome.to_string(),
            candidates: 1,
            cached: false,
            elapsed_ms,
            retries: 0,
            model: None,
            input_tokens: None,
            requests: Some(0),
            redacted_lines: Some(0),
            hedge_delay_ms: None,
            hedge_fired: None,
            hedge_won: None,
            loser_ms: None,
            rejected: None,
            rejected_at: None,
            judged: None,
            applied: false,
        }
    }

    /// The latency the hedge rule's reader takes from a row, or `None` where
    /// it takes none.
    fn read(row: &impl Serialize) -> Option<u64> {
        serde_json::from_value::<Timed>(serde_json::to_value(row).expect("a row serializes"))
            .expect("a row the reader reads")
            .sample()
    }

    #[test]
    fn zos_ledgers_and_wire_speak_the_doors_words() {
        let budget = Refused::Budget.token();
        let routing = serde_json::to_value(routing_row(budget, 0)).expect("a routing row");
        let recall = serde_json::to_value(recall_row(budget, 0)).expect("a recall row");
        for row in [&routing, &recall] {
            assert_eq!(row[door::REQUESTS_KEY], 0, "{row}");
            assert_eq!(row[door::REDACTED_LINES_KEY], 0, "{row}");
            assert!(!door::predates_the_door(&row.to_string()));
        }
        assert_eq!(api::SystemOneFailure::NoKey.token(), Refused::NoKey.token());
    }

    /// One reader, two ledgers — and they do not spell a latency the same
    /// way: a routing row writes `elapsedMs` where a recall row writes
    /// `elapsed_ms`. A reader that knew only one of them would find no
    /// samples at all in the other and turn that use's hedge off without
    /// saying a word about it.
    #[test]
    fn the_hedges_sample_reads_a_latency_out_of_either_ledger() {
        let answered = door::ANSWERED_OUTCOME;
        assert_eq!(read(&routing_row(answered, 641)), Some(641), "the routing ledger's spelling");
        assert_eq!(read(&recall_row(answered, 641)), Some(641), "the recall ledger's spelling");
    }

    /// Only a row that is one request's own latency is a sample of the wire.
    /// A wall, a memo's nothing, an attempt plus its backoffs and the faster
    /// of two copies are none of them that — and the last of them, left in,
    /// would walk the delay down a distribution of the rule's own making.
    #[test]
    fn a_wall_a_memo_a_retry_and_a_hedge_are_not_samples() {
        let answered = door::ANSWERED_OUTCOME;
        assert_eq!(
            read(&routing_row(&api::SystemOneFailure::Timeout.ledger_token(), 8_001)),
            None,
            "a timeout's elapsed is the wall it hit, not a latency"
        );

        let mut memo = recall_row(answered, 0);
        memo.cached = true;
        assert_eq!(read(&memo), None, "a memo answered off no wire");

        let mut retried = routing_row(answered, 1_400);
        retried.retries = 1;
        assert_eq!(read(&retried), None, "an attempt and its backoffs are not one request");

        let mut hedged = recall_row(answered, 300);
        hedged.hedge_fired = Some(true);
        assert_eq!(read(&hedged), None, "the faster of two copies is not one request");
    }

    /// The whole reading, through the real door: this machine's own 22
    /// answered routing rows name the delay
    /// `zerocode_core::jev::hedge` documents, 864 ms against the 1,500 ms
    /// wall — and the recall use, whose ledger holds none of them, plans
    /// nothing from another use's rows.
    #[test]
    fn a_use_reads_its_delay_from_its_own_ledgers_tail() {
        /// Every answered routing judgment this machine's ledger held on
        /// 2026-09-18, in milliseconds.
        const LEDGER: [u64; 22] = [
            210, 218, 241, 250, 292, 311, 349, 473, 487, 616, 636, 641, 702, 1_020, 1_570, 1_953,
            2_257, 2_596, 3_140, 4_259, 4_847, 6_798,
        ];
        let home = tempfile::tempdir().expect("a config home");
        let ledger = home.path().join(ROUTING.ledger);
        for ms in LEDGER {
            super::super::shadow_ledger::append_shadow_row(
                &ledger,
                &routing_row(door::ANSWERED_OUTCOME, ms),
                u64::MAX,
            )
            .expect("a sample row");
        }
        let settings = JevSettings { enabled: true, workspaces: vec!["/work/zo".to_string()], daily_requests: None };
        let door = JevDoor::at(settings, Path::new("/work/zo"), home.path());
        let wall = Duration::from_millis(1_500);

        assert_eq!(
            door.hedge_for(&ROUTING, wall),
            Some(Duration::from_millis(864)),
            "the rule's own number, read back through the ledger"
        );
        assert_eq!(door.hedge_for(&RECALL, wall), None, "one use's rows are not another's sample");
    }

    /// The hedge's four columns are one spelling in both ledgers, and a row
    /// written before them reads back as the row it is.
    #[test]
    fn the_hedges_columns_are_spelled_one_way_in_both_ledgers() {
        let answered = door::ANSWERED_OUTCOME;
        let mut routing = routing_row(answered, 864);
        let mut recall = recall_row(answered, 864);
        routing.hedge_delay_ms = Some(864);
        routing.hedge_fired = Some(true);
        routing.hedge_won = Some(true);
        routing.loser_ms = Some(4_259);
        recall.hedge_delay_ms = Some(864);
        recall.hedge_fired = Some(true);
        recall.hedge_won = Some(true);
        recall.loser_ms = Some(4_259);
        let written = [
            serde_json::to_value(&routing).expect("a routing row"),
            serde_json::to_value(&recall).expect("a recall row"),
        ];
        for row in &written {
            assert_eq!(row["hedgeDelayMs"], 864, "{row}");
            assert_eq!(row["hedgeFired"], true, "{row}");
            assert_eq!(row["hedgeWon"], true, "{row}");
            assert_eq!(row["loserMs"], 4_259, "{row}");
        }

        let plain = serde_json::to_value(routing_row(answered, 641)).expect("a row with no hedge");
        for key in ["hedgeDelayMs", "hedgeFired", "hedgeWon", "loserMs"] {
            assert_eq!(plain.get(key), None, "{key} is absent where there was no hedge");
        }
        assert_eq!(
            serde_json::from_value::<super::super::decision_shadow::DecisionShadowRow>(plain)
                .expect("a row from before the hedge"),
            routing_row(answered, 641)
        );
    }

    /// A door standing in no project — a key check — has no past to read and
    /// so plans no second request.
    #[test]
    fn a_door_with_no_project_plans_no_hedge() {
        assert_eq!(JevDoor::for_key_check().hedge_for(&ROUTING, Duration::from_millis(1_500)), None);
    }

    #[test]
    fn a_consent_is_written_once_and_nothing_else_of_the_file_moves() {
        let home = tempfile::tempdir().expect("a config home");
        let path = home.path().join(SETTINGS_FILE);
        std::fs::write(&path, r#"{"model":"fable","smart":{"decisionShadow":"shadow","jev":{"dailyRequests":40}}}"#)
            .expect("settings");

        consent(&path, "/work/app").expect("consented");
        consent(&path, "/work/app").expect("consented again");

        let root: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert_eq!(root["smart"]["jev"]["workspaces"], serde_json::json!(["/work/app"]));
        assert_eq!(root["smart"]["jev"]["dailyRequests"], 40);
        assert_eq!(root["smart"]["decisionShadow"], "shadow");
        assert_eq!(root["model"], "fable");

        std::fs::write(&path, r#"{"smart":{"jev":{"workspaces":"/work/app"}}}"#).expect("a stranger's shape");
        assert!(consent(&path, "/work/app").is_err());
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            r#"{"smart":{"jev":{"workspaces":"/work/app"}}}"#,
            "a refused consent writes nothing"
        );
    }
}
