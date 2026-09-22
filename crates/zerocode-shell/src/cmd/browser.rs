//! Browser commands.

use crate::*;

/// The callback channel is shared by find/grab/menu and the automation
/// surface below. A page owns every byte it returns, so both the engine wait
/// and the amount accepted back into Rust are bounded here, once.
const BROWSER_CALLBACK_DEADLINE: Duration = Duration::from_secs(5);
const BROWSER_CALLBACK_CAP: usize = 256_000;
const BROWSER_READ_CAP: usize = 20_000;
const BROWSER_DOM_CAP: usize = 20_000;
pub(crate) const BROWSER_TITLE_CAP: usize = 500;
const BROWSER_URL_CAP: usize = 2_000;
const BROWSER_EVAL_CAP: usize = 64_000;
const BROWSER_EXPRESSION_CAP: usize = 64_000;
const BROWSER_SELECTOR_CAP: usize = 4_000;
const BROWSER_TYPE_CAP: usize = 100_000;
/// One `console`/`network` answer: the page-side `take` budget AND the cap
/// re-applied here. The ring itself holds 500 rows of up to 1,000 chars, so
/// a full read is several answers walked with `--since`.
pub(crate) const BROWSER_CONSOLE_CAP: usize = 20_000;
pub(crate) const BROWSER_NETWORK_CAP: usize = 20_000;
/// How long `wait` watches, at most, at least, and how often: the core's
/// browser table (`agent_browser`), which a recipe walk budgets by too.
pub(crate) use zerocode_core::agent_browser::{
    BROWSER_WAIT_DEFAULT_MS, BROWSER_WAIT_MAX_MS, BROWSER_WAIT_MIN_MS,
};
pub(crate) const BROWSER_WAIT_POLL: Duration =
    Duration::from_millis(zerocode_core::agent_browser::BROWSER_WAIT_POLL_MS);
/// The one badge renderer the desktop look and the browser marks share, and
/// the picture it paints on (`computer_use`), reached here so `screenshot
/// --marks` draws with no code of its own.
use crate::computer_use::marks::draw_badges;
use crate::computer_use::screenshot_png::RgbaImage;
/// `wait`'s refusal when the selector never showed — a walk reads it apart
/// from a dead pane or a wrong label (a Flow's baseline and oracle).
pub(crate) const WAIT_TIMED_OUT: &str = "시간 안에 셀렉터가 보이지 않았습니다";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserReadReport {
    pub(crate) title: String,
    pub(crate) url: String,
    pub(crate) text: String,
    /// A selector read returns a deliberately reduced DOM copy. Live form
    /// values, script bodies and non-whitelisted attributes never cross.
    pub(crate) dom: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserInputReport {
    /// `dom-activation`, `editing-command`, `synthetic-events`, or the stdin
    /// road's `value-setter`.
    pub(crate) method: String,
    /// Whether the path is expected to produce engine-trusted input events.
    pub(crate) trusted_events: bool,
    /// A fixed, credential-free statement of what the path cannot promise.
    pub(crate) limitation: Option<String>,
    /// What a click pressed: the element's rectangle in CSS pixels
    /// `[x, y, width, height]`, measured once by the script that pressed it
    /// (plan D11) — and the page's device pixel ratio, so a walk can ring
    /// it on the pane's frame.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) rect: Option<[f64; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) dpr: Option<f64>,
    /// Where the pressed or typed element sits among the page's landmarks —
    /// the chain the read seat's blocks are addressed by
    /// (`zerocode_core::browser_read::inside`), so the window can say
    /// whether the press landed in a block a read folded away
    /// (`crate::browser_read::note_press`). Never printed: it is the
    /// label's fact, not the agent's.
    #[serde(skip)]
    pub(crate) block_path: Option<String>,
    /// The page's address at the press, cleared of credentials by the same
    /// page-side rule the read's address is — so a press after a navigation
    /// is not charged to a read of the page before it. Never printed.
    #[serde(skip)]
    pub(crate) page_url: Option<String>,
}

/// The sentence the door answers a click or a typing with: what it did, then
/// the input path's facts in one parenthesis — `method`, `trusted-events`,
/// and for a press the rect and ratio — and the path's limitation after.
/// `pressed_rect` reads the rect back from this very sentence: the walk's
/// browser step sees the door's words, not the report.
pub(crate) const CLICK_SAID: &str = "클릭 이벤트를 보냈습니다";
pub(crate) const TYPED_SAID: &str = "입력했습니다";
const INPUT_RECT_KEY: &str = "rect";
const INPUT_DPR_KEY: &str = "dpr";
/// The key the click and type scripts answer the element's landmark chain
/// under (`zcStructuralChain`).
const INPUT_BLOCK_PATH_KEY: &str = "blockPath";
/// The key they answer the page's address under, cleared page-side.
const INPUT_PAGE_URL_KEY: &str = "pageUrl";

/// The selectors a page is cut into blocks at, as the page scripts take
/// them: the read seat's own list (`zerocode_core::jev::BROWSER_READ_BLOCK_ROOTS`),
/// joined for `matches`. One producer, so the read that cuts and the press
/// that names its block read the same list.
pub(crate) fn block_roots() -> String {
    zerocode_core::jev::BROWSER_READ_BLOCK_ROOTS.join(",")
}
/// Between the facts in the parenthesis (a rect's sides are joined by a
/// bare comma, so the separator is the comma and a space).
const INPUT_FACT_SEPARATOR: &str = ", ";

pub(crate) fn input_said(what: &str, report: &BrowserInputReport) -> String {
    let mut facts = format!(
        "method={}{INPUT_FACT_SEPARATOR}trusted-events={}",
        report.method, report.trusted_events
    );
    if let (Some([x, y, width, height]), Some(dpr)) = (report.rect, report.dpr) {
        facts.push_str(&format!(
            "{INPUT_FACT_SEPARATOR}{INPUT_RECT_KEY}={x},{y},{width},{height}{INPUT_FACT_SEPARATOR}{INPUT_DPR_KEY}={dpr}"
        ));
    }
    let limitation = report
        .limitation
        .as_deref()
        .map(|limitation| format!(" — {limitation}"))
        .unwrap_or_default();
    format!("{what} ({facts}){limitation}")
}

/// The rect and ratio a click's sentence carries, if it carries them.
#[must_use]
pub(crate) fn pressed_rect(said: &str) -> Option<([f64; 4], f64)> {
    let (_, rest) = said.split_once('(')?;
    let (facts, _) = rest.split_once(')')?;
    let (mut rect, mut dpr) = (None, None);
    for fact in facts.split(INPUT_FACT_SEPARATOR) {
        if let Some(sides) = fact
            .strip_prefix(INPUT_RECT_KEY)
            .and_then(|rest| rest.strip_prefix('='))
        {
            let sides: Vec<f64> = sides
                .split(',')
                .map(str::parse)
                .collect::<Result<_, _>>()
                .ok()?;
            rect = <[f64; 4]>::try_from(sides).ok();
        } else if let Some(ratio) = fact
            .strip_prefix(INPUT_DPR_KEY)
            .and_then(|rest| rest.strip_prefix('='))
        {
            dpr = ratio.parse().ok();
        }
    }
    Some((rect?, dpr?))
}

/// Which road a `type` writes by (review 8, plan D7): its keys — WebKit's
/// editing command, else the synthetic events — refused before a password
/// field; or the value read from stdin, written by the prototype setter
/// alone, no editing command and no read-back, the one road into a
/// password field the person gave the secret for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TypeRoad {
    Keys,
    Setter,
}

impl TypeRoad {
    /// The word the page script reads the road by.
    const fn word(self) -> &'static str {
        match self {
            Self::Keys => "keys",
            Self::Setter => "setter",
        }
    }

    /// The methods a page may answer on this road.
    const fn methods(self) -> &'static [&'static str] {
        match self {
            Self::Keys => &["editing-command", "synthetic-events"],
            Self::Setter => &["value-setter"],
        }
    }
}

/// The failure ladder's words, named so a caller can tell one rung from
/// another without formatting anything the engine said (see `automate_eval`).
pub(crate) const PAGE_SEND_FAILED: &str = "브라우저 판에 자동화 명령을 보낼 수 없습니다";
pub(crate) const PAGE_TIMED_OUT: &str = "브라우저 판이 시간 안에 답하지 않았습니다";
pub(crate) const PAGE_ANSWER_UNREADABLE: &str = "브라우저 판의 답을 읽을 수 없습니다";

/// The one JS -> Rust round trip used throughout this module.
///
/// `eval_with_callback` serializes the JS result once; scripts here normally
/// return JSON themselves, so WebKit can wrap that string a second time. The
/// decoder accepts both shapes, exactly as grab did before this helper was
/// factored out. Raw page text is never included in an error.
async fn page_json(
    pane: &BrowserPane,
    script: impl Into<String>,
    deadline: Duration,
) -> Result<serde_json::Value, String> {
    let (said, heard) = tokio::sync::oneshot::channel::<String>();
    let holder = std::sync::Mutex::new(Some(said));
    pane.eval_with_callback(script, move |result| {
        if let Some(said) = holder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = said.send(result);
        }
    })
    // Some platform errors can carry the evaluated script. An eval or type
    // payload can itself contain a credential, so this boundary speaks only
    // a fixed failure and never formats the engine error.
    .map_err(|_| PAGE_SEND_FAILED.to_string())?;
    let raw = tokio::time::timeout(deadline, heard)
        .await
        .map_err(|_| PAGE_TIMED_OUT.to_string())?
        .map_err(|_| "브라우저 판이 답하지 않았습니다".to_string())?;
    if raw.len() > BROWSER_CALLBACK_CAP {
        return Err("브라우저 판의 답이 너무 큽니다".to_string());
    }
    decode_page_json(&raw)
}

pub(crate) fn decode_page_json(raw: &str) -> Result<serde_json::Value, String> {
    serde_json::from_str(raw)
        .ok()
        .and_then(|outer: serde_json::Value| match outer {
            serde_json::Value::String(inner) => serde_json::from_str(&inner).ok(),
            held => Some(held),
        })
        .ok_or_else(|| PAGE_ANSWER_UNREADABLE.to_string())
}

pub(crate) fn terminal_safe(value: &str, cap: usize) -> String {
    value
        .chars()
        .filter(|ch| *ch == '\n' || *ch == '\t' || (*ch as u32 >= 32 && *ch as u32 != 127))
        .take(cap)
        .collect()
}

pub(crate) fn scrub_url_credentials(url: &str) -> String {
    let Ok(mut parsed) = url.parse::<tauri::Url>() else {
        return terminal_safe(url, BROWSER_URL_CAP);
    };
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    let mut changed = false;
    let pairs: Vec<(String, String)> = parsed
        .query_pairs()
        .map(|(key, value)| {
            let value = if sensitive_name(&key) {
                changed = true;
                "[redacted]".to_string()
            } else {
                value.into_owned()
            };
            (key.into_owned(), value)
        })
        .collect();
    if changed {
        parsed.query_pairs_mut().clear().extend_pairs(pairs);
    }
    let redacted_fragment = parsed.fragment().and_then(|fragment| {
        let mut changed = false;
        let pairs: Vec<(String, String)> = url::form_urlencoded::parse(fragment.as_bytes())
            .map(|(key, value)| {
                let value = if sensitive_name(&key) {
                    changed = true;
                    "[redacted]".to_string()
                } else {
                    value.into_owned()
                };
                (key.into_owned(), value)
            })
            .collect();
        changed.then(|| {
            url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(pairs)
                .finish()
        })
    });
    if let Some(fragment) = redacted_fragment {
        parsed.set_fragment(Some(&fragment));
    }
    terminal_safe(parsed.as_str(), BROWSER_URL_CAP)
}

fn sensitive_name(name: &str) -> bool {
    let folded: String = name
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    [
        "authorization",
        "credential",
        "password",
        "passwd",
        "cookie",
        "secret",
        "sessionid",
        "token",
        "apikey",
        "privatekey",
    ]
    .iter()
    .any(|word| folded.contains(word))
}

pub(crate) fn sanitize_eval_value(value: serde_json::Value, depth: usize) -> serde_json::Value {
    if depth >= 32 {
        return serde_json::Value::String("[depth limit]".to_string());
    }
    match value {
        serde_json::Value::String(text) => {
            let text = terminal_safe(&text, BROWSER_EVAL_CAP);
            let lower = text.trim_start().to_ascii_lowercase();
            if looks_like_credential(&lower) {
                serde_json::Value::String("[redacted]".to_string())
            } else if text.parse::<tauri::Url>().is_ok() {
                serde_json::Value::String(scrub_url_credentials(&text))
            } else {
                serde_json::Value::String(text)
            }
        }
        serde_json::Value::Array(values) => serde_json::Value::Array(
            values
                .into_iter()
                .map(|value| sanitize_eval_value(value, depth + 1))
                .collect(),
        ),
        serde_json::Value::Object(values) => serde_json::Value::Object(
            values
                .into_iter()
                .map(|(key, value)| {
                    let value = if sensitive_name(&key) {
                        serde_json::Value::String("[redacted]".to_string())
                    } else {
                        sanitize_eval_value(value, depth + 1)
                    };
                    (terminal_safe(&key, 500), value)
                })
                .collect(),
        ),
        held => held,
    }
}

fn looks_like_credential(value: &str) -> bool {
    if [
        "bearer ",
        "basic ",
        "sk-",
        "ghp_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
    ]
    .iter()
    .any(|prefix| value.starts_with(prefix))
    {
        return true;
    }
    let parts: Vec<&str> = value.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            part.len() >= 8
                && part
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '='))
        })
}

/// The code a password field's typing is refused by: the core's own word.
const PERSONS_ENTRY: &str =
    zerocode_core::computer_use_protocol::validate::SecretEntryVerdict::PersonsEntry.as_str();

pub(crate) fn page_failure(reply: &serde_json::Value) -> String {
    match reply
        .get("code")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
    {
        PERSONS_ENTRY => {
            return format!(
                "비밀번호 칸입니다 — 사람이 입력합니다; 사용자가 이 칸의 비밀을 주었을 때만 `type <label> <css> {}`로 씁니다",
                zerocode_core::agent_browser::TYPE_VALUE_STDIN_FLAG
            );
        }
        "invalid_selector" => "셀렉터가 올바르지 않습니다",
        "selector_not_found" => "셀렉터와 맞는 요소가 없습니다",
        "element_not_visible" => "고른 요소가 보이지 않습니다",
        "element_obscured" => "고른 요소가 다른 요소에 가려져 있습니다",
        "element_disabled" => "고른 요소가 비활성화되어 있습니다",
        "element_read_only" => "고른 요소는 읽기 전용입니다",
        "element_not_editable" => "고른 요소에는 글을 입력할 수 없습니다",
        "input_cancelled" => "페이지가 입력을 거부했습니다",
        "text_too_long" => "입력 글이 요소의 최대 길이를 넘습니다",
        "async_value" => "비동기 값은 이 eval 왕복에서 돌려줄 수 없습니다",
        "evaluation_failed" => "페이지 식을 평가하지 못했습니다",
        "serialization_failed" => "페이지 식의 값을 직렬화할 수 없습니다",
        "result_too_large" => "페이지 식의 값이 너무 큽니다",
        "ring_missing" => {
            "이 페이지에 관측 링이 없습니다 — 이 빌드 전에 연 판이거나 새로고침이 필요합니다"
        }
        _ => "페이지 자동화가 실패했습니다",
    }
    .to_string()
}

pub(crate) fn page_value(reply: serde_json::Value) -> Result<serde_json::Value, String> {
    if reply.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err(page_failure(&reply));
    }
    Ok(reply
        .get("value")
        .cloned()
        .unwrap_or(serde_json::Value::Null))
}

pub(crate) fn input_report(
    value: serde_json::Value,
    expected: &[&str],
) -> Result<BrowserInputReport, String> {
    let method = value
        .get("method")
        .and_then(serde_json::Value::as_str)
        .filter(|method| expected.contains(method))
        .ok_or_else(|| "페이지 입력 결과를 읽을 수 없습니다".to_string())?;
    let (trusted_events, limitation) = match method {
        "dom-activation" => (
            false,
            Some(
                "DOM 클릭 이벤트는 isTrusted 검사를 요구하는 페이지에서 거부될 수 있습니다"
                    .to_string(),
            ),
        ),
        "editing-command" => (true, None),
        "synthetic-events" => (
            false,
            Some(
                "합성 input fallback은 isTrusted 검사를 요구하는 페이지에서 거부될 수 있습니다"
                    .to_string(),
            ),
        ),
        "value-setter" => (
            false,
            Some(
                "값 setter는 되읽기 없이 쓰며 isTrusted 검사를 요구하는 페이지에서 거부될 수 있습니다"
                    .to_string(),
            ),
        ),
        _ => unreachable!("expected filters every other method"),
    };
    let rect = value
        .get(INPUT_RECT_KEY)
        .cloned()
        .and_then(|rect| serde_json::from_value::<[f64; 4]>(rect).ok());
    let dpr = value.get(INPUT_DPR_KEY).and_then(serde_json::Value::as_f64);
    let block_path = value
        .get(INPUT_BLOCK_PATH_KEY)
        .and_then(serde_json::Value::as_str)
        .map(|path| terminal_safe(path, zerocode_core::jev::BROWSER_READ_PATH_CHAR_CAP));
    let page_url = value
        .get(INPUT_PAGE_URL_KEY)
        .and_then(serde_json::Value::as_str)
        .map(|url| scrub_url_credentials(&terminal_safe(url, BROWSER_URL_CAP)));
    Ok(BrowserInputReport {
        method: method.to_string(),
        trusted_events,
        limitation,
        rect,
        dpr,
        block_path,
        page_url,
    })
}

/// A typing's answer judged: the page said whether the field is a password
/// field; the core's one secret-entry table (`secret_entry`) says whether
/// keys may write there — never on the keys road — and the stdin road is
/// the person's own word that this secret is for this field.
pub(crate) fn typed_report(
    value: serde_json::Value,
    road: TypeRoad,
) -> Result<BrowserInputReport, String> {
    use zerocode_core::computer_use_protocol::validate::{
        SecretEntryFacts, SecretEntryVerdict, secret_entry,
    };
    if road == TypeRoad::Keys {
        let secure_field = value
            .get("secureField")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let verdict = secret_entry(SecretEntryFacts {
            writes_text: true,
            pastes: false,
            secure_field,
            secure_input_on: false,
            secure_input_by_receiver: false,
            focus_read: true,
        });
        if verdict == SecretEntryVerdict::PersonsEntry {
            return Err(page_failure(
                &serde_json::json!({ "code": verdict.as_str() }),
            ));
        }
    }
    input_report(value, road.methods())
}

fn checked_selector(selector: &str) -> Result<(), String> {
    if selector.trim().is_empty() || selector.chars().count() > BROWSER_SELECTOR_CAP {
        Err("셀렉터는 1~4000자여야 합니다".to_string())
    } else {
        Ok(())
    }
}

/// Shared, page-side primitives for the five automation commands.
///
/// This is WKWebView JavaScript, not CDP. In particular a page script cannot
/// forge `Event.isTrusted`; click uses the element's browser activation and
/// type prefers WebKit's editing command, with a standards-event fallback.
/// The distinction is kept explicit instead of calling this a Chrome road.
pub(crate) const BROWSER_AUTOMATION_HELPERS: &str = r#"
const zcSensitive = (name) => {
  const folded = String(name || "").toLowerCase().replace(/[^a-z0-9]/g, "");
  return ["authorization", "credential", "password", "passwd", "cookie",
    "secret", "sessionid", "token", "apikey", "privatekey"]
    .some((word) => folded.includes(word));
};
const zcEncode = (answer, cap = 64000) => {
  try {
    const seen = new WeakSet();
    const encoded = JSON.stringify(answer, function(key, value) {
      if (key && zcSensitive(key)) return "[redacted]";
      if (typeof value === "bigint") return { type: "bigint", value: String(value) };
      if (typeof value === "function" || typeof value === "symbol") {
        return { type: typeof value };
      }
      if (value && typeof value === "object") {
        if (seen.has(value)) return "[circular]";
        seen.add(value);
      }
      return value;
    });
    if (encoded.length > cap) return JSON.stringify({ ok: false, code: "result_too_large" });
    return encoded;
  } catch (_) {
    return JSON.stringify({ ok: false, code: "serialization_failed" });
  }
};
const zcFail = (code) => zcEncode({ ok: false, code });
// The element a selector names is the first one a person could SEE, not the
// first in the document: a page that keeps a hidden twin of its form (a
// responsive layout, a template) puts the twin first, and a door that took it
// clicked nothing, typed into a field nobody reads, and waited for a control
// that was on screen the whole time (a company admin, 2026-09-15: `button.btn-info`
// twice, the first `display: none`). When every match is hidden the first one
// is answered with `hidden`, so a caller can still say "not visible" about it.
const zcSelect = (selector) => {
  let matches;
  try {
    matches = document.querySelectorAll(selector);
  } catch (_) {
    return { code: "invalid_selector" };
  }
  if (!matches.length) return { code: "selector_not_found" };
  for (const element of matches) {
    if (zcVisible(element)) return { element };
  }
  return { element: matches[0], hidden: true };
};
const zcVisible = (element) => {
  if (!element || !element.isConnected || element.hidden) return false;
  const style = getComputedStyle(element);
  if (style.display === "none" || style.visibility === "hidden" || Number(style.opacity) === 0) {
    return false;
  }
  const rect = element.getBoundingClientRect();
  return rect.width > 0 && rect.height > 0;
};
const zcSafeUrl = (raw) => {
  try {
    const url = new URL(String(raw), location.href);
    url.username = "";
    url.password = "";
    for (const key of [...url.searchParams.keys()]) {
      if (zcSensitive(key)) url.searchParams.set(key, "[redacted]");
    }
    if (url.hash) {
      const hash = new URLSearchParams(url.hash.slice(1));
      let changed = false;
      for (const key of [...hash.keys()]) {
        if (zcSensitive(key)) {
          hash.set(key, "[redacted]");
          changed = true;
        }
      }
      if (changed) url.hash = hash.toString();
    }
    return url.href;
  } catch (_) {
    return "";
  }
};
const zcDom = (element) => {
  const clone = element.cloneNode(true);
  if (clone.matches && clone.matches("script, style, noscript, template")) return "";
  const all = [clone, ...clone.querySelectorAll("*")];
  for (const node of all) {
    if (node.matches && node.matches("script, style, noscript, template")) {
      node.remove();
      continue;
    }
    for (const attribute of [...(node.attributes || [])]) {
      const name = attribute.name.toLowerCase();
      const keep = ["id", "class", "name", "role", "type", "title", "placeholder", "alt"]
        .includes(name) || name.startsWith("aria-") || ["href", "src", "action"].includes(name);
      if (!keep || zcSensitive(name) || name === "value") {
        node.removeAttribute(attribute.name);
      } else if (["href", "src", "action"].includes(name)) {
        node.setAttribute(attribute.name, zcSafeUrl(attribute.value));
      }
    }
    if (node.matches && node.matches("input, textarea, select, option")) {
      node.removeAttribute("value");
      if (node.matches("textarea")) node.textContent = "";
      if ("selected" in node) node.selected = false;
      if ("checked" in node) node.checked = false;
    }
  }
  return String(clone.outerHTML || "");
};
// A structural element's word in a block path (the read seat's cut,
// `zerocode_core::browser_read`): its tag, its id, its first class and its
// role — each kept to the characters a selector could carry — then its
// index among structural siblings of the same tag when there is more than
// one, so two `section`s under `main` are two addresses.
const zcPathAtom = (raw) => String(raw || "").trim().replace(/[^A-Za-z0-9_-]/g, "").slice(0, 24);
const zcTagWord = (node, roots) => {
  let word = String(node.tagName || "").toLowerCase();
  const id = zcPathAtom(node.id);
  if (id) word += "\x23" + id;
  const first = zcPathAtom(String(node.getAttribute("class") || "").trim().split(/\s+/)[0]);
  if (first) word += "." + first;
  const role = zcPathAtom(String(node.getAttribute("role") || "").toLowerCase());
  if (role) word += "[role=" + role + "]";
  const parent = node.parentElement;
  if (parent) {
    let nth = 0, alike = 0;
    for (const sibling of parent.children) {
      if (sibling.tagName !== node.tagName || !sibling.matches(roots)) continue;
      alike += 1;
      if (sibling === node) nth = alike;
    }
    if (alike > 1) word += "[" + nth + "]";
  }
  return word;
};
// Where an element sits among the page's landmarks: `body`, then the word of
// every structural ancestor from the outside in. The read's blocks are cut
// at the same list and addressed by the same words, so the block a press
// landed in is the block whose path this chain names (`browser_read::inside`).
const zcStructuralChain = (element, roots) => {
  if (!roots || !roots.length) return null;
  const parts = [];
  let node = element;
  while (node && node !== document.body && node.nodeType === 1) {
    if (node.matches(roots)) parts.unshift(zcTagWord(node, roots));
    node = node.parentElement;
  }
  return ["body", ...parts].join(">");
};
"#;

pub(crate) fn automation_script(request: &serde_json::Value, body: &str) -> String {
    let request = serde_json::to_string(request).unwrap_or_else(|_| "null".to_string());
    [
        "(() => {\n",
        BROWSER_AUTOMATION_HELPERS,
        "\nconst request = ",
        &request,
        ";\ntry {\n",
        body,
        "\n} catch (_) { return zcFail(\"evaluation_failed\"); }\n})()",
    ]
    .concat()
}

#[tauri::command(async)]
pub(crate) fn browser_profiles(
    webview: tauri::Webview,
    state: State<'_, AppState>,
) -> Result<Vec<BrowserProfileView>, String> {
    from_the_main_webview(&webview)?;
    Ok(browser_profile_views(state.config_root()))
}

#[tauri::command(async)]
pub(crate) fn create_browser_profile(
    webview: tauri::Webview,
    state: State<'_, AppState>,
    name: String,
) -> Result<BrowserProfile, String> {
    from_the_main_webview(&webview)?;
    let name = name.trim().to_string();
    if name.is_empty() || name.chars().count() > 40 {
        return Err("프로필 이름은 1~40자입니다".to_string());
    }
    let mut held = load_browser_profiles(state.config_root());
    if held.iter().any(|profile| profile.name == name) {
        return Err("같은 이름의 프로필이 이미 있습니다".to_string());
    }
    // The bridge's own randomness door (hooks::random_token) mints 24 bytes;
    // a store identifier is exactly 16, so the file is read the same way.
    let mut bytes = [0u8; 16];
    {
        use std::io::Read;
        std::fs::File::open("/dev/urandom")
            .and_then(|mut urandom| urandom.read_exact(&mut bytes))
            .map_err(|error| format!("프로필 id를 만들 수 없습니다: {error}"))?;
    }
    let id: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let made = BrowserProfile {
        id,
        name,
        source: None,
    };
    held.push(made.clone());
    save_browser_profiles(state.config_root(), &held)?;
    Ok(made)
}

#[tauri::command(async)]
pub(crate) fn delete_browser_profile(
    webview: tauri::Webview,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    from_the_main_webview(&webview)?;
    remove_browser_profile(state.config_root(), &id)
}

#[tauri::command(async)]
pub(crate) fn set_browser_default_profile(
    webview: tauri::Webview,
    state: State<'_, AppState>,
    id: Option<String>,
) -> Result<(), String> {
    from_the_main_webview(&webview)?;
    choose_default_browser_profile(state.config_root(), id.as_deref())
}

/// 이 기계에 설치된 소스 브라우저들 — 가져오기 메뉴의 재료 (지시서 C2,
/// Orca `detectInstalledBrowsers`).
#[tauri::command(async)]
pub(crate) fn cookie_sources(
    webview: tauri::Webview,
) -> Result<Vec<browser_cookie_import::CookieSource>, String> {
    from_the_main_webview(&webview)?;
    let home = dirs::home_dir().ok_or("홈 디렉터리가 없습니다")?;
    Ok(browser_cookie_import::detect_installed_browsers(&home))
}

/// 한 소스 브라우저의 쿠키를 읽어 대상 프로필의 스테이징에 앉힌다 —
/// 주입은 그 프로필의 판이 태어나는 순간(open_browser_pane) 일어난다.
/// blocking 스레드에서: 키체인 프롬프트와 SQLite 사본은 UI의 시간이 아니다.
#[tauri::command]
pub(crate) async fn import_browser_cookies(
    webview: tauri::Webview,
    state: State<'_, AppState>,
    family: String,
    profile: Option<String>,
    target: String,
) -> Result<BrowserCookieImportSummary, String> {
    from_the_main_webview(&webview)?;
    // 대상은 우리가 mint한 프로필이어야 한다 — id의 모양 검증이 그 담장이다.
    if browser_profile_store(&target).is_none() {
        return Err("프로필 id가 아닙니다".into());
    }
    let config_root = state.config_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let home = dirs::home_dir().ok_or_else(|| "홈 디렉터리가 없습니다".to_string())?;
        let source_profile = profile.as_deref().unwrap_or("Default");
        let scratch =
            std::env::temp_dir().join(format!("zerocode-cookie-import-{}", std::process::id()));
        let outcome = match family.as_str() {
            "safari" => {
                let path = browser_cookie_import::cookie_database_path("safari", "", &home)
                    .ok_or_else(|| "Safari 쿠키 파일이 없습니다".to_string())?;
                browser_cookie_import::read_safari_cookies(&path)?
            }
            "firefox" => {
                let source =
                    browser_cookie_import::cookie_database_path("firefox", source_profile, &home)
                        .ok_or_else(|| "Firefox 쿠키 파일이 없습니다".to_string())?;
                let copy = browser_cookie_import::snapshot_sqlite(&source, &scratch)?;
                let read = browser_cookie_import::read_firefox_cookies(&copy);
                let _ = std::fs::remove_dir_all(&scratch);
                read?
            }
            _ => {
                let source =
                    browser_cookie_import::cookie_database_path(&family, source_profile, &home)
                        .ok_or_else(|| "이 프로필의 쿠키 파일이 없습니다".to_string())?;
                // 열쇠 의식이 먼저다. 가져오기는 명시적 사용자 행위라 키체인
                // 프롬프트가 자연스럽다 — Orca는 암호화 행이 있을 때만 묻지만
                // (`needsSourceKey`), 평문-전용 Chromium DB는 실제로 없다시피
                // 해서 여기서는 단순한 쪽을 골랐다.
                let keys = browser_cookie_import::obtain_keys(&family)?;
                let copy = browser_cookie_import::snapshot_sqlite(&source, &scratch)?;
                let read = browser_cookie_import::read_chromium_cookies(&copy, &keys);
                let _ = std::fs::remove_dir_all(&scratch);
                read?
            }
        };
        let mut domains: Vec<String> = outcome
            .cookies
            .iter()
            .filter_map(|cookie| browser_cookies::normalize_cookie_domain(&cookie.domain))
            .collect();
        domains.sort();
        domains.dedup();
        browser_cookie_import::save_staged_cookies(&config_root, &target, &outcome.cookies)?;
        remember_cookie_source(&config_root, &target, &family, profile);
        let undecryptable = outcome.app_bound + outcome.decrypt_failed;
        Ok(BrowserCookieImportSummary {
            total_cookies: outcome.total,
            imported_cookies: outcome.cookies.len(),
            skipped_cookies: outcome.total - outcome.cookies.len(),
            google_cookies_skipped: outcome.google_skipped + outcome.non_transplantable_skipped,
            partition_skipped_cookies: outcome.partition_skipped,
            domains,
            warning: (undecryptable > 0).then_some(BrowserCookieImportWarning {
                code: "cookies-undecryptable",
                failed_cookies: undecryptable,
            }),
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 파일(Netscape cookies.txt)에서 쿠키를 읽어 대상 프로필 스테이징에 앉힌다 —
/// 원본 `importCookiesToProfile`의 우리 판. 평문 파일이라 키체인 의식이 없고,
/// 파일 선택은 다른 피커와 같은 한 자리(`pick_paths`, 헬퍼 프로세스)를 지난다.
/// 취소는 실패가 아니라 `None`이다(원본의 `reason === "canceled"`).
#[tauri::command]
pub(crate) async fn import_cookie_file(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    target: String,
) -> Result<Option<BrowserCookieImportSummary>, String> {
    from_the_main_webview(&webview)?;
    // 대상은 우리가 mint한 프로필이어야 한다 — id의 모양 검증이 그 담장이다.
    if browser_profile_store(&target).is_none() {
        return Err("프로필 id가 아닙니다".into());
    }
    let mut picked = pick_paths(
        &app,
        zerocode_core::pick::PickRequest::new(zerocode_core::pick::PickKind::File)
            .filtered("Netscape cookies.txt", &["txt"]),
    )
    .await?;
    let Some(path) = picked.pop() else {
        return Ok(None);
    };
    let config_root = state.config_root().to_path_buf();
    let summary = tauri::async_runtime::spawn_blocking(move || {
        let outcome = browser_cookie_import::read_netscape_cookies(&path)?;
        let mut domains: Vec<String> = outcome
            .cookies
            .iter()
            .filter_map(|cookie| browser_cookies::normalize_cookie_domain(&cookie.domain))
            .collect();
        domains.sort();
        domains.dedup();
        browser_cookie_import::save_staged_cookies(&config_root, &target, &outcome.cookies)?;
        remember_file_cookie_source(&config_root, &target);
        Ok::<_, String>(BrowserCookieImportSummary {
            total_cookies: outcome.total,
            imported_cookies: outcome.cookies.len(),
            skipped_cookies: outcome.total - outcome.cookies.len(),
            google_cookies_skipped: outcome.google_skipped + outcome.non_transplantable_skipped,
            partition_skipped_cookies: outcome.partition_skipped,
            domains,
            // 평문 파일엔 복호 실패라는 것이 없다 — 경고 칸은 비운다.
            warning: None,
        })
    })
    .await
    .map_err(|error| error.to_string())??;
    Ok(Some(summary))
}

#[tauri::command(async)]
#[allow(clippy::too_many_arguments)]
// Tauri command arguments are the public IPC wire.
/// A browser pane, on whichever engine this build carries: CEF (Chromium) on
/// macOS with the `chromium-browser` feature, wry (WebKit / WebView2 /
/// webkit2gtk) everywhere else. The shared work is the same for both — who is
/// asking, the profile id's shape, the label, the cookies staged for it — and
/// then each build compiles ONLY its own engine's body: no dead tail, no
/// `allow()`. The wry body, and the reader/table/Safari decision the source
/// contracts read, live in [`open_webkit_browser_pane`] (t-3621).
pub(crate) fn open_browser_pane(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    url: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    user_agent: Option<String>,
    profile: Option<String>,
    label: Option<String>,
) -> Result<String, String> {
    from_the_main_webview(&webview)?;
    // An id that is not one of ours never reaches an engine — the hex shape is
    // minted only by `create_browser_profile`, and the shape check IS the
    // security check for BOTH engines. The wry path recomputes the store bytes
    // it hands `data_store_identifier`; CEF names the jar by the id directly.
    if let Some(id) = profile.as_deref() {
        browser_profile_store(id).ok_or("프로필 id가 아닙니다")?;
    }
    let parsed = browsable(&url)?;
    // A pane is a road a walk starts on, and the walk's judge warms the wire
    // too late to help its first question (t-5535): warm it here, while the
    // pane is still being born, on the helper's own thread.
    crate::systemone::warm_for_walks();
    // The label: minted here for the window's own open, or CLAIMED when the
    // agents' door reserved it first (`zerocode-browser open` answers the
    // label before the pane exists). Only a reserved label is accepted —
    // and only the main webview reaches this command at all.
    let label = {
        let mut born = state.browser_born();
        let mut reserved = state.browser_reserved();
        claim_label(&mut born, &mut reserved, label.as_deref())?
    };
    // Imported cookies waiting for this profile decide WHERE the pane is
    // born. wry sends `loadRequest` the moment the view exists (wkwebview
    // `new`), and `set_cookie` is a per-cookie round trip to the network
    // process — so a pane born on its destination fires the first document
    // request with an EMPTY jar, the site answers logged-out and often seats
    // a fresh anonymous session under the very names just imported (live
    // report 2026-08-26 "쿠키를 가져왔는데 로그인이 풀려 있다"). Such a pane is
    // born on about:blank instead, drinks the staging, and only then leaves
    // by `navigate` — the first real request carries the cookies.
    let staged = profile.as_deref().map_or_else(Vec::new, |id| {
        browser_cookie_import::peek_staged_cookies(state.config_root(), id)
    });
    // The one door forks here into exactly one engine's body — cfg strips the
    // other whole, so neither build carries the other's dead code.
    #[cfg(all(target_os = "macos", feature = "chromium-browser"))]
    return open_chromium_browser_pane(
        &app, &state, parsed, x, y, width, height, user_agent, profile, label, staged,
    );
    #[cfg(not(all(target_os = "macos", feature = "chromium-browser")))]
    return open_webkit_browser_pane(
        &app, &state, parsed, x, y, width, height, user_agent, profile, label, staged,
    );
}

/// The wry pane — every build but macOS-with-Chromium. A user agent is
/// builder-time in wry, so the reader/table/Safari decision is made here for
/// the pane being born (never a running one); the profile's cookie jar is
/// joined at birth by the platform's own partition (`WKWebsiteDataStore` on
/// macOS, a per-profile user-data directory on Windows, the shared default on
/// Linux). The store bytes are recomputed here from the id `open_browser_pane`
/// already validated.
#[cfg(not(all(target_os = "macos", feature = "chromium-browser")))]
#[allow(clippy::too_many_arguments)]
fn open_webkit_browser_pane(
    app: &AppHandle,
    state: &AppState,
    parsed: tauri::Url,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    user_agent: Option<String>,
    profile: Option<String>,
    label: String,
    staged: Vec<browser_cookie_import::StagedCookie>,
) -> Result<String, String> {
    let store = match profile.as_deref() {
        Some(id) => Some(browser_profile_store(id).ok_or("프로필 id가 아닙니다")?),
        None => None,
    };
    let Some(window) = app.get_window("main") else {
        return Err("창이 없습니다".to_string());
    };
    // Tauri's host scripts must be removed before a macOS guest sees a
    // remote document, including a pane with no staged cookies. An explicitly
    // blank destination is navigated again too, to discard the birth globals.
    let born_blank =
        cfg!(target_os = "macos") || (!staged.is_empty() && parsed.as_str() != BROWSER_BLANK_URL);
    let birth = if born_blank {
        blank_page()
    } else {
        parsed.clone()
    };
    // The blank birth is not the address bar's story: its Started/Finished
    // are swallowed until the destination's own Started arrives.
    let load_birth = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(born_blank));
    let load_app = app.clone();
    let load_label = label.clone();
    let title_app = app.clone();
    let title_label = label.clone();
    let popup_app = app.clone();
    let popup_label = label.clone();
    let download_app = app.clone();
    let download_label = label.clone();
    let builder = BROWSER_GUEST_SCRIPTS.iter().fold(
        tauri::webview::WebviewBuilder::new(&label, tauri::WebviewUrl::External(birth)),
        |builder, script| builder.initialization_script(browser_guest_script(script)),
    );
    let builder = builder
        // The guard travels WITH the pane: links and redirects walk the same
        // allowlist the commands do, so a page cannot navigate itself
        // somewhere the address bar would have refused. It fires for EVERY
        // frame the page touches — sub-frames, ad iframes, tracking beacons —
        // so guarding is the ONE thing it does. It must NOT drive the address
        // bar or the spinner: a background beacon to about:blank once emptied
        // the address and left the reload spinning forever (live report
        // 2026-08-14). Where the tab actually went is the MAIN document's
        // story, told below.
        .on_navigation(browsable_target)
        // The address and the loading spinner follow the MAIN document alone —
        // `on_page_load` is that frame's own Started/Finished, which is what a
        // browser's address bar and stop/reload button are really about.
        .on_page_load(move |pane, payload| {
            if load_birth.load(std::sync::atomic::Ordering::Relaxed) {
                if payload.url().as_str() == BROWSER_BLANK_URL {
                    return;
                }
                load_birth.store(false, std::sync::atomic::Ordering::Relaxed);
            }
            let now = std::time::Instant::now();
            let state = match payload.event() {
                tauri::webview::PageLoadEvent::Started => {
                    // Arm the clock: wry never reports a provisional
                    // navigation that failed, so a Started with no Finished
                    // after NAV_DEAD_AFTER_SECS is called dead by THIS timer
                    // — once, and only if no later Started moved the pane on
                    // (the generation says). The window's spinner, `tabs`
                    // and `diagnose` all read the same record.
                    let generation = load_app
                        .state::<AppState>()
                        .browser_records()
                        .get_mut(&load_label)
                        .map(|record| record.nav.started(now));
                    if let Some(generation) = generation {
                        let clock_app = load_app.clone();
                        let clock_label = load_label.clone();
                        tauri::async_runtime::spawn(async move {
                            tokio::time::sleep(Duration::from_secs(NAV_DEAD_AFTER_SECS)).await;
                            let state = clock_app.state::<AppState>();
                            let marked = state.browser_records().get_mut(&clock_label).is_some_and(
                                |record| {
                                    record
                                        .nav
                                        .dead_if_still(generation, std::time::Instant::now())
                                },
                            );
                            if !marked {
                                return;
                            }
                            let url = state
                                .browser_urls()
                                .get(&clock_label)
                                .cloned()
                                .unwrap_or_default();
                            let _ = clock_app.emit_to(
                                "main",
                                "browser:nav",
                                BrowserNav {
                                    label: clock_label,
                                    url,
                                    state: "dead",
                                },
                            );
                        });
                    }
                    "started"
                }
                tauri::webview::PageLoadEvent::Finished => {
                    let blank = payload.url().as_str() == BROWSER_BLANK_URL;
                    if let Some(record) = load_app
                        .state::<AppState>()
                        .browser_records()
                        .get_mut(&load_label)
                    {
                        record.nav.finished(blank);
                    }
                    "finished"
                }
            };
            // A new document is a new listener: the watcher lives in the page,
            // so every navigation has to be handed one again (1-g13).
            if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                let _ = pane.eval(browser_guest_script(BROWSER_MENU_JS));
            }
            // The page's own word on where it stands — never asked of the
            // webview afterwards (wry unwraps a nil URL on the main thread).
            load_app
                .state::<AppState>()
                .browser_urls()
                .insert(load_label.clone(), payload.url().to_string());
            let _ = load_app.emit_to(
                "main",
                "browser:nav",
                BrowserNav {
                    label: load_label.clone(),
                    url: payload.url().to_string(),
                    state,
                },
            );
        })
        .on_document_title_changed(move |_, title| {
            if let Some(record) = title_app
                .state::<AppState>()
                .browser_records()
                .get_mut(&title_label)
            {
                record.title = terminal_safe(&title, BROWSER_TITLE_CAP);
            }
            let _ = title_app.emit_to(
                "main",
                "browser:title",
                BrowserTitle {
                    label: title_label.clone(),
                    title,
                },
            );
        })
        .on_new_window(move |url, _| {
            let _ = popup_app.emit_to(
                "main",
                "browser:popup",
                BrowserPopup {
                    label: popup_label.clone(),
                    url: url.to_string(),
                    // A popup lands beside the page that spawned it; no pane asked for it.
                    term: None,
                },
            );
            tauri::webview::NewWindowResponse::Deny
        })
        // A download lands in the downloads folder under a unique name — no
        // save dialog, Orca's own manner. `started` carries the reserved
        // path because macOS never reports one at the finish (tauri
        // DownloadEvent docs); the window remembers what this side chose.
        .on_download(move |pane, event| {
            match event {
                tauri::webview::DownloadEvent::Requested { url, destination } => {
                    let wanted = destination
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .filter(|name| !name.is_empty())
                        .or_else(|| {
                            url.path_segments().and_then(|mut segments| {
                                segments
                                    .next_back()
                                    .filter(|last| !last.is_empty())
                                    .map(str::to_string)
                            })
                        })
                        .unwrap_or_else(|| "download".to_string());
                    let Ok(dir) = pane.app_handle().path().download_dir() else {
                        return false;
                    };
                    let Ok(landing) = reserved_download_path(&dir, &wanted) else {
                        return false;
                    };
                    let file = landing
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let said = landing.to_string_lossy().into_owned();
                    *destination = landing;
                    let _ = download_app.emit_to(
                        "main",
                        "browser:download",
                        BrowserDownload {
                            label: download_label.clone(),
                            state: "started",
                            url: url.to_string(),
                            path: said,
                            file,
                        },
                    );
                }
                tauri::webview::DownloadEvent::Finished { url, path, success } => {
                    let said = path
                        .map(|landing| landing.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let _ = download_app.emit_to(
                        "main",
                        "browser:download",
                        BrowserDownload {
                            label: download_label.clone(),
                            state: if success { "finished" } else { "failed" },
                            url: url.to_string(),
                            path: said,
                            file: String::new(),
                        },
                    );
                }
                _ => {}
            }
            true
        });
    // A pane is its reader, named at birth: a user agent is builder-time in
    // wry, and a viewport box alone squeezes the DESKTOP site into a phone's
    // width (live report 2026-08-14 "아이폰 안 되는 거 같은데" — the mobile
    // emulator door sends an iPhone here). A pane with no reader of its own
    // wears the per-site table's row for this URL's exact host when there is
    // one (§2.5), else Safari's name rather than wry's bare WebKit agent,
    // which a site that gates on the name reads as no browser at all
    // (Atlassian Cloud's 「지원되지 않는 브라우저」 on the window's own
    // Confluence tab, 2026-09-07). This is the ONE decision — reader, then
    // table, then Safari — and it is made for the pane being born: the table
    // is read here, so a new row reaches the next pane on that host, and a
    // running pane keeps the agent it was built with.
    let reader = user_agent
        .as_deref()
        .map(str::trim)
        .filter(|reader| !reader.is_empty())
        .map(str::to_string);
    let site = if reader.is_none() {
        let table = load_settings_resilient(state.settings())
            .document
            .browser
            .user_agents;
        crate::browser_user_agent::site_user_agent(&table, parsed.host_str()).map(str::to_string)
    } else {
        None
    };
    let safari = if reader.is_none() && site.is_none() {
        crate::browser_user_agent::desktop_user_agent_for_this_machine()
    } else {
        None
    };
    let builder = match (reader.as_deref(), site.as_deref(), safari.as_deref()) {
        (Some(reader), _, _) => builder.user_agent(reader),
        (None, Some(agent), _) => builder.user_agent(agent),
        (None, None, Some(name)) => builder.user_agent(name),
        (None, None, None) => builder,
    };
    // A profile is its own cookie jar: the pane joins that jar at birth.
    // macOS separates by `WKWebsiteDataStore` identifier (14+); Windows by a
    // per-profile WebView2 user-data directory (`data_directory`). Linux's
    // webkit2gtk partitions by WebContext, which tauri does not expose — so
    // there a profile still shares the default store, recorded on the map as
    // the honest remainder.
    #[cfg(target_os = "macos")]
    let builder = match store {
        Some(bytes) => builder.data_store_identifier(bytes),
        None => builder,
    };
    #[cfg(target_os = "windows")]
    let builder = match (store, profile.as_deref()) {
        (Some(_), Some(id)) => {
            builder.data_directory(state.config_root().join("browser-profile-data").join(id))
        }
        _ => builder,
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = store;
    let pane = window
        .add_child(
            builder,
            tauri::LogicalPosition::new(x, y),
            tauri::LogicalSize::new(width.max(1.0), height.max(1.0)),
        )
        .map_err(|error| error.to_string())?;
    #[cfg(target_os = "macos")]
    if let Err(error) = prepare_browser_guest(&pane, WEBVIEW_NATIVE_QUERY_TIMEOUT) {
        // Never let a remote document run with a partially prepared host
        // controller. This child has not entered the browser maps yet.
        let _ = pane.close();
        return Err(error);
    }
    // 가져온 쿠키는 판이 태어나는 이 순간이 유일한 문이다(지시서 C2):
    // WKWebsiteDataStore의 디스크 포맷은 비공개라 Electron처럼 파티션 DB에
    // 직접 쓰는 길이 없고, `set_cookie`는 살아 있는 판을 요구한다. 판은 아직
    // about:blank에 서 있다 — 쿠키가 다 앉은 뒤에야 목적지로 떠난다.
    if let Some(id) = profile.as_deref() {
        inject_staged_cookies(
            state.local_data_root(),
            &pane,
            state.config_root(),
            id,
            staged,
        );
    }
    state.browser_panes().insert(label.clone());
    // The birth facts `tabs` and `diagnose` answer from, and the navigation
    // record the dead-load clock writes. The agent the pane actually wears
    // is remembered because a builder-time agent cannot be asked back.
    state.browser_records().insert(
        label.clone(),
        BrowserPaneRecord {
            profile: profile.clone(),
            reader: reader.clone(),
            agent: reader
                .clone()
                .or(site)
                .or(safari)
                .unwrap_or_else(|| "WebKit (wry default)".to_string()),
            title: String::new(),
            nav: NavRecord::born(
                born_blank || parsed.as_str() == BROWSER_BLANK_URL,
                std::time::Instant::now(),
            ),
        },
    );
    // The address the pane was asked for is its address until a page load
    // says otherwise — the record `zerocode-browser list` answers from.
    state
        .browser_urls()
        .insert(label.clone(), parsed.to_string());
    // Registered first: a pane that failed to leave is still a pane the
    // address bar can steer — an orphan child nobody can close is worse.
    if born_blank && let Err(error) = pane.navigate(parsed) {
        note_window_event(
            state.local_data_root(),
            &format!("guest preparation: {label} could not leave about:blank: {error}"),
        );
    }
    Ok(label)
}

#[cfg(all(target_os = "macos", feature = "chromium-browser"))]
#[allow(clippy::too_many_arguments)]
fn open_chromium_browser_pane(
    app: &AppHandle,
    state: &AppState,
    parsed: tauri::Url,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    user_agent: Option<String>,
    profile: Option<String>,
    label: String,
    staged: Vec<browser_cookie_import::StagedCookie>,
) -> Result<String, String> {
    let reader = user_agent
        .as_deref()
        .map(str::trim)
        .filter(|reader| !reader.is_empty())
        .map(str::to_string);
    let site = if reader.is_none() {
        let table = load_settings_resilient(state.settings())
            .document
            .browser
            .user_agents;
        crate::browser_user_agent::site_user_agent(&table, parsed.host_str()).map(str::to_string)
    } else {
        None
    };
    let effective_agent = reader.clone().or(site);
    // Chromium profiles must be immediate children of its user-data root.
    let profile_cache = state
        .config_root()
        .join("browser-chromium")
        .join(profile.as_deref().unwrap_or("Default"));
    std::fs::create_dir_all(&profile_cache)
        .map_err(|error| format!("Chromium 프로필을 만들 수 없습니다: {error}"))?;
    let mut scripts: Vec<String> = BROWSER_GUEST_SCRIPTS
        .iter()
        .map(|script| browser_guest_script(script))
        .collect();
    scripts.push(browser_guest_script(BROWSER_MENU_JS));
    let pane = crate::chromium_browser::create_pane(crate::chromium_browser::CreatePane {
        app: app.clone(),
        label: label.clone(),
        profile_cache: Some(profile_cache),
        user_agent: effective_agent.clone(),
        x,
        y,
        width: width.max(1.0),
        height: height.max(1.0),
        initialization_scripts: scripts,
    })
    .inspect_err(|error| {
        note_window_event(
            state.local_data_root(),
            &format!("Chromium pane {label} creation failed: {error}"),
        );
    })?;

    state.browser_panes().insert(label.clone());
    state.browser_records().insert(
        label.clone(),
        BrowserPaneRecord {
            profile: profile.clone(),
            reader: reader.clone(),
            agent: effective_agent.unwrap_or_else(|| "Chromium (CEF native default)".to_string()),
            title: String::new(),
            nav: NavRecord::born(
                parsed.as_str() == BROWSER_BLANK_URL,
                std::time::Instant::now(),
            ),
        },
    );
    state
        .browser_urls()
        .insert(label.clone(), parsed.to_string());

    let launch = if let Some(profile_id) = profile {
        pane.navigate_after_cookies(
            profile_id,
            state.config_root().to_path_buf(),
            staged,
            parsed,
        )
    } else {
        pane.navigate(parsed)
    };
    if let Err(error) = launch {
        state.browser_panes().remove(&label);
        state.browser_records().remove(&label);
        state.browser_urls().remove(&label);
        let _ = pane.close();
        return Err(format!("Chromium 브라우저를 열 수 없습니다: {error}"));
    }
    Ok(label)
}

#[tauri::command(async)]
pub(crate) fn browser_navigate(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    label: String,
    url: String,
) -> Result<(), String> {
    from_the_main_webview(&webview)?;
    let parsed = browsable(&url)?;
    let pane = browser_pane_of(&app, &state, &label)?;
    state
        .browser_urls()
        .insert(label.clone(), parsed.to_string());
    pane.navigate(parsed)
        .map_err(|_| "브라우저 판에 이동 요청을 보낼 수 없습니다".to_string())
}

/// Back and forward are the page's own history walked in place — the webview
/// keeps the stack, and `history.go` is how an embedder asks without keeping
/// a second copy that could drift from the real one.
#[tauri::command(async)]
pub(crate) fn browser_history(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    label: String,
    delta: i32,
) -> Result<(), String> {
    from_the_main_webview(&webview)?;
    let pane = browser_pane_of(&app, &state, &label)?;
    #[cfg(all(target_os = "macos", feature = "chromium-browser"))]
    match delta {
        -1 => return pane.go_back(),
        0 => return pane.reload(),
        1 => return pane.go_forward(),
        _ => {}
    }
    pane.eval(format!("history.go({delta})"))
        .map_err(|error| error.to_string())
}

#[tauri::command(async)]
pub(crate) fn browser_reload(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    label: String,
) -> Result<(), String> {
    from_the_main_webview(&webview)?;
    browser_pane_of(&app, &state, &label)?
        .reload()
        .map_err(|error| error.to_string())
}

/// The reload button while the page still loads — Orca's same button stops
/// instead (`browserTab.loading ? webview.stop()`).
#[tauri::command(async)]
pub(crate) fn browser_stop(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    label: String,
) -> Result<(), String> {
    from_the_main_webview(&webview)?;
    let pane = browser_pane_of(&app, &state, &label)?;
    #[cfg(all(target_os = "macos", feature = "chromium-browser"))]
    return pane.stop();
    #[cfg(not(all(target_os = "macos", feature = "chromium-browser")))]
    pane.eval("window.stop()")
        .map_err(|error| error.to_string())
}

/// Where the pane sits and whether it may be seen — ONE command, because the
/// two answers race as two: a hide that lost the queue to an older show put
/// the page back on top of a dialog (gpt-sol review of 1-fy, finding 8). A
/// native view does not follow CSS, so this command IS its layout engine.
#[tauri::command(async)]
#[allow(clippy::too_many_arguments)] // Tauri command arguments are the public IPC wire.
pub(crate) fn browser_place(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    label: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    shown: bool,
) -> Result<(), String> {
    from_the_main_webview(&webview)?;
    let pane = browser_pane_of(&app, &state, &label)?;
    if shown {
        #[cfg(all(target_os = "macos", feature = "chromium-browser"))]
        pane.set_bounds(x, y, width.max(1.0), height.max(1.0))?;
        #[cfg(not(all(target_os = "macos", feature = "chromium-browser")))]
        {
            pane.set_position(tauri::LogicalPosition::new(x, y))
                .map_err(|error| error.to_string())?;
            pane.set_size(tauri::LogicalSize::new(width.max(1.0), height.max(1.0)))
                .map_err(|error| error.to_string())?;
        }
        pane.show().map_err(|error| error.to_string())
    } else {
        pane.hide().map_err(|error| error.to_string())?;
        // A hidden native pane can stay the window's first responder — the
        // palette then opens VISIBLY while every keystroke still runs into
        // the page nobody can see (live report 2026-08-14: "인풋창에 글이 안
        // 써짐"). The keyboard follows the eye: hiding a pane hands focus
        // back to the main webview. Best-effort — a focus that fails must
        // not fail the hide that mattered.
        let _ = webview.set_focus();
        Ok(())
    }
}

/// Orca's "Open browser devtools" — the engine's inspector on the guest
/// page, from the toolbar and the context menu alike. The `devtools` cargo
/// feature exists for exactly this button's release build.
///
/// A TOGGLE, not an open: on macOS the inspector docks INTO this window and
/// reflows everything under it, and a button that can only open is a door
/// with no way back — the whole window "깨짐" until the inspector is closed
/// from chrome the person may not even see (live report 2026-08-14 #63).
/// The same button closing it is the exit.
#[tauri::command(async)]
pub(crate) fn browser_devtools(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    label: String,
) -> Result<(), String> {
    from_the_main_webview(&webview)?;
    let pane = browser_pane_of(&app, &state, &label)?;
    #[cfg(all(target_os = "macos", feature = "chromium-browser"))]
    return pane.toggle_devtools();
    #[cfg(not(all(target_os = "macos", feature = "chromium-browser")))]
    {
        if pane.is_devtools_open() {
            pane.close_devtools();
        } else {
            pane.open_devtools();
        }
        Ok(())
    }
}

/// Find on the page (1-g32/1-g33): install the highlighter if needed, run it,
/// and read back `{count,index}` over the same eval-callback channel the grab
/// tools use. A count of 0 means nothing matched.
#[tauri::command]
pub(crate) async fn browser_find(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    label: String,
    query: String,
    forward: bool,
    match_case: bool,
) -> Result<Option<serde_json::Value>, String> {
    from_the_main_webview(&webview)?;
    automate_find(&app, &state, &label, &query, forward, match_case).await
}

/// The one find executor — the window's toolbar and the door's `find` are
/// its two callers (the two-callers rule of the automation surface).
pub(crate) async fn automate_find(
    app: &AppHandle,
    state: &AppState,
    label: &str,
    query: &str,
    forward: bool,
    match_case: bool,
) -> Result<Option<serde_json::Value>, String> {
    let pane = browser_pane_of(app, state, label)?;
    let needle = serde_json::to_string(query).unwrap_or_else(|_| "\"\"".to_string());
    let fwd = if forward { "true" } else { "false" };
    let cased = if match_case { "true" } else { "false" };
    let script = format!(
        "(function(){{{install}; if(!{needle})return JSON.stringify({{count:0,index:0}});          return JSON.stringify(window.__zerocodeFind.run({needle},{fwd},{cased}));}})()",
        install = BROWSER_FIND_INSTALL,
    );
    let parsed = page_json(&pane, script, BROWSER_CALLBACK_DEADLINE).await?;
    Ok(find_tally(&parsed))
}

/// The find answer as the host builds it: two whole numbers and nothing
/// else. The installer keeps a `window.__zerocodeFind` it finds already
/// there, so a page can pre-seed one whose `run` answers any object it likes
/// — an order in an extra field would reach the agent as the host's own
/// answer (review t-3717 A-1). Only `count` and `index` cross, rebuilt here;
/// anything else, or a non-number, is no answer at all.
pub(crate) fn find_tally(answer: &serde_json::Value) -> Option<serde_json::Value> {
    let count = answer.get("count")?.as_u64()?;
    let index = answer.get("index")?.as_u64()?;
    Some(serde_json::json!({ "count": count, "index": index }))
}

#[tauri::command(async)]
pub(crate) fn browser_find_clear(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    label: String,
) -> Result<(), String> {
    from_the_main_webview(&webview)?;
    let pane = browser_pane_of(&app, &state, &label)?;
    pane.eval("(function(){if(window.__zerocodeFind)window.__zerocodeFind.clear();var s=window.getSelection&&window.getSelection();if(s)s.removeAllRanges();})()")
        .map_err(|error| error.to_string())
}

/// The page's zoom, as a factor. The ladder of steps lives in the window —
/// Chrome's own — and this end only refuses what no ladder says: a factor
/// outside [0.25, 5] is not a page anyone meant to read.
#[tauri::command(async)]
pub(crate) fn browser_zoom(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    label: String,
    scale: f64,
) -> Result<(), String> {
    from_the_main_webview(&webview)?;
    if !(0.25..=5.0).contains(&scale) {
        return Err("확대 범위를 벗어났습니다".to_string());
    }
    browser_pane_of(&app, &state, &label)?
        .set_zoom(scale)
        .map_err(|error| error.to_string())
}

/// Scroll the page — to a selector's middle, to the top or the bottom, or
/// by a delta — and answer where it stands afterwards.
#[tauri::command]
pub(crate) async fn browser_scroll(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    label: String,
    target: String,
) -> Result<(i64, i64), String> {
    from_the_main_webview(&webview)?;
    let target = zerocode_core::agent_browser::parse_scroll(&target)?;
    automate_scroll(&app, &state, &label, &target).await
}

pub(crate) async fn automate_scroll(
    app: &AppHandle,
    state: &AppState,
    label: &str,
    target: &zerocode_core::agent_browser::ScrollTarget,
) -> Result<(i64, i64), String> {
    use zerocode_core::agent_browser::ScrollTarget;
    let request = match target {
        ScrollTarget::Top => serde_json::json!({ "kind": "top" }),
        ScrollTarget::Bottom => serde_json::json!({ "kind": "bottom" }),
        ScrollTarget::By(dx, dy) => serde_json::json!({ "kind": "by", "dx": dx, "dy": dy }),
        ScrollTarget::Selector(selector) => {
            checked_selector(selector)?;
            serde_json::json!({ "kind": "selector", "selector": selector })
        }
    };
    let pane = browser_pane_of(app, state, label)?;
    let script = automation_script(
        &request,
        r#"
if (request.kind === "selector") {
  const selected = zcSelect(request.selector);
  if (selected.code) return zcFail(selected.code);
  selected.element.scrollIntoView({ block: "center", inline: "center", behavior: "auto" });
} else if (request.kind === "top") {
  window.scrollTo(0, 0);
} else if (request.kind === "bottom") {
  const body = document.body ? document.body.scrollHeight : 0;
  window.scrollTo(0, Math.max(document.documentElement.scrollHeight, body));
} else {
  window.scrollBy(Number(request.dx) || 0, Number(request.dy) || 0);
}
return zcEncode({ ok: true, value: { x: Math.round(window.scrollX), y: Math.round(window.scrollY) } });
"#,
    );
    let reply = page_json(&pane, script, BROWSER_CALLBACK_DEADLINE).await?;
    let value = page_value(reply)?;
    let axis = |name: &str| {
        value
            .get(name)
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| "스크롤 좌표를 읽을 수 없습니다".to_string())
    };
    Ok((axis("x")?, axis("y")?))
}

/// The door's budget for a fact the WINDOW makes true — a pane appearing
/// after `browser:agent-open`, leaving after `browser:agent-close`. Seven
/// seconds: the same room `wait` keeps under the bridge's ten.
pub(crate) const BROWSER_DOOR_BUDGET: Duration = Duration::from_millis(BROWSER_WAIT_MAX_MS);

/// Wait for `seen` inside `budget`, polling every `poll`. `false` is not a
/// failure: the window is still at it, and the answer says 「아직」 (design
/// rule 4 — every verb answers inside the budget).
pub(crate) async fn wait_until(
    budget: Duration,
    poll: Duration,
    mut seen: impl FnMut() -> bool,
) -> bool {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        if seen() {
            return true;
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return false;
        }
        tokio::time::sleep(poll.min(remaining)).await;
    }
}

/// `open`'s answer: the label, opened — or the label, still opening.
pub(crate) fn open_answer(label: &str, opened: bool) -> String {
    if opened {
        format!("열림 {label}\n")
    } else {
        format!("아직 여는 중 {label} — `zerocode-browser tabs`가 말해 줍니다\n")
    }
}

/// `close`'s answer: gone — or still closing.
pub(crate) fn close_answer(label: &str, closed: bool) -> String {
    if closed {
        format!("닫힘 {label}\n")
    } else {
        format!("아직 닫는 중 {label} — `zerocode-browser tabs`가 말해 줍니다\n")
    }
}

/// The page-side half of `diagnose`: the ring's document status and counts
/// (only the top rows cross), the visible words of the table, a visible
/// password field, and whether the page's CSP refuses `eval` — probed with
/// `Function`, which the same directive governs.
const BROWSER_DIAGNOSE_FACTS: &str = r#"
const ring = window.__GUEST_STATE__?.ring;
const take = (kind, options) => (ring && typeof ring.take === "function" ? ring.take(kind, 0, options) : null);
const rows = (held) => (held && Array.isArray(held.entries) ? held.entries : []);
const failed = rows(take("network", { failed: true, budget: 4000000 }));
const errorRows = rows(take("console", { level: "error", budget: 4000000 }))
  .concat(rows(take("errors", { budget: 4000000 })));
const lower = (value) => String(value || "").toLowerCase();
const text = lower(document.body && document.body.innerText).slice(0, 20000);
const title = lower(document.title);
const unsupported = request.unsupported.find((word) => text.includes(lower(word)) || title.includes(lower(word))) || null;
const login = request.login.find((word) => title.includes(lower(word))) || null;
let password = false;
try {
  const field = document.querySelector('input[type="password"]');
  password = !!field && zcVisible(field);
} catch (_) {}
let evalBlocked = false;
try { new Function("return 1")(); } catch (_) { evalBlocked = true; }
return zcEncode({ ok: true, value: {
  ring: !!ring,
  document: ring ? ring.document : null,
  failedCount: failed.length,
  failedTop: failed.slice(-request.top),
  errorCount: errorRows.length,
  errorTop: errorRows.slice(-request.top).map((row) => String(row.text || "")),
  unsupported, login, password, evalBlocked,
} });
"#;

/// Why a page is not working, as facts: the record by the clock, the ring's
/// counts, the page's words. The verdict is `browser_diagnose::diagnosis`.
#[tauri::command]
pub(crate) async fn browser_diagnose(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    label: String,
) -> Result<Diagnosis, String> {
    from_the_main_webview(&webview)?;
    let facts = automate_diagnose(&app, &state, &label).await?;
    Ok(diagnosis(&facts))
}

pub(crate) async fn automate_diagnose(
    app: &AppHandle,
    state: &AppState,
    label: &str,
) -> Result<PageFacts, String> {
    let pane = browser_pane_of(app, state, label)?;
    let now = std::time::Instant::now();
    let record = state.browser_records().get(label).cloned();
    let url = state
        .browser_urls()
        .get(label)
        .map(|url| scrub_url_credentials(url))
        .unwrap_or_default();
    let script = automation_script(
        &serde_json::json!({
            "unsupported": UNSUPPORTED_BROWSER_WORDS,
            "login": LOGIN_WORDS,
            "top": DIAGNOSE_TOP,
        }),
        BROWSER_DIAGNOSE_FACTS,
    );
    let reply = page_json(&pane, script, BROWSER_CALLBACK_DEADLINE).await?;
    let value = sanitize_eval_value(page_value(reply)?, 0);
    let text = |held: &serde_json::Value, name: &str| {
        held.get(name)
            .and_then(serde_json::Value::as_str)
            .map(|word| terminal_safe(word, BROWSER_TITLE_CAP))
    };
    let count = |name: &str| {
        value
            .get(name)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as usize
    };
    let status_of = |held: &serde_json::Value| {
        held.get("status")
            .and_then(serde_json::Value::as_u64)
            .filter(|status| *status > 0)
            .and_then(|status| u16::try_from(status).ok())
    };
    let document = value
        .get("document")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let failed_top = value
        .get("failedTop")
        .and_then(serde_json::Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|row| FailedRequest {
                    method: text(row, "method").unwrap_or_default(),
                    status: status_of(row),
                    url: text(row, "url").unwrap_or_default(),
                    error: text(row, "error"),
                })
                .collect()
        })
        .unwrap_or_default();
    let console_top = value
        .get("errorTop")
        .and_then(serde_json::Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row.as_str())
                .map(|row| terminal_safe(row, BROWSER_TITLE_CAP))
                .collect()
        })
        .unwrap_or_default();
    let flag = |name: &str| {
        value
            .get(name)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    };
    Ok(PageFacts {
        label: label.to_string(),
        url,
        nav: record
            .as_ref()
            .map_or(NavState::Loading, |record| record.nav.observed(now))
            .word()
            .to_string(),
        loading_secs: record
            .as_ref()
            .and_then(|record| record.nav.standing_for(now))
            .map(|stood| stood.as_secs()),
        document_status: status_of(&document),
        document_type: text(&document, "type"),
        user_agent: record
            .as_ref()
            .map(|record| record.agent.clone())
            .unwrap_or_default(),
        profile: record.as_ref().and_then(|record| record.profile.clone()),
        failed_requests: count("failedCount"),
        failed_top,
        console_errors: count("errorCount"),
        console_top,
        eval_blocked_by_csp: flag("evalBlocked"),
        unsupported_browser_word: text(&value, "unsupported"),
        login_word: text(&value, "login"),
        password_field: flag("password"),
        ring_present: flag("ring"),
    })
}

/// Evaluate one synchronous expression in the embedded WKWebView and return
/// its JSON value. This is the logged-in pane's own JS world, not Chrome CDP.
#[tauri::command]
pub(crate) async fn browser_eval(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    label: String,
    expression: String,
) -> Result<serde_json::Value, String> {
    from_the_main_webview(&webview)?;
    automate_eval(&app, &state, &label, &expression).await
}

pub(crate) async fn automate_eval(
    app: &AppHandle,
    state: &AppState,
    label: &str,
    expression: &str,
) -> Result<serde_json::Value, String> {
    checked_expression(expression)?;
    let pane = browser_pane_of(app, state, label)?;
    let script = automation_script(&serde_json::json!({}), &inlined_eval_body(expression));
    let reply = page_json(&pane, script, BROWSER_CALLBACK_DEADLINE)
        .await
        .map_err(inlined_eval_failure)?;
    Ok(sanitize_eval_value(page_value(reply)?, 0))
}

/// The expression INLINED into the privileged script (§2.4).
///
/// `evaluateJavaScript` runs past the page's Content-Security-Policy, but an
/// indirect page-side eval of the expression string inside it is the PAGE's
/// eval and dies on a CSP without `unsafe-eval` — X, GitLab and most modern
/// sites, where every `eval` answered `evaluation_failed`. Written into the
/// expression is the privileged script's own text. A newline on each side
/// so a trailing `//` comment cannot eat the closing paren; the async
/// refusal, the `undefined` marker and `zcEncode`'s caps are unchanged.
pub(crate) fn inlined_eval_body(expression: &str) -> String {
    format!(
        "const value = (\n{expression}\n);\n\
         if (value && typeof value.then === \"function\") return zcFail(\"async_value\");\n\
         const held = value === undefined ? {{ type: \"undefined\" }} : value;\n\
         return zcEncode({{ ok: true, value: held }});"
    )
}

/// Inlined, a syntax error is the whole script's: the engine answers with
/// nothing (wry hands the callback an empty string) or with nothing in time,
/// so those rungs of the ladder now say what happened. The engine's own
/// words are still never formatted — the payload may hold a credential.
pub(crate) const EVAL_NOT_ACCEPTED: &str = "식이 문법에 맞지 않거나 판이 받지 않았습니다";

pub(crate) fn inlined_eval_failure(why: String) -> String {
    if why == PAGE_SEND_FAILED || why == PAGE_TIMED_OUT || why == PAGE_ANSWER_UNREADABLE {
        EVAL_NOT_ACCEPTED.to_string()
    } else {
        why
    }
}

pub(crate) fn checked_expression(expression: &str) -> Result<(), String> {
    if expression.chars().count() > BROWSER_EXPRESSION_CAP {
        return Err("페이지 식은 64000자를 넘을 수 없습니다".to_string());
    }
    // Keep the obvious credential stores out of an accidental eval. This is
    // defense in depth, not a JavaScript sandbox: the caller already holds the
    // dedicated browser capability, while return values are independently
    // redacted by key on both sides of the callback. The words are matched at
    // IDENTIFIER boundaries — `localStorageQuota` is not `localStorage`, and
    // `document["cookie"]` is still `document` then `cookie` (§2.4).
    let folded = expression.to_ascii_lowercase();
    let words: Vec<&str> = folded
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '$'))
        .filter(|word| !word.is_empty())
        .collect();
    let alone = ["cookiestore", "localstorage", "sessionstorage"];
    let after = [("document", "cookie"), ("navigator", "credentials")];
    if words.iter().any(|word| alone.contains(word))
        || words
            .windows(2)
            .any(|pair| after.contains(&(pair[0], pair[1])))
    {
        return Err("페이지 자격증명 저장소는 eval 결과로 읽을 수 없습니다".to_string());
    }
    Ok(())
}

/// One read of the page's observation ring (`ui/browser-ring.js`): the rows
/// after `since`, the cursor to continue from, and the document's own status
/// — the pane's word on what it did, never a devtools window.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RingTake {
    /// The newest seq in the ring, whatever was filtered.
    pub(crate) seq: u64,
    /// The seq of the last row answered — the next `--since`.
    pub(crate) last: u64,
    /// Whether the budget cut the answer short of the ring.
    pub(crate) more: bool,
    /// `{status, type, url}` of the main document's navigation.
    pub(crate) document: serde_json::Value,
    pub(crate) entries: Vec<serde_json::Value>,
}

/// The page-side half of a ring read: refuses when the ring is absent
/// (a pane born before this build, or a document the script never met).
const BROWSER_RING_TAKE: &str = r#"
const ring = window.__GUEST_STATE__?.ring;
if (!ring || typeof ring.take !== "function") return zcFail("ring_missing");
return zcEncode({ ok: true, value: ring.take(request.kind, request.since, request.options) });
"#;

async fn ring_take(
    pane: &BrowserPane,
    kind: &str,
    since: u64,
    options: serde_json::Value,
    cap: usize,
) -> Result<RingTake, String> {
    let script = automation_script(
        &serde_json::json!({ "kind": kind, "since": since, "options": options }),
        &browser_guest_script(BROWSER_RING_TAKE),
    );
    let reply = page_json(pane, script, BROWSER_CALLBACK_DEADLINE).await?;
    // Scrubbed on the page, scrubbed again here (§2.1): the ring's own
    // redaction is the page's word, and this side takes nobody's word.
    parse_ring_take(sanitize_eval_value(page_value(reply)?, 0), cap)
}

/// The ring's answer, parsed without being believed: the cap is re-applied
/// to the rows' JSON length, and a cut answer says `more` with the cursor
/// moved back to the last row kept.
pub(crate) fn parse_ring_take(value: serde_json::Value, cap: usize) -> Result<RingTake, String> {
    let number = |name: &str| value.get(name).and_then(serde_json::Value::as_u64);
    let seq = number("seq").ok_or_else(|| "관측 링의 답을 읽을 수 없습니다".to_string())?;
    let mut last = number("last").unwrap_or(seq);
    let mut more = value
        .get("more")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let document = value
        .get("document")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let mut entries = Vec::new();
    let mut spent = 0usize;
    for entry in value
        .get("entries")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let cost = entry.to_string().len();
        if spent + cost > cap && !entries.is_empty() {
            more = true;
            last = entries
                .last()
                .and_then(|kept: &serde_json::Value| kept.get("seq"))
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(last);
            break;
        }
        spent += cost;
        entries.push(entry.clone());
    }
    Ok(RingTake {
        seq,
        last,
        more,
        document,
        entries,
    })
}

/// The page's console rows after `since`, at the asked level.
#[tauri::command]
pub(crate) async fn browser_console(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    label: String,
    since: Option<u64>,
    level: Option<String>,
) -> Result<RingTake, String> {
    from_the_main_webview(&webview)?;
    let since = since.unwrap_or(0);
    let level = level.unwrap_or_else(|| "all".to_string());
    automate_console(&app, &state, &label, since, &level).await
}

pub(crate) async fn automate_console(
    app: &AppHandle,
    state: &AppState,
    label: &str,
    since: u64,
    level: &str,
) -> Result<RingTake, String> {
    if !matches!(level, "error" | "warn" | "all") {
        return Err("콘솔 레벨은 error|warn|all 중 하나입니다".to_string());
    }
    let pane = browser_pane_of(app, state, label)?;
    let options = serde_json::json!({ "level": level, "budget": BROWSER_CONSOLE_CAP });
    ring_take(&pane, "console", since, options, BROWSER_CONSOLE_CAP).await
}

/// The page's request rows after `since` — fetch, XHR and every resource
/// the timeline saw. Never a header, never a body.
#[tauri::command]
pub(crate) async fn browser_network(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    label: String,
    since: Option<u64>,
    failed_only: Option<bool>,
) -> Result<RingTake, String> {
    from_the_main_webview(&webview)?;
    let (since, failed) = (since.unwrap_or(0), failed_only.unwrap_or(false));
    automate_network(&app, &state, &label, since, failed).await
}

pub(crate) async fn automate_network(
    app: &AppHandle,
    state: &AppState,
    label: &str,
    since: u64,
    failed_only: bool,
) -> Result<RingTake, String> {
    let pane = browser_pane_of(app, state, label)?;
    let options = serde_json::json!({ "failed": failed_only, "budget": BROWSER_NETWORK_CAP });
    ring_take(&pane, "network", since, options, BROWSER_NETWORK_CAP).await
}

/// `hh:mm:ss` of an epoch-millisecond stamp on the local clock — the offset
/// is the platform's (`local_offset_secs`), handed in so the table below can
/// be pinned without a timezone.
pub(crate) fn clock_of(at_epoch_ms: i64, local_offset_secs: i64) -> String {
    let secs = (at_epoch_ms.div_euclid(1000) + local_offset_secs).rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

/// The cursor line every ring answer ends with: where the ring stands and,
/// when the budget cut, how to continue.
fn ring_trailer(take: &RingTake) -> String {
    if take.more {
        format!(
            "# seq {} · last {} · 더 있음 → --since {}\n",
            take.seq, take.last, take.last
        )
    } else {
        format!("# seq {} · last {}\n", take.seq, take.last)
    }
}

fn entry_str<'a>(entry: &'a serde_json::Value, name: &str) -> &'a str {
    entry
        .get(name)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
}

fn entry_num(entry: &serde_json::Value, name: &str) -> Option<i64> {
    entry.get(name).and_then(serde_json::Value::as_i64)
}

/// `seq\tlevel\thh:mm:ss\ttext` per row, one line each — a row's own
/// newlines are folded so a line stays a row.
pub(crate) fn console_lines(take: &RingTake, local_offset_secs: i64) -> String {
    let mut out = String::new();
    for entry in &take.entries {
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            entry_num(entry, "seq").unwrap_or(0),
            entry_str(entry, "level"),
            clock_of(entry_num(entry, "at").unwrap_or(0), local_offset_secs),
            entry_str(entry, "text").replace(['\n', '\r'], " ")
        ));
    }
    if take.entries.is_empty() {
        out.push_str("(콘솔 기록 없음)\n");
    }
    out.push_str(&ring_trailer(take));
    out
}

/// `seq\tmethod\tstatus\tms\turl` per row. A network error's status is the
/// error's name (`ERR:TypeError`); a resource whose status the timeline could
/// not see is `?`, never a failure.
pub(crate) fn network_lines(take: &RingTake) -> String {
    let mut out = String::new();
    for entry in &take.entries {
        let status = match entry_num(entry, "status") {
            Some(0) | None if !entry_str(entry, "error").is_empty() => {
                format!("ERR:{}", entry_str(entry, "error"))
            }
            Some(0) | None => "?".to_string(),
            Some(status) => status.to_string(),
        };
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\n",
            entry_num(entry, "seq").unwrap_or(0),
            entry_str(entry, "method"),
            status,
            entry_num(entry, "ms").unwrap_or(0),
            entry_str(entry, "url")
        ));
    }
    if take.entries.is_empty() {
        out.push_str("(요청 기록 없음)\n");
    }
    out.push_str(&ring_trailer(take));
    out
}

/// Read visible text from the document or a selector. A selector read also
/// returns a reduced DOM copy; it never returns live form values or arbitrary
/// attributes from the logged-in page.
#[tauri::command]
pub(crate) async fn browser_read(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    label: String,
    selector: Option<String>,
) -> Result<BrowserReadReport, String> {
    from_the_main_webview(&webview)?;
    automate_read(&app, &state, &label, selector.as_deref()).await
}

pub(crate) async fn automate_read(
    app: &AppHandle,
    state: &AppState,
    label: &str,
    selector: Option<&str>,
) -> Result<BrowserReadReport, String> {
    if let Some(selector) = selector {
        checked_selector(selector)?;
    }
    let pane = browser_pane_of(app, state, label)?;
    let script = automation_script(
        &serde_json::json!({ "selector": selector }),
        &format!(
            r#"
const selected = request.selector === null
  ? {{ element: document.body || document.documentElement }}
  : zcSelect(request.selector);
if (selected.code) return zcFail(selected.code);
const element = selected.element;
const visibleText = typeof element.innerText === "string" ? element.innerText : element.textContent;
return zcEncode({{ ok: true, value: {{
  title: String(document.title || "").slice(0, {BROWSER_TITLE_CAP}),
  url: zcSafeUrl(location.href).slice(0, {BROWSER_URL_CAP}),
  text: String(visibleText || "").slice(0, {BROWSER_READ_CAP}),
  dom: request.selector === null ? null : zcDom(element).slice(0, {BROWSER_DOM_CAP})
}} }});
"#
        ),
    );
    let reply = page_json(&pane, script, BROWSER_CALLBACK_DEADLINE).await?;
    parse_read_reply(reply)
}

pub(crate) fn parse_read_reply(reply: serde_json::Value) -> Result<BrowserReadReport, String> {
    parse_read_value(page_value(reply)?)
}

fn parse_read_value(value: serde_json::Value) -> Result<BrowserReadReport, String> {
    let field = |name: &str, cap: usize| {
        value
            .get(name)
            .and_then(serde_json::Value::as_str)
            .map(|value| terminal_safe(value, cap))
            .unwrap_or_default()
    };
    let dom = value
        .get("dom")
        .and_then(serde_json::Value::as_str)
        .map(|value| terminal_safe(value, BROWSER_DOM_CAP));
    Ok(BrowserReadReport {
        title: field("title", BROWSER_TITLE_CAP),
        url: scrub_url_credentials(&field("url", BROWSER_URL_CAP)),
        text: field("text", BROWSER_READ_CAP),
        dom,
    })
}

/// A whole-document read with the page cut into blocks for the read seat
/// (`crate::browser_read`): the same title, address and text `read` answers
/// today — `text` is the very expression the plain read evaluates, so a read
/// the seat hands back whole is byte for byte the read that existed before
/// it — and beside them the body cut at the seat's landmarks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrowserReadPage {
    pub(crate) report: BrowserReadReport,
    pub(crate) blocks: Vec<zerocode_core::browser_read::ReadBlock>,
}

/// The page script that cuts the body into blocks at the seat's landmarks
/// (`request.blockRoots`, the table's own list) and answers each block's
/// path and visible text, up to the read's text cap (`request.textCap`).
///
/// An element with no landmark inside it is one block; one with landmarks
/// inside is walked, and the children that are not landmarks themselves —
/// a heading, a paragraph, a `div` of them — are one run under it
/// (`path>*`). A child that only CONTAINS landmarks is transparent: walked
/// under the same path. What is not rendered is not read (`innerText`'s own
/// rule for a rendered element; a hidden element is skipped, because
/// `innerText` on one falls back to its whole `textContent`), and scripts,
/// styles and templates never are.
pub(crate) const BROWSER_READ_BLOCKS_BODY: &str = r#"
const roots = request.blockRoots;
const cap = request.textCap;
const body = document.body || document.documentElement;
const blocks = [];
let used = 0;
const unread = (el) => {
  if (!el || el.nodeType !== 1) return true;
  if (el.matches("script, style, noscript, template")) return true;
  if (el.hidden) return true;
  const style = getComputedStyle(el);
  return style.display === "none" || style.visibility === "hidden";
};
const textOf = (el) => String(typeof el.innerText === "string" ? el.innerText : el.textContent || "");
const hasStructure = (el) => !!el.querySelector(roots);
const push = (path, text) => {
  const whole = String(text || "");
  if (!whole.trim() || used >= cap) return;
  const kept = whole.slice(0, cap - used);
  used += kept.length;
  blocks.push({ path, text: kept });
};
const runText = (nodes) => {
  const lines = [];
  for (const node of nodes) {
    const text = node.nodeType === 3 ? String(node.data || "") : (unread(node) ? "" : textOf(node));
    if (text.trim()) lines.push(text.trim());
  }
  return lines.join("\n");
};
const walk = (el, path) => {
  if (unread(el)) return;
  if (!hasStructure(el)) { push(path, textOf(el)); return; }
  let run = [];
  const flush = () => { if (run.length) { push(path + ">*", runText(run)); run = []; } };
  for (const child of el.childNodes) {
    if (child.nodeType === 3) { run.push(child); continue; }
    if (child.nodeType !== 1 || unread(child)) continue;
    if (child.matches(roots)) { flush(); walk(child, path + ">" + zcTagWord(child, roots)); }
    else if (hasStructure(child)) { flush(); walk(child, path); }
    else run.push(child);
  }
  flush();
};
walk(body, "body");
return zcEncode({ ok: true, value: {
  title: String(document.title || "").slice(0, request.titleCap),
  url: zcSafeUrl(location.href).slice(0, request.urlCap),
  text: String(typeof body.innerText === "string" ? body.innerText : body.textContent || "").slice(0, cap),
  blocks
} }, request.answerCap);
"#;

/// The whole document, cut into blocks — what the read seat judges.
pub(crate) async fn automate_read_blocks(
    app: &AppHandle,
    state: &AppState,
    label: &str,
) -> Result<BrowserReadPage, String> {
    let pane = browser_pane_of(app, state, label)?;
    let script = automation_script(
        &serde_json::json!({
            "blockRoots": block_roots(),
            "textCap": BROWSER_READ_CAP,
            "titleCap": BROWSER_TITLE_CAP,
            "urlCap": BROWSER_URL_CAP,
            "answerCap": BROWSER_CALLBACK_CAP,
        }),
        BROWSER_READ_BLOCKS_BODY,
    );
    let reply = page_json(&pane, script, BROWSER_CALLBACK_DEADLINE).await?;
    parse_read_blocks_reply(reply)
}

/// A blocks reply parsed without being believed: the report's own caps and
/// bytes rules, then each block's path and text through the same filter.
pub(crate) fn parse_read_blocks_reply(reply: serde_json::Value) -> Result<BrowserReadPage, String> {
    let value = page_value(reply)?;
    let report = parse_read_value(value.clone())?;
    let blocks = value
        .get("blocks")
        .and_then(serde_json::Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| {
                    let path = block.get("path").and_then(serde_json::Value::as_str)?;
                    let text = block.get("text").and_then(serde_json::Value::as_str)?;
                    Some(zerocode_core::browser_read::ReadBlock::new(
                        &terminal_safe(path, zerocode_core::jev::BROWSER_READ_PATH_CHAR_CAP),
                        &terminal_safe(text, BROWSER_READ_CAP),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(BrowserReadPage { report, blocks })
}

/// Activate the first visible element matching a CSS selector.
#[tauri::command]
pub(crate) async fn browser_click(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    label: String,
    selector: String,
) -> Result<BrowserInputReport, String> {
    from_the_main_webview(&webview)?;
    automate_click(&app, &state, &label, &selector).await
}

pub(crate) async fn automate_click(
    app: &AppHandle,
    state: &AppState,
    label: &str,
    selector: &str,
) -> Result<BrowserInputReport, String> {
    checked_selector(selector)?;
    let pane = browser_pane_of(app, state, label)?;
    let script = automation_script(
        &serde_json::json!({ "selector": selector, "blockRoots": block_roots() }),
        CLICK_BODY,
    );
    let reply = page_json(&pane, script, BROWSER_CALLBACK_DEADLINE).await?;
    input_report(page_value(reply)?, &["dom-activation"])
}

/// The click's page script: the first element the selector names that a
/// person could see (`zcSelect`), scrolled into view, visible, enabled and on
/// top at its centre, pressed through the
/// pointer and mouse events and its own activation — and the rectangle it
/// measured to press, answered with the page's device ratio.
pub(crate) const CLICK_BODY: &str = r#"
const selected = zcSelect(request.selector);
if (selected.code) return zcFail(selected.code);
const element = selected.element;
element.scrollIntoView({ block: "center", inline: "center", behavior: "auto" });
if (!zcVisible(element)) return zcFail("element_not_visible");
if (element.matches && element.matches(":disabled")) return zcFail("element_disabled");
const rect = element.getBoundingClientRect();
const clientX = rect.left + rect.width / 2;
const clientY = rect.top + rect.height / 2;
const hit = document.elementFromPoint(clientX, clientY);
if (hit && hit !== element && !element.contains(hit)) return zcFail("element_obscured");
const common = { bubbles: true, cancelable: true, composed: true, view: window,
  clientX, clientY, button: 0, buttons: 0 };
if (typeof PointerEvent === "function") {
  element.dispatchEvent(new PointerEvent("pointerover", { ...common, pointerId: 1, pointerType: "mouse" }));
  element.dispatchEvent(new PointerEvent("pointerenter", { ...common, pointerId: 1, pointerType: "mouse", bubbles: false }));
  element.dispatchEvent(new PointerEvent("pointerdown", { ...common, buttons: 1, pointerId: 1, pointerType: "mouse" }));
}
element.dispatchEvent(new MouseEvent("mouseover", common));
element.dispatchEvent(new MouseEvent("mouseenter", { ...common, bubbles: false }));
element.dispatchEvent(new MouseEvent("mousedown", { ...common, buttons: 1 }));
if (typeof element.focus === "function") element.focus({ preventScroll: true });
if (typeof PointerEvent === "function") {
  element.dispatchEvent(new PointerEvent("pointerup", { ...common, buttons: 0, pointerId: 1, pointerType: "mouse" }));
}
element.dispatchEvent(new MouseEvent("mouseup", { ...common, buttons: 0 }));
if (typeof element.click === "function") {
  element.click();
} else {
  element.dispatchEvent(new MouseEvent("click", { ...common, detail: 1 }));
}
return zcEncode({ ok: true, value: { method: "dom-activation",
  rect: [rect.left, rect.top, rect.width, rect.height], dpr: window.devicePixelRatio,
  blockPath: zcStructuralChain(element, request.blockRoots), pageUrl: zcSafeUrl(location.href) } });
"#;

// ---- Marks (set-of-marks) for the browser door (t-4246, plan D) ----
//
// The same marks the desktop look draws, on the browser door: `marks` numbers
// the controls a person could hit, `click --mark N` presses one by its number
// with the marks' own pin refusing a control that moved or changed, and
// `screenshot --marks` lays the numbers onto the picture. The page gathers the
// faces (only the live layout knows what is on top); the core numbers them
// (`number_marks`) and judges the pin (`mark_still_holds` → `Pin::holds`), so
// the browser mirrors the desktop's helper-gathers / core-decides split rather
// than trusting or copying a page's own judgment.

use zerocode_core::agent_browser::{
    BROWSER_MARKABLE, BrowserFace, BrowserMark, BrowserRemeasure, mark_still_holds, number_marks,
};
use zerocode_core::computer_use_protocol::cache;

/// Page-side helpers the marks and re-measure scripts share, so the accessible
/// name, role, tag and selector of a control are computed one way — a pin the
/// look drew and the pin the press proves cannot disagree because two scripts
/// spelled a label differently. The script never writes to the page (no
/// attribute, no style): it only reads, and `screenshot --marks` draws the
/// numbers outside the page.
pub(crate) const BROWSER_MARK_HELPERS: &str = r##"
const zcMarkTag = (el) => String(el.tagName || "").toLowerCase();
const zcMarkRole = (el) => {
  const explicit = el.getAttribute && el.getAttribute("role");
  if (explicit && explicit.trim()) return explicit.trim();
  const tag = zcMarkTag(el);
  const type = String((el.getAttribute && el.getAttribute("type")) || "").toLowerCase();
  if (tag === "a" && el.hasAttribute("href")) return "link";
  if (tag === "button") return "button";
  if (tag === "select") return "combobox";
  if (tag === "textarea") return "textbox";
  if (tag === "input") {
    if (["button", "submit", "reset", "image"].includes(type)) return "button";
    if (type === "checkbox") return "checkbox";
    if (type === "radio") return "radio";
    return "textbox";
  }
  if (el.isContentEditable) return "textbox";
  return "";
};
const zcMarkName = (el) => {
  const aria = el.getAttribute && el.getAttribute("aria-label");
  if (aria && aria.trim()) return aria.trim();
  const text = String(el.innerText || el.textContent || "").trim();
  if (text) return text;
  const secret = el.matches && el.matches('input[type="password"]');
  if (!secret && "value" in el && el.value) return String(el.value);
  const title = el.getAttribute && el.getAttribute("title");
  if (title && title.trim()) return title.trim();
  const placeholder = el.getAttribute && el.getAttribute("placeholder");
  if (placeholder && placeholder.trim()) return placeholder.trim();
  return "";
};
const zcMarkSelector = (el) => {
  if (el.id) return "#" + CSS.escape(el.id);
  const parts = [];
  let node = el;
  while (node && node.nodeType === 1 && parts.length < 20) {
    if (node.id) { parts.unshift("#" + CSS.escape(node.id)); break; }
    let segment = zcMarkTag(node);
    const parent = node.parentElement;
    if (parent) {
      const siblings = [...parent.children].filter((child) => child.tagName === node.tagName);
      if (siblings.length > 1) segment += ":nth-of-type(" + (siblings.indexOf(node) + 1) + ")";
    }
    parts.unshift(segment);
    if (!parent || node === document.body) break;
    node = parent;
  }
  return parts.join(" > ");
};
const zcMarkFace = (el) => {
  const r = el.getBoundingClientRect();
  return { tag: zcMarkTag(el), role: zcMarkRole(el), label: zcMarkName(el),
    selector: zcMarkSelector(el), x: r.left, y: r.top, width: r.width, height: r.height };
};
"##;

/// The `marks` walk: every markable control in the viewport, in document
/// order, with whether a person could hit it — `elementFromPoint` at its
/// centre is it, a child of it, or an ancestor around it, never a different
/// control on top (the obscured-element rule of
/// [[a-mark-presses-only-what-a-person-could-hit]]).
pub(crate) const BROWSER_MARKS_BODY: &str = r#"
const seen = document.querySelectorAll(request.selectors.join(","));
const vw = window.innerWidth, vh = window.innerHeight;
const faces = [];
for (const el of seen) {
  if (el.matches && el.matches('[contenteditable="false"]')) continue;
  const r = el.getBoundingClientRect();
  if (!(r.width > 0 && r.height > 0)) continue;
  if (r.bottom <= 0 || r.right <= 0 || r.top >= vh || r.left >= vw) continue;
  const face = zcMarkFace(el);
  const cx = r.left + r.width / 2, cy = r.top + r.height / 2;
  let hit = false;
  if (cx >= 0 && cy >= 0 && cx < vw && cy < vh) {
    const top = document.elementFromPoint(cx, cy);
    hit = !!top && (top === el || el.contains(top) || top.contains(el));
  }
  face.hit = hit;
  faces.push(face);
}
return zcEncode({ ok: true, value: { faces,
  viewport: { width: vw, height: vh, dpr: window.devicePixelRatio || 1 } } }, request.answerCap);
"#;

/// The click-by-mark re-measure: the control at the mark's selector as it
/// stands now, for the core pin to judge before the press. A selector that
/// resolves to nothing answers `found: false`.
pub(crate) const BROWSER_REMEASURE_BODY: &str = r#"
const el = document.querySelector(request.selector);
if (!el) return zcEncode({ ok: true, value: { found: false } });
const face = zcMarkFace(el);
face.found = true;
return zcEncode({ ok: true, value: face });
"#;

/// One pane's last `marks` answer, kept under its label so `click --mark N`
/// and `screenshot --marks` read the very numbers the agent saw, and the
/// viewport those rects were measured in so the picture can place them.
struct BrowserMarkTable {
    marks: Vec<BrowserMark>,
    viewport_width: f64,
    made: std::time::Instant,
}

fn browser_marks_store()
-> &'static std::sync::Mutex<std::collections::HashMap<String, BrowserMarkTable>> {
    static MARKS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, BrowserMarkTable>>,
    > = std::sync::OnceLock::new();
    MARKS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn remember_marks(label: &str, marks: Vec<BrowserMark>, viewport_width: f64) {
    browser_marks_store()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(
            label.to_string(),
            BrowserMarkTable {
                marks,
                viewport_width,
                made: std::time::Instant::now(),
            },
        );
}

/// The pane's last marks and the viewport they were measured in — refused
/// when there are none, or when they are older than the cache's max age (the
/// agent looks again with `marks`).
fn recall_marks(label: &str) -> Result<(Vec<BrowserMark>, f64), String> {
    let held = browser_marks_store()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let table = held.get(label).ok_or_else(|| {
        format!("이 판의 marks 답이 없습니다 — 먼저 `zerocode-browser marks {label}`")
    })?;
    if cache::is_expired(table.made, std::time::Instant::now()) {
        return Err(format!(
            "marks 답이 {}초보다 오래됐습니다 — 다시 `zerocode-browser marks {label}`",
            cache::MAX_AGE.as_secs()
        ));
    }
    Ok((table.marks.clone(), table.viewport_width))
}

/// Walk the page for the controls a person could hit, number the hittable
/// ones in document order (the core's pure step), and remember them under the
/// pane's label.
pub(crate) async fn automate_marks(
    app: &AppHandle,
    state: &AppState,
    label: &str,
) -> Result<Vec<BrowserMark>, String> {
    let pane = browser_pane_of(app, state, label)?;
    let request = serde_json::json!({
        "selectors": BROWSER_MARKABLE,
        "answerCap": BROWSER_CALLBACK_CAP,
    });
    let script = automation_script(
        &request,
        &format!("{BROWSER_MARK_HELPERS}\n{BROWSER_MARKS_BODY}"),
    );
    let reply = page_json(&pane, script, BROWSER_CALLBACK_DEADLINE).await?;
    let value = page_value(reply)?;
    let faces: Vec<BrowserFace> = serde_json::from_value(
        value
            .get("faces")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    )
    .map_err(|_| "브라우저 판의 마크를 읽을 수 없습니다".to_string())?;
    let marks = number_marks(&faces);
    let viewport_width = value
        .pointer("/viewport/width")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0);
    remember_marks(label, marks.clone(), viewport_width);
    Ok(marks)
}

/// A marks answer's items, in CSS pixels plus the centre a legend reads.
pub(crate) fn marks_items(marks: &[BrowserMark]) -> Vec<serde_json::Value> {
    marks
        .iter()
        .map(|mark| {
            serde_json::json!({
                "mark": mark.mark,
                "role": mark.role,
                "tag": mark.tag,
                "label": mark.label,
                "selector": mark.selector,
                "x": mark.x,
                "y": mark.y,
                "width": mark.width,
                "height": mark.height,
                "centerX": mark.x + mark.width / 2.0,
                "centerY": mark.y + mark.height / 2.0,
            })
        })
        .collect()
}

/// The marks a person reads: one legend line per number (the desktop look's
/// own `legend_line`), the empty word when nothing qualified.
pub(crate) fn marks_lines(marks: &[BrowserMark]) -> String {
    use zerocode_core::computer_use_protocol::marks::legend_line;
    let mut lines = String::new();
    for item in marks_items(marks) {
        if let Some(line) = legend_line(&item) {
            lines.push_str(&line);
            lines.push('\n');
        }
    }
    if lines.is_empty() {
        lines.push_str("(뷰포트에서 누를 수 있는 컨트롤이 없습니다)\n");
    }
    lines
}

/// The marks answer as a machine reads it (`--json`): the items under the
/// marks' own `items` key, their count, and the fence's own word for words
/// that came from outside.
///
/// The flag and not the markers, because this answer has a program for a
/// reader: the walk looks at a pane by running exactly this and parsing what
/// comes back (`errand::desk::look`). Wrapped in marker lines it parsed as
/// nothing at all, so every browser walk ended at its first look and the seat
/// that judges one has no rows to show for it (2026-09-19). `diagnose --json`
/// already answers this way for the same reason.
pub(crate) fn marks_json(marks: &[BrowserMark]) -> serde_json::Value {
    use zerocode_core::computer_use_protocol::marks::ITEMS_KEY;
    let mut answer = serde_json::json!({ ITEMS_KEY: marks_items(marks), "count": marks.len() });
    answer[zerocode_core::untrusted::JSON_FLAG] = serde_json::Value::Bool(true);
    answer
}

/// `click <label> --mark N`: press the control numbered N on the pane's last
/// marks, but only after re-measuring it and proving the pin still holds — a
/// control that moved or changed, or a selector the page can no longer find,
/// is refused (`mark_still_holds` → the marks' own `pin_broken`). The press
/// itself walks `automate_click`'s road by the mark's selector, so its own
/// visibility and on-top guards apply at the moment of the press.
pub(crate) async fn automate_click_mark(
    app: &AppHandle,
    state: &AppState,
    label: &str,
    mark_n: usize,
) -> Result<BrowserInputReport, String> {
    let (marks, _) = recall_marks(label)?;
    let mark = mark_n
        .checked_sub(1)
        .and_then(|at| marks.get(at))
        .cloned()
        .ok_or_else(|| format!("mark {mark_n}은 이 판에 없습니다 (1–{})", marks.len()))?;
    let pane = browser_pane_of(app, state, label)?;
    let script = automation_script(
        &serde_json::json!({ "selector": mark.selector }),
        &format!("{BROWSER_MARK_HELPERS}\n{BROWSER_REMEASURE_BODY}"),
    );
    let reply = page_json(&pane, script, BROWSER_CALLBACK_DEADLINE).await?;
    let now: BrowserRemeasure = serde_json::from_value(page_value(reply)?)
        .map_err(|_| "재측정 결과를 읽을 수 없습니다".to_string())?;
    mark_still_holds(&mark, &now).map_err(|error| error.message)?;
    automate_click(app, state, label, &mark.selector).await
}

/// Lay a pane's last marks onto its screenshot: each mark's control outlined
/// and numbered by the one badge renderer the desktop look uses
/// (`draw_badges`). The rects are CSS pixels; the picture is the viewport at
/// the backing scale, so `picture-width / viewport-width` maps one to the
/// other.
pub(crate) fn paint_browser_marks(image: &mut RgbaImage, marks: &[BrowserMark], scale: f64) {
    use zerocode_core::computer_use::{
        MARK_ANCHORS_INSIDE_FIRST, MARK_ANCHORS_OUTSIDE_FIRST, MARK_INSIDE_MAX_SHARE,
    };
    use zerocode_core::computer_use_protocol::marks::badge_size;
    use zerocode_core::computer_use_protocol::render::Rect;
    let placed: Vec<(Rect, Rect, usize)> = marks
        .iter()
        .map(|mark| {
            let element_px = Rect::new(
                mark.x * scale,
                mark.y * scale,
                mark.width * scale,
                mark.height * scale,
            );
            let (width, height) = badge_size(mark.mark);
            let size = (f64::from(width), f64::from(height));
            let fits_inside = size.0 <= element_px.width
                && size.1 <= element_px.height
                && size.0 * size.1 <= MARK_INSIDE_MAX_SHARE * element_px.area();
            let anchors = if fits_inside {
                MARK_ANCHORS_INSIDE_FIRST
            } else {
                MARK_ANCHORS_OUTSIDE_FIRST
            };
            let badge_px = anchors[0].place(&element_px, size);
            (element_px, badge_px, mark.mark)
        })
        .collect();
    draw_badges(image, &placed);
}

/// The pane's last screenshot with its last marks drawn on — refused when the
/// pane has no marks (the agent runs `marks` first) or the picture cannot be
/// read.
pub(crate) fn overlay_last_marks(label: &str, png: &[u8]) -> Result<Vec<u8>, String> {
    let (marks, viewport_width) = recall_marks(label)?;
    let mut image = crate::computer_use::compare::decode_png(png)
        .ok_or_else(|| "스크린샷 PNG를 읽을 수 없습니다".to_string())?;
    let scale = if viewport_width > 0.0 {
        f64::from(image.width) / viewport_width
    } else {
        1.0
    };
    paint_browser_marks(&mut image, &marks, scale);
    image
        .encode()
        .ok_or_else(|| "마크를 얹은 PNG를 인코딩할 수 없습니다".to_string())
}

/// Replace the editable element's contents without ever echoing the supplied
/// text back over IPC. WebKit's editing command is preferred because it keeps
/// the native undo/input path; the fallback uses the native value setter and
/// beforeinput/input events used by controlled web forms.
#[tauri::command]
pub(crate) async fn browser_type(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    label: String,
    selector: String,
    text: String,
) -> Result<BrowserInputReport, String> {
    from_the_main_webview(&webview)?;
    automate_type(&app, &state, &label, &selector, &text, TypeRoad::Keys).await
}

pub(crate) async fn automate_type(
    app: &AppHandle,
    state: &AppState,
    label: &str,
    selector: &str,
    text: &str,
    road: TypeRoad,
) -> Result<BrowserInputReport, String> {
    checked_selector(selector)?;
    if text.chars().count() > BROWSER_TYPE_CAP {
        return Err("한 번에 입력할 글은 100000자를 넘을 수 없습니다".to_string());
    }
    let pane = browser_pane_of(app, state, label)?;
    let script = automation_script(
        &serde_json::json!({
            "selector": selector,
            "text": text,
            "road": road.word(),
            "blockRoots": block_roots(),
        }),
        TYPE_BODY,
    );
    let reply = page_json(&pane, script, BROWSER_CALLBACK_DEADLINE).await?;
    typed_report(page_value(reply)?, road)
}

/// The typing's page script, one body for both roads. A password field is
/// the platform's own fact — the input's type, or the `current-password`
/// a form declares — never a label's word; on the keys road the script
/// holds its keys before one and says so (`held`), for the core's table to
/// refuse by name. The setter road comes first and alone: the prototype
/// setter and an input event, no editing command, no read-back of the
/// value. The keys road is WebKit's editing command, else the synthetic
/// events controlled forms listen to.
pub(crate) const TYPE_BODY: &str = r#"
const selected = zcSelect(request.selector);
if (selected.code) return zcFail(selected.code);
const element = selected.element;
if (!zcVisible(element)) return zcFail("element_not_visible");
if (element.matches && element.matches(":disabled")) return zcFail("element_disabled");
if (element.readOnly) return zcFail("element_read_only");
const input = element instanceof HTMLInputElement;
const area = element instanceof HTMLTextAreaElement;
const editable = element.isContentEditable;
if (!input && !area && !editable) return zcFail("element_not_editable");
if (input && String(element.type).toLowerCase() === "file") return zcFail("element_not_editable");
if ((input || area) && element.maxLength >= 0 && request.text.length > element.maxLength) {
  return zcFail("text_too_long");
}
const secureField = input && (String(element.type).toLowerCase() === "password"
  || String(element.autocomplete || "").toLowerCase() === "current-password");
if (request.road === "keys" && secureField) {
  return zcEncode({ ok: true, value: { method: "held", secureField } });
}
element.scrollIntoView({ block: "center", inline: "center", behavior: "auto" });
element.focus({ preventScroll: true });
if (request.road === "setter") {
  if (input || area) {
    const prototype = input ? HTMLInputElement.prototype : HTMLTextAreaElement.prototype;
    Object.getOwnPropertyDescriptor(prototype, "value").set.call(element, request.text);
  } else {
    element.textContent = request.text;
  }
  element.dispatchEvent(typeof InputEvent === "function"
    ? new InputEvent("input", { bubbles: true, composed: true, inputType: "insertReplacementText" })
    : new Event("input", { bubbles: true }));
  return zcEncode({ ok: true, value: { method: "value-setter", secureField,
    blockPath: zcStructuralChain(element, request.blockRoots), pageUrl: zcSafeUrl(location.href) } });
}
if (input || area) {
  try { element.select(); } catch (_) {}
} else {
  const range = document.createRange();
  range.selectNodeContents(element);
  const selection = getSelection();
  selection.removeAllRanges();
  selection.addRange(range);
}
let edited = false;
let method = "editing-command";
try {
  edited = document.execCommand("insertText", false, request.text);
} catch (_) {}
const current = input || area ? element.value : element.textContent;
if (!edited || current !== request.text) {
  method = "synthetic-events";
  const before = typeof InputEvent === "function"
    ? new InputEvent("beforeinput", { bubbles: true, cancelable: true, composed: true,
        data: request.text, inputType: "insertReplacementText" })
    : new Event("beforeinput", { bubbles: true, cancelable: true });
  if (!element.dispatchEvent(before)) return zcFail("input_cancelled");
  if (input || area) {
    const prototype = input ? HTMLInputElement.prototype : HTMLTextAreaElement.prototype;
    const setter = Object.getOwnPropertyDescriptor(prototype, "value").set;
    setter.call(element, request.text);
  } else {
    element.textContent = request.text;
  }
  const inputEvent = typeof InputEvent === "function"
    ? new InputEvent("input", { bubbles: true, composed: true,
        data: request.text, inputType: "insertReplacementText" })
    : new Event("input", { bubbles: true });
  element.dispatchEvent(inputEvent);
}
return zcEncode({ ok: true, value: { method, secureField,
  blockPath: zcStructuralChain(element, request.blockRoots), pageUrl: zcSafeUrl(location.href) } });
"#;

/// Wait until a CSS selector names a visible element. The repeated callback
/// is deliberate: WKWebView's `evaluateJavaScript` callback does not await a
/// Promise, so Rust owns the clock and the timeout instead of leaving an
/// unbounded timer behind in the logged-in page.
#[tauri::command]
pub(crate) async fn browser_wait(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    label: String,
    selector: String,
    timeout_ms: Option<u64>,
) -> Result<(), String> {
    from_the_main_webview(&webview)?;
    automate_wait(&app, &state, &label, &selector, timeout_ms).await
}

pub(crate) async fn automate_wait(
    app: &AppHandle,
    state: &AppState,
    label: &str,
    selector: &str,
    timeout_ms: Option<u64>,
) -> Result<(), String> {
    checked_selector(selector)?;
    let timeout_ms = timeout_ms.unwrap_or(BROWSER_WAIT_DEFAULT_MS);
    if !(BROWSER_WAIT_MIN_MS..=BROWSER_WAIT_MAX_MS).contains(&timeout_ms) {
        return Err(format!(
            "대기 시간은 {BROWSER_WAIT_MIN_MS}~{BROWSER_WAIT_MAX_MS}ms여야 합니다"
        ));
    }
    let pane = browser_pane_of(app, state, label)?;
    let script = automation_script(
        &serde_json::json!({ "selector": selector }),
        // The same picker every other verb reads: the first match a person
        // could see. A page whose only visible match stands behind a hidden
        // twin used to time this wait out with the control on screen.
        r#"
const selected = zcSelect(request.selector);
if (selected.code === "invalid_selector") return zcFail("invalid_selector");
return zcEncode({ ok: true, value: !selected.code && !selected.hidden && zcVisible(selected.element) });
"#,
    );
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(WAIT_TIMED_OUT.to_string());
        }
        let callback_deadline = remaining.min(BROWSER_CALLBACK_DEADLINE);
        let reply = page_json(&pane, script.clone(), callback_deadline).await?;
        if page_value(reply)?.as_bool() == Some(true) {
            return Ok(());
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(WAIT_TIMED_OUT.to_string());
        }
        tokio::time::sleep(BROWSER_WAIT_POLL.min(remaining)).await;
    }
}

/// Arm element-grab in a browser pane (1-g5). The overlay is the page's; the
/// window learns of a pick by polling `browser_grab_take`.
#[tauri::command(async)]
pub(crate) fn browser_grab_arm(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    label: String,
    mode: Option<String>,
) -> Result<(), String> {
    from_the_main_webview(&webview)?;
    // Two intents, one door — Orca's copy and annotate (1-g8). The page gets
    // the installer that matches, and everything downstream (take, disarm)
    // reads the same globals either way.
    let installer = if mode.as_deref() == Some("annotate") {
        BROWSER_ANNOTATE_INSTALLER
    } else {
        BROWSER_GRAB_INSTALLER
    };
    let pane = browser_pane_of(&app, &state, &label)?;
    // The harvest first: both installers spread `__zerocodeHarvest(el)` into
    // their payloads, and an installer that runs without it throws at pick.
    pane.eval(GRAB_HARVEST_JS)
        .map_err(|error| error.to_string())?;
    // And the hover tag, for the same reason (1-g18 shares it like 1-g12
    // shares the harvest).
    pane.eval(GRAB_DIM_JS).map_err(|error| error.to_string())?;
    pane.eval(installer).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn browser_grab_take(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    label: String,
) -> Result<GrabPoll, String> {
    from_the_main_webview(&webview)?;
    let pane = browser_pane_of(&app, &state, &label)?;
    let parsed = page_json(
        &pane,
        "(() => { const g = window.__zerocodeGrab; window.__zerocodeGrab = null; \
         return JSON.stringify({picked: g || null, cancelled: !!window.__zerocodeGrabCancelled}); })()",
        BROWSER_CALLBACK_DEADLINE,
    )
    .await?;
    Ok(GrabPoll {
        picked: parsed
            .get("picked")
            .filter(|value| !value.is_null())
            .cloned(),
        cancelled: parsed
            .get("cancelled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    })
}

/// What the page saw at the last right-click, or `None`. Taken rather than
/// read: the window asks only while a browser pane is in front, and a menu
/// answered twice would open again over its own dismissal (1-g13).
#[tauri::command]
pub(crate) async fn browser_menu_take(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    label: String,
) -> Result<Option<serde_json::Value>, String> {
    from_the_main_webview(&webview)?;
    let pane = browser_pane_of(&app, &state, &label)?;
    let parsed = page_json(
        &pane,
        &browser_guest_script(
            "(() => { const state = window.__GUEST_STATE__; const m = state?.menu; \
             if (state) state.menu = null; return JSON.stringify(m || null); })()",
        ),
        BROWSER_CALLBACK_DEADLINE,
    )
    .await?;
    Ok(if parsed.is_null() { None } else { Some(parsed) })
}

#[tauri::command(async)]
pub(crate) fn browser_grab_disarm(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    label: String,
) -> Result<(), String> {
    from_the_main_webview(&webview)?;
    browser_pane_of(&app, &state, &label)?
        .eval(BROWSER_GRAB_REMOVER)
        .map_err(|error| error.to_string())
}

/// Give the keyboard back to the window on demand.
///
/// macOS does not always move first responder off a native child webview
/// when the person clicks the window's own surface — so typing kept running
/// into the visible page while a terminal LOOKED focused (live report
/// 2026-08-14: "인풋도 안 쳐지고"). The window calls this on any pointer
/// that lands in its DOM while a browser pane exists: a click in the DOM is
/// a claim on the keyboard, because clicks on the native pane never reach
/// the DOM at all.
#[tauri::command(async)]
pub(crate) fn focus_main(webview: tauri::Webview) -> Result<(), String> {
    from_the_main_webview(&webview)?;
    webview.set_focus().map_err(|error| error.to_string())
}

#[tauri::command(async)]
pub(crate) fn close_browser_pane(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    label: String,
) -> Result<(), String> {
    from_the_main_webview(&webview)?;
    if !state.browser_panes().contains(&label) {
        return Err("이 창이 만든 브라우저 판이 아닙니다".to_string());
    }
    // Hidden first, consumed LAST (gpt-sol review of 1-fy, finding 6): a
    // close that fails after the mint burned would leave a native pane
    // nobody can ever address again. Hiding is the best-effort mercy — even
    // a close that keeps failing leaves no page COVERING the window — and
    // the mint survives an error so the close can be tried again.
    //
    // Through the ENGINE-NEUTRAL handle: a Tauri webview lookup knows
    // nothing of a Chromium pane, so the close burned the mint and walked
    // away while the CEF view stayed painted over the stage — a closed tab's
    // page covering the terminal that took its place (live report
    // 2026-09-09: "터미널창을 덮어버리는 버그"). A pane the engine already
    // lost still gives up its label, as before.
    if let Ok(pane) = browser_pane_of(&app, &state, &label) {
        let _ = pane.hide();
        pane.close().map_err(|error| error.to_string())?;
    }
    state.browser_panes().remove(&label);
    state.browser_urls().remove(&label);
    state.browser_records().remove(&label);
    Ok(())
}

/// A webview exception's landing strip.
///
/// The release webview has no console anybody can read, so a thrown listener
/// dies silently and its symptom arrives as a report with nothing to hold it
/// to. Every uncaught error lands here instead — one line, stamped, capped:
/// an error loop must trim the file, never fill a disk.
#[tauri::command(async)]
pub(crate) fn note_webview_error(state: State<'_, AppState>, text: String) {
    let file = state.local_data_root().join(artifact_file::WEBVIEW_ERRORS);
    if std::fs::metadata(&file).is_ok_and(|held| held.len() > 512 * 1024) {
        let _ = std::fs::remove_file(&file);
    }
    let line: String = text.chars().take(2000).collect();
    if let Ok(mut held) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
    {
        use std::io::Write;
        let _ = writeln!(held, "{} {line}", epoch_ms_now());
    }
}
