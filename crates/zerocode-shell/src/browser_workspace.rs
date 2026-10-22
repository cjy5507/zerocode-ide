use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use zerocode_core::agent_browser::{
    BROWSER_CLI, TAB_MODE_AGENT, TAB_MODE_PAUSED, TAB_MODE_PERSON, WORKSPACE_LOOK_UNANSWERED,
    WORKSPACE_PAUSED, WORKSPACE_PERSON_CONTROLLED,
};

#[cfg(test)]
mod original_observation_regressions {
    use super::*;

    #[test]
    fn an_agent_read_captures_its_revision_without_a_prior_revision_input() {
        let workspaces = Workspaces::default();
        workspaces
            .register("browser-1", Some("pane-a"), false)
            .unwrap();
        for verb in ["read", "marks", "fields", "screenshot"] {
            let result = workspaces.begin_at("browser-1", Some("pane-a"), verb, None);
            assert!(
                result.is_ok(),
                "{verb} must capture the read's own revision"
            );
            if let Ok(Some(operation)) = result {
                operation.finish(true);
            }
        }
    }

    /// A managed write that names no revision is judged at the one its own
    /// agent's last look issued (t-67064): refused before any look, taken
    /// while the tab stands at that look — its settle naming the revision the
    /// next change goes on at — and refused once the tab moved past it, each
    /// refusal naming the look to send.
    #[test]
    fn a_managed_write_without_a_revision_goes_on_by_its_agents_own_look() {
        let workspaces = Workspaces::default();
        workspaces
            .register("browser-1", Some("pane-a"), false)
            .unwrap();
        let says = |result: Result<Option<Operation>, String>, code: &str| {
            result.err().is_some_and(|error| {
                error.starts_with(code) && error.contains("zerocode-browser marks browser-1")
            })
        };
        for verb in ["click", "type", "fill", "goto", "close", "scroll", "find"] {
            assert!(
                says(
                    workspaces.begin_at("browser-1", Some("pane-a"), verb, None),
                    "workspace_observation_required: "
                ),
                "{verb} before any look"
            );
        }
        let seen = workspaces
            .begin_at("browser-1", Some("pane-a"), "read", None)
            .unwrap()
            .unwrap()
            .finish_observation()
            .unwrap();
        for verb in ["click", "type", "fill", "goto", "scroll", "find"] {
            let operation = workspaces
                .begin_at("browser-1", Some("pane-a"), verb, None)
                .unwrap_or_else(|error| panic!("{verb} after the look: {error}"))
                .expect("a work browser");
            assert_eq!(operation.settle(Dispatch::Completed), Some(seen), "{verb}");
        }
        let at = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        workspaces
            .transition("browser-1", Mode::Paused, at)
            .unwrap()
            .finish()
            .unwrap();
        let mut back = workspaces
            .transition("browser-1", Mode::Agent, at + 2)
            .unwrap();
        back.confirm_fence().unwrap();
        back.finish().unwrap();
        assert!(says(
            workspaces.begin_at("browser-1", Some("pane-a"), "click", None),
            "workspace_changed: "
        ));
    }

    async fn switch(workspaces: &Workspaces, mode: Mode) {
        let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        let mut transition = workspaces.transition("browser-1", mode, revision).unwrap();
        transition.wait().await.unwrap();
        transition.confirm_fence().unwrap();
        transition.finish().unwrap();
    }

    #[tokio::test]
    async fn a_request_delayed_before_send_cannot_borrow_a_resumed_agents_new_read() {
        let workspaces = Workspaces::default();
        workspaces
            .register("browser-1", Some("pane-a"), false)
            .unwrap();
        let original = workspaces
            .begin_at("browser-1", Some("pane-a"), "marks", None)
            .unwrap()
            .unwrap()
            .finish_observation()
            .unwrap();
        switch(&workspaces, Mode::Human).await;
        switch(&workspaces, Mode::Agent).await;
        let current = workspaces
            .begin_at("browser-1", Some("pane-a"), "marks", None)
            .unwrap()
            .unwrap()
            .finish_observation()
            .unwrap();
        assert_ne!(original, current);
        let delayed = workspaces.begin_at("browser-1", Some("pane-a"), "click", Some(original));
        assert!(matches!(delayed, Err(error) if error.starts_with("workspace_stale:")));
        workspaces
            .begin_at("browser-1", Some("pane-a"), "click", Some(current))
            .unwrap()
            .unwrap()
            .finish(true);
    }

    #[tokio::test]
    async fn a_read_interrupted_by_handoff_cannot_issue_a_completion_stamp() {
        let workspaces = Workspaces::default();
        workspaces
            .register("browser-1", Some("pane-a"), false)
            .unwrap();
        let reading = workspaces
            .begin_at("browser-1", Some("pane-a"), "read", None)
            .unwrap()
            .unwrap();
        let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        let mut transition = workspaces
            .transition("browser-1", Mode::Human, revision)
            .unwrap();
        assert!(reading.finish_observation().is_err());
        transition.wait().await.unwrap();
        transition.confirm_fence().unwrap();
        transition.finish().unwrap();
        assert!(
            workspaces
                .snapshot("browser-1")
                .unwrap()
                .unwrap()
                .needs_observation
        );
    }

    #[test]
    fn observation_completion_checks_owner_even_when_the_revision_is_unchanged() {
        let workspaces = Workspaces::default();
        workspaces
            .register("browser-1", Some("pane-a"), false)
            .unwrap();
        let reading = workspaces
            .begin_at("browser-1", Some("pane-a"), "read", None)
            .unwrap()
            .unwrap();
        workspaces
            .get("browser-1")
            .unwrap()
            .unwrap()
            .lock()
            .unwrap()
            .owner = Some("pane-b".into());
        assert!(reading.finish_observation().is_err());
    }

    #[test]
    fn an_original_revision_does_not_authorize_another_actor_or_managed_eval() {
        let workspaces = Workspaces::default();
        workspaces
            .register("browser-1", Some("pane-a"), false)
            .unwrap();
        let revision = workspaces
            .begin_at("browser-1", Some("pane-a"), "fields", None)
            .unwrap()
            .unwrap()
            .finish_observation()
            .unwrap();
        let other = workspaces.begin_at("browser-1", Some("pane-b"), "click", Some(revision));
        assert!(matches!(other, Err(error) if error.starts_with("workspace_owner_mismatch:")));
        let eval = workspaces.begin_at("browser-1", Some("pane-a"), "eval", Some(revision));
        assert!(matches!(eval, Err(error) if error.starts_with("workspace_eval_refused:")));
        assert!(
            workspaces
                .begin_at("ordinary", Some("pane-a"), "click", None)
                .unwrap()
                .is_none()
        );
    }
}

/// The agent goes on by its own look (t-67064): a change sent without
/// `--workspace-revision` is judged at the revision its own agent's last look
/// at that tab issued, and a tab the person holds answers at once, by name.
#[cfg(test)]
mod own_look_regressions {
    use super::*;
    use crate::computer_use::guard;

    const AGENT: &str = "term-21";
    const TAB: &str = "browser-4";

    /// One command sent without `--workspace-revision` down the door's own
    /// admission road, the page answering every one: what
    /// `answer_browser_command` asks of the work browsers — `open` registers
    /// by the door that reads the person's stop, every other verb is checked,
    /// admitted and settled. The refusal, when the door refused it.
    fn door(workspaces: &Workspaces, verb: &str, label: &str) -> Option<String> {
        if verb == "open" {
            return workspaces.register_agents(label, Some(AGENT)).err();
        }
        if let Err(error) = workspaces.check_actor(Some(AGENT), verb) {
            return Some(error);
        }
        let operation = match workspaces.begin_agents(label, Some(AGENT), verb, None) {
            Ok(operation) => operation,
            Err(error) => return Some(error),
        };
        let actor = match workspaces.begin_actor_target(Some(AGENT), verb, Some(label)) {
            Ok(actor) => actor,
            Err(error) => {
                if let Some(operation) = operation {
                    operation.settle(Dispatch::NotSent);
                }
                return Some(error);
            }
        };
        let looked = match operation {
            Some(operation) if zerocode_core::agent_browser::observes(verb) => {
                operation.finish_observation().err()
            }
            Some(operation) => {
                operation.settle(Dispatch::Completed);
                None
            }
            None => None,
        };
        if let Some(actor) = actor {
            actor.settle(Dispatch::Completed);
        }
        looked
    }

    /// Every command of `verbs` on the one tab, in order, by one agent: the
    /// refusals the door gave.
    fn walk(verbs: &[&str]) -> Vec<String> {
        let _hand = crate::tests::computer_desktop_wait::ONE_HAND
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard::lift();
        let workspaces = Workspaces::default();
        verbs
            .iter()
            .filter_map(|verb| door(&workspaces, verb, TAB))
            .collect()
    }

    fn changes(verbs: &[&str]) -> usize {
        verbs
            .iter()
            .filter(|verb| {
                **verb != "open"
                    && zerocode_core::agent_browser::browser_verb(verb)
                        .is_some_and(|row| row.changes_page)
            })
            .count()
    }

    /// The briefing's sequence, no `--workspace-revision` anywhere: every one
    /// of its eleven changes is taken on the agent's one look.
    #[test]
    fn the_briefings_sequence_without_a_revision_is_taken_whole() {
        const SENT: [&str; 13] = [
            "open", "read", "fill", "click", "scroll", "scroll", "scroll", "click", "scroll",
            "scroll", "goto", "fill", "click",
        ];
        let refused = walk(&SENT);
        assert_eq!(changes(&SENT), 11);
        assert!(
            refused.is_empty(),
            "{} of the sequence's 11 changes were refused: {refused:#?}",
            refused.len()
        );
    }

    /// The coordinator's own scene of 10-21 05:13–05:15 (the session's step
    /// log, steps 1–26 but the two with a mistaken label): the stretch it sent
    /// before it learnt the flag, with the plain `read` it sent after `wait`,
    /// which the step log keeps no line for. Its 16 changes were all refused
    /// for the revision they did not name; every one is taken.
    #[test]
    fn the_flagless_stretch_of_the_coordinators_scene_is_taken_whole() {
        const SENT: [&str; 25] = [
            "open", "wait", "read", "type", "click", "scroll", "scroll", "scroll", "scroll",
            "marks", "fill", "marks", "marks", "scroll", "scroll", "marks", "click", "scroll",
            "scroll", "scroll", "click", "goto", "marks", "scroll", "marks",
        ];
        let refused = walk(&SENT);
        assert_eq!(changes(&SENT), 16);
        assert!(
            refused.is_empty(),
            "{} of the scene's 16 changes were refused: {refused:#?}",
            refused.len()
        );
    }

    /// Without a look first, the changes before the agent's first look are
    /// refused — `wait` shows the agent nothing — each naming the look to
    /// send; from the look on, every change is taken.
    #[test]
    fn only_the_changes_before_the_first_look_are_refused_and_each_names_the_look() {
        const SENT: [&str; 10] = [
            "open", "wait", "type", "click", "scroll", "scroll", "marks", "fill", "scroll", "click",
        ];
        let refused = walk(&SENT);
        assert_eq!(
            refused.len(),
            4,
            "the four changes before the first look, and none after it: {refused:#?}"
        );
        for refusal in &refused {
            assert!(
                refusal.starts_with("workspace_observation_required: ")
                    && refusal.contains(&format!("zerocode-browser marks {TAB}")),
                "{refusal}"
            );
        }
    }

    /// A tab the person holds answers every agent verb at once and by name
    /// (t-67064) — a look as much as a change — and the agent's `open` beside
    /// it names that tab and the person's 「다시 맡기기」.
    #[test]
    fn a_tab_the_person_holds_answers_the_agent_at_once_by_name() {
        let _hand = crate::tests::computer_desktop_wait::ONE_HAND
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard::lift();
        let workspaces = Workspaces::default();
        workspaces.register(TAB, Some(AGENT), false).unwrap();
        let at = workspaces.snapshot(TAB).unwrap().unwrap().revision;
        let mut transition = workspaces.transition(TAB, Mode::Human, at).unwrap();
        transition.confirm_fence().unwrap();
        transition.finish().unwrap();
        let held = format!("workspace_person_controlled: 작업 브라우저 {TAB}: ");
        for verb in [
            "marks",
            "fields",
            "click",
            "read",
            "screenshot",
            "scroll",
            "type",
            "fill",
            "goto",
            "find",
            "close",
        ] {
            let began = Instant::now();
            let said = workspaces.check_actor(Some(AGENT), verb).and_then(|()| {
                workspaces
                    .begin_agents(TAB, Some(AGENT), verb, None)
                    .map(|_| ())
            });
            let took = began.elapsed();
            assert!(took < Duration::from_secs(1), "{verb} took {took:?}");
            assert!(
                said.as_ref().is_err_and(
                    |error| error.starts_with(&held) && error.contains("「다시 맡기기」")
                ),
                "{verb}: {said:?}"
            );
        }
        let opened = workspaces.register_agents("browser-5", Some(AGENT));
        assert!(
            opened
                .as_ref()
                .is_err_and(|error| error.starts_with(&held) && error.contains("「다시 맡기기」")),
            "{:?}",
            opened.map(|snapshot| snapshot.mode)
        );
    }

    /// Another agent's look never lends its revision (t-67064): a tab handed
    /// to another agent keeps none of its last owner's looks, and the new
    /// owner's change without a revision waits for its own look.
    #[test]
    fn a_tab_handed_to_another_agent_keeps_none_of_its_last_owners_looks() {
        const OTHER: &str = "term-22";
        let _hand = crate::tests::computer_desktop_wait::ONE_HAND
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard::lift();
        let workspaces = Workspaces::default();
        workspaces.register(TAB, Some(AGENT), false).unwrap();
        workspaces
            .begin_at(TAB, Some(AGENT), "marks", None)
            .unwrap()
            .unwrap()
            .finish_observation()
            .unwrap();
        let at = workspaces.snapshot(TAB).unwrap().unwrap().revision;
        let mut handed = workspaces
            .transition_owned(TAB, Mode::Agent, at, Some(OTHER))
            .unwrap();
        handed.confirm_fence().unwrap();
        handed.finish().unwrap();
        let theirs = workspaces.begin_at(TAB, Some(OTHER), "click", None).err();
        let standing = workspaces.standing_agents(TAB, Some(OTHER)).unwrap();
        assert!(
            theirs
                .as_ref()
                .is_some_and(|error| error.starts_with("workspace_observation_required: ")),
            "{theirs:?}"
        );
        assert_eq!(standing.map(|standing| standing.seen), Some(None));
    }

    /// A look whose page never answers is answered at once when the person
    /// takes its tab (t-67064): the person's tab by name, well inside a
    /// second, and the handoff then drains; while nobody takes the tab, the
    /// look waits for its page.
    #[tokio::test]
    async fn a_look_whose_page_never_answers_is_answered_at_once_when_the_person_takes_its_tab() {
        let workspaces = Arc::new(Workspaces::default());
        workspaces.register(TAB, Some(AGENT), false).unwrap();
        let look = workspaces
            .begin_at(TAB, Some(AGENT), "marks", None)
            .unwrap()
            .unwrap();
        let waiting = tokio::time::timeout(
            Duration::from_millis(50),
            unless_taken(Some(&look), std::future::pending::<()>()),
        )
        .await;
        assert!(waiting.is_err(), "a look on a tab nobody took waits");
        let taker = {
            let workspaces = workspaces.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                let at = workspaces.snapshot(TAB).unwrap().unwrap().revision;
                let mut transition = workspaces.transition(TAB, Mode::Human, at).unwrap();
                transition.wait().await.unwrap();
                transition.confirm_fence().unwrap();
                transition.finish().unwrap()
            })
        };
        let began = Instant::now();
        let answered = tokio::time::timeout(
            Duration::from_secs(1),
            unless_taken(Some(&look), std::future::pending::<()>()),
        )
        .await;
        let took = began.elapsed();
        drop(look);
        let handed = tokio::time::timeout(Duration::from_secs(1), taker)
            .await
            .expect("the handoff drains once the look is answered")
            .unwrap();
        assert!(
            matches!(&answered, Ok(Err(error))
                if error.starts_with(&format!("workspace_person_controlled: 작업 브라우저 {TAB}: "))),
            "{answered:?} after {took:?}"
        );
        assert!(took < Duration::from_secs(1), "{took:?}");
        assert_eq!(handed.mode, Mode::Human);
    }

    /// A change in flight is not cut short by its tab's handoff: the handoff
    /// waits for it whole, as it always did (t-62872).
    #[tokio::test]
    async fn a_change_in_flight_is_waited_for_whole() {
        let workspaces = Workspaces::default();
        workspaces.register(TAB, Some(AGENT), false).unwrap();
        workspaces
            .begin_at(TAB, Some(AGENT), "read", None)
            .unwrap()
            .unwrap()
            .finish_observation()
            .unwrap();
        let click = workspaces
            .begin_at(TAB, Some(AGENT), "click", None)
            .unwrap()
            .unwrap();
        let at = workspaces.snapshot(TAB).unwrap().unwrap().revision;
        let _transition = workspaces.transition(TAB, Mode::Human, at).unwrap();
        let pressed = tokio::time::timeout(
            Duration::from_millis(50),
            unless_taken(Some(&click), async { "pressed" }),
        )
        .await;
        assert_eq!(pressed, Ok(Ok("pressed")));
        click.settle(Dispatch::Completed);
    }

    /// `tabs` says who holds each of the agent's work browsers (t-67064): its
    /// mode, the revision the agent's own look issued, and the tab's own —
    /// and leaves off another agent's, and every one while the person's stop
    /// stands (t-62872).
    #[test]
    fn tabs_say_who_holds_each_work_browser_and_the_agents_own_look() {
        use zerocode_core::computer_use::STOP_REASON_HOTKEY;
        let _hand = crate::tests::computer_desktop_wait::ONE_HAND
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard::lift();
        let workspaces = Workspaces::default();
        for label in ["browser-4", "browser-5", "browser-6"] {
            workspaces.register(label, Some(AGENT), false).unwrap();
        }
        workspaces
            .register("browser-7", Some("term-22"), false)
            .unwrap();
        let seen = workspaces
            .begin_at("browser-4", Some(AGENT), "marks", None)
            .unwrap()
            .unwrap()
            .finish_observation()
            .unwrap();
        let hand = |label: &str, mode: Mode| {
            let at = workspaces.snapshot(label).unwrap().unwrap().revision;
            let mut transition = workspaces.transition(label, mode, at).unwrap();
            if mode != Mode::Paused {
                transition.confirm_fence().unwrap();
            }
            transition.finish().unwrap().revision
        };
        let held = hand("browser-5", Mode::Human);
        let paused = hand("browser-6", Mode::Paused);
        let standing = |label: &str| workspaces.standing_agents(label, Some(AGENT));
        assert_eq!(
            standing("browser-4"),
            Ok(Some(Standing {
                mode: "agent",
                seen: Some(seen),
                revision: seen
            }))
        );
        assert_eq!(
            standing("browser-5"),
            Ok(Some(Standing {
                mode: "person",
                seen: None,
                revision: held
            }))
        );
        assert_eq!(
            standing("browser-6"),
            Ok(Some(Standing {
                mode: "paused",
                seen: None,
                revision: paused
            }))
        );
        assert!(standing("browser-7").is_err(), "another agent's tab");
        assert_eq!(standing("ordinary"), Ok(None));
        guard::adopt(STOP_REASON_HOTKEY);
        let stopped = standing("browser-4");
        guard::lift();
        assert!(
            stopped
                .as_ref()
                .is_err_and(|error| error.starts_with("workspace_person_stopped:")),
            "{stopped:?}"
        );
    }

    /// A look its page did not answer says what to do next (t-67064): on a
    /// tab still in the agent's hands, whether the page is busy loading and
    /// the look to send again; on one the person took, who holds it; on
    /// another agent's, nothing.
    #[test]
    fn a_look_its_page_did_not_answer_says_what_to_do_next() {
        let workspaces = Workspaces::default();
        workspaces.register(TAB, Some(AGENT), false).unwrap();
        let loading = workspaces.after_failed_look(TAB, Some(AGENT), true);
        let idle = workspaces.after_failed_look(TAB, Some(AGENT), false);
        let theirs = workspaces.after_failed_look(TAB, Some("term-22"), false);
        let at = workspaces.snapshot(TAB).unwrap().unwrap().revision;
        let mut taken = workspaces.transition(TAB, Mode::Human, at).unwrap();
        taken.confirm_fence().unwrap();
        taken.finish().unwrap();
        let held = workspaces.after_failed_look(TAB, Some(AGENT), false);
        assert!(
            loading.as_deref().is_some_and(|next| next
                .starts_with("workspace_look_unanswered: 페이지가 아직 로딩 중입니다 — ")
                && next.contains("zerocode-browser marks browser-4")),
            "{loading:?}"
        );
        assert!(
            idle.as_deref()
                .is_some_and(|next| next
                    .starts_with("workspace_look_unanswered: 페이지가 답하지 않았습니다 — ")),
            "{idle:?}"
        );
        assert_eq!(theirs, None);
        assert!(
            held.as_deref()
                .is_some_and(|next| next.starts_with("workspace_person_controlled: ")),
            "{held:?}"
        );
    }

    /// The letter a hand-back sends (t-67064): the tab, its mode, its new
    /// revision, 「이어서 하세요」 and the look to send — and its receipt.
    #[test]
    fn a_hand_back_letter_says_the_tab_its_mode_and_revision_and_the_next_look() {
        let (body, receipt) = handed_back(TAB, 9);
        assert_eq!(
            body,
            "작업 브라우저 browser-4: 사람이 다시 맡겼습니다 — mode agent, workspaceRevision 9. 이어서 하세요: zerocode-browser marks browser-4로 다시 보고 이어 가세요"
        );
        assert_eq!(receipt, "browser-handback:browser-4:9");
    }

    /// Every word this module refuses in says what to do next (t-67064): each
    /// refusal is written in the one block, and each ends — after its ` — `
    /// — on the command an agent types or the person it waits for.
    #[test]
    fn every_refusal_says_what_to_do_next() {
        // Spelt in halves: this module stands before the production code,
        // and the first whole spelling in the file must be the code's own.
        let source = include_str!("browser_workspace.rs");
        let production = &source[source.find(concat!("const ", "WORKSPACE_CAP")).unwrap()
            ..source.find(concat!("mod ", "review_regressions")).unwrap()];
        let block = &production[production.find("/* ---- refusals (t-67064)").unwrap()
            ..production.find("/* ---- end of refusals ---- */").unwrap()];
        let outside = production.replace(block, "");
        assert_eq!(
            outside.matches("\"workspace_").count(),
            0,
            "a refusal written outside the one block"
        );
        let refusals = [
            unavailable(AGENTS_UNREADABLE),
            snapshot_busy(),
            changed(),
            stale(TAB),
            look_first(TAB),
            moved_since_look(TAB),
            look_unanswered(TAB, true),
            look_unanswered(TAB, false),
            person_controlled(TAB),
            paused(TAB, Resume::Observe),
            paused(TAB, Resume::Person),
            PERSONS_STOPPED.to_string(),
            busy(TAB),
            actor_required(),
            verb_unknown(),
            owner_mismatch(),
            eval_refused(TAB),
            history_unsupported(),
            limit("작업 브라우저 수"),
            exists(),
            mode_invalid(),
            missing(),
            owner_required(),
        ];
        let code = |text: &str| {
            text.split([':', '['])
                .next()
                .unwrap_or_default()
                .to_string()
        };
        let listed: BTreeSet<String> = refusals
            .iter()
            .map(|refusal| code(refusal.as_str()))
            .collect();
        for (at, _) in block.match_indices("\"workspace_") {
            let written = code(&block[at + 1..]);
            assert!(
                listed.contains(&written),
                "`{written}` is missing from this list"
            );
        }
        for refusal in &refusals {
            let (code, next) = refusal
                .split_once(" — ")
                .unwrap_or_else(|| panic!("no next step: {refusal}"));
            assert!(code.starts_with("workspace_"), "{refusal}");
            assert!(
                next.contains("zerocode-browser ")
                    || next.contains("사람에게 알리세요")
                    || next.contains("다시 요청하세요")
                    || next.contains("다시 보내세요")
                    || next.contains("띠에서"),
                "{refusal}"
            );
        }
        for agents in [
            changed(),
            stale(TAB),
            look_first(TAB),
            moved_since_look(TAB),
            look_unanswered(TAB, true),
            look_unanswered(TAB, false),
            person_controlled(TAB),
            paused(TAB, Resume::Observe),
            paused(TAB, Resume::Person),
            PERSONS_STOPPED.to_string(),
            actor_required(),
            verb_unknown(),
            owner_mismatch(),
            eval_refused(TAB),
            history_unsupported(),
            limit("작업 브라우저 수"),
            exists(),
        ] {
            assert!(agents.contains("zerocode-browser "), "{agents}");
        }
    }
}

const WORKSPACE_CAP: usize = 128;
const FRAME_INTERVAL: Duration = Duration::from_secs(1);
type Notice = Box<dyn Fn(Snapshot) + Send + Sync>;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Mode {
    Agent,
    Pausing,
    Paused,
    Human,
}

/// Who lifts a work browser's pause (t-66200) — the word every
/// `workspace_paused` refusal and the bar carry. `observe`: its owner's own
/// fresh look (`read`, `marks`, `fields`, `screenshot`) lifts it — a pause
/// the owner's change of unknown outcome left, on a tab the person handed
/// back on its bar in this window's session. `person`: only the person does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Resume {
    Observe,
    Person,
}

impl Resume {
    pub(crate) const fn word(self) -> &'static str {
        match self {
            Self::Observe => zerocode_core::agent_browser::RESUME_OBSERVE,
            Self::Person => zerocode_core::agent_browser::RESUME_PERSON,
        }
    }
}

impl Serialize for Resume {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.word())
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    pub(crate) label: String,
    pub(crate) mode: Mode,
    pub(crate) revision: u64,
    pub(crate) in_flight: usize,
    pub(crate) needs_observation: bool,
    pub(crate) owner_term: Option<String>,
    pub(crate) snapshot_supported: bool,
    /// Whether the window shows this tab live for the person's eyes only
    /// (t-66513): the shield stands over a Chromium pane on macOS
    /// ([`Placement::Watch`]); elsewhere watching is the snapshots'.
    pub(crate) live_supported: bool,
    /// The session's regrant ([`State::regrant`]).
    pub(crate) regrant: bool,
    /// Who lifts the tab's pause while it is out of its agent's hands; none
    /// while an agent holds it.
    pub(crate) resume: Option<Resume>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Dispatch {
    Completed,
    NotSent,
    Uncertain,
}

impl Dispatch {
    fn of(succeeded: bool) -> Self {
        if succeeded {
            Self::Completed
        } else {
            Self::Uncertain
        }
    }
}

/// How a work browser stands for one agent on its `tabs` line (t-67064):
/// who holds it — `agent`, `person` or `paused` — the revision that agent's
/// own last look issued, if it has looked, and the tab's revision now.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Standing {
    pub(crate) mode: &'static str,
    pub(crate) seen: Option<u64>,
    pub(crate) revision: u64,
}

/// What one placement of a pane may show (t-66513), decided against the
/// tab's state at the native operation itself ([`Workspaces::with_placement`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Placement {
    /// For the person's hand: shown, and the page takes their input — a work
    /// browser in their hands, or a tab that is no work browser.
    Control,
    /// For the person's eyes only: shown under the shield, which takes every
    /// click, key, scroll and drag before the page could — a tab the person
    /// watches live, whoever holds it.
    Watch,
    /// Neither: the pane stays hidden.
    Hidden,
}

/// The native moves of one placement (t-66513), each engine's own; the
/// window's tests stand a fake here to pin their order ([`stage_pane`]).
#[cfg(any(all(target_os = "macos", feature = "chromium-browser"), test))]
pub(crate) trait PaneStage {
    /// Move and size the hidden pane.
    fn bounds(&mut self, bounds: [f64; 4]) -> Result<(), String>;
    /// Raise the shield over the pane's frame, or take it down.
    fn shield(&mut self, raised: bool) -> Result<(), String>;
    /// Show the pane.
    fn show(&mut self) -> Result<(), String>;
}

/// One placement's native moves, in the order the shield needs (t-66513): the
/// pane is already hidden when the decision is made; its frame goes first,
/// then the shield goes up over that frame — or down — and only then is the
/// pane shown. No frame of a watched pane is ever on screen without the shield
/// over it, and no pane in the person's hands is under one. Whether the pane
/// was shown: a pane the decision keeps hidden is the caller's to refuse.
#[cfg(any(all(target_os = "macos", feature = "chromium-browser"), test))]
pub(crate) fn stage_pane(
    stage: &mut impl PaneStage,
    placement: Placement,
    bounds: [f64; 4],
    shown: bool,
) -> Result<bool, String> {
    stage.bounds(bounds)?;
    stage.shield(shown && placement == Placement::Watch)?;
    if !shown || placement == Placement::Hidden {
        return Ok(false);
    }
    stage.show()?;
    Ok(true)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StepControl {
    label: String,
    owner: String,
    revision: u64,
}

#[derive(Clone, Default)]
pub(crate) struct EvidenceCheck(Option<(Arc<Workspace>, StepControl)>);

impl EvidenceCheck {
    pub(crate) fn holds(&self) -> bool {
        self.0.as_ref().is_none_or(|(workspace, control)| {
            workspace
                .lock()
                .is_ok_and(|state| agent_holds(&state, control))
        })
    }

    pub(crate) fn start<T>(&self, send: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        let Some((workspace, control)) = &self.0 else {
            return send();
        };
        let _sending = {
            let mut state = workspace.lock()?;
            if !agent_holds(&state, control) {
                return Err(stale(&control.label));
            }
            state.capturing += 1;
            Sending(workspace)
        };
        send()
    }
}

struct Sending<'a>(&'a Workspace);

impl Drop for Sending<'_> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.0.lock() {
            state.capturing = state.capturing.saturating_sub(1);
        }
        self.0.drained.notify_waiters();
    }
}

struct State {
    mode: Mode,
    revision: u64,
    in_flight: usize,
    capturing: usize,
    needs_observation: bool,
    owner: Option<String>,
    popup_pending: bool,
    closed: bool,
    person_controlled: bool,
    /// The person's 「다시 맡기기」 on this tab's bar turned the session's
    /// regrant on (t-66200): a pause its owner's own change of unknown outcome
    /// leaves is lifted by that owner's next fresh look. Off at birth, off
    /// again when the person takes the tab by hand or turns it off on the bar,
    /// gone with the tab — never kept past this window's session.
    regrant: bool,
    /// The revision the pause of the owner's own change of unknown outcome
    /// left (t-66200), the one pause a look may lift: any later move of the
    /// revision — the person's pause or stop, a handoff, a close, a popup —
    /// leaves it behind.
    uncertain_at: Option<u64>,
    /// The agent whose look last completed on this tab, and the revision that
    /// look issued (t-67064): what a change that agent sends without
    /// `--workspace-revision` is judged at. Never another agent's — a tab
    /// handed to another owner forgets it.
    seen: Option<(String, u64)>,
    /// The mode a handoff under way hands the tab to (t-67064), while the tab
    /// is `Pausing`: a tab on its way into the person's hands answers its agent
    /// as one in them.
    handing: Option<Mode>,
}

struct Workspace {
    label: String,
    state: Mutex<State>,
    /// Rung whenever a request on the tab ends or the tab's state moves under
    /// one — a handoff, a pause, a close — for the handoff waiting on the tab
    /// to drain and the look waiting on its page ([`Operation::taken`]).
    drained: Notify,
}

impl Workspace {
    fn lock(&self) -> Result<MutexGuard<'_, State>, String> {
        self.state
            .lock()
            .map_err(|_| unavailable("작업 브라우저 상태를 확인할 수 없습니다"))
    }

    fn snapshot(&self, state: &State) -> Snapshot {
        Snapshot {
            label: self.label.clone(),
            mode: state.mode,
            revision: state.revision,
            in_flight: state.in_flight,
            needs_observation: state.needs_observation,
            owner_term: state
                .owner
                .as_deref()
                .and_then(crate::hooks::term_of_pane_key)
                .map(|term| term.to_string()),
            snapshot_supported: cfg!(target_os = "macos"),
            live_supported: cfg!(all(target_os = "macos", feature = "chromium-browser")),
            regrant: state.regrant,
            resume: (state.mode != Mode::Agent).then(|| resume_of(state)),
        }
    }
}

#[derive(Default)]
struct FrameState {
    in_flight: bool,
    started: Option<Instant>,
    /// Agent captures waiting for the one in flight (t-66513): no watch frame
    /// starts before them.
    agents_waiting: usize,
    /// A watch frame waiting for its turn: one at a time.
    watch_waiting: bool,
}

/// One tab's capture lease, and the bell its release rings for the captures
/// waiting their turn (t-66513).
#[derive(Default)]
struct FrameSlot {
    state: Mutex<FrameState>,
    released: Notify,
}

impl FrameSlot {
    fn lock(&self) -> Result<MutexGuard<'_, FrameState>, String> {
        self.state.lock().map_err(|_| unavailable(WATCH_UNREADABLE))
    }
}

/// When a capture that may not start now may (t-66513): once the capture in
/// flight is released, or at an instant — the end of a watch frame's second.
enum Turn {
    Released,
    At(Instant),
}

/// Take the tab's capture lease if it is free now: no capture in flight, and
/// for a watch frame no agent capture waiting and its second over.
fn take_frame(slot: &Arc<FrameSlot>, spectator: bool) -> Result<Result<Frame, Turn>, String> {
    let mut state = slot.lock()?;
    if state.in_flight || (spectator && state.agents_waiting != 0) {
        return Ok(Err(Turn::Released));
    }
    let now = Instant::now();
    if spectator
        && let Some(started) = state.started
        && now.duration_since(started) < FRAME_INTERVAL
    {
        return Ok(Err(Turn::At(started + FRAME_INTERVAL)));
    }
    state.in_flight = true;
    if spectator {
        state.started = Some(now);
    }
    drop(state);
    Ok(Ok(Frame(slot.clone())))
}

/// A capture waiting for its turn, counted while it waits (t-66513): a watch
/// frame alone of its kind, an agent capture beside any other.
struct Waiting {
    slot: Arc<FrameSlot>,
    spectator: bool,
}

impl Waiting {
    fn enter(slot: Arc<FrameSlot>, spectator: bool) -> Result<Self, String> {
        let mut state = slot.lock()?;
        if spectator {
            if state.watch_waiting {
                return Err(snapshot_busy());
            }
            state.watch_waiting = true;
        } else {
            state.agents_waiting += 1;
        }
        drop(state);
        Ok(Self { slot, spectator })
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        if let Ok(mut state) = self.slot.lock() {
            if self.spectator {
                state.watch_waiting = false;
            } else {
                state.agents_waiting = state.agents_waiting.saturating_sub(1);
            }
        }
        // A watch frame held off by this agent capture may go now.
        self.slot.released.notify_waiters();
    }
}

/// An agent capture's turn at a tab's lease ([`Workspaces::agent_turn`]):
/// while it is held, no watch frame starts.
pub(crate) struct FrameTurn {
    _waiting: Waiting,
}

#[derive(Default)]
struct ActorState {
    in_flight: usize,
    uncertain: BTreeSet<String>,
}

#[derive(Default)]
struct ActorActivity {
    state: Mutex<ActorState>,
    drained: Notify,
}

#[derive(Default)]
pub(crate) struct Workspaces {
    held: Mutex<BTreeMap<String, Arc<Workspace>>>,
    actors: Mutex<BTreeMap<String, Arc<ActorActivity>>>,
    frames: Mutex<BTreeMap<String, Arc<FrameSlot>>>,
    closed: Mutex<BTreeMap<String, u64>>,
}

/* ---- refusals (t-67064) -------------------------------------------------
 * Every word this module refuses in is written between here and the end
 * mark below, and each one ends with what to do next: the command an agent
 * types, or the person it waits for. `every_refusal_says_what_to_do_next`
 * reads this file and holds both. */

/// The look an agent sends to see a tab again — what most refusals end on.
fn look_at(label: &str) -> String {
    format!("{BROWSER_CLI} marks {label}")
}

/// What the window cannot read of its own work browsers — a lock a panic
/// left behind, a revision past `u64`: nothing an agent can fix.
fn unavailable(what: &str) -> String {
    format!("workspace_unavailable: {what} — 멈추고 사람에게 알리세요")
}

const WATCH_UNREADABLE: &str = "관전 상태를 확인할 수 없습니다";
const AGENTS_UNREADABLE: &str = "에이전트 요청 상태를 확인할 수 없습니다";
const CLOSES_UNREADABLE: &str = "닫기 확인 상태를 읽을 수 없습니다";

/// A capture that met the tab's one capture lease past its wait (t-66513).
fn snapshot_busy() -> String {
    "workspace_snapshot_busy: 관전 화면은 초당 한 번만 요청할 수 있습니다 — 1초 뒤 다시 요청하세요"
        .to_string()
}

/// The tab's state moved under a request that does not name it here — a
/// handoff, a close, an open the window did not finish.
fn changed() -> String {
    format!(
        "workspace_stale: 작업 브라우저 상태가 바뀌었습니다 — {BROWSER_CLI} tabs로 탭을 확인하고, 다시 본 뒤 번호 없이 다시 보내세요"
    )
}

/// [`changed`], for the tab named: the revision the agent named is not the
/// tab's now, or the tab moved while the request was on its way.
fn stale(label: &str) -> String {
    format!(
        "workspace_stale: 작업 브라우저 {label}의 상태가 바뀌었습니다 — {}로 다시 본 뒤 번호 없이 다시 보내세요",
        look_at(label)
    )
}

/// A change before its agent's first look at the tab, or one the tab wants
/// a fresh look for first (t-67064).
fn look_first(label: &str) -> String {
    format!(
        "workspace_observation_required: 먼저 read, marks 또는 fields를 실행하세요 — {}로 화면을 본 뒤 다시 보내세요",
        look_at(label)
    )
}

/// A change sent without `--workspace-revision` after the tab moved past
/// its agent's last look (t-67064).
fn moved_since_look(label: &str) -> String {
    format!(
        "workspace_changed: 화면이 바뀌었습니다 — {}(또는 read·fields)로 다시 보고 다시 보내세요",
        look_at(label)
    )
}

/// A look that came back without its page's answer — the page timed out,
/// went away, or answered nothing readable — on a tab still in its agent's
/// hands (t-67064): whether the page is busy loading, and the look to send
/// again; a page that keeps not answering is the person's to hear of.
fn look_unanswered(label: &str, loading: bool) -> String {
    let why = if loading {
        "페이지가 아직 로딩 중입니다"
    } else {
        "페이지가 답하지 않았습니다"
    };
    format!(
        "{WORKSPACE_LOOK_UNANSWERED}: {why} — 1초 뒤 {}로 다시 보세요. 다시 답이 없으면 {BROWSER_CLI} tabs로 상태를 보고 사람에게 알리세요",
        look_at(label)
    )
}

/// A tab the person holds by hand, or is taking into it (t-67064): its
/// agent neither looks at it nor changes any page nor opens a tab until the
/// person hands it back, and hears so at once — the tab, the person's
/// 「다시 맡기기」, and the look to send then.
fn person_controlled(label: &str) -> String {
    format!(
        "{WORKSPACE_PERSON_CONTROLLED}: 작업 브라우저 {label}: 사람이 조작 중입니다 — 사람이 「다시 맡기기」를 누를 때까지 이 탭은 볼 수 없습니다. 이 판의 다른 변경과 새 탭 열기도 받지 않습니다. 어디까지 했는지 알리고 기다리세요. 맡겨지면 {}로 다시 보고 이어 가세요",
        look_at(label)
    )
}

/// A paused tab's refusal (t-66200): the word, who lifts the pause — the
/// token `[resume=observe]` or `[resume=person]` a program reads off the
/// line — the tab, and what its agent does next.
fn paused(label: &str, resume: Resume) -> String {
    let (why, next) = match resume {
        Resume::Observe => (
            "결과를 모르는 변경 뒤 일시정지됐습니다",
            format!(
                "{}(또는 read·fields·screenshot)로 새로 본 뒤 이어 가세요",
                look_at(label)
            ),
        ),
        Resume::Person => (
            "일시정지됐습니다",
            format!(
                "사람이 풀면 {}로 다시 보고 이어 가세요. 그전까지 어디까지 했는지 알리고 사람이 풀 때까지 멈추세요",
                look_at(label)
            ),
        ),
    };
    format!(
        "{WORKSPACE_PAUSED}[resume={}]: 작업 브라우저 {label}: {why} — 자동으로 재시도하지 마세요. {next}",
        resume.word()
    )
}

/// What an agent is told at the work browsers' door while the person's stop
/// stands (t-62872): the person stopped it, and only they start it again.
pub(crate) const PERSONS_STOPPED: &str = "workspace_person_stopped: 사람이 정지했습니다 — 사람이 띠에서 다시 시작할 때까지 작업 브라우저를 열거나 쓰지 마세요. 어디까지 했는지 알리고 기다리세요. 다시 시작되면 zerocode-browser tabs로 모드를 확인하세요";

fn busy(label: &str) -> String {
    format!(
        "workspace_busy: 작업 브라우저 {label}의 이전 요청이 끝나지 않았습니다 — 앞 명령의 답을 받은 뒤 다시 보내세요"
    )
}

fn actor_required() -> String {
    format!(
        "workspace_actor_required: 현재 창이 보유한 에이전트 판이 필요합니다 — 이 창이 띄운 에이전트 판에서 {BROWSER_CLI} 명령을 다시 보내세요"
    )
}

fn verb_unknown() -> String {
    format!(
        "workspace_verb_unknown: 지원하지 않는 브라우저 명령입니다 — {BROWSER_CLI} --help로 명령을 보세요"
    )
}

fn owner_mismatch() -> String {
    format!(
        "workspace_owner_mismatch: 다른 에이전트의 작업 브라우저입니다 — {BROWSER_CLI} open <url>로 자신의 작업 탭을 여세요"
    )
}

fn eval_refused(label: &str) -> String {
    format!(
        "workspace_eval_refused: 전용 작업 브라우저는 임의 eval을 받지 않습니다 — {BROWSER_CLI} read·fields·fill {label}을 사용하세요"
    )
}

fn history_unsupported() -> String {
    format!(
        "workspace_history_unsupported: 사람이 조작한 작업 탭의 콘솔·요청 기록은 에이전트에게 주지 않습니다 — {BROWSER_CLI} open <url>로 새 작업 탭을 여세요"
    )
}

/// One of the window's bounds reached: work browsers, an agent's requests,
/// watch captures — `what` names its count.
fn limit(what: &str) -> String {
    format!(
        "workspace_limit: {what}가 한도에 도달했습니다 — {BROWSER_CLI} close <label>로 다 쓴 작업 탭을 닫거나, 앞 요청의 답을 받은 뒤 다시 보내세요"
    )
}

fn exists() -> String {
    format!(
        "workspace_exists: 이미 등록된 작업 브라우저입니다 — {BROWSER_CLI} tabs로 라벨을 확인하세요"
    )
}

/// The bar's own refusals, the person's to read (t-66200).
fn mode_invalid() -> String {
    "workspace_mode_invalid: 전환 중 상태를 직접 지정할 수 없습니다 — 띠에서 일시정지·내가 조작·다시 맡기기 중 하나를 고르세요"
        .to_string()
}

fn missing() -> String {
    "workspace_missing: 이 탭은 전용 작업 브라우저가 아닙니다 — 작업 탭의 띠에서 다시 고르세요"
        .to_string()
}

fn owner_required() -> String {
    "workspace_owner_required: 맡길 에이전트 판이 없습니다 — 띠에서 맡길 에이전트 판을 선택하세요"
        .to_string()
}

/* ---- end of refusals ---- */

/// A refusal's resume word and the words after its token, when the refusal
/// is a paused work browser's ([`paused`]).
pub(crate) fn paused_resume(error: &str) -> Option<(Resume, &str)> {
    let (word, message) = error
        .strip_prefix(WORKSPACE_PAUSED)?
        .strip_prefix("[resume=")?
        .split_once("]: ")?;
    [Resume::Observe, Resume::Person]
        .into_iter()
        .find(|resume| resume.word() == word)
        .map(|resume| (resume, message))
}

/// The words after the code, when a refusal is a tab the person holds
/// ([`person_controlled`]) — only the person lifts it, as `resume=person`.
pub(crate) fn person_controlled_words(error: &str) -> Option<&str> {
    error
        .strip_prefix(WORKSPACE_PERSON_CONTROLLED)?
        .strip_prefix(": ")
}

/// Whether the person holds the tab by hand, or a handoff under way is
/// putting it into their hands (t-67064).
fn persons_hand(state: &State) -> bool {
    state.mode == Mode::Human || (state.mode == Mode::Pausing && state.handing == Some(Mode::Human))
}

/// Why a tab is out of its agent's hands, as its agent hears it (t-67064):
/// the person holds it — or is taking it — by name; any other mode is a
/// pause, with who lifts it.
fn out_of_hands(label: &str, state: &State) -> String {
    if persons_hand(state) {
        person_controlled(label)
    } else {
        paused(label, resume_of(state))
    }
}

/// Who holds the tab, in the word `tabs` says it by (t-67064).
fn mode_word(state: &State) -> &'static str {
    if state.mode == Mode::Agent {
        TAB_MODE_AGENT
    } else if persons_hand(state) {
        TAB_MODE_PERSON
    } else {
        TAB_MODE_PAUSED
    }
}

/// Whether `actor`'s own last look at the tab still stands (t-67064): a
/// change sent without `--workspace-revision` is judged at the revision that
/// look issued. Another agent's look is never one — a tab handed over
/// forgets its last owner's — so no look is a look to send first, and an
/// older one a look to send again.
fn own_look(state: &State, actor: &str, label: &str) -> Result<(), String> {
    match &state.seen {
        Some((seer, revision)) if seer == actor => {
            if *revision == state.revision {
                Ok(())
            } else {
                Err(moved_since_look(label))
            }
        }
        _ => Err(look_first(label)),
    }
}

/// The letter the agent holding a work browser is sent when the person
/// hands it back (t-67064), and the receipt that keeps one hand-back one
/// letter: the tab, its mode, its new revision and the look to send — none
/// of what the person did on the page.
pub(crate) fn handed_back(label: &str, revision: u64) -> (String, String) {
    (
        format!(
            "작업 브라우저 {label}: 사람이 다시 맡겼습니다 — mode {TAB_MODE_AGENT}, workspaceRevision {revision}. 이어서 하세요: {}로 다시 보고 이어 가세요",
            look_at(label)
        ),
        format!("browser-handback:{label}:{revision}"),
    )
}

/// Who lifts a tab's pause (t-66200): its owner's look only for the pause
/// its own change of unknown outcome left, still standing at that revision,
/// with the session's regrant on and no popup waiting; the person for every
/// other — their pause or stop, their hand, a handoff, a navigation or a
/// popup the window did not finish.
fn resume_of(state: &State) -> Resume {
    if state.mode == Mode::Paused
        && !state.closed
        && state.regrant
        && !state.popup_pending
        && state.uncertain_at == Some(state.revision)
    {
        Resume::Observe
    } else {
        Resume::Person
    }
}

fn next_revision(state: &mut State) -> Result<(), String> {
    state.revision = state
        .revision
        .checked_add(1)
        .ok_or_else(|| unavailable("제어 상태 번호가 소진됐습니다"))?;
    Ok(())
}

/// Whether `actor` may change a page: not while a tab it owns is out of its
/// hands. The refusal names that tab and who lifts it (t-66200) — a tab only
/// the person can lift before one the actor's own look can — and a tab the
/// person holds by name (t-67064).
fn owner_ready(held: &BTreeMap<String, Arc<Workspace>>, actor: &str) -> Result<(), String> {
    let mut looks = None;
    for (label, workspace) in held {
        let state = workspace.lock()?;
        if state.owner.as_deref() != Some(actor) || state.mode == Mode::Agent {
            continue;
        }
        match resume_of(&state) {
            Resume::Person => return Err(out_of_hands(label, &state)),
            Resume::Observe if looks.is_none() => looks = Some(paused(label, Resume::Observe)),
            Resume::Observe => {}
        }
    }
    looks.map_or(Ok(()), Err)
}

fn agent_holds(state: &State, control: &StepControl) -> bool {
    !state.closed
        && state.mode == Mode::Agent
        && state.revision == control.revision
        && state.owner.as_deref() == Some(control.owner.as_str())
}

impl Workspaces {
    fn lock(&self) -> Result<MutexGuard<'_, BTreeMap<String, Arc<Workspace>>>, String> {
        self.held
            .lock()
            .map_err(|_| unavailable("작업 브라우저 목록을 확인할 수 없습니다"))
    }

    pub(crate) fn register(
        &self,
        label: &str,
        owner: Option<&str>,
        restored: bool,
    ) -> Result<Snapshot, String> {
        let mut held = self.lock()?;
        if held.contains_key(label) {
            return Err(exists());
        }
        if held.len() >= WORKSPACE_CAP {
            return Err(limit("작업 브라우저 수"));
        }
        if let Some(owner) = owner {
            owner_ready(&held, owner)?;
        }
        let workspace = Arc::new(Workspace {
            label: label.to_string(),
            state: Mutex::new(State {
                mode: if restored || owner.is_none() {
                    Mode::Paused
                } else {
                    Mode::Agent
                },
                revision: 1,
                in_flight: 0,
                capturing: 0,
                needs_observation: true,
                owner: owner.map(str::to_string),
                popup_pending: false,
                closed: false,
                person_controlled: false,
                regrant: false,
                uncertain_at: None,
                seen: None,
                handing: None,
            }),
            drained: Notify::new(),
        });
        let snapshot = workspace.snapshot(&*workspace.lock()?);
        self.closed
            .lock()
            .map_err(|_| unavailable(CLOSES_UNREADABLE))?
            .remove(label);
        held.insert(label.to_string(), workspace);
        Ok(snapshot)
    }

    fn get(&self, label: &str) -> Result<Option<Arc<Workspace>>, String> {
        Ok(self.lock()?.get(label).cloned())
    }

    pub(crate) fn snapshot(&self, label: &str) -> Result<Option<Snapshot>, String> {
        self.get(label)?
            .map(|workspace| Ok(workspace.snapshot(&*workspace.lock()?)))
            .transpose()
    }

    pub(crate) fn snapshots(&self) -> Result<Vec<Snapshot>, String> {
        self.lock()?
            .values()
            .map(|workspace| Ok(workspace.snapshot(&*workspace.lock()?)))
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn allows_person(&self, label: &str) -> Result<bool, String> {
        Ok(self
            .snapshot(label)?
            .is_none_or(|snapshot| snapshot.mode == Mode::Human && snapshot.in_flight == 0))
    }

    #[cfg(test)]
    pub(crate) fn allows_agent_metadata(
        &self,
        label: &str,
        actor: Option<&str>,
    ) -> Result<bool, String> {
        let Some(workspace) = self.get(label)? else {
            return Ok(actor.is_some());
        };
        let state = workspace.lock()?;
        Ok(!state.closed
            && state.mode == Mode::Agent
            && state.owner.as_deref() == actor
            && actor.is_some())
    }

    pub(crate) fn check_actor(&self, actor: Option<&str>, verb: &str) -> Result<(), String> {
        let actor = actor.ok_or_else(actor_required)?;
        let row = zerocode_core::agent_browser::browser_verb(verb).ok_or_else(verb_unknown)?;
        let held = self.lock()?;
        if row.changes_page {
            owner_ready(&held, actor)?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn begin_actor(
        &self,
        actor: Option<&str>,
        verb: &str,
    ) -> Result<Option<ActorOperation>, String> {
        self.begin_actor_target(actor, verb, None)
    }

    pub(crate) fn begin_actor_target(
        &self,
        actor: Option<&str>,
        verb: &str,
        label: Option<&str>,
    ) -> Result<Option<ActorOperation>, String> {
        let actor = actor.ok_or_else(actor_required)?;
        let row = zerocode_core::agent_browser::browser_verb(verb).ok_or_else(verb_unknown)?;
        if !row.changes_page {
            return Ok(None);
        }
        let held = self.lock()?;
        owner_ready(&held, actor)?;
        let mut actors = self
            .actors
            .lock()
            .map_err(|_| unavailable(AGENTS_UNREADABLE))?;
        let mut owners = Vec::with_capacity(held.len());
        let mut workspaces = Vec::new();
        for workspace in held.values() {
            let state = workspace.lock()?;
            if let Some(owner) = &state.owner {
                owners.push(owner.clone());
                if owner == actor {
                    workspaces.push(workspace.clone());
                }
            }
        }
        actors.retain(|actor, activity| {
            owners.contains(actor)
                || activity.state.lock().map_or(true, |state| {
                    state.in_flight != 0 || !state.uncertain.is_empty()
                })
        });
        if actors.len() >= WORKSPACE_CAP && !actors.contains_key(actor) {
            return Err(limit("에이전트 요청 수"));
        }
        let activity = actors.entry(actor.to_string()).or_default().clone();
        let mut state = activity
            .state
            .lock()
            .map_err(|_| unavailable(AGENTS_UNREADABLE))?;
        if state.in_flight >= WORKSPACE_CAP
            || (state.uncertain.len() >= WORKSPACE_CAP
                && label.is_some_and(|label| !state.uncertain.contains(label)))
        {
            return Err(limit("에이전트 요청 수"));
        }
        state.in_flight += 1;
        drop(state);
        Ok(Some(ActorOperation {
            activity,
            label: label.map(str::to_string),
            workspaces,
            completed: false,
            notice: None,
        }))
    }

    #[cfg(test)]
    pub(crate) fn begin(
        &self,
        label: &str,
        actor: Option<&str>,
        verb: &str,
    ) -> Result<Option<Operation>, String> {
        let revision = self.snapshot(label)?.map(|snapshot| snapshot.revision);
        self.begin_at(label, actor, verb, revision)
    }

    pub(crate) fn begin_at(
        &self,
        label: &str,
        actor: Option<&str>,
        verb: &str,
        expected_revision: Option<u64>,
    ) -> Result<Option<Operation>, String> {
        let actor = actor.ok_or_else(actor_required)?;
        let row = zerocode_core::agent_browser::browser_verb(verb).ok_or_else(verb_unknown)?;
        let held = self.lock()?;
        if row.changes_page {
            owner_ready(&held, actor)?;
        }
        let Some(workspace) = held.get(label).cloned() else {
            if expected_revision.is_some() {
                return Err(stale(label));
            }
            return Ok(None);
        };
        let mut state = workspace.lock()?;
        if state.closed || expected_revision.is_some_and(|revision| revision != state.revision) {
            return Err(stale(label));
        }
        if state.owner.as_deref() != Some(actor) {
            return Err(owner_mismatch());
        }
        let observes = zerocode_core::agent_browser::observes(verb);
        // The owner's look at a tab its own change of unknown outcome paused,
        // under the session's regrant, is the one verb such a tab admits: it
        // lifts the pause when it completes (t-66200).
        let resumes = state.mode != Mode::Agent && observes && resume_of(&state) == Resume::Observe;
        if state.mode != Mode::Agent && !resumes {
            return Err(out_of_hands(label, &state));
        }
        let activity = if resumes {
            self.actors
                .lock()
                .map_err(|_| unavailable(AGENTS_UNREADABLE))?
                .get(actor)
                .cloned()
        } else {
            None
        };
        if verb == "eval" {
            return Err(eval_refused(label));
        }
        if state.person_controlled && zerocode_core::agent_browser::reads_history(verb) {
            return Err(history_unsupported());
        }
        // A change that names no revision is judged at the one its own
        // agent's last look issued (t-67064) — never another agent's.
        if row.changes_page && expected_revision.is_none() {
            own_look(&state, actor, label)?;
        }
        if state.in_flight != 0 {
            return Err(busy(label));
        }
        if row.changes_page && state.needs_observation {
            return Err(look_first(label));
        }
        state.in_flight = 1;
        let revision = state.revision;
        drop(state);
        drop(held);
        Ok(Some(Operation {
            workspace,
            revision,
            owner: Some(actor.to_string()),
            changes_page: row.changes_page,
            observes,
            navigates: zerocode_core::agent_browser::navigates(verb),
            resumes,
            activity,
            completed: false,
            notice: None,
        }))
    }

    /// An agent's `open`, at the door (t-62872): refused while the person's
    /// stop stands, and registered under that stop's own lock, so no work
    /// browser comes into an agent's hands after the person's stop.
    pub(crate) fn register_agents(
        &self,
        label: &str,
        owner: Option<&str>,
    ) -> Result<Snapshot, String> {
        crate::computer_use::guard::unless_persons_stop(|| self.register(label, owner, false))
            .unwrap_or_else(|| Err(PERSONS_STOPPED.to_string()))
    }

    /// An agent's verb on a browser, at the door (t-62872): on a work browser
    /// it is refused while the person's stop stands — the window's own stop,
    /// the helper never asked — and admitted under that stop's own lock, so no
    /// read or input an agent sends after the person's stop gets in while the
    /// window is still on its way to pause the browser. A browser that is no
    /// work browser is not the stop's.
    pub(crate) fn begin_agents(
        &self,
        label: &str,
        actor: Option<&str>,
        verb: &str,
        expected_revision: Option<u64>,
    ) -> Result<Option<Operation>, String> {
        if self.get(label)?.is_none() {
            return self.begin_at(label, actor, verb, expected_revision);
        }
        crate::computer_use::guard::unless_persons_stop(|| {
            self.begin_at(label, actor, verb, expected_revision)
        })
        .unwrap_or_else(|| Err(PERSONS_STOPPED.to_string()))
    }

    /// How the work browser `label` stands for `actor` on its `tabs` line
    /// (t-67064): who holds it, the revision `actor`'s own last look issued,
    /// and the tab's own — read by the door that reads the person's stop
    /// (t-62872). `None` for a tab that is no work browser; refused — so
    /// `tabs` leaves the tab off — while the person's stop stands, and for a
    /// tab another agent holds.
    pub(crate) fn standing_agents(
        &self,
        label: &str,
        actor: Option<&str>,
    ) -> Result<Option<Standing>, String> {
        let actor = actor.ok_or_else(actor_required)?;
        let Some(workspace) = self.get(label)? else {
            return Ok(None);
        };
        crate::computer_use::guard::unless_persons_stop(|| {
            let state = workspace.lock()?;
            if state.closed {
                return Err(stale(label));
            }
            if state.owner.as_deref() != Some(actor) {
                return Err(owner_mismatch());
            }
            Ok(Some(Standing {
                mode: mode_word(&state),
                seen: state
                    .seen
                    .as_ref()
                    .filter(|(seer, _)| seer == actor)
                    .map(|(_, revision)| *revision),
                revision: state.revision,
            }))
        })
        .unwrap_or_else(|| Err(PERSONS_STOPPED.to_string()))
    }

    /// What the agent does after its look at a work browser came back
    /// without the page's answer (t-67064): who holds the tab now, when the
    /// tab left its hands; otherwise whether the page is busy loading and
    /// the look to send again. None for a tab that is no work browser or not
    /// this agent's.
    pub(crate) fn after_failed_look(
        &self,
        label: &str,
        actor: Option<&str>,
        loading: bool,
    ) -> Option<String> {
        let workspace = self.get(label).ok()??;
        let state = workspace.lock().ok()?;
        if state.closed || actor.is_none() || state.owner.as_deref() != actor {
            return None;
        }
        Some(if state.mode == Mode::Agent {
            look_unanswered(label, loading)
        } else {
            out_of_hands(label, &state)
        })
    }

    pub(crate) fn begin_person(
        &self,
        label: &str,
        expected_revision: Option<u64>,
    ) -> Result<Option<Operation>, String> {
        let Some(workspace) = self.get(label)? else {
            return if expected_revision.is_some() {
                Err(changed())
            } else {
                Ok(None)
            };
        };
        let mut state = workspace.lock()?;
        if state.closed || expected_revision != Some(state.revision) {
            return Err(changed());
        }
        if state.mode != Mode::Human {
            return Err(paused(label, Resume::Person));
        }
        if state.in_flight != 0 {
            return Err(busy(label));
        }
        state.in_flight = 1;
        let revision = state.revision;
        drop(state);
        Ok(Some(Operation {
            workspace,
            revision,
            owner: None,
            changes_page: true,
            observes: false,
            navigates: false,
            resumes: false,
            activity: None,
            completed: false,
            notice: None,
        }))
    }

    /// Decide a pane's placement against its tab's state and make the native
    /// moves inside that decision, under the tab's lock (t-66513). A tab still
    /// at the revision the window placed it by is shown for the person's eyes
    /// only when they watch it — whoever holds it, the shield takes their
    /// input — and for their hand only while it is in their hands; anything
    /// else stays hidden. A tab that is no work browser is the person's own.
    pub(crate) fn with_placement<T>(
        &self,
        label: &str,
        expected_revision: Option<u64>,
        watching: bool,
        operation: impl FnOnce(Placement) -> Result<T, String>,
    ) -> Result<T, String> {
        let shown = |current: bool, persons: bool| match (current, watching, persons) {
            (false, _, _) => Placement::Hidden,
            (true, true, _) => Placement::Watch,
            (true, false, true) => Placement::Control,
            (true, false, false) => Placement::Hidden,
        };
        let Some(workspace) = self.get(label)? else {
            return operation(shown(expected_revision.is_none(), true));
        };
        let state = workspace.lock()?;
        operation(shown(
            !state.closed && expected_revision == Some(state.revision),
            state.mode == Mode::Human,
        ))
    }

    #[cfg(test)]
    pub(crate) fn transition(
        &self,
        label: &str,
        target: Mode,
        expected_revision: u64,
    ) -> Result<Transition, String> {
        self.transition_owned(label, target, expected_revision, None)
    }

    pub(crate) fn transition_owned(
        &self,
        label: &str,
        target: Mode,
        expected_revision: u64,
        owner: Option<&str>,
    ) -> Result<Transition, String> {
        if target == Mode::Pausing {
            return Err(mode_invalid());
        }
        let held = self.lock()?;
        let workspace = held.get(label).cloned().ok_or_else(missing)?;
        let mut state = workspace.lock()?;
        if state.closed || state.revision != expected_revision {
            return Err(changed());
        }
        if target == Mode::Agent && owner.or(state.owner.as_deref()).is_none() {
            return Err(owner_required());
        }
        let activity = match state.owner.as_deref() {
            Some(owner) => self
                .actors
                .lock()
                .map_err(|_| unavailable(AGENTS_UNREADABLE))?
                .get(owner)
                .cloned(),
            None => None,
        };
        next_revision(&mut state)?;
        state.mode = Mode::Pausing;
        state.handing = Some(target);
        state.needs_observation = true;
        let revision = state.revision;
        drop(state);
        drop(held);
        // A look still waiting on the tab's page hears at once that the tab
        // left its agent's hands (t-67064).
        workspace.drained.notify_waiters();
        Ok(Transition {
            workspace,
            activity,
            owner: owner.map(str::to_string),
            revision,
            target,
            fenced: target == Mode::Paused,
            regrant: false,
            completed: false,
            notice: None,
        })
    }

    /// The person turns the session's regrant off on a tab's bar (t-66200):
    /// a pause its owner's change of unknown outcome leaves waits for the
    /// person again, the one standing now included.
    pub(crate) fn end_regrant(&self, label: &str) -> Result<Snapshot, String> {
        let workspace = self.get(label)?.ok_or_else(missing)?;
        let mut state = workspace.lock()?;
        state.regrant = false;
        Ok(workspace.snapshot(&state))
    }

    pub(crate) fn reserve_popup(
        &self,
        label: &str,
        expected_revision: u64,
    ) -> Result<bool, String> {
        let Some(workspace) = self.get(label)? else {
            return Ok(false);
        };
        let mut state = workspace.lock()?;
        if state.closed
            || state.mode != Mode::Agent
            || state.revision != expected_revision
            || state.popup_pending
        {
            return Ok(false);
        }
        state.popup_pending = true;
        Ok(true)
    }

    pub(crate) fn release_popup(&self, label: &str) -> Result<(), String> {
        if let Some(workspace) = self.get(label)? {
            workspace.lock()?.popup_pending = false;
        }
        Ok(())
    }

    pub(crate) fn with_agent_popup(
        &self,
        label: &str,
        expected_revision: u64,
        navigate: impl FnOnce() -> Result<(), String>,
    ) -> Result<bool, String> {
        let held = self.lock()?;
        let Some(workspace) = held.get(label) else {
            return Ok(false);
        };
        workspace.lock()?.popup_pending = false;
        let owner = workspace.lock()?.owner.clone();
        let Some(owner) = owner else {
            return Ok(false);
        };
        if owner_ready(&held, &owner).is_err() {
            return Ok(false);
        }
        let mut state = workspace.lock()?;
        if state.closed || state.mode != Mode::Agent || state.revision != expected_revision {
            return Ok(false);
        }
        state.needs_observation = true;
        next_revision(&mut state)?;
        let navigated = navigate();
        if navigated.is_err() {
            state.mode = Mode::Paused;
        }
        drop(state);
        // A look still waiting on the tab's old page hears at once that the
        // tab moved under it (t-67064).
        workspace.drained.notify_waiters();
        navigated.map(|()| true)
    }

    #[cfg(test)]
    pub(crate) fn begin_snapshot(&self, label: &str) -> Result<Frame, String> {
        self.begin_snapshot_for(label, true)
    }

    fn frame_slot(&self, label: &str) -> Result<Arc<FrameSlot>, String> {
        let mut frames = self
            .frames
            .lock()
            .map_err(|_| unavailable(WATCH_UNREADABLE))?;
        if frames.len() >= WORKSPACE_CAP && !frames.contains_key(label) {
            return Err(limit("관전 화면 수"));
        }
        Ok(frames.entry(label.to_string()).or_default().clone())
    }

    /// The tab's capture lease now, or busy.
    pub(crate) fn begin_snapshot_for(&self, label: &str, spectator: bool) -> Result<Frame, String> {
        take_frame(&self.frame_slot(label)?, spectator)?.map_err(|_| snapshot_busy())
    }

    /// The tab's capture lease, waited for instead of refused (t-66513). A
    /// watch frame that meets another capture in flight — an agent's
    /// screenshot, a step's evidence — or its own second not yet over waits
    /// for its turn, at most [`FRAME_INTERVAL`], and starts after every agent
    /// capture waiting beside it; one watch frame waits at a time. An agent
    /// capture waits the same at most for the capture in flight.
    pub(crate) async fn lease_frame(&self, label: &str, spectator: bool) -> Result<Frame, String> {
        let slot = self.frame_slot(label)?;
        let _waiting = Waiting::enter(slot.clone(), spectator)?;
        let deadline = Instant::now() + FRAME_INTERVAL;
        loop {
            let released = slot.released.notified();
            tokio::pin!(released);
            released.as_mut().enable();
            let wake = match take_frame(&slot, spectator)? {
                Ok(frame) => return Ok(frame),
                Err(Turn::Released) => deadline,
                Err(Turn::At(at)) => at.min(deadline),
            };
            if Instant::now() >= deadline {
                return Err(snapshot_busy());
            }
            tokio::select! {
                () = released => {}
                () = tokio::time::sleep_until(wake.into()) => {}
            }
        }
    }

    /// An agent capture's turn at the tab's lease (t-66513): it waits — at
    /// most [`FRAME_INTERVAL`] — for the capture in flight to end, and holds
    /// every watch frame off while it is held, so the lease a step's evidence
    /// takes next under its own admission is the agent's. A turn that waited
    /// out its bound is given all the same: the admission then meets the busy
    /// lease, as it did before any waiting.
    pub(crate) async fn agent_turn(&self, label: &str) -> Result<FrameTurn, String> {
        let slot = self.frame_slot(label)?;
        let waiting = Waiting::enter(slot.clone(), false)?;
        let deadline = tokio::time::Instant::now() + FRAME_INTERVAL;
        loop {
            let released = slot.released.notified();
            tokio::pin!(released);
            released.as_mut().enable();
            let in_flight = slot.lock()?.in_flight;
            if !in_flight || tokio::time::timeout_at(deadline, released).await.is_err() {
                return Ok(FrameTurn { _waiting: waiting });
            }
        }
    }

    pub(crate) fn navigation_check(
        &self,
        label: &str,
        actor: Option<&str>,
    ) -> Result<EvidenceCheck, String> {
        let Some(workspace) = self.get(label)? else {
            return Ok(EvidenceCheck(None));
        };
        let control = {
            let state = workspace.lock()?;
            let control = StepControl {
                label: label.to_string(),
                owner: actor.ok_or_else(actor_required)?.to_string(),
                revision: state.revision,
            };
            if !agent_holds(&state, &control) {
                return Err(stale(label));
            }
            control
        };
        Ok(EvidenceCheck(Some((workspace, control))))
    }

    pub(crate) fn admit_evidence(
        &self,
        label: &str,
        control: Option<&StepControl>,
    ) -> Result<Option<(Frame, EvidenceCheck)>, String> {
        match (self.get(label)?, control) {
            (None, None) => Ok(Some((
                self.begin_snapshot_for(label, false)?,
                EvidenceCheck(None),
            ))),
            (Some(workspace), Some(control)) if control.label == label => {
                let state = workspace.lock()?;
                if !agent_holds(&state, control) {
                    return Ok(None);
                }
                let frame = self.begin_snapshot_for(label, false)?;
                drop(state);
                Ok(Some((
                    frame,
                    EvidenceCheck(Some((workspace, control.clone()))),
                )))
            }
            _ => Ok(None),
        }
    }

    pub(crate) fn publish(
        &self,
        label: &str,
        expected_revision: u64,
        publish: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        let held = self.lock()?;
        let workspace = held.get(label).ok_or_else(changed)?;
        let state = workspace.lock()?;
        if state.closed || state.revision != expected_revision {
            return Err(changed());
        }
        publish()
    }

    pub(crate) fn cancel_unpublished(
        &self,
        label: &str,
        expected_revision: u64,
        published: impl FnOnce() -> bool,
    ) -> Result<bool, String> {
        let mut held = self.lock()?;
        if published() {
            return Ok(false);
        }
        let Some(workspace) = held.get(label).cloned() else {
            return Ok(true);
        };
        let mut state = workspace.lock()?;
        if state.revision != expected_revision {
            return Err(changed());
        }
        state.closed = true;
        state.mode = Mode::Paused;
        state.needs_observation = true;
        drop(state);
        held.remove(label);
        workspace.drained.notify_waiters();
        Ok(true)
    }

    pub(crate) fn begin_close(
        &self,
        label: &str,
        expected_revision: Option<u64>,
    ) -> Result<Option<Closing>, String> {
        let Some(workspace) = self.get(label)? else {
            return Ok(None);
        };
        let mut state = workspace.lock()?;
        if state.closed || expected_revision.is_some_and(|expected| expected != state.revision) {
            return Err(changed());
        }
        let revision = state.revision;
        next_revision(&mut state)?;
        state.closed = true;
        state.mode = Mode::Paused;
        state.needs_observation = true;
        drop(state);
        workspace.drained.notify_waiters();
        Ok(Some(Closing {
            workspace,
            revision,
            completed: false,
            notice: None,
        }))
    }

    pub(crate) fn closed_at(&self, label: &str, revision: u64) -> Result<bool, String> {
        Ok(self
            .closed
            .lock()
            .map_err(|_| unavailable(CLOSES_UNREADABLE))?
            .get(label)
            == Some(&revision))
    }

    pub(crate) fn confirm_closed(&self, label: &str, revision: u64) -> Result<(), String> {
        let mut closed = self
            .closed
            .lock()
            .map_err(|_| unavailable(CLOSES_UNREADABLE))?;
        if closed.len() >= WORKSPACE_CAP && !closed.contains_key(label) {
            closed.pop_first();
        }
        closed.insert(label.to_string(), revision);
        Ok(())
    }

    pub(crate) fn remove(&self, label: &str) -> Result<(), String> {
        let workspace = self.lock()?.remove(label);
        if let Some(workspace) = workspace {
            workspace.lock()?.closed = true;
            workspace.drained.notify_waiters();
        }
        self.frames
            .lock()
            .map_err(|_| unavailable(WATCH_UNREADABLE))?
            .remove(label);
        for activity in self
            .actors
            .lock()
            .map_err(|_| unavailable(AGENTS_UNREADABLE))?
            .values()
        {
            activity
                .state
                .lock()
                .map_err(|_| unavailable(AGENTS_UNREADABLE))?
                .uncertain
                .remove(label);
        }
        Ok(())
    }
}

pub(crate) struct Closing {
    workspace: Arc<Workspace>,
    revision: u64,
    completed: bool,
    notice: Option<Notice>,
}

impl Closing {
    pub(crate) fn notifying(mut self, notice: impl Fn(Snapshot) + Send + Sync + 'static) -> Self {
        self.notice = Some(Box::new(notice));
        self
    }

    pub(crate) fn finish(mut self) {
        self.completed = true;
    }
}

impl Drop for Closing {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        let snapshot = if let Ok(mut state) = self.workspace.lock() {
            if state.revision <= self.revision {
                return;
            }
            state.closed = false;
            state.mode = Mode::Paused;
            state.needs_observation = true;
            state.revision = state.revision.saturating_add(1);
            Some(self.workspace.snapshot(&state))
        } else {
            None
        };
        self.workspace.drained.notify_waiters();
        if let (Some(notice), Some(snapshot)) = (&self.notice, snapshot) {
            notice(snapshot);
        }
    }
}

pub(crate) struct ActorOperation {
    activity: Arc<ActorActivity>,
    label: Option<String>,
    workspaces: Vec<Arc<Workspace>>,
    completed: bool,
    notice: Option<Notice>,
}

impl ActorOperation {
    pub(crate) fn notifying(mut self, notice: impl Fn(Snapshot) + Send + Sync + 'static) -> Self {
        self.notice = Some(Box::new(notice));
        self
    }

    pub(crate) fn settle(mut self, dispatch: Dispatch) {
        self.completed = dispatch != Dispatch::Uncertain;
    }
}

impl Drop for ActorOperation {
    fn drop(&mut self) {
        if let Ok(mut state) = self.activity.state.lock() {
            state.in_flight = state.in_flight.saturating_sub(1);
            if !self.completed
                && let Some(label) = &self.label
            {
                state.uncertain.insert(label.clone());
            }
        }
        if !self.completed {
            // A change of unknown outcome on one of the actor's own work
            // browsers (t-66200): a look may lift each pause it leaves on a tab
            // that stood in the actor's hands — or under the pause its own tab
            // left — and none it leaves after a change on any other tab.
            let own = self.label.as_ref().is_some_and(|label| {
                self.workspaces
                    .iter()
                    .any(|workspace| workspace.label == *label)
            });
            for workspace in &self.workspaces {
                if let Ok(mut state) = workspace.lock()
                    && !state.closed
                {
                    let agents = own
                        && (state.mode == Mode::Agent
                            || state.uncertain_at == Some(state.revision));
                    state.mode = Mode::Paused;
                    state.needs_observation = true;
                    state.revision = state.revision.saturating_add(1);
                    state.uncertain_at = agents.then_some(state.revision);
                    let snapshot = workspace.snapshot(&state);
                    drop(state);
                    workspace.drained.notify_waiters();
                    if let Some(notice) = &self.notice {
                        notice(snapshot);
                    }
                }
            }
        }
        self.activity.drained.notify_waiters();
    }
}

pub(crate) struct Frame(Arc<FrameSlot>);
impl Drop for Frame {
    fn drop(&mut self) {
        if let Ok(mut state) = self.0.lock() {
            state.in_flight = false;
        }
        self.0.released.notify_waiters();
    }
}

pub(crate) struct Operation {
    workspace: Arc<Workspace>,
    revision: u64,
    owner: Option<String>,
    changes_page: bool,
    observes: bool,
    /// A navigation the window must see settle before the tab changes hands
    /// (`navigates`): a pause it leaves is the person's to lift (t-66200).
    navigates: bool,
    /// The owner's look at a tab its own uncertain change paused, under the
    /// session's regrant: completing, it lifts the pause (t-66200).
    resumes: bool,
    /// The owner's requests, whose record of uncertain targets a lifting look
    /// clears of its own tab.
    activity: Option<Arc<ActorActivity>>,
    completed: bool,
    notice: Option<Notice>,
}

impl Operation {
    pub(crate) fn notifying(mut self, notice: impl Fn(Snapshot) + Send + Sync + 'static) -> Self {
        self.notice = Some(Box::new(notice));
        self
    }

    pub(crate) fn step_control(&self) -> Option<StepControl> {
        Some(StepControl {
            label: self.workspace.label.clone(),
            owner: self.owner.clone()?,
            revision: self.revision,
        })
    }

    /// Issue the read's captured revision only while the same actor still
    /// owns the same Agent control interval. A transition during the await
    /// makes the whole observation unusable, including an otherwise good PNG.
    /// A look that lifts a pause (t-66200) issues the revision it lifts the
    /// tab to — after the change it settles — and does so under the person's
    /// stop's own lock, so no tab comes back into an agent's hands after the
    /// person stopped.
    pub(crate) fn finish_observation(self) -> Result<u64, String> {
        if self.resumes {
            return crate::computer_use::guard::unless_persons_stop(|| self.observed())
                .unwrap_or_else(|| Err(PERSONS_STOPPED.to_string()));
        }
        self.observed()
    }

    fn observed(mut self) -> Result<u64, String> {
        let mut state = self.workspace.lock()?;
        let lifts = self.lifts(&state);
        if let Some(refusal) = self.refused(&state, lifts) {
            return Err(refusal);
        }
        if lifts {
            if let Some(activity) = &self.activity {
                activity
                    .state
                    .lock()
                    .map_err(|_| unavailable(AGENTS_UNREADABLE))?
                    .uncertain
                    .remove(&self.workspace.label);
            }
            next_revision(&mut state)?;
            state.mode = Mode::Agent;
            state.uncertain_at = None;
        }
        state.needs_observation = false;
        // The look its agent's next change without a revision is judged at
        // (t-67064).
        let revision = state.revision;
        state.seen = self.owner.clone().map(|owner| (owner, revision));
        self.completed = true;
        Ok(revision)
    }

    /// Whether this look lifts the pause it was let in under (t-66200).
    fn lifts(&self, state: &State) -> bool {
        self.resumes && state.mode == Mode::Paused && resume_of(state) == Resume::Observe
    }

    /// The refusal this look's completion meets as the tab stands now, or
    /// none: the one reading the look completes by and is answered early by
    /// ([`Self::taken`]). A tab the person's hand took under it is said by
    /// name (t-67064); any other move is stale.
    fn refused(&self, state: &State, lifts: bool) -> Option<String> {
        let label = &self.workspace.label;
        if !self.observes || state.closed || state.owner != self.owner || self.owner.is_none() {
            return Some(stale(label));
        }
        if state.revision != self.revision {
            return Some(if persons_hand(state) {
                person_controlled(label)
            } else {
                stale(label)
            });
        }
        (state.mode != Mode::Agent && !lifts).then(|| out_of_hands(label, state))
    }

    /// Resolves once this look's tab leaves its agent's hands while its page
    /// is still asked (t-67064) — the person takes it, a pause or the person's
    /// stop holds it, a change of unknown outcome pauses it, it closes — with
    /// the refusal its completion would meet then. Never while the tab stands
    /// as the look was let in.
    async fn taken(&self) -> String {
        loop {
            let notified = self.workspace.drained.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let state = match self.workspace.lock() {
                    Ok(state) => state,
                    Err(error) => return error,
                };
                if let Some(refusal) = self.refused(&state, self.lifts(&state)) {
                    return refusal;
                }
            }
            notified.await;
        }
    }

    pub(crate) fn finish(self, succeeded: bool) {
        self.settle(Dispatch::of(succeeded));
    }

    /// Settle the request as its dispatch went. Answers the revision the
    /// agent's next change may name (t-67064): a change that completed and
    /// left its tab in the same agent's hands, at the revision it was let in
    /// at, wanting no fresh look — what the door stamps on the change's
    /// answer. A look's revision is its completion's ([`Self::finish_observation`]).
    pub(crate) fn settle(mut self, dispatch: Dispatch) -> Option<u64> {
        let mut next = None;
        if dispatch != Dispatch::NotSent
            && let Ok(mut state) = self.workspace.lock()
            && !state.closed
            && state.revision == self.revision
            && state.mode == Mode::Agent
            && state.owner == self.owner
        {
            if self.observes {
                state.needs_observation = dispatch == Dispatch::Uncertain;
            } else if self.changes_page
                && dispatch == Dispatch::Completed
                && !state.needs_observation
            {
                next = Some(state.revision);
            }
        }
        self.completed = dispatch != Dispatch::Uncertain || !self.changes_page;
        next
    }
}

/// A look's page work, unless its tab leaves its agent's hands first
/// (t-67064): then the look answers at once with who holds the tab now — a
/// page that never answers no longer holds the agent until it times out —
/// and its page script is let go; the handoff's page fence still waits for
/// it. Anything but a look on a work browser is waited for whole: a change
/// in flight is the handoff's to wait for.
pub(crate) async fn unless_taken<T>(
    operation: Option<&Operation>,
    work: impl std::future::Future<Output = T>,
) -> Result<T, String> {
    let Some(look) = operation.filter(|operation| operation.observes) else {
        return Ok(work.await);
    };
    tokio::select! {
        biased;
        done = work => Ok(done),
        refusal = look.taken() => Err(refusal),
    }
}

impl Drop for Operation {
    fn drop(&mut self) {
        let snapshot = if let Ok(mut state) = self.workspace.lock() {
            state.in_flight = state.in_flight.saturating_sub(1);
            if !state.closed
                && !self.completed
                && self.observes
                && state.revision == self.revision
                && state.mode == Mode::Agent
            {
                state.needs_observation = true;
            }
            if !state.closed && !self.completed && self.changes_page {
                // The owner's own change of unknown outcome (t-66200): a look
                // may lift the pause it leaves — unless the person's handoff
                // was already on its way, the change was a navigation, or the
                // change was the person's own.
                let agents = self.owner.is_some() && state.mode == Mode::Agent && !self.navigates;
                state.mode = Mode::Paused;
                state.needs_observation = true;
                state.revision = state.revision.saturating_add(1);
                state.uncertain_at = agents.then_some(state.revision);
            }
            Some(self.workspace.snapshot(&state))
        } else {
            None
        };
        self.workspace.drained.notify_waiters();
        if let (Some(notice), Some(snapshot)) = (&self.notice, snapshot) {
            notice(snapshot);
        }
    }
}

pub(crate) struct Transition {
    workspace: Arc<Workspace>,
    activity: Option<Arc<ActorActivity>>,
    owner: Option<String>,
    revision: u64,
    target: Mode,
    fenced: bool,
    /// The person's 「다시 맡기기」 on the bar (t-66200): handing the tab
    /// back turns the session's regrant on.
    regrant: bool,
    completed: bool,
    notice: Option<Notice>,
}

impl Transition {
    pub(crate) fn notifying(mut self, notice: impl Fn(Snapshot) + Send + Sync + 'static) -> Self {
        self.notice = Some(Box::new(notice));
        self
    }

    /// Whether this handoff is the person's 「다시 맡기기」 on the tab's bar —
    /// the one press that turns the session's regrant on (t-66200). Another
    /// road back to the agent (the band's resume) leaves it as it stood.
    pub(crate) fn regranting(mut self, regrant: bool) -> Self {
        self.regrant = regrant;
        self
    }

    pub(crate) fn snapshot(&self) -> Result<Snapshot, String> {
        Ok(self.workspace.snapshot(&*self.workspace.lock()?))
    }

    pub(crate) async fn wait(&self) -> Result<(), String> {
        loop {
            let notified = self.workspace.drained.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let activity_notified = self
                .activity
                .as_ref()
                .map(|activity| activity.drained.notified());
            tokio::pin!(activity_notified);
            if let Some(notified) = activity_notified.as_mut().as_pin_mut() {
                notified.enable();
            }
            let actor_drained = self.actor_drained()?;
            {
                let state = self.workspace.lock()?;
                if state.closed || state.revision != self.revision || state.mode != Mode::Pausing {
                    return Err(changed());
                }
                if state.in_flight == 0 && state.capturing == 0 && actor_drained {
                    return Ok(());
                }
            }
            tokio::select! {
                _ = notified => {},
                _ = async {
                    if let Some(notified) = activity_notified.as_mut().as_pin_mut() { notified.await; }
                    else { std::future::pending::<()>().await; }
                } => {}
            }
        }
    }

    fn actor_drained(&self) -> Result<bool, String> {
        self.activity.as_ref().map_or(Ok(true), |activity| {
            activity
                .state
                .lock()
                .map(|state| state.in_flight == 0)
                .map_err(|_| unavailable(AGENTS_UNREADABLE))
        })
    }

    pub(crate) fn fence_labels(&self) -> Result<Vec<String>, String> {
        if !self.actor_drained()? {
            return Err(changed());
        }
        let mut labels = BTreeSet::from([self.workspace.label.clone()]);
        if let Some(activity) = &self.activity {
            labels.extend(
                activity
                    .state
                    .lock()
                    .map_err(|_| unavailable(AGENTS_UNREADABLE))?
                    .uncertain
                    .iter()
                    .cloned(),
            );
        }
        Ok(labels.into_iter().collect())
    }

    pub(crate) fn confirm_fence(&mut self) -> Result<(), String> {
        if !self.actor_drained()? {
            return Err(changed());
        }
        let state = self.workspace.lock()?;
        if state.closed
            || state.revision != self.revision
            || state.mode != Mode::Pausing
            || state.in_flight != 0
        {
            return Err(changed());
        }
        if let Some(activity) = &self.activity {
            activity
                .state
                .lock()
                .map_err(|_| unavailable(AGENTS_UNREADABLE))?
                .uncertain
                .clear();
        }
        self.fenced = true;
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<Snapshot, String> {
        if !self.actor_drained()? {
            return Err(changed());
        }
        let snapshot = {
            let mut state = self.workspace.lock()?;
            if state.closed
                || state.revision != self.revision
                || state.mode != Mode::Pausing
                || state.in_flight != 0
                || state.capturing != 0
                || !self.fenced
            {
                return Err(changed());
            }
            next_revision(&mut state)?;
            state.mode = self.target;
            state.handing = None;
            state.person_controlled |= self.target == Mode::Human;
            // The person's 「다시 맡기기」 turns the session's regrant on, their
            // hand turns it off (t-66200).
            match self.target {
                Mode::Agent => state.regrant |= self.regrant,
                Mode::Human => state.regrant = false,
                Mode::Paused | Mode::Pausing => {}
            }
            if let Some(owner) = self.owner.take() {
                // A tab handed to another agent keeps no look of its last
                // owner's for the new one to go on by (t-67064).
                if state.owner.as_deref() != Some(owner.as_str()) {
                    state.seen = None;
                }
                state.owner = Some(owner);
            }
            self.workspace.snapshot(&state)
        };
        self.completed = true;
        if let Some(notice) = &self.notice {
            notice(snapshot.clone());
        }
        Ok(snapshot)
    }
}

impl Drop for Transition {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        let snapshot = if let Ok(mut state) = self.workspace.lock() {
            if state.closed || state.revision != self.revision || state.mode != Mode::Pausing {
                return;
            }
            state.mode = Mode::Paused;
            state.handing = None;
            state.needs_observation = true;
            state.revision = state.revision.saturating_add(1);
            Some(self.workspace.snapshot(&state))
        } else {
            None
        };
        self.workspace.drained.notify_waiters();
        if let (Some(notice), Some(snapshot)) = (&self.notice, snapshot) {
            notice(snapshot);
        }
    }
}

#[cfg(test)]
mod review_regressions {
    use super::*;

    fn owned() -> Workspaces {
        let workspaces = Workspaces::default();
        workspaces
            .register("browser-1", Some("term-1"), false)
            .unwrap();
        workspaces
    }

    #[test]
    fn a_failed_mutation_pauses_before_a_late_page_reply() {
        let workspaces = owned();
        workspaces
            .begin("browser-1", Some("term-1"), "read")
            .unwrap()
            .unwrap()
            .finish(true);
        workspaces
            .begin("browser-1", Some("term-1"), "click")
            .unwrap()
            .unwrap()
            .finish(false);
        assert_eq!(
            workspaces.snapshot("browser-1").unwrap().unwrap().mode,
            Mode::Paused
        );
    }

    #[test]
    fn an_unbound_workspace_is_paused_without_a_restore_flag() {
        let workspaces = Workspaces::default();
        assert_eq!(
            workspaces.register("browser-1", None, false).unwrap().mode,
            Mode::Paused
        );
    }

    #[test]
    fn an_unbound_workspace_cannot_be_resumed_by_the_first_agent() {
        let workspaces = Workspaces::default();
        workspaces.register("browser-1", None, false).unwrap();
        assert!(workspaces.transition("browser-1", Mode::Agent, 1).is_err());
    }

    #[test]
    fn arbitrary_eval_cannot_schedule_a_write_in_a_managed_workspace() {
        let workspaces = owned();
        workspaces
            .begin("browser-1", Some("term-1"), "read")
            .unwrap()
            .unwrap()
            .finish(true);
        assert!(
            workspaces
                .begin("browser-1", Some("term-1"), "eval")
                .is_err()
        );
    }

    #[test]
    fn an_unknown_verb_is_refused_before_an_operation_is_created() {
        let workspaces = owned();
        workspaces
            .begin("browser-1", Some("term-1"), "read")
            .unwrap()
            .unwrap()
            .finish(true);
        assert!(
            workspaces
                .begin("browser-1", Some("term-1"), "unknown")
                .is_err()
        );
    }

    #[test]
    fn dropping_a_transition_leaves_actionable_paused_state() {
        let workspaces = owned();
        drop(workspaces.transition("browser-1", Mode::Human, 1).unwrap());
        assert_eq!(
            workspaces.snapshot("browser-1").unwrap().unwrap().mode,
            Mode::Paused
        );
    }

    #[test]
    fn one_paused_tab_blocks_its_owners_other_tab_mutations() {
        let workspaces = owned();
        workspaces
            .register("browser-2", Some("term-1"), false)
            .unwrap();
        workspaces
            .begin("browser-2", Some("term-1"), "read")
            .unwrap()
            .unwrap()
            .finish(true);
        workspaces
            .transition("browser-1", Mode::Paused, 1)
            .unwrap()
            .finish()
            .unwrap();
        assert!(
            workspaces
                .begin("browser-2", Some("term-1"), "click")
                .is_err()
        );
        assert!(
            workspaces
                .begin("browser-person", Some("term-1"), "goto")
                .is_err()
        );
    }

    #[test]
    fn nil_actors_cannot_use_unmanaged_tabs() {
        assert!(
            Workspaces::default()
                .begin("browser-person", None, "read")
                .is_err()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opened() -> Workspaces {
        let workspaces = Workspaces::default();
        workspaces
            .register("browser-1", Some("pane-a"), false)
            .unwrap();
        workspaces
    }

    fn observe(workspaces: &Workspaces) {
        workspaces
            .begin("browser-1", Some("pane-a"), "read")
            .unwrap()
            .unwrap()
            .finish(true);
    }

    async fn change(workspaces: &Workspaces, target: Mode) -> Snapshot {
        let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        let mut transition = workspaces
            .transition("browser-1", target, revision)
            .unwrap();
        transition.wait().await.unwrap();
        if target != Mode::Paused {
            transition.confirm_fence().unwrap();
        }
        transition.finish().unwrap()
    }

    #[test]
    fn a_new_workspace_hides_native_input_and_requires_a_fresh_observation() {
        let workspaces = opened();
        assert!(!workspaces.allows_person("browser-1").unwrap());
        assert!(
            workspaces
                .begin("browser-1", Some("pane-a"), "click")
                .is_err()
        );
        observe(&workspaces);
        workspaces
            .begin("browser-1", Some("pane-a"), "click")
            .unwrap()
            .unwrap()
            .finish(true);
    }

    #[test]
    fn an_unmanaged_person_tab_is_not_silently_registered() {
        let workspaces = opened();
        assert!(
            workspaces
                .begin("browser-2", Some("pane-a"), "read")
                .unwrap()
                .is_none()
        );
        assert!(workspaces.allows_person("browser-2").unwrap());
        assert_eq!(workspaces.snapshots().unwrap().len(), 1);
    }

    #[test]
    fn a_restored_workspace_starts_paused_without_an_old_actor() {
        let workspaces = Workspaces::default();
        let snapshot = workspaces.register("browser-1", None, true).unwrap();
        assert_eq!(snapshot.mode, Mode::Paused);
        assert!(
            workspaces
                .begin("browser-1", Some("pane-a"), "goto")
                .is_err()
        );
        assert!(!workspaces.allows_person("browser-1").unwrap());
    }

    #[tokio::test]
    async fn observing_a_paused_workspace_does_not_claim_its_next_run() {
        let workspaces = Workspaces::default();
        workspaces.register("browser-1", None, true).unwrap();
        assert!(
            workspaces
                .begin("browser-1", Some("pane-b"), "read")
                .is_err()
        );
        assert!(workspaces.transition("browser-1", Mode::Agent, 1).is_err());
        let mut transition = workspaces
            .transition_owned("browser-1", Mode::Agent, 1, Some("pane-a"))
            .unwrap();
        transition.wait().await.unwrap();
        transition.confirm_fence().unwrap();
        transition.finish().unwrap();
        observe(&workspaces);
        workspaces
            .begin("browser-1", Some("pane-a"), "click")
            .unwrap()
            .unwrap()
            .finish(true);
        assert!(
            workspaces
                .begin("browser-1", Some("pane-b"), "read")
                .is_err()
        );
    }

    #[test]
    fn another_actor_cannot_read_or_write_an_owned_workspace() {
        let workspaces = opened();
        for verb in ["read", "marks", "eval", "click", "close"] {
            assert!(workspaces.begin("browser-1", Some("pane-b"), verb).is_err());
            assert!(workspaces.begin("browser-1", None, verb).is_err());
        }
    }

    #[tokio::test]
    async fn takeover_waits_for_the_in_flight_operation_and_blocks_new_input() {
        let workspaces = opened();
        observe(&workspaces);
        let operation = workspaces
            .begin("browser-1", Some("pane-a"), "click")
            .unwrap()
            .unwrap();
        let mut transition = workspaces.transition("browser-1", Mode::Human, 1).unwrap();
        assert_eq!(transition.snapshot().unwrap().mode, Mode::Pausing);
        assert!(!workspaces.allows_person("browser-1").unwrap());
        assert!(
            workspaces
                .begin("browser-1", Some("pane-a"), "eval")
                .is_err()
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(5), transition.wait())
                .await
                .is_err()
        );
        operation.finish(true);
        transition.wait().await.unwrap();
        transition.confirm_fence().unwrap();
        assert_eq!(transition.finish().unwrap().mode, Mode::Human);
        assert!(workspaces.allows_person("browser-1").unwrap());
    }

    #[tokio::test]
    async fn every_page_mutation_including_eval_is_blocked_during_human_control() {
        let workspaces = opened();
        change(&workspaces, Mode::Human).await;
        for verb in [
            "eval", "click", "type", "fill", "goto", "close", "viewport", "scroll", "find",
        ] {
            assert!(
                workspaces.begin("browser-1", Some("pane-a"), verb).is_err(),
                "{verb}"
            );
        }
        assert!(
            workspaces
                .begin("browser-1", Some("pane-a"), "read")
                .is_err()
        );
        assert!(
            workspaces
                .snapshot("browser-1")
                .unwrap()
                .unwrap()
                .needs_observation
        );
    }

    #[tokio::test]
    async fn a_resume_requires_observation_after_the_control_transition() {
        let workspaces = opened();
        observe(&workspaces);
        change(&workspaces, Mode::Human).await;
        assert!(
            workspaces
                .begin("browser-1", Some("pane-a"), "read")
                .is_err()
        );
        change(&workspaces, Mode::Agent).await;
        assert!(
            workspaces
                .begin("browser-1", Some("pane-a"), "fill")
                .is_err()
        );
        observe(&workspaces);
        workspaces
            .begin("browser-1", Some("pane-a"), "fill")
            .unwrap()
            .unwrap()
            .finish(true);
    }

    #[test]
    fn a_stale_control_response_cannot_replace_the_newer_transition() {
        let workspaces = opened();
        let old = workspaces.transition("browser-1", Mode::Human, 1).unwrap();
        assert!(workspaces.transition("browser-1", Mode::Agent, 1).is_err());
        let newer = workspaces.transition("browser-1", Mode::Paused, 2).unwrap();
        assert!(old.finish().is_err());
        assert_eq!(newer.finish().unwrap().mode, Mode::Paused);
    }

    #[tokio::test]
    async fn a_cancelled_input_cannot_satisfy_a_pending_takeover() {
        let workspaces = opened();
        observe(&workspaces);
        let operation = workspaces
            .begin("browser-1", Some("pane-a"), "type")
            .unwrap()
            .unwrap();
        let transition = workspaces.transition("browser-1", Mode::Human, 1).unwrap();
        drop(operation);
        assert!(transition.wait().await.is_err());
        assert!(!workspaces.allows_person("browser-1").unwrap());
    }

    #[tokio::test]
    async fn closing_a_workspace_invalidates_held_operations_and_transitions() {
        let workspaces = opened();
        let transition = workspaces.transition("browser-1", Mode::Human, 1).unwrap();
        workspaces.remove("browser-1").unwrap();
        assert!(transition.wait().await.is_err());
        assert!(workspaces.snapshots().unwrap().is_empty());
    }

    /// The person's one stop pauses a work browser while its agent keeps
    /// sending input from another thread (t-62872): an input is admitted
    /// before the pause began — and the pause waits for it — or refused; none
    /// is admitted once the pause began, and the pause leaves nothing in
    /// flight.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_pause_racing_an_agents_input_admits_none_once_it_began() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        const ROUNDS: usize = 200;
        for _ in 0..ROUNDS {
            let workspaces = Arc::new(opened());
            observe(&workspaces);
            let before = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
            let paused = Arc::new(AtomicBool::new(false));
            let landed = Arc::new(AtomicUsize::new(0));
            let agent = {
                let workspaces = workspaces.clone();
                let paused = paused.clone();
                let landed = landed.clone();
                std::thread::spawn(move || {
                    let mut admitted = Vec::new();
                    while !paused.load(Ordering::Acquire) {
                        let seen = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
                        if let Ok(Some(operation)) =
                            workspaces.begin_at("browser-1", Some("pane-a"), "click", Some(seen))
                        {
                            admitted.push(operation.revision);
                            landed.fetch_add(1, Ordering::AcqRel);
                            operation.finish(true);
                        }
                    }
                    admitted
                })
            };
            // The pause comes while the agent's input is already landing.
            while landed.load(Ordering::Acquire) == 0 {
                std::thread::yield_now();
            }
            let transition = workspaces
                .transition("browser-1", Mode::Paused, before)
                .unwrap();
            transition.wait().await.unwrap();
            let left = transition.finish().unwrap();
            let again = Some(left.revision);
            let after = workspaces.begin_at("browser-1", Some("pane-a"), "click", again);
            paused.store(true, Ordering::Release);
            let admitted = agent.join().unwrap();
            assert!(!admitted.is_empty(), "the agent's input raced the pause");
            assert!(
                admitted.iter().all(|revision| *revision == before),
                "an input was admitted after the pause began at {before}: {admitted:?}"
            );
            assert_eq!(left.mode, Mode::Paused);
            assert_eq!(left.in_flight, 0, "the pause leaves nothing in flight");
            assert!(after.is_err(), "nothing is admitted once paused");
        }
    }

    /// The person's stop at the work browsers' door (t-62872): an agent that
    /// keeps sending input from another thread gets none in once the person's
    /// stop is made, though the window has not paused its browser yet — its
    /// walk is still on the way — and the pause that follows finds nothing in
    /// flight. The inputs admitted between the person's stop and the window's
    /// pause, over every round, are what the stop holds at nought.
    #[test]
    fn no_agent_input_gets_in_between_the_persons_stop_and_the_windows_pause() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use zerocode_core::computer_use::STOP_REASON_WINDOW;
        const ROUNDS: usize = 50;
        const TRIES_AFTER: usize = 64;
        let _hand = crate::tests::computer_desktop_wait::ONE_HAND
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (mut between, mut tried) = (0, 0);
        for _ in 0..ROUNDS {
            crate::computer_use::guard::lift();
            let workspaces = Arc::new(opened());
            observe(&workspaces);
            let stopped = Arc::new(AtomicBool::new(false));
            let done = Arc::new(AtomicBool::new(false));
            let landed = Arc::new(AtomicUsize::new(0));
            let tries_after = Arc::new(AtomicUsize::new(0));
            let agent = {
                let (workspaces, stopped, done) =
                    (workspaces.clone(), stopped.clone(), done.clone());
                let (landed, tries_after) = (landed.clone(), tries_after.clone());
                std::thread::spawn(move || {
                    let mut after = 0;
                    while !done.load(Ordering::Acquire) {
                        let late = stopped.load(Ordering::Acquire);
                        let seen = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
                        if let Ok(Some(operation)) = workspaces.begin_agents(
                            "browser-1",
                            Some("pane-a"),
                            "click",
                            Some(seen),
                        ) {
                            landed.fetch_add(1, Ordering::AcqRel);
                            after += usize::from(late);
                            operation.finish(true);
                        }
                        if late {
                            tries_after.fetch_add(1, Ordering::AcqRel);
                        }
                    }
                    after
                })
            };
            while landed.load(Ordering::Acquire) == 0 {
                std::thread::yield_now();
            }
            crate::computer_use::guard::stop(STOP_REASON_WINDOW);
            stopped.store(true, Ordering::Release);
            // The window's walk is still on the way: the agent keeps trying.
            while tries_after.load(Ordering::Acquire) < TRIES_AFTER {
                std::thread::yield_now();
            }
            let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
            let transition = workspaces
                .transition("browser-1", Mode::Paused, revision)
                .unwrap();
            runtime.block_on(transition.wait()).unwrap();
            let left = transition.finish().unwrap();
            done.store(true, Ordering::Release);
            between += agent.join().unwrap();
            tried += tries_after.load(Ordering::Acquire);
            crate::computer_use::guard::lift();
            assert_eq!(left.mode, Mode::Paused);
            assert_eq!(left.in_flight, 0, "the pause leaves nothing in flight");
        }
        assert!(
            tried >= ROUNDS * TRIES_AFTER,
            "the agent kept trying after every stop: {tried}"
        );
        assert_eq!(
            between, 0,
            "agent inputs admitted after the person's stop and before the window's pause, over {ROUNDS} rounds: {between}"
        );
    }

    /// While the person's stop stands (t-62872) an agent's `open` registers no
    /// work browser, and every verb an agent sends to a work browser — a read
    /// as much as an input — is refused in words that say the person stopped
    /// it. A browser that is no work browser is not the stop's, nor is an
    /// agent's own stop the person's; once the person's stop is lifted, the
    /// door opens again for a browser in the agent's hands.
    #[test]
    fn the_persons_stop_refuses_an_agents_open_and_every_verb_on_a_work_browser() {
        use zerocode_core::computer_use::{STOP_REASON_HOTKEY, STOP_REASON_REQUEST};
        fn person<T>(result: &Result<T, String>) -> bool {
            result
                .as_ref()
                .is_err_and(|error| error.starts_with("workspace_person_stopped:"))
        }
        let _hand = crate::tests::computer_desktop_wait::ONE_HAND
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let admitted = |result: Result<Option<Operation>, String>| {
            result.map(|operation| operation.map(|operation| operation.finish(true)).is_some())
        };
        crate::computer_use::guard::lift();
        let workspaces = opened();
        observe(&workspaces);
        let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        crate::computer_use::guard::adopt(STOP_REASON_HOTKEY);
        let open = workspaces.register_agents("browser-2", Some("pane-a"));
        let registered = workspaces.snapshot("browser-2").unwrap().is_some();
        let read = workspaces.begin_agents("browser-1", Some("pane-a"), "read", None);
        let read_refused = person(&read);
        let read = admitted(read);
        let click = workspaces.begin_agents("browser-1", Some("pane-a"), "click", Some(revision));
        let click_refused = person(&click);
        let click = admitted(click);
        let ordinary = admitted(workspaces.begin_agents("ordinary", Some("pane-a"), "read", None));
        crate::computer_use::guard::lift();
        crate::computer_use::guard::stop(STOP_REASON_REQUEST);
        let under_agents_stop =
            admitted(workspaces.begin_agents("browser-1", Some("pane-a"), "read", None));
        crate::computer_use::guard::lift();
        let lifted = admitted(workspaces.begin_agents("browser-1", Some("pane-a"), "read", None));
        let open_lifted = workspaces
            .register_agents("browser-3", Some("pane-a"))
            .map(|snapshot| snapshot.mode);
        assert!(
            person(&open) && !registered,
            "an agent's open while the person's stop stands registers no work browser: {:?}",
            open.map(|snapshot| snapshot.mode)
        );
        assert!(
            read_refused && click_refused,
            "a read and an input on a work browser are refused in the person's words: {read:?} {click:?}"
        );
        assert_eq!(
            ordinary,
            Ok(false),
            "a browser that is no work browser is not the stop's"
        );
        assert_eq!(
            under_agents_stop,
            Ok(true),
            "an agent's own stop is not the person's"
        );
        assert_eq!(lifted, Ok(true), "the lifted stop opens the door again");
        assert_eq!(open_lifted, Ok(Mode::Agent), "and an agent may open again");
    }

    #[test]
    fn simultaneous_commands_do_not_overlap_on_one_workspace() {
        let workspaces = opened();
        let operation = workspaces
            .begin("browser-1", Some("pane-a"), "read")
            .unwrap()
            .unwrap();
        assert!(
            workspaces
                .begin("browser-1", Some("pane-a"), "marks")
                .is_err()
        );
        operation.finish(true);
        assert_eq!(
            workspaces.snapshot("browser-1").unwrap().unwrap().in_flight,
            0
        );
    }
}

#[cfg(test)]
mod boundary_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn owned() -> Workspaces {
        let workspaces = Workspaces::default();
        workspaces
            .register("browser-1", Some("term-1"), false)
            .unwrap();
        workspaces
    }

    fn observe(workspaces: &Workspaces) {
        workspaces
            .begin("browser-1", Some("term-1"), "read")
            .unwrap()
            .unwrap()
            .finish(true);
    }

    fn change(workspaces: &Workspaces, mode: Mode) -> Snapshot {
        let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        let mut transition = workspaces.transition("browser-1", mode, revision).unwrap();
        if mode != Mode::Paused {
            transition.confirm_fence().unwrap();
        }
        transition.finish().unwrap()
    }

    #[test]
    fn human_control_refuses_every_registered_agent_verb_without_changing_visibility() {
        let workspaces = owned();
        change(&workspaces, Mode::Human);
        for row in zerocode_core::agent_browser::BROWSER_VERBS {
            assert!(
                workspaces
                    .begin("browser-1", Some("term-1"), row.word)
                    .is_err(),
                "{}",
                row.word
            );
            assert!(
                workspaces.allows_person("browser-1").unwrap(),
                "{}",
                row.word
            );
            assert_eq!(
                workspaces.snapshot("browser-1").unwrap().unwrap().in_flight,
                0
            );
        }
        assert!(
            !workspaces
                .allows_agent_metadata("browser-1", Some("term-1"))
                .unwrap()
        );
    }

    #[test]
    fn an_uncertain_mutation_requires_a_page_fence_before_human_control() {
        let workspaces = owned();
        observe(&workspaces);
        workspaces
            .begin("browser-1", Some("term-1"), "click")
            .unwrap()
            .unwrap()
            .finish(false);
        let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        let transition = workspaces
            .transition("browser-1", Mode::Human, revision)
            .unwrap();
        assert!(transition.finish().is_err());
        assert_eq!(
            workspaces.snapshot("browser-1").unwrap().unwrap().mode,
            Mode::Paused
        );
        assert!(!workspaces.allows_person("browser-1").unwrap());
        assert_eq!(change(&workspaces, Mode::Human).mode, Mode::Human);
    }

    #[test]
    fn a_fence_cannot_confirm_while_the_old_script_is_in_flight() {
        let workspaces = owned();
        observe(&workspaces);
        let operation = workspaces
            .begin("browser-1", Some("term-1"), "click")
            .unwrap()
            .unwrap();
        let mut transition = workspaces.transition("browser-1", Mode::Human, 1).unwrap();
        assert!(transition.confirm_fence().is_err());
        operation.finish(true);
        transition.confirm_fence().unwrap();
        assert_eq!(transition.finish().unwrap().mode, Mode::Human);
    }

    #[tokio::test]
    async fn cancellation_of_the_handoff_wait_is_paused_and_can_be_retried() {
        let workspaces = owned();
        let operation = workspaces
            .begin("browser-1", Some("term-1"), "read")
            .unwrap()
            .unwrap();
        let transition = workspaces.transition("browser-1", Mode::Human, 1).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(1), transition.wait())
                .await
                .is_err()
        );
        drop(transition);
        assert_eq!(
            workspaces.snapshot("browser-1").unwrap().unwrap().mode,
            Mode::Paused
        );
        operation.finish(true);
        assert_eq!(change(&workspaces, Mode::Human).mode, Mode::Human);
    }

    #[test]
    fn pause_blocks_open_and_unmanaged_writes_for_the_same_owner_only() {
        let workspaces = owned();
        change(&workspaces, Mode::Paused);
        for verb in ["open", "goto", "type", "fill", "eval", "viewport", "close"] {
            assert!(
                workspaces.check_actor(Some("term-1"), verb).is_err(),
                "{verb}"
            );
            assert!(
                workspaces.begin_actor(Some("term-1"), verb).is_err(),
                "{verb}"
            );
        }
        assert!(
            workspaces
                .register("browser-2", Some("term-1"), false)
                .is_err()
        );
        assert!(workspaces.check_actor(Some("term-2"), "open").is_ok());
        assert!(
            workspaces
                .begin("browser-person", Some("term-2"), "goto")
                .unwrap()
                .is_none()
        );
        assert_eq!(workspaces.snapshots().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn handoff_drains_a_previously_accepted_unmanaged_write_by_the_owner() {
        let workspaces = owned();
        let activity = workspaces
            .begin_actor(Some("term-1"), "type")
            .unwrap()
            .unwrap();
        let mut transition = workspaces.transition("browser-1", Mode::Human, 1).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(1), transition.wait())
                .await
                .is_err()
        );
        assert!(transition.confirm_fence().is_err());
        activity.settle(Dispatch::Completed);
        tokio::time::timeout(Duration::from_millis(100), transition.wait())
            .await
            .unwrap()
            .unwrap();
        transition.confirm_fence().unwrap();
        assert_eq!(transition.finish().unwrap().mode, Mode::Human);
    }

    #[test]
    fn changing_the_owner_cleans_the_old_owners_pause_only_when_the_transition_finishes() {
        let workspaces = owned();
        let mut transition = workspaces
            .transition_owned("browser-1", Mode::Human, 1, Some("term-2"))
            .unwrap();
        assert!(workspaces.check_actor(Some("term-1"), "open").is_err());
        transition.confirm_fence().unwrap();
        let snapshot = transition.finish().unwrap();
        assert_eq!(snapshot.owner_term.as_deref(), Some("2"));
        assert!(workspaces.check_actor(Some("term-1"), "open").is_ok());
        assert!(workspaces.check_actor(Some("term-2"), "open").is_err());
    }

    #[test]
    fn goto_after_human_control_requires_a_new_agent_observation() {
        let workspaces = owned();
        observe(&workspaces);
        change(&workspaces, Mode::Human);
        change(&workspaces, Mode::Agent);
        assert!(
            workspaces
                .begin("browser-1", Some("term-1"), "goto")
                .is_err()
        );
        assert!(
            workspaces
                .begin("browser-1", Some("term-1"), "close")
                .is_err()
        );
        observe(&workspaces);
        workspaces
            .begin("browser-1", Some("term-1"), "goto")
            .unwrap()
            .unwrap()
            .finish(true);
    }

    #[test]
    fn a_request_queued_before_handoff_cannot_borrow_a_new_observation() {
        let workspaces = owned();
        let old = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        change(&workspaces, Mode::Human);
        change(&workspaces, Mode::Agent);
        observe(&workspaces);
        assert!(
            workspaces
                .begin_at("browser-1", Some("term-1"), "click", Some(old))
                .is_err()
        );
        let current = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        workspaces
            .begin_at("browser-1", Some("term-1"), "click", Some(current))
            .unwrap()
            .unwrap()
            .finish(true);
    }

    #[test]
    fn human_mutations_require_the_current_revision_and_keep_normal_tabs_compatible() {
        let workspaces = owned();
        let human = change(&workspaces, Mode::Human);
        assert!(workspaces.begin_person("browser-1", None).is_err());
        assert!(
            workspaces
                .begin_person("browser-1", Some(human.revision - 1))
                .is_err()
        );
        workspaces
            .begin_person("browser-1", Some(human.revision))
            .unwrap()
            .unwrap()
            .finish(true);
        assert!(
            workspaces
                .begin_person("browser-person", None)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_late_human_popup_does_not_enter_a_later_human_session() {
        let workspaces = owned();
        let old = change(&workspaces, Mode::Human);
        change(&workspaces, Mode::Agent);
        let new = change(&workspaces, Mode::Human);
        assert!(
            workspaces
                .begin_person("browser-1", Some(old.revision))
                .is_err()
        );
        workspaces
            .begin_person("browser-1", Some(new.revision))
            .unwrap()
            .unwrap()
            .finish(true);
    }

    #[test]
    fn placement_rechecks_the_revision_at_the_native_operation() {
        let workspaces = owned();
        let old = change(&workspaces, Mode::Human);
        change(&workspaces, Mode::Agent);
        let current = change(&workspaces, Mode::Human);
        workspaces
            .with_placement("browser-1", Some(old.revision), false, |placement| {
                assert_eq!(placement, Placement::Hidden);
                Ok(())
            })
            .unwrap();
        workspaces
            .with_placement("browser-1", Some(current.revision), false, |placement| {
                assert_eq!(placement, Placement::Control);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn agent_popup_navigation_invalidates_observation_without_an_extra_agent_request() {
        let workspaces = owned();
        observe(&workspaces);
        let navigated = AtomicUsize::new(0);
        assert!(
            workspaces
                .with_agent_popup("browser-1", 1, || {
                    navigated.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                })
                .unwrap()
        );
        assert_eq!(navigated.load(Ordering::Relaxed), 1);
        assert!(
            workspaces
                .snapshot("browser-1")
                .unwrap()
                .unwrap()
                .needs_observation
        );
        assert!(
            workspaces
                .begin("browser-1", Some("term-1"), "click")
                .is_err()
        );
    }

    #[test]
    fn paused_stale_and_closed_popups_never_navigate() {
        let workspaces = owned();
        let navigated = AtomicUsize::new(0);
        let paused = change(&workspaces, Mode::Paused);
        assert!(
            !workspaces
                .with_agent_popup("browser-1", paused.revision, || {
                    navigated.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                })
                .unwrap()
        );
        change(&workspaces, Mode::Agent);
        assert!(
            !workspaces
                .with_agent_popup("browser-1", 1, || {
                    navigated.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                })
                .unwrap()
        );
        workspaces.remove("browser-1").unwrap();
        assert!(
            !workspaces
                .with_agent_popup("browser-1", 1, || {
                    navigated.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                })
                .unwrap()
        );
        assert_eq!(navigated.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_failed_native_popup_is_paused() {
        let workspaces = owned();
        assert!(
            workspaces
                .with_agent_popup("browser-1", 1, || Err("native unavailable".to_string()))
                .is_err()
        );
        assert_eq!(
            workspaces.snapshot("browser-1").unwrap().unwrap().mode,
            Mode::Paused
        );
    }

    #[test]
    fn every_browser_verb_uses_the_core_mutation_classification() {
        for row in zerocode_core::agent_browser::BROWSER_VERBS {
            assert!(!row.acts || row.changes_page, "{}", row.word);
        }
        let find = zerocode_core::agent_browser::browser_verb("find").unwrap();
        assert!(find.check && find.changes_page);
        for verb in ["open", "close", "viewport"] {
            assert!(
                zerocode_core::agent_browser::browser_verb(verb)
                    .unwrap()
                    .changes_page
            );
        }
    }

    #[test]
    fn cancellation_notifies_main_with_paused_state_after_releasing_the_operation() {
        let workspaces = owned();
        observe(&workspaces);
        let notices = Arc::new(Mutex::new(Vec::new()));
        let received = notices.clone();
        let operation = workspaces
            .begin("browser-1", Some("term-1"), "type")
            .unwrap()
            .unwrap()
            .notifying(move |snapshot| received.lock().unwrap().push(snapshot));
        drop(operation);
        let notices = notices.lock().unwrap();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].mode, Mode::Paused);
        assert_eq!(notices[0].in_flight, 0);
    }

    #[test]
    fn observer_snapshots_do_not_satisfy_the_agents_observation_requirement() {
        let workspaces = owned();
        let frame = workspaces.begin_snapshot("browser-1").unwrap();
        assert!(workspaces.begin_snapshot("browser-1").is_err());
        drop(frame);
        assert!(workspaces.begin_snapshot("browser-1").is_err());
        assert!(
            workspaces
                .snapshot("browser-1")
                .unwrap()
                .unwrap()
                .needs_observation
        );
    }

    #[test]
    fn a_capture_lease_stays_exclusive_until_the_native_callback_releases_it() {
        let workspaces = owned();
        let frame = workspaces.begin_snapshot("browser-1").unwrap();
        frame.0.lock().unwrap().started = Some(Instant::now() - FRAME_INTERVAL);
        assert!(workspaces.begin_snapshot("browser-1").is_err());
        drop(frame);
        assert!(workspaces.begin_snapshot("browser-1").is_ok());
    }

    #[test]
    fn registry_and_frame_records_are_bounded_and_removed_on_close() {
        let workspaces = Workspaces::default();
        for index in 0..WORKSPACE_CAP {
            let label = format!("browser-{index}");
            workspaces.register(&label, Some("term-1"), false).unwrap();
            drop(workspaces.begin_snapshot(&label).unwrap());
        }
        assert!(
            workspaces
                .register("browser-overflow", None, false)
                .is_err()
        );
        assert!(workspaces.begin_snapshot("browser-overflow").is_err());
        workspaces.remove("browser-0").unwrap();
        assert!(workspaces.register("browser-overflow", None, false).is_ok());
        assert!(workspaces.begin_snapshot("browser-overflow").is_ok());
    }

    #[test]
    fn snapshots_keep_null_owner_and_report_the_actual_platform_capability() {
        let workspaces = Workspaces::default();
        let snapshot = workspaces.register("browser-1", None, false).unwrap();
        let value = serde_json::to_value(snapshot).unwrap();
        assert_eq!(value["mode"], "paused");
        assert!(value["ownerTerm"].is_null());
        assert_eq!(value["snapshotSupported"], cfg!(target_os = "macos"));
        assert_eq!(value["inFlight"], 0);
        assert_eq!(value["needsObservation"], true);
    }

    #[test]
    fn unavailable_state_never_grants_native_input() {
        let workspaces = Arc::new(owned());
        let broken = workspaces.clone();
        assert!(
            std::thread::spawn(move || {
                let _state = broken.held.lock().unwrap();
                panic!("poison the registry");
            })
            .join()
            .is_err()
        );
        assert!(workspaces.allows_person("browser-1").is_err());
        assert!(
            workspaces
                .begin("browser-1", Some("term-1"), "read")
                .is_err()
        );
        assert!(
            workspaces
                .with_placement("browser-1", Some(1), false, |_| -> Result<(), String> {
                    panic!("unavailable state must not show a pane")
                })
                .is_err()
        );
    }

    #[test]
    fn managed_mutations_require_an_epoch_even_when_the_owner_is_correct() {
        let workspaces = owned();
        observe(&workspaces);
        for entry in zerocode_core::agent_browser::BROWSER_VERBS {
            if !entry.changes_page {
                continue;
            }
            assert!(
                workspaces
                    .begin_at("browser-1", Some("term-1"), entry.word, None)
                    .is_err(),
                "{}",
                entry.word
            );
            assert_eq!(
                workspaces.snapshot("browser-1").unwrap().unwrap().in_flight,
                0,
                "{}",
                entry.word
            );
        }
    }

    #[test]
    fn a_first_observation_acquires_its_revision_only_when_it_completes() {
        let workspaces = owned();
        let initial = workspaces.snapshot("browser-1").unwrap().unwrap();
        let reading = workspaces
            .begin_at("browser-1", Some("term-1"), "read", None)
            .unwrap()
            .unwrap();
        let pending = workspaces.snapshot("browser-1").unwrap().unwrap();
        assert_eq!(pending.in_flight, 1);
        assert!(pending.needs_observation);
        assert_eq!(reading.finish_observation().unwrap(), initial.revision);
        let completed = workspaces.snapshot("browser-1").unwrap().unwrap();
        assert_eq!(completed.in_flight, 0);
        assert!(!completed.needs_observation);
    }

    #[test]
    fn a_stale_read_cannot_borrow_a_resumed_agents_revision() {
        let workspaces = owned();
        let original = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        change(&workspaces, Mode::Human);
        let current = change(&workspaces, Mode::Agent);
        let delayed = workspaces.begin_at("browser-1", Some("term-1"), "read", Some(original));
        assert!(matches!(delayed, Err(error) if error.starts_with("workspace_stale:")));
        let state = workspaces.snapshot("browser-1").unwrap().unwrap();
        assert_eq!(state.revision, current.revision);
        assert_eq!(state.in_flight, 0);
        assert!(state.needs_observation);
    }

    #[test]
    fn a_removed_workspace_cannot_be_reinterpreted_as_a_normal_visible_tab() {
        let workspaces = owned();
        let human = change(&workspaces, Mode::Human);
        workspaces.remove("browser-1").unwrap();
        workspaces
            .with_placement("browser-1", Some(human.revision), false, |placement| {
                assert_eq!(placement, Placement::Hidden);
                Ok(())
            })
            .unwrap();
        assert!(
            workspaces
                .begin_person("browser-1", Some(human.revision))
                .is_err()
        );
        workspaces
            .with_placement("browser-person", None, false, |placement| {
                assert_eq!(placement, Placement::Control);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn an_uncertain_unmanaged_write_is_fenced_without_adopting_the_persons_tab() {
        let workspaces = owned();
        let operation = workspaces
            .begin_actor_target(Some("term-1"), "type", Some("browser-person"))
            .unwrap()
            .unwrap();
        operation.settle(Dispatch::Uncertain);
        assert_eq!(workspaces.snapshots().unwrap().len(), 1);
        assert_eq!(
            workspaces.snapshot("browser-1").unwrap().unwrap().mode,
            Mode::Paused
        );
        let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        let mut transition = workspaces
            .transition("browser-1", Mode::Human, revision)
            .unwrap();
        assert_eq!(
            transition.fence_labels().unwrap(),
            ["browser-1", "browser-person"]
        );
        transition.confirm_fence().unwrap();
        transition.finish().unwrap();
        let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        let transition = workspaces
            .transition("browser-1", Mode::Agent, revision)
            .unwrap();
        assert_eq!(transition.fence_labels().unwrap(), ["browser-1"]);
    }

    #[test]
    fn at_most_one_managed_popup_is_deferred_and_a_failed_initial_open_releases_it() {
        let workspaces = owned();
        assert!(workspaces.reserve_popup("browser-1", 1).unwrap());
        assert!(!workspaces.reserve_popup("browser-1", 1).unwrap());
        workspaces.release_popup("browser-1").unwrap();
        assert!(workspaces.reserve_popup("browser-1", 1).unwrap());
        change(&workspaces, Mode::Paused);
        assert!(
            !workspaces
                .with_agent_popup("browser-1", 1, || panic!("stale popup must not navigate"))
                .unwrap()
        );
        assert!(!workspaces.reserve_popup("browser-1", 1).unwrap());
    }

    #[test]
    fn a_canceled_human_mutation_also_requires_recovery_before_control_is_granted() {
        let workspaces = owned();
        let human = change(&workspaces, Mode::Human);
        drop(
            workspaces
                .begin_person("browser-1", Some(human.revision))
                .unwrap()
                .unwrap(),
        );
        assert_eq!(
            workspaces.snapshot("browser-1").unwrap().unwrap().mode,
            Mode::Paused
        );
        assert!(
            workspaces
                .begin_person("browser-1", Some(human.revision))
                .is_err()
        );
    }

    #[test]
    fn the_watch_rate_limit_does_not_starve_the_agents_fresh_screenshot() {
        let workspaces = owned();
        let watch = workspaces.begin_snapshot_for("browser-1", true).unwrap();
        assert!(workspaces.begin_snapshot_for("browser-1", false).is_err());
        drop(watch);
        drop(workspaces.begin_snapshot_for("browser-1", false).unwrap());
        assert!(workspaces.begin_snapshot_for("browser-1", true).is_err());
    }

    #[test]
    fn stale_agent_close_preserves_human_control_but_user_close_is_still_allowed() {
        let workspaces = owned();
        let human = change(&workspaces, Mode::Human);
        assert!(workspaces.begin_close("browser-1", Some(1)).is_err());
        assert_eq!(
            workspaces.snapshot("browser-1").unwrap().unwrap().revision,
            human.revision
        );
        assert!(workspaces.allows_person("browser-1").unwrap());
        drop(workspaces.begin_close("browser-1", None).unwrap().unwrap());
        assert_eq!(
            workspaces.snapshot("browser-1").unwrap().unwrap().mode,
            Mode::Paused
        );
    }

    #[test]
    fn native_close_failure_rolls_back_to_paused_without_a_closed_receipt() {
        let workspaces = owned();
        let closing = workspaces
            .begin_close("browser-1", Some(1))
            .unwrap()
            .unwrap();
        assert!(workspaces.check_actor(Some("term-1"), "open").is_err());
        assert!(workspaces.transition("browser-1", Mode::Human, 1).is_err());
        assert!(!workspaces.closed_at("browser-1", 1).unwrap());
        drop(closing);
        assert_eq!(
            workspaces.snapshot("browser-1").unwrap().unwrap().mode,
            Mode::Paused
        );
        assert!(!workspaces.closed_at("browser-1", 1).unwrap());
        assert_eq!(change(&workspaces, Mode::Human).mode, Mode::Human);
    }

    #[test]
    fn an_unpublished_cancelled_open_cannot_publish_a_late_native_view() {
        let workspaces = owned();
        assert!(
            workspaces
                .cancel_unpublished("browser-1", 1, || false)
                .unwrap()
        );
        assert!(
            workspaces
                .publish("browser-1", 1, || panic!(
                    "canceled birth must not become public"
                ))
                .is_err()
        );
        assert!(workspaces.snapshots().unwrap().is_empty());
        assert!(!workspaces.closed_at("browser-1", 1).unwrap());
    }

    #[test]
    fn cancellation_cannot_claim_an_already_published_tab_with_a_new_human_revision() {
        let workspaces = owned();
        workspaces.publish("browser-1", 1, || Ok(())).unwrap();
        change(&workspaces, Mode::Human);
        assert!(
            !workspaces
                .cancel_unpublished("browser-1", 1, || true)
                .unwrap()
        );
        assert!(workspaces.begin_close("browser-1", Some(1)).is_err());
        assert!(workspaces.allows_person("browser-1").unwrap());
    }

    #[test]
    fn closed_acknowledgements_are_exact_bounded_and_cleared_on_a_new_registration() {
        let workspaces = Workspaces::default();
        assert!(!workspaces.closed_at("browser-absent", 1).unwrap());
        for index in 0..=WORKSPACE_CAP {
            workspaces
                .confirm_closed(&format!("browser-{index}"), 1)
                .unwrap();
        }
        assert_eq!(workspaces.closed.lock().unwrap().len(), WORKSPACE_CAP);
        assert!(
            workspaces
                .closed_at(&format!("browser-{WORKSPACE_CAP}"), 1)
                .unwrap()
        );
        assert!(
            !workspaces
                .closed_at(&format!("browser-{WORKSPACE_CAP}"), 2)
                .unwrap()
        );
        let label = format!("browser-{WORKSPACE_CAP}");
        workspaces.register(&label, None, false).unwrap();
        assert!(!workspaces.closed_at(&label, 1).unwrap());
    }

    #[test]
    fn held_actor_identity_does_not_depend_on_provider_or_model_names() {
        for term in [7, 44, 2048] {
            let actor = crate::hooks::pane_key_of(term);
            let workspaces = Workspaces::default();
            let snapshot = workspaces
                .register("browser-1", Some(&actor), false)
                .unwrap();
            assert_eq!(snapshot.owner_term, Some(term.to_string()));
            workspaces
                .begin("browser-1", Some(&actor), "read")
                .unwrap()
                .unwrap()
                .finish(true);
            workspaces
                .begin("browser-1", Some(&actor), "click")
                .unwrap()
                .unwrap()
                .finish(true);
            let mut transition = workspaces.transition("browser-1", Mode::Human, 1).unwrap();
            transition.confirm_fence().unwrap();
            transition.finish().unwrap();
            for row in zerocode_core::agent_browser::BROWSER_VERBS {
                assert!(
                    workspaces
                        .begin("browser-1", Some(&actor), row.word)
                        .is_err(),
                    "{term} {}",
                    row.word
                );
            }
            assert!(workspaces.allows_person("browser-1").unwrap());
        }
    }
}

#[cfg(test)]
mod handoff_followup_regressions {
    use super::*;

    const HISTORY: [&str; 3] = ["console", "network", "diagnose"];

    fn owned() -> Workspaces {
        let workspaces = Workspaces::default();
        workspaces
            .register("browser-1", Some("term-1"), false)
            .unwrap();
        workspaces
    }

    fn observe(workspaces: &Workspaces) {
        workspaces
            .begin("browser-1", Some("term-1"), "read")
            .unwrap()
            .unwrap()
            .finish(true);
    }

    fn change(workspaces: &Workspaces, mode: Mode) -> Snapshot {
        let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        let mut transition = workspaces.transition("browser-1", mode, revision).unwrap();
        if mode != Mode::Paused {
            transition.confirm_fence().unwrap();
        }
        transition.finish().unwrap()
    }

    fn stepped(workspaces: &Workspaces, verb: &str) -> StepControl {
        let operation = workspaces
            .begin("browser-1", Some("term-1"), verb)
            .unwrap()
            .unwrap();
        let control = operation.step_control().unwrap();
        operation.finish(true);
        control
    }

    #[test]
    fn a_step_frame_queued_before_a_person_takes_the_tab_is_skipped_without_a_capture() {
        let workspaces = owned();
        observe(&workspaces);
        let control = stepped(&workspaces, "click");
        change(&workspaces, Mode::Human);
        assert!(
            workspaces
                .admit_evidence("browser-1", Some(&control))
                .unwrap()
                .is_none()
        );
        drop(workspaces.begin_snapshot_for("browser-1", false).unwrap());
        change(&workspaces, Mode::Agent);
        assert!(
            workspaces
                .admit_evidence("browser-1", Some(&control))
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_step_frame_admitted_just_before_a_transfer_is_never_kept_and_never_delays_it() {
        let workspaces = owned();
        observe(&workspaces);
        let control = stepped(&workspaces, "click");
        let (frame, check) = workspaces
            .admit_evidence("browser-1", Some(&control))
            .unwrap()
            .unwrap();
        assert!(check.holds());
        let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        let mut transition = workspaces
            .transition("browser-1", Mode::Human, revision)
            .unwrap();
        assert!(!check.holds());
        tokio::time::timeout(Duration::from_millis(100), transition.wait())
            .await
            .unwrap()
            .unwrap();
        transition.confirm_fence().unwrap();
        assert_eq!(transition.finish().unwrap().mode, Mode::Human);
        assert!(!check.holds());
        drop(frame);
        assert!(
            workspaces
                .admit_evidence("browser-1", Some(&control))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn admitting_and_keeping_a_step_frame_issues_no_observation() {
        let workspaces = owned();
        let control = stepped(&workspaces, "wait");
        let before = workspaces.snapshot("browser-1").unwrap().unwrap();
        let (frame, check) = workspaces
            .admit_evidence("browser-1", Some(&control))
            .unwrap()
            .unwrap();
        assert!(workspaces.begin_snapshot_for("browser-1", true).is_err());
        assert!(check.holds());
        drop(frame);
        let after = workspaces.snapshot("browser-1").unwrap().unwrap();
        assert!(before.needs_observation && after.needs_observation);
        assert_eq!(before.revision, after.revision);
        assert!(matches!(
            workspaces.begin_at("browser-1", Some("term-1"), "click", Some(after.revision)),
            Err(error) if error.starts_with("workspace_observation_required:")
        ));
        let watch = workspaces.begin_snapshot_for("browser-1", true).unwrap();
        assert!(
            workspaces
                .admit_evidence("browser-1", Some(&control))
                .is_err()
        );
        drop(watch);
    }

    #[test]
    fn a_step_frame_needs_its_own_tab_owner_and_managed_state() {
        let workspaces = owned();
        workspaces
            .register("browser-2", Some("term-1"), false)
            .unwrap();
        observe(&workspaces);
        let control = stepped(&workspaces, "click");
        assert!(
            workspaces
                .admit_evidence("browser-2", Some(&control))
                .unwrap()
                .is_none()
        );
        assert!(
            workspaces
                .admit_evidence("browser-1", None)
                .unwrap()
                .is_none()
        );
        assert!(
            workspaces
                .admit_evidence("browser-person", Some(&control))
                .unwrap()
                .is_none()
        );
        drop(
            workspaces
                .admit_evidence("browser-person", None)
                .unwrap()
                .unwrap(),
        );
        workspaces
            .get("browser-1")
            .unwrap()
            .unwrap()
            .lock()
            .unwrap()
            .owner = Some("term-2".into());
        assert!(
            workspaces
                .admit_evidence("browser-1", Some(&control))
                .unwrap()
                .is_none()
        );
        workspaces.remove("browser-1").unwrap();
        assert!(
            workspaces
                .admit_evidence("browser-1", Some(&control))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_refusal_before_dispatch_keeps_control_of_the_tab_and_the_owners_other_tabs() {
        let workspaces = owned();
        workspaces
            .register("browser-2", Some("term-1"), false)
            .unwrap();
        observe(&workspaces);
        let before = workspaces.snapshot("browser-1").unwrap().unwrap();
        workspaces
            .begin("browser-1", Some("term-1"), "click")
            .unwrap()
            .unwrap()
            .settle(Dispatch::NotSent);
        for verb in ["goto", "type", "fill"] {
            workspaces
                .begin_actor_target(Some("term-1"), verb, Some("browser-person"))
                .unwrap()
                .unwrap()
                .settle(Dispatch::NotSent);
        }
        workspaces
            .begin_actor_target(Some("term-1"), "open", None)
            .unwrap()
            .unwrap()
            .settle(Dispatch::NotSent);
        let after = workspaces.snapshot("browser-1").unwrap().unwrap();
        assert_eq!(
            (after.mode, after.revision, after.needs_observation),
            (Mode::Agent, before.revision, false)
        );
        assert_eq!(
            workspaces.snapshot("browser-2").unwrap().unwrap().mode,
            Mode::Agent
        );
        workspaces
            .begin("browser-1", Some("term-1"), "click")
            .unwrap()
            .unwrap()
            .settle(Dispatch::Completed);
        let transition = workspaces
            .transition("browser-1", Mode::Human, before.revision)
            .unwrap();
        assert_eq!(transition.fence_labels().unwrap(), ["browser-1"]);
    }

    #[test]
    fn an_uncertain_dispatch_pauses_the_tab_and_every_target_of_its_owner() {
        let workspaces = owned();
        observe(&workspaces);
        workspaces
            .begin("browser-1", Some("term-1"), "fill")
            .unwrap()
            .unwrap()
            .settle(Dispatch::Uncertain);
        let paused = workspaces.snapshot("browser-1").unwrap().unwrap();
        assert!(paused.mode == Mode::Paused && paused.needs_observation);
        let workspaces = owned();
        workspaces
            .register("browser-2", Some("term-1"), false)
            .unwrap();
        workspaces
            .begin_actor_target(Some("term-1"), "eval", Some("browser-person"))
            .unwrap()
            .unwrap()
            .settle(Dispatch::Uncertain);
        for label in ["browser-1", "browser-2"] {
            assert_eq!(
                workspaces.snapshot(label).unwrap().unwrap().mode,
                Mode::Paused,
                "{label}"
            );
        }
        let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        let transition = workspaces
            .transition("browser-1", Mode::Human, revision)
            .unwrap();
        assert_eq!(
            transition.fence_labels().unwrap(),
            ["browser-1", "browser-person"]
        );
    }

    #[test]
    fn a_persons_own_request_keeps_the_human_view_placed_and_a_second_one_is_busy() {
        let workspaces = owned();
        let human = change(&workspaces, Mode::Human);
        let operation = workspaces
            .begin_person("browser-1", Some(human.revision))
            .unwrap()
            .unwrap();
        workspaces
            .with_placement("browser-1", Some(human.revision), false, |placement| {
                assert_eq!(placement, Placement::Control);
                Ok(())
            })
            .unwrap();
        assert!(matches!(
            workspaces.begin_person("browser-1", Some(human.revision)),
            Err(error) if error.starts_with("workspace_busy:")
        ));
        assert!(
            workspaces
                .begin("browser-1", Some("term-1"), "read")
                .is_err()
        );
        operation.finish(true);
        workspaces
            .with_placement("browser-1", Some(human.revision), false, |placement| {
                assert_eq!(placement, Placement::Control);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn page_history_is_refused_to_agents_once_a_person_has_controlled_the_tab() {
        let workspaces = owned();
        for verb in HISTORY {
            workspaces
                .begin("browser-1", Some("term-1"), verb)
                .unwrap()
                .unwrap()
                .finish(true);
        }
        change(&workspaces, Mode::Human);
        change(&workspaces, Mode::Agent);
        observe(&workspaces);
        for verb in HISTORY {
            assert!(
                matches!(
                    workspaces.begin("browser-1", Some("term-1"), verb),
                    Err(error) if error.starts_with("workspace_history_unsupported:")
                ),
                "{verb}"
            );
        }
        change(&workspaces, Mode::Paused);
        change(&workspaces, Mode::Agent);
        observe(&workspaces);
        for verb in HISTORY {
            assert!(
                workspaces.begin("browser-1", Some("term-1"), verb).is_err(),
                "{verb}"
            );
        }
        workspaces
            .register("browser-2", Some("term-1"), false)
            .unwrap();
        for verb in HISTORY {
            workspaces
                .begin("browser-2", Some("term-1"), verb)
                .unwrap()
                .unwrap()
                .finish(true);
        }
        for row in zerocode_core::agent_browser::BROWSER_VERBS {
            assert_eq!(
                zerocode_core::agent_browser::reads_history(row.word),
                HISTORY.contains(&row.word),
                "{}",
                row.word
            );
        }
    }
}

#[cfg(test)]
mod capture_start_regressions {
    use super::*;
    use std::cell::Cell;

    fn stepped(observed: bool, verb: &str) -> (Workspaces, StepControl) {
        let workspaces = Workspaces::default();
        workspaces
            .register("browser-1", Some("term-1"), false)
            .unwrap();
        if observed {
            workspaces
                .begin("browser-1", Some("term-1"), "read")
                .unwrap()
                .unwrap()
                .finish(true);
        }
        let operation = workspaces
            .begin("browser-1", Some("term-1"), verb)
            .unwrap()
            .unwrap();
        let control = operation.step_control().unwrap();
        operation.finish(true);
        (workspaces, control)
    }

    #[test]
    fn a_capture_queued_before_a_transfer_starts_nothing_once_the_person_holds_the_tab() {
        let (workspaces, control) = stepped(true, "click");
        let (frame, check) = workspaces
            .admit_evidence("browser-1", Some(&control))
            .unwrap()
            .unwrap();
        let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        let mut transition = workspaces
            .transition("browser-1", Mode::Human, revision)
            .unwrap();
        transition.confirm_fence().unwrap();
        assert_eq!(transition.finish().unwrap().mode, Mode::Human);
        let sends = Cell::new(0);
        let started = check.start(|| {
            sends.set(sends.get() + 1);
            Ok(())
        });
        assert!(started.is_err());
        assert_eq!(
            sends.get(),
            0,
            "a native capture started while the person held the tab"
        );
        assert!(!check.holds());
        drop(frame);
    }

    #[tokio::test]
    async fn a_transfer_waits_for_a_capture_send_in_progress_but_not_for_its_picture() {
        let (workspaces, control) = stepped(true, "click");
        let (frame, check) = workspaces
            .admit_evidence("browser-1", Some(&control))
            .unwrap()
            .unwrap();
        let (entered, entering) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let sender = check.clone();
        let native = std::thread::spawn(move || {
            sender.start(|| {
                entered.send(()).unwrap();
                released.recv().unwrap();
                Ok(())
            })
        });
        entering.recv().unwrap();
        let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        let mut transition = workspaces
            .transition("browser-1", Mode::Human, revision)
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), transition.wait())
                .await
                .is_err(),
            "the person was handed the tab while a capture was still being sent"
        );
        release.send(()).unwrap();
        native.join().unwrap().unwrap();
        tokio::time::timeout(Duration::from_millis(100), transition.wait())
            .await
            .unwrap()
            .unwrap();
        transition.confirm_fence().unwrap();
        assert_eq!(transition.finish().unwrap().mode, Mode::Human);
        assert!(
            !check.holds(),
            "a picture that arrives after the transfer is kept"
        );
        drop(frame);
        assert!(
            workspaces
                .admit_evidence("browser-1", Some(&control))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_capture_sent_under_agent_control_keeps_its_lease_and_issues_no_observation() {
        let (workspaces, control) = stepped(false, "wait");
        let before = workspaces.snapshot("browser-1").unwrap().unwrap();
        let (frame, check) = workspaces
            .admit_evidence("browser-1", Some(&control))
            .unwrap()
            .unwrap();
        let sends = Cell::new(0);
        check
            .start(|| {
                sends.set(sends.get() + 1);
                Ok(())
            })
            .unwrap();
        assert_eq!(sends.get(), 1);
        assert!(workspaces.begin_snapshot_for("browser-1", false).is_err());
        assert!(check.holds());
        drop(frame);
        let after = workspaces.snapshot("browser-1").unwrap().unwrap();
        assert_eq!(
            (after.revision, after.needs_observation, after.mode),
            (before.revision, true, Mode::Agent)
        );
        assert!(
            EvidenceCheck::default()
                .start(|| Ok::<_, String>(sends.get()))
                .is_ok_and(|seen| seen == 1)
        );
        drop(workspaces.begin_snapshot_for("browser-1", false).unwrap());
    }
}

/// The session's regrant (t-66200): the person's one 「다시 맡기기」 on a
/// work browser's bar lets its agent's own fresh look lift the pause the
/// agent's change of unknown outcome left; every pause the person made — or
/// that a navigation, a popup or the person's hand stands behind — stays
/// theirs; and every refusal says, in its token, who lifts it.
#[cfg(test)]
mod session_regrant_tests {
    use super::*;
    use crate::computer_use::guard;

    const TAB: &str = "browser-1";
    const OTHER: &str = "browser-2";
    const AGENT: &str = "term-1";

    fn state(workspaces: &Workspaces, label: &str) -> Snapshot {
        workspaces.snapshot(label).unwrap().unwrap()
    }

    /// The agent's look: a read begun at the tab's revision and completed.
    fn look(workspaces: &Workspaces, label: &str) -> Result<u64, String> {
        let at = state(workspaces, label).revision;
        workspaces
            .begin_at(label, Some(AGENT), "read", Some(at))?
            .expect("a work browser")
            .finish_observation()
    }

    /// The agent's change at the revision its look issued, settled as its
    /// dispatch says — the two operations a door request holds.
    fn input(
        workspaces: &Workspaces,
        label: &str,
        verb: &str,
        at: u64,
        dispatch: Dispatch,
    ) -> Result<(), String> {
        let operation = workspaces
            .begin_at(label, Some(AGENT), verb, Some(at))?
            .expect("a work browser");
        let actor = workspaces
            .begin_actor_target(Some(AGENT), verb, Some(label))?
            .expect("a page change");
        operation.settle(dispatch);
        actor.settle(dispatch);
        Ok(())
    }

    /// The person hands the tab back: the bar's 「다시 맡기기」 (`regrant`),
    /// or another road back — the band's resume — without it.
    fn hand_back(workspaces: &Workspaces, label: &str, regrant: bool) -> Snapshot {
        let mut transition = workspaces
            .transition(label, Mode::Agent, state(workspaces, label).revision)
            .unwrap()
            .regranting(regrant);
        transition.confirm_fence().unwrap();
        transition.finish().unwrap()
    }

    /// The person's own pause or hand, by the bar's road.
    fn person(workspaces: &Workspaces, label: &str, mode: Mode) -> Snapshot {
        let mut transition = workspaces
            .transition(label, mode, state(workspaces, label).revision)
            .unwrap();
        if mode != Mode::Paused {
            transition.confirm_fence().unwrap();
        }
        transition.finish().unwrap()
    }

    /// Work browsers whose session regrant the person turned on: the agent's
    /// first change of unknown outcome paused every one, and the person
    /// pressed 「다시 맡기기」 on each.
    fn regranted_on(labels: &[&str]) -> Workspaces {
        let workspaces = Workspaces::default();
        for label in labels {
            workspaces.register(label, Some(AGENT), false).unwrap();
        }
        let seen = look(&workspaces, labels[0]).unwrap();
        input(&workspaces, labels[0], "click", seen, Dispatch::Uncertain).unwrap();
        for label in labels {
            hand_back(&workspaces, label, true);
        }
        workspaces
    }

    fn regranted() -> Workspaces {
        regranted_on(&[TAB])
    }

    fn says<T>(result: &Result<T, String>, token: &str) -> bool {
        result.as_ref().is_err_and(|error| error.starts_with(token))
    }

    const OBSERVE: &str = "workspace_paused[resume=observe]: ";
    const PERSON: &str = "workspace_paused[resume=person]: ";

    /// The login scene's second half: after the person's one 「다시 맡기기」,
    /// a click of unknown outcome still pauses the tab, and the agent's next
    /// input is refused saying a look lifts it; the agent's own read lifts it
    /// at a revision after that change, and the next click quoting it is
    /// taken — no second press of the person's.
    #[test]
    fn under_the_sessions_regrant_the_agents_own_look_lifts_its_uncertain_changes_pause() {
        let _hand = crate::tests::computer_desktop_wait::ONE_HAND
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard::lift();
        let workspaces = regranted();
        let granted = state(&workspaces, TAB);
        assert!(
            granted.regrant,
            "the person's 「다시 맡기기」 turned the session's regrant on: {granted:?}"
        );
        let seen = look(&workspaces, TAB).expect("the handed-back tab is looked at");
        input(&workspaces, TAB, "click", seen, Dispatch::Uncertain).unwrap();
        let paused = state(&workspaces, TAB);
        assert_eq!(
            (paused.mode, paused.resume),
            (Mode::Paused, Some(Resume::Observe)),
            "a change of unknown outcome still pauses the tab, and a look lifts it"
        );
        let early = input(
            &workspaces,
            TAB,
            "click",
            paused.revision,
            Dispatch::Completed,
        );
        assert!(
            says(&early, OBSERVE),
            "an input before the look is refused, saying a look lifts the pause: {early:?}"
        );
        let lifted = look(&workspaces, TAB);
        assert!(
            lifted.as_ref().is_ok_and(|at| *at > paused.revision),
            "the agent's own look lifts the pause at a revision after the change {}: {lifted:?}",
            paused.revision
        );
        let resumed = state(&workspaces, TAB);
        assert_eq!(
            (resumed.mode, resumed.needs_observation, resumed.resume),
            (Mode::Agent, false, None)
        );
        let next = input(
            &workspaces,
            TAB,
            "click",
            lifted.unwrap(),
            Dispatch::Completed,
        );
        assert!(
            next.is_ok(),
            "the next click at the look's revision is taken: {next:?}"
        );
    }

    /// Off by default: with no 「다시 맡기기」 on its bar, the pause of the
    /// agent's change of unknown outcome stays the person's — the agent's look
    /// is refused, saying so — and a hand-back by another road leaves the
    /// regrant off.
    #[test]
    fn without_the_sessions_regrant_an_uncertain_changes_pause_stays_the_persons() {
        let workspaces = Workspaces::default();
        workspaces.register(TAB, Some(AGENT), false).unwrap();
        let seen = look(&workspaces, TAB).unwrap();
        input(&workspaces, TAB, "click", seen, Dispatch::Uncertain).unwrap();
        let paused = state(&workspaces, TAB);
        let looked = look(&workspaces, TAB);
        let band = hand_back(&workspaces, TAB, false);
        assert_eq!(
            (paused.mode, paused.regrant, paused.resume),
            (Mode::Paused, false, Some(Resume::Person)),
            "off by default, the pause is the person's"
        );
        assert!(
            says(&looked, PERSON),
            "the agent's look is refused, saying the person lifts the pause: {looked:?}"
        );
        assert_eq!(
            (band.mode, band.regrant),
            (Mode::Agent, false),
            "a hand-back other than the bar's 「다시 맡기기」 leaves the regrant off"
        );
    }

    /// The person's stop stands over the session's regrant (t-62872): while
    /// it stands the door refuses the agent's look in the person's words; its
    /// walk pauses the tab by the bar's road; and once the stop is lifted,
    /// the agent's look does not lift that pause — the person's press does.
    #[test]
    fn the_persons_stop_is_lifted_by_the_person_alone_under_the_sessions_regrant() {
        use zerocode_core::computer_use::STOP_REASON_HOTKEY;
        let _hand = crate::tests::computer_desktop_wait::ONE_HAND
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard::lift();
        let workspaces = regranted();
        look(&workspaces, TAB).unwrap();
        guard::adopt(STOP_REASON_HOTKEY);
        let at = state(&workspaces, TAB).revision;
        let door = workspaces
            .begin_agents(TAB, Some(AGENT), "read", Some(at))
            .map(|operation| operation.is_some());
        person(&workspaces, TAB, Mode::Paused);
        guard::lift();
        let stopped = state(&workspaces, TAB);
        let looked = look(&workspaces, TAB);
        assert!(
            says(&door, "workspace_person_stopped:"),
            "while the person's stop stands the door refuses the look: {door:?}"
        );
        assert_eq!(
            (stopped.mode, stopped.regrant, stopped.resume),
            (Mode::Paused, true, Some(Resume::Person)),
            "the stop's pause is the person's, the regrant on or not"
        );
        assert!(
            says(&looked, PERSON),
            "the agent's look does not lift the person's stop: {looked:?}"
        );
    }

    /// The person's hand ends the session's regrant: 「내가 조작」 turns it
    /// off, and after the band hands the tab back the next change of unknown
    /// outcome is the person's to lift again.
    #[test]
    fn taking_the_tab_by_hand_ends_the_sessions_regrant() {
        let workspaces = regranted();
        let human = person(&workspaces, TAB, Mode::Human);
        let back = hand_back(&workspaces, TAB, false);
        let seen = look(&workspaces, TAB).unwrap();
        input(&workspaces, TAB, "click", seen, Dispatch::Uncertain).unwrap();
        let paused = state(&workspaces, TAB);
        assert!(
            !human.regrant && !back.regrant,
            "the person's hand turned the regrant off: {human:?} {back:?}"
        );
        assert_eq!(
            paused.resume,
            Some(Resume::Person),
            "the next uncertain change waits for the person again"
        );
    }

    /// A popup waiting on the tab keeps its pause the person's.
    #[test]
    fn a_waiting_popup_keeps_the_pause_the_persons_under_the_sessions_regrant() {
        let workspaces = regranted();
        let seen = look(&workspaces, TAB).unwrap();
        assert!(
            workspaces.reserve_popup(TAB, seen).unwrap(),
            "a popup waits"
        );
        input(&workspaces, TAB, "click", seen, Dispatch::Uncertain).unwrap();
        let paused = state(&workspaces, TAB);
        let looked = look(&workspaces, TAB);
        assert_eq!(
            paused.resume,
            Some(Resume::Person),
            "a waiting popup is the person's to settle"
        );
        assert!(says(&looked, PERSON), "{looked:?}");
    }

    /// A navigation of unknown outcome is the person's to lift: their handoff
    /// waits for it to settle, and a look does not.
    #[test]
    fn a_navigation_of_unknown_outcome_is_the_persons_to_lift_under_the_sessions_regrant() {
        let workspaces = regranted();
        let seen = look(&workspaces, TAB).unwrap();
        input(&workspaces, TAB, "goto", seen, Dispatch::Uncertain).unwrap();
        let paused = state(&workspaces, TAB);
        let looked = look(&workspaces, TAB);
        assert_eq!(
            (paused.mode, paused.resume),
            (Mode::Paused, Some(Resume::Person)),
            "an unfinished navigation is the person's to lift"
        );
        assert!(says(&looked, PERSON), "{looked:?}");
    }

    /// The person's own pause is theirs to lift, the regrant on or not.
    #[test]
    fn the_persons_pause_is_theirs_to_lift_under_the_sessions_regrant() {
        let workspaces = regranted();
        let paused = person(&workspaces, TAB, Mode::Paused);
        let looked = look(&workspaces, TAB);
        assert_eq!(
            (paused.regrant, paused.resume),
            (true, Some(Resume::Person))
        );
        assert!(says(&looked, PERSON), "{looked:?}");
    }

    /// The look must come after the change it settles: a read in flight on
    /// the agent's other tab when the change paused it, or one quoting a
    /// revision from before the change, does not lift the pause; the next
    /// look does.
    #[test]
    fn a_look_from_before_the_change_does_not_lift_its_pause() {
        let _hand = crate::tests::computer_desktop_wait::ONE_HAND
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard::lift();
        let workspaces = regranted_on(&[TAB, OTHER]);
        let seen = look(&workspaces, TAB).unwrap();
        let before = state(&workspaces, OTHER).revision;
        let early = workspaces
            .begin_at(OTHER, Some(AGENT), "read", Some(before))
            .unwrap()
            .unwrap();
        input(&workspaces, TAB, "click", seen, Dispatch::Uncertain).unwrap();
        let paused = state(&workspaces, OTHER);
        let early = early.finish_observation();
        let quoted = workspaces
            .begin_at(OTHER, Some(AGENT), "read", Some(before))
            .map(|operation| operation.is_some());
        let lifted = look(&workspaces, OTHER);
        assert_eq!(
            (paused.mode, paused.resume),
            (Mode::Paused, Some(Resume::Observe)),
            "the change on the agent's one tab paused its other tab too, a look to lift it"
        );
        assert!(
            says(&early, "workspace_stale:"),
            "a look begun before the change does not lift its pause: {early:?}"
        );
        assert!(
            says(&quoted, "workspace_stale:"),
            "nor does one quoting a revision from before it: {quoted:?}"
        );
        assert!(
            lifted.as_ref().is_ok_and(|at| *at > paused.revision),
            "a look after the change lifts it: {lifted:?}"
        );
    }

    /// The bar's off switch ends the session's regrant: the pause standing
    /// now, at the same revision, and every later one wait for the person.
    #[test]
    fn turning_the_regrant_off_returns_its_pauses_to_the_person() {
        let workspaces = regranted();
        let seen = look(&workspaces, TAB).unwrap();
        input(&workspaces, TAB, "click", seen, Dispatch::Uncertain).unwrap();
        let before = state(&workspaces, TAB);
        let ended = workspaces.end_regrant(TAB).unwrap();
        let looked = look(&workspaces, TAB);
        assert_eq!(
            before.resume,
            Some(Resume::Observe),
            "a look lifted it before the switch"
        );
        assert_eq!(
            (ended.regrant, ended.resume, ended.revision),
            (false, Some(Resume::Person), before.revision),
            "the switch turns the regrant off and leaves the standing pause the person's"
        );
        assert!(says(&looked, PERSON), "{looked:?}");
    }

    /// An input waits for every tab its agent owns, and its refusal names the
    /// tab still paused and who lifts it: the agent's look at that tab, or —
    /// once the person's pause stands there — the person.
    #[test]
    fn an_input_waits_for_the_agents_every_tab_and_names_the_one_to_look_at() {
        let _hand = crate::tests::computer_desktop_wait::ONE_HAND
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard::lift();
        let workspaces = regranted_on(&[TAB, OTHER]);
        let seen = look(&workspaces, TAB).unwrap();
        input(&workspaces, TAB, "click", seen, Dispatch::Uncertain).unwrap();
        let lifted = look(&workspaces, TAB);
        assert!(
            lifted.is_ok(),
            "the agent's own look lifts the pause its change left on that tab: {lifted:?}"
        );
        let lifted = lifted.unwrap();
        let waiting = input(&workspaces, TAB, "click", lifted, Dispatch::Completed);
        person(&workspaces, OTHER, Mode::Paused);
        let persons = input(&workspaces, TAB, "click", lifted, Dispatch::Completed);
        hand_back(&workspaces, OTHER, false);
        let freed = input(&workspaces, TAB, "click", lifted, Dispatch::Completed);
        assert!(
            says(&waiting, &format!("{OBSERVE}작업 브라우저 {OTHER}: ")),
            "the input waits for the other tab a look lifts, and names it: {waiting:?}"
        );
        assert!(
            says(&persons, &format!("{PERSON}작업 브라우저 {OTHER}: ")),
            "and for the person once their pause stands there: {persons:?}"
        );
        assert!(freed.is_ok(), "{freed:?}");
    }

    /// The refusal's words: the token, the tab, 「자동으로 재시도하지
    /// 마세요」 kept, and what to do — look again, or stop for the person —
    /// read back by the one parser the door's JSON answer uses.
    #[test]
    fn a_paused_refusal_says_who_lifts_it_and_what_to_do() {
        for (resume, next) in [
            (Resume::Observe, "새로 본 뒤 이어 가세요"),
            (Resume::Person, "사람이 풀 때까지 멈추세요"),
        ] {
            let said = paused(TAB, resume);
            assert!(
                said.starts_with(&format!(
                    "workspace_paused[resume={}]: 작업 브라우저 {TAB}: ",
                    resume.word()
                )) && said.contains("자동으로 재시도하지 마세요")
                    && said.ends_with(next),
                "{said}"
            );
            assert_eq!(
                paused_resume(&said).map(|(read, _)| read),
                Some(resume),
                "{said}"
            );
            assert_eq!(serde_json::to_value(resume).unwrap(), resume.word());
        }
        assert_eq!(paused_resume(&changed()), None);
    }
}

/// Watching a work browser live (t-66513): the placement that shows an
/// agent's tab under the shield, and the capture leases that wait their turn
/// instead of turning the watch dark.
#[cfg(test)]
mod live_watch_tests {
    use super::*;

    const TAB: &str = "browser-1";
    const BOUNDS: [f64; 4] = [10.0, 20.0, 800.0, 600.0];

    fn owned() -> Arc<Workspaces> {
        let workspaces = Workspaces::default();
        workspaces.register(TAB, Some("term-1"), false).unwrap();
        Arc::new(workspaces)
    }

    fn observe(workspaces: &Workspaces) {
        workspaces
            .begin(TAB, Some("term-1"), "read")
            .unwrap()
            .unwrap()
            .finish(true);
    }

    fn revision(workspaces: &Workspaces) -> u64 {
        workspaces.snapshot(TAB).unwrap().unwrap().revision
    }

    fn hand_over(workspaces: &Workspaces, mode: Mode) {
        let mut transition = workspaces
            .transition(TAB, mode, revision(workspaces))
            .unwrap();
        if mode != Mode::Paused {
            transition.confirm_fence().unwrap();
        }
        transition.finish().unwrap();
    }

    /// A fake engine that writes down each native move a placement makes.
    #[derive(Default)]
    struct Stage(Vec<String>);

    impl PaneStage for Stage {
        fn bounds(&mut self, bounds: [f64; 4]) -> Result<(), String> {
            self.0.push(format!("bounds {bounds:?}"));
            Ok(())
        }

        fn shield(&mut self, raised: bool) -> Result<(), String> {
            self.0
                .push(if raised { "shield up" } else { "shield down" }.to_string());
            Ok(())
        }

        fn show(&mut self) -> Result<(), String> {
            self.0.push("show".to_string());
            Ok(())
        }
    }

    /// One placement the way the engines make it: decided at the tab's
    /// revision, staged inside that decision.
    fn place(
        workspaces: &Workspaces,
        revision: u64,
        watching: bool,
    ) -> (Option<Placement>, Vec<String>, bool) {
        let mut stage = Stage::default();
        let mut decided = None;
        let shown = workspaces
            .with_placement(TAB, Some(revision), watching, |placement| {
                decided = Some(placement);
                stage_pane(&mut stage, placement, BOUNDS, true)
            })
            .unwrap();
        (decided, stage.0, shown)
    }

    fn moves(shield: &str, shown: bool) -> Vec<String> {
        let mut moves = vec![format!("bounds {BOUNDS:?}"), shield.to_string()];
        if shown {
            moves.push("show".to_string());
        }
        moves
    }

    /// (가) An agent's tab the person watches is shown for their eyes only:
    /// the shield goes up over the pane's frame before the pane is shown, and
    /// the person's own commands stay refused while the agent holds it.
    #[test]
    fn a_watched_agent_tab_is_shown_under_the_shield_and_the_persons_input_is_refused() {
        let workspaces = owned();
        observe(&workspaces);
        let (placement, staged, shown) = place(&workspaces, revision(&workspaces), true);
        assert_eq!(
            placement,
            Some(Placement::Watch),
            "a watched agent tab is placed for the person's eyes"
        );
        assert_eq!(
            staged,
            moves("shield up", true),
            "the shield goes up over the pane's frame before the pane is shown"
        );
        assert!(shown);
        assert!(
            workspaces
                .begin_person(TAB, Some(revision(&workspaces)))
                .is_err(),
            "the person's own commands stay refused while the agent holds the tab"
        );
        let (placement, staged, shown) = place(&workspaces, revision(&workspaces), false);
        assert_eq!(placement, Some(Placement::Hidden));
        assert_eq!(staged, moves("shield down", false));
        assert!(!shown, "an agent tab nobody watches stays hidden");
        let (placement, _, shown) = place(&workspaces, revision(&workspaces) - 1, true);
        assert_eq!(placement, Some(Placement::Hidden));
        assert!(!shown, "a watch placed by an older revision shows nothing");
    }

    /// (나) The person's 「내가 조작」: the tab placed for their hand has its
    /// shield taken down before the pane is shown again.
    #[test]
    fn taking_the_tab_by_hand_takes_the_shield_down_before_the_pane_is_shown() {
        let workspaces = owned();
        observe(&workspaces);
        let (placement, ..) = place(&workspaces, revision(&workspaces), true);
        assert_eq!(
            placement,
            Some(Placement::Watch),
            "the tab was watched under the shield first"
        );
        hand_over(&workspaces, Mode::Human);
        let (placement, staged, shown) = place(&workspaces, revision(&workspaces), false);
        assert_eq!(placement, Some(Placement::Control));
        assert_eq!(
            staged,
            moves("shield down", true),
            "the shield comes down before the pane in the person's hands is shown"
        );
        assert!(shown);
        hand_over(&workspaces, Mode::Agent);
        let (placement, staged, shown) = place(&workspaces, revision(&workspaces), false);
        assert_eq!(placement, Some(Placement::Hidden));
        assert_eq!(staged, moves("shield down", false));
        assert!(
            !shown,
            "handed back, the tab is hidden until the person watches it"
        );
    }

    /// (마) A watch frame that meets a step's evidence capture in flight waits
    /// for it and is taken next, instead of being refused.
    #[tokio::test]
    async fn a_watch_frame_waits_for_a_steps_evidence_capture_instead_of_being_refused() {
        let workspaces = owned();
        observe(&workspaces);
        let step = workspaces
            .begin(TAB, Some("term-1"), "click")
            .unwrap()
            .unwrap();
        let control = step.step_control();
        let (evidence, _check) = workspaces
            .admit_evidence(TAB, control.as_ref())
            .unwrap()
            .unwrap();
        let watching = {
            let workspaces = workspaces.clone();
            tokio::spawn(async move { workspaces.lease_frame(TAB, true).await.map(drop) })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !watching.is_finished(),
            "the watch frame waits while the step's evidence capture is in flight"
        );
        drop(evidence);
        let watched = watching.await.unwrap();
        assert!(
            watched.is_ok(),
            "the watch frame is taken once the evidence capture is released: {watched:?}"
        );
        step.finish(true);
    }

    /// An agent capture goes first: it waits for the watch frame in flight
    /// instead of being refused, and a watch frame asked meanwhile waits
    /// behind it; a second watch frame asked while one waits is busy.
    #[tokio::test]
    async fn an_agent_capture_waits_for_the_watch_frame_and_goes_before_the_next_one() {
        let workspaces = owned();
        let watch = workspaces.begin_snapshot_for(TAB, true).unwrap();
        let agent = {
            let workspaces = workspaces.clone();
            tokio::spawn(async move { workspaces.lease_frame(TAB, false).await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !agent.is_finished(),
            "the agent's capture waits for the watch frame in flight"
        );
        let next_watch = {
            let workspaces = workspaces.clone();
            tokio::spawn(async move { workspaces.lease_frame(TAB, true).await.map(drop) })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            workspaces.lease_frame(TAB, true).await.is_err(),
            "one watch frame waits at a time"
        );
        drop(watch);
        let agent = agent.await.unwrap();
        assert!(
            agent.is_ok(),
            "the agent's capture is taken once the watch frame is released: {:?}",
            agent.as_ref().err()
        );
        assert!(
            !next_watch.is_finished(),
            "the waiting watch frame goes after the agent's capture"
        );
        drop(agent);
        assert!(next_watch.await.unwrap().is_ok());
    }

    /// The watch loop's own clock runs a little ahead of the shell's now and
    /// then: a watch frame asked before its second is over waits out the rest
    /// of it instead of being refused.
    #[tokio::test]
    async fn a_watch_frame_asked_early_in_its_second_waits_out_the_second() {
        let workspaces = owned();
        drop(workspaces.begin_snapshot_for(TAB, true).unwrap());
        let asked = Instant::now();
        let frame = workspaces.lease_frame(TAB, true).await;
        assert!(
            frame.is_ok(),
            "an early watch frame waits out its second: {:?}",
            frame.as_ref().err()
        );
        assert!(asked.elapsed() >= FRAME_INTERVAL - Duration::from_millis(100));
    }

    /// A step's evidence takes its turn before a watch frame: the turn waits
    /// for the watch frame in flight, and holds the next one off until the
    /// evidence's own lease is taken.
    #[tokio::test]
    async fn a_steps_evidence_takes_its_turn_before_the_next_watch_frame() {
        let workspaces = owned();
        observe(&workspaces);
        let watch = workspaces.begin_snapshot_for(TAB, true).unwrap();
        let turn = {
            let workspaces = workspaces.clone();
            tokio::spawn(async move { workspaces.agent_turn(TAB).await.map(drop) })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !turn.is_finished(),
            "the evidence's turn waits for the watch frame in flight"
        );
        drop(watch);
        assert!(turn.await.unwrap().is_ok());
        // The watch frame's second is over: only the turn holds the next off.
        workspaces.frame_slot(TAB).unwrap().lock().unwrap().started = None;
        let turn = workspaces.agent_turn(TAB).await.unwrap();
        assert!(
            workspaces.begin_snapshot_for(TAB, true).is_err(),
            "no watch frame starts while the evidence holds its turn"
        );
        let step = workspaces
            .begin(TAB, Some("term-1"), "click")
            .unwrap()
            .unwrap();
        let control = step.step_control();
        let evidence = workspaces.admit_evidence(TAB, control.as_ref()).unwrap();
        assert!(
            evidence.is_some(),
            "the evidence's own admission takes the lease its turn kept"
        );
        drop(turn);
        drop(evidence);
        assert!(workspaces.begin_snapshot_for(TAB, true).is_ok());
        step.finish(true);
    }
}
