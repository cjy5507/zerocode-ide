//! Public, bounded process facts. The socket only clones the installed value;
//! selection/catalog work happens at the frontend's change boundary.

use serde::Serialize;
use serde_json::{json, Value};
use runtime::subagent_panes::{ModeChoice, SubagentMode, TeamEvidence};

use crate::effort::Effort;
use crate::session::plain_session::StatusSnapshot;

pub const MAX_BYTES: usize = 32 * 1024;

/// `support.steer.scope` of a root session: words go into the turn that is
/// running, and nowhere else.
pub const STEER_SCOPE_ROOT: &str = "active-root-turn";
/// `support.steer.scope` of a teammate pane (t-2513 §2.2): the running turn,
/// or — while the pane is idle between turns — its next one.
pub const STEER_SCOPE_TEAMMATE: &str = "active-root-turn | idle-next-turn";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Requested {
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// The exact launch contract's receipt (`crate::launch_contract`): what was
/// asked, what was installed, and the verdict with its reason. `effective` is
/// null on a refusal; nothing ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Contract {
    pub version: u32,
    pub request_id: Option<String>,
    pub requested: Value,
    pub effective: Value,
    pub verdict: &'static str,
    pub reason: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Mode {
    pub frontend: Option<&'static str>,
    pub requested: Option<&'static str>,
    pub effective: Option<&'static str>,
    pub available: Option<bool>,
    pub reason: Option<&'static str>,
}

impl Mode {
    #[must_use]
    pub fn observe(choice: ModeChoice, team: &TeamEvidence, interactive: bool, no_spawn: bool) -> Self {
        let nested = runtime::subagent_panes::nested();
        let effective = runtime::subagent_panes::decide(choice, team, interactive, nested);
        let reason = if no_spawn {
            "no-spawn"
        } else if nested {
            "nested-child"
        } else {
            match choice {
                ModeChoice::Inline => "explicit-inline",
                ModeChoice::Panes if !team.can_split() => "missing-team-binding",
                ModeChoice::Panes => "explicit-panes",
                ModeChoice::Auto if !interactive => "plain-output",
                ModeChoice::Auto if team.can_split() => "team-evidence-complete",
                ModeChoice::Auto => "missing-team-binding",
            }
        };
        Self {
            frontend: Some(if interactive { "tui" } else { "plain" }),
            requested: Some(choice.key()),
            effective: Some(match effective { SubagentMode::Panes => "panes", SubagentMode::Inline => "inline" }),
            available: Some(!no_spawn && (effective == SubagentMode::Inline || team.can_split())),
            reason: Some(reason),
        }
    }

    #[must_use]
    pub fn from_environment(no_spawn: bool) -> Self {
        let choice = runtime::subagent_panes::choice_from(
            std::env::var(runtime::subagent_panes::MODE_ENV).ok().as_deref(),
            &runtime::default_config_home().join("settings.json"),
        );
        Self::observe(choice, &TeamEvidence::from_env(), runtime::subagent_panes::on_screen(), no_spawn)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    #[serde(skip)]
    pub catalog_generation: u64,
    #[serde(skip)]
    pub no_spawn: bool,
    pub protocol: Value,
    pub revision: u64,
    pub process: Value,
    pub identity: Value,
    pub launch: Value,
    pub support: Value,
    pub mode: Mode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<Value>,
}

impl Snapshot {
    /// An opened endpoint can be queried before the frontend installs its
    /// selection. Unknown values remain null; revision zero is not acceptance.
    #[must_use]
    pub fn pending(channel_session_id: &str) -> Self {
        Self {
            catalog_generation: 0,
            no_spawn: false,
            protocol: json!({"name":"zo-events", "major":1, "minor":0}),
            revision: 0,
            process: json!({
                "instance_id": format!("{:032x}", rand::random::<u128>()),
                "pid": std::process::id(),
                "executable": std::env::current_exe().ok().and_then(|path| safe_path(&path.to_string_lossy())),
                "build": crate::build_info::current(),
            }),
            identity: json!({"channel_session_id": public_token(channel_session_id),
                "session_id": public_token(channel_session_id), "parent_session_id": null,
                "agent_id": null, "pane": null,
                // t-2513 §2.5: which executor this process is, and the parent
                // registry it opened. `root` until a brief says otherwise.
                "execution": "root", "registry_locator": null}),
            launch: json!({"request_id":null,"policy":"legacy", "requested":null,
                "resolved":null,"effective":null,"origins":null,
                "validation":{"state":"unknown","reason":"selection-pending"},"catalog":null,
                // t-2513 §2.1: a teammate says whether it runs the harness its
                // parent carried, re-derived one (v1 brief), or a stale one.
                "harness": null, "harness_digest": null}),
            support: json!({
                "roster":{"supported":true,"transport":"subagents-snapshot","method":null,"registry_ordering":true},
                "steer":{"supported":true,"method":"session.steer","scope":STEER_SCOPE_ROOT},
                "resume":{"supported":true,"transport":"cli","selector":"--resume","continuation":"composer"},
                "subagent_panes":{"supported":true},
                // Implemented ability on this binary (t-2513): a child is
                // steered over its own channel with a receipt, and resumed in
                // the mode its manifest names.
                "subagent_steer":{"supported":true,"transport":"child-channel","method":"session.steer",
                    "receipts":["consumed","queued","rejected"]},
                "subagent_resume":{"supported":true,"transport":"manifest-execution",
                    "pane":"session.steer | --resume-transcript","inline":"thread"},
                // Implemented ability, whatever this launch asked for: the
                // window reads it to know the guard exists on this binary.
                "exact_launch":{"supported":true,"version":crate::launch_contract::VERSION}
            }),
            mode: Mode { frontend:None, requested:None, effective:None, available:None, reason:None },
            current: None,
        }
    }

    /// A launch the contract refused before a session existed: the receipt
    /// carries the verdict and the safe selection fields, nothing effective,
    /// and the channel's own id as the session. Mode stays unobserved.
    pub fn install_refusal(&mut self, requested: &Requested, contract: &Contract) {
        self.catalog_generation = crate::model_wire_env::generation();
        self.launch = json!({
            "request_id": contract.request_id.as_deref().and_then(public_token),
            "policy": "exact",
            "requested":{"agent":"zo", "model":requested.model.as_deref().and_then(public_token),
                "effort":requested.effort.as_deref().and_then(public_token)},
            "resolved": null, "effective": null, "origins": null,
            "validation":{"state": contract.verdict, "reason": contract.reason},
            "catalog": null,
            "contract": contract,
        });
    }

    pub fn install(&mut self, requested: &Requested, status: &StatusSnapshot, origins: (&str, &str), mode: Mode, brief: Option<&runtime::subagent_panes::Brief>, contract: Option<&Contract>) {
        self.identity["session_id"] = json!(public_token(&status.session_id));
        if let Some(brief) = brief {
            self.identity["parent_session_id"] = json!(brief.parent_session.as_deref().and_then(public_token));
            self.identity["agent_id"] = json!(public_token(&brief.agent_id));
            // A teammate pane (t-2513 §2.5): the executor, the parent
            // registry it opened, and where its harness came from. The door
            // `session.steer` widens to the next turn while it is idle.
            self.identity["execution"] = json!("pane");
            self.identity["registry_locator"] = json!(brief
                .registry_locator
                .as_deref()
                .and_then(|path| safe_path(&path.to_string_lossy())));
            self.support["steer"]["scope"] = json!(STEER_SCOPE_TEAMMATE);
        }
        // Host coordinates are observations, never authority over a pane.
        self.identity["pane"] = std::env::var("ZEROCODE_PANE_KEY").ok()
            .and_then(|id| public_token(&id)).map_or(Value::Null, |id| json!({"namespace":"zerocode","id":id}));
        self.catalog_generation = crate::model_wire_env::generation();
        let model_origin = if origins.0 == "launch" {
            match requested.model.as_deref() {
                None => "default", Some(raw) if raw == status.model => "explicit", Some(_) => "explicit-alias",
            }
        } else { origins.0 };
        let effort_origin = if origins.1 == "launch" { "explicit" } else { origins.1 };
        let effective = selection(&status.model, status.effort);
        let mut resolved = effective.clone();
        if let Some(object) = resolved.as_object_mut() {
            object.remove("wire_model"); object.remove("wire_effort"); object.remove("speed_tier");
        }
        let (policy, request_id, validation) = match contract {
            Some(contract) => ("exact", contract.request_id.as_deref().and_then(public_token),
                json!({"state": contract.verdict, "reason": contract.reason})),
            None => ("legacy", None, json!({"state":"unvalidated","reason":"legacy-policy"})),
        };
        self.launch = json!({
            "request_id": request_id, "policy": policy,
            "requested":{"agent":"zo", "model":requested.model.as_deref().and_then(public_token),
                "effort":requested.effort.as_deref().and_then(public_token)},
            "resolved":resolved, "effective":effective,
            "origins":{"model":model_origin,"effort":effort_origin},
            "validation": validation,
            "catalog":catalog(&status.model),
            "contract": contract,
            // t-2513 §2.1: where a teammate's harness came from. A root
            // session has no harness to speak of — null, like a v1 receipt.
            "harness": brief.map(|brief| brief.harness_origin().as_str()),
            "harness_digest": brief.and_then(|brief| brief.harness.as_ref())
                .map(runtime::subagent_panes::ResolvedHarness::digest),
        });
        self.mode = mode;
    }

    pub fn refresh(&mut self, status: &StatusSnapshot, mode: Mode) {
        if self.launch["effective"].is_null() { return; }
        let previous = self.current.as_ref().map_or(&self.launch["effective"], |current| &current["effective"]);
        let generation = crate::model_wire_env::generation();
        if self.identity["session_id"].as_str() == Some(status.session_id.as_str())
            && previous["model"].as_str() == Some(status.model.as_str())
            && previous["effort"].as_str() == status.effort
            && self.mode == mode && self.catalog_generation == generation { return; }
        self.catalog_generation = generation;
        self.identity["session_id"] = json!(public_token(&status.session_id));
        let effective = selection(&status.model, status.effort);
        let facts = catalog(&status.model);
        let (previous, previous_catalog) = self.current.as_ref().map_or(
            (&self.launch["effective"], &self.launch["catalog"]), |current| (&current["effective"], &current["catalog"]));
        if effective != *previous || facts != *previous_catalog {
            self.current = Some(json!({"effective":effective,"catalog":facts,"origin":"runtime-change"}));
        }
        self.mode = mode;
    }
}

/// The public name of a provider kind — the one spelling the receipt, the
/// contract and the window's summary share.
#[must_use]
pub fn provider_name(provider: api::ProviderKind) -> &'static str {
    match provider {
        api::ProviderKind::Anthropic => "anthropic", api::ProviderKind::OpenAi => "openai",
        api::ProviderKind::Google => "google", api::ProviderKind::Xai => "xai", api::ProviderKind::Ollama => "ollama",
    }
}

/// The digest of the selected row's public facts, as `catalog.revision`
/// reports it — what a launch request may pin.
#[must_use]
pub(crate) fn catalog_revision(model: &str) -> Option<String> {
    catalog(model)["revision"].as_str().map(str::to_string)
}

fn selection(model: &str, effort: Option<&str>) -> Value {
    let level = effort.and_then(Effort::from_token).filter(|effort| *effort != Effort::Smart).and_then(Effort::level);
    let provider = api::detect_provider_kind(model);
    let provider_name = provider_name(provider);
    let wire_effort = level.and_then(|level| match provider {
        api::ProviderKind::Anthropic => Some(level.anthropic_for_model(model).anthropic()),
        api::ProviderKind::OpenAi => Some(level.gpt_for_model(model)),
        // These providers do not expose a single comparable effort enum.
        api::ProviderKind::Google | api::ProviderKind::Xai | api::ProviderKind::Ollama => None,
    });
    let wire_model = level.and_then(|level| api::wire_model_for_effort(model, level))
        .unwrap_or_else(|| api::wire_model_id(model));
    json!({"agent":"zo","provider":provider_name,"model":public_token(model),
        "effort":effort.and_then(public_token), "wire_model":public_token(&wire_model),
        "wire_effort":wire_effort,"speed_tier":api::openai_fast_tier_enabled(model).then_some("priority")})
}

fn catalog(model: &str) -> Value {
    let facts = json!({"id":public_token(model),
        "family":api::model_family(model).as_deref().and_then(public_token),
        "display_name":public_label(&api::model_display_name(model)),
        "effort_levels":api::declared_effort_levels(model),
        "speed_tiers":api::declared_speed_tiers(model).iter().filter_map(|v| public_token(v)).take(16).collect::<Vec<_>>(),
        "capabilities":api::declared_capabilities(model).iter().filter_map(|v| public_token(v)).take(32).collect::<Vec<_>>(),
        "wire":api::wire_model_for_effort(model, api::EffortLevel::High).as_deref().and_then(public_token),
    });
    let (source, provenance) = crate::model_wire_env::selection_provenance(model);
    let freshness = if source == "discovered" {
        runtime::model_discovery::current().map(|cache| if cache.stale(runtime::model_discovery::now_secs(),runtime::model_discovery::ttl_secs()) { "stale" } else { "fresh" })
    } else { None };
    let public = json!({"source":source,"freshness":freshness,"facts_provenance":provenance,"model":facts});
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in public.to_string().bytes() { hash = (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3); }
    let mut result = public;
    result["revision"] = json!(format!("{hash:016x}"));
    result
}

/// No arbitrary env, free-form errors, catalog notes, URLs or credential
/// paths enter the public snapshot. Selection tokens are bounded identifiers.
#[must_use]
pub fn public_token(value: &str) -> Option<String> {
    (!value.is_empty() && value.len() <= 256 && value.chars().all(|c| c.is_ascii_alphanumeric() || "._-:/@[]+%".contains(c)) && !value.contains("://"))
        .then(|| value.to_string())
}

fn public_label(value: &str) -> Option<String> {
    (value.len() <= 256 && !value.chars().any(char::is_control) && !value.contains("://"))
        .then(|| value.to_string())
}

#[must_use]
pub fn safe_path(value: &str) -> Option<String> {
    (value.len() <= 4096 && !value.chars().any(char::is_control)).then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_distinguishes_requested_panes_from_available_plumbing() {
        let complete = TeamEvidence { tmux:Some("tmux".into()), pane:Some("%1".into()), team_id:Some("team".into()), token:true };
        let absent = TeamEvidence::default();
        for (choice, team, tui, disabled, expected, available, reason) in [
            (ModeChoice::Auto,&complete,true,false,"panes",true,"team-evidence-complete"),
            (ModeChoice::Auto,&complete,false,false,"inline",true,"plain-output"),
            (ModeChoice::Auto,&absent,true,false,"inline",true,"missing-team-binding"),
            (ModeChoice::Panes,&absent,true,false,"panes",false,"missing-team-binding"),
            (ModeChoice::Panes,&complete,true,false,"panes",true,"explicit-panes"),
            (ModeChoice::Inline,&complete,true,false,"inline",true,"explicit-inline"),
            (ModeChoice::Auto,&complete,true,true,"panes",false,"no-spawn"),
        ] {
            let actual = Mode::observe(choice,team,tui,disabled);
            assert_eq!((actual.effective,actual.available,actual.reason),(Some(expected),Some(available),Some(reason)));
        }
    }

    /// A teammate's snapshot says which executor it is, whose registry it
    /// opened, where its harness came from, and that its steer door is wider
    /// (t-2513 §2.5); a root session says none of that.
    #[test]
    fn a_teammate_snapshot_names_its_execution_registry_and_harness_origin() {
        use runtime::subagent_panes::{Brief, ResolvedHarness};
        let status = StatusSnapshot {
            model: "claude-fable-5-1".to_string(),
            permission_mode: "read-only",
            effort: Some("high"),
            session_id: "child-session".to_string(),
            context_tokens: 0,
            goal: String::new(),
            autonomous: String::new(),
            loops: String::new(),
            autonomy: crate::autonomy::AutonomyStatus::default(),
        };
        let mode = Mode::observe(ModeChoice::Auto, &TeamEvidence::default(), true, true);
        let mut root = Snapshot::pending("root-endpoint");
        root.install(&Requested::default(), &status, ("launch", "default"), mode.clone(), None, None);
        assert_eq!(root.identity["execution"], "root");
        assert!(root.identity["registry_locator"].is_null());
        assert!(root.launch["harness"].is_null());
        assert_eq!(root.support["steer"]["scope"], STEER_SCOPE_ROOT);

        let harness = ResolvedHarness {
            system_prompt: vec!["You review.".to_string()],
            ..ResolvedHarness::default()
        };
        let brief = Brief {
            agent_id: "agent-1".to_string(),
            parent_session: Some("parent-session".to_string()),
            registry_locator: Some(std::path::PathBuf::from("/tmp/agents/registries/parent-session.json")),
            harness: Some(harness.clone()),
            ..Brief::default()
        };
        let mut child = Snapshot::pending("child-endpoint");
        child.install(&Requested::default(), &status, ("launch", "default"), mode.clone(), Some(&brief), None);
        assert_eq!(child.identity["execution"], "pane");
        assert_eq!(child.identity["parent_session_id"], "parent-session");
        assert_eq!(child.identity["agent_id"], "agent-1");
        assert_eq!(child.identity["registry_locator"], "/tmp/agents/registries/parent-session.json");
        assert_eq!(child.launch["harness"], "carried");
        assert_eq!(child.launch["harness_digest"], harness.digest());
        assert_eq!(child.support["steer"]["scope"], STEER_SCOPE_TEAMMATE);
        assert_eq!(child.support["subagent_steer"]["supported"], true);

        // A v1 brief (no harness) is said to be derived; a stale one, stale.
        let mut derived = Snapshot::pending("child-endpoint");
        derived.install(&Requested::default(), &status, ("launch", "default"), mode.clone(), Some(&Brief { harness: None, ..brief.clone() }), None);
        assert_eq!(derived.launch["harness"], "derived");
        assert!(derived.launch["harness_digest"].is_null());
        let mut stale = Snapshot::pending("child-endpoint");
        stale.install(&Requested::default(), &status, ("launch", "default"), mode, Some(&Brief { harness_stale: true, ..brief }), None);
        assert_eq!(stale.launch["harness"], "stale");
    }

    #[test]
    fn diagnostic_identifiers_are_bounded_and_do_not_accept_urls_or_controls() {
        for bad in ["https://canary/token", "model\nsecret", "model\u{1b}", ""] { assert!(public_token(bad).is_none()); }
        assert!(public_token(&"a".repeat(257)).is_none());
        assert_eq!(public_token("fixture-model-v2"),Some("fixture-model-v2".into()));
        assert!(safe_path("/tmp/zo\ncanary").is_none());
    }
}
