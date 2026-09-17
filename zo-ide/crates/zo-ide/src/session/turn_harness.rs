//! 호스트 측 턴 스캐폴딩 — 라이브 API 클라이언트 구성과 reactive 자동검증 게이트.
//!
//! 포팅: zo-cli `session/turn_harness.rs`. 자동화 실험군에 걸린 부분(`auto_fanout`
//! 라우트 힌트, `turn_controller` 디자인 리마인더/verify intent, `automation`
//! plan/permission 게이트)은 계획대로 제외했다. 남은 것은 런타임 크레이트가
//! 이미 갖고 있는 것을 턴 단위로 배선하는 얇은 층이다.

use std::sync::Arc;

use super::runtime_bridge::LiveAsyncApiClient;
use super::BuiltRuntime;
use crate::cli_args::AllowedToolSet;

pub(crate) struct TurnHarness;

/// 턴 하나의 판정: 난이도(프로브 융합)와 오케스트레이션 형태. 한 번 읽어
/// verify band·에이전트 정책·호스트 프렐류드가 같은 답을 본다.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TurnSetup {
    pub(crate) assessment: tools::TurnProbeAssessment,
    pub(crate) orchestration: tools::TurnOrchestrationHint,
}

impl TurnHarness {
    /// 턴 시작 배선: (선택) 이전 턴이 남긴 reactive 게이트 정리, 이번 턴의
    /// verify band, 에이전트 정책, 그리고 라우트 힌트 슬롯 비우기.
    pub(crate) fn setup_model_led_turn(
        runtime: &mut BuiltRuntime,
        input: &str,
        clear_stale_reactive_gate: bool,
    ) -> TurnSetup {
        if clear_stale_reactive_gate {
            Self::clear_stale_reactive_gate(runtime);
        }
        // The probe's verdict is read by a deep gate's VERIFY leg (the opt-in
        // reactive gate this harness installs, or a gate already on the
        // runtime) and by an exec contract the settings may arm; with neither,
        // the probe is declined instead of holding the first request.
        // The attempt the turn ABOUT to start will carry: the probe fires
        // before the first request leaves, so its tax has to be billed to the
        // turn that is paying for it, not to the one that just ended.
        let attempt = runtime
            .try_runtime()
            .map(runtime::ConversationRuntime::next_attempt)
            .unwrap_or_default();
        let assessment = tools::assess_turn_probed(
            input,
            runtime.api_client().model(),
            tools::AssessmentReaders {
                verify_leg: Self::auto_verify_opted_in() || runtime.deep_gate().is_some(),
            },
            &attempt,
        );
        let orchestration = tools::assess_turn_orchestration(input);
        if let Some(inner) = runtime.try_runtime_mut() {
            inner.set_verify_band(assessment.complexity, orchestration.risk);
            // 라우트 힌트는 디자인 안내와 같은 턴 단위 set-or-clear: 여기서
            // 비우고, 이번 턴에 호스트 프렐류드가 돌면 그쪽이 다시 채운다.
            inner.replace_transient_system_reminder_by_prefix(
                runtime::ROUTE_HINT_REMINDER_PREFIX,
                None,
            );
        }
        Self::install_turn_agent_policy(runtime, input, &orchestration);
        TurnSetup {
            assessment,
            orchestration,
        }
    }

    /// 이번 턴의 [`tools::TurnAgentPolicy`] 를 공유 툴 컨텍스트에 설치 — 같은
    /// 모델의 단순 구현 spawn 을 인라인으로 접는 `Agent` 디스패치 가드의 입력.
    fn install_turn_agent_policy(
        runtime: &mut BuiltRuntime,
        input: &str,
        orchestration: &tools::TurnOrchestrationHint,
    ) {
        let policy = tools::TurnAgentPolicy {
            user_complexity: tools::assess_turn_complexity(input),
            user_shape: orchestration.shape,
            user_need_count: orchestration.need_count,
            user_requested_delegation: orchestration.user_requested_delegation(),
            user_named_model: orchestration.user_named_model,
        };
        if let Some(inner) = runtime.try_runtime_mut() {
            inner
                .tool_executor_mut()
                .tool_registry_mut()
                .context()
                .set_turn_agent_policy(Some(policy));
        }
    }

    pub(crate) fn build_live_client(
        runtime: &BuiltRuntime,
        allowed_tools: Option<AllowedToolSet>,
        thinking: Option<api::ThinkingConfig>,
        named_effort: Option<api::EffortLevel>,
        effort_band_ceiling: Option<api::EffortLevel>,
    ) -> Arc<LiveAsyncApiClient> {
        let api_client = runtime.api_client();
        Arc::new(LiveAsyncApiClient::new(
            api_client.client(),
            api_client.model().to_string(),
            api_client.auth_route(),
            api_client.enable_tools(),
            allowed_tools,
            api_client.tool_registry(),
            thinking,
            named_effort,
            effort_band_ceiling,
        ))
    }

    /// 코드 변경 턴에 reactive 자동검증(구현→검증→재시도) 게이트 설치.
    /// 기본은 꺼짐 — `ZO_AUTO_VERIFY=1`(또는 `on`) 로 옵트인한 턴에만 설치되고,
    /// 이미 게이트가 있으면 no-op.
    pub(crate) fn install_reactive_verify_gate_if_coding(
        input: &str,
        runtime: &mut BuiltRuntime,
    ) -> DeepGateRestore {
        if !Self::reactive_verify_gate_wanted(
            input,
            Self::auto_verify_opted_in(),
            runtime.deep_gate().is_some(),
        ) {
            return DeepGateRestore::NotInstalled;
        }
        let previous = runtime.deep_gate().cloned();
        if let Some(inner) = runtime.try_runtime_mut() {
            inner.set_deep_gate(Some(runtime::DeepGateConfig {
                mode: runtime::DeepMode::Reactive,
                check_command: Self::headless_objective_check_command(),
                max_attempts: 2,
            }));
        }
        DeepGateRestore::Installed { previous }
    }

    pub(crate) fn restore_deep_gate(runtime: &mut BuiltRuntime, restore: DeepGateRestore) {
        if let DeepGateRestore::Installed { previous } = restore {
            if let Some(inner) = runtime.try_runtime_mut() {
                inner.set_deep_gate(previous);
            }
        }
    }

    /// 장수 런타임에서 이전 턴이 취소돼 남긴 reactive 게이트를 지운다.
    pub(crate) fn clear_stale_reactive_gate(runtime: &mut BuiltRuntime) {
        if matches!(
            runtime.deep_gate().map(|gate| gate.mode),
            Some(runtime::DeepMode::Reactive)
        ) {
            if let Some(inner) = runtime.try_runtime_mut() {
                inner.set_deep_gate(None);
            }
        }
    }

    pub(crate) fn reactive_verify_gate_wanted(
        input: &str,
        opted_in: bool,
        has_gate: bool,
    ) -> bool {
        opted_in && !has_gate && crate::support::prompt_is_coding_task(input)
    }

    pub(crate) fn auto_verify_opted_in() -> bool {
        let value = std::env::var("ZO_AUTO_VERIFY").ok();
        Self::auto_verify_env_opts_in(value.as_deref())
    }

    pub(crate) fn auto_verify_env_opts_in(value: Option<&str>) -> bool {
        value.is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("on"))
    }

    pub(crate) fn headless_objective_check_command() -> Option<String> {
        std::env::var("ZO_AUTO_VERIFY_CMD")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }
}

/// 한 턴짜리 deep-gate 복원 토큰. "설치 안 함" 과 "없던 게이트 위에 설치" 를
/// `Option<Option<_>>` 로 뭉개지 않기 위한 이름 붙인 상태.
#[derive(Debug)]
pub(crate) enum DeepGateRestore {
    NotInstalled,
    Installed {
        previous: Option<runtime::DeepGateConfig>,
    },
}

#[cfg(test)]
mod tests {
    use super::TurnHarness;

    #[test]
    fn reactive_gate_wants_coding_turns_only_when_opted_in() {
        assert!(TurnHarness::reactive_verify_gate_wanted("fix src/a.rs", true, false));
        assert!(!TurnHarness::reactive_verify_gate_wanted("fix src/a.rs", false, false));
        assert!(!TurnHarness::reactive_verify_gate_wanted("fix src/a.rs", true, true));
        assert!(!TurnHarness::reactive_verify_gate_wanted("explain this", true, false));
    }

    #[test]
    fn auto_verify_defaults_to_absent_without_the_env_opt_in() {
        // 프로세스 환경을 만지지 않고 파서 계약만 고정한다: 값이 없으면 꺼짐.
        assert!(!TurnHarness::auto_verify_env_opts_in(None));
        assert!(TurnHarness::auto_verify_env_opts_in(Some("1")));
        assert!(TurnHarness::auto_verify_env_opts_in(Some("on")));
        assert!(!TurnHarness::auto_verify_env_opts_in(Some("0")));
        assert!(!TurnHarness::auto_verify_env_opts_in(Some("off")));
        assert!(!TurnHarness::auto_verify_env_opts_in(Some("")));
    }
}
