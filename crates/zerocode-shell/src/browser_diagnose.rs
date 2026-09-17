//! `zerocode-browser diagnose` — one paragraph from facts, in a fixed order.
//!
//! The facts are the window's to gather (`cmd::browser::automate_diagnose`:
//! the navigation record by the clock, the ring's document status, failed
//! requests and console errors, the page's visible words); the verdict is
//! this module's and pure, so the table below is tested without a webview.
//! Top wins — a dead server is the answer whatever the console says.

use super::*;

/// Words a page shows when it gates on the browser's NAME — or broke in the
/// way the name is the first suspect for (X's 「Something went wrong」 of
/// 2026-09-07, beside Atlassian's 「지원되지 않는 브라우저」).
pub(super) const UNSUPPORTED_BROWSER_WORDS: [&str; 5] = [
    "지원되지 않는 브라우저",
    "unsupported browser",
    "browser is not supported",
    "browser isn't supported",
    "something went wrong",
];

/// Words a login wall puts in its TITLE. The body is not read for these —
/// half the web has a "Sign in" link in its header.
pub(super) const LOGIN_WORDS: [&str; 6] =
    ["로그인", "log in", "login", "sign in", "sign-in", "signin"];

/// How many failed requests make a page "failing" rather than noisy: a lone
/// 404 favicon is not a diagnosis.
pub(super) const DIAGNOSE_FAILED_REQUESTS_MANY: usize = 3;

/// How many failed requests and console errors the answer names.
pub(super) const DIAGNOSE_TOP: usize = 3;

/// One failed request, as the ring recorded it — method, status or the
/// error's name, and the scrubbed address. Never a header or a body.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(super) struct FailedRequest {
    pub(super) method: String,
    pub(super) status: Option<u16>,
    pub(super) url: String,
    pub(super) error: Option<String>,
}

impl FailedRequest {
    fn said(&self) -> String {
        let status = match (self.status, &self.error) {
            (Some(status), _) => status.to_string(),
            (None, Some(error)) => format!("ERR:{error}"),
            (None, None) => "?".to_string(),
        };
        format!("{} {status} {}", self.method, self.url)
    }
}

/// Everything the verdict is read from. Every field is a fact somebody
/// recorded — a hook, the ring, the page's own text — never a guess.
#[derive(Debug, Clone, Default, Serialize)]
pub(super) struct PageFacts {
    pub(super) label: String,
    pub(super) url: String,
    /// `loading|finished|dead|blank` — the clock's reading of the record.
    pub(super) nav: String,
    /// How long the current load has stood, when it is still standing.
    pub(super) loading_secs: Option<u64>,
    /// The main document's own HTTP status, from the navigation timeline.
    pub(super) document_status: Option<u16>,
    pub(super) document_type: Option<String>,
    /// The agent the pane actually wears (reader, or Safari's name).
    pub(super) user_agent: String,
    pub(super) profile: Option<String>,
    pub(super) failed_requests: usize,
    pub(super) failed_top: Vec<FailedRequest>,
    pub(super) console_errors: usize,
    pub(super) console_top: Vec<String>,
    /// Whether the page's CSP refuses `eval` — the door's own eval is
    /// inlined and unaffected (§2.4); this is a fact about the page.
    pub(super) eval_blocked_by_csp: bool,
    pub(super) unsupported_browser_word: Option<String>,
    pub(super) login_word: Option<String>,
    pub(super) password_field: bool,
    /// A pane born before the ring existed has no counts to read.
    pub(super) ring_present: bool,
}

/// The table's rows, top first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Verdict {
    Dead,
    DocumentStatus,
    NameGated,
    LoginWall,
    RequestsFailing,
    ScriptErrors,
    Fine,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct Diagnosis {
    pub(super) verdict: Verdict,
    pub(super) said: String,
}

fn top_requests(facts: &PageFacts) -> String {
    facts
        .failed_top
        .iter()
        .take(DIAGNOSE_TOP)
        .map(FailedRequest::said)
        .collect::<Vec<_>>()
        .join(" · ")
}

fn top_errors(facts: &PageFacts) -> String {
    facts
        .console_top
        .iter()
        .take(DIAGNOSE_TOP)
        .cloned()
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The verdict, read top-down (§2.3): dead → document 4xx/5xx → name gating
/// → login wall → many failed requests → console errors → fine. Pure: the
/// same facts always say the same sentence.
pub(super) fn diagnosis(facts: &PageFacts) -> Diagnosis {
    if facts.nav == NavState::Dead.word() {
        let secs = facts.loading_secs.unwrap_or(NAV_DEAD_AFTER_SECS);
        return Diagnosis {
            verdict: Verdict::Dead,
            said: format!(
                "서버가 답하지 않았습니다 — {} 에 Started 뒤 {secs}초 동안 Finished 없음 \
                 (wry 가 아니라 시계의 판정, {NAV_DEAD_AFTER_SECS}초 규칙)",
                facts.url
            ),
        };
    }
    if let Some(status) = facts.document_status.filter(|status| *status >= 400) {
        return Diagnosis {
            verdict: Verdict::DocumentStatus,
            said: format!("문서 자체가 {status} — {}", facts.url),
        };
    }
    if let Some(word) = &facts.unsupported_browser_word {
        return Diagnosis {
            verdict: Verdict::NameGated,
            said: format!(
                "이름 게이팅 의심 — 페이지가 「{word}」라 말함 · UA {}",
                facts.user_agent
            ),
        };
    }
    if facts.password_field || facts.login_word.is_some() {
        let why = if facts.password_field {
            "비밀번호 입력란이 보임".to_string()
        } else {
            format!(
                "제목에 「{}」",
                facts.login_word.as_deref().unwrap_or_default()
            )
        };
        return Diagnosis {
            verdict: Verdict::LoginWall,
            said: format!(
                "로그인 필요 — {why} (프로필 {})",
                facts.profile.as_deref().unwrap_or("기본")
            ),
        };
    }
    if facts.failed_requests >= DIAGNOSE_FAILED_REQUESTS_MANY {
        return Diagnosis {
            verdict: Verdict::RequestsFailing,
            said: format!(
                "API 실패 {}건 — 상위: {}",
                facts.failed_requests,
                top_requests(facts)
            ),
        };
    }
    if facts.console_errors > 0 {
        return Diagnosis {
            verdict: Verdict::ScriptErrors,
            said: format!(
                "스크립트 오류 {}건 — 상위: {}",
                facts.console_errors,
                top_errors(facts)
            ),
        };
    }
    Diagnosis {
        verdict: Verdict::Fine,
        said: format!(
            "이상 없음 (문서 {}·실패 {}·오류 {})",
            facts
                .document_status
                .map_or("?".to_string(), |status| status.to_string()),
            facts.failed_requests,
            facts.console_errors
        ),
    }
}

/// The text answer: the verdict, then the facts it was read from — one
/// line each, so an agent can quote the one that matters.
pub(super) fn diagnose_words(facts: &PageFacts, diagnosis: &Diagnosis) -> String {
    let mut out = format!("{}\n", diagnosis.said);
    out.push_str(&format!(
        "- 항해: {}{} · 문서 {}{}\n",
        facts.nav,
        facts
            .loading_secs
            .map(|secs| format!(" ({secs}초)"))
            .unwrap_or_default(),
        facts
            .document_status
            .map_or("?".to_string(), |status| status.to_string()),
        facts
            .document_type
            .as_deref()
            .map(|kind| format!(" ({kind})"))
            .unwrap_or_default()
    ));
    out.push_str(&format!("- UA: {}\n", facts.user_agent));
    out.push_str(&format!(
        "- 프로필: {}\n",
        facts.profile.as_deref().unwrap_or("기본")
    ));
    out.push_str(&format!(
        "- 실패 요청: {}건{}\n",
        facts.failed_requests,
        if facts.failed_top.is_empty() {
            String::new()
        } else {
            format!(" — {}", top_requests(facts))
        }
    ));
    out.push_str(&format!(
        "- 콘솔 오류: {}건{}\n",
        facts.console_errors,
        if facts.console_top.is_empty() {
            String::new()
        } else {
            format!(" — {}", top_errors(facts))
        }
    ));
    out.push_str(if facts.eval_blocked_by_csp {
        "- eval: 페이지 CSP 가 eval 을 막음 — 문의 eval 은 인라인이라 답합니다\n"
    } else {
        "- eval: 페이지 CSP 가 eval 을 막지 않음\n"
    });
    if !facts.ring_present {
        out.push_str(
            "- 관측 링 없음 — 이 빌드 전에 연 판이거나 새로고침이 필요합니다 (요청·오류 수는 0 으로 읽힘)\n",
        );
    }
    out
}

/// `--json`: the facts and the verdict as they are. The facts are the page's
/// own words (its title, its console, its request URLs), so the object says
/// so under the fence's JSON key — the answer stays parseable, and a model
/// reading it is told whose words those are.
pub(super) fn diagnose_json(facts: &PageFacts, diagnosis: &Diagnosis) -> String {
    let mut answer = serde_json::json!({
        "facts": facts,
        "verdict": diagnosis.verdict,
        "said": diagnosis.said,
    });
    answer[zerocode_core::untrusted::JSON_FLAG] = serde_json::Value::Bool(true);
    serde_json::to_string_pretty(&answer).unwrap_or_else(|_| "{}".to_string())
}
