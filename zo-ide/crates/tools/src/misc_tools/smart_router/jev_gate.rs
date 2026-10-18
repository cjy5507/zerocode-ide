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
use zerocode_core::jev::{count, hedge, JevMode, JevUse, RECALL, ROUTING, SMART_SETTINGS_KEY};
use zerocode_core::jev::summary;

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

/// Requests in a row, of one seat's series, that may go unanswered before a
/// seat that applies nothing rests (t-58193) — the series being the seat's
/// ledger under this provider's folder and its current rubric.
///
/// More than the three in a row that end an acting seat's rise
/// ([`zerocode_core::jev::promote::FALLBACKS_THAT_END_IT`]), so a bad minute
/// of the wire — a restart, a few timeouts — never rests a seat that answers
/// at all; few enough that a seat whose every request fails stops within
/// two minutes at the pace the recall asked on 2026-10-15, when 343 of its
/// requests were refused in 68 minutes.
pub const UNANSWERED_BEFORE_REST: usize = 8;

/// How long such a seat rests before one request tries the server again
/// (t-58193), from the newest of its unanswered requests.
///
/// zo cannot see a server get better — a person or an update restarts it —
/// so the rest is a time and not a signal. Two tries an hour cost a server
/// that never answers two requests where the recall sent about five a
/// minute, and those held the one-at-a-time server for 25.7 of 68 minutes
/// on 2026-10-15; a server restarted on the same address is found within
/// half an hour. A new address or model is a new ledger folder, where the
/// seat has no rest to wait out.
pub const REST: Duration = Duration::from_secs(30 * 60);

/// The uses whose ledgers live under a zo project's state: the ones whose rows
/// say a person had a judgment turned on in a workspace.
const ZO_USES: [JevUse; 2] = [ROUTING, RECALL];

/// Current authorization for locally reusing historical evidence. This reads
/// without migrating consent and does not discard old evidence on a model pin change.
pub(super) fn workspace_permitted_now(cwd: &Path) -> bool {
    let settings = read_settings(&runtime::default_config_home().join(SETTINGS_FILE));
    settings.enabled && settings.consents(&door::resolved_path(cwd))
}

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
    review_home: Option<PathBuf>,
    settings_file: Option<PathBuf>,
    review_origin: Option<(String, String)>,
}

impl JevDoor {
    /// Bind opted-in local review evidence to the caller's actual session/task.
    #[must_use]
    pub fn with_origin(mut self, namespace: &str, origin: &str) -> Self {
        self.review_origin = Some((namespace.to_string(), origin.to_string()));
        self
    }
    /// The door for words from `cwd`: the person's own settings, `cwd` spelled
    /// as the filesystem spells it, and today's count under zo's config home.
    /// A workspace whose ledgers predate the door is consented here, once.
    #[must_use]
    pub fn open(cwd: &Path) -> Self {
        let home = runtime::default_config_home();
        let settings_path = home.join(SETTINGS_FILE);
        let root = read_settings_root(&settings_path);
        let mut settings = JevSettings::from_root(&root).resolved();
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
            review_home: zerocode_core::jev::promote::label_drafts_wanted(&root).then_some(home),
            settings_file: Some(settings_path),
            review_origin: None,
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
            review_home: None,
            settings_file: Some(home.join(SETTINGS_FILE)),
            review_origin: None,
        }
    }

    /// The door for words from `cwd` as it stands, consenting nothing: what a
    /// reader of the door's state stands on, such as the summary that says
    /// why a seat rests (t-58193), which must not write the person's
    /// settings.
    #[must_use]
    pub fn reading(cwd: &Path) -> Self {
        let home = runtime::default_config_home();
        let settings_path = home.join(SETTINGS_FILE);
        Self {
            settings: JevSettings::from_root(&read_settings_root(&settings_path)).resolved(),
            workspace: Some(door::resolved_path(cwd)),
            requests: todays_requests(&home),
            ledgers: Some(shadow_ledger_dir(cwd)),
            review_home: None,
            settings_file: Some(settings_path),
            review_origin: None,
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
            review_home: None,
            settings_file: None,
            review_origin: None,
        }
    }

    /// The model every request this door clears names — the person's pin
    /// (`smart.jevModel`, t-6187), or the vendor's alias when they pinned
    /// none. The door writes it into the body it clears, whatever the seat
    /// built the body with; a seat reads it here only to key what it
    /// remembers by the model it actually asked.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.settings.model
    }

    /// Recheck consent and the model pin before a delayed answer changes live state.
    #[must_use]
    pub fn permits_application_now(&self) -> bool {
        let settings = self.settings_file.as_deref().map_or_else(|| self.settings.clone(), read_settings);
        settings.enabled && settings.model == self.settings.model
            && self.workspace.as_deref().is_some_and(|workspace| settings.consents(workspace))
    }

    /// Recheck the original pin and consent together with this seat's current
    /// merged mode and promotion standing. Callers also keep their original
    /// acting flag so a recording request cannot gain effects while it waits.
    #[must_use]
    pub fn permits_application_for(&self, cwd: &Path, seat: &JevUse) -> bool {
        self.permits_application_with(cwd, seat, || runtime::jev_seat_applies(cwd, seat))
    }

    /// Some seats keep their standing outside the per-project ledger.
    pub(super) fn permits_application_with(&self, cwd: &Path, seat: &JevUse, raised: impl FnOnce() -> bool) -> bool {
        self.permits_application_now()
            && super::settings::merged_settings_root_from(&runtime::ConfigLoader::default_for(cwd))
                .is_some_and(|root| {
                    let mode = seat.mode_in(&root);
                    mode.applies_with(mode.automatic() && raised())
                })
    }

    fn seat_asks_now(&self, seat: &JevUse) -> bool {
        self.settings_file.is_none() || self.workspace.as_deref().and_then(|cwd|
            super::settings::merged_settings_root_from(&runtime::ConfigLoader::default_for(Path::new(cwd))))
            .is_some_and(|root| seat.mode_in(&root).asks())
    }

    /// [`Self::model`] as the fingerprint an in-process memo keys on
    /// ([`model_key`]).
    #[must_use]
    pub fn model_key(&self) -> u64 {
        model_key(self.model())
    }

    /// Ask the door about one request of `row`'s whose body is `body`; cleared,
    /// the request is counted in the day.
    ///
    /// # Errors
    /// The door's refusal.
    pub fn pass(&self, row: &JevUse, key: bool, body: Value) -> Result<Cleared, Refused> {
        let settings = self.settings_file.as_deref().map_or_else(|| self.settings.clone(), read_settings);
        door::pass(
            |asking| self.may_send_now(row, &settings, asking, body),
            key,
            &settings,
            self.workspace.as_deref(),
            &self.requests,
        ).map(|cleared| self.with_review(row, cleared))
    }

    /// [`Self::pass`] for one of a person's own decisions (t-62170), whose
    /// requests count in two hours besides the day
    /// ([`door::pass_within_the_hour`]): the decision's own hour file
    /// `decision`, at most `decision_most`, and the file `every` decision
    /// shares, at most what the person's settings hold
    /// ([`door::custom_hourly_requests`]).
    ///
    /// # Errors
    /// The door's refusal, `hourly_budget` among them.
    pub fn pass_within_the_hour(
        &self,
        row: &JevUse,
        key: bool,
        body: Value,
        decision: &Path,
        decision_most: u64,
        every: &Path,
    ) -> Result<Cleared, Refused> {
        let root = self.settings_file.as_deref().map_or(Value::Null, read_settings_root);
        let settings = self.settings_file.as_ref().map_or_else(
            || self.settings.clone(),
            |_| JevSettings::from_root(&root).resolved(),
        );
        let hour = door::Hour { decision, decision_most, every, every_most: door::custom_hourly_requests(&root) };
        door::pass_within_the_hour(
            |asking| self.may_send_now(row, &settings, asking, body),
            key,
            &settings,
            self.workspace.as_deref(),
            &self.requests,
            hour,
        ).map(|cleared| self.with_review(row, cleared))
    }

    /// The door's own questions of a request, then this door's: the seat
    /// still asks, the model pin has not moved, and the seat is not resting.
    fn may_send_now(&self, row: &JevUse, settings: &JevSettings, asking: &door::Asking<'_>, body: Value) -> Result<Cleared, Refused> {
        let cleared = door::may_send(row, asking, body)?;
        if !self.seat_asks_now(row) { Err(Refused::Off) }
        else if settings.model != self.settings.model { Err(Refused::SettingsChanged) }
        else if self.resting(row) { Err(Refused::Resting) }
        else { Ok(cleared) }
    }

    fn with_review(&self, row: &JevUse, cleared: Cleared) -> Cleared {
        match (&self.review_home, &self.workspace) {
            (Some(home), Some(workspace)) => {
                let cleared = cleared.with_review(home, row, workspace);
                match &self.review_origin {
                    Some((namespace, origin)) => cleared.with_review_origin(namespace, origin),
                    None => cleared,
                }
            },
            _ => cleared,
        }
    }

    /// Clear `body` as a request of `row`'s — the key, the switch, the
    /// workspace's consent and the day's budget asked, and every text the
    /// row points at cleared as the door clears it (each line that may carry
    /// a credential withheld, then cut to its cap) — without sending or
    /// counting anything. For a seat whose words leave by another road
    /// before they reach the judge: the challenger's design request carries
    /// the head of the task to the person's own provider, and it is these
    /// bytes, cleared by the door as it stands the moment before they
    /// leave, that go — never the task as the spawn was handed it.
    ///
    /// # Errors
    /// The door's refusal, as [`Self::pass`] would answer it now.
    pub fn clear(&self, row: &JevUse, key: bool, body: Value) -> Result<Cleared, Refused> {
        let settings = self.settings_file.as_deref().map_or_else(|| self.settings.clone(), read_settings);
        let asking = door::Asking {
            key,
            settings: &settings,
            workspace: self.workspace.as_deref(),
            sent_today: count::sent(&self.requests),
        };
        let cleared = door::may_send(row, &asking, body)?;
        if !self.seat_asks_now(row) { Err(Refused::Off) }
        else if settings.model != self.settings.model { Err(Refused::SettingsChanged) }
        else if self.resting(row) { Err(Refused::Resting) }
        else { Ok(cleared) }
    }

    /// Ask the door about a key check; cleared, it is counted in the day.
    ///
    /// # Errors
    /// The door's refusal.
    pub fn pass_key_check(&self, key: bool, body: &Value) -> Result<Cleared, Refused> {
        let settings = self.settings_file.as_deref().map_or_else(|| self.settings.clone(), read_settings);
        door::pass(|asking| door::may_check_key(asking, body), key, &settings, None, &self.requests)
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
    /// Which use is hedged at all follows from that, and is the rule's
    /// answer rather than a setting: on the routing sample the delay
    /// [`hedge::MAX_EXTRA_LOAD`] can afford is itself past the wall, so
    /// routing plans nothing and asks once. It had fired five times before
    /// t-5874 and the second copy won none of them.
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
    /// A withdrawn switch/consent, a changed model pin, or [`Refused::Budget`]
    /// when the day has no place left for it.
    pub fn count_a_hedge(&self) -> Result<(), Refused> {
        let settings = self.settings_file.as_deref().map_or_else(|| self.settings.clone(), read_settings);
        if !settings.enabled { return Err(Refused::Off); }
        if !self.workspace.as_deref().is_some_and(|workspace| settings.consents(workspace)) {
            return Err(Refused::NotConsented);
        }
        if settings.model != self.settings.model { return Err(Refused::SettingsChanged); }
        door::count_a_hedge(&settings, &self.requests)
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

    /// Whether `row`'s seat rests now ([`Self::resting_now`]).
    fn resting(&self, row: &JevUse) -> bool {
        self.resting_now(row).is_some()
    }

    /// Why `row`'s seat rests now and until when (t-58193): what the door
    /// refuses it with `resting` for, and what the panel says of it
    /// (`super::jev_summary::with_rests`). Its server asked it to wait and
    /// the time has not come ([`Self::asked_to_wait`]); its provider cut the
    /// smallest request it can make, one item, and [`REST`] has not passed
    /// since ([`Self::room_of`]); or it applies nothing and its own ledger
    /// says it rests ([`Self::rest_of`]). `None` while it may ask.
    #[must_use]
    pub fn resting_now(&self, row: &JevUse) -> Option<Resting> {
        if let Some(until_ms) = self.asked_to_wait(row) {
            return Some(Resting { until_ms, cause: RestCause::Waiting, unanswered: None });
        }
        if self.room_of(row) == Some(0) {
            let cut = self.read_back(row, |lines| newest_sent_in(row, lines)).flatten();
            let cut = cut.unwrap_or_else(super::decision_shadow::now_ms);
            let until_ms = cut.saturating_add(rest_ms());
            return Some(Resting { until_ms, cause: RestCause::TooLong, unanswered: None });
        }
        let rest = self.rest_of(row).filter(|_| !self.applies_now(row))?;
        let cause = RestCause::of_rest(&rest.cause);
        Some(Resting { until_ms: rest.until_ms, cause, unanswered: Some(UNANSWERED_BEFORE_REST) })
    }

    /// Until when `row`'s seat waits because its server asked it to
    /// (t-58193), in Unix milliseconds: the `serverRetryAt` ([`ServerSaid`])
    /// of the newest request the seat sent to this provider, under any rubric
    /// — the wait is the server's — while that moment is still ahead. A `503`
    /// or `429` naming a `Retry-After` is the provider asking the seat to
    /// wait: the local server says it while it waits to restart after a
    /// stop, a remote provider when it is overloaded. Every seat waits, one
    /// that applies its judgment too, since the server would refuse it.
    ///
    /// The rows are read where the seat writes them: a guard's in its own
    /// ledger under the config home, one for every project on this machine
    /// ([`super::tool_guard::guard_ledger_of`]), every other seat's in this
    /// project's folder. Each is the provider's own folder
    /// (`runtime::judgment_evidence_dir`), so a wait one provider named
    /// holds no seat asking another.
    ///
    /// Read from the tail the hedge reads alone: no wait is longer than one
    /// [`REST`], and a tail without a request lets one go, whose answer
    /// names the wait again.
    fn asked_to_wait(&self, row: &JevUse) -> Option<i64> {
        let ledger = match super::tool_guard::guard_ledger_of(row) {
            Some(own) => own,
            None => self.ledgers.as_deref()?.join(row.ledger),
        };
        let tail = last_shadow_lines(&ledger, HEDGE_SAMPLE_ROWS);
        let newest = tail
            .iter()
            .rev()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(request_left)?;
        let until = newest.get(SERVER_RETRY_AT).and_then(Value::as_i64)?;
        (until > super::decision_shadow::now_ms()).then_some(until)
    }

    /// The most items — notes, skills — one request of `row`'s seat may
    /// carry to this provider (t-58193), as its own ledger in this provider's
    /// folder says ([`room_in`]): `None` while the provider has cut none of
    /// the seat's requests in its series, and again once the seat asks under
    /// none.
    ///
    /// No provider declares how much it reads — not the local server's model
    /// list, not a provider's settings — and zo cannot count a model's
    /// tokens, so the room is learned from the provider's own refusal
    /// ([`api::SYSTEMONE_CUT_REFUSAL`]), halved until a request fits and
    /// doubled again as answers come. A limit a provider declared would be
    /// read here, in place of the halving.
    #[must_use]
    pub fn room_of(&self, row: &JevUse) -> Option<usize> {
        let now = super::decision_shadow::now_ms();
        self.read_back(row, |lines| room_in(row, lines, now)).flatten()
    }

    /// Whether one [`REST`] has passed since `row`'s seat last sent this
    /// provider a request in its series, or it sent none (t-58193): when a
    /// seat that cannot carry fewer items than its room holds asks again.
    #[must_use]
    pub fn rested(&self, row: &JevUse) -> bool {
        let now = super::decision_shadow::now_ms();
        let newest = self.read_back(row, |lines| newest_sent_in(row, lines)).flatten();
        newest.is_none_or(|at| at.saturating_add(rest_ms()) <= now)
    }

    /// `row`'s rest as its own ledger in this provider's folder says it
    /// ([`rest_in`]).
    #[must_use]
    pub fn rest_of(&self, row: &JevUse) -> Option<Rest> {
        let now = super::decision_shadow::now_ms();
        self.read_back(row, |lines| rest_in(row, lines, now)).flatten()
    }

    /// What `read` makes of `row`'s own ledger in this provider's folder,
    /// newest line first: read from the tail the hedge reads, and from the
    /// whole ledger only when that tail ran out before `read` settled — a
    /// seat that has rested for hours wrote a row each time it was asked
    /// meanwhile. `None` for a door that stands in no project.
    fn read_back<T>(
        &self,
        row: &JevUse,
        read: impl Fn(&mut dyn Iterator<Item = &str>) -> Reading<T>,
    ) -> Option<T> {
        let ledger = self.ledgers.as_deref()?.join(row.ledger);
        let tail = last_shadow_lines(&ledger, HEDGE_SAMPLE_ROWS);
        match read(&mut tail.iter().rev().map(String::as_str)) {
            Reading::Settled(said) => Some(said),
            Reading::Short(_) => {
                let whole = std::fs::read_to_string(&ledger).unwrap_or_default();
                Some(read(&mut whole.lines().rev()).said())
            }
        }
    }

    /// Whether `row`'s seat applies its judgment now — the person's `on`, or
    /// an `auto` its own ledger raised — which no rest touches. A door that
    /// reads no settings file (a test's) knows no word, and applies nothing.
    fn applies_now(&self, row: &JevUse) -> bool {
        let (Some(_), Some(cwd), Some(ledgers)) =
            (&self.settings_file, self.workspace.as_deref(), self.ledgers.as_deref())
        else {
            return false;
        };
        let loader = runtime::ConfigLoader::default_for(Path::new(cwd));
        super::settings::merged_settings_root_from(&loader).is_some_and(|root| {
            let mode = row.mode_in(&root);
            let raised = || super::jev_summary::raised_in(row, &ledgers.join(row.ledger));
            mode.applies_with(mode.automatic() && raised())
        })
    }
}

/// Why a seat that applies nothing does not ask now (t-58193): its last
/// [`UNANSWERED_BEFORE_REST`] requests went unanswered, and [`REST`] has not
/// passed since the newest of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rest {
    /// When the rest ends, in Unix milliseconds.
    pub until_ms: i64,
    /// What those requests ended on most often, the newest first among
    /// equals: the provider's refusal where the server named one, else the
    /// row's outcome.
    pub cause: String,
}

/// Why a seat rests now and until when (t-58193), as the door reads it
/// (`JevDoor::resting_now`): the one field the summary hands the panel,
/// whose chip and sentence both read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resting {
    /// When the seat may ask again, in Unix milliseconds.
    pub until_ms: i64,
    pub cause: RestCause,
    /// For a rest of the seat's own, how many of its requests in a row went
    /// unanswered (`UNANSWERED_BEFORE_REST`); `None` for a wait its server
    /// asked for and for a room of none.
    pub unanswered: Option<usize>,
}

/// What a seat rests for (t-58193), by the word the summary writes, which
/// the panel words in every language (`ui/shell-jev.js`, `JEV_REST_CAUSES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestCause {
    /// Its server asked it to wait (`JevDoor::asked_to_wait`).
    Waiting,
    /// Its provider cut a request of one item (`JevDoor::room_of`).
    TooLong,
    /// Its last `UNANSWERED_BEFORE_REST` requests went unanswered, most
    /// often because the provider cut them for their length,
    Cut,
    /// because they timed out,
    Timeout,
    /// because they never reached the server,
    Unreachable,
    /// or because the server refused them another way.
    Refused,
}

impl RestCause {
    /// The word the summary writes for the cause.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::TooLong => "too_long",
            Self::Cut => "cut",
            Self::Timeout => "timeout",
            Self::Unreachable => "unreachable",
            Self::Refused => "refused",
        }
    }

    /// The cause of a rest of the seat's own, from what its unanswered
    /// requests ended on most often ([`Rest::cause`]).
    fn of_rest(cause: &str) -> Self {
        if cause == api::SYSTEMONE_CUT_REFUSAL {
            Self::Cut
        } else if cause == api::SystemOneFailure::Timeout.token() {
            Self::Timeout
        } else if cause == api::SystemOneFailure::Transport.token() {
            Self::Unreachable
        } else {
            Self::Refused
        }
    }
}

/// What a ledger's lines, read newest first, settled — or, when they ran out
/// first, what they said so far.
enum Reading<T> {
    Settled(T),
    Short(T),
}

impl<T> Reading<T> {
    /// What the lines said, settled or not.
    fn said(self) -> T {
        match self {
            Self::Settled(said) | Self::Short(said) => said,
        }
    }
}

/// A seat's rest as a ledger's `lines` say it, newest line first.
///
/// What is read is the requests the seat sent in its series
/// ([`sent_in_series`]); a request the door refused, a memo's answer, a
/// judge's transition and a mark are no request, and count neither way. Nor
/// does a request its provider cut under a room the seat had learned: the
/// fit rule answers that one by halving the room ([`room_in`]), which ends
/// the cuts within a few requests. Nor does an answer that asked for a wait:
/// its server said when to come back, and the door holds the seat until
/// then ([`JevDoor::asked_to_wait`]). An answer, or
/// [`UNANSWERED_BEFORE_REST`] unanswered requests, settles it.
fn rest_in<'a>(
    seat: &JevUse,
    lines: impl Iterator<Item = &'a str>,
    now_ms: i64,
) -> Reading<Option<Rest>> {
    let mut causes: Vec<String> = Vec::new();
    let mut newest = None;
    for row in lines.filter_map(|line| serde_json::from_str::<Value>(line).ok()) {
        if !sent_in_series(seat, &row)
            || (cut(&row) && row.get(ROOM).is_some())
            || row.get(SERVER_RETRY_AT).is_some()
        {
            continue;
        }
        if answered(&row) {
            return Reading::Settled(None);
        }
        newest = newest.or_else(|| summary::AT.read(&row).and_then(Value::as_i64));
        let refusal = row.get(SERVER_REFUSAL).or_else(|| summary::OUTCOME.read(&row));
        causes.push(refusal.and_then(Value::as_str).unwrap_or_default().to_string());
        if causes.len() == UNANSWERED_BEFORE_REST {
            let until_ms = newest.unwrap_or_default().saturating_add(rest_ms());
            let cause = most_common(causes);
            return Reading::Settled((until_ms > now_ms).then_some(Rest { until_ms, cause }));
        }
    }
    Reading::Short(None)
}

/// [`REST`] in the Unix milliseconds a row's `at` is written in.
fn rest_ms() -> i64 {
    i64::try_from(REST.as_millis()).unwrap_or(i64::MAX)
}

/// Whether `row` keeps a request its judgment answered.
fn answered(row: &Value) -> bool {
    summary::OUTCOME.read(row).and_then(Value::as_str) == Some(door::ANSWERED_OUTCOME)
}

/// When `seat` last sent a request in its series, as a ledger's `lines` say
/// it, newest line first.
fn newest_sent_in<'a>(seat: &JevUse, lines: impl Iterator<Item = &'a str>) -> Reading<Option<i64>> {
    let newest = lines
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|row| sent_in_series(seat, row));
    match newest {
        Some(row) => Reading::Settled(summary::AT.read(&row).and_then(Value::as_i64)),
        None => Reading::Short(None),
    }
}

/// Where [`ServerSaid`] keeps the provider's refusal, and the moment until
/// which the server asked for nothing more, on a row.
const SERVER_REFUSAL: &str = "serverRefusal";
const SERVER_RETRY_AT: &str = "serverRetryAt";

/// Where a seat's row keeps the room it was asked under, how many items its
/// request carried, and the model that answered it.
const ROOM: &str = "room";
const CANDIDATES: &str = "candidates";
const MODEL: &str = "model";

/// A seat's room as a ledger's `lines` say it, newest line first, read from
/// the requests the seat sent in its series ([`sent_in_series`]), back to
/// the one that set the room:
///
/// - a request its provider cut ([`cut`]) leaves room for half the items it
///   carried, and a cut that leaves room for none wakes as room for one once
///   [`REST`] has passed since it, so a short question can show that the
///   provider reads again;
/// - [`UNANSWERED_BEFORE_REST`] answers under one room double it for the
///   next request — the window that decides a rest decides a growth, so the
///   growth has no number of its own — and a cut of that request halves it
///   again;
/// - a request asked under no room says there is none, which is how a seat
///   whose room has grown to its whole request ends it.
fn room_in<'a>(
    seat: &JevUse,
    lines: impl Iterator<Item = &'a str>,
    now_ms: i64,
) -> Reading<Option<usize>> {
    let mut room = None;
    let mut answers = 0;
    for row in lines.filter_map(|line| serde_json::from_str::<Value>(line).ok()) {
        if !sent_in_series(seat, &row) {
            continue;
        }
        let count = |key: &str| {
            row.get(key).and_then(Value::as_u64).and_then(|count| usize::try_from(count).ok())
        };
        if let Some(carried) = count(CANDIDATES).filter(|_| cut(&row)) {
            if room.is_some() {
                return Reading::Settled(room);
            }
            let at = summary::AT.read(&row).and_then(Value::as_i64).unwrap_or_default();
            let woke = at.saturating_add(rest_ms()) <= now_ms;
            let halved = carried / 2;
            return Reading::Settled(Some(if halved == 0 && woke { 1 } else { halved }));
        }
        let held = count(ROOM);
        match room {
            None if held.is_none() => return Reading::Settled(None),
            None => room = held,
            Some(_) if held != room => return Reading::Settled(room),
            Some(_) => {}
        }
        if answered(&row) {
            answers += 1;
            if answers == UNANSWERED_BEFORE_REST {
                return Reading::Settled(room.map(|room| room.saturating_mul(2)));
            }
        }
    }
    Reading::Short(room)
}

/// Whether `row` keeps a request its provider cut for being longer than its
/// model reads ([`api::SYSTEMONE_CUT_REFUSAL`]). A row that also names the
/// model that answered had an earlier stage answered and a later one cut,
/// whose items it does not count; it says nothing of the room.
fn cut(row: &Value) -> bool {
    row.get(SERVER_REFUSAL).and_then(Value::as_str) == Some(api::SYSTEMONE_CUT_REFUSAL)
        && row.get(MODEL).is_none()
}

/// Whether `row` belongs to `seat`'s series: it names the rubric the seat
/// asks now — read as the judge reads a row's rubric, where a row naming
/// none is the first rubric's (`zerocode_core::jev::promote`). The provider
/// is the ledger's folder.
fn in_series(seat: &JevUse, row: &Value) -> bool {
    let rubric = summary::RUBRIC_VERSION.read(row).map_or(
        Some(zerocode_core::jev::questions::UNVERSIONED_RUBRIC),
        |spelled| spelled.as_u64().and_then(|version| u32::try_from(version).ok()),
    );
    rubric == Some(seat.rubric_version)
}

/// Whether `row` is a request its seat sent: one that asked something and
/// left at least once.
fn request_left(row: &Value) -> bool {
    summary::asked_something(row).is_some()
        && summary::REQUESTS.read(row).and_then(Value::as_u64).is_some_and(|sent| sent > 0)
}

/// Whether `row` is a request `seat` sent in its series ([`in_series`]).
fn sent_in_series(seat: &JevUse, row: &Value) -> bool {
    request_left(row) && in_series(seat, row)
}

/// The word `causes` hold most often, the first of them among equals.
fn most_common(causes: Vec<String>) -> String {
    let mut tally: Vec<(String, usize)> = Vec::new();
    for cause in causes {
        match tally.iter_mut().find(|(seen, _)| *seen == cause) {
            Some((_, count)) => *count += 1,
            None => tally.push((cause, 1)),
        }
    }
    tally
        .into_iter()
        .rev()
        .max_by_key(|(_, count)| *count)
        .map(|(cause, _)| cause)
        .unwrap_or_default()
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

/// A requested model as the fingerprint an in-process memo keys on, so a
/// judgment one pin answered is never recalled under another (t-6187). One
/// reading for every seat that keeps a memo.
#[must_use]
pub fn model_key(model: &str) -> u64 {
    super::probe_exec::task_fingerprint(model, "")
}

/// A latency or a planned delay as the whole milliseconds a ledger column
/// holds — one reading of it, so no two ledgers round a call differently.
#[must_use]
pub fn millis(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

/// What a managed server said beside a refusal (t-55423, t-58193): the code it
/// refused with, beside a `502` the provider's refusal of the answer —
/// `provider_context_truncated` for a request longer than the model reads —
/// and, beside a refusal that named a `Retry-After`, until when it asked for
/// nothing more. Every seat's row keeps them under the window's keys, through
/// [`ServerSaid::of`], so a refused judgment says why; all are absent on an
/// answer and wherever the server named nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
// The `server_*` prefix is the point: each field is the ledger key the window reads.
#[allow(clippy::struct_field_names)]
pub struct ServerSaid {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_refusal: Option<String>,
    /// Until when the server asked for nothing more from this seat, in Unix
    /// milliseconds (t-58193): the call's `retry_after` from when its answer
    /// was read, and never further off than one [`REST`], so no header keeps
    /// a seat silent longer than the rest does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_retry_at: Option<i64>,
}

impl ServerSaid {
    /// What `call`'s server said.
    #[must_use]
    pub fn of(call: &SystemOneCall) -> Self {
        let until = |after: Duration| {
            let after = i64::try_from(after.min(REST).as_millis()).unwrap_or(i64::MAX);
            super::decision_shadow::now_ms().saturating_add(after)
        };
        Self {
            server_error_code: call.server_error_code.map(str::to_string),
            server_refusal: call.server_refusal.map(str::to_string),
            server_retry_at: call.retry_after.map(until),
        }
    }

    /// The words written onto `row`, a row kept as JSON.
    pub fn stamp(&self, row: &mut Value) {
        let said = serde_json::to_value(self);
        if let (Ok(Value::Object(said)), Some(fields)) = (said, row.as_object_mut()) {
            fields.extend(said);
        }
    }

    /// `row` written with the words beside it, for a row whose type is not
    /// zo's own to add them to.
    #[must_use]
    pub fn beside<R: Serialize>(self, row: R) -> Beside<R> {
        Beside { row, said: self }
    }
}

/// A row and what its server said, written as one ([`ServerSaid::beside`]).
#[derive(Serialize)]
pub struct Beside<R> {
    #[serde(flatten)]
    row: R,
    #[serde(flatten)]
    said: ServerSaid,
}

/// Research replays make one paid attempt and keep their own explicit receipts.
/// The supplied request has already passed the same settings and consent door.
#[cfg(test)]
pub(super) async fn send_once(cleared: Cleared, client: &SystemOneClient, deadline: Duration) -> SystemOneCall {
    client.decide_body_once(cleared.into_bytes(), deadline).await
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
    let review = cleared.prepare_review();
    let call = client.decide_body(cleared.into_bytes(), deadline, hedge).await;
    if let (Some(review), Ok(response)) = (review, &call.outcome) {
        let response = serde_json::json!({"model":response.model,"answers":response.answers});
        let at = super::decision_shadow::now_ms();
        let _ = tokio::task::spawn_blocking(move || review.finish(&response, at)).await;
    }
    call
}

/// The door's settings in the person's settings file, each root spelled as the
/// filesystem spells it. A file that cannot be read or parsed consents to
/// nothing.
fn read_settings(path: &Path) -> JevSettings {
    JevSettings::from_root(&read_settings_root(path)).resolved()
}

fn read_settings_root(path: &Path) -> Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or(Value::Null)
}

/// The person's own mode word for a seat. Project settings never turn on a
/// judgment of the vault's private pages.
#[must_use]
pub fn mode_in_person_settings(row: &JevUse) -> JevMode {
    row.mode_in(&read_settings_root(&runtime::default_config_home().join(SETTINGS_FILE)))
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

    #[test]
    fn the_jev_tool_uses_the_existing_switches_for_calls_and_review_retention() {
        use super::super::{agent_tool, jev_mock};
        use zerocode_core::jev::{AGENT_TOOL, learning::store::Store};
        let endpoint = jev_mock::Mock::answering(|body| {
            let request: Value = serde_json::from_str(body).unwrap();
            let answers: Map<String, Value> = request["questions"].as_object().unwrap().keys().map(|name| (
                name.clone(), serde_json::json!({"type":"choice","choice":"yes",
                    "probabilities":{"yes":0.5,"no":0.5},"confidence":0.1})
            )).collect();
            (200, serde_json::json!({"model":"jev-test","answers":answers,
                "usage":{"input_tokens":10,"output_tokens":0}}).to_string())
        });
        jev_mock::machine(&AGENT_TOOL, "on", &endpoint.base_url, |cwd| {
            let home = runtime::default_config_home();
            let path = home.join("settings.json");
            let mut root: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            root["smart"]["jev"]["labelDrafts"] = Value::Bool(true);
            std::fs::write(&path, root.to_string()).unwrap();
            let ask = |context: &str| {
                let question = agent_tool::JevQuestion::new(agent_tool::JevShape::Ask, "Is this useful?",
                    Some(context), &[], &[], &[]).unwrap();
                agent_tool::decide(cwd, agent_tool::JevCaller::Cli, &question)
            };
            assert!(ask("first result").answered());
            assert_eq!(Store::at(&home).snapshot().unwrap().cases.len(), 1);
            root["smart"]["jev"]["labelDrafts"] = Value::Bool(false);
            std::fs::write(&path, root.to_string()).unwrap();
            assert!(ask("second result").answered());
            assert_eq!(Store::at(&home).snapshot().unwrap().cases.len(), 1);
            root["smart"][AGENT_TOOL.setting] = Value::String("off".to_string());
            std::fs::write(&path, root.to_string()).unwrap();
            assert!(!ask("third result").answered());
            assert_eq!(endpoint.requests().len(), 2, "off reaches neither the wire nor capture");
        });
    }

    #[test]
    fn a_prepared_batch_rechecks_the_global_switch_and_does_not_relabel_a_changed_model_pin() {
        use super::super::jev_mock;
        use zerocode_core::jev::AGENT_TOOL;
        let endpoint = jev_mock::Mock::serving(200, "{}".into());
        jev_mock::machine(&AGENT_TOOL, "on", &endpoint.base_url, |cwd| {
            let path = runtime::default_config_home().join("settings.json");
            let door = JevDoor::open(cwd);
            let mut root: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            root["smart"]["jevModel"] = Value::String("jev-another-pin".into());
            std::fs::write(&path, root.to_string()).unwrap();
            let body = || serde_json::json!({"state":{"question":"test"},"questions":{}});
            assert_eq!(door.pass(&AGENT_TOOL, false, body()).unwrap_err(), Refused::NoKey);
            assert_eq!(door.pass(&AGENT_TOOL, true, body()).unwrap_err(), Refused::SettingsChanged);
            assert_eq!(door.clear(&AGENT_TOOL, true, body()).unwrap_err(), Refused::SettingsChanged);
            assert_eq!(door.count_a_hedge(), Err(Refused::SettingsChanged));
            assert!(!door.permits_application_now());
            root["smart"]["jev"]["enabled"] = Value::Bool(false);
            std::fs::write(&path, root.to_string()).unwrap();
            assert_eq!(door.pass(&AGENT_TOOL, true, body()).unwrap_err(), Refused::Off);
            assert_eq!(door.clear(&AGENT_TOOL, true, body()).unwrap_err(), Refused::Off);
            assert_eq!(door.count_a_hedge(), Err(Refused::Off));
            root["smart"]["jev"]["enabled"] = Value::Bool(true);
            root["smart"]["jevModel"] = Value::String(door.model().to_string());
            root["smart"][AGENT_TOOL.setting] = Value::String("off".into());
            std::fs::write(&path, root.to_string()).unwrap();
            assert_eq!(door.pass(&AGENT_TOOL, true, body()).unwrap_err(), Refused::Off);
            assert_eq!(door.clear(&AGENT_TOOL, true, body()).unwrap_err(), Refused::Off);
            assert_eq!(count::sent(&door.requests), 0, "withheld requests are not counted as sent");
            assert!(endpoint.requests().is_empty());
        });
    }

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
                std::fs::read_to_string(path).is_ok_and(|text|
                    text.contains(".decide_body(") || text.contains(".decide_body_once("))
            })
            .map(|path| path.display().to_string())
            .collect();
        assert_eq!(callers.len(), 1, "the wire is called from {callers:?}");
        assert!(Path::new(&callers[0]).ends_with(Path::new("smart_router").join(gate)), "{callers:?}");
        assert!(sources.len() > 100, "the scan found the workspace: {} files", sources.len());
    }

    /// Every seat that sends through the door keeps what a managed server said
    /// beside a refusal on its row, and through the one helper (t-58193): a
    /// seat that sends and never calls it drops the reason a judgment failed.
    #[test]
    fn every_seat_that_sends_keeps_what_the_server_said() {
        let seats = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/misc_tools/smart_router");
        let gate = Path::new(file!()).file_name().expect("this file");
        let mut senders = Vec::new();
        for entry in std::fs::read_dir(&seats).expect("the seats' directory").flatten() {
            let path = entry.path();
            let rust = path.extension().is_some_and(|extension| extension == "rs");
            if path.file_name() == Some(gate) || !rust {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("a seat's source");
            if text.contains("jev_gate::send(") || text.contains("jev_gate::send_once(") {
                senders.push((path.display().to_string(), text.contains("ServerSaid::of(")));
            }
        }
        assert!(senders.len() >= 18, "the scan found the seats: {senders:?}");
        let silent: Vec<&String> =
            senders.iter().filter(|(_, keeps)| !keeps).map(|(path, _)| path).collect();
        assert!(silent.is_empty(), "these seats drop what the server said: {silent:?}");
    }

    /// A request of `seat`'s series as a seat's row keeps one: it left once,
    /// ended `outcome`, `ago` before now.
    fn sent(seat: &JevUse, outcome: &str, ago: Duration) -> Value {
        let ago = i64::try_from(ago.as_millis()).expect("a short time");
        serde_json::json!({"at": super::super::decision_shadow::now_ms() - ago,
            "rubricVersion": seat.rubric_version, "outcome": outcome, "requests": 1})
    }

    /// A request of `seat`'s series its server answered with a wait until
    /// `until`, in Unix milliseconds ([`SERVER_RETRY_AT`]).
    fn waited(seat: &JevUse, until: i64) -> Value {
        let mut row = sent(seat, "http_503", Duration::ZERO);
        row[SERVER_RETRY_AT] = until.into();
        row
    }

    /// `seat`'s ledger under `cwd`, holding `rows` and nothing else.
    fn ledger_holding(cwd: &Path, seat: &JevUse, rows: &[Value]) {
        holding(&shadow_ledger_path(cwd, seat.ledger), rows);
    }

    /// The ledger at `path`, holding `rows` and nothing else.
    fn holding(path: &Path, rows: &[Value]) {
        use std::fmt::Write as _;
        std::fs::create_dir_all(path.parent().expect("a ledger folder")).expect("the folder");
        let mut text = String::new();
        for row in rows {
            let _ = writeln!(text, "{row}");
        }
        std::fs::write(path, text).expect("the ledger");
    }

    /// The person's word for `seat`, written into the settings file the door
    /// reads.
    fn word_for(seat: &JevUse, mode: JevMode) {
        let path = runtime::default_config_home().join(SETTINGS_FILE);
        let mut root: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("settings")).expect("json");
        root["smart"][seat.setting] = Value::String(mode.key().into());
        std::fs::write(&path, root.to_string()).expect("settings written");
    }

    /// What the door says now of one request of `seat`'s: `None` when it lets
    /// it through, else its refusal's word — asked both ways a seat asks,
    /// clearing and passing, which must agree.
    fn door_says(cwd: &Path, seat: &JevUse) -> Option<&'static str> {
        let body = || serde_json::json!({"state": {"question": "test"}, "questions": {}});
        let door = JevDoor::open(cwd);
        let cleared = door.clear(seat, true, body()).err().map(Refused::token);
        let passed = door.pass(seat, true, body()).err().map(Refused::token);
        assert_eq!(cleared, passed, "clearing and passing disagree");
        passed
    }

    /// A seat that applies nothing rests once its last
    /// [`UNANSWERED_BEFORE_REST`] requests went unanswered (t-58193): the door
    /// refuses it with `resting` and counts nothing. When [`REST`] has passed
    /// since the newest of them one request goes, and a try that fails rests
    /// the seat again; an answer among them is no rest at all.
    #[test]
    fn a_recording_seat_whose_requests_go_unanswered_rests_then_tries_once() {
        use super::super::jev_mock;
        let endpoint = jev_mock::Mock::serving(200, "{}".into());
        jev_mock::machine(&RECALL, "shadow", &endpoint.base_url, |cwd| {
            let minute = Duration::from_secs(60);
            let failures = ["http_502", "timeout", "transport"];
            let unanswered = |count: usize, ago: Duration| -> Vec<Value> {
                (0..count).map(|n| sent(&RECALL, failures[n % failures.len()], ago)).collect()
            };
            ledger_holding(cwd, &RECALL, &unanswered(UNANSWERED_BEFORE_REST - 1, minute));
            assert_eq!(door_says(cwd, &RECALL), None, "one short of the rule asks");

            ledger_holding(cwd, &RECALL, &unanswered(UNANSWERED_BEFORE_REST, minute));
            let requests = JevDoor::open(cwd).requests;
            let counted = count::sent(&requests);
            assert_eq!(door_says(cwd, &RECALL), Some("resting"));
            assert_eq!(count::sent(&requests), counted, "a resting seat is not counted");

            let mut rested = unanswered(UNANSWERED_BEFORE_REST, REST + minute);
            ledger_holding(cwd, &RECALL, &rested);
            assert_eq!(door_says(cwd, &RECALL), None, "a rest that has passed lets one try go");
            rested.push(sent(&RECALL, "http_502", Duration::ZERO));
            ledger_holding(cwd, &RECALL, &rested);
            assert_eq!(door_says(cwd, &RECALL), Some("resting"), "a failed try rests it again");

            let mut answered = unanswered(UNANSWERED_BEFORE_REST, minute);
            answered[2] = sent(&RECALL, door::ANSWERED_OUTCOME, minute);
            ledger_holding(cwd, &RECALL, &answered);
            assert_eq!(door_says(cwd, &RECALL), None, "an answer among the last eight is no rest");
            assert!(endpoint.requests().is_empty(), "the door sends nothing itself");
        });
    }

    /// The rest reads the requests the seat sent in its series and nothing
    /// else (t-58193): a door's refusal, a memo's answer, a judge's
    /// transition and a mark count neither way, an answer under another
    /// rubric is not this series' answer, and a seat whose rubric moved on
    /// starts a new series with no rest to wait out.
    #[test]
    fn the_rest_reads_only_the_requests_a_seat_sent_in_its_series() {
        use super::super::jev_mock;
        let endpoint = jev_mock::Mock::serving(200, "{}".into());
        jev_mock::machine(&RECALL, "shadow", &endpoint.base_url, |cwd| {
            let minute = Duration::from_secs(60);
            let now = super::super::decision_shadow::now_ms();
            let rubric = RECALL.rubric_version;
            let answered = door::ANSWERED_OUTCOME;
            let mut rows = Vec::new();
            for _ in 0..UNANSWERED_BEFORE_REST {
                rows.push(sent(&RECALL, "http_502", minute));
                rows.push(serde_json::json!({"at": now, "rubricVersion": rubric,
                    "outcome": Refused::Budget.token(), "requests": 0}));
                rows.push(serde_json::json!({"at": now, "rubricVersion": rubric,
                    "outcome": answered, "cached": true, "requests": 0}));
                rows.push(serde_json::json!({"at": now, "transition": "fall",
                    "rubricVersions": [rubric]}));
                rows.push(serde_json::json!({"at": now, "label": "1:1", "agreed": true}));
            }
            ledger_holding(cwd, &RECALL, &rows);
            assert_eq!(door_says(cwd, &RECALL), Some("resting"));

            let moved = JevUse { rubric_version: rubric + 1, ..RECALL };
            assert_eq!(door_says(cwd, &moved), None, "a new rubric is a new series");
            rows.push(sent(&moved, answered, Duration::ZERO));
            ledger_holding(cwd, &RECALL, &rows);
            assert_eq!(door_says(cwd, &RECALL), Some("resting"), "another rubric's answer");
        });
    }

    /// A seat that applies its judgment never rests (t-58193): the person's
    /// `on`, and an `auto` its own ledger raised. An `auto` nobody raised
    /// records, and rests as `shadow` does.
    #[test]
    fn a_seat_that_applies_its_judgment_never_rests() {
        use super::super::jev_mock;
        let endpoint = jev_mock::Mock::serving(200, "{}".into());
        jev_mock::machine(&RECALL, "shadow", &endpoint.base_url, |cwd| {
            let minute = Duration::from_secs(60);
            let mut rows: Vec<Value> =
                (0..UNANSWERED_BEFORE_REST).map(|_| sent(&RECALL, "http_502", minute)).collect();
            ledger_holding(cwd, &RECALL, &rows);
            assert_eq!(door_says(cwd, &RECALL), Some("resting"), "shadow");
            word_for(&RECALL, JevMode::On);
            assert_eq!(door_says(cwd, &RECALL), None, "on");
            word_for(&RECALL, JevMode::Auto);
            assert_eq!(door_says(cwd, &RECALL), Some("resting"), "an auto nobody raised");
            rows.push(serde_json::json!({"at": super::super::decision_shadow::now_ms(),
                "transition": zerocode_core::jev::promote::ROSE,
                "rubricVersions": [RECALL.rubric_version]}));
            ledger_holding(cwd, &RECALL, &rows);
            assert_eq!(door_says(cwd, &RECALL), None, "an auto its ledger raised");
        });
    }

    /// A server that answers with a wait — a `503` or a `429` with a
    /// `Retry-After` — asks that seat for nothing until then (t-58193): the
    /// door refuses it with `resting` whatever its word, an applying seat's
    /// too, and under any rubric, since the wait is the server's; once the
    /// time has passed the seat asks again.
    #[test]
    fn a_seat_its_server_asked_to_wait_sends_nothing_until_then() {
        use super::super::jev_mock;
        let endpoint = jev_mock::Mock::serving(200, "{}".into());
        jev_mock::machine(&RECALL, "shadow", &endpoint.base_url, |cwd| {
            let now = super::super::decision_shadow::now_ms();
            let mut asked = sent(&RECALL, "http_503", Duration::ZERO);
            asked["serverRetryAt"] = (now + 60_000).into();
            ledger_holding(cwd, &RECALL, &[asked.clone()]);
            assert_eq!(door_says(cwd, &RECALL), Some("resting"), "shadow");
            word_for(&RECALL, JevMode::On);
            assert_eq!(door_says(cwd, &RECALL), Some("resting"), "on");
            let moved = JevUse { rubric_version: RECALL.rubric_version + 1, ..RECALL };
            assert_eq!(door_says(cwd, &moved), Some("resting"), "another rubric");
            asked["serverRetryAt"] = (now - 1).into();
            ledger_holding(cwd, &RECALL, &[asked]);
            assert_eq!(door_says(cwd, &RECALL), None, "a wait that has passed");
        });
    }

    /// An answer that asked for a wait is not an unanswered request
    /// (t-58193): its server said when to come back, and the door holds the
    /// seat until then instead. So such answers never bring the rest on —
    /// [`UNANSWERED_BEFORE_REST`] of them whose waits have passed let the
    /// next request go — and among other failures they count neither way.
    #[test]
    fn answers_that_asked_for_a_wait_do_not_count_toward_the_rest() {
        use super::super::jev_mock;
        let endpoint = jev_mock::Mock::serving(200, "{}".into());
        jev_mock::machine(&RECALL, "shadow", &endpoint.base_url, |cwd| {
            let minute = Duration::from_secs(60);
            let passed = super::super::decision_shadow::now_ms() - 1;
            let limited = api::SystemOneFailure::RateLimited.ledger_token();
            let asked = |outcome: &str| {
                let mut row = sent(&RECALL, outcome, minute);
                row["serverRetryAt"] = passed.into();
                row
            };
            let mut rows: Vec<Value> = (0..UNANSWERED_BEFORE_REST)
                .map(|n| asked(if n % 2 == 0 { "http_503" } else { limited.as_str() }))
                .collect();
            ledger_holding(cwd, &RECALL, &rows);
            assert_eq!(door_says(cwd, &RECALL), None, "a wait the server asked for is no rest");
            for _ in 0..UNANSWERED_BEFORE_REST {
                rows.push(sent(&RECALL, "http_502", minute));
                rows.push(asked("http_503"));
            }
            ledger_holding(cwd, &RECALL, &rows);
            assert_eq!(door_says(cwd, &RECALL), Some("resting"), "the others still count");
        });
    }

    /// A guard keeps its rows under the config home, one ledger for every
    /// project on this machine ([`super::super::tool_guard::command_guard_path`]),
    /// and the door reads the wait its server named there (t-58193), until
    /// the wait has passed. Every other seat's wait is read from the
    /// project's folder as before: a recall's row in the guards' folder
    /// holds nothing.
    #[test]
    fn a_guard_waits_on_its_own_ledger_and_a_project_seat_on_the_projects() {
        use super::super::jev_mock;
        use super::super::tool_guard::{command_guard_path, tool_text_guard_path};
        use zerocode_core::jev::{COMMAND_GUARD, TOOL_TEXT_GUARD};
        let endpoint = jev_mock::Mock::serving(200, "{}".into());
        let words = [
            (COMMAND_GUARD.setting, "on"),
            (TOOL_TEXT_GUARD.setting, "on"),
            (RECALL.setting, "shadow"),
        ];
        jev_mock::machine_words(&words, &endpoint.base_url, |cwd| {
            let now = super::super::decision_shadow::now_ms();
            let among_the_guards = command_guard_path().with_file_name(RECALL.ledger);
            holding(&among_the_guards, &[waited(&RECALL, now + 60_000)]);
            assert_eq!(door_says(cwd, &RECALL), None, "the guards' folder");
            ledger_holding(cwd, &RECALL, &[waited(&RECALL, now + 60_000)]);
            assert_eq!(door_says(cwd, &RECALL), Some("resting"), "the project's folder");
            let own = [
                (&COMMAND_GUARD, command_guard_path()),
                (&TOOL_TEXT_GUARD, tool_text_guard_path()),
            ];
            for (guard, ledger) in own {
                holding(&ledger, &[waited(guard, now + 60_000)]);
                assert_eq!(door_says(cwd, guard), Some("resting"), "{}", guard.id);
                holding(&ledger, &[waited(guard, now - 1)]);
                assert_eq!(door_says(cwd, guard), None, "{} once its wait has passed", guard.id);
            }
        });
    }

    /// A guard's wait is the provider's that named it (t-58193): each
    /// provider keeps its rows in a folder of its own
    /// (`runtime::judgment_evidence_dir`), so a guard whose person's settings
    /// name another provider asks it, and the first one's wait holds the
    /// guard again once they name the first.
    #[test]
    fn a_wait_one_provider_named_does_not_hold_a_guard_asking_another() {
        use super::super::jev_mock;
        use super::super::tool_guard::command_guard_path;
        use zerocode_core::jev::COMMAND_GUARD;
        let endpoint = jev_mock::Mock::serving(200, "{}".into());
        jev_mock::machine(&COMMAND_GUARD, "on", &endpoint.base_url, |cwd| {
            let ahead = super::super::decision_shadow::now_ms() + 60_000;
            holding(&command_guard_path(), &[waited(&COMMAND_GUARD, ahead)]);
            let path = runtime::default_config_home().join(SETTINGS_FILE);
            let first = std::fs::read_to_string(&path).expect("settings");
            let another = judgment_provider::Provider {
                kind: judgment_provider::ProviderKind::Local,
                protocol: judgment_provider::Protocol::SystemOne,
                base_url: endpoint.base_url.clone(),
                model: "acme/decision-model".to_string(),
                requires_auth: false,
            };
            let mut root: Value = serde_json::from_str(&first).expect("json");
            root["smart"][judgment_provider::SETTINGS_KEY] =
                serde_json::to_value(another).expect("a provider");
            std::fs::write(&path, root.to_string()).expect("another provider");
            assert_eq!(door_says(cwd, &COMMAND_GUARD), None, "another provider");
            std::fs::write(&path, first).expect("the first provider again");
            assert_eq!(door_says(cwd, &COMMAND_GUARD), Some("resting"), "the one that named it");
        });
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
            current_input_tokens: None,
            current_elapsed_ms: None,
            route_use: super::super::decision_shadow::DecisionRouteUse::RecordOnly,
            requests: Some(0),
            redacted_lines: Some(0),
            hedge_delay_ms: None,
            hedge_fired: None,
            hedge_won: None,
            loser_ms: None,
            probe: super::super::decision_shadow::ProbeCell::Failed("timeout".to_string()),
            jev: None,
            band: None,
            confidence: None,
            reading: None,
            rule: None,
            hangul_permille: None,
            server_said: ServerSaid::default(),
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
            room: None,
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
            server_said: ServerSaid::default(),
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

    /// The whole reading, through the real door: each use's delay comes from
    /// its OWN ledger's tail and from no other's.
    ///
    /// Both ledgers are written here, with real rows, and the two answers are
    /// opposites — so neither `None` can be the door quietly reading nothing.
    /// The routing rows are refused and the recall rows are planned, which is
    /// what `zerocode_core::jev::hedge` says of these two samples: routing's
    /// p75 is 2,257 ms, already past the wall, while recall's leaves the
    /// second copy 1,104 ms against a 275 ms median (t-5874).
    #[test]
    fn a_use_reads_its_delay_from_its_own_ledgers_tail() {
        /// Every answered routing judgment this machine's ledger held on
        /// 2026-09-18, in milliseconds.
        const ROUTING_LEDGER: [u64; 22] = [
            210, 218, 241, 250, 292, 311, 349, 473, 487, 616, 636, 641, 702, 1_020, 1_570, 1_953,
            2_257, 2_596, 3_140, 4_259, 4_847, 6_798,
        ];
        /// The recall window behind this machine's firing of 2026-09-21
        /// 13:30:10, which the second copy won.
        const RECALL_LEDGER: [u64; 16] = [
            238, 242, 245, 246, 270, 271, 272, 275, 301, 316, 336, 396, 490, 529, 556, 590,
        ];
        let home = tempfile::tempdir().expect("a config home");
        for ms in ROUTING_LEDGER {
            super::super::shadow_ledger::append_shadow_row(
                &home.path().join(ROUTING.ledger),
                &routing_row(door::ANSWERED_OUTCOME, ms),
                u64::MAX,
            )
            .expect("a sample row");
        }
        for ms in RECALL_LEDGER {
            super::super::shadow_ledger::append_shadow_row(
                &home.path().join(RECALL.ledger),
                &recall_row(door::ANSWERED_OUTCOME, ms),
                u64::MAX,
            )
            .expect("a sample row");
        }
        let settings = JevSettings {
            enabled: true,
            workspaces: vec!["/work/zo".to_string()],
            daily_requests: None,
            model: zerocode_core::jev::DEFAULT_MODEL.to_string(),
        };
        let door = JevDoor::at(settings, Path::new("/work/zo"), home.path());
        let wall = Duration::from_millis(1_500);

        assert_eq!(
            door.hedge_for(&RECALL, wall),
            Some(Duration::from_millis(396)),
            "the rule's own number, read back through the recall ledger"
        );
        assert_eq!(
            door.hedge_for(&ROUTING, wall),
            None,
            "routing's own rows name a delay past the wall, and the recall rows beside them are not its sample"
        );
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

    /// zo's client cannot read the core crate, so it spells the vendor's
    /// alias itself; the pin's default is that word, or an unpinned zo would
    /// ask for another model than an unpinned window does (t-6187).
    #[test]
    fn the_clients_alias_is_the_pins_default() {
        assert_eq!(api::SYSTEMONE_MODEL, zerocode_core::jev::DEFAULT_MODEL);
    }

    /// The model a seat keys its memo by is the one the door asks for: two
    /// pins are two keys, so a judgment one pin answered is never recalled
    /// under the other (t-6187).
    #[test]
    fn a_memo_is_keyed_by_the_model_the_door_asks_for() {
        let home = tempfile::tempdir().expect("a config home");
        let door_for = |model: &str| {
            let settings = JevSettings {
                enabled: true,
                workspaces: vec!["/work/zo".to_string()],
                daily_requests: None,
                model: model.to_string(),
            };
            JevDoor::at(settings, Path::new("/work/zo"), home.path())
        };
        let pinned = door_for("jev-1.13.0");
        let alias = door_for(zerocode_core::jev::DEFAULT_MODEL);
        assert_eq!(pinned.model(), "jev-1.13.0");
        assert_eq!(pinned.model_key(), model_key("jev-1.13.0"));
        assert_ne!(pinned.model_key(), alias.model_key());
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

    /// A person's own decisions ask the judgment provider and no one else
    /// (t-62170): at each of the four points every request reaches the System
    /// One route under its key, and — where a script can stand in for one — a
    /// vendor's own client first on `PATH`, the subscription road of
    /// `zerocode_core::type_value::Road`, never runs. The decisions' source
    /// names neither that module nor a road.
    #[test]
    fn custom_decisions_reach_only_the_judgment_provider() {
        use api::sync_bridge::run_blocking;
        use runtime::skill_rank::SkillSuggestionSeat;
        use runtime::{ConversationMessage, ToolGuardSeat};
        use zerocode_core::jev::custom::Point;
        use super::super::custom_decisions::{decide, tests as decisions};
        use super::super::jev_mock::machine_words;
        use super::super::project_rules::note_project_rule_turn;
        use super::super::skill_search::SkillSuggestionJudge;
        use super::super::tool_guard::ToolGuardJudge;

        let source = include_str!("custom_decisions.rs");
        for named in ["type_value", "Road::"] {
            assert!(!source.contains(named), "the decisions name {named}");
        }
        let mock = decisions::replying(|_| decisions::a_noul(0.9));
        machine_words(&decisions::words(Some(JevMode::On.key())), &mock.base_url, |cwd| {
            let clients = tempfile::tempdir().expect("a folder for the vendors' clients");
            let ran = clients.path().join("ran");
            // Telemetry looks for claude on PATH once per process: settle that before the fake claude goes first.
            let _ = telemetry::claude_code_version();
            #[cfg(unix)]
            let _restored = clients_first_on_path(clients.path(), &ran);
            for point in Point::ALL {
                decisions::keep(&decisions::yes_no(point.key(), point), JevMode::On);
            }
            run_blocking(SkillSuggestionJudge::at(cwd).suggest(decisions::TASK.to_string()));
            let guard = ToolGuardJudge::at(cwd);
            guard.command(decisions::command_ask(cwd, "clean", "rm -rf build"));
            run_blocking(guard.command_ran(decisions::finished("clean")));
            note_project_rule_turn(cwd, "session", "turn-1", &[ConversationMessage::user_text(decisions::TASK)]);
            decide(cwd, Point::AgentAsks.key(), Some("the diff adds one test"), &[]).expect("a verdict");
            let rows = decisions::rows_in(cwd, Point::ALL.len());
            assert!(rows.len() == Point::ALL.len() && rows.iter().all(|row| row["requests"] == 1), "{rows:?}");
            let key = std::env::var(api::SYSTEMONE_API_KEY_ENV).expect("the judgment provider's key");
            let heads = mock.heads();
            assert_eq!(heads.len(), Point::ALL.len(), "one request a point");
            for head in heads.iter().map(|head| head.to_ascii_lowercase()) {
                assert!(head.starts_with(&format!("post {} http/1.1", api::SYSTEMONE_PATH)), "{head}");
                assert!(head.contains(&format!("authorization: bearer {key}")), "{head}");
            }
            assert!(!ran.exists(), "a vendor's own client ran: {:?}", std::fs::read_to_string(&ran));
        });
    }

    /// `PATH` as it stood before a test put a folder first, put back however
    /// the test ends.
    #[cfg(unix)]
    struct PathAsItWas(std::ffi::OsString);

    #[cfg(unix)]
    impl Drop for PathAsItWas {
        fn drop(&mut self) {
            std::env::set_var("PATH", &self.0);
        }
    }

    /// The subscription roads' own clients as scripts in `folder`, each noting
    /// its every run in `ran`, first on `PATH` until what this returns drops.
    #[cfg(unix)]
    fn clients_first_on_path(folder: &Path, ran: &Path) -> PathAsItWas {
        use std::os::unix::fs::PermissionsExt;
        use zerocode_core::AgentKind;
        for agent in [AgentKind::Claude, AgentKind::Codex] {
            let client = folder.join(agent.slug());
            std::fs::write(&client, format!("#!/bin/sh\necho {} >> '{}'\n", agent.slug(), ran.display()))
                .expect("a client");
            std::fs::set_permissions(&client, std::fs::Permissions::from_mode(0o755)).expect("a client that runs");
        }
        let path = std::env::var_os("PATH").unwrap_or_default();
        let first = std::iter::once(folder.to_path_buf());
        std::env::set_var("PATH", std::env::join_paths(first.chain(std::env::split_paths(&path))).expect("a PATH"));
        PathAsItWas(path)
    }
}
