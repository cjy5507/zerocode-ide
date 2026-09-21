//! Reusable `/status` formatting helpers.
//!
//! The plain loop and the TUI own their presentation, but the arithmetic must
//! not fork: context usage is the provider's input-side occupancy, tokens are
//! compacted consistently, and cost uses the model-specific pricing table with
//! an explicit approximation marker for unknown models.

use std::sync::OnceLock;

use runtime::{TokenUsage, UsageCostEstimate};

use crate::tui::ansi::{str_width, write_spans, Line, Span, RESET};
use crate::tui::cells;
use crate::tui::fast;
use crate::tui::paths::center_truncate_path;
use crate::session::plain_session::PlainSession;
use crate::usage::{UsageReport, UsageWindow};
use crate::goal::GoalFooterStatus;

/// Codex's footer has one collaboration-mode indicator.  zo's actual plan
/// approval action is `/plan off`, not Shift+Tab, so there is intentionally no
/// keyboard-cycle suffix here.
pub const PLAN_MODE_FOOTER_TEXT: &str = "Plan mode";

/// The exact Codex goal-status grammar for the footer.  `HitAutonomousLimits`
/// is the one zo extension: its continuation, assistant-turn, output-token,
/// and wall-clock ceilings are not all Codex usage limits, so calling all of
/// them usage would be inaccurate.
#[must_use]
pub const fn goal_footer_text(status: GoalFooterStatus) -> &'static str {
    match status {
        GoalFooterStatus::Pursuing => "Pursuing goal",
        GoalFooterStatus::Paused => "Goal paused (/goal resume)",
        GoalFooterStatus::Stalled => "Goal stalled (/goal resume)",
        GoalFooterStatus::HitAutonomousLimits => "Goal hit autonomous limits (/goal resume)",
        GoalFooterStatus::Unmet => "Goal unmet",
        GoalFooterStatus::Achieved => "Goal achieved",
    }
}

/// Shared wording for the one-shot account handoff notice. Both frontends use
/// this table entry so the channel event cannot acquire two translations.
#[must_use]
pub fn account_switch_notice(label: &str) -> String {
    let label = crate::util::ansi::sanitize_inline(label.trim());
    format!("계정 전환 · {label}")
}

/// Context occupancy as a clamped percentage in the inclusive `0..=100` range.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn context_usage_percent(used_tokens: u64, context_limit: u64) -> f64 {
    if context_limit == 0 {
        return 0.0;
    }
    (used_tokens as f64 / context_limit as f64 * 100.0).clamp(0.0, 100.0)
}

// The system prompt and tool manifest are assembled dynamically by
// `conversation_support`, while the recall reservation is applied in the
// runtime builder. The ledger exposes neither the complete prefix nor a
// tokenizer-backed fixed count. The brief's 1,987-token recall reservation
// therefore cannot be applied on its own without making this percentage
// asymmetric; keep the proven zo baseline at zero until the runtime reports
// the complete prefix.
const ZO_CONTEXT_PREFIX_TOKENS: u64 = 0;

/// Remaining context as a rounded percentage.
///
/// A zero context limit means that the provider did not expose a usable
/// window, so callers can fall back to the used-token form of the footer.
#[must_use]
pub fn context_left_percent(used_tokens: u64, context_limit: u64) -> Option<u8> {
    if context_limit == 0 {
        return None;
    }
    let usable_limit = context_limit.saturating_sub(ZO_CONTEXT_PREFIX_TOKENS);
    if usable_limit == 0 {
        return Some(0);
    }

    let adjusted_used = used_tokens.saturating_sub(ZO_CONTEXT_PREFIX_TOKENS);
    let remaining = usable_limit.saturating_sub(adjusted_used);
    // Integer round-half-up. codex rounds a float here, but a percentage of a
    // token count has no fractional part worth a cast that clippy has to be
    // told to ignore — and `remaining * 200` cannot overflow a context window.
    let percent = (remaining * 200 + usable_limit) / (usable_limit * 2);
    Some(u8::try_from(percent.min(100)).unwrap_or(100))
}

/// Format context occupancy and both token counts for a status row.
#[must_use]
pub fn format_context_usage(used_tokens: u64, context_limit: u64) -> String {
    if context_limit == 0 {
        return format!("ctx {}", format_tokens(used_tokens));
    }
    format!(
        "ctx {:.1}% ({}/{})",
        context_usage_percent(used_tokens, context_limit),
        format_tokens(used_tokens),
        format_tokens(context_limit),
    )
}

/// Compact a token count for human-facing status output.
///
/// The spelling lives in [`core_types::helper_run`] so the footer, the helper
/// rows, and the model-facing completion notice all read one implementation.
pub use core_types::helper_run::tokens_compact as format_tokens;

/// Codex's compact token spelling used by the context footer.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn format_tokens_compact(tokens: u64) -> String {
    if tokens < 1_000 {
        return tokens.to_string();
    }

    let mut value = tokens as f64;
    let mut suffix = "K";
    for candidate in ["K", "M", "B", "T"] {
        value /= 1_000.0;
        suffix = candidate;
        if value < 1_000.0 || candidate == "T" {
            break;
        }
    }

    let mut formatted = format!("{value:.1}");
    if formatted.ends_with(".0") {
        formatted.truncate(formatted.len().saturating_sub(2));
    }
    format!("{formatted}{suffix}")
}

/// Estimate a token sample using the model-specific public pricing table.
#[must_use]
pub fn estimate_usage_cost(model: &str, usage: TokenUsage) -> UsageCostEstimate {
    core_types::usage::estimate_usage_cost(model, usage)
}

/// Return the total estimated USD cost as a number for machine consumers.
#[must_use]
pub fn estimated_cost_usd(model: &str, usage: TokenUsage) -> f64 {
    estimate_usage_cost(model, usage).total_cost_usd()
}

/// Format a cost with the same two-decimal presentation as the forge HUD.
#[must_use]
pub fn format_cost_usd(amount: f64) -> String {
    if amount < 0.001 {
        "$0.00".to_string()
    } else {
        format!("${amount:.2}")
    }
}

/// Format a model-aware estimated cost. Unknown models use the fallback tier
/// and are marked with `~` so a status line cannot imply a metered bill.
#[must_use]
pub fn format_estimated_cost(model: &str, usage: TokenUsage) -> String {
    let prefix = if runtime::pricing_for_model(model).is_some() {
        ""
    } else {
        "~"
    };
    format!("{prefix}{}", format_cost_usd(estimated_cost_usd(model, usage)))
}

/// The provider's input-side context occupancy for a usage sample.
#[must_use]
/// Context tokens from **one request's** usage.
///
/// Only ever call this with a single turn's usage. Passing the session
/// cumulative counts every turn's cache reads again, which with prompt caching
/// is roughly the whole window per turn — a long session then reports `0%
/// context left` while the real window is barely half full. The live screen
/// reads `RenderBlock::Usage.ctx_tokens` instead, which the runtime already
/// computes for exactly this.
pub fn usage_context_tokens(usage: TokenUsage) -> u64 {
    u64::from(usage.context_tokens())
}

/// Shared codex-style context footer text.
///
/// `None, None` is the pending state and intentionally produces no text. Once
/// usage is known, a missing percentage falls back to compact used tokens.
#[must_use]
pub fn context_footer_text(
    percent_left: Option<u8>,
    used_tokens: Option<u64>,
) -> Option<String> {
    match (percent_left, used_tokens) {
        (Some(percent), _) => Some(format!("{percent}% context left")),
        (None, Some(used)) => Some(format!("{} used", format_tokens_compact(used))),
        (None, None) => None,
    }
}

/// A compact status fragment containing context, token total, and estimated
/// cost. The caller can prepend model/permission/session fields as needed.
#[must_use]
pub fn format_usage_status(model: &str, usage: TokenUsage, context_limit: u64) -> String {
    let context = format_context_usage(usage_context_tokens(usage), context_limit);
    format!(
        "{context} · tokens {} · cost {}",
        format_tokens(u64::from(usage.total_tokens())),
        format_estimated_cost(model, usage)
    )
}

// ============================================================================
// `/status` 카드 — codex 0.150.1 문법
// ============================================================================
//
// 실측: `docs/captures/codex-tui-v0.150.1-status.bin` (120x40 PTY, 스크롤백
// 25~44행). 카드에서 직접 읽어 낸 사실이 아래 상수·함수의 근거다.
//
// ```text
// │  Model:                       gpt-5.3-codex-spark (reasoning xhigh, …)   │
// │  Context window:              100% left (0 used / 1M)                    │
// │  Weekly limit:                [█░░░░░░░░░░░░░░░░░░░] 5% left (resets 01:02 on 2 Sep)
// │  GPT-5.3-Codex-Spark limit:
// │  5h limit:                    [███████████████████░] 93% left (resets 20:11)
// │  Weekly limit:                [██████████░░░░░░░░░░] 52% left (resets 20:22 on 1 Sep)
// ```
//
// 1. 값 열은 상자 안쪽 31칸째다 — 상자 패딩 한 칸 + 행 자신의 한 칸 + 가장 긴
//    라벨(값 없는 머리글 `GPT-5.3-Codex-Spark limit:`, 26칸) + [`LABEL_GAP`].
//    **상수 폭이 아니라 행 목록에서 나온다**(머리글도 그 목록에 든다).
// 2. 게이지는 스무 칸이고 **채운 칸이 남은 양**이다(5%→1, 93%→19, 52%→10).
//    세 값이 내림(93%→18)과 올림(52%→11)을 함께 배제하므로 반올림이다.
// 3. 백분율은 우측 정렬이 **아니다** — `] 5% left` 와 `] 93% left` 가 똑같이
//    한 칸 뒤에 붙는다.
// 4. dim 은 라벨·여백·꼬리 괄호이고 값만 밝다(`ESC[2m` 라벨 `ESC[22m` 값
//    `ESC[2m` 꼬리). 컨텍스트 창 줄에는 게이지가 없다.

/// 게이지 칸 수 — 캡처의 스무 칸.
pub const GAUGE_CELLS: usize = 20;

/// 라벨 열의 여백 — 가장 긴 라벨 뒤 세 칸(캡처의 값 열이 31칸째인 근거).
pub const LABEL_GAP: usize = 3;

/// 게이지 문자 — 남은 칸과 쓴 칸.
const GAUGE_LEFT: &str = "█";
const GAUGE_SPENT: &str = "░";

/// 리셋 문구의 달 이름 — 캡처의 `on 1 Sep` 과 같은 세 글자.
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// 남은 양을 채운 게이지 — `[███████████████████░]`.
///
/// **채운 칸이 남은 양**이다. 반대로 읽으면 한도가 바닥일 때 게이지가 가득 차
/// 보이므로, 이 방향은 캡처에서 직접 확인한 사실이다(93% → 19칸).
#[must_use]
pub fn gauge(remaining_percent: u8) -> String {
    let percent = usize::from(remaining_percent.min(100));
    let filled = (percent * GAUGE_CELLS + 50) / 100;
    format!(
        "[{}{}]",
        GAUGE_LEFT.repeat(filled),
        GAUGE_SPENT.repeat(GAUGE_CELLS - filled)
    )
}

/// 라벨 열의 폭 — 가장 긴 라벨 + [`LABEL_GAP`]. 상수 폭을 박지 않는다:
/// 창 라벨은 provider 가 붙이므로 행 목록을 봐야 알 수 있다.
#[must_use]
pub fn label_column<'a>(labels: impl IntoIterator<Item = &'a str>) -> usize {
    labels
        .into_iter()
        .map(str_width)
        .max()
        .unwrap_or(0)
        .saturating_add(LABEL_GAP)
}

/// provider 가 붙인 창 이름 → 카드에 찍히는 라벨. **매핑은 여기 한 함수뿐**이다.
///
/// `7d` 가 `Weekly` 인 것은 캡처를 따른 것이다 — codex 의 주간 창도
/// `Weekly limit` 으로 찍힌다. `429` 는 측정된 창이 아니라 쿨다운에서 **추정한**
/// 줄이라(`api::quota::ProviderQuotaView`) 사람 말로 `Rate limit` 이다.
#[must_use]
pub fn window_label(label: &str) -> String {
    match label {
        "5h" => "5h limit".to_string(),
        "7d" | "weekly" => "Weekly limit".to_string(),
        "429" => "Rate limit".to_string(),
        other => format!("{other} limit"),
    }
}

/// 리셋 시각 한 문구 — 같은 날이면 시각만, 다른 날이면 날짜를 붙인다.
///
/// 캡처의 두 모양 그대로다: `resets 20:11` · `resets 20:22 on 1 Sep`.
/// 시각은 **지역 시각**이다(`local_offset_seconds`).
#[must_use]
pub fn format_reset(resets_at_unix: u64, now_unix: u64) -> String {
    format_reset_at(resets_at_unix, now_unix, local_offset_seconds())
}

/// [`format_reset`] 의 순수한 속 — 오프셋을 주입받아 시계를 읽지 않는다.
#[must_use]
pub fn format_reset_at(resets_at_unix: u64, now_unix: u64, offset_seconds: i64) -> String {
    let (day, seconds) = local_day_and_seconds(resets_at_unix, offset_seconds);
    let (today, _) = local_day_and_seconds(now_unix, offset_seconds);
    let clock = format!("{:02}:{:02}", seconds / 3_600, (seconds % 3_600) / 60);
    if day == today {
        return format!("resets {clock}");
    }
    let (_, month, date) = civil_from_days(day);
    let month = usize::try_from(month).unwrap_or(1).clamp(1, MONTHS.len());
    format!("resets {clock} on {date} {}", MONTHS[month - 1])
}

/// unix 초 → (지역 기준 일수, 그 날의 초).
fn local_day_and_seconds(unix: u64, offset_seconds: i64) -> (i64, i64) {
    let local = i64::try_from(unix)
        .unwrap_or(i64::MAX)
        .saturating_add(offset_seconds);
    (local.div_euclid(86_400), local.rem_euclid(86_400))
}

/// 1970-01-01 로부터의 일수 → (연, 월, 일). Howard Hinnant 의 `civil_from_days`
/// 를 정수 그대로 옮긴 것이라 부동소수도 윤년 표도 없다.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let date = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    (year_of_era + era * 400 + i64::from(month <= 2), month, date)
}

/// 지역 UTC 오프셋(초). 프로세스당 한 번 묻고 캐시한다.
///
/// 이 워크스페이스에는 달력 크레이트가 없고(`chrono`·`time` 둘 다 직접 의존이
/// 아니다) `unsafe_code = "forbid"` 라 `localtime_r` 도 부를 수 없다. 그래서 OS
/// 에게 한 번 묻는다 — 못 물으면 UTC(0)로 떨어져 시각이 틀리게 보일지언정 카드가
/// 사라지지는 않는다. `/status` 는 사람이 치는 명령이라 프로세스당 한 번의
/// `date` 호출이 프레임을 붙잡지 않는다.
pub fn local_offset_seconds() -> i64 {
    static OFFSET: OnceLock<i64> = OnceLock::new();
    *OFFSET.get_or_init(|| {
        std::process::Command::new("date")
            .arg("+%z")
            .output()
            .ok()
            .and_then(|probe| parse_utc_offset(String::from_utf8_lossy(&probe.stdout).trim()))
            .unwrap_or(0)
    })
}

/// `+0900` 꼴 한 낱말을 초로. `date +%z` 의 답만 받는다.
fn parse_utc_offset(text: &str) -> Option<i64> {
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, text.strip_prefix('+').unwrap_or(text)),
    };
    if digits.len() != 4 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let hours: i64 = digits[..2].parse().ok()?;
    let minutes: i64 = digits[2..].parse().ok()?;
    Some(sign * (hours * 3_600 + minutes * 60))
}

/// `/status` 카드의 재료. 두 프런트엔드가 같은 구조체를 채우고 같은 행을 받는다.
#[derive(Debug, Clone, Copy)]
pub struct StatusCard<'a> {
    pub product: &'a str,
    pub version: &'a str,
    pub model: &'a str,
    pub effort: Option<&'a str>,
    pub cwd: &'a str,
    pub permissions: &'a str,
    pub permissions_pending: bool,
    pub session_id: &'a str,
    /// 지속 목표와 autonomous controller의 현재 한 줄 상태.
    pub goal: &'a str,
    pub autonomous: &'a str,
    pub loops: &'a str,
    pub context_tokens: u64,
    pub context_limit: u64,
    /// 읽기 전용 — 사용량은 [`crate::usage`] 가 소유한다.
    pub usage: &'a UsageReport,
    /// 이 판이 자격을 찾을 수 있는 프로바이더의 계정들
    /// (`crate::runtime_support::all_account_facts`). 비면 그 구역은 통째로
    /// 접힌다 — 계정 저장소가 없는 판에 빈 줄을 세우지 않는다.
    pub accounts: &'a [crate::runtime_support::AccountFacts],
    /// 마지막 드리머 패스 한 줄(`crate::dream::last_pass_line`). 이 프로세스에서
    /// 패스가 끝난 적이 없으면 `None` 이고 그 행은 통째로 접힌다 — "아직
    /// 모른다" 를 "아무 일도 없었다" 로 그리지 않는다.
    pub dream: Option<&'a str>,
    /// "같은 날인가" 를 재는 기준 시각.
    pub now_unix: u64,
    /// 화면 폭. 상자의 **천장**만 정한다 — 상자는 내용에 맞춰 줄어든다.
    pub width: usize,
}

/// 라벨-값 한 행.
struct Field {
    /// 콜론까지 포함한 라벨(`"Model:"`).
    label: String,
    value: String,
    /// 값 뒤의 dim 꼬리(`"(resets 20:11)"`). 비면 스팬을 만들지 않는다.
    suffix: String,
    /// 값이 경로라 넘칠 때 **가운데**를 버린다(부팅 카드와 같은 규칙).
    path: bool,
}

impl Field {
    fn new(label: &str, value: impl Into<String>, suffix: impl Into<String>) -> Self {
        Self {
            label: format!("{label}:"),
            value: value.into(),
            suffix: suffix.into(),
            path: false,
        }
    }

    fn path(mut self) -> Self {
        self.path = true;
        self
    }

    fn line(&self, label_width: usize, ceiling: usize) -> Line {
        let value = if self.path {
            center_truncate_path(
                &self.value,
                ceiling.saturating_sub(label_width.saturating_add(1)),
            )
        } else {
            self.value.clone()
        };
        // 행 자신의 한 칸 — 상자의 패딩 한 칸 위에 얹혀 캡처의 두 칸이 된다.
        let mut spans = vec![
            Span::dim(format!(" {:<label_width$}", self.label)),
            Span::raw(value),
        ];
        if !self.suffix.is_empty() {
            spans.push(Span::dim(format!(" {}", self.suffix)));
        }
        Line::new(spans)
    }
}

/// 카드 한 줄.
enum Row {
    Blank,
    Spans(Vec<Span>),
    /// 값 없는 머리글(`GPT-5.3-Codex-Spark limit:`). 라벨 열 폭에는 **들되**
    /// 뒤에 여백이 붙지 않는다 — 캡처의 그 행은 라벨에서 곧장 끝난다.
    Header(String),
    Field(Field),
}

impl Row {
    fn label(&self) -> Option<&str> {
        match self {
            Row::Header(label) => Some(label.as_str()),
            Row::Field(field) => Some(field.label.as_str()),
            Row::Blank | Row::Spans(_) => None,
        }
    }

    fn line(&self, label_width: usize, ceiling: usize) -> Line {
        match self {
            Row::Blank => Line::empty(),
            Row::Spans(spans) => Line::new(spans.clone()),
            Row::Header(label) => Line::new(vec![Span::dim(format!(" {label}"))]),
            Row::Field(field) => field.line(label_width, ceiling),
        }
    }
}

/// `/status` 카드의 행 — **두 프런트엔드가 이 한 함수에서만 받는다.**
///
/// 사용량 구역은 값이 없으면 통째로 접힌다: 창이 하나도 없으면 창 줄이 없고,
/// 백분율을 모르는 창은 게이지 대신 `unknown` 을 단다. 모르는 것을 0% 로 그리는
/// 것이 가장 나쁘다.
#[must_use]
pub fn status_card(card: &StatusCard<'_>) -> Vec<Line> {
    let ceiling = card.width.max(4).saturating_sub(4);
    let (model, fast_enabled) = fast::display(card.model);
    let mut notes = Vec::new();
    if let Some(effort) = card.effort.filter(|effort| !effort.is_empty()) {
        notes.push(format!("reasoning {effort}"));
    }
    if fast_enabled {
        notes.push(fast::FOOTER_TOKEN.to_string());
    }

    let mut rows = vec![
        Row::Spans(vec![
            Span::dim(" >_ "),
            Span::bold(card.product.to_string()),
            Span::dim(format!(" (v{})", card.version)),
        ]),
        Row::Blank,
        Row::Field(Field::new("Model", model, parenthetical(&notes.join(", ")))),
        Row::Field(Field::new("Directory", card.cwd, "").path()),
        Row::Field(Field::new(
            "Permissions",
            card.permissions,
            if card.permissions_pending {
                "(approvals switch when this turn ends)"
            } else {
                ""
            },
        )),
    ];
    if let Some(account) = card.usage.account.as_deref() {
        rows.push(Row::Field(Field::new("Account", account, "")));
    }
    rows.extend(account_rows(card.accounts));
    rows.push(Row::Field(Field::new("Session", card.session_id, "")));
    rows.push(Row::Field(Field::new("Goal", card.goal, "")));
    rows.push(Row::Field(Field::new(
        "Autonomous",
        card.autonomous,
        "",
    )));
    if card.loops != "none" {
        rows.push(Row::Field(Field::new("Loops", card.loops, "")));
    }
    rows.push(Row::Blank);
    rows.push(Row::Field(context_window_field(
        card.context_tokens,
        card.context_limit,
    )));
    rows.extend(usage_rows(card.usage, card.model, card.now_unix));
    if let Some(dream) = card.dream {
        // 기억이 이 CLI 의 컨셉이라 카드가 그 증거를 든다. 문안이 `dreamer:` 로
        // 스스로를 밝히므로 라벨 열에 넣지 않는다 — 넣으면 라벨 폭만 넓히고
        // 값 열은 비는 행이 된다.
        rows.push(Row::Blank);
        rows.push(Row::Spans(vec![Span::dim(format!(" {dream}"))]));
    }

    let label_width = label_column(rows.iter().filter_map(Row::label));
    let lines = rows
        .iter()
        .map(|row| row.line(label_width, ceiling))
        .collect();
    cells::card(lines, ceiling)
}

/// 카드 행을 파이프가 그대로 인쇄할 한 덩어리로 — 같은 행, 같은 스타일.
/// 색을 끈 렌더러가 SGR 을 걷어내므로 `NO_COLOR` 파이프에는 평문만 나간다.
#[must_use]
pub fn card_text(rows: &[Line]) -> String {
    let mut out = String::new();
    for (index, line) in rows.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        write_spans(line, &mut out);
        out.push_str(RESET);
    }
    out
}

/// 계정 구역 — 프로바이더마다 한 줄, 라벨과 출처.
///
/// 라벨은 창이 `auth.reload` 에 실어 보낸 표시 이름이다. 창이 말해 주지 않았으면
/// 출처만 말한다: `/status` 가 이메일이나 계정 id 를 지어내는 자리가 되면 그
/// 마스킹 규칙이 두 벌이 된다(설계 §2.3).
fn account_rows(accounts: &[crate::runtime_support::AccountFacts]) -> Vec<Row> {
    accounts
        .iter()
        .map(|facts| {
            let origin = origin_phrase(facts.origin);
            let label = provider_label(facts.provider);
            Row::Field(match facts.label.as_deref() {
                Some(named) => Field::new(label, named, parenthetical(origin)),
                None => Field::new(label, origin, ""),
            })
        })
        .collect()
}

/// 카드의 라벨 열에 서는 프로바이더 이름.
const fn provider_label(provider: &str) -> &'static str {
    match provider.as_bytes() {
        b"anthropic" => "Anthropic",
        b"openai" => "OpenAI",
        b"google" => "Google",
        _ => "Provider",
    }
}

/// 출처 한 낱말 — 사람이 읽는 쪽.
const fn origin_phrase(origin: crate::runtime_support::AccountOrigin) -> &'static str {
    match origin {
        crate::runtime_support::AccountOrigin::IdeManaged => "IDE-managed",
        crate::runtime_support::AccountOrigin::OwnLogin => "own login",
        crate::runtime_support::AccountOrigin::Keychain => "Claude Code keychain",
        crate::runtime_support::AccountOrigin::Env => "environment",
    }
}

/// 한도 창 구역 — 계정 전체 창이 먼저, 그 뒤로 모델마다 머리글 하나와 그 창들.
/// 캡처의 배치 그대로다(`Weekly limit` → `GPT-5.3-Codex-Spark limit:` 머리글
/// → `5h limit` · `Weekly limit`).
fn usage_rows(usage: &UsageReport, current_model: &str, now_unix: u64) -> Vec<Row> {
    let mut account: Vec<Row> = Vec::new();
    let mut scoped: Vec<(&str, Vec<Row>)> = Vec::new();
    for window in &usage.windows {
        let (model, name) = window.model();
        let row = Row::Field(window_field(window, name, now_unix));
        match model {
            None => account.push(row),
            // 지금 쓰지 않는 모델의 창은 이 카드의 질문("내 창이 언제 닫히나")에
            // 답하지 않는다. 게다가 라벨이 계정 창과 같은 낱말이라(`Weekly
            // limit`) 나란히 서면 같은 값이 두 번 찍힌 것처럼 읽힌다 — 실제로
            // 그렇게 보였다(사용자 판정, 2026-08-27: "weekly limit 이 두 개 보임").
            // 모델을 바꾸면 그 모델의 창이 이 자리에 선다.
            Some(model) if !model_matches(current_model, model) => {}
            Some(model) => match scoped.iter_mut().find(|(seen, _)| *seen == model) {
                Some((_, group)) => group.push(row),
                None => scoped.push((model, vec![row])),
            },
        }
    }
    for (model, group) in scoped {
        account.push(Row::Header(format!("{model} limit:")));
        account.extend(group);
    }
    account
}

/// provider 가 붙인 **사람이 읽는 모델 이름**(`Fable`·`GPT-5.3-Codex-Spark`)이
/// 지금 쓰는 모델 id(`claude-opus-5`·`gpt-5.3-codex-spark`)를 가리키는가.
///
/// 두 이름은 같은 문자열이 아니다: provider 는 표시명을, 우리는 카탈로그 id 를
/// 든다. 그래서 글자·숫자만 남겨 소문자로 접은 뒤 **표시명이 id 안에 들어
/// 있는지**로 판단한다 — `opus` ⊂ `claudeopus5` 는 참이고 `fable` ⊄
/// `claudeopus5` 는 거짓이며, `gpt53codexspark` 는 서로 같다.
///
/// 규칙은 여기 한 곳에만 있다. 표시명이 비면(있을 수 없지만) 아무것도 가리키지
/// 않는 것으로 본다 — 빈 문자열은 모든 id 의 부분열이라 그대로 두면 남의 모델
/// 창이 전부 딸려 온다.
fn model_matches(model_id: &str, display_name: &str) -> bool {
    fn folded(text: &str) -> String {
        text.chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|character| character.to_ascii_lowercase())
            .collect()
    }
    let display = folded(display_name);
    !display.is_empty() && folded(model_id).contains(&display)
}

/// 컨텍스트 창 한 행 — 캡처는 여기에 게이지를 달지 않는다(`100% left (0 used / 1M)`).
fn context_window_field(used_tokens: u64, context_limit: u64) -> Field {
    if context_limit == 0 {
        return Field::new(
            "Context window",
            format!("{} used", format_tokens(used_tokens)),
            "",
        );
    }
    let left = context_left_percent(used_tokens, context_limit).unwrap_or(0);
    Field::new(
        "Context window",
        format!("{left}% left"),
        format!(
            "({} used / {})",
            format_tokens(used_tokens),
            format_tokens(context_limit)
        ),
    )
}

/// 한도 창 한 행. `estimated` 줄은 429 빈도로 **추정한** 값이라 꼬리에 `est` 를
/// 단다 — `api::quota::ProviderQuotaView` 가 "a HUD renders these `(est)`" 라고
/// 적어 둔 그 표기다. 캡처에 없는 상태이므로 codex 문법(`(resets …)`)을 깨지
/// 않도록 같은 괄호 안에 넣는다.
fn window_field(window: &UsageWindow, name: &str, now_unix: u64) -> Field {
    let value = match window.remaining_percent {
        Some(percent) => format!("{} {percent}% left", gauge(percent)),
        None => "unknown".to_string(),
    };
    let mut notes = Vec::new();
    if window.estimated {
        notes.push("est".to_string());
    }
    if let Some(resets_at) = window.resets_at_unix {
        notes.push(format_reset(resets_at, now_unix));
    }
    Field::new(&window_label(name), value, parenthetical(&notes.join(" · ")))
}

/// 비면 빈 문자열, 아니면 괄호로 싼다.
fn parenthetical(inner: &str) -> String {
    if inner.is_empty() {
        String::new()
    } else {
        format!("({inner})")
    }
}

/// 카드 머리의 제품 이름 — 부팅 카드와 `/status` 카드가 같은 이름을 읽는다.
pub const PRODUCT: &str = "zo";

/// 이 모델이 실제로 붙는 provider.
///
/// 판별표는 **런타임이 클라이언트를 세울 때 쓰는 그것 하나뿐**이다
/// (`crate::runtime_support::provider_kind_for_model`). 카드가 제 표를 따로
/// 들면 별칭 해석 순서가 갈리는 날 화면이 A 를 말하고 요청은 B 로 나간다 —
/// 사용량 카드에서 그것은 "남의 계정 한도를 내 것처럼 보여 주는" 오류다.
#[must_use]
pub fn provider_for_model(model: &str) -> api::ProviderKind {
    crate::runtime_support::provider_kind_for_model(&crate::cli_args::resolve_model_alias(model))
}

/// 지금 시각(unix 초) — 리셋 문구의 "같은 날인가" 기준.
#[must_use]
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// 지금 세션의 `/status` 카드 — **두 프런트엔드가 이 한 함수를 부른다.**
///
/// 사용량은 읽기 전이다: 지금 슬롯에 있는 것만 그린다(프로브 예약은
/// [`PlainSession::status`] 가 든다). 기다리지 않으므로 첫 `/status` 는 헤더
/// 스냅샷(있으면)으로 답하고 다음 번에 측정값이 자리를 잡는다 — 프레임을
/// 붙잡느니 사용량 구역을 접는 편이 낫다.
#[must_use]
pub fn session_status_card(session: &PlainSession, width: usize) -> Vec<Line> {
    let status = session.status();
    card_from_facts(
        &StatusFacts {
            model: status.model.clone(),
            effort: status.effort.map(str::to_string),
            cwd: crate::tui::view::short_cwd(&session.cwd.to_string_lossy()),
            permissions: status.permission_mode,
            // An idle card is asked between turns, so nothing is pending.
            permissions_pending: false,
            session_id: status.session_id.clone(),
            context_tokens: status.context_tokens as u64,
            goal: status.goal,
            autonomous: status.autonomous,
            loops: status.loops,
        },
        width,
    )
}

/// 카드가 세션에게 묻는 것 전부.
///
/// 턴이 도는 동안 세션은 턴 task 가 통째로 가져가 있어(`App::turn`) 물어볼 수
/// 없다. 그런데 이 값들은 화면이 푸터·배너·goal HUD를 그리느라 이미 들고
/// 있는 것들이라,
/// 그때는 화면이 채워 넣는다 — 그래서 `/status` 가 턴 중에도 답한다. 사용량과
/// 드리머 줄은 프로세스 전역이라 어느 쪽에서 오든 같다.
pub struct StatusFacts {
    pub model: String,
    pub effort: Option<String>,
    pub cwd: String,
    pub permissions: &'static str,
    /// A mode the user selected that the APPROVAL half has not taken yet.
    ///
    /// Only half a permission switch can land mid-turn: the shared cell moves
    /// the file-tool workspace boundary at once, while the approval policy
    /// lives inside the runtime the turn owns and swaps only when the turn
    /// ends. `permissions` above is the label the user picked, so without this
    /// the card reads `danger-full-access` while the enforcer is still denying
    /// writes as read-only — which is exactly what a screen showed.
    pub permissions_pending: bool,
    pub session_id: String,
    pub context_tokens: u64,
    pub goal: String,
    pub autonomous: String,
    pub loops: String,
}

/// 사실 한 벌 → 카드. **조립은 여기 한 번만** 일어난다.
#[must_use]
pub fn card_from_facts(facts: &StatusFacts, width: usize) -> Vec<Line> {
    let usage = crate::usage::for_provider(provider_for_model(&facts.model));
    let dream = crate::dream::last_pass_line();
    let accounts = crate::runtime_support::all_account_facts();
    status_card(&StatusCard {
        accounts: &accounts,
        dream: dream.as_deref(),
        product: PRODUCT,
        version: env!("CARGO_PKG_VERSION"),
        model: &facts.model,
        effort: facts.effort.as_deref(),
        cwd: &facts.cwd,
        permissions: facts.permissions,
        permissions_pending: facts.permissions_pending,
        session_id: &facts.session_id,
        goal: &facts.goal,
        autonomous: &facts.autonomous,
        loops: &facts.loops,
        context_tokens: facts.context_tokens,
        context_limit: api::context_window_for_model(&facts.model),
        usage: &usage,
        now_unix: now_unix(),
        width,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        context_footer_text, context_left_percent, context_usage_percent, estimated_cost_usd,
        format_context_usage, format_cost_usd, format_estimated_cost, format_tokens,
        format_tokens_compact, format_usage_status, usage_context_tokens,
    };
    use runtime::TokenUsage;

    #[test]
    #[allow(clippy::float_cmp)]
    fn context_percentage_is_clamped_and_zero_safe() {
        assert_eq!(context_usage_percent(50_000, 200_000), 25.0);
        assert_eq!(context_usage_percent(300, 200), 100.0);
        assert_eq!(context_usage_percent(300, 0), 0.0);
        assert_eq!(context_left_percent(0, 200_000), Some(100));
        assert_eq!(context_left_percent(50_000, 200_000), Some(75));
        assert_eq!(context_left_percent(300, 200), Some(0));
        assert_eq!(context_left_percent(300, 0), None);
        // Integer round-half-up, matching codex's `.round()`: 0.5 goes up.
        assert_eq!(context_left_percent(995, 1_000), Some(1));
        assert_eq!(context_left_percent(996, 1_000), Some(0));
        assert_eq!(context_left_percent(5, 1_000), Some(100));
    }

    #[test]
    fn token_and_context_formats_are_compact() {
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(2_345), "2.3k");
        assert_eq!(format_tokens(1_000_000), "1.0M");
        assert_eq!(format_tokens(1_050_000), "1.05M");
        assert_eq!(format_tokens_compact(0), "0");
        assert_eq!(format_tokens_compact(999), "999");
        assert_eq!(format_tokens_compact(6_575), "6.6K");
        assert_eq!(format_tokens_compact(12_000), "12K");
        assert_eq!(format_context_usage(50_000, 200_000), "ctx 25.0% (50.0k/200.0k)");
        assert_eq!(format_context_usage(50_000, 0), "ctx 50.0k");
    }

    #[test]
    fn context_footer_matches_codex_pending_and_fallback_states() {
        assert_eq!(
            context_footer_text(Some(92), Some(6_575)).as_deref(),
            Some("92% context left")
        );
        assert_eq!(
            context_footer_text(None, Some(6_575)).as_deref(),
            Some("6.6K used")
        );
        assert_eq!(context_footer_text(None, None), None);
    }

    #[test]
    fn cost_uses_known_model_pricing_and_marks_fallbacks() {
        let model = api::resolve_catalog_alias(api::OPENAI_LATEST_MODEL_ALIAS);
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..TokenUsage::default()
        };
        assert_eq!(
            format_estimated_cost(&model, usage),
            format_cost_usd(estimated_cost_usd(&model, usage))
        );
        let fallback = format_estimated_cost("unknown-model", usage);
        assert_eq!(
            fallback,
            format!("~{}", format_cost_usd(estimated_cost_usd("unknown-model", usage)))
        );
        assert_eq!(format_cost_usd(0.0001), "$0.00");
        assert!(estimated_cost_usd(&model, usage) > 0.0);
    }

    #[test]
    fn usage_status_uses_input_side_context_not_output_tokens() {
        let model = api::resolve_catalog_alias(api::OPENAI_LATEST_MODEL_ALIAS);
        let usage = TokenUsage {
            input_tokens: 20,
            output_tokens: 8,
            cache_creation_input_tokens: 3,
            cache_read_input_tokens: 1,
            output_tokens_details: None,
        };
        assert_eq!(usage_context_tokens(usage), 24);
        let status = format_usage_status(&model, usage, 100);
        assert!(status.contains("ctx 24.0%"));
        assert!(status.contains("tokens 32"));
        assert!(status.contains("cost $"));
    }
}

/// `/status` 카드 — 근거는 `docs/captures/codex-tui-v0.150.1-status.bin` 이다.
/// 캡처 **바이트**와 나란히 놓는 핀은 `tests/tui_bytes.rs` 13번에 있고, 여기서는
/// 그 문법이 조립 쪽에서 지켜지는지를 잰다.
#[cfg(test)]
mod card_tests {
    use super::{
        card_text, format_reset_at, gauge, label_column, parse_utc_offset, status_card, StatusCard,
        GAUGE_CELLS,
    };
    use crate::tui::ansi::Line;
    use crate::usage::{UsageReport, UsageWindow};
    use crate::util::ansi::strip_ansi;

    fn window(label: &str, remaining: Option<u8>) -> UsageWindow {
        UsageWindow {
            label: label.to_string(),
            remaining_percent: remaining,
            resets_at_unix: None,
            estimated: false,
        }
    }

    fn report(windows: Vec<UsageWindow>) -> UsageReport {
        UsageReport {
            account: Some("someone@example.com".to_string()),
            windows,
        }
    }

    /// 캡처를 뜬 날 17:25 UTC.
    const NOW: u64 = 1_787_851_500;

    fn rows(usage: &UsageReport) -> Vec<Line> {
        card_rows(usage, None)
    }

    fn card_rows(usage: &UsageReport, dream: Option<&str>) -> Vec<Line> {
        status_card(&StatusCard {
            dream,
            product: "zo",
            version: "0.1.0",
            model: "claude-opus-5",
            effort: Some("high"),
            cwd: "/tmp/x",
            permissions: "workspace-write",
            permissions_pending: false,
            session_id: "01a04247-06ef-76b2-9fce-16cfdee620be",
            goal: "none",
            autonomous: "off",
            loops: "none",
            context_tokens: 0,
            context_limit: 1_000_000,
            usage,
            accounts: &[],
            now_unix: NOW,
            width: 120,
        })
    }

    /// `/status` 는 프로바이더마다 계정을 말한다 — 라벨이 있으면 라벨과 출처,
    /// 없으면 출처만. 창이 이름을 말해 주지 않았다고 계정을 지어내지 않는다.
    #[test]
    fn the_card_names_each_providers_account_and_where_it_came_from() {
        use crate::runtime_support::{AccountFacts, AccountOrigin};

        let accounts = vec![
            AccountFacts {
                provider: "anthropic",
                label: Some("work".to_string()),
                origin: AccountOrigin::IdeManaged,
            },
            AccountFacts {
                provider: "openai",
                label: None,
                origin: AccountOrigin::OwnLogin,
            },
        ];
        let usage = UsageReport {
            account: None,
            windows: Vec::new(),
        };
        let text = super::card_text(&super::status_card(&super::StatusCard {
            dream: None,
            product: "zo",
            version: "0.1.0",
            model: "claude-opus-5",
            effort: None,
            cwd: "/tmp/x",
            permissions: "workspace-write",
            permissions_pending: false,
            session_id: "s",
            goal: "none",
            autonomous: "off",
            loops: "none",
            context_tokens: 0,
            context_limit: 1_000_000,
            usage: &usage,
            accounts: &accounts,
            now_unix: NOW,
            width: 120,
        }));

        assert!(text.contains("Anthropic:"), "{text}");
        assert!(text.contains("work"), "{text}");
        assert!(text.contains("(IDE-managed)"), "{text}");
        assert!(text.contains("OpenAI:"), "{text}");
        assert!(text.contains("own login"), "{text}");
        assert!(
            !text.contains("Google:"),
            "a provider with no credentials got a row:\n{text}"
        );
    }

    /// 조립 전체를 한 번에 — 프로세스 안 덮어쓰기부터 카드의 바이트까지.
    ///
    /// 앞의 시험은 사실 한 벌을 손으로 만들어 넣는다. 이것은 `auth.reload` 가
    /// 앉히는 그 자리에서 시작해 `card_from_facts` 가 실제로 읽게 한다 — 배선이
    /// 끊기면 앞의 시험은 그대로 초록이고 이것만 빨강이 된다.
    #[test]
    fn a_switched_account_reaches_the_rendered_card() {
        let _lock = crate::test_env_lock();
        let managed = crate::support::temp_dir("status-card-account");
        std::fs::write(
            managed.join(".credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"unused-by-the-card","scopes":["user:inference"]}}"#,
        )
        .expect("managed credentials");
        let _launched_with = crate::support::EnvVarGuard::set("CLAUDE_CONFIG_DIR", Some(""));
        api::managed_account::clear();
        api::managed_account::apply(
            api::ManagedProvider::Anthropic,
            &api::ManagedAccountUpdate {
                label: Some("joe@example.com · Acme".to_string()),
                claude_config_dir: Some(managed.clone()),
                codex_home: None,
            },
        );

        let text = super::card_text(&super::card_from_facts(
            &super::StatusFacts {
                model: "claude-opus-5".to_string(),
                effort: None,
                cwd: "/tmp/x".to_string(),
                permissions: "workspace-write",
                permissions_pending: false,
                session_id: "s".to_string(),
                context_tokens: 0,
                goal: "none".to_string(),
                autonomous: "off".to_string(),
                loops: "none".to_string(),
            },
            120,
        ));

        assert!(text.contains("Anthropic:"), "{text}");
        assert!(text.contains("joe@example.com · Acme"), "{text}");
        assert!(text.contains("(IDE-managed)"), "{text}");
        api::managed_account::clear();
        std::fs::remove_dir_all(managed).ok();
    }

    /// 계정을 하나도 못 찾은 판의 카드에는 그 구역이 통째로 없다.
    #[test]
    fn a_card_with_no_accounts_grows_no_account_rows() {
        let usage = UsageReport {
            account: None,
            windows: Vec::new(),
        };
        let text = super::card_text(&rows(&usage));
        assert!(!text.contains("Anthropic:"), "{text}");
        assert!(!text.contains("OpenAI:"), "{text}");
        let _ = &usage;
    }

    /// 지금 쓰지 않는 모델의 창은 카드에 서지 않는다 — 계정 창과 같은 낱말이라
    /// 나란히 서면 같은 값이 두 번 찍힌 것처럼 읽힌다.
    #[test]
    fn another_models_window_stays_off_the_card() {
        let usage = UsageReport {
            account: None,
            windows: vec![
                UsageWindow {
                    label: "7d".to_string(),
                    remaining_percent: Some(20),
                    resets_at_unix: None,
                    estimated: false,
                },
                UsageWindow {
                    label: "Fable 7d".to_string(),
                    remaining_percent: Some(0),
                    resets_at_unix: None,
                    estimated: false,
                },
            ],
        };
        let rows: Vec<String> = rows(&usage).iter().map(Line::plain).collect();

        assert!(
            !rows.iter().any(|row| row.contains("Fable")),
            "claude-opus-5 세션에 Fable 창은 없다: {rows:#?}"
        );
        assert_eq!(
            rows.iter().filter(|row| row.contains("Weekly limit")).count(),
            1,
            "주간 창은 하나뿐이다: {rows:#?}"
        );
    }

    /// 지금 쓰는 모델의 창은 제 머리글 아래 선다.
    #[test]
    fn the_current_models_window_keeps_its_heading() {
        let usage = UsageReport {
            account: None,
            windows: vec![UsageWindow {
                label: "Opus 5h".to_string(),
                remaining_percent: Some(96),
                resets_at_unix: None,
                estimated: false,
            }],
        };
        let rows: Vec<String> = rows(&usage).iter().map(Line::plain).collect();

        assert!(
            rows.iter().any(|row| row.contains("Opus limit:")),
            "머리글이 선다: {rows:#?}"
        );
    }

    #[test]
    fn a_card_without_a_dreamer_pass_has_no_dreamer_row() {
        let rows: Vec<String> = card_rows(&report(Vec::new()), None)
            .iter()
            .map(Line::plain)
            .collect();

        assert!(
            !rows.iter().any(|row| row.contains("dreamer")),
            "패스가 없으면 줄을 접는다: {rows:#?}"
        );
    }

    /// 문안은 스스로를 밝히므로 라벨 열에 들어가지 않는다 — 들어가면 값 열이
    /// 빈 채로 라벨 폭만 넓어지고, 그만큼 모든 행의 값이 오른쪽으로 밀린다.
    #[test]
    fn the_dreamer_row_stands_outside_the_label_column() {
        let dream = "dreamer: promoted 2, unchanged 1 (2m ago): zo-gate, orc-verbs";
        let rows: Vec<String> = card_rows(&report(Vec::new()), Some(dream))
            .iter()
            .map(Line::plain)
            .collect();

        let row = row_with(&rows, "dreamer:");
        assert!(row.contains(dream), "문안 그대로 실린다: {row:?}");
        let model = row_with(&rows, "Model:");
        let value_column = model.find("claude-opus-5").expect("모델 값 열");
        let dream_column = row.find("dreamer:").expect("드리머 줄 시작");
        assert!(
            dream_column < value_column,
            "라벨 열 밖에서 시작한다: {dream_column} < {value_column}"
        );
    }

    fn plain(usage: &UsageReport) -> Vec<String> {
        rows(usage).iter().map(Line::plain).collect()
    }

    fn row_with<'a>(rows: &'a [String], needle: &str) -> &'a str {
        rows.iter()
            .find(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("no row holds {needle:?} in {rows:#?}"))
    }

    /// 캡처의 세 값이 방향(채운 칸 = 남은 양)과 반올림을 함께 못 박는다:
    /// 내림이면 93%가 18칸, 올림이면 52%가 11칸이라 셋 다 맞을 수 없다.
    #[test]
    fn the_gauge_fills_the_remaining_share_of_twenty_cells() {
        assert_eq!(GAUGE_CELLS, 20);
        assert_eq!(gauge(5), "[█░░░░░░░░░░░░░░░░░░░]");
        assert_eq!(gauge(93), "[███████████████████░]");
        assert_eq!(gauge(52), "[██████████░░░░░░░░░░]");
        assert_eq!(gauge(0), "[░░░░░░░░░░░░░░░░░░░░]");
        assert_eq!(gauge(100), "[████████████████████]");
        assert_eq!(gauge(200), gauge(100));
    }

    /// 캡처의 두 모양. 날짜는 **지역 날짜**라 오프셋이 경계를 함께 민다.
    #[test]
    fn the_reset_phrase_drops_the_date_only_on_the_same_day() {
        assert_eq!(format_reset_at(1_787_861_460, NOW, 0), "resets 20:11");
        assert_eq!(format_reset_at(1_788_294_120, NOW, 0), "resets 20:22 on 1 Sep");
        assert_eq!(format_reset_at(1_788_310_920, NOW, 0), "resets 01:02 on 2 Sep");
        // UTC 로는 이튿날 01:02 인 순간이 -08:00 에서는 아직 1 Sep 저녁이다.
        assert_eq!(
            format_reset_at(1_788_310_920, NOW, -8 * 3_600),
            "resets 17:02 on 1 Sep"
        );
        // 반대로 +09:00 에서는 기준 시각이 이미 이튿날이라 같은 날이 된다.
        assert_eq!(format_reset_at(1_787_861_460, NOW, 9 * 3_600), "resets 05:11");
    }

    #[test]
    fn the_utc_offset_probe_reads_only_the_four_digit_form() {
        assert_eq!(parse_utc_offset("+0900"), Some(32_400));
        assert_eq!(parse_utc_offset("-0330"), Some(-12_600));
        assert_eq!(parse_utc_offset("0000"), Some(0));
        assert_eq!(parse_utc_offset("+09:00"), None);
        assert_eq!(parse_utc_offset("+090"), None);
        assert_eq!(parse_utc_offset(""), None);
    }

    /// 라벨 열은 **행 목록**에서 나온다 — 캡처의 31칸째는 값 없는 머리글까지
    /// 세어야 나오는 값이다.
    #[test]
    fn the_label_column_comes_from_the_row_list_not_a_constant() {
        let captured = [
            "Model:",
            "Directory:",
            "Permissions:",
            "Agents.md:",
            "Account:",
            "Collaboration mode:",
            "Session:",
            "Context window:",
            "Weekly limit:",
            "GPT-5.3-Codex-Spark limit:",
            "5h limit:",
        ];
        assert_eq!(label_column(captured), 29);
        let without_header = captured
            .iter()
            .copied()
            .filter(|label| *label != "GPT-5.3-Codex-Spark limit:");
        assert_ne!(label_column(without_header), 29);
    }

    /// 사용량이 아직 없으면 한도 구역만 접힌다 — 모르는 것을 0% 게이지로
    /// 그리느니 줄이 없는 편이 낫다.
    #[test]
    fn an_empty_report_folds_the_limit_rows_and_keeps_the_rest() {
        let rows = plain(&UsageReport::default());
        assert!(rows[0].starts_with('╭') && rows[0].ends_with('╮'));
        assert!(rows.last().is_some_and(|row| row.starts_with('╰')));
        assert!(row_with(&rows, "Model:").contains("claude-opus-5 (reasoning high)"));
        assert!(row_with(&rows, "Context window:").contains("100% left (0 used / 1.0M)"));
        assert!(!rows.iter().any(|row| row.contains("limit:")));
        assert!(!rows.iter().any(|row| row.contains('[')));
        // 계정을 모르면 계정 줄도 없다.
        assert!(!rows.iter().any(|row| row.contains("Account:")));
    }

    /// W15 visible-state pin: a persistent goal and every autonomous limit are
    /// inspectable on the same `/status` card in both frontends.
    #[test]
    fn the_status_card_always_shows_goal_and_autonomous_state() {
        let rows = plain(&UsageReport::default());
        assert!(row_with(&rows, "Goal:").contains("none"));
        assert!(row_with(&rows, "Autonomous:").contains("off"));
    }

    /// 캡처의 배치 그대로: 계정 전체 창이 먼저, 그 뒤에 모델 머리글과 그 창들.
    /// 모델별 창은 **지금 쓰는 모델**의 것이라야 선다(`claude-opus-5` ← `Opus`)..
    #[test]
    fn model_scoped_windows_group_under_a_header_like_the_capture() {
        let rows = plain(&report(vec![
            window("7d", Some(5)),
            window("Opus 5h", Some(93)),
            window("Opus 7d", Some(52)),
        ]));
        assert!(row_with(&rows, "Account:").contains("someone@example.com"));
        assert!(row_with(&rows, "Weekly limit:").contains("[█░░░░░░░░░░░░░░░░░░░] 5% left"));
        // 머리글은 값이 없다 — 라벨에서 곧장 상자 여백으로 넘어간다.
        let header = row_with(&rows, "Opus limit:");
        assert!(header
            .trim_end_matches(['│', ' '])
            .ends_with("Opus limit:"), "the header row was {header:?}");
        assert!(row_with(&rows, "5h limit:").contains("[███████████████████░] 93% left"));

        // 값 열은 한 자리다 — 백분율이 우측 정렬이 아니므로 게이지의 시작이
        // 모든 창 줄에서 같아야 한다.
        let starts: Vec<Option<usize>> = rows
            .iter()
            .filter(|row| row.contains('['))
            .map(|row| row.find('['))
            .collect();
        assert!(starts.len() == 3 && starts.iter().all(|start| *start == starts[0]));
    }

    #[test]
    fn a_window_without_a_percentage_never_draws_a_zero_gauge() {
        let rows = plain(&report(vec![window("5h", None)]));
        let row = row_with(&rows, "5h limit:");
        assert!(row.contains("unknown"), "the row was {row:?}");
        assert!(!row.contains('['));
    }

    /// 429 로 **추정한** 줄은 그렇다고 말한다 — 엔진이 `(est)` 로 적어 둔 표기다.
    #[test]
    fn an_estimated_window_says_so_in_the_same_parenthetical() {
        let rows = plain(&report(vec![UsageWindow {
            label: "429".to_string(),
            remaining_percent: Some(0),
            resets_at_unix: None,
            estimated: true,
        }]));
        assert!(row_with(&rows, "Rate limit:").contains("0% left (est)"));
    }

    /// 두 프런트엔드가 같은 행을 낸다 — 파이프가 인쇄하는 바이트에서 SGR 만
    /// 걷어내면 TUI 히스토리의 평문과 글자 하나까지 같다.
    #[test]
    fn the_pipe_text_is_the_same_rows_as_the_tui_history() {
        let usage = report(vec![window("7d", Some(52))]);
        let rows = rows(&usage);
        let printed: Vec<String> = card_text(&rows)
            .lines()
            .map(strip_ansi)
            .collect();
        let history: Vec<String> = rows.iter().map(Line::plain).collect();
        assert_eq!(printed, history);
    }
}
