//! A repository's settings document may narrow what a session asks outside
//! itself, and never widen it.
//!
//! `<workspace>/.zo/settings.json` is committed with the repository: whoever
//! opens the folder runs under it without having read it. Merged after the
//! person's own settings, it had the last word on `smart`. A person who wrote
//! `"autoClassifier": "deterministic"` so that no model is asked about their
//! tasks had the question asked again in any repository that said `"probed"`
//! (measured on v1.1.150).
//!
//! So what such a document says is kept only where it asks no more than the
//! person's own documents do. A team can still switch the routing probe off
//! for its repository; it cannot switch it on for someone who switched it
//! off.
//!
//! The person's own documents are the ones the loader already takes the
//! `enableAllProject…` opt-ins from: the user settings, a `.zo/settings.local.json`
//! that is a real file git does not track, and a `--settings` file. A local
//! file the repository committed is read as the repository's.

use std::collections::BTreeMap;

use crate::json::JsonValue;
use crate::model_router::RouteAutoClassifierMode;

const SMART: &str = "smart";

/// One `smart` key a repository's document may only narrow: its name, and how
/// much a document that says `said` — or does not say the key — has the
/// session ask. The reading is the one the key's reader makes of that value
/// alone, so a value of the wrong type counts as the default it reads as.
struct Narrowed {
    key: &'static str,
    asks: fn(Option<&JsonValue>) -> u8,
}

const NARROWED: [Narrowed; 2] = [
    Narrowed { key: "enabled", asks: switch },
    Narrowed { key: "autoClassifier", asks: classifier },
];

/// A switch that is on unless a boolean says off.
fn switch(said: Option<&JsonValue>) -> u8 {
    u8::from(said.and_then(JsonValue::as_bool).unwrap_or(true))
}

fn classifier(said: Option<&JsonValue>) -> u8 {
    said.map_or(RouteAutoClassifierMode::UNSAID, |said| {
        RouteAutoClassifierMode::from_settings_word(said.as_str())
    })
    .asks()
}

/// Takes out of a repository's document every `smart` key that would have the
/// session ask more than `persons` — the person's own documents, merged —
/// says it does.
pub(super) fn only_narrows(
    repository: &mut BTreeMap<String, JsonValue>,
    persons: &BTreeMap<String, JsonValue>,
) {
    match repository.get_mut(SMART) {
        None => {}
        Some(JsonValue::Object(smart)) => {
            let persons = persons.get(SMART).and_then(JsonValue::as_object);
            for narrowed in &NARROWED {
                let own = (narrowed.asks)(persons.and_then(|smart| smart.get(narrowed.key)));
                if smart.get(narrowed.key).is_some_and(|said| (narrowed.asks)(Some(said)) > own) {
                    smart.remove(narrowed.key);
                }
            }
        }
        // Anything but an object replaces the person's whole `smart` in the
        // merge, and every key of it then reads as its default: the widest.
        Some(_) => {
            repository.remove(SMART);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(json: &str) -> BTreeMap<String, JsonValue> {
        match JsonValue::parse(json).expect("a json document") {
            JsonValue::Object(document) => document,
            other => panic!("not an object: {other:?}"),
        }
    }

    /// Whether `smart.<key>` of `repository` is still said once the document
    /// was read beside `persons`.
    fn kept(persons: &str, repository: &str, key: &str) -> bool {
        let mut repository = document(repository);
        only_narrows(&mut repository, &document(persons));
        repository
            .get(SMART)
            .and_then(JsonValue::as_object)
            .is_some_and(|smart| smart.contains_key(key))
    }

    #[test]
    fn a_repository_may_ask_less_of_the_routing_probe_and_never_more() {
        let key = "autoClassifier";
        let says = |word: &str| format!(r#"{{"smart":{{"autoClassifier":{word}}}}}"#);
        // (the person's own, the repository's, whether the repository's stays)
        for (persons, repository, stays) in [
            (r#""deterministic""#, r#""probed""#, false),
            (r#""assisted""#, r#""probed""#, false),
            (r#""off""#, r#""probed""#, false),
            (r#""off""#, r#""deterministic""#, false),
            (r#""off""#, r#""assisted""#, false),
            // A word nobody knows, another case, no string: each reads as
            // `deterministic`, which asks more than `off`.
            (r#""off""#, r#""Probed""#, false),
            (r#""off""#, "null", false),
            (r#""off""#, "7", false),
            (r#""probed""#, r#""deterministic""#, true),
            (r#""probed""#, r#""off""#, true),
            (r#""deterministic""#, r#""off""#, true),
            (r#""deterministic""#, r#""assisted""#, true),
            (r#""deterministic""#, r#""Probed""#, true),
            (r#""probed""#, r#""probed""#, true),
        ] {
            assert_eq!(kept(&says(persons), &says(repository), key), stays, "{persons} then {repository}");
        }
        // A person who says nothing runs probed: a repository cannot ask more than that.
        for repository in [r#""probed""#, r#""deterministic""#, r#""off""#] {
            assert!(kept("{}", &says(repository), key), "nothing said, then {repository}");
            assert!(kept(r#"{"smart":{"policy":"classic"}}"#, &says(repository), key));
        }
    }

    #[test]
    fn a_repository_may_switch_smart_routing_off_and_not_back_on() {
        let key = "enabled";
        let says = |value: &str| format!(r#"{{"smart":{{"enabled":{value}}}}}"#);
        for (persons, repository, stays) in [
            ("false", "true", false),
            // No boolean reads as on.
            ("false", r#""yes""#, false),
            ("false", "null", false),
            ("false", "false", true),
            ("true", "false", true),
            ("true", "true", true),
        ] {
            assert_eq!(kept(&says(persons), &says(repository), key), stays, "{persons} then {repository}");
        }
        assert!(kept("{}", &says("true"), key));
        assert!(kept("{}", &says("false"), key));
    }

    #[test]
    fn a_repository_that_says_smart_is_no_object_does_not_wipe_the_persons() {
        let persons = document(r#"{"smart":{"autoClassifier":"off"}}"#);
        for repository in [r#"{"smart":"x"}"#, r#"{"smart":null}"#, r#"{"smart":[]}"#, r#"{"smart":false}"#] {
            let mut said = document(repository);
            only_narrows(&mut said, &persons);
            assert!(!said.contains_key(SMART), "{repository} would replace the person's whole `smart`");
            // The same with nothing of the person's to wipe: it means nothing either way.
            let mut said = document(repository);
            only_narrows(&mut said, &BTreeMap::new());
            assert!(!said.contains_key(SMART), "{repository}");
        }
    }

    #[test]
    fn what_a_repository_says_of_anything_else_is_left_as_it_is() {
        let mut repository = document(
            r#"{"model":"sonnet","smart":{"autoClassifier":"probed","policy":"classic","exploration":false}}"#,
        );
        only_narrows(&mut repository, &document(r#"{"smart":{"autoClassifier":"off"}}"#));
        assert_eq!(
            repository,
            document(r#"{"model":"sonnet","smart":{"policy":"classic","exploration":false}}"#)
        );
        let untouched = document(r#"{"model":"sonnet","permissions":{"deny":["Bash(rm:*)"]}}"#);
        let mut repository = untouched.clone();
        only_narrows(&mut repository, &document(r#"{"smart":{"autoClassifier":"off"}}"#));
        assert_eq!(repository, untouched);
    }
}
