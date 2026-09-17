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

use api::ModelClass;

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
}

/// 자격증명이 있는 provider 의 모델들. 카탈로그 순서(anthropic → openai → …)를
/// 지키고, 같은 정규 id 를 가리키는 별칭들은 한 줄로 합친다.
#[must_use]
pub fn choices() -> Vec<ModelChoice> {
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
        out.push(ModelChoice {
            id: entry.canonical_model_id.to_string(),
            description: describe(entry),
            unlisted_since: runtime::model_discovery::unlisted_since(entry.canonical_model_id),
        });
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
            });
        }
    }
    out
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
        let rows = choices();
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
        let rows = choices();
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
}
