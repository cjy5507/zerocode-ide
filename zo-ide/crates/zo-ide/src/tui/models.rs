//! `/model` 피커가 보여 줄 모델 목록 — 카탈로그에서 뽑고, 자격증명으로 거른다.
//!
//! 사실은 하나도 여기서 만들지 않는다. 어떤 모델이 있는지는
//! `api::provider_catalog()`, 그 모델이 무엇에 좋은지는
//! `api::declared_model_class()`, 컨텍스트 창은 `api::context_window_for_model()`,
//! 로컬 모델의 메모리 감은 `api::fit_hint_for_model()` 이 안다. 이 파일은 그
//! 사실들을 캡처의 피커 문법(`1. <id> (current)   <한 줄>`)으로 옮길 뿐이다.
//!
//! 거르는 기준은 `api::provider_usable_for_smart_inventory(kind)` 다 — Smart
//! 라우터가 "실제로 쓸 수 있는 모델"을 셀 때 쓰는 바로 그 술어라, anthropic 은
//! 언제나 참이고 openai 는 `CODEX_HOME/auth.json` 로그인(또는 zo 자신의
//! ChatGPT OAuth·API 키)이 있을 때만 참이다. 그 판단이 이미 `api` 안에 있으므로
//! tui 는 새 pub 표면을 요구하지 않는다.
//!
//! 구글(Antigravity) 로그인으로 가는 Gemini 줄은 그 계정의 레지스트리가 정한다
//! (t-71998, `runtime::model_discovery::antigravity_reach_in`): 레지스트리가
//! 내주는 판은 그 표시 이름으로 서고, 다른 이름으로 내주는 출하 id 는 지금
//! 모델일 때만 흐리게 서서 Enter 가 그 판을 고르며, 아무것도 내주지 않는 id 는
//! 흐리게 서서 Enter 가 고르지 않는다. 키로 가는 줄과 다른 공급자의 줄은 그대로다.

use api::ModelClass;
use runtime::model_discovery::{AntigravityList, AntigravityReach, DiscoveredCatalog};

/// 줄에서 Enter 가 하는 일(t-71998).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    /// 그 줄의 id 를 고른다.
    Takes,
    /// 이 id 를 고른다 — 구글 로그인의 레지스트리가 그 줄의 판을 이 이름으로
    /// 내준다. 저장된 선택은 제 이름 그대로 두고, 새로 고를 때만 이 이름이다.
    LandsOn(String),
    /// 아무것도 고르지 않는다 — 로그인이 가진 어느 목록도 내주지 않는다.
    Refused,
}

/// 피커 한 줄.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelChoice {
    /// 확정될 때 `set_model` 에 넘길 값 — 정규 모델 id.
    pub id: String,
    /// 오른쪽 한 줄 설명.
    pub description: String,
    /// 출처가 이 모델을 목록에서 뺀 시각(unix 초) — 세션이 고른 모델은
    /// 사라지지 않고 흐리게 남는다(t-3054). 고르면 여전히 통한다; 답은 와이어가
    /// 정한다.
    pub unlisted_since: Option<u64>,
    /// Enter 가 하는 일. [`Pick::Takes`] 가 아닌 줄은 흐리고, 까닭은
    /// `description` 이 말한다.
    pub pick: Pick,
}

/// 자격증명이 있는 provider 의 모델들. 카탈로그 순서(anthropic → openai → …)를
/// 지키고, 같은 정규 id 를 가리키는 별칭들은 한 줄로 합친다. `current` 는 지금
/// 모델이다.
#[must_use]
pub fn choices(current: &str) -> Vec<ModelChoice> {
    let discovered = runtime::model_discovery::current();
    let catalog = runtime::model_catalog::ModelCatalog::load().ok();
    let route = |id: &str| {
        catalog
            .as_ref()
            .and_then(|catalog| catalog.auth_route_for_model(id))
            .unwrap_or(api::AuthRoute::Auto)
    };
    choices_with(current, discovered.as_deref(), login_list(), &route)
}

/// 피커와 상태 줄이 읽는 구글 로그인의 목록. `modelUpdatePolicy` 가 `pinned`
/// 면 발견한 것을 하나도 게시하지 않으므로 목록도 없다(t-71998).
fn login_list() -> AntigravityList {
    if runtime::model_discovery::UpdatePolicy::load() == runtime::model_discovery::UpdatePolicy::Pinned {
        AntigravityList::NoLogin
    } else {
        runtime::model_discovery::antigravity_list()
    }
}

/// [`choices`] 를 주어진 발견 사진·구글 로그인 목록·줄마다의 길 위에서 —
/// 시험이 모는 자리.
pub(crate) fn choices_with(
    current: &str,
    discovered: Option<&DiscoveredCatalog>,
    list: AntigravityList,
    route: &dyn Fn(&str) -> api::AuthRoute,
) -> Vec<ModelChoice> {
    let mut out: Vec<ModelChoice> = Vec::new();
    for entry in api::provider_catalog() {
        if !api::provider_usable_for_smart_inventory(entry.provider) {
            continue;
        }
        if out
            .iter()
            .any(|choice| choice.id.eq_ignore_ascii_case(entry.canonical_model_id))
        {
            continue;
        }
        let mut choice = ModelChoice {
            id: entry.canonical_model_id.to_string(),
            description: describe(entry),
            unlisted_since: discovered
                .and_then(|catalog| runtime::model_discovery::unlisted_since_in(catalog, entry.canonical_model_id)),
            pick: Pick::Takes,
        };
        // 키로 가는 줄(키 목록에만 있는 발견 줄, 키로 정한 줄)은 로그인의 판단 밖이다.
        let over_the_login = entry.provider == api::ProviderKind::Google
            && list != AntigravityList::NoLogin
            && route(entry.canonical_model_id) != api::AuthRoute::ApiKey;
        if over_the_login && !judge_gemini(&mut choice, current, discovered, list) {
            continue;
        }
        out.push(choice);
    }
    // The providers the person connected — in the window's settings (API
    // 라우터) or through `/connect` — after the built-ins, each model as the
    // explicit `<provider>/<model>` so the pick routes to that provider even
    // when the bare id collides with a built-in. Only the usable ones: a
    // provider whose key zo cannot see would answer every pick with a 401.
    for (provider, models) in api::custom_provider_usable_catalog() {
        for model in models {
            let id = api::format_provider_model_ref(provider, &model);
            if out.iter().any(|choice| choice.id.eq_ignore_ascii_case(&id)) {
                continue;
            }
            let description = describe_custom(provider, &id);
            out.push(ModelChoice {
                id,
                description,
                unlisted_since: None,
                pick: Pick::Takes,
            });
        }
    }
    out
}

/// 구글 로그인으로 가는 Gemini 줄에 로그인의 말을 적는다(t-71998). 레지스트리가
/// 내주는 판은 그 표시 이름을 앞에, 다른 이름으로 내주는 출하 id 는 지금
/// 모델일 때만 그 판으로 가는 줄로, 아무것도 내주지 않는 id 는 고를 수 없는
/// 줄로. 목록을 확인 못 했으면 그 말과 그 목록을 읽은 때를 뒤에 붙인다.
/// `false` 면 그 줄은 서지 않는다.
fn judge_gemini(
    choice: &mut ModelChoice,
    current: &str,
    discovered: Option<&DiscoveredCatalog>,
    list: AntigravityList,
) -> bool {
    let reach = discovered.map_or(AntigravityReach::Unjudged, |catalog| {
        runtime::model_discovery::antigravity_reach_in(catalog, &choice.id)
    });
    match reach {
        AntigravityReach::Listed => {
            choice.description = format!("{} · {}", api::model_display_name(&choice.id), choice.description);
        }
        AntigravityReach::Renamed(release) => {
            if !choice.id.eq_ignore_ascii_case(current) {
                return false;
            }
            choice.description = super::strings::antigravity_renamed(&release);
            choice.unlisted_since = None;
            choice.pick = Pick::LandsOn(release);
        }
        AntigravityReach::Unserved { .. } => {
            choice.description = super::strings::ANTIGRAVITY_UNLISTED.to_string();
            choice.unlisted_since = None;
            choice.pick = Pick::Refused;
        }
        AntigravityReach::KeyOnly | AntigravityReach::Unjudged => {}
    }
    if let Some(mark) = unconfirmed_mark(list) {
        choice.description = format!("{} · {mark}", choice.description);
    }
    true
}

/// 「Antigravity 목록 확인 못 함 · <읽은 때> 목록」 — 레지스트리를 못 읽었을 때만.
fn unconfirmed_mark(list: AntigravityList) -> Option<String> {
    match list {
        AntigravityList::Unconfirmed(read_at) => {
            Some(super::strings::antigravity_unconfirmed(read_at.map(local_clock).as_deref()))
        }
        AntigravityList::NoLogin | AntigravityList::Read(_) => None,
    }
}

/// `name` 의 정규 id — 구글 로그인이 판단하는 이름, 곧 공급자를 앞에 적지 않은
/// Gemini id 일 때만. 자격증명을 읽지 않는 판별로 묻는다: 상태 줄은 모델이
/// 바뀔 때마다 이것을 묻는다.
fn gemini_canonical(name: &str) -> Option<String> {
    let canonical = api::resolve_catalog_alias(name);
    let google = api::explicit_non_claude_provider_kind(&canonical) == Some(api::ProviderKind::Google);
    (!canonical.contains('/') && google).then_some(canonical)
}

/// 상태 줄이 모델 옆에 적는 구글 로그인의 말(t-71998) — 고른 모델이 로그인으로
/// 갈 때만. 다른 공급자의 모델, 키로 가는 Gemini 에는 `None` 이다.
#[must_use]
pub fn login_note(model: &str) -> Option<String> {
    gemini_canonical(model)?;
    let discovered = runtime::model_discovery::current();
    login_note_with(
        model,
        discovered.as_deref(),
        login_list(),
        crate::runtime_support::catalog_auth_route_for_model(model),
    )
}

/// [`login_note`] 를 주어진 사진·목록·길 위에서.
pub(crate) fn login_note_with(
    model: &str,
    discovered: Option<&DiscoveredCatalog>,
    list: AntigravityList,
    route: api::AuthRoute,
) -> Option<String> {
    let canonical = gemini_canonical(model)?;
    if route == api::AuthRoute::ApiKey || list == AntigravityList::NoLogin {
        return None;
    }
    let word = match discovered.map(|catalog| runtime::model_discovery::antigravity_reach_in(catalog, &canonical)) {
        Some(AntigravityReach::Renamed(release)) => Some(super::strings::antigravity_renamed(&release)),
        Some(AntigravityReach::Unserved { .. }) => Some(super::strings::ANTIGRAVITY_UNLISTED.to_string()),
        _ => None,
    };
    match (word, unconfirmed_mark(list)) {
        (Some(word), Some(mark)) => Some(format!("{word} · {mark}")),
        (word, mark) => word.or(mark),
    }
}

/// `/model <이름>` 이 고르는 것(t-71998): 친 이름 그대로, 또는 구글 로그인의
/// 레지스트리가 그 판을 내주는 이름. 로그인이 가진 어느 목록도 내주지 않는
/// Gemini 면 거절하는 말이다. 저장된 선택은 건드리지 않는다 — 새로 고를 때만이다.
pub fn typed_pick(name: &str) -> Result<String, String> {
    if gemini_canonical(name).is_none() {
        return Ok(name.to_string());
    }
    let discovered = runtime::model_discovery::current();
    typed_pick_with(
        name,
        discovered.as_deref(),
        login_list(),
        crate::runtime_support::catalog_auth_route_for_model(name),
    )
}

/// [`typed_pick`] 를 주어진 사진·목록·길 위에서.
pub(crate) fn typed_pick_with(
    name: &str,
    discovered: Option<&DiscoveredCatalog>,
    list: AntigravityList,
    route: api::AuthRoute,
) -> Result<String, String> {
    let Some(canonical) = gemini_canonical(name) else {
        return Ok(name.to_string());
    };
    if route == api::AuthRoute::ApiKey || list == AntigravityList::NoLogin {
        return Ok(name.to_string());
    }
    match discovered.map(|catalog| runtime::model_discovery::antigravity_reach_in(catalog, &canonical)) {
        Some(AntigravityReach::Renamed(release)) => Ok(release),
        Some(AntigravityReach::Unserved { .. }) => Err(format!("{canonical}: {}", super::strings::ANTIGRAVITY_UNLISTED)),
        _ => Ok(name.to_string()),
    }
}

/// A connected provider's row: whose it is, and the context window the
/// provider declared (or the default the catalog falls back to).
fn describe_custom(provider: &str, id: &str) -> String {
    format!(
        "{provider} · {} context",
        context_label(api::context_window_for_model(id))
    )
}

/// `MM-DD HH:MM` in the person's own zone — the clock a picker row shows
/// for when its source stopped listing it.
#[must_use]
pub fn local_clock(unix_secs: u64) -> String {
    local_clock_at(unix_secs, core_types::date::local_utc_offset_secs())
}

fn local_clock_at(unix_secs: u64, offset_secs: i64) -> String {
    let local = i64::try_from(unix_secs).unwrap_or(i64::MAX).saturating_add(offset_secs);
    let days = local.div_euclid(86_400);
    let rem = local.rem_euclid(86_400);
    let (_, month, day) = core_types::date::civil_from_unix_days(days);
    format!("{month:02}-{day:02} {:02}:{:02}", rem / 3600, (rem % 3600) / 60)
}

/// 카탈로그가 선언한 것만으로 쓰는 한 줄. 문안은 우리 것이고, 어떤 문안을
/// 고를지는 카탈로그가 정한다.
fn describe(entry: &api::ProviderCatalogEntry) -> String {
    let mut text = if entry.orchestration_rank.is_some() {
        "Reasoning-first — plans and verifies".to_string()
    } else {
        match api::declared_model_class(entry.canonical_model_id) {
            Some(ModelClass::Frontier) => "Frontier model for complex work".to_string(),
            Some(ModelClass::Balanced) => "Balanced model for everyday work".to_string(),
            Some(ModelClass::Fast) => "Fast, affordable model".to_string(),
            None => "General-purpose model".to_string(),
        }
    };
    text.push_str(" · ");
    text.push_str(&context_label(api::context_window_for_model(
        entry.canonical_model_id,
    )));
    text.push_str(" context");
    if let Some(hint) = api::fit_hint_for_model(entry.canonical_model_id) {
        text.push_str(" · ");
        text.push_str(&hint.display_label());
    }
    text
}

/// `1_000_000 → 1M`, `258_000 → 258k`. 창 크기는 사람이 읽는 자리에만 쓴다.
fn context_label(window: u64) -> String {
    if window >= 1_000_000 && window.is_multiple_of(1_000_000) {
        return format!("{}M", window / 1_000_000);
    }
    if window >= 1_000 {
        return format!("{}k", window / 1_000);
    }
    window.to_string()
}

#[cfg(test)]
mod tests {
    use super::{choices, context_label, local_clock_at};

    /// A provider connected in the window's settings (or through `/connect`)
    /// shows in the picker as `<provider>/<model>` beside the built-ins; one
    /// whose key zo cannot see does not — it would answer every pick with a 401.
    #[test]
    fn connected_providers_models_show_in_the_picker_as_explicit_refs() {
        let _lock = crate::support::test_env_lock();
        let prior = std::env::var_os(api::CUSTOM_PROVIDERS_ENV);
        std::env::remove_var(api::CUSTOM_PROVIDERS_ENV);
        api::refresh_custom_providers_from_json(
            r#"[{"name":"stubrouter","base_url":"http://127.0.0.1:9/v1",
                 "models":["stub/router-model-a","routermodel-b"],"requires_auth":false,
                 "context_window":128000},
                {"name":"lockedrouter","base_url":"http://127.0.0.1:9/v1",
                 "models":["vendor/locked"],"requires_auth":true,
                 "auth_env":"ZO_TEST_LOCKED_ROUTER_KEY_THAT_IS_NEVER_SET"}]"#,
        )
        .expect("custom providers");
        let rows = choices("");
        let row = |id: &str| rows.iter().find(|choice| choice.id == id);
        let slashed = row("stubrouter/stub/router-model-a").expect("the slashed router model is listed");
        assert!(
            slashed.description.starts_with("stubrouter · ") && slashed.description.contains("context"),
            "{}",
            slashed.description
        );
        assert!(row("stubrouter/routermodel-b").is_some());
        assert!(
            row("lockedrouter/vendor/locked").is_none(),
            "a provider without a visible key was offered"
        );
        let first_custom = rows
            .iter()
            .position(|choice| choice.id.starts_with("stubrouter/"))
            .expect("custom rows");
        assert!(
            rows[..first_custom].iter().all(|choice| !choice.id.contains("stubrouter")),
            "connected providers come after the built-ins"
        );
        api::refresh_custom_providers_from_json("[]").expect("clear");
        match prior {
            Some(value) => std::env::set_var(api::CUSTOM_PROVIDERS_ENV, value),
            None => std::env::remove_var(api::CUSTOM_PROVIDERS_ENV),
        }
    }

    /// The unlisted clock is local: 2026-09-07T14:25:08Z is 23:25 in Seoul.
    #[test]
    fn the_unlisted_clock_reads_in_the_persons_zone() {
        assert_eq!(local_clock_at(1_788_791_108, 9 * 3600), "09-07 23:25");
        assert_eq!(local_clock_at(1_788_791_108, 0), "09-07 14:25");
        assert_eq!(local_clock_at(1_788_791_108, -10 * 3600), "09-07 04:25");
        assert_eq!(local_clock_at(1_788_791_108, 12 * 3600), "09-08 02:25", "an offset can cross midnight");
    }

    #[test]
    fn context_labels_read_the_way_a_person_would_say_them() {
        assert_eq!(context_label(1_000_000), "1M");
        assert_eq!(context_label(258_000), "258k");
        assert_eq!(context_label(900), "900");
    }

    /// anthropic 은 자격증명 술어가 언제나 참이므로 최소한 그 줄들은 선다.
    /// 별칭이 여럿인 정규 id 도 한 줄이어야 한다.
    #[test]
    fn every_row_is_a_distinct_credentialled_model() {
        let rows = choices("");
        assert!(!rows.is_empty(), "anthropic rows are always available");
        let mut seen: Vec<&str> = Vec::new();
        for row in &rows {
            assert!(!seen.contains(&row.id.as_str()), "{} listed twice", row.id);
            seen.push(&row.id);
            assert!(row.description.contains("context"), "{:?}", row.description);
        }
        assert!(
            rows.iter().any(|row| row.id.starts_with("claude-")),
            "rows were {rows:?}"
        );
    }

    /// The registry a synthetic Google login answers (t-71998): no
    /// `gemini-3.1-pro-preview` — 3.1 Pro is `gemini-3.1-pro-low` and the
    /// agent id labelled its High — a Flash newer than the shipped one, and
    /// the 3.5 Flash rungs. Nothing serves the shipped `gemini-3.6-flash` or
    /// `gemini-3-flash`.
    fn registry_document() -> serde_json::Value {
        serde_json::json!({"models": {
            "gemini-3.1-pro-low": {"displayName": "Gemini 3.1 Pro (Low)", "modelProvider": "MODEL_PROVIDER_GOOGLE"},
            "gemini-pro-agent": {"displayName": "Gemini 3.1 Pro (High)", "modelProvider": "MODEL_PROVIDER_GOOGLE"},
            "gemini-3.8-flash-low": {"displayName": "Gemini 3.8 Flash (Low)", "modelProvider": "MODEL_PROVIDER_GOOGLE"},
            "gemini-3.8-flash-medium": {"displayName": "Gemini 3.8 Flash (Medium)", "modelProvider": "MODEL_PROVIDER_GOOGLE"},
            "gemini-3.8-flash-high": {"displayName": "Gemini 3.8 Flash (High)", "modelProvider": "MODEL_PROVIDER_GOOGLE"},
            "gemini-3.5-flash-extra-low": {"displayName": "Gemini 3.5 Flash (Low)", "modelProvider": "MODEL_PROVIDER_GOOGLE"},
            "gemini-3.5-flash-low": {"displayName": "Gemini 3.5 Flash (Medium)", "modelProvider": "MODEL_PROVIDER_GOOGLE"},
            "gemini-3-flash-agent": {"displayName": "Gemini 3.5 Flash (High)", "modelProvider": "MODEL_PROVIDER_GOOGLE"}
        }})
    }

    /// (a)(b)(c)(d) Under the Google login the picker, the aliases, every
    /// rung and the status line answer from the account's registry
    /// (t-71998). Published the way a session publishes it, from a synthetic
    /// registry document only.
    #[test]
    fn under_the_google_login_the_picker_offers_what_the_registry_serves() {
        use super::{choices_with, local_clock, login_note_with, typed_pick_with, Pick};
        use runtime::model_discovery::{
            antigravity_rows, overlay, refresh_with, AntigravityList, Answered, UpdatePolicy, ANTIGRAVITY_SOURCE,
            ORIGIN_LIVE, SOURCES,
        };
        let _lock = crate::support::test_env_lock();
        let _env = crate::support::EnvVarGuard::set(api::MODEL_CONTEXT_WINDOWS_ENV, None);
        let _google = crate::support::EnvVarGuard::set("GOOGLE_BASE_URL", Some("http://127.0.0.1:9/v1beta"));
        crate::model_wire_env::reset_ownership_for_tests();

        let document = registry_document();
        let now = 1_792_700_000;
        let every: Vec<&'static str> = SOURCES.iter().map(|source| source.key).collect();
        let catalog = refresh_with(None, &every, now, &[], |source| {
            if source == ANTIGRAVITY_SOURCE {
                Ok(Answered {
                    source: source.to_string(),
                    models: antigravity_rows(&document, source),
                    origin: ORIGIN_LIVE.to_string(),
                    fetched_at: now,
                    note: None,
                })
            } else {
                Err((source.to_string(), runtime::model_discovery::Fault::skipped("skipped: not asked in this test")))
            }
        });
        crate::model_wire_env::publish(None, overlay(&catalog, UpdatePolicy::Auto).json.as_deref()).expect("publish");

        // (a) The Gemini aliases and the demotion rung resolve from the list.
        assert_eq!(api::resolve_model_alias("gemini-pro"), "gemini-3.1-pro");
        assert_eq!(api::resolve_model_alias("gemini-3.5-pro"), "gemini-3.1-pro");
        assert_eq!(api::starvation_demotion_model("gemini-pro").as_deref(), Some("gemini-3.8-flash"));

        // (c) Every rung of every Gemini name sends an id the registry serves.
        let served: Vec<&str> = document["models"].as_object().unwrap().keys().map(String::as_str).collect();
        let efforts = [
            api::EffortLevel::Low,
            api::EffortLevel::Medium,
            api::EffortLevel::High,
            api::EffortLevel::Xhigh,
            api::EffortLevel::Max,
            api::EffortLevel::Ultra,
        ];
        for model in ["gemini-pro", "gemini-3.1-pro", "gemini-3.1-pro-preview", "gemini-flash", "google-latest", "gemini-3.5-flash"] {
            for effort in efforts {
                let wire = api::wire_model_for_effort(model, effort).unwrap_or_else(|| api::resolve_catalog_alias(model));
                assert!(served.contains(&wire.as_str()), "{model} at {effort:?} sends {wire}");
            }
        }

        // (b) The picker offers the registry's releases under their names; a
        // shipped id it names otherwise is no pick of its own unless it is the
        // saved one, and then Enter takes the registry's release; one nothing
        // serves stands dimmed and Enter takes nothing.
        let auto = |_: &str| api::AuthRoute::Auto;
        let rows = choices_with("claude-opus-5", Some(&catalog), AntigravityList::Read(now), &auto);
        let row = |id: &str| rows.iter().find(|choice| choice.id == id);
        assert!(row("gemini-3.1-pro-preview").is_none(), "{rows:?}");
        let pro = row("gemini-3.1-pro").expect("the registry's 3.1 Pro is a pick");
        assert_eq!(pro.pick, Pick::Takes);
        assert!(pro.description.starts_with("Gemini 3.1 Pro · "), "{}", pro.description);
        let gone = row("gemini-3-flash").expect("a shipped id nothing serves stands");
        assert_eq!(gone.pick, Pick::Refused);
        assert_eq!(gone.description, crate::tui::strings::ANTIGRAVITY_UNLISTED);
        let saved = choices_with("gemini-3.1-pro-preview", Some(&catalog), AntigravityList::Read(now), &auto);
        let saved = saved.iter().find(|choice| choice.id == "gemini-3.1-pro-preview").expect("the saved pick stands");
        assert_eq!(saved.pick, Pick::LandsOn("gemini-3.1-pro".to_string()));
        assert_eq!(
            typed_pick_with("gemini-3.1-pro-preview", Some(&catalog), AntigravityList::Read(now), api::AuthRoute::Auto),
            Ok("gemini-3.1-pro".to_string())
        );
        assert!(
            typed_pick_with("gemini-3-flash", Some(&catalog), AntigravityList::Read(now), api::AuthRoute::Auto).is_err()
        );

        // (d) The status line says the list could not be read, and when it
        // was — beside a model that goes over the login, and nowhere else.
        let unread = AntigravityList::Unconfirmed(Some(now));
        let mark = crate::tui::strings::antigravity_unconfirmed(Some(&local_clock(now)));
        assert_eq!(login_note_with("gemini-3.1-pro", Some(&catalog), unread, api::AuthRoute::Auto), Some(mark.clone()));
        assert_eq!(login_note_with("claude-opus-5", Some(&catalog), unread, api::AuthRoute::Auto), None);
        assert_eq!(login_note_with("gemini-3.1-pro", Some(&catalog), unread, api::AuthRoute::ApiKey), None);
        assert_eq!(
            login_note_with("gemini-3.1-pro-preview", Some(&catalog), AntigravityList::Read(now), api::AuthRoute::Auto),
            Some(crate::tui::strings::antigravity_renamed("gemini-3.1-pro"))
        );
        let marked = choices_with("claude-opus-5", Some(&catalog), unread, &auto);
        assert!(marked.iter().filter(|choice| choice.id.starts_with("gemini-")).all(|choice| choice.description.ends_with(&mark)));

        // (e) No other provider's row changes.
        let not_gemini = |rows: Vec<super::ModelChoice>| -> Vec<super::ModelChoice> {
            rows.into_iter().filter(|choice| !choice.id.starts_with("gemini-")).collect()
        };
        assert_eq!(
            not_gemini(choices_with("claude-opus-5", Some(&catalog), unread, &auto)),
            not_gemini(choices_with("claude-opus-5", Some(&catalog), AntigravityList::NoLogin, &auto))
        );

        crate::model_wire_env::publish(Some(r#"{"models":[],"aliases":[]}"#), None).expect("restore");
        assert_eq!(api::resolve_catalog_alias("gemini-pro"), "gemini-3.1-pro-preview");
    }
}
