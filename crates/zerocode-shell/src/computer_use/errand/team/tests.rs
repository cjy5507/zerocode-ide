//! The second reader over a zo that is a script: what it is started with,
//! what it is told, how its words are read, and how its silence is bounded.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;
use zerocode_core::jev::choice::ChoiceRefusal;
use zerocode_core::screen_action::{ActionChoice, ActionLook, Chosen, Errand, Where, ask};

use super::*;

/// The question every case here asks: two controls, a goal.
fn asked() -> ActionAsk {
    let items = vec![
        json!({ "mark": 1, "role": "button", "label": "저장", "centerX": 10.0, "centerY": 20.0 }),
        json!({ "mark": 2, "role": "button", "label": "닫기", "centerX": 30.0, "centerY": 20.0 }),
    ];
    ask(&ActionLook {
        goal: "채팅방 열기",
        errand: Errand::Goal,
        at: Where::Page {
            host: "app.local",
            path: "/settings",
        },
        tried: &[],
        items: &items,
        pressed: &[],
        shows: &[],
    })
    .expect("a screen with controls asks")
}

/// A zo that is a shell script: it records its arguments and its stdin
/// beside `record`, and writes `answer` to the file `--last-message` names
/// after sleeping `sleep_s`.
fn fake_zo(root: &Path, answer: &str, sleep_s: &str) -> (PathBuf, PathBuf) {
    let record = root.join("record");
    std::fs::create_dir_all(&record).expect("a record folder");
    let program = root.join("zo");
    let body = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$RECORD/argv\"\ncat > \"$RECORD/stdin\"\nout=\"\"\nwhile [ $# -gt 0 ]; do case \"$1\" in --last-message) out=\"$2\"; shift;; esac; shift; done\nsleep {sleep_s}\nprintf '%s' \"$ANSWER\" > \"$out\"\n",
    );
    std::fs::write(&program, body).expect("the script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let _ = answer;
    (program, record)
}

fn judge_over(program: PathBuf, record: &Path, answer: &str, model: Option<&str>) -> TeamJudge {
    TeamJudge::at(
        program,
        vec![
            ("RECORD".to_string(), record.to_string_lossy().into_owned()),
            ("ANSWER".to_string(), answer.to_string()),
        ],
        None,
        model.map(str::to_string),
    )
}

#[test]
fn the_reader_is_started_headless_in_a_session_of_its_own_and_told_the_whole_question() {
    let root = tempfile::tempdir().expect("a root");
    let (program, record) = fake_zo(root.path(), "", "0");
    let mut judge = judge_over(
        program,
        &record,
        r#"{"choice":"mark:2","confidence":0.9}"#,
        None,
    );
    let asked = asked();

    let Judged::Chose(choice) = judge.choose(&asked) else {
        panic!("a well formed answer is a choice");
    };
    assert_eq!(choice.chosen, Chosen::Mark(2));
    assert_eq!(choice.confidence, 0.9);
    assert_eq!(choice.probabilities, [("mark:2".to_string(), 0.9)].into());

    let argv = std::fs::read_to_string(record.join("argv")).expect("the arguments");
    let argv: Vec<&str> = argv.lines().collect();
    for word in ["-p", "--no-spawn", "--effort", EFFORT, "--last-message"] {
        assert!(argv.contains(&word), "{word} is not among {argv:?}");
    }
    let at = argv
        .iter()
        .position(|w| *w == "--permission-mode")
        .expect("read-only");
    assert_eq!(argv[at + 1], "read-only");
    for never in ["--resume", "--continue", "-c", "--model", "--json"] {
        assert!(!argv.contains(&never), "{never} must not be among {argv:?}");
    }
    let stdin = std::fs::read_to_string(record.join("stdin")).expect("the prompt");
    assert!(stdin.starts_with(PROMPT_HEAD));
    assert!(
        stdin.contains(&asked.state.to_string()),
        "the state, as the seat rendered it"
    );
    assert!(
        stdin.contains(&asked.questions.to_string()),
        "the question, as the seat rendered it"
    );
    assert!(stdin.contains("options: mark:1, mark:2, give_up, done"));
    assert!(stdin.trim_end().ends_with(ANSWER_CONTRACT));
    let last = judge.last().expect("what it cost");
    assert_eq!(last.prompt_bytes, stdin.len());
    assert!(last.answer_bytes > 0);
    assert_eq!(last.model, None);
}

#[test]
fn a_pinned_fast_model_is_named_and_an_unpinned_one_is_zos_own_pick() {
    let root = tempfile::tempdir().expect("a root");
    let (program, record) = fake_zo(root.path(), "", "0");
    let mut judge = judge_over(
        program,
        &record,
        r#"{"choice":"mark:1","confidence":0.8}"#,
        Some("fast-model-x"),
    );
    assert!(matches!(judge.choose(&asked()), Judged::Chose(_)));
    let argv = std::fs::read_to_string(record.join("argv")).expect("the arguments");
    let argv: Vec<&str> = argv.lines().collect();
    let at = argv.iter().position(|w| *w == "--model").expect("the pin");
    assert_eq!(argv[at + 1], "fast-model-x");

    assert_eq!(
        pinned_fast_model(&json!({
            "modelRouter": { "roles": { "fast": { "mode": "pinned", "model": " m-1 " } } }
        })),
        Some("m-1".to_string())
    );
    for unpinned in [
        json!({}),
        json!({ "modelRouter": { "roles": { "fast": { "mode": "auto" } } } }),
        json!({ "modelRouter": { "roles": { "fast": { "mode": "pinned", "model": "" } } } }),
        json!({ "modelRouter": { "roles": { "coding": { "mode": "pinned", "model": "m" } } } }),
    ] {
        assert_eq!(pinned_fast_model(&unpinned), None, "{unpinned}");
    }
}

#[test]
fn the_readers_words_are_read_through_the_question_and_anything_else_is_refused() {
    let asked = asked();
    let read = |text: &str| TeamJudge::read_answer(&asked, text);
    assert!(
        matches!(
            read("Sure! {\"choice\":\"mark:1\",\"confidence\":0.75} — done."),
            Judged::Chose(_)
        ),
        "a sentence around the line is still read"
    );
    assert!(matches!(
        read("```json\n{\"choice\": \"give_up\", \"confidence\": 0.6}\n```"),
        Judged::Chose(ActionChoice {
            chosen: Chosen::GiveUp,
            ..
        })
    ));
    assert_eq!(
        read("{\"choice\":\"mark:9\",\"confidence\":0.9}"),
        Judged::Refused(ChoiceRefusal::UnknownOption.token().to_string()),
        "a number nobody offered"
    );
    assert_eq!(
        read("{\"choice\":\"mark:1\",\"confidence\":1.5}"),
        Judged::Refused(ChoiceRefusal::NotOne.token().to_string()),
        "a confidence that is not a share"
    );
    for garbage in [
        "",
        "I would press 1.",
        "{\"choice\":\"mark:1\"}",
        "{not json",
        "[1,2]",
    ] {
        assert_eq!(
            read(garbage),
            Judged::Refused(SCHEMA.to_string()),
            "{garbage:?}"
        );
    }
}

#[test]
fn a_reader_that_outlives_the_wall_is_killed_and_its_silence_is_a_timeout() {
    let root = tempfile::tempdir().expect("a root");
    let (program, record) = fake_zo(root.path(), "", "5");
    let mut judge = judge_over(
        program,
        &record,
        r#"{"choice":"mark:1","confidence":0.9}"#,
        None,
    );
    let began = std::time::Instant::now();
    assert_eq!(
        judge.choose_within(&asked(), Duration::from_millis(300)),
        Judged::Refused(TIMEOUT.to_string())
    );
    assert!(
        began.elapsed() < Duration::from_secs(3),
        "killed at the wall, not waited out"
    );
    assert_eq!(
        judge.choose_within(&asked(), Duration::ZERO),
        Judged::Refused(TIMEOUT.to_string()),
        "no time left asks nothing"
    );
}

#[test]
fn a_reader_that_cannot_be_started_is_transport() {
    let mut judge = TeamJudge::at(PathBuf::from("/nonexistent/zo"), Vec::new(), None, None);
    assert_eq!(
        judge.choose(&asked()),
        Judged::Refused(TRANSPORT.to_string())
    );
}

/// What the second rung costs against the real zo: the same marked look the
/// Jev bench reads (`ZEROCODE_JEV_BENCH_LOOK`, `ZEROCODE_JEV_BENCH_GOAL`),
/// asked `ZEROCODE_JEV_BENCH_PASSES` times of the person's own zo under the
/// window's login (`ZEROCODE_TEAM_CONFIG_ROOT`, the window's config root).
/// Prints each pass's answer, wall time and prompt bytes, then reads the
/// transcript the headless session wrote for its model and token usage and
/// prices it by the standing list (`model_prices::price`). Asserts nothing
/// about the answer. Not part of the gate — it spends a frontier turn.
#[test]
#[ignore = "spends the person's own zo on the real frontier"]
fn what_the_second_reader_costs_against_the_real_zo() {
    let look = std::env::var("ZEROCODE_JEV_BENCH_LOOK").expect("a marked look's json");
    let goal = std::env::var("ZEROCODE_JEV_BENCH_GOAL").expect("a goal sentence");
    let config_root = PathBuf::from(
        std::env::var("ZEROCODE_TEAM_CONFIG_ROOT").expect("the window's config root"),
    );
    let passes: usize = std::env::var("ZEROCODE_JEV_BENCH_PASSES")
        .ok()
        .and_then(|passes| passes.parse().ok())
        .unwrap_or(3);
    let said: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&look).expect("the look")).expect("json");
    let said = said.get("result").unwrap_or(&said);
    let items = said
        .pointer("/marks/items")
        .or_else(|| said.get("items"))
        .and_then(serde_json::Value::as_array)
        .expect("items")
        .clone();
    let asked = ask(&ActionLook {
        goal: &goal,
        errand: Errand::Goal,
        at: Where::Page {
            host: "bench.local",
            path: "/",
        },
        tried: &[],
        items: &items,
        pressed: &[],
        shows: &[],
    })
    .expect("a screen with controls asks");
    let scratch = tempfile::tempdir().expect("a session directory of its own");
    let mut judge = TeamJudge::new(&config_root, Some(scratch.path())).expect("zo and a login");
    let began_at = std::time::SystemTime::now();
    let mut answered = 0usize;
    let mut waits = Vec::new();
    for pass in 1..=passes {
        let judged = judge.choose(&asked);
        let last = judge.last().cloned().expect("what it cost");
        waits.push(last.elapsed_ms);
        match &judged {
            Judged::Chose(choice) => {
                answered += 1;
                println!(
                    "pass {pass}: {:?} confidence {} in {} ms (prompt {} B, answer {} B, model {:?})",
                    choice.chosen,
                    choice.confidence,
                    last.elapsed_ms,
                    last.prompt_bytes,
                    last.answer_bytes,
                    last.model
                );
            }
            Judged::Refused(token) => println!(
                "pass {pass}: refused {token} in {} ms (prompt {} B)",
                last.elapsed_ms, last.prompt_bytes
            ),
        }
    }
    // The sessions the headless runs wrote, newest first, priced by the list.
    let home = dirs::home_dir().expect("a home");
    let mut cost_usd = 0.0_f64;
    let mut priced = 0usize;
    let mut models = std::collections::BTreeSet::new();
    if let Ok(projects) = std::fs::read_dir(home.join(".zo").join("projects")) {
        for project in projects.flatten() {
            let Ok(sessions) = std::fs::read_dir(project.path().join("sessions")) else {
                continue;
            };
            for session in sessions.flatten() {
                let path = session.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                let fresh = std::fs::metadata(&path)
                    .and_then(|meta| meta.modified())
                    .is_ok_and(|modified| modified >= began_at);
                if !fresh {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                for line in text.lines() {
                    let Ok(row) = serde_json::from_str::<serde_json::Value>(line) else {
                        continue;
                    };
                    let Some(usage) = row.pointer("/message/usage").or_else(|| row.get("usage"))
                    else {
                        continue;
                    };
                    let model = row
                        .pointer("/message/model")
                        .or_else(|| row.get("model"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let Some(price) = model_prices::price(&model) else {
                        continue;
                    };
                    let tokens = |key: &str| {
                        usage
                            .get(key)
                            .and_then(serde_json::Value::as_f64)
                            .unwrap_or(0.0)
                    };
                    cost_usd += tokens("input_tokens") * price.input / 1e6
                        + tokens("cache_read_input_tokens") * price.cache_read / 1e6
                        + tokens("cache_creation_input_tokens") * price.cache_write / 1e6
                        + tokens("output_tokens") * price.output / 1e6;
                    priced += 1;
                    models.insert(model);
                }
            }
        }
    }
    waits.sort_unstable();
    println!(
        "measure: team-rung passes={passes} answered={answered} p50_ms={} max_ms={} priced_messages={priced} models={models:?} cost_usd_total={cost_usd:.5} cost_usd_per_pass={:.5}",
        waits[waits.len() / 2],
        waits[waits.len() - 1],
        cost_usd / passes as f64
    );
}
