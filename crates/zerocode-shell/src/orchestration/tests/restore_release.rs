//! t-46882: a window restart brings no ledger pane back on a release the
//! catalog has superseded.
//!
//! The person, 10-11 16:5x: 「페이블 5.1이 있는데 5를 왜」. The coordinator's
//! conversation came back as `claude --resume <id>` with no `--model`, so the
//! CLI ran the selection its session had saved (`claude-fable-5`) while the
//! ledger's own catalog held only `claude-fable-5-1`; a sleeper whose row
//! named the older release would have been reseated on it as named.
//!
//! Driven through a private window's production doors, as `restore.rs` is:
//! the goodbye, the next boot's sweep, the doors' wakes through the fake
//! launcher and the ledger's own reseat, for each CLI the ledger resumes —
//! Claude Code (`--resume`), Codex (`resume`) and zo (`--resume`) — and the
//! beat's pass for a pane that came back before the window held a lineup.
//! Nothing here reads a person's account, settings or sessions: the lineup
//! is handed over, and every transcript is a file in the test's own folder.

use super::restore::{Restoring, a_run, a_worker, census_without_commands, the_window_goes};
use super::restore_door::{Door, coordinator_back, the_next_boot};
use super::*;
use std::path::{Path, PathBuf};
use zerocode_core::summon_difficulty::lineup::{Lineup, Seen};

/// Today's catalog, as `zo models --json` answers it: Claude's
/// `claude-fable-5` superseded by `claude-fable-5-1`, Codex's `gpt-6-sol` by
/// `gpt-6.1-sol`.
fn todays_catalog() -> serde_json::Value {
    serde_json::json!({"models": [
        {"provider": "anthropic", "id": "claude-fable-5", "band": "superseded", "supersededBy": "claude-fable-5-1"},
        {"provider": "anthropic", "id": "claude-fable-5-1", "band": "top", "rungs": []},
        {"provider": "anthropic", "id": "claude-opus-5-5", "band": "second", "rungs": ["hard"]},
        {"provider": "openai", "id": "gpt-6-sol", "band": "superseded", "supersededBy": "gpt-6.1-sol"},
        {"provider": "openai", "id": "gpt-6.1-sol", "band": "second", "rungs": ["hard"]},
    ]})
}

/// The releases today's catalog superseded, and the one standing for each —
/// read off the catalog itself, so what a test counts as left behind does
/// not hang on the resolver it is testing.
fn superseded_today() -> std::collections::BTreeMap<String, String> {
    todays_catalog()["models"]
        .as_array()
        .expect("rows")
        .iter()
        .filter(|row| row["band"] == "superseded")
        .map(|row| {
            (
                row["id"].as_str().expect("an id").to_string(),
                row["supersededBy"]
                    .as_str()
                    .expect("its successor")
                    .to_string(),
            )
        })
        .collect()
}

/// The lineup the window holds once it has read today's catalog — and zo,
/// which runs any provider's model, holding every row.
fn todays_lineup() -> crate::summon_lineup::Snapshot {
    let catalog = todays_catalog();
    let one = |provider: &str| Lineup::from_catalog(&catalog, provider).expect("a lineup");
    crate::summon_lineup::Snapshot {
        lineups: std::collections::BTreeMap::from([
            ("claude".to_string(), one("anthropic")),
            ("codex".to_string(), one("openai")),
            (
                "zo".to_string(),
                Lineup::from_catalog_all(&catalog).expect("every provider's rows"),
            ),
        ]),
        seen: Seen::default(),
    }
}

/// One answer `model` wrote at `at_ms`, appended in the shape `agent`'s CLI
/// writes it: Claude Code's assistant record, zo's message record, Codex's
/// turn context.
fn append_answer(path: &Path, agent: &str, model: &str, at_ms: i64) {
    use std::io::Write as _;
    let stamp = zerocode_core::civil::iso_utc_of(at_ms);
    let line = match agent {
        "codex" => serde_json::json!({
            "type": "turn_context", "timestamp": stamp, "payload": {"model": model},
        }),
        "zo" => serde_json::json!({
            "type": "message", "updated_at_ms": at_ms,
            "message": {"role": "assistant", "model": model, "blocks": []},
        }),
        _ => serde_json::json!({
            "type": "assistant", "timestamp": stamp,
            "message": {"role": "assistant", "model": model, "content": "ok"},
        }),
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("the transcript");
    writeln!(file, "{line}").expect("one record");
}

/// A conversation's transcript in `dir`, its newest answer `model`'s.
fn transcript(dir: &Path, name: &str, agent: &str, model: &str, at_ms: i64) -> PathBuf {
    let path = dir.join(format!("{name}.jsonl"));
    append_answer(&path, agent, model, at_ms);
    path
}

/// The conversation `id`, as a door's record names it.
fn conversation(id: &str, transcript: Option<&Path>) -> zerocode_core::ProviderSession {
    zerocode_core::ProviderSession {
        key: zerocode_core::SessionKey::SessionId,
        id: id.to_string(),
        transcript_path: transcript.map(|path| path.to_string_lossy().into_owned()),
    }
}

/// The word after `flag` in a command line.
fn flag<'a>(command: &'a str, flag: &str) -> Option<&'a str> {
    let mut words = command.split_whitespace();
    while let Some(word) = words.next() {
        if word == flag {
            return words.next();
        }
    }
    None
}

/// The release a started CLI runs: the `--model` its command names, else the
/// one its conversation's transcript last ran — the selection a resume with
/// no `--model` keeps.
fn runs_on(command: &str, transcript: Option<&Path>) -> Option<String> {
    flag(command, "--model").map(str::to_string).or_else(|| {
        let lines = zerocode_core::transcript::tail_lines(transcript?)?;
        zerocode_core::transcript::model_in(&lines.join("\n"))
    })
}

/// t-46882 ①②④ and the number the task asks for: a restart with a run at
/// work — its coordinator's conversation and three workers, one per CLI the
/// ledger resumes, each on a release today's catalog has superseded — brings
/// every ledger pane back on the release that stands for it, through a
/// door's wake and through the ledger's own reseat alike: the panes left on
/// a superseded release go from 4 to 0. A person's own tab and a worker the
/// person took over come back exactly as they were.
#[test]
fn a_restart_brings_no_ledger_pane_back_on_a_superseded_release() {
    const OLD_LEADER: u32 = 4_688_200;
    const FIRST_WORKER: u32 = 4_688_201;
    const NEW_LEADER: u32 = 4_688_230;
    const COORDINATOR_TAB: u32 = 4_688_240;
    const CLAUDE_DOOR: u32 = 4_688_241;
    const ZO_DOOR: u32 = 4_688_242;
    const PERSONS_TAB: u32 = 4_688_243;
    const CLAUDE_SESSION: &str = "46882c1a-0000-4000-8000-0000000000c1";
    const TAKEN_SESSION: &str = "46882c1a-0000-4000-8000-0000000000c4";
    const PERSONS_SESSION: &str = "46882c1a-0000-4000-8000-0000000000e1";
    let (_window, _store) = PrivateWindow::boot();
    let checkout = tempfile::tempdir().expect("the workers' checkout");
    let written = tempfile::tempdir().expect("the conversations' transcripts");
    let host = Restoring::new(
        checkout.path(),
        FIRST_WORKER,
        (NEW_LEADER, test_actor(OLD_LEADER)),
    );
    let (team, _run) = a_run(&host, OLD_LEADER, "superseded");
    let (_, _, claude_term) = a_worker(
        &host,
        &team,
        "--agent claude --model claude-fable-5 --effort xhigh",
        Some(CLAUDE_SESSION),
    );
    let (_, _, codex_term) = a_worker(
        &host,
        &team,
        "--agent codex --model gpt-6-sol --effort high",
        Some("session-46882-codex"),
    );
    let (_, _, zo_term) = a_worker(
        &host,
        &team,
        "--agent zo --model claude-fable-5 --effort max",
        Some("session-46882-zo"),
    );
    let (_, taken, taken_term) = a_worker(
        &host,
        &team,
        "--agent claude --model claude-fable-5 --effort xhigh",
        Some(TAKEN_SESSION),
    );
    super::super::pane_taken_over(taken_term, "words", clock());
    assert!(
        super::restore::row(&taken).taken_over,
        "the person's key never reached the row"
    );
    the_window_goes(
        &census_without_commands,
        &[claude_term, codex_term, zo_term, taken_term],
    );
    the_next_boot(OLD_LEADER);
    // The next window has read today's lineup.
    super::super::hold_lineup_for_tests(Some(todays_lineup()));
    host.cut.lock().unwrap().clear();

    // Restored tabs first: the coordinator's own conversation, two sleepers'
    // and a person's own tab — whose conversation, like the coordinator's,
    // last ran the superseded release.
    let before_the_restart = clock() - 60_000;
    let coordinators = transcript(
        written.path(),
        "coordinator",
        "claude",
        "claude-fable-5",
        before_the_restart,
    );
    let persons = transcript(
        written.path(),
        "persons-own",
        "claude",
        "claude-fable-5",
        before_the_restart,
    );
    let door = Door::new(checkout.path());
    door.wake_with(
        COORDINATOR_TAB,
        "claude",
        conversation(
            &format!("test-session-of-term-{OLD_LEADER}"),
            Some(&coordinators),
        ),
    )
    .expect("the coordinator's wake");
    door.wake_with(CLAUDE_DOOR, "claude", conversation(CLAUDE_SESSION, None))
        .expect("the claude worker's wake");
    door.wake_with(ZO_DOOR, "zo", conversation("session-46882-zo", None))
        .expect("the zo worker's wake");
    door.wake_with(
        PERSONS_TAB,
        "claude",
        conversation(PERSONS_SESSION, Some(&persons)),
    )
    .expect("the person's own tab");
    // Then the coordinator back in its new leader pane: the ledger reseats
    // the sleepers no door brought back — the Codex worker and the one the
    // person took.
    assert_eq!(
        coordinator_back(&host, "superseded", OLD_LEADER, NEW_LEADER),
        2,
        "the ledger reseated another number of sleepers"
    );

    let started = door.started();
    let at_door = |term: u32| {
        started
            .iter()
            .find(|(at, _)| *at == term)
            .map(|(_, command)| command.clone())
            .unwrap_or_else(|| panic!("no process started in terminal {term}: {started:?}"))
    };
    let cuts = host.cuts();
    let cut_for = |session: &str| {
        cuts.iter()
            .find(|(_, command)| command.contains(session))
            .map(|(_, command)| command.clone())
            .unwrap_or_else(|| panic!("no pane cut for {session}: {cuts:?}"))
    };
    // Every ledger pane: what came back, on which CLI, and on what release.
    let ledger_panes = [
        (
            "the coordinator (claude --resume, door)",
            at_door(COORDINATOR_TAB),
            Some(coordinators.as_path()),
        ),
        (
            "the claude worker (claude --resume, door)",
            at_door(CLAUDE_DOOR),
            None,
        ),
        ("the zo worker (zo --resume, door)", at_door(ZO_DOOR), None),
        (
            "the codex worker (codex resume, the ledger's reseat)",
            cut_for("session-46882-codex"),
            None,
        ),
    ];
    let superseded = superseded_today();
    let left: Vec<String> = ledger_panes
        .iter()
        .filter_map(|(who, command, transcript)| {
            let model = runs_on(command, *transcript)?;
            superseded
                .get(&model)
                .map(|standing| format!("{who} on {model}, not {standing}: {command}"))
        })
        .collect();
    eprintln!(
        "t-46882 count: ledger panes left on a superseded release after the restart: {} of {}",
        left.len(),
        ledger_panes.len()
    );
    assert!(
        left.is_empty(),
        "{} ledger pane(s) came back on a superseded release:\n{}",
        left.len(),
        left.join("\n")
    );
    // Each CLI's own resume spelling still selects the conversation.
    for (who, selector, command) in [
        (
            "claude",
            format!("--resume {CLAUDE_SESSION}"),
            at_door(CLAUDE_DOOR),
        ),
        (
            "zo",
            "--resume session-46882-zo".to_string(),
            at_door(ZO_DOOR),
        ),
        (
            "codex",
            "resume session-46882-codex".to_string(),
            cut_for("session-46882-codex"),
        ),
    ] {
        assert!(
            command.contains(&selector),
            "{who}'s conversation is not the one resumed: {command}"
        );
    }
    // The effort each worker was summoned with rides beside the release.
    assert_eq!(flag(&at_door(CLAUDE_DOOR), "--effort"), Some("xhigh"));
    assert!(
        cut_for("session-46882-codex").contains("model_reasoning_effort=high"),
        "{}",
        cut_for("session-46882-codex")
    );

    // ② A person's own tab is theirs: no `--model` is put on it.
    assert_eq!(
        flag(&at_door(PERSONS_TAB), "--model"),
        None,
        "a person's own tab was given a model: {}",
        at_door(PERSONS_TAB)
    );
    // ② And so is a worker the person took: its row's release, as named.
    assert_eq!(
        flag(&cut_for(TAKEN_SESSION), "--model"),
        Some("claude-fable-5"),
        "a worker the person took came back on another model: {}",
        cut_for(TAKEN_SESSION)
    );
    for term in [
        COORDINATOR_TAB,
        CLAUDE_DOOR,
        ZO_DOOR,
        PERSONS_TAB,
        NEW_LEADER,
    ]
    .into_iter()
    .chain(cuts.iter().map(|(term, _)| *term))
    {
        super::super::superseded::forget(term);
        crate::agent_teams::forget_term(term);
    }
}

/// t-46882: the beat reads a restored pane's release at its rest BEFORE the
/// mail pointer can type into that pane and start its next turn — the first
/// gap after a turn, kept by the source.
#[test]
fn the_beat_moves_a_restored_pane_before_the_pointer_types() {
    let shipped = include_str!("../../orchestration.rs");
    let start = shipped
        .find("pub(crate) fn tick(")
        .expect("the beat's tick");
    let tick = &shipped[start..];
    let tick = &tick[..tick.find("\n}\n").expect("the end of the tick")];
    let moved = tick
        .find("superseded::sweep(host);")
        .expect("the beat reads restored panes' releases");
    let point = tick
        .find("point_at_waiting_mail(host, now_ms);")
        .expect("the beat points at mail");
    assert!(
        moved < point,
        "the pointer types before the release is moved"
    );
}

/// t-46882 ③: a ledger pane that came back before the window held a lineup
/// — the read is off the beat at boot, and a resume does not wait for it —
/// came back on the selection its session saved. Once the lineup is read and
/// the pane has run a turn on the superseded release, it is moved to the
/// release that stands at the rest that turn ends in: once, through the
/// agent's own line (`/model <id>`), and never again. A person's own tab,
/// and a pane the person's hand has reached since it came back, are left as
/// they are.
#[test]
fn a_ledger_pane_that_came_back_before_the_lineup_was_read_is_moved_at_its_first_rest() {
    const OLD_LEADER: u32 = 4_688_300;
    const COORDINATOR_TAB: u32 = 4_688_340;
    const PERSONS_TAB: u32 = 4_688_341;
    const HELD_TAB: u32 = 4_688_342;
    let (_window, _store) = PrivateWindow::boot();
    let _beat = one_beat_at_a_time();
    let checkout = tempfile::tempdir().expect("the checkout");
    let written = tempfile::tempdir().expect("the conversations' transcripts");
    let host = Restoring::new(
        checkout.path(),
        OLD_LEADER + 1,
        (OLD_LEADER, test_actor(OLD_LEADER)),
    );
    let (_team, _run) = a_run(&host, OLD_LEADER, "unread");
    let (_held_team, _held_run) = a_run(&host, OLD_LEADER + 2, "held");
    the_window_goes(&census_without_commands, &[]);
    the_next_boot(OLD_LEADER);
    crate::agent_teams::forget_term(OLD_LEADER + 2);
    super::super::hold_lineup_for_tests(None);

    let before_the_restart = clock() - 60_000;
    let door = Door::new(checkout.path());
    let mut transcripts = Vec::new();
    for (term, session) in [
        (
            COORDINATOR_TAB,
            format!("test-session-of-term-{OLD_LEADER}"),
        ),
        (
            PERSONS_TAB,
            "46882c1a-0000-4000-8000-0000000000e2".to_string(),
        ),
        (HELD_TAB, format!("test-session-of-term-{}", OLD_LEADER + 2)),
    ] {
        let path = transcript(
            written.path(),
            &format!("t{term}"),
            "claude",
            "claude-fable-5",
            before_the_restart,
        );
        door.wake_with(term, "claude", conversation(&session, Some(&path)))
            .expect("the wake");
        assert_eq!(
            flag(&door.started().last().expect("a start").1, "--model"),
            None,
            "no lineup was held, and yet a release was named"
        );
        transcripts.push((term, path));
    }
    // The lineup lands; every pane rests on a conversation nothing has
    // answered in since it came back: nothing is known of the release it
    // runs, and nothing is typed.
    super::super::hold_lineup_for_tests(Some(todays_lineup()));
    for (term, _) in &transcripts {
        super::super::pane_lead_rested(*term, clock(), false);
    }
    super::super::superseded::sweep(&host);
    for (term, _) in &transcripts {
        assert!(
            host.typed_at(*term).is_empty(),
            "terminal {term} was typed at before it ran a turn: {:?}",
            host.typed_at(*term)
        );
    }
    // A person's hand reaches the held pane's line; then every pane runs a
    // turn on the superseded release, and rests.
    crate::human_input::typed(HELD_TAB);
    let answered = clock() + 1_000;
    for (term, path) in &transcripts {
        append_answer(path, "claude", "claude-fable-5", answered);
        super::super::pane_turn_began(*term, answered);
        super::super::pane_lead_rested(*term, answered + 1, false);
    }
    super::super::superseded::sweep(&host);
    super::super::superseded::sweep(&host);
    assert_eq!(
        host.typed_at(COORDINATOR_TAB),
        ["/model claude-fable-5-1", "\r"],
        "the coordinator was not moved to the release that stands, once"
    );
    assert!(
        host.typed_at(PERSONS_TAB).is_empty(),
        "a person's own tab was moved: {:?}",
        host.typed_at(PERSONS_TAB)
    );
    assert!(
        host.typed_at(HELD_TAB).is_empty(),
        "a pane the person's hand reached was moved: {:?}",
        host.typed_at(HELD_TAB)
    );
    crate::human_input::forget_term(HELD_TAB);
    for (term, _) in &transcripts {
        super::super::terminal_gone(*term, clock());
        crate::agent_teams::forget_term(*term);
    }
}
