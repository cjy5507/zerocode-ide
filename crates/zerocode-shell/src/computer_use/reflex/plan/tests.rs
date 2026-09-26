use std::collections::VecDeque;

use zerocode_core::computer_use_protocol::game_state::check_palette;
use zerocode_core::computer_use_protocol::reflex::Surface;

use super::*;
use crate::computer_use::errand::value::Tokens;
use zerocode_core::type_value::GeneratorRoad;

/// The scope a person named: the macOS desktop, the fixture app.
pub(in crate::computer_use::reflex) fn scope() -> Scope {
    Scope {
        surface: Surface::MacosDesktop,
        target: "com.example.Fixture".into(),
    }
}

/// The contract's golden plan, as a model answers it: for `scope`, its hash
/// left empty.
pub(in crate::computer_use::reflex) fn answer_for(scope: &Scope) -> Value {
    let mut plan = serde_json::from_str::<Value>(EXAMPLE).expect("the golden")["plan"].clone();
    plan["scope"] = json!(scope);
    plan["plan_hash"] = json!("");
    plan
}

/// A 100 × 60 capture of a 100 × 60-point display: wallpaper around the
/// app's window at (10, 10) 80 × 40 — a black ground with a near-black
/// curtain strip, a red target with one pixel of its edge a shade off, and a
/// smaller blue decoy.
pub(in crate::computer_use::reflex) fn capture() -> (RgbaImage, Stage) {
    let (width, height) = (100_u32, 60_u32);
    let mut pixels = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let inside = (10..90).contains(&x) && (10..50).contains(&y);
            let colour: [u8; 3] = if !inside {
                [200, 100, 50]
            } else if (20..30).contains(&x) && (20..30).contains(&y) {
                if (x, y) == (20, 20) {
                    [250, 5, 5]
                } else {
                    [255, 0, 0]
                }
            } else if (60..66).contains(&x) && (30..36).contains(&y) {
                [0, 0, 255]
            } else if (40..44).contains(&x) {
                [14, 14, 14]
            } else {
                [0, 0, 0]
            };
            pixels.extend_from_slice(&[colour[0], colour[1], colour[2], 255]);
        }
    }
    (
        RgbaImage::new(width, height, pixels).expect("an image"),
        Stage {
            width: 100,
            height: 60,
            window: Rect {
                x: 10,
                y: 10,
                width: 80,
                height: 40,
            },
        },
    )
}

/// The palette is the app's window and nothing around it: its largest
/// colour the ground and the rest by area, a shade a class at the table's
/// widest tolerance cannot tell apart merged into the larger colour, each
/// class as wide as keeps it apart from the others and never wider than the
/// table — so the table's own check passes it — and a window of one colour
/// is no palette. A capture twice the display's size reads the same window.
#[test]
fn a_palette_is_the_windows_colours_by_area_apart_and_inside_the_table() {
    let (image, stage) = capture();
    let palette = palette_of(&image, &stage).expect("a palette");
    let rgb = |class: &ColorClass| [class.r, class.g, class.b];
    assert_eq!(
        rgb(&palette.ground),
        [0, 0, 0],
        "the largest area is the ground"
    );
    assert_eq!(
        palette.classes.iter().map(rgb).collect::<Vec<_>>(),
        [[255, 0, 0], [0, 0, 255]],
        "the rest by area: the curtain is the ground's, the edge shade the target's, the wallpaper nobody's"
    );
    let widest = u8::try_from(game_state::LIMITS.max_tolerance).expect("a tolerance");
    assert!(
        palette
            .classes
            .iter()
            .all(|class| class.tolerance == widest)
    );
    assert_eq!(
        check_palette(&palette.classes, Some(&palette.ground), &game_state::LIMITS),
        Ok(())
    );
    // 3,200 pixels: the target's 100, the decoy's 36, the ground the rest.
    assert_eq!(palette.area_permille, [957, 31, 11]);
    // Twice the pixels for the same display points: the same colours.
    let doubled = RgbaImage::new(
        image.width * 2,
        image.height * 2,
        (0..image.height * 2)
            .flat_map(|y| {
                let row = image.clone();
                (0..image.width * 2).flat_map(move |x| {
                    let at = usize::try_from((y / 2) * row.width + x / 2).unwrap_or(0) * 4;
                    row.pixels[at..at + 4].to_vec()
                })
            })
            .collect(),
    )
    .expect("a doubled image");
    let again = palette_of(&doubled, &stage).expect("a palette");
    assert_eq!(
        (again.ground, again.classes.clone()),
        (palette.ground, palette.classes.clone())
    );
    // A window of one colour, or one the capture does not show.
    let flat = RgbaImage::new(10, 10, [0, 0, 0, 255].repeat(100)).expect("an image");
    let whole = Stage {
        width: 10,
        height: 10,
        window: Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        },
    };
    assert_eq!(palette_of(&flat, &whole), None);
    let elsewhere = Stage {
        window: Rect {
            x: 20,
            ..whole.window
        },
        ..whole
    };
    assert_eq!(
        palette_of(&image, &elsewhere).map(|palette| palette.ground),
        None
    );
}

/// However many colours a window shows, a palette holds the table's classes
/// at most, and still passes its check.
#[test]
fn a_palette_holds_the_tables_classes_at_most() {
    let colours: Vec<[u8; 3]> = (0_u8..3)
        .flat_map(|r| {
            (0_u8..3).flat_map(move |g| (0_u8..2).map(move |b| [r * 120, g * 120, b * 240]))
        })
        .collect();
    assert!(colours.len() > usize::try_from(game_state::LIMITS.max_classes).unwrap_or(0) + 1);
    let mut pixels = Vec::new();
    for (at, colour) in colours.iter().enumerate() {
        for _ in 0..=at {
            pixels.extend_from_slice(&[colour[0], colour[1], colour[2], 255]);
        }
    }
    let width = u32::try_from(pixels.len() / 4).expect("a width");
    let image = RgbaImage::new(width, 1, pixels).expect("an image");
    let stage = Stage {
        width: i64::from(width),
        height: 1,
        window: Rect {
            x: 0,
            y: 0,
            width: i64::from(width),
            height: 1,
        },
    };
    let palette = palette_of(&image, &stage).expect("a palette");
    assert_eq!(palette.classes.len() as u64, game_state::LIMITS.max_classes);
    assert_eq!(palette.area_permille.len(), palette.classes.len() + 1);
    assert_eq!(
        check_palette(&palette.classes, Some(&palette.ground), &game_state::LIMITS),
        Ok(())
    );
}

/// The stage is the app's first window on the display the person named, in
/// that display's points from its corner, and the scope names the app by its
/// bundle id — its name only where it has none. A window on another display,
/// an app with no window, or a display the helper does not show is said.
#[test]
fn the_stage_is_the_apps_window_on_the_display_named() {
    let displays = json!({ "displays": [
        { "index": 0, "scale": 2, "bounds": { "x": 0, "y": 0, "width": 1512, "height": 982 } },
        { "index": 1, "scale": 1, "bounds": { "x": 1512, "y": 0, "width": 1920, "height": 1080 } },
    ] });
    let window = |x: i64, bundle: Value| {
        json!({ "windows": [{ "id": 7, "x": x, "y": 120, "width": 720, "height": 440,
            "app": { "name": "Reflex Fixture", "bundleId": bundle, "pid": 99 } }] })
    };
    let (stage, target) =
        stage_of(&displays, &window(1712, json!("com.example.Fixture")), 1).expect("a stage");
    assert_eq!(
        stage,
        Stage {
            width: 1920,
            height: 1080,
            window: Rect {
                x: 200,
                y: 120,
                width: 720,
                height: 440,
            },
        }
    );
    assert_eq!(target, "com.example.Fixture");
    let (_, named) = stage_of(&displays, &window(1712, Value::Null), 1).expect("a stage");
    assert_eq!(named, "Reflex Fixture");
    assert!(stage_of(&displays, &window(100, json!("com.example.Fixture")), 1).is_err());
    assert!(stage_of(&displays, &json!({ "windows": [] }), 1).is_err());
    assert!(stage_of(&displays, &window(1712, json!("com.example.Fixture")), 4).is_err());
}

/// A plan is read the contract's way and no other: its words, the scope the
/// person named, the contract's hash in place of the one a model cannot
/// write, and the contract's own reader over the sections it writes — and a
/// refusal is the contract's own sentence, naming what it refused.
#[test]
fn a_plan_is_read_the_contracts_way_for_the_scope_the_person_named() {
    let answer = answer_for(&scope());
    let plan = plan_of(&answer.to_string(), &scope()).expect("the golden reads");
    assert_eq!(plan.plan().plan_hash, plan_hash(plan.plan()));
    assert_eq!(plan.plan().scope, scope());
    // One pair of code fences, its language word and all, is taken off.
    let fenced = format!(
        "```json\n{}\n```",
        serde_json::to_string_pretty(&answer).unwrap()
    );
    assert!(plan_of(&fenced, &scope()).is_ok(), "{fenced}");
    // Anything else is the model's to write again.
    let Err(Refused::Contract(prose)) = plan_of(&format!("Here is the plan: {answer}"), &scope())
    else {
        panic!("prose is no plan");
    };
    assert!(prose.contains("contract's words"), "{prose}");
    let mut unknown = answer.clone();
    unknown["detectors"][0]["aim"] = json!("nearest");
    let Err(Refused::Contract(field)) = plan_of(&unknown.to_string(), &scope()) else {
        panic!("a field the contract does not know is refused");
    };
    assert!(field.contains("aim"), "the refusal names it: {field}");
    // A word the contract offers reads; one it does not is refused by name.
    let mut picked = answer.clone();
    picked["detectors"][0]["pick"] = json!("nearest");
    assert!(plan_of(&picked.to_string(), &scope()).is_ok());
    picked["detectors"][0]["pick"] = json!("closest");
    let Err(Refused::Contract(word)) = plan_of(&picked.to_string(), &scope()) else {
        panic!("a pick the contract does not offer is refused");
    };
    assert!(
        word.contains("closest") && word.contains("nearest"),
        "{word}"
    );
    let mut empty = answer.clone();
    empty["rules"] = json!([]);
    assert_eq!(
        plan_of(&empty.to_string(), &scope()).err(),
        Some(Refused::Contract("reflex validation: Budget".into()))
    );
    // Another app's scope is the one refusal the model is told apart.
    let elsewhere = Scope {
        target: "com.example.Other".into(),
        ..scope()
    };
    let Err(Refused::Scope(sentence)) = plan_of(&answer_for(&elsewhere).to_string(), &scope())
    else {
        panic!("another scope is refused");
    };
    assert!(sentence.contains("com.example.Other") && sentence.contains("com.example.Fixture"));
}

/// A generator that answers from a script and remembers what it was asked.
pub(in crate::computer_use::reflex) struct Scripted {
    pub answers: VecDeque<Result<String, String>>,
    pub asked: Vec<(String, String)>,
}

impl Generator for Scripted {
    fn unready(&self) -> Option<String> {
        None
    }

    fn model(&self) -> Option<String> {
        Some("a-model".into())
    }

    fn ask(&mut self, system: &str, user: &str, _left: Duration) -> Result<Said, String> {
        self.asked.push((system.to_string(), user.to_string()));
        self.answers
            .pop_front()
            .expect("an answer scripted")
            .map(|text| Said {
                bytes_out: user.len(),
                bytes_in: text.len(),
                tokens: Some(Tokens {
                    input: 100,
                    output: 10,
                }),
                text,
                answered: Answered {
                    road: GeneratorRoad::CodexLogin.word(),
                    model: "a-model".into(),
                    passed: vec!["claude_login=quota_wall".into()],
                },
            })
    }
}

/// A generator standing in for a model: a script's answers under a word of
/// its own.
pub(in crate::computer_use::reflex) struct Named {
    pub scripted: Scripted,
    pub source: &'static str,
}

impl Generator for Named {
    fn unready(&self) -> Option<String> {
        self.scripted.unready()
    }

    fn model(&self) -> Option<String> {
        None
    }

    fn ask(&mut self, system: &str, user: &str, left: Duration) -> Result<Said, String> {
        self.scripted.ask(system, user, left)
    }

    fn source(&self) -> &'static str {
        self.source
    }
}

fn ask_of<'a>(stage: &'a Stage, palette: &'a Palette, scope: &'a Scope) -> Ask<'a> {
    Ask {
        goal: "press the red dots, never the blue",
        scope,
        stage,
        palette,
        previous: None,
    }
}

/// Two login roads, as `auto` asks them (t-10372): the first answers with
/// plans the contract refuses, the second with the plan; each answer says
/// which road gave it and the roads set aside before it.
struct TwoRoads {
    first: VecDeque<String>,
    second: VecDeque<String>,
    on_second: bool,
    set_aside: Vec<String>,
}

impl Generator for TwoRoads {
    fn unready(&self) -> Option<String> {
        None
    }

    fn model(&self) -> Option<String> {
        Some("first-model".into())
    }

    fn ask(&mut self, _system: &str, user: &str, _left: Duration) -> Result<Said, String> {
        let (text, road, model) = if self.on_second {
            (self.second.pop_front(), "codex_login", "second-model")
        } else {
            (self.first.pop_front(), "claude_login", "first-model")
        };
        let text = text.expect("an answer scripted");
        Ok(Said {
            bytes_out: user.len(),
            bytes_in: text.len(),
            tokens: Some(Tokens {
                input: 100,
                output: 10,
            }),
            text,
            answered: Answered {
                road,
                model: model.into(),
                passed: self.set_aside.clone(),
            },
        })
    }

    fn pass_over(&mut self, why: &str) -> bool {
        if self.on_second {
            return false;
        }
        self.on_second = true;
        self.set_aside.push(format!("claude_login={why}"));
        true
    }
}

/// A road whose every answer the contract refused is set aside for the next
/// road `auto` asks (t-10372): the next road is asked afresh — none of the
/// first road's refusals in its question — its plan is the one written, and
/// the row carries every request, both roads' refusals and the road set
/// aside with its reason. A generator with no next road ends refused.
#[test]
fn a_road_whose_plans_the_contract_refused_is_passed_over_for_the_next_login() {
    let (image, stage) = capture();
    let palette = palette_of(&image, &stage).expect("a palette");
    let scope = scope();
    let good = answer_for(&scope).to_string();
    let refused = || VecDeque::from(vec!["no".to_string(), "no".into(), "no".into()]);
    let mut generator = TwoRoads {
        first: refused(),
        second: VecDeque::from(vec![good]),
        on_second: false,
        set_aside: Vec::new(),
    };
    let written = write_plan(&mut generator, &ask_of(&stage, &palette, &scope));
    assert!(written.plan.is_ok(), "{:?}", written.plan.as_ref().err());
    let tries = usize::try_from(REFLEX_PLAN_RETRIES).expect("a count") + 1;
    assert_eq!(written.requests as usize, tries + 1);
    assert_eq!(written.refusals.len(), tries);
    let answered = written.answered.as_ref().expect("an answer");
    assert_eq!(answered.road, "codex_login");
    assert_eq!(answered.model, "second-model");
    assert_eq!(
        answered.passed,
        vec![format!("claude_login={PLAN_REFUSED}")]
    );
    let row = ledger_row(
        0,
        None,
        1,
        "g",
        &palette,
        Some("first-model"),
        &written,
        "answered",
    );
    assert_eq!(row["model"], json!("second-model"));
    assert_eq!(
        row["passedOver"],
        json!([format!("claude_login={PLAN_REFUSED}")])
    );

    // With no road after it, the refusal stands.
    let mut alone = TwoRoads {
        first: refused(),
        second: refused(),
        on_second: true,
        set_aside: Vec::new(),
    };
    let written = write_plan(&mut alone, &ask_of(&stage, &palette, &scope));
    assert_eq!(written.plan.err().as_deref(), Some(PLAN_REFUSED));
    assert_eq!(written.requests as usize, tries);
}

/// A refused plan is asked for again with every refusal so far attached, at
/// most the table's retries — a second plan for another scope ends it at
/// once — and a request the wire could not answer is not asked again. What
/// every request cost adds up on the row.
#[test]
fn a_refused_plan_is_asked_again_with_its_refusals_at_most_the_retries() {
    let (image, stage) = capture();
    let palette = palette_of(&image, &stage).expect("a palette");
    let scope = scope();
    let good = answer_for(&scope).to_string();
    let other = answer_for(&Scope {
        target: "com.example.Other".into(),
        ..scope.clone()
    })
    .to_string();
    let run = |answers: Vec<Result<String, String>>| {
        let mut generator = Scripted {
            answers: answers.into(),
            asked: Vec::new(),
        };
        let written = write_plan(&mut generator, &ask_of(&stage, &palette, &scope));
        (written, generator.asked)
    };
    let (written, asked) = run(vec![Ok("no".into()), Ok("{}".into()), Ok(good.clone())]);
    assert!(written.plan.is_ok());
    assert_eq!(written.requests, 3);
    assert_eq!(written.refusals.len(), 2);
    assert_eq!(
        written.tokens,
        Some(Tokens {
            input: 300,
            output: 30
        })
    );
    let last: Value = serde_json::from_str(&asked[2].1).expect("a json request");
    assert_eq!(
        last["refused"],
        json!(written.refusals),
        "every refusal so far, as said"
    );
    assert!(serde_json::from_str::<Value>(&asked[0].1).expect("json")["refused"].is_null());
    assert!(asked.iter().all(|(system, _)| system == INSTRUCTIONS));
    let (written, _) = run(vec![Ok("no".into()), Ok("no".into()), Ok("no".into())]);
    assert_eq!(written.plan.err().as_deref(), Some(PLAN_REFUSED));
    assert_eq!(written.requests, 1 + REFLEX_PLAN_RETRIES);
    let (written, _) = run(vec![Ok(other.clone()), Ok(good)]);
    assert!(written.plan.is_ok(), "one scope refusal is asked again");
    let (written, _) = run(vec![Ok(other.clone()), Ok(other)]);
    assert_eq!(written.plan.err().as_deref(), Some(SCOPE_REFUSED));
    assert_eq!(
        written.requests, 2,
        "a second scope refusal ends it at once"
    );
    let (written, _) = run(vec![Err("http_503".into())]);
    assert_eq!(written.plan.err().as_deref(), Some("http_503"));
    assert_eq!(written.requests, 1, "the wire is not asked again");
}

/// What a plan is asked from is words and numbers: the goal, the contract's
/// version and the scope, the display, the window, the palette, the
/// contract's tables and its example — no picture of the screen, no
/// capture's bytes — and a re-plan adds the plan it replaces with its
/// outcomes.
#[test]
fn a_plan_is_asked_from_words_and_numbers_with_no_picture() {
    let (image, stage) = capture();
    let palette = palette_of(&image, &stage).expect("a palette");
    let scope = scope();
    let said = user_text(&ask_of(&stage, &palette, &scope), &[]);
    let sent: Value = serde_json::from_str(&said).expect("json");
    let keys: Vec<&str> = sent
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    let mut keys = keys;
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "contract", "display", "example", "goal", "limits", "palette", "window"
        ]
    );
    assert_eq!(sent["contract"]["version"], json!(VERSION));
    assert_eq!(
        sent["contract"]["pick"],
        json!(Pick::ALL.map(Pick::word)),
        "the contract's own words for a pick"
    );
    assert_eq!(sent["contract"]["scope"], json!(scope));
    assert_eq!(sent["palette"], palette.json());
    assert_eq!(sent["limits"]["reflex"], json!(reflex::LIMITS));
    assert_eq!(sent["limits"]["perception"], json!(game_state::LIMITS));
    assert_eq!(
        sent["example"]["version"],
        json!(VERSION),
        "the contract's own golden"
    );
    for word in ["image", "png", "base64", "screenshot", "data:"] {
        assert!(!said.contains(word), "{word}");
    }
    assert!(
        said.len() < image.pixels.len(),
        "{} bytes: smaller than one tiny capture's pixels",
        said.len()
    );
    let old = plan_of(&answer_for(&scope).to_string(), &scope).expect("a plan");
    let outcomes = json!({ "done": 4, "moved": 9 });
    let again = user_text(
        &Ask {
            previous: Some(Previous {
                plan: old.plan(),
                outcomes: &outcomes,
            }),
            ..ask_of(&stage, &palette, &scope)
        },
        &["refused once".into()],
    );
    let again: Value = serde_json::from_str(&again).expect("json");
    assert_eq!(again["previous"]["plan"], json!(old.plan()));
    assert_eq!(again["previous"]["outcomes"], outcomes);
    assert_eq!(again["refused"], json!(["refused once"]));
}

/// The words are pinned to their version: a word changed without a new
/// version is red here, because a plan's label is read per version.
#[test]
fn the_version_is_pinned_to_the_words() {
    assert_eq!(PROMPT_VERSION, 1);
    assert_eq!(
        zerocode_core::jev::fingerprint_of(INSTRUCTIONS),
        "7d2efde36762721d"
    );
}

/// A plan's ledger row says what the plan came to and what it cost, the goal
/// by its fingerprint alone.
#[test]
fn a_plan_leaves_one_row_with_its_cost_and_the_goal_by_its_fingerprint() {
    let (image, stage) = capture();
    let palette = palette_of(&image, &stage).expect("a palette");
    let scope = scope();
    let mut generator = Scripted {
        answers: vec![Ok("no".into()), Ok(answer_for(&scope).to_string())].into(),
        asked: Vec::new(),
    };
    let written = write_plan(&mut generator, &ask_of(&stage, &palette, &scope));
    let goal = "press the red dots, never the blue";
    let row = ledger_row(
        1_000,
        Some("rx-1"),
        1,
        goal,
        &palette,
        Some("a-model"),
        &written,
        "answered",
    );
    assert_eq!(row["run"], json!("rx-1"));
    assert_eq!(row["source"], json!(SOURCE_MODEL));
    assert_eq!(row["promptVersion"], json!(PROMPT_VERSION));
    assert_eq!(row["requests"], json!(2));
    assert_eq!(row["refusals"].as_array().map(Vec::len), Some(1));
    assert_eq!(row["tokens"], json!({ "input": 200, "output": 20 }));
    // Which road wrote the plan, the model that answered, and the road
    // passed over with its reason (t-10372).
    assert_eq!(row["road"], json!("codex_login"));
    assert_eq!(row["model"], json!("a-model"));
    assert_eq!(row["passedOver"], json!(["claude_login=quota_wall"]));
    assert_eq!(
        row["goalHash"],
        json!(zerocode_core::jev::fingerprint_of(goal))
    );
    assert!(
        !row.to_string().contains(goal),
        "the goal's words stay home"
    );
    assert_eq!(
        row["planHash"],
        json!(
            written
                .plan
                .as_ref()
                .map(|plan| plan.plan().plan_hash.clone())
                .ok()
        )
    );
}

/// What the plan road costs on this machine, for the report: a palette
/// counted off a full-resolution capture of a 1512 × 982-point display at
/// twice its points with an app window of 720 × 440, the request's bytes,
/// and a plan read the contract's way. Run by hand; prints, asserts nothing
/// about the numbers.
#[test]
#[ignore = "a measurement for the report"]
fn measure_the_plan_road() {
    let (width, height) = (3_024_u32, 1_964_u32);
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let colour: [u8; 4] = if (x / 40 + y / 40) % 7 == 0 {
                [255, 0, 0, 255]
            } else if (x / 55 + y / 23) % 11 == 0 {
                [0, 0, 255, 255]
            } else {
                [u8::try_from(x % 3).unwrap_or(0), 0, 0, 255]
            };
            pixels.extend_from_slice(&colour);
        }
    }
    let image = RgbaImage::new(width, height, pixels).expect("an image");
    let stage = Stage {
        width: 1_512,
        height: 982,
        window: Rect {
            x: 396,
            y: 271,
            width: 720,
            height: 440,
        },
    };
    let rank = |mut values: Vec<f64>, p: f64| {
        values.sort_by(f64::total_cmp);
        let at = ((p * values.len() as f64).ceil() as usize).clamp(1, values.len());
        values[at - 1]
    };
    let mut palette_ms = Vec::new();
    let mut palette = None;
    for _ in 0..9 {
        let began = Instant::now();
        palette = palette_of(&image, &stage);
        palette_ms.push(began.elapsed().as_secs_f64() * 1_000.0);
    }
    let palette = palette.expect("a palette");
    let scope = scope();
    let ask = Ask {
        goal: "press the red dots, never the blue",
        scope: &scope,
        stage: &stage,
        palette: &palette,
        previous: None,
    };
    let said = user_text(&ask, &[]);
    let answer = answer_for(&scope).to_string();
    let mut read_ms = Vec::new();
    for _ in 0..99 {
        let began = Instant::now();
        let _ = plan_of(&answer, &scope).expect("a plan");
        read_ms.push(began.elapsed().as_secs_f64() * 1_000.0);
    }
    println!(
        "{}",
        json!({
            "paletteMs": { "p50": rank(palette_ms.clone(), 0.5), "max": rank(palette_ms, 1.0) },
            "paletteClasses": palette.classes.len(),
            "requestBytes": { "system": INSTRUCTIONS.len(), "user": said.len() },
            "planReadMs": { "p50": rank(read_ms.clone(), 0.5), "p95": rank(read_ms, 0.95) },
        })
    );
}

/// A plan says where it came from by its generator's word: the window's
/// writer is a model's, and a generator standing in for one — the bench's —
/// is never counted as one on the plan's row.
#[test]
fn a_plan_says_which_generator_wrote_it() {
    assert_eq!(
        Generator::source(&LiveWriter::window(
            crate::computer_use::errand::value::Setup::new(
                GeneratorRoad::Auto,
                std::path::PathBuf::new(),
                std::path::PathBuf::new(),
            )
        )),
        SOURCE_MODEL
    );
    let (image, stage) = capture();
    let palette = palette_of(&image, &stage).expect("a palette");
    let scope = scope();
    let mut generator = Named {
        scripted: Scripted {
            answers: vec![Ok(answer_for(&scope).to_string())].into(),
            asked: Vec::new(),
        },
        source: "stub",
    };
    let written = write_plan(&mut generator, &ask_of(&stage, &palette, &scope));
    assert_eq!(written.source, "stub");
    let row = ledger_row(
        1_000,
        Some("rx-1"),
        1,
        "press the red dots, never the blue",
        &palette,
        None,
        &written,
        "answered",
    );
    assert_eq!(row["source"], json!("stub"));
}
