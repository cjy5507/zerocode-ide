//! What the read's question, its reader and its fold promise.

use serde_json::json;

use super::*;
use crate::jev::{BROWSER_READ, rubric_fingerprint};

fn page() -> Vec<ReadBlock> {
    vec![
        ReadBlock::new("body>header", "Site name\nHome  News  Sport"),
        ReadBlock::new("body>nav[role=navigation]", "Skip to content\nSections"),
        ReadBlock::new(
            "body>main>article",
            "The headline\n\nThe first paragraph of the story, which is what the page is for.",
        ),
        ReadBlock::new("body>main>aside", "Related stories\nMore like this"),
        ReadBlock::new("body>main>*", "Comments (12)"),
        ReadBlock::new("body>footer", "© acme · Privacy · Terms"),
    ]
}

/// The endpoint's answers for `page()`: everything but the article and the
/// comments run is chrome, at the confidences given.
fn answers(chrome: &[(usize, f64)]) -> Value {
    let mut map = Map::new();
    for index in 0..page().len() {
        let (word, sure) = chrome
            .iter()
            .find(|(at, _)| *at == index)
            .map_or((BROWSER_READ_CONTENT, 0.9), |(_, sure)| {
                (BROWSER_READ_CHROME, *sure)
            });
        let other = 1.0 - sure;
        let probabilities = if word == BROWSER_READ_CHROME {
            json!({ BROWSER_READ_CHROME: sure, BROWSER_READ_CONTENT: other })
        } else {
            json!({ BROWSER_READ_CONTENT: sure, BROWSER_READ_CHROME: other })
        };
        map.insert(
            format!("b{index}"),
            json!({ "type": "choice", "choice": word, "probabilities": probabilities, "confidence": sure }),
        );
    }
    Value::Object(map)
}

#[test]
fn the_version_is_pinned_to_the_words() {
    // Bump BROWSER_READ_RUBRIC_VERSION when this changes: a reading taken
    // under other words is not evidence about these ones.
    assert_eq!(
        (
            BROWSER_READ_RUBRIC_VERSION,
            rubric_fingerprint(rubric_words).as_str()
        ),
        (1, "76d0fa650444e3f2")
    );
}

#[test]
fn a_page_of_one_block_is_not_a_question() {
    assert!(ask("Docs", &page()[..1]).is_empty());
    assert!(ask("Docs", &[]).is_empty());
    assert_eq!(ask("Docs", &page()[..2]).len(), 1);
}

#[test]
fn a_question_sends_the_title_each_blocks_path_and_head_and_never_a_body() {
    let blocks = page();
    let asks = ask("The headline — acme news", &blocks);
    assert_eq!(asks.len(), 1, "six blocks fit one shard");
    let ask = &asks[0];
    assert_eq!(ask.asked(), &[0, 1, 2, 3, 4, 5]);
    assert_eq!(ask.state["title"], "The headline — acme news");
    let sent = ask.state["blocks"].as_array().expect("blocks");
    assert_eq!(sent.len(), 6);
    assert_eq!(sent[2]["path"], "body>main>article");
    assert_eq!(sent[2]["head"], blocks[2].head());
    assert!(sent[2].get("text").is_none(), "a block's body never leaves");
    for index in 0..6 {
        let question = &ask.questions[format!("b{index}")];
        assert_eq!(question["type"], "choice");
        assert!(
            question["instructions"]
                .as_str()
                .expect("words")
                .contains(&format!("`blocks[{index}]`")),
            "{question}"
        );
        assert!(question["criteria"][BROWSER_READ_CONTENT].is_string());
        assert!(question["criteria"][BROWSER_READ_CHROME].is_string());
    }
}

#[test]
fn a_long_page_is_asked_in_even_shards_and_its_tail_is_never_asked() {
    let blocks: Vec<ReadBlock> = (0..(BROWSER_READ_BLOCK_CAP + 7))
        .map(|at| ReadBlock::new(&format!("body>section[{at}]"), &format!("block {at}")))
        .collect();
    let asks = ask("Long", &blocks);
    let widths: Vec<usize> = asks.iter().map(|ask| ask.asked().len()).collect();
    assert_eq!(widths, vec![12, 12, 12, 12]);
    let asked: Vec<usize> = asks
        .iter()
        .flat_map(|ask| ask.asked().iter().copied())
        .collect();
    assert_eq!(asked, (0..BROWSER_READ_BLOCK_CAP).collect::<Vec<_>>());
    // A second shard's instructions name its OWN state's places.
    let second = &asks[1];
    assert!(
        second.questions["b12"]["instructions"]
            .as_str()
            .expect("words")
            .contains("`blocks[0]`")
    );
    assert_eq!(second.state["blocks"][0]["path"], "body>section[12]");
}

#[test]
fn an_answer_is_read_only_through_the_questions_that_were_asked() {
    let blocks = page();
    let ask = ask("Docs", &blocks).remove(0);
    let read = ask
        .read(&answers(&[(0, 0.9), (1, 0.8), (3, 0.6), (5, 0.95)]))
        .expect("a well formed body");
    let verdict = Verdict::of(read);
    assert_eq!(verdict.answered, 6);
    assert_eq!(
        verdict.chrome.keys().copied().collect::<Vec<_>>(),
        vec![0, 1, 3, 5]
    );
    // One block unanswered refuses the whole shard.
    let mut short = answers(&[]);
    short.as_object_mut().expect("map").remove("b4");
    assert_eq!(ask.read(&short), Err(ChoiceRefusal::NoAnswer));
    // An option nobody offered refuses it too.
    let mut wrong = answers(&[]);
    wrong["b0"]["choice"] = json!("advert");
    assert_eq!(ask.read(&wrong), Err(ChoiceRefusal::UnknownOption));
}

#[test]
fn only_a_chrome_answer_over_the_seats_own_line_is_dropped() {
    let blocks = page();
    let ask = ask("Docs", &blocks).remove(0);
    let verdict = Verdict::of(
        ask.read(&answers(&[(0, 0.9), (1, 0.8), (3, 0.6), (5, 0.95)]))
            .expect("read"),
    );
    // 0.6 sits under BROWSER_READ_FOLD_FLOOR_PERMILLE (700): the aside stays.
    assert_eq!(verdict.droppable(&BROWSER_READ), vec![0, 1, 5]);
    let folded = fold(
        &blocks,
        &verdict.droppable(&BROWSER_READ),
        "`zerocode-browser read browser-1 --full`",
    );
    assert_eq!(folded.dropped, vec![0, 1, 5]);
    assert_eq!(
        folded.text,
        format!(
            "{}\n\n{}\n\n{}\n\n[3개 블록 생략: header·nav·footer — 전체는 `zerocode-browser read browser-1 --full`]",
            blocks[2].text, blocks[3].text, blocks[4].text
        )
    );
}

#[test]
fn nothing_dropped_is_nothing_folded() {
    let blocks = page();
    let folded = fold(&blocks, &[], "x");
    assert!(folded.dropped.is_empty());
    assert!(!folded.text.contains("생략"));
    assert_eq!(
        folded.text,
        blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
    );
    // An index past the page and a repeat are not blocks.
    let folded = fold(&blocks, &[9, 5, 5], "x");
    assert_eq!(folded.dropped, vec![5]);
}

#[test]
fn a_blocks_kind_is_its_last_segments_tag_word() {
    assert_eq!(kind_of("body>main>aside[role=complementary]"), "aside");
    assert_eq!(kind_of("body>div#cookie-banner.banner[2]"), "div");
    assert_eq!(kind_of("body>main>*"), "*");
    assert_eq!(kind_of("body"), "body");
    let blocks = vec![
        ReadBlock::new("body>nav", ""),
        ReadBlock::new("body>nav[2]", ""),
        ReadBlock::new("body>main>*", ""),
        ReadBlock::new("body>footer", ""),
    ];
    assert_eq!(
        kinds(&blocks, &[0, 1, 2, 3, 7]),
        vec!["nav".to_string(), "block".to_string(), "footer".to_string()]
    );
}

#[test]
fn a_press_is_inside_a_block_by_the_one_containment_rule() {
    // A whole structural block: the press's chain IS the path.
    assert!(inside("body>nav", "body>nav"));
    // The run under a landmark: the press's chain is the landmark's.
    assert!(inside("body>main>*", "body>main"));
    // A press under a deeper landmark is that landmark's block, not the
    // run's and not the outer element's.
    assert!(!inside("body>main>*", "body>main>aside"));
    assert!(!inside("body>main", "body>main>aside"));
    // A sibling of the same tag is told apart by its index.
    assert!(!inside("body>section", "body>section[2]"));
}

#[test]
fn the_head_and_path_are_cut_at_the_tables_caps() {
    let block = ReadBlock::new(&"p".repeat(500), &"t".repeat(900));
    assert_eq!(block.path.chars().count(), BROWSER_READ_PATH_CHAR_CAP);
    assert_eq!(block.head().chars().count(), BROWSER_READ_HEAD_CHAR_CAP);
    assert_eq!(
        block.text.chars().count(),
        900,
        "the body is kept whole for the fold"
    );
    let asks = ask(&"T".repeat(400), &[block.clone(), block]);
    assert_eq!(
        asks[0].state["title"]
            .as_str()
            .expect("title")
            .chars()
            .count(),
        BROWSER_READ_TITLE_CHAR_CAP
    );
}
