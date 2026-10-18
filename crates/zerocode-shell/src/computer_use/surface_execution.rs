use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zerocode_core::computer_use::{ComputerCommand, ComputerMethod};
use zerocode_core::surface_execution::{
    SURFACE_EXECUTION_CACHE_LIMIT, SurfaceEffect, SurfaceExecution,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Epoch {
    provider: u64,
    stopped: u64,
}

impl Epoch {
    fn current() -> Self {
        Self {
            provider: super::session_epoch(),
            stopped: super::guard::stop_generation(),
        }
    }
}

pub(crate) struct Context {
    actor: crate::run_evidence::Actor,
    stopped: u64,
    provider: Rc<Cell<ProviderTrace>>,
    previous: Option<Rc<Cell<ProviderTrace>>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProviderTrace {
    Unseen,
    One(u64),
    Changed,
}

thread_local! {
    static PROVIDER_TRACE: RefCell<Option<Rc<Cell<ProviderTrace>>>> = const { RefCell::new(None) };
}

impl Context {
    fn epoch(&self) -> Option<Epoch> {
        match self.provider.get() {
            ProviderTrace::One(provider) => Some(Epoch {
                provider,
                stopped: self.stopped,
            }),
            ProviderTrace::Unseen | ProviderTrace::Changed => None,
        }
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        PROVIDER_TRACE.with(|active| {
            let mut active = active.borrow_mut();
            if active
                .as_ref()
                .is_some_and(|trace| Rc::ptr_eq(trace, &self.provider))
            {
                *active = self.previous.take();
            }
        });
    }
}

fn begin_for(actor: crate::run_evidence::Actor, before: Epoch) -> Context {
    let provider = Rc::new(Cell::new(ProviderTrace::Unseen));
    let previous = PROVIDER_TRACE.with(|active| active.replace(Some(Rc::clone(&provider))));
    Context {
        actor,
        stopped: before.stopped,
        provider,
        previous,
    }
}

pub(super) fn record_provider(provider: u64) {
    PROVIDER_TRACE.with(|active| {
        if let Some(trace) = active.borrow().as_ref() {
            trace.set(match trace.get() {
                ProviderTrace::Unseen => ProviderTrace::One(provider),
                ProviderTrace::One(previous) if previous == provider => {
                    ProviderTrace::One(provider)
                }
                ProviderTrace::One(_) | ProviderTrace::Changed => ProviderTrace::Changed,
            });
        }
    });
}

pub(crate) fn begin(method: ComputerMethod) -> Option<Context> {
    if !supported(method) {
        return None;
    }
    let actor = crate::run_evidence::actor()?;
    actor.pane.as_deref().filter(|pane| !pane.is_empty())?;
    actor.scope.as_deref().filter(|scope| !scope.is_empty())?;
    Some(begin_for(actor, Epoch::current()))
}

fn supported(method: ComputerMethod) -> bool {
    matches!(
        method,
        ComputerMethod::Observe
            | ComputerMethod::GetAppState
            | ComputerMethod::Click
            | ComputerMethod::SetValue
            | ComputerMethod::TypeText
            | ComputerMethod::PressKey
            | ComputerMethod::Scroll
    )
}

struct HeldSurface {
    identity: String,
    views: VecDeque<(String, [u8; 32])>,
    revision: u64,
    acknowledged: Option<(u64, String)>,
}

struct Tracker {
    instance: [u8; 16],
    revision: u64,
    held: VecDeque<HeldSurface>,
}

impl Default for Tracker {
    fn default() -> Self {
        Self {
            instance: rand::random(),
            revision: 0,
            held: VecDeque::with_capacity(SURFACE_EXECUTION_CACHE_LIMIT),
        }
    }
}

impl Tracker {
    fn identity(&self, context: &Context, target: &Target<'_>, epoch: Epoch) -> String {
        let mut digest = Sha256::new();
        digest.update(self.instance);
        digest.update(epoch.provider.to_le_bytes());
        hash_word(&mut digest, context.actor.pane.as_deref());
        hash_word(&mut digest, context.actor.task.as_deref());
        hash_word(&mut digest, context.actor.scope.as_deref());
        digest.update(target.process.to_le_bytes());
        digest.update(target.window_id.to_le_bytes());
        format!("{:x}", digest.finalize())
    }

    fn observe(&mut self, identity: String, fingerprint: [u8; 32]) -> Option<SurfaceExecution> {
        self.observe_view(identity, String::new(), fingerprint)
    }

    fn observe_view(
        &mut self,
        identity: String,
        view: String,
        fingerprint: [u8; 32],
    ) -> Option<SurfaceExecution> {
        let index = self.held.iter().position(|held| held.identity == identity);
        let (mut held, effect) = if let Some(index) = index {
            let mut held = self.held.remove(index)?;
            let previous = held.revision;
            let previous_view = held
                .views
                .iter()
                .position(|(key, _)| key == &view)
                .and_then(|index| held.views.remove(index));
            let changed = previous_view.is_some_and(|(_, previous)| previous != fingerprint);
            let acknowledged = held.acknowledged.take();
            let effect = if changed {
                let Some(revision) = self.revision.checked_add(1) else {
                    self.held.push_back(held);
                    return None;
                };
                self.revision = revision;
                held.revision = revision;
                if acknowledged.is_some_and(|(revision, observed_view)| {
                    revision == previous && observed_view == view
                }) {
                    SurfaceEffect::ConfirmedChange {
                        previous_revision: previous,
                    }
                } else {
                    SurfaceEffect::Observation
                }
            } else {
                SurfaceEffect::Observation
            };
            (held, effect)
        } else {
            if self.held.len() >= SURFACE_EXECUTION_CACHE_LIMIT {
                self.held.pop_front();
            }
            (
                HeldSurface {
                    identity: identity.clone(),
                    views: VecDeque::new(),
                    revision: 0,
                    acknowledged: None,
                },
                SurfaceEffect::Observation,
            )
        };
        if held.views.len() >= SURFACE_EXECUTION_CACHE_LIMIT {
            held.views.pop_front();
        }
        held.views.push_back((view, fingerprint));
        let evidence = SurfaceExecution {
            surface_id: identity,
            state_revision: held.revision,
            effect,
        };
        self.held.push_back(held);
        Some(evidence)
    }

    fn cancel_confirmation(&mut self, identity: &str) {
        if let Some(held) = self.held.iter_mut().find(|held| held.identity == identity) {
            held.acknowledged = None;
        }
    }

    fn acknowledge(&mut self, identity: String, can_confirm: bool) -> Option<SurfaceExecution> {
        let held = self
            .held
            .iter_mut()
            .find(|held| held.identity == identity)?;
        held.acknowledged = if can_confirm {
            held.views
                .back()
                .map(|(view, _)| (held.revision, view.clone()))
        } else {
            None
        };
        Some(SurfaceExecution {
            surface_id: identity,
            state_revision: held.revision,
            effect: SurfaceEffect::Acknowledged,
        })
    }
}

struct Target<'a> {
    process: u64,
    window_id: u64,
    window: &'a Value,
    text: Option<&'a str>,
    elements: Option<u64>,
}

impl<'a> Target<'a> {
    fn read(command: &ComputerCommand, result: &'a Value) -> Option<Self> {
        let (app, window, text, elements) = if command.method == ComputerMethod::Observe {
            let tree = result.get("tree")?;
            (
                result.get("app")?,
                tree.get("window")?,
                tree.get("text"),
                tree.get("elementCount"),
            )
        } else {
            let snapshot = result.get("snapshot")?;
            (
                snapshot.get("app")?,
                snapshot.get("window")?,
                snapshot.get("treeText"),
                snapshot.get("elementCount"),
            )
        };
        let process = app.get("pid")?.as_u64().filter(|process| *process > 0)?;
        let window_id = window.get("id")?.as_u64().filter(|window| *window > 0)?;
        if let Some(requested) = command.params.get("windowId")
            && requested.as_u64() != Some(window_id)
        {
            return None;
        }
        if let Some(requested) = command
            .params
            .get("app")
            .and_then(Value::as_str)
            .and_then(|app| app.strip_prefix("pid:"))
            && requested.parse::<u64>().ok() != Some(process)
        {
            return None;
        }
        Some(Self {
            process,
            window_id,
            window,
            text: text.and_then(Value::as_str),
            elements: elements.and_then(Value::as_u64),
        })
    }

    fn fingerprint(&self) -> Option<[u8; 32]> {
        let mut digest = Sha256::new();
        hash_word(&mut digest, Some(self.text?));
        hash_word(
            &mut digest,
            self.window.get("title").and_then(Value::as_str),
        );
        hash_word(
            &mut digest,
            self.elements.map(|count| count.to_string()).as_deref(),
        );
        for field in ["x", "y", "width", "height"] {
            let value = self.window.get(field).map(Value::to_string);
            hash_word(&mut digest, value.as_deref());
        }
        Some(digest.finalize().into())
    }
}

fn hash_word(digest: &mut Sha256, value: Option<&str>) {
    if let Some(value) = value {
        digest.update([1]);
        digest.update((value.len() as u64).to_le_bytes());
        digest.update(value.as_bytes());
    } else {
        digest.update([0]);
    }
}

static TRACKER: OnceLock<Mutex<Tracker>> = OnceLock::new();

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct BrowserObservation {
    label: String,
    fingerprint: [u8; 32],
}

impl BrowserObservation {
    fn of(label: &str, report: &crate::cmd::browser::BrowserReadReport) -> Self {
        let mut digest = Sha256::new();
        for value in [
            Some(report.title.as_str()),
            Some(report.url.as_str()),
            Some(report.text.as_str()),
            report.dom.as_deref(),
        ] {
            hash_word(&mut digest, value);
        }
        Self {
            label: label.into(),
            fingerprint: digest.finalize().into(),
        }
    }
}

#[derive(Default)]
struct BrowserTrace {
    observation: Option<BrowserObservation>,
    changed: bool,
}

tokio::task_local! {
    static BROWSER_OBSERVATION: Mutex<BrowserTrace>;
}

/// A fresh task-local for each request, including concurrent or nested reads.
/// Cancellation drops its observation rather than leaving it for another call.
pub(crate) async fn observing_browser<T>(
    run: impl std::future::Future<Output = T>,
) -> (T, Option<BrowserObservation>) {
    BROWSER_OBSERVATION
        .scope(Mutex::new(BrowserTrace::default()), async {
            let answer = run.await;
            let observation = BROWSER_OBSERVATION.with(|held| {
                held.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .observation
                    .take()
            });
            (answer, observation)
        })
        .await
}

/// Called on the typed engine result before presentation or noise removal.
/// No page text is interpreted as evidence and no content is retained here.
pub(crate) fn record_browser_read(label: &str, report: &crate::cmd::browser::BrowserReadReport) {
    let _ = BROWSER_OBSERVATION.try_with(|held| {
        let mut trace = held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let observed = BrowserObservation::of(label, report);
        if trace.changed {
            return;
        }
        match trace.observation.as_ref() {
            None => trace.observation = Some(observed),
            Some(previous) if previous == &observed => {}
            Some(_) => {
                trace.changed = true;
                trace.observation = None;
            }
        }
    });
}

/// Captured only for a managed browser whose original control interval is
/// checked before and after dispatch. The caller also rechecks the actor.
pub(crate) struct BrowserContext {
    actor: crate::run_evidence::Actor,
    label: String,
    control_revision: u64,
    stopped: u64,
}

impl BrowserContext {
    pub(crate) fn new(
        actor: crate::run_evidence::Actor,
        label: &str,
        control_revision: u64,
    ) -> Option<Self> {
        actor.pane.as_deref().filter(|value| !value.is_empty())?;
        actor.scope.as_deref().filter(|value| !value.is_empty())?;
        Some(Self {
            actor,
            label: label.into(),
            control_revision,
            stopped: super::guard::stop_generation(),
        })
    }

    pub(crate) fn finish(
        &self,
        argv: &[String],
        answer: &zerocode_hookd::TeamAnswer,
        observation: Option<&BrowserObservation>,
    ) -> Option<SurfaceExecution> {
        if self.stopped != super::guard::stop_generation() {
            return None;
        }
        let mut tracker = TRACKER
            .get_or_init(|| Mutex::new(Tracker::default()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.finish_at(argv, answer, observation, &mut tracker)
    }

    fn finish_at(
        &self,
        argv: &[String],
        answer: &zerocode_hookd::TeamAnswer,
        observation: Option<&BrowserObservation>,
        tracker: &mut Tracker,
    ) -> Option<SurfaceExecution> {
        let command = zerocode_core::agent_browser::parse_workspace_command(argv, None).ok()?;
        let verb = command.argv.first()?.as_str();
        if command.argv.get(1)? != &self.label {
            return None;
        }
        let mut digest = Sha256::new();
        digest.update(tracker.instance);
        hash_word(&mut digest, Some("managed-browser"));
        hash_word(&mut digest, self.actor.pane.as_deref());
        hash_word(&mut digest, self.actor.task.as_deref());
        hash_word(&mut digest, self.actor.scope.as_deref());
        hash_word(&mut digest, Some(&self.label));
        digest.update(self.control_revision.to_le_bytes());
        let identity = format!("{:x}", digest.finalize());
        if verb == "eval" {
            // A failed evaluation can still have changed the page before it
            // failed. It cannot confirm an earlier acknowledged action.
            tracker.cancel_confirmation(&identity);
        }
        if answer.exit_code != 0 || !answer.stderr.is_empty() {
            return None;
        }
        if verb == "read" {
            // This last line was stamped by finish_observation, after the page
            // fence. A page's revision-shaped text never supplies this proof.
            let text = answer.stdout.strip_suffix('\n').unwrap_or(&answer.stdout);
            let (_, revision) = text.rsplit_once('\n')?;
            if revision
                .strip_prefix(zerocode_core::agent_browser::WORKSPACE_REVISION_LINE)?
                .parse::<u64>()
                .ok()?
                != self.control_revision
            {
                return None;
            }
            let observation = observation.filter(|observed| observed.label == self.label)?;
            let read = zerocode_core::agent_browser::parse_read(&command.argv).ok()?;
            let view = serde_json::to_string(&(
                verb,
                read.selector
                    .as_deref()
                    .map(zerocode_core::agent_browser::read_selector_view),
                read.full,
            ))
            .ok()?;
            tracker.observe_view(identity, view, observation.fingerprint)
        } else if zerocode_core::agent_browser::acts(verb) {
            tracker.acknowledge(identity, verb != "eval")
        } else {
            None
        }
    }
}

pub(crate) fn envelope(
    command: &ComputerCommand,
    result: Value,
    context: Option<&Context>,
) -> Value {
    let Some(context) = context else {
        return json!({ "ok": true, "result": result });
    };
    let mut tracker = TRACKER
        .get_or_init(|| Mutex::new(Tracker::default()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    envelope_at(
        command,
        result,
        Some(context),
        Epoch::current(),
        &mut tracker,
    )
}

fn envelope_at(
    command: &ComputerCommand,
    result: Value,
    context: Option<&Context>,
    current: Epoch,
    tracker: &mut Tracker,
) -> Value {
    let evidence = context
        .and_then(|context| Some((context, context.epoch()?)))
        .filter(|(context, epoch)| {
            *epoch == current
                && supported(command.method)
                && context
                    .actor
                    .pane
                    .as_deref()
                    .is_some_and(|pane| !pane.is_empty())
                && context
                    .actor
                    .scope
                    .as_deref()
                    .is_some_and(|scope| !scope.is_empty())
        })
        .and_then(|(context, epoch)| {
            let target = Target::read(command, &result)?;
            let identity = tracker.identity(context, &target, epoch);
            if matches!(
                command.method,
                ComputerMethod::Observe | ComputerMethod::GetAppState
            ) {
                tracker.observe(identity, target.fingerprint()?)
            } else {
                tracker.acknowledge(identity, true)
            }
        });
    let mut envelope = json!({ "ok": true, "result": result });
    if let Some(evidence) = evidence.and_then(|evidence| serde_json::to_value(evidence).ok()) {
        envelope["surfaceExecution"] = evidence;
    }
    envelope
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run_evidence::Actor;
    use zerocode_core::surface_execution::{SurfaceEffect, SurfaceExecution};

    const EPOCH: Epoch = Epoch {
        provider: 7,
        stopped: 3,
    };

    fn context(pane: &str) -> Context {
        Context {
            actor: Actor {
                pane: Some(pane.to_string()),
                task: Some("task-fixture".to_string()),
                scope: Some("assignment-fixture".to_string()),
            },
            stopped: EPOCH.stopped,
            provider: Rc::new(Cell::new(ProviderTrace::One(EPOCH.provider))),
            previous: None,
        }
    }

    fn command(method: ComputerMethod, window: u64) -> ComputerCommand {
        ComputerCommand {
            method,
            params: json!({ "app": "Fixture", "windowId": window }),
            json: true,
        }
    }

    fn observed(window: u64, text: &str) -> Value {
        json!({
            "app": { "pid": 4242, "bundleId": "test.fixture" },
            "tree": {
                "id": "snapshot-fixture",
                "window": { "id": window, "title": "Fixture" },
                "text": text,
                "elementCount": 2
            },
            "screenshot": null,
            "marks": { "lookId": "look-fixture" }
        })
    }

    fn execution(answer: &Value) -> SurfaceExecution {
        assert!(
            answer.get("surfaceExecution").is_some(),
            "the host must produce separate surface evidence for an owned provider response"
        );
        serde_json::from_value(answer["surfaceExecution"].clone())
            .expect("the host reports typed evidence outside the result body")
    }

    fn snapshot(window: u64, text: &str) -> Value {
        let observed = observed(window, text);
        json!({
            "snapshot": {
                "id": "snapshot-other",
                "app": observed["app"],
                "window": observed["tree"]["window"],
                "treeText": text,
                "elementCount": 2
            }
        })
    }

    fn look(tracker: &mut Tracker, owner: &Context, window: u64, text: &str) -> Value {
        envelope_at(
            &command(ComputerMethod::Observe, window),
            observed(window, text),
            Some(owner),
            EPOCH,
            tracker,
        )
    }

    #[test]
    fn alternating_browser_views_keep_one_unchanged_surface_revision() {
        let mut tracker = Tracker::default();
        let scope = BrowserContext::new(context("term-7").actor.clone(), "browser-1", 7).unwrap();
        let mut revisions = Vec::new();
        for _ in 0..4 {
            for (argv, text) in [
                (vec!["read", "browser-1"], "Whole page including Next"),
                (vec!["read", "browser-1", "h1"], "Next"),
                (
                    vec!["read", "browser-1", "--full"],
                    "Full page including Next",
                ),
            ] {
                let read = argv.into_iter().map(str::to_string).collect::<Vec<_>>();
                let evidence = scope
                    .finish_at(
                        &read,
                        &browser_answer(text, 7),
                        Some(&browser_observation("browser-1", text)),
                        &mut tracker,
                    )
                    .unwrap();
                revisions.push(evidence.state_revision);
            }
        }
        let advances = revisions.iter().copied().max().unwrap_or(0);
        eprintln!(
            "SURFACE_VIEW_METRIC {}",
            json!({"views":3,"reads":revisions.len(),"revisionAdvances":advances})
        );
        assert_eq!(
            advances, 0,
            "an unchanged view must not borrow another view's fingerprint"
        );
    }

    #[test]
    fn non_ascii_and_escaped_selector_characters_keep_distinct_observation_views() {
        let mut tracker = Tracker::default();
        let scope = BrowserContext::new(context("term-7").actor.clone(), "browser-1", 7).unwrap();
        let selectors = [
            "h1".to_string(),
            "h1\u{00a0}".to_string(),
            format!("#tail{} ", char::from(92_u8)),
            format!("#tail{}\t", char::from(92_u8)),
        ];
        for _ in 0..3 {
            for (index, selector) in selectors.iter().enumerate() {
                let read = ["read".to_string(), "browser-1".into(), selector.clone()];
                let text = format!("View {index}");
                let evidence = scope
                    .finish_at(
                        &read,
                        &browser_answer(&text, 7),
                        Some(&browser_observation("browser-1", &text)),
                        &mut tracker,
                    )
                    .unwrap();
                assert_eq!(
                    evidence.state_revision, 0,
                    "non-ASCII and escaped selector characters must keep their own unchanged observation view"
                );
            }
        }
    }

    #[test]
    fn a_browser_view_switch_after_an_action_is_not_a_confirmed_change() {
        let mut tracker = Tracker::default();
        let scope = BrowserContext::new(context("term-7").actor.clone(), "browser-1", 7).unwrap();
        scope
            .finish_at(
                &["read".into(), "browser-1".into()],
                &browser_answer("Whole page", 7),
                Some(&browser_observation("browser-1", "Whole page")),
                &mut tracker,
            )
            .unwrap();
        scope
            .finish_at(
                &["click".into(), "browser-1".into(), "#next".into()],
                &browser_answer("clicked", 7),
                None,
                &mut tracker,
            )
            .unwrap();
        let switched = scope
            .finish_at(
                &["read".into(), "browser-1".into(), "h1".into()],
                &browser_answer("Heading", 7),
                Some(&browser_observation("browser-1", "Heading")),
                &mut tracker,
            )
            .unwrap();
        assert_eq!(
            switched.effect,
            SurfaceEffect::Observation,
            "a first observation of another view cannot confirm the preceding act"
        );
        assert_eq!(
            switched.state_revision, 0,
            "a new view starts at the existing revision"
        );
    }

    #[test]
    fn a_changed_stale_view_cannot_confirm_an_action_observed_through_another_view() {
        let mut tracker = Tracker::default();
        let scope = BrowserContext::new(context("term-7").actor.clone(), "browser-1", 7).unwrap();
        let whole = ["read".into(), "browser-1".into()];
        let selected = ["read".into(), "browser-1".into(), "h1".into()];
        scope
            .finish_at(
                &selected,
                &browser_answer("Old heading", 7),
                Some(&browser_observation("browser-1", "Old heading")),
                &mut tracker,
            )
            .unwrap();
        scope
            .finish_at(
                &whole,
                &browser_answer("Old whole page", 7),
                Some(&browser_observation("browser-1", "Old whole page")),
                &mut tracker,
            )
            .unwrap();
        let before = scope
            .finish_at(
                &whole,
                &browser_answer("New whole page", 7),
                Some(&browser_observation("browser-1", "New whole page")),
                &mut tracker,
            )
            .unwrap();
        scope
            .finish_at(
                &["click".into(), "browser-1".into(), "#next".into()],
                &browser_answer("accepted", 7),
                None,
                &mut tracker,
            )
            .unwrap();
        let after = scope
            .finish_at(
                &selected,
                &browser_answer("New heading", 7),
                Some(&browser_observation("browser-1", "New heading")),
                &mut tracker,
            )
            .unwrap();
        assert!(after.state_revision > before.state_revision);
        assert_eq!(
            after.effect,
            SurfaceEffect::Observation,
            "a changed stale view cannot confirm an action whose baseline used another view"
        );
    }

    #[test]
    fn browser_eval_content_changes_do_not_confirm_an_action() {
        let mut tracker = Tracker::default();
        let scope = BrowserContext::new(context("term-7").actor.clone(), "browser-1", 7).unwrap();
        let read = ["read".into(), "browser-1".into()];
        scope
            .finish_at(
                &read,
                &browser_answer("Before", 7),
                Some(&browser_observation("browser-1", "Before")),
                &mut tracker,
            )
            .unwrap();
        scope
            .finish_at(
                &[
                    "eval".into(),
                    "browser-1".into(),
                    "document.title = 'After'".into(),
                ],
                &browser_answer("After", 7),
                None,
                &mut tracker,
            )
            .unwrap();
        let changed = scope
            .finish_at(
                &read,
                &browser_answer("After", 7),
                Some(&browser_observation("browser-1", "After")),
                &mut tracker,
            )
            .unwrap();
        assert!(
            changed.state_revision > 0,
            "changed content still advances its observation revision"
        );
        assert_eq!(
            changed.effect,
            SurfaceEffect::Observation,
            "agent-authored eval changes cannot confirm progress"
        );
    }

    #[test]
    fn a_failed_browser_eval_cannot_confirm_a_preceding_action() {
        let mut tracker = Tracker::default();
        let scope = BrowserContext::new(context("term-7").actor.clone(), "browser-1", 7).unwrap();
        let read = ["read".into(), "browser-1".into()];
        scope
            .finish_at(
                &read,
                &browser_answer("Before", 7),
                Some(&browser_observation("browser-1", "Before")),
                &mut tracker,
            )
            .unwrap();
        scope
            .finish_at(
                &["click".into(), "browser-1".into(), "#next".into()],
                &browser_answer("accepted", 7),
                None,
                &mut tracker,
            )
            .unwrap();
        let mut failed = browser_answer("evaluation failed after a partial page change", 7);
        failed.exit_code = 1;
        failed.stderr = "fixture evaluation failure".into();
        assert!(
            scope
                .finish_at(
                    &[
                        "eval".into(),
                        "browser-1".into(),
                        "fixture expression".into()
                    ],
                    &failed,
                    None,
                    &mut tracker
                )
                .is_none()
        );
        let after = scope
            .finish_at(
                &read,
                &browser_answer("After", 7),
                Some(&browser_observation("browser-1", "After")),
                &mut tracker,
            )
            .unwrap();
        assert!(after.state_revision > 0);
        assert_eq!(
            after.effect,
            SurfaceEffect::Observation,
            "a failed eval's content change cannot confirm a preceding acknowledged action"
        );
    }

    #[test]
    fn computer_stop_and_resume_preserve_the_owned_surface_identity() {
        let mut tracker = Tracker::default();
        let mut owner = context("term-7");
        let before = execution(&look(&mut tracker, &owner, 43, "Ready"));
        owner.stopped += 1;
        let after = execution(&envelope_at(
            &command(ComputerMethod::Observe, 43),
            observed(43, "Ready"),
            Some(&owner),
            Epoch {
                stopped: owner.stopped,
                ..EPOCH
            },
            &mut tracker,
        ));
        assert_eq!(
            after, before,
            "an agent stop/resume cannot mint a new Computer surface or revision"
        );
    }

    #[test]
    fn browser_stop_and_resume_preserve_the_owned_surface_identity() {
        let mut tracker = Tracker::default();
        let mut scope =
            BrowserContext::new(context("term-7").actor.clone(), "browser-1", 7).unwrap();
        let read = ["read".into(), "browser-1".into()];
        let before = scope
            .finish_at(
                &read,
                &browser_answer("Ready", 7),
                Some(&browser_observation("browser-1", "Ready")),
                &mut tracker,
            )
            .unwrap();
        scope.stopped += 1;
        let after = scope
            .finish_at(
                &read,
                &browser_answer("Ready", 7),
                Some(&browser_observation("browser-1", "Ready")),
                &mut tracker,
            )
            .unwrap();
        assert_eq!(
            after, before,
            "an agent stop/resume cannot mint a new browser surface or revision"
        );
    }

    #[test]
    fn three_agent_stop_resume_cycles_cannot_mint_repeat_allowances() {
        let mut tracker = Tracker::default();
        let mut owner = context("term-7");
        let mut identities = std::collections::HashSet::new();
        identities.insert(execution(&look(&mut tracker, &owner, 43, "Ready")).surface_id);
        for _ in 0..3 {
            owner.stopped += 1;
            let after = execution(&envelope_at(
                &command(ComputerMethod::Observe, 43),
                observed(43, "Ready"),
                Some(&owner),
                Epoch {
                    stopped: owner.stopped,
                    ..EPOCH
                },
                &mut tracker,
            ));
            identities.insert(after.surface_id);
        }
        let extra = identities.len() - 1;
        eprintln!(
            "SURFACE_STOP_METRIC {}",
            json!({"stopResumeCycles":3,"newSurfaceIds":extra})
        );
        assert_eq!(
            extra, 0,
            "three agent stop/resume cycles must create zero extra surface identities"
        );
    }

    #[test]
    fn a_first_host_observation_is_a_zero_baseline_not_an_effect() {
        let mut tracker = Tracker::default();
        let answer = look(&mut tracker, &context("term-7"), 43, "Next");
        let evidence = execution(&answer);
        assert!(!evidence.surface_id.is_empty());
        assert_eq!(evidence.state_revision, 0);
        assert_eq!(evidence.effect, SurfaceEffect::Observation);
        assert_eq!(answer["result"]["tree"]["text"], "Next");
    }

    #[test]
    fn new_snapshot_ids_and_timings_do_not_change_the_observed_state() {
        let mut tracker = Tracker::default();
        let owner = context("term-7");
        let first = execution(&look(&mut tracker, &owner, 43, "Next"));
        let mut next = observed(43, "Next");
        next["tree"]["id"] = json!("another-snapshot");
        next["marks"]["lookId"] = json!("another-look");
        next["elapsedMs"] = json!(991);
        next["screenshot"] = json!({ "path": "frame-fixture.png" });
        let answer = envelope_at(
            &command(ComputerMethod::Observe, 43),
            next,
            Some(&owner),
            EPOCH,
            &mut tracker,
        );
        assert_eq!(execution(&answer), first);
    }

    #[test]
    fn changed_observation_is_not_a_confirmed_action() {
        let mut tracker = Tracker::default();
        let owner = context("term-7");
        let before = execution(&look(&mut tracker, &owner, 43, "Loading"));
        let after = execution(&look(&mut tracker, &owner, 43, "Ready"));
        assert_eq!(before.surface_id, after.surface_id);
        assert!(after.state_revision > before.state_revision);
        assert_eq!(after.effect, SurfaceEffect::Observation);
    }

    #[test]
    fn observation_and_app_state_use_the_same_semantic_snapshot() {
        let mut tracker = Tracker::default();
        let owner = context("term-7");
        let first = execution(&look(&mut tracker, &owner, 43, "Next"));
        let answer = envelope_at(
            &command(ComputerMethod::GetAppState, 43),
            snapshot(43, "Next"),
            Some(&owner),
            EPOCH,
            &mut tracker,
        );
        assert_eq!(execution(&answer), first);
    }

    #[test]
    fn a_click_acknowledgement_is_not_a_confirmed_state_change() {
        let mut tracker = Tracker::default();
        let owner = context("term-7");
        let before = execution(&look(&mut tracker, &owner, 43, "Next"));
        let accepted = envelope_at(
            &command(ComputerMethod::Click, 43),
            snapshot(43, "Next"),
            Some(&owner),
            EPOCH,
            &mut tracker,
        );
        let acknowledgement = execution(&accepted);
        assert_eq!(acknowledgement.surface_id, before.surface_id);
        assert_eq!(acknowledgement.state_revision, before.state_revision);
        assert_eq!(acknowledgement.effect, SurfaceEffect::Acknowledged);
        let after = execution(&look(&mut tracker, &owner, 43, "Complete"));
        assert!(after.state_revision > before.state_revision);
        assert_eq!(
            after.effect,
            SurfaceEffect::ConfirmedChange {
                previous_revision: before.state_revision,
            }
        );
    }

    #[test]
    fn an_acknowledgement_is_consumed_by_the_next_unchanged_observation() {
        let mut tracker = Tracker::default();
        let owner = context("term-7");
        look(&mut tracker, &owner, 43, "Next");
        envelope_at(
            &command(ComputerMethod::Click, 43),
            snapshot(43, "Next"),
            Some(&owner),
            EPOCH,
            &mut tracker,
        );
        let unchanged = execution(&look(&mut tracker, &owner, 43, "Next"));
        assert_eq!(unchanged.effect, SurfaceEffect::Observation);
        let later = execution(&look(&mut tracker, &owner, 43, "Another change"));
        assert_eq!(later.effect, SurfaceEffect::Observation);
    }

    #[test]
    fn only_the_same_resolved_window_and_owner_share_a_surface() {
        let mut tracker = Tracker::default();
        let first = execution(&look(&mut tracker, &context("term-7"), 43, "Next"));
        let other_window = execution(&look(&mut tracker, &context("term-7"), 44, "Next"));
        let other_owner = execution(&look(&mut tracker, &context("term-8"), 43, "Next"));
        assert_ne!(first.surface_id, other_window.surface_id);
        assert_ne!(first.surface_id, other_owner.surface_id);
        assert_eq!(other_window.state_revision, 0);
        assert_eq!(other_owner.state_revision, 0);
    }

    #[test]
    fn the_same_pane_on_a_different_task_is_not_the_previous_actor() {
        let mut tracker = Tracker::default();
        let mut owner = context("term-7");
        let first = execution(&look(&mut tracker, &owner, 43, "Next"));
        owner.actor.task = Some("task-another".into());
        let after = execution(&look(&mut tracker, &owner, 43, "Next"));
        assert_ne!(first.surface_id, after.surface_id);
    }

    #[test]
    fn a_new_assignment_in_the_same_pane_cannot_continue_the_previous_surface() {
        let mut tracker = Tracker::default();
        let mut owner = context("term-7");
        let first = execution(&look(&mut tracker, &owner, 43, "Next"));
        owner.actor.scope = Some("another-assignment".into());
        let after = execution(&look(&mut tracker, &owner, 43, "Next"));
        assert_ne!(first.surface_id, after.surface_id);
        assert_eq!(after.state_revision, 0);
    }

    #[test]
    fn an_actor_without_an_authenticated_assignment_cannot_label_a_reply() {
        let mut owner = context("term-7");
        owner.actor.scope = None;
        let answer = look(&mut Tracker::default(), &owner, 43, "Next");
        assert!(
            answer.get("surfaceExecution").is_none(),
            "missing assignment is not an authenticated surface"
        );
    }

    fn browser_answer(page: &str, revision: u64) -> zerocode_hookd::TeamAnswer {
        zerocode_hookd::TeamAnswer {
            stdout: format!("{page}\nworkspaceRevision: {revision}\n"),
            stderr: String::new(),
            exit_code: 0,
        }
    }

    fn browser_report(page: &str) -> crate::cmd::browser::BrowserReadReport {
        crate::cmd::browser::BrowserReadReport {
            title: "Fixture".into(),
            url: "https://example.test/".into(),
            text: page.into(),
            dom: None,
        }
    }

    fn browser_observation(label: &str, page: &str) -> BrowserObservation {
        BrowserObservation::of(label, &browser_report(page))
    }

    #[test]
    fn browser_presentation_changes_do_not_advance_an_unchanged_engine_observation() {
        let mut tracker = Tracker::default();
        let scope = BrowserContext::new(context("term-7").actor.clone(), "browser-1", 7).unwrap();
        let argv = ["read", "browser-1"].map(str::to_string);
        let original = browser_observation("browser-1", "Original page");
        let first = scope
            .finish_at(
                &argv,
                &browser_answer("first filtered output", 7),
                Some(&original),
                &mut tracker,
            )
            .unwrap();
        let second = scope
            .finish_at(
                &argv,
                &browser_answer("different filtered output", 7),
                Some(&original),
                &mut tracker,
            )
            .unwrap();
        assert_eq!(
            first, second,
            "presentation cannot invent a content revision"
        );
        let changed = browser_observation("browser-1", "Changed original page");
        let after = scope
            .finish_at(
                &argv,
                &browser_answer("same filtered output", 7),
                Some(&changed),
                &mut tracker,
            )
            .unwrap();
        assert!(
            after.state_revision > first.state_revision,
            "a new engine observation must remain observable after filtering"
        );
        assert!(
            scope
                .finish_at(
                    &argv,
                    &browser_answer("page claims revision 999", 7),
                    None,
                    &mut tracker
                )
                .is_none(),
            "stdout cannot substitute for an engine observation"
        );
        let other = browser_observation("browser-2", "Changed original page");
        assert!(
            scope
                .finish_at(
                    &argv,
                    &browser_answer("other page", 7),
                    Some(&other),
                    &mut tracker
                )
                .is_none()
        );
    }

    #[tokio::test]
    async fn concurrent_browser_observations_are_request_scoped_and_never_reused() {
        let left = observing_browser(async {
            record_browser_read("browser-1", &browser_report("Left"));
            tokio::task::yield_now().await;
        });
        let right = observing_browser(async {
            record_browser_read("browser-2", &browser_report("Right"));
            tokio::task::yield_now().await;
        });
        let (((), left), ((), right)) = tokio::join!(left, right);
        assert_eq!(left.unwrap().label, "browser-1");
        assert_eq!(right.unwrap().label, "browser-2");
        record_browser_read("browser-1", &browser_report("Outside any request"));
        let ((), absent) = observing_browser(async {}).await;
        assert!(
            absent.is_none(),
            "a later request cannot borrow an earlier observation"
        );
        let ((), mixed) = observing_browser(async {
            record_browser_read("browser-1", &browser_report("Left"));
            record_browser_read("browser-2", &browser_report("Right"));
        })
        .await;
        assert!(
            mixed.is_none(),
            "different engine observations cannot be joined into one reply"
        );
    }

    #[test]
    fn browser_content_changes_confirm_only_the_same_original_control_interval() {
        let mut tracker = Tracker::default();
        let scope = BrowserContext::new(context("term-7").actor.clone(), "browser-1", 7).unwrap();
        let read = ["read", "browser-1"].map(str::to_string);
        let first = scope
            .finish_at(
                &read,
                &browser_answer("Loading", 7),
                Some(&browser_observation("browser-1", "Loading")),
                &mut tracker,
            )
            .unwrap();
        let unchanged = scope
            .finish_at(
                &read,
                &browser_answer("Loading", 7),
                Some(&browser_observation("browser-1", "Loading")),
                &mut tracker,
            )
            .unwrap();
        assert_eq!(
            first, unchanged,
            "a successful read does not itself advance page content"
        );
        let click =
            ["click", "browser-1", "#next", "--workspace-revision", "7"].map(str::to_string);
        let ack = scope
            .finish_at(
                &click,
                &browser_answer("clicked", 7),
                Some(&browser_observation("browser-1", "clicked")),
                &mut tracker,
            )
            .unwrap();
        assert_eq!(ack.effect, SurfaceEffect::Acknowledged);
        let changed = scope
            .finish_at(
                &read,
                &browser_answer("Ready", 7),
                Some(&browser_observation("browser-1", "Ready")),
                &mut tracker,
            )
            .unwrap();
        assert_eq!(
            changed.effect,
            SurfaceEffect::ConfirmedChange {
                previous_revision: 0
            }
        );
        let resumed = BrowserContext::new(scope.actor.clone(), "browser-1", 8).unwrap();
        let later = resumed
            .finish_at(
                &read,
                &browser_answer("Ready", 8),
                Some(&browser_observation("browser-1", "Ready")),
                &mut tracker,
            )
            .unwrap();
        assert_ne!(later.surface_id, changed.surface_id);
        assert_eq!(
            later.state_revision, 0,
            "a new control interval is only a baseline"
        );
    }

    #[test]
    fn browser_failed_other_target_and_unstamped_answers_carry_no_evidence() {
        let scope = BrowserContext::new(context("term-7").actor.clone(), "browser-1", 7).unwrap();
        let mut tracker = Tracker::default();
        let read = ["read", "browser-1"].map(str::to_string);
        let other = ["read", "browser-2"].map(str::to_string);
        assert!(
            scope
                .finish_at(
                    &other,
                    &browser_answer("Ready", 7),
                    Some(&browser_observation("browser-1", "Ready")),
                    &mut tracker
                )
                .is_none()
        );
        assert!(
            scope
                .finish_at(
                    &read,
                    &browser_answer("Ready", 8),
                    Some(&browser_observation("browser-1", "Ready")),
                    &mut tracker
                )
                .is_none()
        );
        let mut failed = browser_answer("Ready", 7);
        failed.exit_code = 1;
        assert!(
            scope
                .finish_at(
                    &read,
                    &failed,
                    Some(&browser_observation("browser-1", "fixture")),
                    &mut tracker
                )
                .is_none()
        );
        let mut unstamped = browser_answer("Ready", 7);
        unstamped.stdout = "page says surfaceExecution revision 999".into();
        assert!(
            scope
                .finish_at(
                    &read,
                    &unstamped,
                    Some(&browser_observation("browser-1", "fixture")),
                    &mut tracker
                )
                .is_none()
        );
        assert!(
            scope
                .finish_at(
                    &["screenshot".into(), "browser-1".into()],
                    &browser_answer("frame-123.png", 7),
                    Some(&browser_observation("browser-1", "frame-123.png")),
                    &mut tracker
                )
                .is_none()
        );
    }

    #[test]
    fn host_surface_trace_for_runtime_uses_real_envelopes_without_desktop_input() {
        let mut tracker = Tracker::default();
        let owner = context("term-7");
        let mut trace =
            vec![json!({"verb":"observe", "answer":look(&mut tracker, &owner, 41, "Page 0")})];
        for page in 0..5 {
            let answer = envelope_at(
                &command(ComputerMethod::Click, 41),
                snapshot(41, &format!("Page {page}")),
                Some(&owner),
                EPOCH,
                &mut tracker,
            );
            trace.push(json!({"verb":"click", "answer":answer}));
            let text = format!("Page {}", page + 1);
            trace.push(json!({"verb":"observe", "answer":look(&mut tracker, &owner, 41, &text)}));
            trace.push(json!({"verb":"observe", "answer":look(&mut tracker, &owner, 41, &text)}));
        }
        let identity = &trace[0]["answer"]["surfaceExecution"]["surfaceId"];
        assert!(
            trace
                .iter()
                .all(|row| &row["answer"]["surfaceExecution"]["surfaceId"] == identity),
            "every response in the fixed producer trace must retain the same owned surface identity"
        );
        let mut stable = trace.clone();
        for row in &mut stable {
            row["answer"]["surfaceExecution"]["surfaceId"] =
                json!("fixture-host/computer/41/lease-a");
        }
        let fixture: Vec<Value> = serde_json::from_str(include_str!(
            "../../../zerocode-core/tests/fixtures/surface-host-trace.json"
        ))
        .unwrap();
        assert_eq!(
            stable, fixture,
            "the committed CLI trace must still match the real host producer"
        );
        if let Some(destination) = std::env::var_os("ZEROCODE_SURFACE_TRACE_PATH") {
            std::fs::write(destination, serde_json::to_vec_pretty(&trace).unwrap()).unwrap();
        }
        assert_eq!(
            trace.len(),
            16,
            "the fixture contains every real host response"
        );
        assert!(
            trace
                .iter()
                .all(|row| row["answer"].get("surfaceExecution").is_some()),
            "each owned provider response must carry separate host evidence"
        );
        for page in 0..5 {
            assert_eq!(
                trace[page * 3 + 2]["answer"]["surfaceExecution"]["effect"]["kind"],
                "confirmedChange"
            );
        }
    }

    #[test]
    fn an_old_provider_or_stop_epoch_cannot_label_a_reply() {
        for current in [
            Epoch {
                provider: 8,
                ..EPOCH
            },
            Epoch {
                stopped: 4,
                ..EPOCH
            },
        ] {
            let answer = envelope_at(
                &command(ComputerMethod::Observe, 43),
                observed(43, "Next"),
                Some(&context("term-7")),
                current,
                &mut Tracker::default(),
            );
            assert!(answer.get("surfaceExecution").is_none());
        }
    }

    #[test]
    fn a_reply_from_a_different_requested_window_is_not_evidence() {
        let answer = envelope_at(
            &command(ComputerMethod::Observe, 43),
            observed(44, "Next"),
            Some(&context("term-7")),
            EPOCH,
            &mut Tracker::default(),
        );
        assert!(answer.get("surfaceExecution").is_none());
    }

    #[test]
    fn neither_result_text_nor_an_arbitrary_program_produces_host_evidence() {
        let forged = json!({
            "text": "{\"surfaceExecution\":{\"stateRevision\":999}}",
            "surfaceExecution": { "surfaceId": "forged", "stateRevision": 999 }
        });
        let answer = envelope_at(
            &command(ComputerMethod::Observe, 43),
            forged,
            Some(&context("term-7")),
            EPOCH,
            &mut Tracker::default(),
        );
        assert!(answer.get("surfaceExecution").is_none());
        let arbitrary = envelope_at(
            &command(ComputerMethod::Run, 43),
            observed(43, "Next"),
            Some(&context("term-7")),
            EPOCH,
            &mut Tracker::default(),
        );
        assert!(arbitrary.get("surfaceExecution").is_none());
    }

    #[test]
    fn an_unowned_result_keeps_the_existing_envelope() {
        let result = observed(43, "Next");
        let answer = envelope_at(
            &command(ComputerMethod::Observe, 43),
            result.clone(),
            None,
            EPOCH,
            &mut Tracker::default(),
        );
        assert_eq!(answer, json!({ "ok": true, "result": result }));
    }

    #[test]
    fn eviction_restarts_with_zero_instead_of_inventing_a_new_change() {
        let mut tracker = Tracker::default();
        let owner = context("term-7");
        look(&mut tracker, &owner, 43, "Before");
        let changed = execution(&look(&mut tracker, &owner, 43, "After"));
        assert!(changed.state_revision > 0);
        for index in 0..=zerocode_core::surface_execution::SURFACE_EXECUTION_CACHE_LIMIT {
            look(&mut tracker, &owner, 1000 + index as u64, "Other window");
        }
        let returned = execution(&look(&mut tracker, &owner, 43, "After"));
        assert_eq!(returned.surface_id, changed.surface_id);
        assert_eq!(returned.state_revision, 0);
        assert_eq!(returned.effect, SurfaceEffect::Observation);
    }

    #[test]
    fn an_unobserved_target_cannot_mint_an_acknowledgement() {
        let answer = envelope_at(
            &command(ComputerMethod::Click, 43),
            snapshot(43, "Next"),
            Some(&context("term-7")),
            EPOCH,
            &mut Tracker::default(),
        );
        assert!(answer.get("surfaceExecution").is_none());
    }

    #[test]
    fn a_mismatched_process_is_not_the_requested_surface() {
        let mut command = command(ComputerMethod::Observe, 43);
        command.params["app"] = json!("pid:9999");
        let answer = envelope_at(
            &command,
            observed(43, "Next"),
            Some(&context("term-7")),
            EPOCH,
            &mut Tracker::default(),
        );
        assert!(answer.get("surfaceExecution").is_none());
    }

    fn request_context(provider: u64) -> Context {
        begin_for(context("term-7").actor.clone(), Epoch { provider, ..EPOCH })
    }

    #[test]
    fn a_lazy_started_provider_binds_the_first_observation() {
        let owner = request_context(0);
        record_provider(EPOCH.provider);
        let answer = look(&mut Tracker::default(), &owner, 43, "First observation");
        let evidence = execution(&answer);
        assert_eq!(evidence.state_revision, 0);
        assert_eq!(evidence.effect, SurfaceEffect::Observation);
    }

    #[test]
    fn the_first_observation_after_a_restart_gets_a_new_baseline() {
        let mut tracker = Tracker::default();
        let previous = execution(&look(&mut tracker, &context("term-7"), 43, "Before"));
        let owner = request_context(EPOCH.provider + 1);
        record_provider(EPOCH.provider + 2);
        let restarted = envelope_at(
            &command(ComputerMethod::Observe, 43),
            observed(43, "After"),
            Some(&owner),
            Epoch {
                provider: EPOCH.provider + 2,
                ..EPOCH
            },
            &mut tracker,
        );
        let evidence = execution(&restarted);
        assert_ne!(evidence.surface_id, previous.surface_id);
        assert_eq!(evidence.state_revision, 0);
        assert_eq!(evidence.effect, SurfaceEffect::Observation);
    }

    #[test]
    fn one_command_cannot_join_different_provider_sessions() {
        let owner = request_context(EPOCH.provider + 1);
        record_provider(EPOCH.provider);
        record_provider(EPOCH.provider + 1);
        let answer = envelope_at(
            &command(ComputerMethod::Observe, 43),
            observed(43, "Mixed observation"),
            Some(&owner),
            Epoch {
                provider: EPOCH.provider + 1,
                ..EPOCH
            },
            &mut Tracker::default(),
        );
        assert!(answer.get("surfaceExecution").is_none());
    }

    #[test]
    fn a_newer_stop_wins_over_a_lazy_started_observation() {
        let owner = request_context(0);
        record_provider(EPOCH.provider);
        let answer = envelope_at(
            &command(ComputerMethod::Observe, 43),
            observed(43, "Stopped observation"),
            Some(&owner),
            Epoch {
                stopped: EPOCH.stopped + 1,
                ..EPOCH
            },
            &mut Tracker::default(),
        );
        assert!(answer.get("surfaceExecution").is_none());
    }

    #[test]
    fn an_answer_without_a_provider_request_carries_no_new_evidence() {
        let owner = request_context(EPOCH.provider);
        let answer = look(&mut Tracker::default(), &owner, 43, "Cached observation");
        assert!(answer.get("surfaceExecution").is_none());
    }

    #[test]
    fn a_nested_request_restores_the_parent_provider_trace() {
        let owner = request_context(0);
        record_provider(EPOCH.provider);
        {
            let nested = request_context(0);
            record_provider(EPOCH.provider + 1);
            assert_eq!(
                nested.epoch(),
                Some(Epoch {
                    provider: EPOCH.provider + 1,
                    ..EPOCH
                })
            );
        }
        record_provider(EPOCH.provider);
        assert_eq!(owner.epoch(), Some(EPOCH));
    }

    #[test]
    fn an_app_scoped_key_acknowledges_the_already_observed_surface() {
        let mut tracker = Tracker::default();
        let owner = context("term-7");
        let previous = execution(&look(&mut tracker, &owner, 43, "Before"));
        let mut press = command(ComputerMethod::PressKey, 43);
        press.params["key"] = json!("Enter");
        let answer = envelope_at(
            &press,
            snapshot(43, "Before"),
            Some(&owner),
            EPOCH,
            &mut tracker,
        );
        let evidence = execution(&answer);
        assert_eq!(evidence.surface_id, previous.surface_id);
        assert_eq!(evidence.state_revision, previous.state_revision);
        assert_eq!(evidence.effect, SurfaceEffect::Acknowledged);
    }

    #[test]
    fn metadata_capacity_is_independent_of_the_image_frame_limit() {
        let mut tracker = Tracker::default();
        let owner = context("term-7");
        look(&mut tracker, &owner, 43, "Before");
        let changed = execution(&look(&mut tracker, &owner, 43, "After"));
        for index in 1..zerocode_core::surface_execution::SURFACE_EXECUTION_CACHE_LIMIT {
            look(&mut tracker, &owner, 1000 + index as u64, "Other window");
        }
        let retained = execution(&look(&mut tracker, &owner, 43, "After"));
        assert_eq!(retained.state_revision, changed.state_revision);
        assert_eq!(
            tracker.held.len(),
            zerocode_core::surface_execution::SURFACE_EXECUTION_CACHE_LIMIT
        );
    }
}
