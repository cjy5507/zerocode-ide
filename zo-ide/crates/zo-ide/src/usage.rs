//! 사용량 — provider 가 스스로 말하는 한도 창을 **한 가지 모양**으로.
//!
//! 긴 세션이 이 크레이트의 컨셉이고, 긴 세션에서 사람이 실제로 묻는 것은
//! "이 창이 언제 닫히나" 다. Anthropic 구독은 응답 헤더로 5시간/7일 창을 매
//! 턴 실어 보내고, ChatGPT(codex) 계정은 자기 usage 엔드포인트로 같은 성격의
//! 창을 답한다. 둘은 **숫자가 오는 길만 다르고 보여 줄 것은 같다** — 그래서
//! provider 별 분기는 여기서 끝나고, 화면은 창 목록 하나만 안다.
//!
//! 신선도·우선순위 판단은 엔진의 [`api::quota::provider_quota_views`] 가
//! 이미 든다(측정값 > 응답 헤더 > 429 추정). 여기서 다시 정하지 않는다 —
//! 두 자리에 적힌 정책은 반드시 갈라진다.

use std::time::Duration;

use api::quota::{self, ProviderQuotaView};
use api::ProviderKind;

/// 한도 창 하나. 백분율은 **남은** 여유다(엔진이 경계에서 한 번 뒤집어 둔다).
///
/// 라벨은 provider 가 붙인 이름 그대로(`5h`·`7d`·`weekly`)이되, **모델별** 창은
/// 모델 이름이 앞에 붙는다(`GPT-5.3-Codex-Spark 5h`·`Fable 7d`) —
/// [`MODEL_LABEL_SEPARATOR`] 를 보라. 두 provider 다 계정 전체 창 **옆에**
/// 모델별 창을 따로 답하고(codex `additional_rate_limits`, Anthropic
/// `limits[].scope.model`), 그 둘은 서로 다른 사실이다: 계정에 주가 남았어도
/// 지금 모델은 0% 일 수 있다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageWindow {
    pub label: String,
    pub remaining_percent: Option<u8>,
    pub resets_at_unix: Option<u64>,
    /// 429 빈도로 **추정한** 줄. 측정값이 아니라는 표시를 화면이 달 수 있다.
    pub estimated: bool,
}

/// 지금 모델이 쓰는 provider 의 사용량 한 벌.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UsageReport {
    /// 그 provider 자격증명이 들고 있는 계정(있을 때만).
    pub account: Option<String>,
    pub windows: Vec<UsageWindow>,
}

/// 모델별 창의 라벨에서 모델 이름과 창 이름을 가르는 글자.
///
/// [`UsageWindow`] 의 모양은 카드가 이미 그것에 맞춰 그려져 고정이라, 모델
/// 이름이 실릴 자리가 라벨밖에 없다. 마지막 이 글자에서 한 번 자르면 모델과 창이
/// 되돌아온다([`UsageWindow::model`]) — 모델 이름 자체에 이 글자가 있어도
/// **뒤에서** 자르므로 창 이름은 언제나 온전하다.
pub const MODEL_LABEL_SEPARATOR: char = ' ';

impl UsageWindow {
    /// 이 창이 묶인 모델과 창 이름. 계정 전체 창이면 `(None, label)`.
    ///
    /// 카드가 codex 처럼 모델별 창을 제 머리글 아래 묶으려면 이 갈래가 필요하다
    /// (`GPT-5.3-Codex-Spark limit:` 밑에 `5h`·`Weekly`). 자르는 규칙은 여기
    /// 한 곳에만 있다.
    #[must_use]
    pub fn model(&self) -> (Option<&str>, &str) {
        match self.label.rsplit_once(MODEL_LABEL_SEPARATOR) {
            Some((model, window)) if !model.is_empty() && !window.is_empty() => {
                (Some(model), window)
            }
            _ => (None, self.label.as_str()),
        }
    }
}

impl UsageReport {
    /// 보여 줄 것이 하나라도 있는가. 빈 보고서는 화면에서 통째로 빠진다 —
    /// 모르는 것을 0% 로 그리는 것이 가장 나쁘다.
    #[must_use]
    pub fn has_data(&self) -> bool {
        !self.windows.is_empty()
    }

    /// 가장 빠듯한 창. 부팅 경고와 `/status` 강조가 같은 답을 읽는다.
    #[must_use]
    pub fn tightest(&self) -> Option<&UsageWindow> {
        self.windows
            .iter()
            .filter(|window| window.remaining_percent.is_some())
            .min_by_key(|window| window.remaining_percent.unwrap_or(u8::MAX))
    }
}

/// `provider` 가 지금 말하는 사용량. 아무 신호도 없으면 빈 보고서다.
#[must_use]
pub fn for_provider(provider: ProviderKind) -> UsageReport {
    UsageReport {
        account: quota::provider_account(provider),
        windows: quota::provider_quota_views()
            .into_iter()
            .filter(|view| view.provider == provider)
            .map(window_from_view)
            .collect(),
    }
}

fn window_from_view(view: ProviderQuotaView) -> UsageWindow {
    UsageWindow {
        label: match view.model {
            Some(model) => format!("{model}{MODEL_LABEL_SEPARATOR}{}", view.window_label),
            None => view.window_label,
        },
        remaining_percent: view.remaining_percent,
        resets_at_unix: view.resets_at_unix,
        estimated: view.estimated,
    }
}

/// 프로브를 예약한다 — 세션이 열릴 때와 `/status` 앞에서 부른다.
///
/// 엔진이 배경으로 한 번 돌리고 결과를 자기 슬롯에 넣는다. 화면은 그 슬롯을
/// 읽을 뿐이라 프로브가 늦어도 프레임이 멈추지 않는다: 첫 `/status` 는 헤더
/// 스냅샷(있으면)으로 답하고, 다음 번에 측정값이 자리를 잡는다.
pub fn refresh_soon() {
    api::quota_probe::refresh_measured_quotas_soon();
}

/// 측정값이 아직 없을 때 프로브를 기다려 줄 만한 시간.
///
/// `/status` 는 사람이 답을 보려고 친 명령이라 빈 카드보다 반 박자 기다린
/// 카드가 낫다. 창 하나가 몇 시간 단위로 움직이므로 이 정도 지연은 값이
/// 낡을 위험이 없다.
pub const PROBE_GRACE: Duration = Duration::from_millis(700);

#[cfg(test)]
mod tests {
    use super::{window_from_view, UsageReport, UsageWindow};
    use api::quota::ProviderQuotaView;
    use api::ProviderKind;

    fn window(label: &str, remaining: Option<u8>) -> UsageWindow {
        UsageWindow {
            label: label.to_string(),
            remaining_percent: remaining,
            resets_at_unix: None,
            estimated: false,
        }
    }

    #[test]
    fn an_empty_report_shows_nothing() {
        assert!(!UsageReport::default().has_data());
        assert!(UsageReport::default().tightest().is_none());
    }

    /// 백분율을 모르는 창은 "가장 빠듯한" 후보가 아니다 — `None` 을 0 으로
    /// 읽으면 모르는 창이 언제나 경고를 이긴다.
    #[test]
    fn the_tightest_window_ignores_unknown_percentages() {
        let report = UsageReport {
            account: None,
            windows: vec![window("5h", None), window("7d", Some(12)), window("weekly", Some(80))],
        };

        assert_eq!(report.tightest().map(|window| window.label.as_str()), Some("7d"));
    }

    /// 엔진의 모델 표시가 라벨로 접히고, 카드가 그것을 도로 편다.
    #[test]
    fn a_model_scoped_view_folds_into_the_label_and_unfolds_again() {
        let view = |model: Option<&str>, label: &str| ProviderQuotaView {
            provider: ProviderKind::OpenAi,
            window_label: label.to_string(),
            remaining_percent: Some(52),
            resets_at_unix: None,
            estimated: false,
            model: model.map(str::to_string),
        };

        let account = window_from_view(view(None, "7d"));
        assert_eq!(account.label, "7d");
        assert_eq!(account.model(), (None, "7d"));

        let scoped = window_from_view(view(Some("GPT-5.3-Codex-Spark"), "7d"));
        assert_eq!(scoped.label, "GPT-5.3-Codex-Spark 7d");
        assert_eq!(scoped.model(), (Some("GPT-5.3-Codex-Spark"), "7d"));
    }

    /// 이름에 공백이 있는 모델도 창 이름을 잃지 않는다 — 자르는 자리는 **마지막**
    /// 구분자다.
    #[test]
    fn a_model_name_with_spaces_keeps_its_window_name() {
        assert_eq!(
            window("Claude Opus 4 5h", Some(3)).model(),
            (Some("Claude Opus 4"), "5h")
        );
    }
}
