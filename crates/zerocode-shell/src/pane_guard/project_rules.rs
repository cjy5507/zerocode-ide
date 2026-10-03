//! Project rules share the pane's already-collected turn evidence. The hook
//! loop gathers facts; this worker performs file/network work after the hook.

use super::*;
use zerocode_core::jev::{
    PROJECT_RULES,
    project_rules::{self as rules, Facts, store},
};

#[derive(Debug, Clone)]
pub(super) struct Turn {
    pub id: String,
    pub at: u64,
    pub task: String,
    pub edited: Vec<String>,
    pub said: SaidAt,
    pub evidence: Vec<Evidence<String>>,
}

pub(super) fn check(wire: &Wire, pane: &Pane, turn: &Turn) {
    let scoped = pane
        .session
        .as_deref()
        .map(|session| wire.for_origin(&format!("{}/session", pane.agent.slug()), session));
    let wire = scoped.as_ref().unwrap_or(wire);
    let initial = wire.settings_root();
    let mode = PROJECT_RULES.mode_in(&initial);
    let asked_model = zerocode_core::jev::model_in(&initial);
    let root = pane.root();
    let consent = zerocode_core::jev::door::JevSettings::from_root(&initial);
    let Some(home) = wire.config_home() else {
        return;
    };
    if !matches!(
        store::begin_check(home, &root, &pane.owner(), turn.at, &turn.id),
        Ok(true)
    ) {
        return;
    }
    if !mode.asks()
        || !consent.enabled
        || !consent.consents(&zerocode_core::jev::door::resolved_path(&root))
    {
        return;
    }
    let Some(ledger) = systemone::project_ledger_of(wire, &PROJECT_RULES, &root) else {
        return;
    };
    let book = match store::load(home, &root) {
        Ok(Some(book)) => book,
        Ok(None) => return,
        Err(error) => {
            let row = serde_json::json!({"at":turn.at,"from":pane.agent.slug(),"turn":turn.id,
                "rubricVersion":PROJECT_RULES.rubric_version,"outcome":"rules_unavailable",
                "requests":0,"applied":false,"reason":error.to_string()});
            systemone::record_rows(
                &PROJECT_RULES,
                &ledger,
                &[row],
                crate::usage_runtime::epoch_ms_now(),
            );
            return;
        }
    };
    let Ok(definition) = book.identity() else {
        return;
    };
    let final_text = match &turn.said {
        SaidAt::Words(words) => words.clone(),
        SaidAt::Transcript(path) => {
            zerocode_core::transcript::last_assistant_words(path).unwrap_or_default()
        }
    };
    let mut outside = false;
    let edited = rules::scope_paths(
        &book,
        turn.edited
            .iter()
            .filter_map(|path| match store::edit_path(&root, path) {
                Ok(path) => Some(path),
                Err(rules::Invalid::Outside) => {
                    outside = true;
                    None
                }
                Err(_) => Some("..".into()),
            }),
    );
    let evidence = zerocode_core::jev::door::newest_within(
        &turn
            .evidence
            .iter()
            .map(|evidence| evidence.output.as_str())
            .collect::<Vec<_>>(),
        16 * 1024,
    );
    let claims = claim::scan(&turn.evidence, &final_text);
    let facts = Facts {
        checks_after_edits: None, // this hook envelope does not prove check/edit ordering
        no_contradicted_completion: Some(
            !claims
                .iter()
                .any(|claim| claim.code == CodeVerdict::Contradicted),
        ),
        worktree_edits: if outside {
            None
        } else if edited.is_empty() {
            Some(true)
        } else {
            let project = store::workspace_root(&root);
            zerocode_core::git_dir::owning_checkout_of(&project).map(|owner| owner != project)
        },
    };
    let mut readings = rules::deterministic(&book, &edited, facts);
    let mut row = serde_json::json!({"at":turn.at,"from":pane.agent.slug(),"turn":turn.id,
        "definition":definition,"rubricVersion":PROJECT_RULES.rubric_version,
        "outcome":"control","requests":0,"inputTokens":0,"elapsedMs":0,"redactedLines":0,
        "applied":false,"routeUse":mode.key()});
    if let Err(error) = store::verify_scope(&root, &book, &edited) {
        row["outcome"] = serde_json::json!("rules_unavailable");
        row["reason"] = serde_json::json!(error.to_string());
        systemone::record_rows(
            &PROJECT_RULES,
            &ledger,
            &[row],
            crate::usage_runtime::epoch_ms_now(),
        );
        return;
    }
    if let Some(mut body) = rules::request(&book, &edited, &turn.task, &evidence, &final_text) {
        let state = body["state"].take();
        let Value::Object(questions) = body["questions"].take() else {
            return;
        };
        let questions = questions.into_iter().collect::<BTreeMap<_, _>>();
        let ids = questions.keys().cloned().collect::<Vec<_>>();
        let (trip, answered) = put(wire, &PROJECT_RULES, &root, state, &questions, |answers| {
            ids.iter()
                .map(|id| rules::read(answers, id))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| SCHEMA.to_string())
        });
        row["outcome"] = serde_json::json!(trip.outcome);
        row["requests"] = serde_json::json!(trip.requests);
        row["elapsedMs"] = serde_json::json!(trip.elapsed_ms);
        row["inputTokens"] = serde_json::json!(trip.input_tokens);
        row["model"] = serde_json::json!(trip.model);
        row["redactedLines"] = serde_json::json!(trip.redacted_lines);
        row["cached"] = serde_json::json!(trip.cached);
        if let Some(answered) = answered {
            readings.extend(answered);
        }
    }
    let current = wire.settings_root();
    let consent = zerocode_core::jev::door::JevSettings::from_root(&current);
    let known_session = pane
        .session
        .as_deref()
        .is_some_and(|session| !session.is_empty());
    let can_deliver = known_session && hook_guard::turn_start_road(pane.agent).yes();
    if mode == JevMode::On && !can_deliver {
        row["withheld"] = serde_json::json!(if known_session {
            "no_context_road"
        } else {
            "session_unknown"
        });
        row["routeUse"] = serde_json::json!(JevMode::Shadow.key());
    }
    if mode == JevMode::On
        && can_deliver
        && PROJECT_RULES.mode_in(&current) == JevMode::On
        && zerocode_core::jev::model_in(&current) == asked_model
        && consent.enabled
        && consent.consents(&zerocode_core::jev::door::resolved_path(&root))
    {
        let context = format!("{}\n{}\n{}", turn.task, evidence, final_text);
        if let Ok(Some(advice)) =
            rules::advice(&book, &readings, &context, &edited, turn.at, &turn.id)
        {
            match store::offer(home, &root, &pane.owner(), &advice) {
                Ok(true) => {
                    row["queued"] = serde_json::json!(true);
                    row["deliveryKey"] =
                        serde_json::json!(rules::delivery_key(&advice, &pane.owner()));
                }
                Ok(false) => {}
                Err(error) => row["withheld"] = serde_json::json!(error.to_string()),
            }
        }
    }
    row["readings"] = serde_json::json!(readings);
    systemone::record_rows(
        &PROJECT_RULES,
        &ledger,
        &[row],
        crate::usage_runtime::epoch_ms_now(),
    );
}

/// Preparing context consumes nothing. Only the client's receipt commits it.
pub(super) fn brief_line(ask: &zerocode_hookd::TurnBriefAsk, room: usize) -> Option<String> {
    if !STANDING.asking().project_rules {
        return None;
    }
    brief_in(&GUARDS, &Wire::of_this_machine(), ask, room)
}

fn brief_in(
    guards: &Mutex<Guards>,
    wire: &Wire,
    ask: &zerocode_hookd::TurnBriefAsk,
    room: usize,
) -> Option<String> {
    if room <= rules::WINDOW_RECEIPT_OVERHEAD
        || !ask.supports_receipts
        || ask.wall.is_zero()
        || !hook_guard::turn_start_road(ask.agent).yes()
    {
        return None;
    }
    let session = ask
        .session_id
        .as_deref()
        .filter(|session| !session.is_empty())?;
    let until = Instant::now() + ask.wall;
    let home = wire.config_home()?;
    let term = crate::hooks::term_of_pane_key(&ask.pane_key)?;
    let root = Path::new(&ask.worktree).canonicalize().ok()?;
    let key = prompt_key(&ask.prompt);
    // The matching prompt must have reached the book first. A new session in
    // the same pane must never receive the old occupant's advisory.
    let recipient = loop {
        if let Some(owner) = matching_recipient(guards, term, ask, session, &root, &key) {
            break owner;
        }
        if Instant::now() >= until {
            return None;
        }
        std::thread::sleep(BRIEF_POLL.min(until.saturating_duration_since(Instant::now())));
    };
    let allowed = || {
        let settings = wire.settings_root();
        let consent = zerocode_core::jev::door::JevSettings::from_root(&settings);
        PROJECT_RULES.mode_in(&settings) == JevMode::On
            && consent.enabled
            && consent.consents(&zerocode_core::jev::door::resolved_path(&root))
    };
    if Instant::now() >= until || !allowed() {
        return None;
    }
    let advice = store::pending(
        home,
        &root,
        &recipient,
        room.saturating_sub(rules::WINDOW_RECEIPT_OVERHEAD)
            .min(rules::ADVICE_CHAR_CAP),
    )
    .ok()??;
    if Instant::now() >= until || !allowed() {
        return None;
    }
    if matching_recipient(guards, term, ask, session, &root, &key).as_deref()
        != Some(recipient.as_str())
        || Instant::now() >= until
    {
        return None;
    }
    let text = rules::window_text(&advice, &recipient);
    (text.chars().count() <= room).then_some(text)
}

pub(super) fn delivered(ask: &zerocode_hookd::TurnBriefAsk, text: &str) -> std::io::Result<()> {
    delivered_in(&Wire::of_this_machine(), ask, text)
}

fn delivered_in(
    wire: &Wire,
    ask: &zerocode_hookd::TurnBriefAsk,
    text: &str,
) -> std::io::Result<()> {
    let Some((advice, receipt)) = rules::window_receipt(text) else {
        return Ok(());
    };
    let recipient = ask
        .session_id
        .as_deref()
        .ok_or_else(|| std::io::Error::other("missing receipt session"))?;
    let home = wire
        .config_home()
        .ok_or_else(|| std::io::Error::other("missing receipt home"))?;
    let root = Path::new(&ask.worktree).canonicalize()?;
    let ledger = systemone::project_ledger_of(wire, &PROJECT_RULES, &root)
        .ok_or_else(|| std::io::Error::other("missing receipt ledger"))?;
    systemone::append_rows_checked(
        &ledger,
        &[rules::receipt_for_key(
            receipt,
            crate::usage_runtime::epoch_ms_now(),
        )],
    )?;
    store::delivered(home, &root, recipient, advice)
}

fn matching_recipient(
    guards: &Mutex<Guards>,
    term: u32,
    ask: &zerocode_hookd::TurnBriefAsk,
    session: &str,
    root: &Path,
    prompt: &str,
) -> Option<String> {
    let (owner, directory) = {
        let held = guards.lock().unwrap_or_else(PoisonError::into_inner);
        let book = held.panes.get(&term)?;
        let pane = book.pane.as_ref()?;
        if book.owner != session
            || pane.session.as_deref() != Some(session)
            || pane.agent != ask.agent
            || pane.prompt_key.as_deref() != Some(prompt)
        {
            return None;
        }
        (book.owner.clone(), pane.worktree.clone())
    };
    // Filesystem work must not hold every pane's shared book lock.
    (directory.canonicalize().ok()?.as_path() == root).then_some(owner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systemone::tests::Endpoint;
    use serde_json::json;

    fn configured(mode: &str) -> (tempfile::TempDir, tempfile::TempDir, Wire, Endpoint, Pane) {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path().canonicalize().unwrap();
        std::fs::write(root.join("AGENTS.md"), "Avoid duplicated logic.\n").unwrap();
        let book = rules::Book {
            schema_version: 1,
            rules: vec![rules::Rule {
                id: "duplicates".into(),
                source: rules::Source {
                    path: "AGENTS.md".into(),
                    sha256: String::new(),
                    first_line: 1,
                    last_line: 1,
                    text: "Avoid duplicated logic.".into(),
                },
                paths: Vec::new(),
                check: rules::Check::Model {
                    question: "Is the same logic newly implemented more than once?".into(),
                },
            }],
        };
        store::compile(home.path(), &root, book).unwrap();
        let settings = home.path().join("settings.json");
        std::fs::write(
            &settings,
            json!({"smart":{PROJECT_RULES.setting:mode,
            "jev":{"enabled":true,"workspaces":[root]}}})
            .to_string(),
        )
        .unwrap();
        let endpoint = Endpoint::serving_each("HTTP/1.1 200 OK", json!({"model":"jev-test",
            "answers":{"duplicates":{"type":"choice","choice":"violated","confidence":0.8,
                "probabilities":{"violated":0.9,"followed":0.0,"not_applicable":0.0,"unknown":0.1}}},
            "usage":{"input_tokens":10,"output_tokens":0}}).to_string(), 0);
        let wire = Wire::at(&endpoint.base(), "test-key", Some(settings));
        let pane = Pane {
            term: 7,
            agent: AgentKind::Claude,
            session: Some("rule-session".into()),
            worktree: root,
            worker: false,
            prompt_key: None,
        };
        (home, workspace, wire, endpoint, pane)
    }

    #[test]
    fn an_on_advisory_waits_for_budget_and_only_reaches_the_same_sessions_prompt_once() {
        assert!(
            BRIEF_CONTRIBUTORS
                .iter()
                .any(|voice| std::ptr::fn_addr_eq(*voice, brief_line as BriefContributor)),
            "the working adapter must be registered on the window's production brief"
        );
        let (home, _workspace, wire, endpoint, pane) = configured("on");
        check(
            &wire,
            &pane,
            &Turn {
                id: "turn-one".into(),
                at: 1,
                task: "Check code.".into(),
                edited: Vec::new(),
                said: SaidAt::Words("The change is ready.".into()),
                evidence: Vec::new(),
            },
        );
        assert_eq!(endpoint.asked().len(), 1);
        assert!(
            store::pending(home.path(), &pane.root(), &pane.owner(), 1200)
                .unwrap()
                .is_some()
        );
        let ask = zerocode_hookd::TurnBriefAsk {
            agent: pane.agent,
            pane_key: crate::hooks::pane_key_of(pane.term),
            launch_token: String::new(),
            session_id: pane.session.clone(),
            supports_receipts: true,
            worktree: pane.root().to_string_lossy().into_owned(),
            prompt: "Continue.".into(),
            wall: Duration::from_millis(100),
        };
        let mut current = pane.clone();
        current.prompt_key = Some(prompt_key(&ask.prompt));
        let guards = Mutex::new(Guards {
            panes: HashMap::from([(
                pane.term,
                PaneBook {
                    owner: pane.owner(),
                    pane: Some(current.clone()),
                    ..PaneBook::default()
                },
            )]),
        });
        assert!(brief_in(&guards, &wire, &ask, 1).is_none());
        let expired = zerocode_hookd::TurnBriefAsk {
            wall: Duration::ZERO,
            ..ask.clone()
        };
        assert!(brief_in(&guards, &wire, &expired, 1200).is_none());
        let next_session = zerocode_hookd::TurnBriefAsk {
            session_id: Some("new-session".into()),
            wall: Duration::from_millis(1),
            ..ask.clone()
        };
        assert!(
            brief_in(&guards, &wire, &next_session, 1200).is_none(),
            "the same prompt in a new session must not take the previous book's advice"
        );
        let unidentified = zerocode_hookd::TurnBriefAsk {
            session_id: None,
            ..ask.clone()
        };
        assert!(brief_in(&guards, &wire, &unidentified, 1200).is_none());
        let ledger = systemone::project_ledger_of(&wire, &PROJECT_RULES, &pane.root()).unwrap();
        let counts = || zerocode_core::jev::summary::summarize(&systemone::read_rows(&ledger), 0);
        assert_eq!(
            counts().applied,
            0,
            "no receipt for a budget-refused contribution"
        );
        assert!(
            store::pending(home.path(), &pane.root(), &pane.owner(), 1200)
                .unwrap()
                .is_some()
        );
        guards
            .lock()
            .unwrap()
            .panes
            .get_mut(&pane.term)
            .unwrap()
            .owner = "new-session".into();
        assert!(brief_in(&guards, &wire, &ask, 1200).is_none());
        guards
            .lock()
            .unwrap()
            .panes
            .get_mut(&pane.term)
            .unwrap()
            .owner = pane.owner();
        let delivered = brief_in(&guards, &wire, &ask, 1200).unwrap();
        assert!(delivered.contains("duplicates"));
        assert!(
            brief_in(&guards, &wire, &ask, 1200).is_some(),
            "a producer is not an acknowledgment"
        );
        assert_eq!(counts().applied, 0);
        delivered_in(&wire, &ask, &delivered).unwrap();
        delivered_in(&wire, &ask, &delivered).unwrap();
        assert!(brief_in(&guards, &wire, &ask, 1200).is_none());
        assert_eq!(endpoint.asked().len(), 1, "briefing reads no model");
        let counted = counts();
        assert_eq!((counted.rows, counted.requests, counted.applied), (1, 1, 1));
    }

    #[test]
    fn a_recording_mode_and_changed_source_never_deliver_a_rule_advisory() {
        let (home, _workspace, wire, endpoint, pane) = configured("shadow");
        let turn = Turn {
            id: "turn-one".into(),
            at: 1,
            task: "Check code.".into(),
            edited: Vec::new(),
            said: SaidAt::Words("The change is ready.".into()),
            evidence: Vec::new(),
        };
        check(&wire, &pane, &turn);
        assert_eq!(endpoint.asked().len(), 1);
        assert!(
            store::pending(home.path(), &pane.root(), &pane.owner(), 1200)
                .unwrap()
                .is_none()
        );
        std::fs::write(pane.root().join("AGENTS.md"), "Changed instructions.\n").unwrap();
        check(&wire, &pane, &turn);
        assert_eq!(
            endpoint.asked().len(),
            1,
            "changed instructions ask nothing"
        );
    }
}
