//! A repository's settings document may narrow what a session asks outside
//! itself and where its requests may go, and never widen either.
//!
//! `<workspace>/.zo/settings.json` is committed with the repository: whoever
//! opens the folder runs under it without having read it. Merged after the
//! person's own settings, it had the last word on `smart`. A person who wrote
//! `"autoClassifier": "deterministic"` so that no model is asked about their
//! tasks had the question asked again in any repository that said `"probed"`
//! (measured on v1.1.150). One who switched `verifyCrossProvider` or
//! `quotaFallback` off, so that the verify leg and a turn out of quota stay
//! with the session's provider, had them sent to another by a repository's
//! `true`; `allowCrossProviderDiversity`, which has the checking roles
//! prefer another provider, was switched back on the same way, and so was
//! `classifierFallback: "auto"`, which moves a refused turn to another model
//! without asking, and `orchestration: "auto"`, which has the host spawn
//! helpers of its own; and a `providerAllowlist` of theirs was replaced by
//! the repository's — by `[]`, which is every provider.
//!
//! So what such a document says is kept only where it asks no more, and
//! sends nowhere else, than the person's own documents do. A team can still
//! switch the routing probe off for its repository, or name the providers
//! its routes may go to; it cannot switch back on what a person switched
//! off, nor add a provider to the ones they named.
//!
//! The person's own documents are the ones the loader already takes the
//! `enableAllProject…` opt-ins from: the user settings, a `.zo/settings.local.json`
//! that is a real file git does not track, and a `--settings` file. A local
//! file the repository committed is read as the repository's.

use std::collections::BTreeMap;

use crate::json::JsonValue;
use crate::model_router::RouteAutoClassifierMode;
use crate::ClassifierFallback;

const SMART: &str = "smart";

/// One `smart` key a repository's document may only narrow: its name, and how
/// much a document that says `said` — or does not say the key — has the
/// session ask. The reading is the one the key's reader makes of that value
/// alone, so a value of the wrong type counts as the default it reads as.
struct Narrowed {
    key: &'static str,
    asks: fn(Option<&JsonValue>) -> u8,
}

const NARROWED: [Narrowed; 7] = [
    Narrowed { key: "enabled", asks: switch },
    Narrowed { key: "autoClassifier", asks: classifier },
    // Whether a turn the provider refused goes on at another model: never,
    // after the person said yes, or at once.
    Narrowed { key: "classifierFallback", asks: refusal_fallback },
    // Whether the host spawns helpers of its own before a broad turn.
    Narrowed { key: "orchestration", asks: host_orchestration },
    // Whether the checking roles prefer a provider other than the session's,
    // whether the verify leg may be on one, and whether a turn that ran out
    // of quota goes on at one.
    Narrowed { key: "allowCrossProviderDiversity", asks: switch },
    Narrowed { key: "verifyCrossProvider", asks: switch },
    Narrowed { key: "quotaFallback", asks: switch },
];

/// The providers a route may go to. A list that names none is every provider.
const ALLOWLIST: &str = "providerAllowlist";

/// A switch that is on unless a boolean says off.
fn switch(said: Option<&JsonValue>) -> u8 {
    u8::from(said.and_then(JsonValue::as_bool).unwrap_or(true))
}

/// The provider names a `providerAllowlist` reads as: the trimmed strings of
/// an array that are not empty, and none for anything else.
fn allowlist(said: Option<&JsonValue>) -> Vec<&str> {
    said.and_then(JsonValue::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(JsonValue::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// The repository's `providerAllowlist` cut down to the providers the
/// person's own list names. Where the two share none — and where the
/// repository's names none, which is every provider — it is taken out, and
/// the person's list stands. A person who named none has allowed every
/// provider, and the repository's list narrows that as it is.
fn narrow_the_allowlist(
    smart: &mut BTreeMap<String, JsonValue>,
    persons: Option<&BTreeMap<String, JsonValue>>,
) {
    let own = allowlist(persons.and_then(|smart| smart.get(ALLOWLIST)));
    if own.is_empty() {
        return;
    }
    let Some(said) = smart.get(ALLOWLIST) else {
        return;
    };
    let both: Vec<JsonValue> = allowlist(Some(said))
        .into_iter()
        .filter(|name| own.iter().any(|allowed| allowed.eq_ignore_ascii_case(name)))
        .map(|name| JsonValue::String(name.to_string()))
        .collect();
    if both.is_empty() {
        smart.remove(ALLOWLIST);
    } else {
        smart.insert(ALLOWLIST.to_string(), JsonValue::Array(both));
    }
}

/// `smart.classifierFallback` as its reader takes it: a word of the three in
/// any case, and anything else — another word, no string — the default, `ask`.
fn refusal_fallback(said: Option<&JsonValue>) -> u8 {
    match said.and_then(JsonValue::as_str).and_then(ClassifierFallback::from_word).unwrap_or_default() {
        ClassifierFallback::Off => 0,
        ClassifierFallback::Ask => 1,
        ClassifierFallback::Auto => 2,
    }
}

/// `smart.orchestration` as its reader takes it (`tools` `HostOrchestration`):
/// `model` and `off` in any case keep the host from spawning; anything else —
/// `auto`, another word, nothing — lets it.
fn host_orchestration(said: Option<&JsonValue>) -> u8 {
    let keeps_out = said
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .is_some_and(|word| word.eq_ignore_ascii_case("model") || word.eq_ignore_ascii_case("off"));
    u8::from(!keeps_out)
}

fn classifier(said: Option<&JsonValue>) -> u8 {
    said.map_or(RouteAutoClassifierMode::UNSAID, |said| {
        RouteAutoClassifierMode::from_settings_word(said.as_str())
    })
    .asks()
}

/// Takes out of a repository's document every `smart` key that would have the
/// session ask more, or send to a provider it does not send to, than
/// `persons` — the person's own documents, merged — says it does.
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
            narrow_the_allowlist(smart, persons);
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
    fn a_repository_may_switch_off_what_sends_to_another_provider_and_not_back_on() {
        for key in ["allowCrossProviderDiversity", "verifyCrossProvider", "quotaFallback"] {
            let says = |value: &str| format!(r#"{{"smart":{{"{key}":{value}}}}}"#);
            for (persons, repository, stays) in [
                ("false", "true", false),
                // No boolean reads as on.
                ("false", r#""no""#, false),
                ("false", "null", false),
                ("false", "0", false),
                ("false", "false", true),
                ("true", "false", true),
                ("true", "true", true),
            ] {
                assert_eq!(kept(&says(persons), &says(repository), key), stays, "{key}: {persons} then {repository}");
            }
            assert!(kept("{}", &says("true"), key), "{key}: nothing said is on");
            assert!(kept("{}", &says("false"), key), "{key}");
        }
    }

    #[test]
    fn a_repository_may_not_move_a_refused_turn_to_another_model_where_the_person_would_be_asked() {
        let key = "classifierFallback";
        let says = |word: &str| format!(r#"{{"smart":{{"classifierFallback":{word}}}}}"#);
        for (persons, repository, stays) in [
            (r#""off""#, r#""auto""#, false),
            (r#""off""#, r#""ask""#, false),
            (r#""ask""#, r#""auto""#, false),
            // Read as its reader reads it: any case, and the default for the rest.
            (r#""ask""#, r#"" AUTO ""#, false),
            (r#""off""#, r#""sometimes""#, false),
            (r#""off""#, "true", false),
            (r#""ask""#, r#""sometimes""#, true),
            (r#""auto""#, r#""auto""#, true),
            (r#""auto""#, r#""off""#, true),
            (r#""ask""#, r#""off""#, true),
        ] {
            assert_eq!(kept(&says(persons), &says(repository), key), stays, "{persons} then {repository}");
        }
        // A person who said nothing is asked: a repository cannot answer for them.
        assert!(!kept("{}", &says(r#""auto""#), key));
        assert!(kept("{}", &says(r#""ask""#), key));
        assert!(kept("{}", &says(r#""off""#), key));
    }

    #[test]
    fn a_repository_may_keep_the_host_from_spawning_and_not_let_it_spawn() {
        let key = "orchestration";
        let says = |word: &str| format!(r#"{{"smart":{{"orchestration":{word}}}}}"#);
        for (persons, repository, stays) in [
            (r#""model""#, r#""auto""#, false),
            (r#""off""#, r#""auto""#, false),
            // Another word, no string: the reader's `auto`.
            (r#""model""#, r#""sometimes""#, false),
            (r#""off""#, "null", false),
            (r#""model""#, r#""OFF""#, true),
            (r#""model""#, r#"" Model ""#, true),
            (r#""auto""#, r#""auto""#, true),
            (r#""auto""#, r#""model""#, true),
        ] {
            assert_eq!(kept(&says(persons), &says(repository), key), stays, "{persons} then {repository}");
        }
        assert!(kept("{}", &says(r#""auto""#), key), "nothing said is auto");
        assert!(kept("{}", &says(r#""model""#), key));
    }

    /// What `repository` says of `smart.providerAllowlist` once it was read
    /// beside `persons`; `None` where it no longer says it.
    fn allowed(persons: &str, repository: &str) -> Option<Vec<String>> {
        let says = |list: &str| document(&format!(r#"{{"smart":{{"providerAllowlist":{list}}}}}"#));
        let mut repository = says(repository);
        only_narrows(&mut repository, &says(persons));
        let smart = repository.get(SMART).and_then(JsonValue::as_object).expect("an object stays");
        smart.get(ALLOWLIST).map(|said| allowlist(Some(said)).into_iter().map(str::to_string).collect())
    }

    #[test]
    fn a_repository_may_cut_the_persons_allowlist_down_and_not_add_to_it() {
        let names = |names: &[&str]| Some(names.iter().map(ToString::to_string).collect::<Vec<_>>());
        // Every provider is what an empty list means: it would wipe the person's.
        assert_eq!(allowed(r#"["anthropic"]"#, "[]"), None);
        assert_eq!(allowed(r#"["anthropic"]"#, "null"), None);
        assert_eq!(allowed(r#"["anthropic"]"#, r#""openai""#), None);
        assert_eq!(allowed(r#"["anthropic"]"#, r#"["", "  "]"#), None);
        // A provider the person did not name is not added.
        assert_eq!(allowed(r#"["anthropic"]"#, r#"["openai"]"#), None);
        assert_eq!(allowed(r#"["anthropic"]"#, r#"["anthropic","openai"]"#), names(&["anthropic"]));
        assert_eq!(allowed(r#"["anthropic","google"]"#, r#"["openai"," Google "]"#), names(&["Google"]));
        // Fewer of the person's is the repository's to say.
        assert_eq!(allowed(r#"["anthropic","google"]"#, r#"["google"]"#), names(&["google"]));
        // A person who named none allowed every provider: any list narrows that.
        for unsaid in ["[]", "null", r#"[""]"#] {
            assert_eq!(allowed(unsaid, r#"["openai"]"#), names(&["openai"]), "{unsaid}");
            assert_eq!(allowed(unsaid, "[]"), names(&[]), "{unsaid}");
        }
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
