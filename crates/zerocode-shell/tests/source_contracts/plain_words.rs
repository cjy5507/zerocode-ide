//! 화면 글에 「벽」이 없다 (t-81155).
//!
//! 「한도 벽」·「벽에 선 판」·「walled panes」는 이 코드베이스의 비유어다. 사람이
//! 읽는 말은 「한도에 닿은 판」이다 — t-80303이 고른 계정 문구 둘을 바꿨고, 이
//! 계약은 나머지가 돌아오지 않게 창의 모든 글에서 그 말을 막는다: 한국어 원문
//! (호출 자리의 기본값, `word:` 표, `index.html`의 글), 영·일·중·스페인어 사전,
//! 그리고 창에 글을 건네는 백엔드의 문장.
//!
//! 키 이름(`usage.nextWalled`, `quotaWall.*`)과 코드의 값(`"walled"`)은 화면에
//! 서지 않으므로 막지 않는다: 사전은 값만, 원문은 한국어 글자만 읽는다.

use super::support::{WINDOW_PARTS, shipped_backend, strip_comments, strip_rust_comments};

/// 「벽」 — 「완벽」의 벽은 다른 낱말이라 뺀다.
fn korean_wall(text: &str) -> bool {
    let mut before = None;
    for glyph in text.chars() {
        if glyph == '벽' && before != Some('완') {
            return true;
        }
        before = Some(glyph);
    }
    false
}

/// 사전 값 하나가 「벽」 계열을 쓰는가 — 다섯 언어의 그 말. `{{wall}}` 같은
/// 자리표시는 화면에 그대로 서지 않으므로 지우고 읽는다.
fn wall_word(value: &str) -> bool {
    let mut shown = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(open) = rest.find("{{") {
        shown.push_str(&rest[..open]);
        rest = rest[open..]
            .find("}}")
            .map_or("", |close| &rest[open + close + 2..]);
    }
    shown.push_str(rest);
    let lower = shown.to_lowercase();
    let latin = lower
        .split(|glyph: char| !glyph.is_alphabetic())
        .any(|word| {
            matches!(
                word,
                "wall" | "walls" | "walled" | "muro" | "muros" | "pared" | "paredes"
            )
        });
    latin || korean_wall(&shown) || shown.contains(['壁', '墙', '牆'])
}

/// 사전 파일의 `"키": "값"` 줄에서 값만.
fn catalog_values(catalogs: &str) -> Vec<(&str, &str)> {
    catalogs
        .lines()
        .filter_map(|line| {
            let (key, value) = line.trim().strip_prefix('"')?.split_once("\":")?;
            let value = value.trim_start().strip_prefix('"')?;
            Some((key, value.trim_end_matches(',').trim_end_matches('"')))
        })
        .collect()
}

#[test]
fn no_catalog_says_wall_in_any_of_the_five_languages() {
    let catalogs = strip_comments(include_str!("../../../../ui/shell-i18n.js"));
    let values = catalog_values(&catalogs);
    // The reader has to be reading the catalogs at all — four languages of
    // thousands of keys — or every line below passes on nothing.
    assert!(
        values.len() > 10_000,
        "the catalog reader found {} values",
        values.len()
    );
    let walled: Vec<String> = values
        .iter()
        .filter(|(_, value)| wall_word(value))
        .map(|(key, value)| format!("{key}: {value}"))
        .collect();
    assert!(
        walled.is_empty(),
        "these catalog words still say 「벽」/wall/壁/墙/muro — say 한도/limit/上限/límite instead:\n{}",
        walled.join("\n")
    );
}

#[test]
fn no_korean_source_word_in_the_window_says_wall() {
    let mut walled = Vec::new();
    for (name, part) in WINDOW_PARTS {
        for line in strip_comments(part).lines() {
            if korean_wall(line) {
                walled.push(format!("{name}: {}", line.trim()));
            }
        }
    }
    let markup = include_str!("../../../../ui/index.html");
    let mut rest = markup;
    while let Some(open) = rest.find("<!--") {
        let shown = &rest[..open];
        walled.extend(
            shown
                .lines()
                .filter(|line| korean_wall(line))
                .map(|line| format!("index.html: {}", line.trim())),
        );
        rest = rest[open..]
            .find("-->")
            .map_or("", |close| &rest[open + close + 3..]);
    }
    walled.extend(
        rest.lines()
            .filter(|line| korean_wall(line))
            .map(|line| format!("index.html: {}", line.trim())),
    );
    assert!(
        walled.is_empty(),
        "these window words still say 「벽」 — 「한도에 닿은 판」, 「한도 도달」 are the plain words:\n{}",
        walled.join("\n")
    );
}

/// 백엔드가 지은 문장도 창에 그대로 선다 — 계정 전환의 거절이 초안 자리에
/// 서는 것처럼(`launch_budget_door::refusal_said`).
#[test]
fn no_sentence_the_backend_hands_the_window_says_wall() {
    let door = include_str!("../../src/launch_budget_door.rs");
    let shipped = door
        .split_once("#[cfg(test)]")
        .map_or(door, |(before, _)| before);
    let mut walled = Vec::new();
    for (name, source) in [
        ("launch_budget_door.rs", shipped),
        ("shipped backend", shipped_backend()),
    ] {
        for line in strip_rust_comments(source).lines() {
            if korean_wall(line) {
                walled.push(format!("{name}: {}", line.trim()));
            }
        }
    }
    assert!(
        walled.is_empty(),
        "these backend sentences still say 「벽」:\n{}",
        walled.join("\n")
    );
}

#[test]
fn the_wall_reader_knows_the_five_words_and_leaves_the_rest() {
    for said in [
        "한도 벽",
        "벽에 선 판 {{count}}개",
        "walled panes {{count}}",
        "Quota wall",
        "上限の壁",
        "墙前窗格 {{count}}",
        "paneles en el muro {{count}}",
    ] {
        assert!(wall_word(said), "{said} is a wall word");
    }
    for said in [
        "완벽합니다.",
        "한도에 닿은 판 {{count}}",
        "the record of limit {{wall}} is gone",
        "wallet",
        "上限に達したペイン",
        "en su límite",
    ] {
        assert!(!wall_word(said), "{said} is not a wall word");
    }
}
