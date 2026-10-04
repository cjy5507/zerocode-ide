//! The batch road, held with a seat of its own that judges numbers: what a
//! request carries once, how a batch above the cap is cut, and — the rule of
//! stage one — that no item's question or reading touches another's.

use serde_json::{Map, Value, json};

use super::*;

/// What every request of the toy seat carries once, in its shared part: the
/// words the road must not repeat for each item.
const SHARED_WORDS: &str = "judge each number alone, by its own value";

/// The suffix of the toy seat's second question about an item.
const WHOLE: &str = "whole";

/// A seat that judges numbers: one closed choice about each, and a Noul
/// beside it.
struct Toy {
    cap: usize,
}

impl Judgment for Toy {
    type Verdict = String;
    type Refusal = &'static str;

    fn cap(&self) -> usize {
        self.cap
    }

    fn items_key(&self) -> &'static str {
        "numbers"
    }

    fn shared(&self) -> Map<String, Value> {
        Map::from_iter([("rubric".to_string(), Value::from(SHARED_WORDS))])
    }

    fn questions(&self, at: usize) -> Vec<(&'static str, Value)> {
        vec![
            (
                "",
                json!({
                    "type": "choice",
                    "instructions": format!("Is `numbers[{at}]` big or small?"),
                    "criteria": { "big": "More than ten.", "small": "Ten or less." },
                }),
            ),
            (
                WHOLE,
                json!({
                    "type": "noul",
                    "instructions": format!("Is `numbers[{at}]` a whole number?"),
                    "criteria": { "true": "It is.", "false": "It is not." },
                }),
            ),
        ]
    }

    fn read(&self, answers: &Answers<'_>) -> Result<String, &'static str> {
        let said = answers.get("").ok_or("no_answer")?;
        let word = said
            .get("choice")
            .and_then(Value::as_str)
            .ok_or("not_a_choice")?;
        Ok(word.to_string())
    }
}

/// A seat whose second question's suffix starts with a digit: the shape that,
/// joined to an item's place as it stands, reads as another item's place.
struct Digits;

impl Judgment for Digits {
    type Verdict = ();
    type Refusal = ();

    fn cap(&self) -> usize {
        20
    }

    fn items_key(&self) -> &'static str {
        "numbers"
    }

    fn shared(&self) -> Map<String, Value> {
        Map::new()
    }

    fn questions(&self, at: usize) -> Vec<(&'static str, Value)> {
        let big = json!({
            "type": "choice",
            "instructions": format!("Is `numbers[{at}]` big?"),
            "criteria": { "yes": "It is.", "no": "It is not." },
        });
        vec![("", big.clone()), ("2", big)]
    }

    fn read(&self, _answers: &Answers<'_>) -> Result<(), ()> {
        Ok(())
    }
}

/// `count` items' facts, each naming its own number.
fn numbers(count: usize) -> Vec<Value> {
    (0..count).map(|n| json!({ "n": n })).collect()
}

/// A reply that answers the first question of each item named, with the word
/// beside it.
fn reply(chosen: &[(usize, &str)]) -> Value {
    Value::Object(
        chosen
            .iter()
            .map(|(item, word)| {
                (
                    question_name(*item, ""),
                    json!({ "type": "choice", "choice": word, "confidence": 0.9 }),
                )
            })
            .collect(),
    )
}

/// A batch is cut into requests of at most the cap, as evenly as it can be,
/// every item in exactly one of them and in its place; nothing to judge is
/// nothing asked.
#[test]
fn a_batch_is_cut_into_even_requests_none_over_the_cap_and_every_item_once() {
    for (count, cap) in [(1_usize, 4_usize), (4, 4), (5, 4), (9, 4), (17, 4), (10, 1)] {
        let asked = requests(&Toy { cap }, numbers(count));
        let sizes: Vec<usize> = asked.iter().map(|request| request.items().len()).collect();
        assert_eq!(
            sizes.iter().sum::<usize>(),
            count,
            "{count} at {cap}: every item is asked about, once ({sizes:?})"
        );
        assert_eq!(
            asked.len(),
            count.div_ceil(cap),
            "{count} at {cap}: no more requests than the cap needs ({sizes:?})"
        );
        let widest = sizes.iter().max().copied().unwrap_or(0);
        let narrowest = sizes.iter().min().copied().unwrap_or(0);
        assert!(widest <= cap, "{count} at {cap}: {sizes:?}");
        assert!(widest - narrowest <= 1, "{count} at {cap}: {sizes:?}");
        let mut next = 0;
        for request in &asked {
            assert_eq!(request.items().start, next, "{count} at {cap}: end to end");
            next = request.items().end;
            let listed = request.state["numbers"].as_array().expect("the items");
            assert_eq!(listed.len(), request.items().len());
            for (at, item) in request.items().enumerate() {
                assert_eq!(listed[at], json!({ "n": item }), "item {item}'s own facts");
            }
        }
        assert_eq!(next, count, "{count} at {cap}");
    }
    assert!(
        requests(&Toy { cap: 4 }, Vec::new()).is_empty(),
        "a batch with nothing in it asks nothing"
    );
}

/// What every item shares travels once in a request — not once per item —
/// whatever the number of items: the whole point of asking them together.
#[test]
fn the_shared_words_travel_once_per_request_whatever_the_number_of_items() {
    let judgment = Toy { cap: 50 };
    for count in [1_usize, 2, 7, 50] {
        let asked = requests(&judgment, numbers(count));
        assert_eq!(
            asked.len(),
            1,
            "{count} items under the cap are one request"
        );
        let whole = json!({ "state": asked[0].state, "questions": asked[0].questions });
        assert_eq!(
            whole.to_string().matches(SHARED_WORDS).count(),
            1,
            "{count} items"
        );
        assert!(
            !asked[0].questions.to_string().contains(SHARED_WORDS),
            "{count} items: a question repeats what the state says once"
        );
    }
}

/// A question names its item by its place in its OWN request's list — which
/// is where its facts stand — and is named by the item's place in the whole
/// batch, so a name is unique across the requests of one batch.
#[test]
fn a_question_names_its_item_by_its_place_in_its_own_request_and_in_the_whole_batch() {
    let asked = requests(&Toy { cap: 3 }, numbers(7));
    let spans: Vec<Range<usize>> = asked.iter().map(Request::items).collect();
    assert_eq!(spans, [0..3, 3..5, 5..7], "seven at three: 3, 2 and 2");
    for request in &asked {
        let questions = request.questions.as_object().expect("questions");
        assert_eq!(questions.len(), request.items().len() * 2, "two per item");
        for (at, item) in request.items().enumerate() {
            for suffix in ["", WHOLE] {
                let said = questions[&question_name(item, suffix)]["instructions"]
                    .as_str()
                    .expect("its words");
                assert!(
                    said.contains(&format!("numbers[{at}]")),
                    "item {item} is `numbers[{at}]` here: {said}"
                );
            }
        }
    }
    assert_eq!(
        asked[1].questions["q3"]["instructions"], "Is `numbers[0]` big or small?",
        "the first item of the second request is its place 0"
    );
}

/// Stage one's rule on the building side: a request is made of the items and
/// the seat's shared words and of nothing else — the same again to the byte —
/// and every question of it is ONE item's: it names its own place and no
/// other, and none stands under a name of its own, as a comparison between
/// items would.
#[test]
fn no_question_is_built_from_another_items_answer_or_asks_about_another_item() {
    let judgment = Toy { cap: 4 };
    let first = requests(&judgment, numbers(9));
    assert_eq!(first.len(), 3);
    assert_eq!(first, requests(&judgment, numbers(9)));
    for request in &first {
        let questions = request.questions.as_object().expect("questions");
        for item in request.items() {
            let at = item - request.items().start;
            for suffix in ["", WHOLE] {
                let name = question_name(item, suffix);
                let said = questions[&name]["instructions"].as_str().expect("words");
                for other in (0..request.items().len()).filter(|other| *other != at) {
                    assert!(
                        !said.contains(&format!("numbers[{other}]")),
                        "{name} reaches `numbers[{other}]`: {said}"
                    );
                }
            }
        }
        assert_eq!(
            questions.len(),
            request.items().len() * 2,
            "a question that is no item's own is a comparison, and a comparison is stage two"
        );
    }
}

/// Stage one's rule on the reading side: an item is read from its own
/// answers. A neighbour's answer that is broken or missing refuses that
/// neighbour alone; an answer put under another item's name is that item's.
#[test]
fn an_item_is_read_from_its_own_answers_and_no_neighbours() {
    let judgment = Toy { cap: 8 };
    let asked = requests(&judgment, numbers(3));
    assert_eq!(asked.len(), 1);
    let request = &asked[0];
    let big = || -> Result<String, &'static str> { Ok("big".to_string()) };
    let small = || -> Result<String, &'static str> { Ok("small".to_string()) };

    let whole = reply(&[(0, "big"), (1, "small"), (2, "big")]);
    assert_eq!(request.read(&judgment, &whole), vec![big(), small(), big()]);

    let mut broken = whole.clone();
    broken["q1"] = json!("not an answer at all");
    assert_eq!(
        request.read(&judgment, &broken),
        vec![big(), Err("not_a_choice"), big()],
        "one item's broken answer refuses that item alone"
    );

    let mut missing = whole.clone();
    missing
        .as_object_mut()
        .expect("answers")
        .remove(&question_name(1, ""));
    assert_eq!(
        request.read(&judgment, &missing),
        vec![big(), Err("no_answer"), big()],
        "one item's missing answer refuses that item alone"
    );

    // The answer item 0 gave, found under item 1's name, is item 1's to read.
    let swapped = reply(&[(1, "big"), (2, "small")]);
    assert_eq!(
        request.read(&judgment, &swapped),
        vec![Err("no_answer"), big(), small()],
        "an answer is read by the name of the item it was asked about"
    );
}

/// A name is one item's one question whatever its suffix says: item 1's
/// question `2` is not item 12's own question, as a name made by writing the
/// place and the suffix side by side would make it (`q1` and `2` are `q12`).
/// Nothing is written over another question, and an item is never read from
/// another item's answer.
#[test]
fn a_suffix_that_starts_with_a_digit_is_not_another_items_name() {
    let items = 13;
    let asked = requests(&Digits, numbers(items));
    assert_eq!(asked.len(), 1);
    let questions = asked[0].questions.as_object().expect("questions");
    assert_eq!(
        questions.len(),
        items * 2,
        "a question of its own for every item and suffix: one was written over another"
    );
    let all = json!({ (question_name(12, "")): "item twelve's own answer" });
    let item = 1;
    assert_eq!(
        Answers { all: &all, item }.get("2"),
        None,
        "item 1 was read from an answer item 12 gave"
    );
}

/// The view an item is read through holds that item's names and no other's:
/// `q1` is not `q10`, and item 0's answer is not item 1's.
#[test]
fn the_view_holds_one_items_names_and_no_others() {
    let whole = json!({ "q0": "zero", "q0/whole": "zero too", "q1": "one", "q10": "ten" });
    let of = |item| Answers { all: &whole, item };
    assert_eq!(of(1).get(""), Some(&json!("one")));
    assert_eq!(of(0).get(WHOLE), Some(&json!("zero too")));
    assert_eq!(of(1).get(WHOLE), None, "item 0's name is not item 1's");
    assert_eq!(of(10).get(""), Some(&json!("ten")));
    assert_eq!(of(2).get(""), None);
}
