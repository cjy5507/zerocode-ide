//! The seat on a scripted wire: what leaves, what comes back, what the row
//! says, what the surface is handed, what the label writes. No machine
//! settings, no network — a mock on a loopback port, a door told its facts,
//! a ledger in a directory of its own.

use std::path::Path;
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

use api::SystemOneClient;
use zerocode_core::jev::door::JevSettings;
use zerocode_core::jev::{JevMode, MENTION_CANDIDATE_CAP, MENTION_HEAD_BYTE_CAP, ROUTE_USE_APPLIED, ROUTE_USE_FALLBACK};

use super::super::settings::JEV_MENTION_RERANK_SETTING;
use super::super::shadow_ledger::read_shadow_rows;
use super::super::jev_mock::Mock;
use super::*;

/// The workspace every page here comes from, consented at a door that
/// reads no machine's settings.
const WORKSPACE: &str = "/work/zo";

fn door(home: &Path) -> JevDoor {
    let settings = JevSettings {
        enabled: true,
        workspaces: vec![WORKSPACE.to_string()],
        daily_requests: None,
        model: zerocode_core::jev::DEFAULT_MODEL.to_string(),
    };
    JevDoor::at(settings, Path::new(WORKSPACE), home)
}

fn candidate(name: &str, head: &str) -> MentionCandidate {
    MentionCandidate {
        name: name.to_string(),
        head: head.to_string(),
    }
}

/// A page of three files the fuzzy search put in a deliberately unhelpful
/// order for a sentence about the composer.
fn page(intent: &str) -> MentionAsk {
    MentionAsk {
        surface: MentionSurface::Mention,
        intent: intent.to_string(),
        query: "comp".to_string(),
        candidates: vec![
            candidate("src/compact/mod.rs", ""),
            candidate("src/tui/composer.rs", ""),
            candidate("src/compose_tests.rs", ""),
        ],
    }
}

/// A choice answer over `n` options whose probabilities are `shares`, in
/// option order, choosing the largest.
fn answer(shares: &[f64]) -> serde_json::Value {
    let probabilities: serde_json::Map<String, serde_json::Value> = shares
        .iter()
        .enumerate()
        .map(|(position, share)| (option_id(position), serde_json::json!(share)))
        .collect();
    let chosen = shares
        .iter()
        .enumerate()
        .max_by(|left, right| left.1.partial_cmp(right.1).expect("a share"))
        .map(|(position, _)| option_id(position))
        .expect("an option");
    serde_json::json!({
        "type": "choice",
        "choice": chosen,
        "probabilities": probabilities,
        "confidence": 0.8,
    })
}

fn reply(shares: &[f64]) -> String {
    serde_json::json!({
        "model": "jev-test",
        "answers": { QUESTION: answer(shares) },
        "usage": {"input_tokens": 321, "output_tokens": 12}
    })
    .to_string()
}

fn judged(base_url: &str, ask: &MentionAsk) -> (MentionRerankRow, Option<Vec<usize>>) {
    let home = tempfile::tempdir().expect("a config home");
    let client = SystemOneClient::new(base_url, "test-key");
    let door = door(home.path());
    api::sync_bridge::run_blocking(judge(&door, Some(&client), ask))
}

#[test]
fn the_setting_reads_the_tables_words_and_a_slip_is_off() {
    let home = tempfile::tempdir().expect("home");
    let cwd = tempfile::tempdir().expect("cwd");
    for (word, expected) in [
        ("shadow", Some(JevMode::Shadow)),
        ("on", Some(JevMode::On)),
        ("auto", Some(JevMode::Auto)),
        ("shadwo", Some(JevMode::Off)),
    ] {
        std::fs::write(
            home.path().join("settings.json"),
            serde_json::json!({ "smart": { JEV_MENTION_RERANK_SETTING: word } }).to_string(),
        )
        .expect("settings");
        let loader = runtime::ConfigLoader::new(cwd.path(), home.path());
        assert_eq!(jev_mention_rerank_mode_from(&loader), expected, "jevMentionRerank = {word}");
    }
    std::fs::write(home.path().join("settings.json"), "{}").expect("settings");
    let loader = runtime::ConfigLoader::new(cwd.path(), home.path());
    assert_eq!(jev_mention_rerank_mode_from(&loader), Some(JevMode::Off), "no word is off");
    assert_eq!(JEV_MENTION_RERANK_SETTING, "jevMentionRerank");
}

/// The words the judgment is shown are pinned: a rewording is a version.
#[test]
fn the_rubric_is_pinned_to_its_version() {
    assert_eq!(MENTION_RUBRIC_VERSION, 1);
    assert_eq!(rubric_pin(), "3f155c8d1efc284d", "the rubric's words moved: bump MENTION_RUBRIC_VERSION");
}

#[test]
fn an_answered_page_writes_the_fuzzy_and_proposed_names_and_never_the_words() {
    let mock = Mock::serving(200, reply(&[0.1, 0.7, 0.2]));
    let ask = page("fix the caret in the INTENT-SENTINEL composer");

    let (row, order) = judged(&mock.base_url, &ask);

    assert_eq!(row.outcome, MENTION_OUTCOME_ANSWERED);
    assert_eq!(order, Some(vec![1, 2, 0]), "by probability, highest first");
    assert_eq!(row.candidates, 3);
    assert_eq!(row.surface, MentionSurface::Mention);
    assert_eq!(row.model.as_deref(), Some("jev-test"));
    assert_eq!(row.input_tokens, Some(321));
    assert_eq!((row.requests, row.redacted_lines), (1, 0));
    assert!(!row.cached && row.retries == 0);
    let judged = row.judged.as_ref().expect("a checked judgment");
    assert_eq!(judged.fuzzy, ["src/compact/mod.rs", "src/tui/composer.rs", "src/compose_tests.rs"]);
    assert_eq!(judged.proposed, ["src/tui/composer.rs", "src/compose_tests.rs", "src/compact/mod.rs"]);
    assert!(judged.top_changed);
    assert!((judged.confidence - 0.8).abs() < f64::EPSILON);
    // The row is fingerprints and names.
    let line = serde_json::to_string(&row).expect("a ledger line");
    assert!(line.contains("src/tui/composer.rs"));
    assert!(!line.contains("INTENT-SENTINEL"), "the sentence never reaches the ledger: {line}");
    assert!(!line.contains("\"comp\""), "the token never reaches the ledger: {line}");
    assert_ne!(row.query, 0);
    assert_ne!(row.notes, 0);
    assert_eq!(row.rubric_version, MENTION_RUBRIC_VERSION);
    // The wire saw the sentence, the token, the names, and one choice.
    let sent = mock.requests();
    assert_eq!(sent.len(), 1);
    let body: serde_json::Value = serde_json::from_str(&sent[0]).expect("a JSON request");
    assert_eq!(body["state"]["intent"], "fix the caret in the INTENT-SENTINEL composer");
    assert_eq!(body["state"]["query"], "comp");
    assert_eq!(body["state"]["candidates"][1]["name"], "src/tui/composer.rs");
    assert_eq!(body["state"]["candidates"][1].get("head"), None, "an empty head is not sent");
    assert_eq!(body["questions"][QUESTION]["type"], "choice");
    assert_eq!(body["questions"][QUESTION]["criteria"]["c1"], "`candidates[1]`");
    assert!(!sent[0].contains("/Users/"), "no absolute path leaves the machine: {}", sent[0]);
}

/// Equal probabilities keep the fuzzy order between them: the judgment
/// moves only what it had an opinion about.
#[test]
fn a_judgment_with_no_opinion_keeps_the_fuzzy_order() {
    let mock = Mock::serving(200, reply(&[0.34, 0.33, 0.33]));
    let (row, order) = judged(&mock.base_url, &page("anything (no opinion)"));
    assert_eq!(order, Some(vec![0, 1, 2]));
    assert!(!row.judged.expect("judged").top_changed);
}

/// A head rides along cut to the row's cap; a ninth candidate is not asked
/// about at all.
#[test]
fn a_head_is_cut_at_the_cap_and_the_page_is_one_page() {
    let mock = Mock::serving(200, reply(&[0.5, 0.5]));
    let long_head = "h".repeat(MENTION_HEAD_BYTE_CAP + 40);
    let mut ask = MentionAsk {
        surface: MentionSurface::Resume,
        intent: String::new(),
        query: "parser".to_string(),
        candidates: (0..MENTION_CANDIDATE_CAP + 2)
            .map(|n| candidate(&format!("session-{n}-0"), &long_head))
            .collect(),
    };
    ask = cut_to_caps(ask);
    assert_eq!(ask.candidates.len(), MENTION_CANDIDATE_CAP, "one page, never a ninth row");
    assert!(ask.candidates[0].head.len() <= MENTION_HEAD_BYTE_CAP + zerocode_core::jev::CUT_MARK.len());
    assert!(ask.candidates[0].head.ends_with(zerocode_core::jev::CUT_MARK));
    let (row, _) = judged(&mock.base_url, &ask);
    assert_eq!(row.surface, MentionSurface::Resume);
    let body: serde_json::Value = serde_json::from_str(&mock.requests()[0]).expect("json");
    assert_eq!(body["state"]["candidates"].as_array().map(Vec::len), Some(MENTION_CANDIDATE_CAP));
    assert_eq!(body["state"]["intent"], "", "a resume search has no sentence but the words typed");
}

#[test]
fn a_keyless_page_is_refused_at_the_door_and_says_so() {
    let home = tempfile::tempdir().expect("a config home");
    let door = door(home.path());
    let (row, order) = api::sync_bridge::run_blocking(judge(&door, None, &page("anything (no key)")));
    assert_eq!(row.outcome, zerocode_core::jev::door::Refused::NoKey.token());
    assert_eq!(order, None);
    assert_eq!((row.requests, row.redacted_lines), (0, 0));
    assert!(row.judged.is_none());
}

#[test]
fn a_reply_that_fails_the_checks_is_a_schema_row_that_still_bills() {
    let mut broken: serde_json::Value = serde_json::from_str(&reply(&[0.1, 0.9, 0.0])).expect("json");
    broken["answers"][QUESTION]["choice"] = serde_json::json!("c9");
    let mock = Mock::serving(200, broken.to_string());
    let (row, order) = judged(&mock.base_url, &page("anything (schema)"));
    assert_eq!(row.outcome, api::SystemOneFailure::Schema.ledger_token());
    assert_eq!(row.rejected.as_deref(), Some(choice::ChoiceRefusal::UnknownOption.token()));
    assert_eq!(order, None);
    assert!(row.judged.is_none());
    assert_eq!(row.input_tokens, Some(321), "an answer that arrived and failed still billed");
}

#[test]
fn the_same_page_is_asked_once_and_recalled_after() {
    let mock = Mock::serving(200, reply(&[0.2, 0.6, 0.2]));
    let ask = page("the same words (memo)");
    let (first, first_order) = judged(&mock.base_url, &ask);
    let (second, second_order) = judged(&mock.base_url, &ask);
    assert!(!first.cached && second.cached);
    assert_eq!(first_order, second_order);
    assert_eq!(first.judged, second.judged);
    assert_eq!(mock.requests().len(), 1, "the memo answers the second page");
}

/// A credential in the sentence never reaches the wire, and the row counts
/// what was withheld.
#[test]
fn a_credential_in_the_sentence_stays_behind_the_door() {
    let mock = Mock::serving(200, reply(&[0.5, 0.3, 0.2]));
    let ask = page("deploy with\nexport DEPLOY_TOKEN=sk-live-SENTINEL\nthen fix the composer");
    let (row, _) = judged(&mock.base_url, &ask);
    let sent = mock.requests();
    assert_eq!(sent.len(), 1);
    assert!(!sent[0].contains("SENTINEL"), "a credential left the door: {}", sent[0]);
    assert_eq!(row.redacted_lines, 1);
    assert_eq!(row.outcome, MENTION_OUTCOME_ANSWERED);
}

/* ---- the seat: armed at a boundary, asked per page, told the pick ------ */

/// The whole of what the seat reads: a config home holding one consented
/// workspace and the seat's switch set to `mode`, a key, and a mock origin.
fn machine<T>(mode: &str, base_url: &str, body: impl FnOnce(&Path) -> T) -> T {
    let home = tempfile::tempdir().expect("a config home");
    let work = tempfile::tempdir().expect("a workspace");
    let cwd = std::fs::canonicalize(work.path()).expect("the workspace resolved");
    std::fs::write(
        home.path().join("settings.json"),
        serde_json::json!({
            zerocode_core::jev::SMART_SETTINGS_KEY: {
                MENTION_RERANK.setting: mode,
                "jev": {"enabled": true, "workspaces": [cwd.to_string_lossy()]},
            }
        })
        .to_string(),
    )
    .expect("a settings file");
    let _env = crate::tests::EnvGuard::set("ZO_CONFIG_HOME", &home.path().to_string_lossy())
        .set_also("ZO_HOME", home.path())
        .set_also("HOME", home.path())
        .set_also(core_types::paths::ZO_STATE_DIR_ENV, home.path())
        .set_also(api::SYSTEMONE_API_KEY_ENV, "test-key")
        .set_also(api::SYSTEMONE_BASE_URL_ENV, base_url);
    body(&cwd)
}

/// A seat whose answers land on a channel the test reads.
fn seat(cwd: &Path) -> (MentionRerank, Receiver<MentionAnswer>) {
    let (tx, rx) = channel();
    let seat = MentionRerank::at(cwd, move |answer| {
        let _ = tx.send(answer);
    });
    (seat, rx)
}

fn rows(cwd: &Path) -> Vec<MentionRerankRow> {
    read_shadow_rows(&mention_rerank_path(cwd))
}

fn labels(cwd: &Path) -> Vec<MentionLabelRow> {
    read_shadow_rows(&mention_rerank_path(cwd))
}

/// Wait for the ledger to hold `n` reading rows: the row lands after the
/// answer was delivered, on a blocking task of its own.
fn rows_settled(cwd: &Path, n: usize) -> Vec<MentionRerankRow> {
    let waited = std::time::Instant::now();
    while rows(cwd).len() < n && waited.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    rows(cwd)
}

const ANSWER_WAIT: Duration = Duration::from_secs(5);

/// `off` is bytes-identical to a seat that does not exist: nothing armed,
/// nothing asked, nothing sent, nothing written.
#[test]
fn off_arms_nothing_asks_nothing_and_writes_nothing() {
    let mock = Mock::serving(200, reply(&[0.1, 0.7, 0.2]));
    machine(JevMode::Off.key(), &mock.base_url, |cwd| {
        let (seat, rx) = seat(cwd);
        seat.arm_now();
        assert!(!seat.asks());
        assert_eq!(seat.ask(page("anything (off)")), None);
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        assert!(rows(cwd).is_empty() && mock.requests().is_empty());
        assert!(!seat.note_chosen(1, Some(0), false));
    });
}

/// A recording mode asks and hands the surface an order it may not apply,
/// and the row says so.
#[test]
fn shadow_delivers_an_order_that_does_not_apply_and_the_row_says_so() {
    let mock = Mock::serving(200, reply(&[0.1, 0.7, 0.2]));
    machine(JevMode::Shadow.key(), &mock.base_url, |cwd| {
        let (seat, rx) = seat(cwd);
        seat.arm_now();
        assert!(seat.asks());
        let ticket = seat.ask(page("which file holds the caret (shadow)")).expect("a question left");
        let answer = rx.recv_timeout(ANSWER_WAIT).expect("an answer");
        assert_eq!(answer, MentionAnswer { ticket, surface: MentionSurface::Mention, order: vec![1, 2, 0], applies: false });
        let rows = rows_settled(cwd, 1);
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].applied);
        assert_eq!(rows[0].route_use, JevMode::Shadow.key());
        assert_eq!(rows[0].outcome, MENTION_OUTCOME_ANSWERED);
    });
}

#[test]
fn on_delivers_an_order_that_applies_and_the_row_says_so() {
    let mock = Mock::serving(200, reply(&[0.1, 0.7, 0.2]));
    machine(JevMode::On.key(), &mock.base_url, |cwd| {
        let (seat, rx) = seat(cwd);
        seat.arm_now();
        let ticket = seat.ask(page("which file holds the caret (on)")).expect("a question left");
        let answer = rx.recv_timeout(ANSWER_WAIT).expect("an answer");
        assert_eq!((answer.ticket, answer.applies), (ticket, true));
        assert_eq!(answer.order, vec![1, 2, 0]);
        let rows = rows_settled(cwd, 1);
        assert!(rows[0].applied);
        assert_eq!(rows[0].route_use, ROUTE_USE_APPLIED);
    });
}

/// A page of one row, or an empty token, asks nothing: there is nothing to
/// reorder and nothing to rank by.
#[test]
fn a_page_of_one_row_or_an_empty_token_asks_nothing() {
    let mock = Mock::serving(200, reply(&[1.0]));
    machine(JevMode::On.key(), &mock.base_url, |cwd| {
        let (seat, _rx) = seat(cwd);
        seat.arm_now();
        let mut one = page("one row");
        one.candidates.truncate(1);
        assert_eq!(seat.ask(one), None);
        let mut bare = page("bare token");
        bare.query = "  ".to_string();
        assert_eq!(seat.ask(bare), None);
        assert!(mock.requests().is_empty());
    });
}

/// A newer keystroke abandons the older question: only the latest ticket's
/// answer ever reaches the surface, and the wire is not waited on for the
/// old one.
#[test]
fn a_newer_question_abandons_the_older_one() {
    let mock = Mock::slow_first(Duration::from_secs(1), reply(&[0.1, 0.7, 0.2]));
    machine(JevMode::On.key(), &mock.base_url, |cwd| {
        let (seat, rx) = seat(cwd);
        seat.arm_now();
        let older = seat.ask(page("the first keystroke (abandon)")).expect("asked");
        let newer = seat.ask(page("the second keystroke (abandon)")).expect("asked");
        assert!(newer > older);
        let answer = rx.recv_timeout(ANSWER_WAIT).expect("the newer answer");
        assert_eq!(answer.ticket, newer);
        assert!(
            rx.recv_timeout(Duration::from_millis(1_500)).is_err(),
            "the older question's answer was delivered after it was abandoned"
        );
    });
}

/// A wire that never answers delivers nothing — the fuzzy page stands —
/// and the row names the timeout.
#[test]
fn a_silent_wire_delivers_nothing_and_the_row_names_the_timeout() {
    let mock = Mock::silent();
    machine(JevMode::On.key(), &mock.base_url, |cwd| {
        let (seat, rx) = seat(cwd);
        seat.arm_now();
        let _ticket = seat.ask(page("nobody answers (timeout)")).expect("asked");
        assert!(rx.recv_timeout(MENTION_RERANK_DEADLINE + Duration::from_millis(500)).is_err());
        let rows = rows_settled(cwd, 1);
        assert_eq!(rows[0].outcome, api::SystemOneFailure::Timeout.ledger_token());
        assert_eq!(rows[0].route_use, ROUTE_USE_FALLBACK);
        assert!(!rows[0].applied);
    });
}

/// A reply that breaks the contract delivers nothing on the apply road too:
/// the fuzzy page stands and the row says why.
#[test]
fn a_reply_that_breaks_the_contract_delivers_nothing() {
    let mut broken: serde_json::Value = serde_json::from_str(&reply(&[0.1, 0.9, 0.0])).expect("json");
    broken["answers"][QUESTION]["probabilities"]["c0"] = serde_json::json!(0.9);
    let mock = Mock::serving(200, broken.to_string());
    machine(JevMode::On.key(), &mock.base_url, |cwd| {
        let (seat, rx) = seat(cwd);
        seat.arm_now();
        let _ticket = seat.ask(page("a broken reply (schema)")).expect("asked");
        assert!(rx.recv_timeout(Duration::from_secs(2)).is_err());
        let rows = rows_settled(cwd, 1);
        assert_eq!(rows[0].outcome, api::SystemOneFailure::Schema.ledger_token());
        assert_eq!(rows[0].rejected.as_deref(), Some(choice::ChoiceRefusal::NotOne.token()));
    });
}

/// The label (t-5806's pattern): whether the row the person took was the
/// row the judgment put first, and the rank the judgment gave the row they
/// took; `applied` is whether the page had actually been reordered.
#[test]
fn the_label_says_whether_the_person_took_what_the_judgment_put_first() {
    let mock = Mock::serving(200, reply(&[0.1, 0.7, 0.2]));
    machine(JevMode::On.key(), &mock.base_url, |cwd| {
        let (seat, rx) = seat(cwd);
        seat.arm_now();
        let ticket = seat.ask(page("the caret (label agreed)")).expect("asked");
        let answer = rx.recv_timeout(ANSWER_WAIT).expect("an answer");
        assert_eq!(answer.order[0], 1);
        // The person took the page's row 1 — the judgment's first: agreed.
        assert!(seat.note_chosen(ticket, Some(1), true));
        // Nothing settled since: a second label has nothing to grade.
        assert!(!seat.note_chosen(ticket, Some(1), true));
        let labels = labels(cwd);
        assert_eq!(labels.len(), 1, "{labels:?}");
        let label = &labels[0];
        assert_eq!((label.agreed, label.rank, label.applied), (Some(true), Some(0), true));
        // The fuzzy page's own first row was not the one taken: the baseline
        // missed where the judgment called it (t-6342).
        assert_eq!(label.baseline_agreed, Some(false));
        assert_eq!(label.kind, LABEL_ROW_KIND);
        assert_eq!(label.surface, MentionSurface::Mention);
        assert_eq!(label.not_compared, None);
        let row = rows_settled(cwd, 1).remove(0);
        assert_eq!(label.label, format!("{}:{}", row.query, row.notes), "the label names the reading it grades");
    });
}

#[test]
fn a_pick_of_another_row_disagrees_at_its_rank_and_a_pick_off_the_page_is_not_compared() {
    let mock = Mock::serving(200, reply(&[0.1, 0.7, 0.2]));
    machine(JevMode::Shadow.key(), &mock.base_url, |cwd| {
        let (seat, rx) = seat(cwd);
        seat.arm_now();
        let ticket = seat.ask(page("the caret (label rank)")).expect("asked");
        let _answer = rx.recv_timeout(ANSWER_WAIT).expect("an answer");
        // Row 0 of the page sits third in the judgment's order [1, 2, 0].
        assert!(seat.note_chosen(ticket, Some(0), false));
        let ticket = seat.ask(page("the caret (label off the page)")).expect("asked");
        let _answer = rx.recv_timeout(ANSWER_WAIT).expect("an answer");
        assert!(seat.note_chosen(ticket, None, false));
        let labels = labels(cwd);
        assert_eq!(labels.len(), 2, "{labels:?}");
        assert_eq!((labels[0].agreed, labels[0].rank, labels[0].applied), (Some(false), Some(2), false));
        // The page's own first row was taken: the baseline called it.
        assert_eq!(labels[0].baseline_agreed, Some(true));
        assert_eq!((labels[1].agreed, labels[1].rank, labels[1].baseline_agreed), (None, None, None));
        assert_eq!(labels[1].not_compared.as_deref(), Some(zerocode_core::summon_choice::NOT_OFFERED));
        let line = serde_json::to_string(&labels[1]).expect("a line");
        assert!(line.contains(&format!("\"{}\"", zerocode_core::summon_choice::NOT_COMPARED_KEY)), "{line}");
        assert!(!line.contains("agreed"), "a pick the seat never offered carries no mark: {line}");
    });
}

/// A pick made against an older ticket — the page moved on and the person
/// took a row of the newer one before its answer came — labels nothing.
#[test]
fn a_pick_against_another_ticket_labels_nothing() {
    let mock = Mock::serving(200, reply(&[0.1, 0.7, 0.2]));
    machine(JevMode::On.key(), &mock.base_url, |cwd| {
        let (seat, rx) = seat(cwd);
        seat.arm_now();
        let older = seat.ask(page("the caret (stale ticket)")).expect("asked");
        let _answer = rx.recv_timeout(ANSWER_WAIT).expect("an answer");
        assert!(!seat.note_chosen(older + 1, Some(1), true));
        assert!(labels(cwd).is_empty());
        // Disarmed, the settled answer is gone with the page.
        let ticket = seat.ask(page("the caret (disarmed)")).expect("asked");
        let _answer = rx.recv_timeout(ANSWER_WAIT).expect("an answer");
        seat.disarm();
        assert!(!seat.note_chosen(ticket, Some(1), true));
        assert_eq!(seat.ask(page("after disarm")), None, "a disarmed seat asks nothing");
    });
}

/// `auto` rises on the seat's own labels: a window of pages that answered
/// inside the wall and a window of picks that took what the judgment put
/// first clear every line, the judge writes the rise in this ledger, and the
/// next page armed under `auto` takes the judgment's order — where the page
/// before the rise stood.
#[test]
fn auto_rises_on_its_own_labels_and_the_next_page_takes_the_judgments_order() {
    use zerocode_core::jev::promote::{marks_that_can_clear, window_wanted_for, Verdict, ROSE};
    let mock = Mock::serving(200, reply(&[0.1, 0.7, 0.2]));
    machine(JevMode::Auto.key(), &mock.base_url, |cwd| {
        let (seat, rx) = seat(cwd);
        seat.arm_now();
        let before = seat.ask(page("the caret (auto, before)")).expect("asked");
        let answer = rx.recv_timeout(ANSWER_WAIT).expect("an answer");
        assert_eq!((answer.ticket, answer.applies), (before, false), "a recording seat hands an order it may not apply");
        let _ = rows_settled(cwd, 1);
        let ledger = mention_rerank_path(cwd);
        let wanted = window_wanted_for(&MENTION_RERANK).expect("the seat rises");
        let already = rows(cwd).len();
        // A comparison seat's labels count only from the window's oldest
        // reading on, so the labels are clocked after the reading the page
        // above just wrote.
        let now = super::super::decision_shadow::unix_millis();
        for at in 0..(wanted - already) {
            let row = serde_json::json!({
                "at": now + 1_000 + at as u64, "surface": "mention", "query": at, "notes": at, "rubricVersion": MENTION_RUBRIC_VERSION,
                "outcome": MENTION_OUTCOME_ANSWERED, "candidates": 3, "elapsedMs": 300, "retries": 0,
                "requests": 1, "redactedLines": 0, "routeUse": "auto", "applied": false,
            });
            append_shadow_row(&ledger, &row, SHADOW_LEDGER_MAX_BYTES).expect("a reading");
        }
        // Enough picks to bound above the budget with the three the label
        // said no to inside, and the fuzzy page's own first row beside each
        // (t-6342).
        let misses = MENTION_RERANK.negatives_wanted.expect("the seat rises");
        let marks = marks_that_can_clear(&MENTION_RERANK).expect("the seat rises");
        for at in 0..marks {
            // Named by the page's words and the time it was asked, as the
            // seat's label writer names it (t-6877).
            let label = serde_json::json!({
                "kind": LABEL_ROW_KIND, "at": now + 5_000 + at as u64, "surface": "mention", "label": format!("{at}:{at}"),
                "requestAt": now + 1_000 + at as u64,
                "query": at, "notes": at, "applied": false, "agreed": at >= misses, "baselineAgreed": at % 2 == 0, "rank": 0,
            });
            append_shadow_row(&ledger, &label, SHADOW_LEDGER_MAX_BYTES).expect("a label");
        }
        let verdict = judge_ledger(&ledger, i64::try_from(now + 9_000).expect("a clock"));
        assert_eq!(verdict, Some(Verdict::Rise), "the labels did not raise the seat");
        let rose = super::super::jev_summary::read_rows(&ledger)
            .iter()
            .any(|row| zerocode_core::jev::summary::TRANSITION.read(row) == Some(&serde_json::json!(ROSE)));
        assert!(rose, "the rise was not written in the seat's own ledger");
        // Raised: the next page armed under `auto` takes the judgment's order.
        seat.disarm();
        seat.arm_now();
        let after = seat.ask(page("the caret (auto, after)")).expect("asked");
        let answer = rx.recv_timeout(ANSWER_WAIT).expect("an answer");
        assert_eq!((answer.ticket, answer.applies), (after, true), "the raised seat did not act");
    });
}

/* ---- the replay: this machine's prompts, the product's page, the real wire */

const REPLAY_SEED_ENV: &str = "ZEROCODE_MENTION_REPLAY_SEED";
const REPLAY_QUERY_CHARS_ENV: &str = "ZEROCODE_MENTION_REPLAY_QUERY_CHARS";
const REPLAY_LIMIT_ENV: &str = "ZEROCODE_MENTION_REPLAY_LIMIT";
/// The letters a person is taken to have typed after `@` before picking the
/// file: the head of its name. Three is the harness's assumption, not a
/// measurement — the transcripts hold the pick and not the keystrokes.
const REPLAY_QUERY_CHARS_DEFAULT: usize = 3;

/// The prompt at `line` of a transcript in `store`, as the seed named it.
fn prompt_at(store: &str, path: &str, line: usize) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let row: serde_json::Value = serde_json::from_str(text.lines().nth(line)?).ok()?;
    match store {
        "zo" => row["message"]["blocks"]
            .as_array()?
            .iter()
            .find(|block| block["type"] == "text")
            .and_then(|block| block["text"].as_str())
            .map(str::to_string),
        "claude" => match &row["message"]["content"] {
            serde_json::Value::String(text) => Some(text.clone()),
            serde_json::Value::Array(blocks) => blocks
                .iter()
                .find(|block| block["type"] == "text")
                .and_then(|block| block["text"].as_str())
                .map(str::to_string),
            _ => None,
        },
        _ => None,
    }
}

/// The prompt without the file it named — the whole path and its name, as
/// substrings wherever they sit — so what the seat reads is the sentence
/// the person was writing around the mention. Taken out as substrings and
/// not as words (t-6155 F4): a mention with a line range on its tail, an
/// absolute path with the mention inside it, or a name in backticks with a
/// full stop after it is not a whitespace word, and 22 of 136 seed prompts
/// carried the file's whole name into the intent that way.
fn intent_of(prompt: &str, mention: &str) -> String {
    let name = Path::new(mention).file_name().and_then(|name| name.to_str()).unwrap_or(mention);
    let mut intent = prompt.to_string();
    for needle in [format!("@{mention}"), mention.to_string(), format!("@{name}"), name.to_string()] {
        if !needle.is_empty() {
            intent = intent.replace(&needle, " ");
        }
    }
    intent.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The intent a replay hands the seat is the sentence around the mention,
/// with the file the person named taken out — the whole path and its name,
/// wherever they sit in the prose (t-6155 F4). A judgment that reads
/// `fanout.rs` in the intent and `fanout.rs` on the page is not reading
/// intent; it is reading the answer.
#[test]
fn the_intent_carries_neither_the_mentioned_path_nor_its_name() {
    let mention = "crates/zerocode-core/src/orchestration/fanout.rs";
    for prompt in [
        "@crates/zerocode-core/src/orchestration/fanout.rs 의 스폰 순서를 봐 줘",
        "`/Users/dev/zerocode/crates/zerocode-core/src/orchestration/fanout.rs:60-96` 여기 고쳐",
        "fanout.rs에서 파도가 끊기는 곳(crates/zerocode-core/src/orchestration/fanout.rs).",
        "봐 줘: crates/zerocode-core/src/orchestration/fanout.rs, 그리고 fanout.rs 시험도",
    ] {
        let intent = intent_of(prompt, mention);
        assert!(!intent.contains(mention), "{prompt:?} -> {intent:?}");
        assert!(!intent.contains("fanout.rs"), "{prompt:?} -> {intent:?}");
        assert!(!intent.contains("  "), "{prompt:?} -> {intent:?}: one space between words");
    }
    assert_eq!(
        intent_of("@crates/zerocode-core/src/orchestration/fanout.rs 의 스폰 순서를 봐 줘", mention),
        "의 스폰 순서를 봐 줘"
    );
    // A word that merely shares letters with the name stays.
    assert!(intent_of("fanout 전략을 fanout.rs 말고 설명해", mention).contains("fanout 전략을"));
}

/// Replays this machine's own prompts against the product's page and the
/// real endpoint, and prints the table the brief asks for: how often the
/// fuzzy page put the named file first against how often the judgment did,
/// pooled and as Wilson lower bounds, the wall's p50/p95 and the cost.
///
/// Every input a judgment reads is the prompt and the page the product's
/// fuzzy search makes of the working tree now. Rows go nowhere: the door
/// stands in a temporary home, and `judge` writes no ledger.
///
/// ```sh
/// python3 tools/mention-rerank-replay/seed.py --out /tmp/mention-rerank-replay/seed.json
/// TYPESAFE_API_KEY=$(security find-generic-password -s dev.zerocode.key.TYPESAFE_API_KEY -a "$USER" -w) \
/// ZEROCODE_MENTION_REPLAY_SEED=/tmp/mention-rerank-replay/seed.json \
///   cargo test -p tools --release --lib mention_rerank::tests::the_pages_this_machine_would_have_reranked -- --ignored --nocapture
/// ```
// One measurement, read top to bottom: the page, the ask, the comparison
// and the table are one story.
#[allow(clippy::too_many_lines)]
#[test]
#[ignore = "reads this machine's transcripts through tools/mention-rerank-replay/seed.py and asks the real endpoint"]
fn the_pages_this_machine_would_have_reranked() {
    use runtime::file_search::{run, FileSearchOptions, SearchRoot};
    use zerocode_core::jev::summary::{percentile, wilson_lower, WILSON_Z_95};

    let seed_path = std::env::var(REPLAY_SEED_ENV).expect("ZEROCODE_MENTION_REPLAY_SEED names the seed");
    let seed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&seed_path).expect("the seed reads")).expect("the seed parses");
    assert_eq!(
        seed["pageRows"].as_u64(),
        Some(MENTION_CANDIDATE_CAP as u64),
        "the seed was made for another page"
    );
    let query_chars: usize = std::env::var(REPLAY_QUERY_CHARS_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .unwrap_or(REPLAY_QUERY_CHARS_DEFAULT);
    let limit: usize = std::env::var(REPLAY_LIMIT_ENV).ok().and_then(|raw| raw.trim().parse().ok()).unwrap_or(usize::MAX);
    let client = api::SystemOneConfig::from_env().expect("TYPESAFE_API_KEY in the environment").into_client();
    let home = tempfile::tempdir().expect("a config home of the replay's own");
    let door = door(home.path());

    // One prompt asked once: the same sentence naming the same file from two
    // checkouts of one tree is one comparison, not two.
    let mut asked_once: std::collections::BTreeSet<(String, u64)> = std::collections::BTreeSet::new();
    let (mut prompts, mut skipped, mut not_offered, mut compared) = (0usize, 0usize, 0usize, 0usize);
    // What the intent still carries of the file after `intent_of` (t-6155
    // F4): its name must be gone; its stem may remain when the person used
    // it as a word ("fanout 전략"), and is counted so a reader can see it.
    let (mut name_left, mut stem_left) = (0usize, 0usize);
    let (mut fuzzy_hits, mut jev_hits) = (0usize, 0usize);
    let mut per_store: std::collections::BTreeMap<String, (usize, usize, usize)> = std::collections::BTreeMap::new();
    let (mut input_tokens, mut requests) = (0u64, 0u32);
    let mut walls: Vec<u64> = Vec::new();
    let mut outcomes: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut lines: Vec<String> = Vec::new();

    for prompt in seed["prompts"].as_array().expect("prompts").iter().take(limit) {
        let (store, path, line, cwd, mention) = (
            prompt["store"].as_str().unwrap_or(""),
            prompt["path"].as_str().unwrap_or(""),
            usize::try_from(prompt["line"].as_u64().unwrap_or(0)).unwrap_or(0),
            prompt["cwd"].as_str().unwrap_or(""),
            prompt["mention"].as_str().unwrap_or(""),
        );
        let Some(text) = prompt_at(store, path, line) else {
            skipped += 1;
            continue;
        };
        let intent = intent_of(&text, mention);
        if !asked_once.insert((mention.to_string(), task_fingerprint(&intent, ""))) {
            continue;
        }
        prompts += 1;
        let name = Path::new(mention).file_name().and_then(|name| name.to_str()).unwrap_or(mention);
        name_left += usize::from(intent.contains(name));
        let stem = Path::new(name).file_stem().and_then(|stem| stem.to_str()).unwrap_or(name);
        stem_left += usize::from(stem.chars().count() >= 3 && intent.contains(stem));
        let query: String = name.chars().take(query_chars).collect();
        let Ok(results) = run(
            &query,
            vec![SearchRoot::repo(cwd)],
            FileSearchOptions { compute_indices: false, ..FileSearchOptions::default() },
            None,
        ) else {
            skipped += 1;
            continue;
        };
        let names: Vec<String> = results
            .matches
            .iter()
            .take(MENTION_CANDIDATE_CAP)
            .map(|found| found.path.to_string_lossy().into_owned())
            .collect();
        let Some(position) = names.iter().position(|found| found == mention) else {
            not_offered += 1;
            lines.push(format!("{store:>6} q={query:<8} not on the page: {mention}"));
            continue;
        };
        let ask = MentionAsk {
            surface: MentionSurface::Mention,
            intent,
            query: query.clone(),
            candidates: names.iter().map(|found| candidate(found, "")).collect(),
        };
        let (row, order) = api::sync_bridge::run_blocking(judge(&door, Some(&client), &cut_to_caps(ask)));
        *outcomes.entry(row.outcome.clone()).or_default() += 1;
        walls.push(row.elapsed_ms);
        input_tokens += row.input_tokens.unwrap_or(0);
        requests += row.requests;
        let Some(order) = order else {
            lines.push(format!("{store:>6} q={query:<8} {mention}: {}", row.outcome));
            continue;
        };
        compared += 1;
        let fuzzy_hit = position == 0;
        let jev_hit = order.first() == Some(&position);
        fuzzy_hits += usize::from(fuzzy_hit);
        jev_hits += usize::from(jev_hit);
        let tally = per_store.entry(store.to_string()).or_default();
        tally.0 += 1;
        tally.1 += usize::from(fuzzy_hit);
        tally.2 += usize::from(jev_hit);
        lines.push(format!(
            "{store:>6} q={query:<8} fuzzy#{:<2} jev#{:<2} {} {mention}  wall={:>5} ms",
            position,
            order.iter().position(|at| *at == position).unwrap_or(usize::MAX),
            if jev_hit { "JEV" } else if fuzzy_hit { "fuz" } else { "   " },
            row.elapsed_ms
        ));
    }
    let mut sorted = walls.clone();
    sorted.sort_unstable();
    let cost = api::systemone_rate(api::SYSTEMONE_MODEL).map(|rate| rate.input_cost_usd(input_tokens));
    let bound = |hits: usize, n: usize| if n == 0 { 0.0 } else { wilson_lower(hits, n, WILSON_Z_95) };
    println!("--- mention rerank replay: {prompts} distinct prompts ({skipped} skipped), query = first {query_chars} chars of the file name");
    for line in &lines {
        println!("{line}");
    }
    println!("outcomes: {outcomes:?}");
    println!("not on the fuzzy page (not compared): {not_offered}");
    println!("intent residue: file name left in {name_left} of {prompts} prompts (must be 0), stem left in {stem_left}");
    println!(
        "compared {compared}: fuzzy first {fuzzy_hits} ({:.1}%, Wilson lower {:.1}%) · Jev first {jev_hits} ({:.1}%, Wilson lower {:.1}%)",
        share(fuzzy_hits, compared),
        100.0 * bound(fuzzy_hits, compared),
        share(jev_hits, compared),
        100.0 * bound(jev_hits, compared),
    );
    for (store, (n, fuzzy, jev)) in &per_store {
        println!("  {store}: compared {n}, fuzzy first {fuzzy} ({:.1}%), Jev first {jev} ({:.1}%)", share(*fuzzy, *n), share(*jev, *n));
    }
    println!(
        "wall ms: p50 {} p95 {} max {}",
        percentile(&sorted, 0.5).unwrap_or(0),
        percentile(&sorted, 0.95).unwrap_or(0),
        sorted.last().copied().unwrap_or(0)
    );
    println!("requests: {requests}, input tokens: {input_tokens}, cost: {cost:?} USD");
}

#[allow(clippy::cast_precision_loss)]
fn share(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        0.0
    } else {
        100.0 * part as f64 / whole as f64
    }
}
