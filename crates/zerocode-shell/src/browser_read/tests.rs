//! What a judged read promises the agent and the ledger. Every case here
//! crosses a loopback socket or none at all — the fake System One is the
//! one every window seat's tests stand up (`systemone::tests::Endpoint`).

use std::time::Instant;

use serde_json::{Map, json};
use zerocode_core::browser_read::ReadBlock;
use zerocode_core::jev::door::{REDACTED_LINES_KEY, REQUESTS_KEY, Refused};
use zerocode_core::jev::{BROWSER_READ_CHROME, BROWSER_READ_CONTENT, SMART_SETTINGS_KEY};

use super::*;
use crate::cmd::browser::BrowserReadReport;
use crate::systemone::tests::{ANSWERING_VERSION, Endpoint};
use crate::systemone::{SYSTEMONE_MODEL, TIMEOUT};

const PANE: &str = "browser-7";
const URL: &str = "https://docs.example.com/guide/start";

/// The page every case reads: a masthead, a nav, the article, a related
/// rail, the comments run and a footer.
fn page() -> BrowserReadPage {
    let blocks = vec![
        ReadBlock::new("body>header", "acme docs\nGuides  Reference  Blog"),
        ReadBlock::new("body>nav[role=navigation]", "Skip to content\nOn this page"),
        ReadBlock::new(
            "body>main>article",
            "Getting started\n\nInstall the tool, then run it once in an empty folder. The first run writes a settings file beside your project and prints the three commands you will use most. Nothing is sent anywhere until you say so.\n\nThe second run reads that settings file back and checks the tool against your project's own layout.",
        ),
        ReadBlock::new(
            "body>main>aside",
            "Related guides\nConfiguration\nUpgrading",
        ),
        ReadBlock::new("body>main>*", "Was this page helpful?"),
        ReadBlock::new("body>footer", "© acme · Privacy · Terms"),
    ];
    let text = blocks
        .iter()
        .map(|block| block.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    BrowserReadPage {
        report: BrowserReadReport {
            title: "Getting started — acme docs".to_string(),
            url: URL.to_string(),
            text,
            dom: None,
        },
        blocks,
    }
}

/// The endpoint's body: `chrome` for the blocks named, at their confidence,
/// `content` for the rest — answers for every block a page of `count`
/// blocks could ask about, so a sharded read finds each of its ids.
fn body(count: usize, chrome: &[(usize, f64)]) -> String {
    let mut answers = Map::new();
    for index in 0..count {
        let (word, sure) = chrome
            .iter()
            .find(|(at, _)| *at == index)
            .map_or((BROWSER_READ_CONTENT, 0.9), |(_, sure)| {
                (BROWSER_READ_CHROME, *sure)
            });
        let probabilities = if word == BROWSER_READ_CHROME {
            json!({ BROWSER_READ_CHROME: sure, BROWSER_READ_CONTENT: 1.0 - sure })
        } else {
            json!({ BROWSER_READ_CONTENT: sure, BROWSER_READ_CHROME: 1.0 - sure })
        };
        answers.insert(
            format!("b{index}"),
            json!({ "type": "choice", "choice": word, "probabilities": probabilities, "confidence": sure }),
        );
    }
    json!({
        "model": ANSWERING_VERSION,
        "answers": answers,
        "usage": { "input_tokens": 700, "output_tokens": 0 },
    })
    .to_string()
}

/// zo's settings in a folder of the case's own, consenting to `work` under
/// it. Answers the settings file and the consented workspace.
fn consented(home: &tempfile::TempDir) -> (PathBuf, PathBuf) {
    let work = home.path().join("work");
    std::fs::create_dir_all(&work).expect("a workspace");
    let settings = home.path().join("settings.json");
    std::fs::write(
        &settings,
        json!({ SMART_SETTINGS_KEY: { "jev": { "workspaces": [work.display().to_string()] } } })
            .to_string(),
    )
    .expect("zo's settings");
    (settings, work)
}

fn wire_at(endpoint: &Endpoint, settings: PathBuf) -> Wire {
    Wire::at(&endpoint.base(), "test-key", Some(settings))
}

#[test]
fn off_asks_nothing_and_hands_back_the_page_whole() {
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", body(6, &[(0, 0.9)]), 0);
    let home = tempfile::tempdir().expect("a zo home");
    let (settings, work) = consented(&home);
    let page = page();
    let judged = settle(
        &wire_at(&endpoint, settings),
        Some(&work),
        PANE,
        &page,
        JevMode::Off,
        false,
        1_790_050_000_000,
    );
    assert_eq!(
        judged.text, page.report.text,
        "byte for byte the plain read"
    );
    assert_eq!(judged.row, None, "nothing asked, nothing recorded");
    assert_eq!(judged.judgment, None);
    assert!(endpoint.asked().is_empty(), "no socket opened");
}

#[test]
fn shadow_records_the_judgment_and_hands_back_the_page_whole() {
    let endpoint = Endpoint::serving(
        "HTTP/1.1 200 OK",
        body(6, &[(0, 0.9), (1, 0.85), (3, 0.6), (5, 0.95)]),
        0,
    );
    let home = tempfile::tempdir().expect("a zo home");
    let (settings, work) = consented(&home);
    let page = page();
    let judged = settle(
        &wire_at(&endpoint, settings),
        Some(&work),
        PANE,
        &page,
        JevMode::Shadow,
        false,
        1_790_050_000_000,
    );
    assert_eq!(judged.text, page.report.text, "shadow reads the whole page");
    let row = judged.row.expect("a row");
    assert_eq!(row["outcome"], ANSWERED);
    assert_eq!(row[ROUTE_USE.canonical], JevMode::Shadow.key());
    assert_eq!(row[APPLIED.canonical], false);
    assert_eq!(row[REASON], SEAT_RECORDING);
    assert_eq!(row["blocks"], 6);
    assert_eq!(row["asked"], 6);
    assert_eq!(row["shards"], 1);
    assert_eq!(row["shardsAnswered"], 1);
    assert_eq!(row["chrome"], 4, "four blocks were called chrome");
    assert_eq!(
        row["droppable"], 3,
        "the aside at 0.6 sits under the fold line"
    );
    assert_eq!(row["folded"], 0);
    assert_eq!(row["charsBefore"], row["charsAfter"]);
    assert_eq!(row["host"], "docs.example.com");
    // The path never reaches a row (t-6155 F8): its fingerprint does, from
    // the table's one producer, so two reads of one page still group.
    assert_eq!(
        row["pathFingerprint"],
        zerocode_core::jev::fingerprint_of("/guide/start")
    );
    assert!(row.get("path").is_none());
    assert!(
        !row.to_string().contains("/guide"),
        "no piece of the address past the host is in the row:\n{row}"
    );
    assert_eq!(row["pane"], PANE);
    assert_eq!(row[READ_KEY], format!("{PANE}@1790050000000"));
    assert_eq!(row[REQUESTS_KEY], 1);
    assert_eq!(
        row[zerocode_core::jev::summary::MODEL.canonical],
        ANSWERING_VERSION,
        "the version that answered"
    );
    assert_eq!(row[INPUT_TOKENS.canonical], 700);
    assert!(row[ELAPSED_MS.canonical].is_u64());
    assert!(row[AT.canonical].is_i64());
    let judgment = judged.judgment.expect("a judgment a press may label");
    assert_eq!(
        judgment.chrome,
        vec![
            "body>header",
            "body>nav[role=navigation]",
            "body>main>aside",
            "body>footer"
        ]
    );
    assert!(!judgment.applied);
    // What left the door: the title, each block's path and head, never a
    // body — and no block's text.
    let asked = endpoint.asked();
    assert_eq!(asked.len(), 1);
    let sent = &asked[0];
    assert!(sent.contains("Getting started — acme docs"));
    assert!(sent.contains("body>main>article"));
    assert!(
        sent.contains("Install the tool, then run it once in an empty folder."),
        "the head of a block is what leaves"
    );
    assert!(
        !sent.contains("checks the tool against your project's own layout"),
        "a block's body past its head never leaves"
    );
    assert!(sent.contains("\"head\":\"Getting started"));
}

#[test]
fn on_folds_the_sure_chrome_blocks_into_one_line() {
    let endpoint = Endpoint::serving(
        "HTTP/1.1 200 OK",
        body(6, &[(0, 0.9), (1, 0.85), (3, 0.6), (5, 0.95)]),
        0,
    );
    let home = tempfile::tempdir().expect("a zo home");
    let (settings, work) = consented(&home);
    let page = page();
    let judged = settle(
        &wire_at(&endpoint, settings),
        Some(&work),
        PANE,
        &page,
        JevMode::On,
        true,
        1_790_050_000_000,
    );
    let expected = format!(
        "{}\n\n{}\n\n{}\n\n[3개 블록 생략: header·nav·footer — 전체는 `zerocode-browser read {PANE} --full`]",
        page.blocks[2].text, page.blocks[3].text, page.blocks[4].text
    );
    assert_eq!(judged.text, expected);
    let row = judged.row.expect("a row");
    assert_eq!(row[ROUTE_USE.canonical], ROUTE_USE_APPLIED);
    assert_eq!(row[APPLIED.canonical], true);
    assert_eq!(row["folded"], 3);
    assert!(row.get(REASON).is_none());
    assert!(
        row["charsAfter"].as_u64() < row["charsBefore"].as_u64(),
        "{row}"
    );
    let judgment = judged.judgment.expect("a judgment");
    assert!(judgment.applied);
    assert_eq!(
        judgment.chrome.len(),
        4,
        "the judgment, not the fold, is what a press compares"
    );
}

#[test]
fn a_judgment_that_drops_nothing_folds_nothing_and_leaves_no_label_to_write() {
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", body(6, &[]), 0);
    let home = tempfile::tempdir().expect("a zo home");
    let (settings, work) = consented(&home);
    let page = page();
    let judged = settle(
        &wire_at(&endpoint, settings),
        Some(&work),
        PANE,
        &page,
        JevMode::On,
        true,
        1_790_050_000_000,
    );
    assert_eq!(
        judged.text, page.report.text,
        "no fold line on a page with no furniture"
    );
    let row = judged.row.expect("a row");
    assert_eq!(row["chrome"], 0);
    assert_eq!(row["folded"], 0);
    assert_eq!(row[ROUTE_USE.canonical], ROUTE_USE_APPLIED);
    assert_eq!(judged.judgment, None, "nothing a press could regret");
}

#[test]
fn a_wall_hands_back_the_page_whole_and_says_so() {
    let endpoint = Endpoint::serving(
        "HTTP/1.1 200 OK",
        body(6, &[(0, 0.9)]),
        BROWSER_READ_APPLY_DEADLINE_MS + 700,
    );
    let home = tempfile::tempdir().expect("a zo home");
    let (settings, work) = consented(&home);
    let page = page();
    let began = Instant::now();
    let judged = settle(
        &wire_at(&endpoint, settings),
        Some(&work),
        PANE,
        &page,
        JevMode::On,
        true,
        1_790_050_000_000,
    );
    let elapsed = began.elapsed();
    assert!(
        elapsed < READ_DEADLINE + Duration::from_millis(500),
        "the wall bounds the whole read: {elapsed:?}"
    );
    assert_eq!(judged.text, page.report.text);
    let row = judged.row.expect("a row");
    assert_eq!(row["outcome"], TIMEOUT);
    assert_eq!(row[ROUTE_USE.canonical], ROUTE_USE_FALLBACK);
    assert_eq!(row[APPLIED.canonical], false);
    assert_eq!(row["shardsAnswered"], 0);
    assert_eq!(judged.judgment, None);
}

#[test]
fn a_shard_that_breaks_the_closed_choice_hands_back_the_page_whole() {
    let mut broken: serde_json::Value = serde_json::from_str(&body(6, &[(0, 0.9)])).expect("json");
    broken["answers"]["b3"]["choice"] = json!("advert");
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", broken.to_string(), 0);
    let home = tempfile::tempdir().expect("a zo home");
    let (settings, work) = consented(&home);
    let page = page();
    let judged = settle(
        &wire_at(&endpoint, settings),
        Some(&work),
        PANE,
        &page,
        JevMode::On,
        true,
        1_790_050_000_000,
    );
    assert_eq!(judged.text, page.report.text);
    let row = judged.row.expect("a row");
    assert_eq!(row["outcome"], "schema_unknown_option");
    assert_eq!(row[ROUTE_USE.canonical], ROUTE_USE_FALLBACK);
}

#[test]
fn a_workspace_nobody_consented_to_is_refused_before_a_socket() {
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", body(6, &[(0, 0.9)]), 0);
    let home = tempfile::tempdir().expect("a zo home");
    let (settings, _work) = consented(&home);
    let elsewhere = home.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).expect("a folder beside");
    let page = page();
    let judged = settle(
        &wire_at(&endpoint, settings),
        Some(&elsewhere),
        PANE,
        &page,
        JevMode::On,
        true,
        1_790_050_000_000,
    );
    assert_eq!(judged.text, page.report.text);
    let row = judged.row.expect("a row");
    assert_eq!(row["outcome"], Refused::NotConsented.token());
    assert_eq!(row[REQUESTS_KEY], 0);
    assert!(endpoint.asked().is_empty(), "nothing left the door");
}

/// A page of many blocks is asked in shards that leave together: two shards
/// held 300 ms each come back in well under 600 ms, and one verdict is read
/// over both.
#[test]
fn shards_leave_side_by_side_and_are_read_into_one_verdict() {
    let blocks: Vec<ReadBlock> = (0..20)
        .map(|at| {
            ReadBlock::new(
                &format!("body>section[{}]", at + 1),
                &format!("block {at} words"),
            )
        })
        .collect();
    let text = blocks
        .iter()
        .map(|block| block.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    let page = BrowserReadPage {
        report: BrowserReadReport {
            title: "Long".to_string(),
            url: URL.to_string(),
            text,
            dom: None,
        },
        blocks,
    };
    let endpoint = Endpoint::serving_each("HTTP/1.1 200 OK", body(20, &[(1, 0.9), (17, 0.9)]), 300);
    let home = tempfile::tempdir().expect("a zo home");
    let (settings, work) = consented(&home);
    let began = Instant::now();
    let judged = settle(
        &wire_at(&endpoint, settings),
        Some(&work),
        PANE,
        &page,
        JevMode::On,
        true,
        1_790_050_000_000,
    );
    let elapsed = began.elapsed();
    assert!(
        elapsed < Duration::from_millis(550),
        "shards were asked one after another: {elapsed:?}"
    );
    let row = judged.row.expect("a row");
    assert_eq!(row["shards"], 2);
    assert_eq!(row["shardsAnswered"], 2);
    assert_eq!(row[REQUESTS_KEY], 2);
    assert_eq!(row["folded"], 2);
    assert_eq!(endpoint.asked().len(), 2);
    assert!(!judged.text.contains("block 1 words"));
    assert!(!judged.text.contains("block 17 words"));
    assert!(judged.text.contains("[2개 블록 생략: section — 전체는"));
}

#[test]
fn a_press_labels_the_read_it_followed_once_and_only_on_the_same_page() {
    let label = "browser-label-case";
    forget(label);
    let judgment = ReadJudgment {
        read: format!("{label}@1"),
        url: URL.to_string(),
        chrome: vec!["body>nav".to_string(), "body>main>aside".to_string()],
        applied: false,
        mode: JevMode::Shadow,
    };
    // A press inside a chrome block: the fold would have been regretted.
    remember(label, judgment.clone());
    let row = note_press(label, "click", Some(URL), Some("body>nav"), 5).expect("a label");
    assert_eq!(row[LABEL.canonical], format!("{label}@1"));
    assert_eq!(row[AGREED.canonical], false);
    assert_eq!(row["verb"], "click");
    assert_eq!(row[APPLIED.canonical], false);
    assert_eq!(row["blockPath"], "body>nav");
    // One label per read.
    assert_eq!(
        note_press(label, "click", Some(URL), Some("body>nav"), 6),
        None
    );
    // A press in the article, or in the run under main, agreed — the run is
    // not the aside.
    remember(label, judgment.clone());
    let row = note_press(
        label,
        "type",
        Some(&format!("{URL}#install")),
        Some("body>main>article"),
        7,
    )
    .expect("a label");
    assert_eq!(row[AGREED.canonical], true);
    remember(label, judgment.clone());
    let row = note_press(label, "type", Some(URL), Some("body>main"), 8).expect("a label");
    assert_eq!(
        row[AGREED.canonical], true,
        "the run under main is not the aside"
    );
    // A press on another page labels nothing and spends the judgment.
    remember(label, judgment.clone());
    assert_eq!(
        note_press(
            label,
            "click",
            Some("https://docs.example.com/other"),
            Some("body>nav"),
            9
        ),
        None
    );
    assert_eq!(
        note_press(label, "click", Some(URL), Some("body>nav"), 10),
        None
    );
    // A press whose block the page could not name labels nothing.
    remember(label, judgment);
    assert_eq!(note_press(label, "click", Some(URL), None, 11), None);
    forget(label);
}

/// A `read --full` after a fold is the fold's first regret (t-6155 F6): the
/// agent asked for the whole page back. One label per read, in the press's
/// book; nothing under a seat that folded nothing, since the agent already
/// had the page whole.
#[test]
fn a_whole_read_after_a_fold_labels_it_regretted_once_and_only_when_something_was_folded() {
    let label = "browser-full-case";
    forget(label);
    let folded = ReadJudgment {
        read: format!("{label}@1"),
        url: URL.to_string(),
        chrome: vec!["body>nav".to_string()],
        applied: true,
        mode: JevMode::Auto,
    };
    remember(label, folded.clone());
    let row = note_full_read(label, URL, 5).expect("a label");
    assert_eq!(row[LABEL.canonical], format!("{label}@1"));
    assert_eq!(row[AGREED.canonical], false);
    assert_eq!(row["verb"], READ_FULL_VERB);
    assert_eq!(row[APPLIED.canonical], true);
    assert_eq!(row["pane"], label);
    assert_eq!(row["mode"], JevMode::Auto.key());
    // One label per read: neither a press nor a second whole read finds it.
    assert_eq!(
        note_press(label, "click", Some(URL), Some("body>main"), 6),
        None
    );
    assert_eq!(note_full_read(label, URL, 7), None);
    // A fragment is the same page.
    remember(label, folded.clone());
    assert!(note_full_read(label, &format!("{URL}#install"), 8).is_some());
    // Another page labels nothing and spends the judgment, as a press does.
    remember(label, folded.clone());
    assert_eq!(
        note_full_read(label, "https://docs.example.com/other", 9),
        None
    );
    assert_eq!(
        note_press(label, "click", Some(URL), Some("body>nav"), 10),
        None
    );
    // Under a seat that folded nothing the whole page was already in hand:
    // no label, nothing spent, and the press still labels the judgment.
    let recorded = ReadJudgment {
        applied: false,
        mode: JevMode::Shadow,
        ..folded
    };
    remember(label, recorded);
    assert_eq!(note_full_read(label, URL, 11), None);
    let row = note_press(label, "click", Some(URL), Some("body>nav"), 12)
        .expect("the press still labels");
    assert_eq!(row[AGREED.canonical], false);
    forget(label);
}

#[test]
fn a_read_of_one_block_asks_nothing() {
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", body(6, &[(0, 0.9)]), 0);
    let home = tempfile::tempdir().expect("a zo home");
    let (settings, work) = consented(&home);
    let mut page = page();
    page.blocks.truncate(1);
    let judged = settle(
        &wire_at(&endpoint, settings),
        Some(&work),
        PANE,
        &page,
        JevMode::On,
        true,
        1_790_050_000_000,
    );
    assert_eq!(judged.text, page.report.text);
    assert_eq!(judged.row, None);
    assert!(endpoint.asked().is_empty());
}

/// The rows a read writes are judged by the same judge every window seat's
/// rows are, and a label row joins the agreement by the read's own name.
#[test]
fn the_rows_land_in_the_seats_own_ledger_and_are_read_back_as_one_agreement() {
    let home = tempfile::tempdir().expect("a zo home");
    let (settings, _work) = consented(&home);
    let wire = Wire::at("http://127.0.0.1:9", "test-key", Some(settings));
    let ledger = crate::systemone::ledger_of(&wire, &BROWSER_READ).expect("a ledger");
    let read = json!({
        AT.canonical: 1, READ_KEY: "p@1", "outcome": ANSWERED, ROUTE_USE.canonical: JevMode::Shadow.key(),
        REQUESTS_KEY: 1, REDACTED_LINES_KEY: 0, ELAPSED_MS.canonical: 300,
    });
    let label = json!({ AT.canonical: 2, LABEL.canonical: "p@1", AGREED.canonical: false });
    record(&wire, &[read, label], 2);
    let rows = crate::systemone::read_rows(&ledger);
    assert_eq!(rows.len(), 2);
    assert!(ledger.ends_with(BROWSER_READ.ledger));
    let agreement = zerocode_core::jev::summary::agreement_since(&rows, 0);
    assert_eq!((agreement.compared, agreement.agreed), (1, 0));
    let tally = zerocode_core::jev::summary::summarize(&rows, 0);
    assert_eq!(
        (tally.rows, tally.answered),
        (1, 1),
        "a label row is not a request"
    );
}

/// The measurement (t-6041): the pages `tools/browser-read-replay/gather.mjs`
/// cut with the window's own script, asked of the real endpoint through the
/// seat's own question builder, folded under `on`. Prints, per page, the
/// characters before and after, the blocks, how many were called chrome and
/// how many folded, the wall time, the tokens billed and their cost — and
/// every folded block's path and head, which is what a reader judges the
/// golden false positives on.
///
/// Ignored because it crosses the network and spends the key: the key comes
/// from `TYPESAFE_API_KEY`, the seed from `ZEROCODE_BROWSER_READ_SEED`, and
/// the questions leave through a zo home of this test's own so the person's
/// ledger and day count are untouched.
#[test]
#[ignore = "crosses the network and spends the key — run by hand"]
fn the_pages_that_were_gathered() {
    let seed_path =
        std::env::var("ZEROCODE_BROWSER_READ_SEED").expect("ZEROCODE_BROWSER_READ_SEED");
    let key = std::env::var("TYPESAFE_API_KEY").expect("TYPESAFE_API_KEY");
    let seed: Value = serde_json::from_str(&std::fs::read_to_string(&seed_path).expect("the seed"))
        .expect("seed json");
    let home = tempfile::tempdir().expect("a zo home");
    let (settings, work) = consented(&home);
    let wire = Wire::at(crate::systemone::SYSTEMONE_BASE_URL, &key, Some(settings));
    wire.warm();
    std::thread::sleep(Duration::from_millis(800));
    let rate = model_prices::systemone_rate(SYSTEMONE_MODEL);
    let mut elapsed_all: Vec<u64> = Vec::new();
    let (mut before_all, mut after_all, mut tokens_all, mut folded_all, mut blocks_all) =
        (0_u64, 0_u64, 0_u64, 0_u64, 0_u64);
    // Per page (t-6155 F6): each page's own saved share, for a median one
    // page cannot own, and the totals with the bot-wall pages left out —
    // a wall's own words are the page, and folding them says nothing about
    // folding a page.
    let mut saved_shares: Vec<f64> = Vec::new();
    let (mut before_clear, mut after_clear, mut walls) = (0_u64, 0_u64, 0_usize);
    println!(
        "| page | chars before | chars after | saved | blocks | asked | chrome | folded | wall ms | tokens | outcome | wall? |"
    );
    println!("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---|");
    let mut folded_paths: Vec<String> = Vec::new();
    for entry in seed["pages"].as_array().expect("pages") {
        if entry.get("error").is_some_and(|error| !error.is_null()) {
            println!(
                "| {} | — | — | — | — | — | — | — | — | — | not gathered |",
                entry["url"]
            );
            continue;
        }
        let blocks: Vec<ReadBlock> = entry["blocks"]
            .as_array()
            .expect("blocks")
            .iter()
            .map(|block| {
                ReadBlock::new(
                    block["path"].as_str().unwrap_or_default(),
                    block["text"].as_str().unwrap_or_default(),
                )
            })
            .collect();
        let page = BrowserReadPage {
            report: BrowserReadReport {
                title: entry["title"].as_str().unwrap_or_default().to_string(),
                url: entry["url"].as_str().unwrap_or_default().to_string(),
                text: entry["text"].as_str().unwrap_or_default().to_string(),
                dom: None,
            },
            blocks,
        };
        let judged = settle(
            &wire,
            Some(&work),
            "browser-measure",
            &page,
            JevMode::On,
            true,
            1,
        );
        let row = judged.row.clone().unwrap_or(json!({}));
        let before = page.report.text.chars().count() as u64;
        let after = judged.text.chars().count() as u64;
        let elapsed = row[ELAPSED_MS.canonical].as_u64().unwrap_or(0);
        let tokens = row[INPUT_TOKENS.canonical].as_u64().unwrap_or(0);
        elapsed_all.push(elapsed);
        before_all += before;
        after_all += after;
        tokens_all += tokens;
        folded_all += row["folded"].as_u64().unwrap_or(0);
        blocks_all += row["blocks"].as_u64().unwrap_or(0);
        let saved = if before == 0 {
            0.0
        } else {
            100.0 * (before - after) as f64 / before as f64
        };
        let wall = entry["wall"].as_bool().unwrap_or(false);
        saved_shares.push(saved);
        if wall {
            walls += 1;
        } else {
            before_clear += before;
            after_clear += after;
        }
        println!(
            "| {} | {before} | {after} | {saved:.0}% | {} | {} | {} | {} | {elapsed} | {tokens} | {} | {} |",
            page.report.url,
            row["blocks"],
            row["asked"],
            row["chrome"],
            row["folded"],
            row["outcome"],
            if wall { "bot wall" } else { "" }
        );
        // The folded blocks, for the golden reading.
        if let Some(judgment) = &judged.judgment {
            for path in &judgment.chrome {
                let head = page
                    .blocks
                    .iter()
                    .find(|block| &block.path == path)
                    .map(|block| block.head().replace('\n', " ⏎ "))
                    .unwrap_or_default();
                let dropped = !judged.text.contains(
                    &page
                        .blocks
                        .iter()
                        .find(|block| &block.path == path)
                        .map(|block| block.text.clone())
                        .unwrap_or_default(),
                );
                folded_paths.push(format!(
                    "    {} {} — {path} — {}",
                    page.report.url,
                    if dropped { "FOLDED " } else { "chrome<line" },
                    head.chars().take(90).collect::<String>()
                ));
            }
        }
    }
    elapsed_all.sort_unstable();
    let p50 = zerocode_core::jev::summary::percentile(&elapsed_all, 0.50).unwrap_or(0);
    let p95 = zerocode_core::jev::summary::percentile(&elapsed_all, 0.95).unwrap_or(0);
    let cost = rate.map(|rate| rate.input_cost_usd(tokens_all));
    println!();
    println!(
        "TOTAL chars {before_all} → {after_all} ({:.1}% saved) · blocks {blocks_all} · folded {folded_all} · wall p50 {p50} ms p95 {p95} ms · tokens {tokens_all} · cost {}",
        if before_all == 0 {
            0.0
        } else {
            100.0 * (before_all - after_all) as f64 / before_all as f64
        },
        cost.map_or("unpriced".to_string(), |usd| format!("${usd:.4}"))
    );
    saved_shares.sort_by(f64::total_cmp);
    let median = if saved_shares.is_empty() {
        0.0
    } else {
        let mid = saved_shares.len() / 2;
        if saved_shares.len().is_multiple_of(2) {
            f64::midpoint(saved_shares[mid - 1], saved_shares[mid])
        } else {
            saved_shares[mid]
        }
    };
    println!(
        "per-page median saved {median:.1}% · without {walls} bot-wall page(s): chars {before_clear} → {after_clear} ({:.1}% saved)",
        if before_clear == 0 {
            0.0
        } else {
            100.0 * (before_clear - after_clear) as f64 / before_clear as f64
        }
    );
    println!(
        "chrome blocks (FOLDED = dropped; chrome<line = called chrome under the fold line, kept):"
    );
    for line in folded_paths {
        println!("{line}");
    }
}
