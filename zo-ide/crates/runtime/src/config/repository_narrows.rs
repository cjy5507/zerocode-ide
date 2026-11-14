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
//! The same held for three settings beside those (152). `smart.policy` other
//! than `classic` puts the verify leg and the plan stage on the connected top
//! tier, which no allowlist bounds; `cache.refresh: true` keeps an idle
//! conversation's cache warm with requests sent while the person is away;
//! and a Jev use's word — `smart.jevPatchReview: "on"`, a saved built-in
//! decision, the switch itself — had a use the person set lower ask again.
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

use zerocode_core::jev::{builtin, door, JevMode, JevUse, JEV_USES};

use super::parsers::deep_merge_objects;
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

const NARROWED: [Narrowed; 8] = [
    Narrowed { key: "enabled", asks: switch },
    Narrowed { key: "autoClassifier", asks: classifier },
    // Whether the verify leg and the plan stage go to the router's pick,
    // inside the allowlist, or to the connected top tier.
    Narrowed { key: "policy", asks: smart_policy },
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

/// `smart.policy` as its reader takes it (`SmartPolicy::from_settings_value`):
/// `classic`, trimmed and as written, is Classic; anything else — another
/// word, another case, nothing — is Architect.
fn smart_policy(said: Option<&JsonValue>) -> u8 {
    u8::from(said.and_then(JsonValue::as_str).map(str::trim) != Some("classic"))
}

/// The conversation cache's settings, beside `smart`.
const CACHE: &str = "cache";

/// `cache.refresh`: whether an idle conversation's cache is kept warm with
/// requests sent while the person is away. On unless a boolean says off.
const CACHE_REFRESH: &str = "refresh";

/// `cache.anchorTtl`: how long the conversation's cache is written to last.
/// The hour costs twice the input on every write, five minutes 1.25 times;
/// what the person's conversations cost is theirs to choose, so a
/// repository's word is not read (152, review t-107910).
const CACHE_ANCHOR_TTL: &str = "anchorTtl";

/// Takes a repository's `cache.refresh` out where it would refresh what the
/// person's own documents switched off, its `cache.anchorTtl` always, and a
/// `cache` that is no object — it would replace the person's whole `cache`
/// and read as the defaults.
fn narrow_the_cache(
    repository: &mut BTreeMap<String, JsonValue>,
    persons: &BTreeMap<String, JsonValue>,
) {
    match repository.get_mut(CACHE) {
        None => {}
        Some(JsonValue::Object(cache)) => {
            cache.remove(CACHE_ANCHOR_TTL);
            let own = switch(
                persons
                    .get(CACHE)
                    .and_then(JsonValue::as_object)
                    .and_then(|cache| cache.get(CACHE_REFRESH)),
            );
            if cache.get(CACHE_REFRESH).is_some_and(|said| switch(Some(said)) > own) {
                cache.remove(CACHE_REFRESH);
            }
        }
        Some(_) => {
            repository.remove(CACHE);
        }
    }
}

/// How far a Jev use goes in `mode`: `off` asks nothing, `shadow` asks and
/// records, `auto` asks and acts once its own evidence stands, `on` asks
/// and acts.
fn reach(mode: JevMode) -> u8 {
    match mode {
        JevMode::Off => 0,
        JevMode::Shadow => 1,
        JevMode::Auto => 2,
        JevMode::On => 3,
    }
}

/// A settings document as `zerocode_core::jev` reads one.
fn as_read(document: &BTreeMap<String, JsonValue>) -> Option<serde_json::Value> {
    serde_json::from_str(&JsonValue::Object(document.clone()).render()).ok()
}

/// Takes out of a repository's `smart` what would have a Jev use go further
/// than the person's own documents have it go: a switch turned on where
/// theirs is off (and a `jev` that is no object, which would replace
/// theirs), and for each use read further than theirs read it, the
/// repository's word for the use, for the use it was split from, and its
/// saved built-in decision. A use is read the way its reader reads a whole
/// document (`JevUse::chosen_mode_in`), so a word of the wrong kind counts
/// as what it reads as.
fn narrow_the_jev_uses(
    repository: &mut BTreeMap<String, JsonValue>,
    persons: &BTreeMap<String, JsonValue>,
) {
    let Some(JsonValue::Object(smart)) = repository.get_mut(SMART) else {
        return;
    };
    match smart.get_mut(door::JEV_SETTINGS_KEY) {
        None => {}
        Some(JsonValue::Object(jev)) => {
            let own = persons
                .get(SMART)
                .and_then(JsonValue::as_object)
                .and_then(|smart| smart.get(door::JEV_SETTINGS_KEY))
                .and_then(JsonValue::as_object)
                .and_then(|jev| jev.get(door::ENABLED_SETTING))
                .and_then(JsonValue::as_bool)
                .unwrap_or(false);
            if !own && jev.get(door::ENABLED_SETTING).and_then(JsonValue::as_bool) == Some(true) {
                jev.remove(door::ENABLED_SETTING);
            }
        }
        Some(_) => {
            smart.remove(door::JEV_SETTINGS_KEY);
        }
    }
    // A document that names no use, no saved decision and no switch reads
    // every use as the person's documents do: nothing to weigh.
    let names_a_use = smart.contains_key(door::JEV_SETTINGS_KEY)
        || smart.contains_key(builtin::SETTING)
        || JEV_USES.iter().any(|seat| smart.contains_key(seat.setting));
    if !names_a_use {
        return;
    }
    let mut merged = persons.clone();
    deep_merge_objects(&mut merged, repository);
    let further: Vec<&JevUse> = match (as_read(persons), as_read(&merged)) {
        (Some(own), Some(merged)) => JEV_USES
            .iter()
            .filter(|seat| reach(seat.chosen_mode_in(&merged)) > reach(seat.chosen_mode_in(&own)))
            .collect(),
        // A document that does not read back is not read for the person:
        // every word of the repository's for a use goes.
        _ => JEV_USES.iter().collect(),
    };
    let Some(JsonValue::Object(smart)) = repository.get_mut(SMART) else {
        return;
    };
    for seat in further {
        smart.remove(seat.setting);
        if let Some(follows) = seat.follows {
            smart.remove(follows);
        }
        if let Some(JsonValue::Object(saved)) = smart.get_mut(builtin::SETTING) {
            saved.remove(seat.id);
        }
    }
}

fn classifier(said: Option<&JsonValue>) -> u8 {
    said.map_or(RouteAutoClassifierMode::UNSAID, |said| {
        RouteAutoClassifierMode::from_settings_word(said.as_str())
    })
    .asks()
}

/// Takes out of a repository's document every `smart` key, Jev word and
/// `cache` key that would have the session ask more, or send to a provider
/// it does not send to, than `persons` — the person's own documents, merged —
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
            narrow_the_allowlist(smart, persons);
            // Which provider Jev's judgments come from is read off the
            // person's own settings when a request is sent; the merged
            // document's word only says whether its probabilities can carry
            // an act (`zerocode_core::jev::calibrated_in`). A repository's
            // word there would have a use that only records act (152, review
            // t-107910).
            smart.remove(judgment_provider::SETTINGS_KEY);
        }
        // Anything but an object replaces the person's whole `smart` in the
        // merge, and every key of it then reads as its default: the widest.
        Some(_) => {
            repository.remove(SMART);
        }
    }
    narrow_the_jev_uses(repository, persons);
    narrow_the_cache(repository, persons);
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

    #[test]
    fn a_repository_may_keep_the_verify_leg_to_the_router_and_not_send_it_to_the_top_tier() {
        let says = |word: &str| format!(r#"{{"smart":{{"policy":{word}}}}}"#);
        // (the person's own, the repository's, whether the repository's stays)
        for (persons, repository, stays) in [
            (r#""classic""#, r#""architect""#, false),
            (r#""classic""#, r#""Classic""#, false),
            (r#""classic""#, "null", false),
            (r#""classic""#, "7", false),
            (r#""classic""#, r#"" classic ""#, true),
            (r#""architect""#, r#""classic""#, true),
            (r#""architect""#, r#""architect""#, true),
        ] {
            assert_eq!(kept(&says(persons), &says(repository), "policy"), stays, "{persons} / {repository}");
        }
        // A person who said nothing is on Architect already.
        assert!(kept("{}", &says(r#""architect""#), "policy"));
    }

    fn cache_after(persons: &str, repository: &str) -> Option<BTreeMap<String, JsonValue>> {
        let mut repository = document(repository);
        only_narrows(&mut repository, &document(persons));
        repository.get(CACHE).and_then(JsonValue::as_object).cloned()
    }

    #[test]
    fn a_repository_may_stop_an_idle_conversations_refreshes_and_not_start_them() {
        let off = r#"{"cache":{"refresh":false}}"#;
        for (persons, repository, stays) in [
            (off, r#"{"cache":{"refresh":true}}"#, false),
            (off, r#"{"cache":{"refresh":"no"}}"#, false),
            (off, r#"{"cache":{"refresh":false}}"#, true),
            ("{}", r#"{"cache":{"refresh":false}}"#, true),
            ("{}", r#"{"cache":{"refresh":true}}"#, true),
        ] {
            let cache = cache_after(persons, repository).unwrap_or_default();
            assert_eq!(cache.contains_key(CACHE_REFRESH), stays, "{persons} / {repository}");
        }
        // The cache's lifetime is what each write costs: the person's to choose.
        for persons in [off, "{}", r#"{"cache":{"anchorTtl":"5m"}}"#] {
            let cache = cache_after(persons, r#"{"cache":{"refresh":false,"anchorTtl":"1h"}}"#).expect("the cache stays");
            assert!(!cache.contains_key(CACHE_ANCHOR_TTL), "{persons}: {cache:?}");
            assert!(cache.contains_key(CACHE_REFRESH), "switching refreshes off may stand");
        }
        for repository in [r#"{"cache":"x"}"#, r#"{"cache":null}"#, r#"{"cache":[]}"#] {
            let mut said = document(repository);
            only_narrows(&mut said, &document(off));
            assert!(!said.contains_key(CACHE), "{repository} would replace the person's whole `cache`");
        }
    }

    /// A Jev use that offers `off`, `shadow` and `on`, and follows no other.
    fn a_use_of_every_reach() -> &'static JevUse {
        JEV_USES
            .iter()
            .find(|seat| {
                seat.follows.is_none()
                    && [JevMode::Off, JevMode::Shadow, JevMode::On].iter().all(|mode| seat.modes.contains(mode))
            })
            .expect("a use offering off, shadow and on")
    }

    /// The repository's `smart` once read beside the person's documents.
    fn smart_after(persons: &serde_json::Value, repository: &serde_json::Value) -> BTreeMap<String, JsonValue> {
        let mut repository = document(&repository.to_string());
        only_narrows(&mut repository, &document(&persons.to_string()));
        repository
            .get(SMART)
            .and_then(JsonValue::as_object)
            .cloned()
            .unwrap_or_default()
    }

    #[test]
    fn a_repository_may_set_a_jev_use_lower_and_never_further() {
        let seat = a_use_of_every_reach();
        let switched_on = |word: Option<JevMode>| {
            let mut smart = serde_json::json!({ "jev": { "enabled": true } });
            if let Some(mode) = word {
                smart[seat.setting] = serde_json::json!(mode.key());
            }
            serde_json::json!({ "smart": smart })
        };
        let says = |mode: JevMode| serde_json::json!({ "smart": { seat.setting: mode.key() } });
        // (the person's own word, the repository's, whether the repository's stays)
        for (own, theirs, stays) in [
            (Some(JevMode::Off), JevMode::On, false),
            (Some(JevMode::Off), JevMode::Shadow, false),
            (Some(JevMode::Shadow), JevMode::On, false),
            (Some(JevMode::On), JevMode::Shadow, true),
            (Some(JevMode::On), JevMode::Off, true),
            (Some(JevMode::Off), JevMode::Off, true),
        ] {
            let smart = smart_after(&switched_on(own), &says(theirs));
            assert_eq!(smart.contains_key(seat.setting), stays, "{} {own:?} / {theirs:?}", seat.id);
        }
        // Nothing of the person's: Jev is off, and every use with it.
        assert!(!smart_after(&serde_json::json!({}), &says(JevMode::Shadow)).contains_key(seat.setting));
        // A word the use does not offer reads as `off`, and may stand.
        let odd = serde_json::json!({ "smart": { seat.setting: "louder" } });
        assert!(smart_after(&serde_json::json!({}), &odd).contains_key(seat.setting));
    }

    #[test]
    fn a_repository_does_not_switch_jev_on_for_a_person_who_has_it_off() {
        let on = serde_json::json!({ "smart": { "jev": { "enabled": true, "dailyLimit": 3 } } });
        let smart = smart_after(&serde_json::json!({}), &on);
        let jev = smart.get("jev").and_then(JsonValue::as_object).expect("the rest of `jev` stays");
        assert!(!jev.contains_key("enabled"), "{jev:?}");
        // Off, it may say.
        let off = serde_json::json!({ "smart": { "jev": { "enabled": false } } });
        let persons = serde_json::json!({ "smart": { "jev": { "enabled": true } } });
        let jev = smart_after(&persons, &off).get("jev").and_then(JsonValue::as_object).cloned().expect("jev");
        assert_eq!(jev.get("enabled").and_then(JsonValue::as_bool), Some(false));
        for odd in [serde_json::json!("x"), serde_json::json!(null), serde_json::json!([])] {
            let smart = smart_after(&persons, &serde_json::json!({ "smart": { "jev": odd } }));
            assert!(!smart.contains_key("jev"), "{odd} would replace the person's whole `jev`");
        }
    }

    #[test]
    fn a_repositorys_saved_decision_or_older_word_does_not_take_a_use_further() {
        let off = serde_json::json!({ "smart": { "jev": { "enabled": true } } });
        // A use a saved built-in decision can set: the first one that reads
        // `on` from a decision saying so.
        let decided = JEV_USES
            .iter()
            .find(|seat| {
                let root = serde_json::json!({ "smart": { builtin::SETTING: { seat.id: { "mode": "on" } } } });
                seat.chosen_mode_in(&root) == JevMode::On
            })
            .expect("a use a saved decision sets");
        let mut persons = off.clone();
        persons["smart"][decided.setting] = serde_json::json!("off");
        let repository = serde_json::json!({ "smart": { builtin::SETTING: { decided.id: { "mode": "on" } } } });
        let smart = smart_after(&persons, &repository);
        let saved = smart.get(builtin::SETTING).and_then(JsonValue::as_object).cloned().unwrap_or_default();
        assert!(!saved.contains_key(decided.id), "{} {saved:?}", decided.id);

        // A use split from another reads the older use's word while it has
        // none of its own: the repository's older word is taken out with it.
        let split = JEV_USES.iter().find(|seat| seat.follows.is_some()).expect("a use split from another");
        let older = split.follows.expect("its older use");
        let mut persons = off;
        persons["smart"][older] = serde_json::json!("off");
        let repository = serde_json::json!({ "smart": { older: "on" } });
        assert!(!smart_after(&persons, &repository).contains_key(older), "{} follows {older}", split.id);
    }

    /// Which provider Jev's judgments come from is the person's: a
    /// repository's `smart.judgmentProvider` would have the merged document
    /// read its probabilities as able to carry an act, and a use the
    /// person's provider keeps recording would act (152, review t-107910).
    #[test]
    fn a_repository_does_not_name_the_judgment_provider() {
        let persons = r#"{"smart":{"judgmentProvider":{"protocol":"openai_json"}}}"#;
        for repository in [
            r#"{"smart":{"judgmentProvider":{"protocol":"system_one"}}}"#,
            r#"{"smart":{"judgmentProvider":"x"}}"#,
        ] {
            assert!(!kept(persons, repository, "judgmentProvider"), "{repository}");
            assert!(!kept("{}", repository, "judgmentProvider"), "{repository}");
        }
    }
}
