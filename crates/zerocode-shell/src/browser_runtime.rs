use super::*;

/* ---- 브라우저 판 (1-fy) --------------------------------------------------
 *
 * Orca's browser tab, on Tauri's chassis. Orca embeds a guest WebContents and
 * the renderer holds the tab state (`BrowserPane`,
 * unsaved-close-queue-BHrI4TA0.js); here the pane is a CHILD WEBVIEW the
 * window positions over its own surface — same shape, different engine. The
 * window owns the toolbar and the address bar; this side owns the page.
 *
 * The page never gets IPC: a child webview on an external URL is outside the
 * app origin, so nothing here is reachable from the sites it visits. Traffic
 * flows one way — builder hooks below EMIT what the page did (navigated,
 * titled, asked for a popup) and the window repaints from the events. */

/// What a browser pane may load. The allowlist is the point: the label
/// commands arrive over loopback IPC, and a scheme like `javascript:` or a
/// platform handler (`ssh:`, `vscode:`) reached through them would run with
/// the pane's shoulders. `http`/`https` browse, `file` is the address bar's
/// local-path road (Orca converts absolute paths the same way), `data` and
/// `about` are the blank page.
/// The engine's own empty page — the one `about` address `browsable_target`
/// admits, and where a pane with imported cookies waiting is born (see
/// `open_browser_pane`).
pub(super) const BROWSER_BLANK_URL: &str = "about:blank";

pub(super) fn blank_page() -> tauri::Url {
    tauri::Url::parse(BROWSER_BLANK_URL).expect("about:blank parses")
}

pub(super) fn browsable(url: &str) -> Result<tauri::Url, String> {
    let parsed: tauri::Url = url
        .parse()
        .map_err(|_| "주소로 읽을 수 없습니다".to_string())?;
    if browsable_target(&parsed) {
        Ok(parsed)
    } else {
        Err(format!(
            "{}: 브라우저 판이 여는 주소가 아닙니다",
            parsed.scheme()
        ))
    }
}

/// Whether a parsed address is one a browser pane may stand on. One function
/// for BOTH doors — the commands and the page's own `on_navigation` — so a
/// page cannot redirect itself somewhere the address bar would have refused
/// (gpt-sol review of 1-fy, findings 1·2·7):
///
/// - `javascript:` and platform handlers (`ssh:`, `vscode:`) would run with
///   the pane's shoulders — refused with every other unnamed scheme.
/// - `about` means exactly `about:blank`, the engine's own empty page.
/// - `data` is refused outright: the engine needs a feature flag to open one
///   as a root URL anyway, and an arbitrary `data:` document is an arbitrary
///   active document.
/// - `http(s)` must not reach the app's own origin: on Windows the bundle is
///   served as `http(s)://tauri.localhost`, and a page landing there boots
///   this window's own script WITH IPC — the one invariant the guest must
///   never break. Plain `localhost:PORT` stays browsable; dev servers are
///   half of what a workspace browser is FOR.
pub(super) fn browsable_target(parsed: &tauri::Url) -> bool {
    match parsed.scheme() {
        "http" | "https" => !parsed
            .host_str()
            .is_some_and(|host| host == "tauri.localhost" || host.ends_with(".tauri.localhost")),
        "file" => true,
        "about" => parsed.as_str() == "about:blank",
        _ => false,
    }
}

/// Only the main window steers browser panes. The capability file scopes IPC
/// to the named webviews, but the board pop-out is named too and runs this
/// same script — a pane it opened would attach to a window it cannot see,
/// and a pane it closed would be somebody else's.
pub(super) fn from_the_main_webview(webview: &tauri::Webview) -> Result<(), String> {
    if webview.label() == "main" {
        Ok(())
    } else {
        Err("브라우저 판은 메인 창만 조종합니다".to_string())
    }
}

/// Engine-neutral handle used by browser commands. On macOS it resolves a
/// CEF child NSView; other platforms retain Tauri's existing webview.
#[cfg(all(target_os = "macos", feature = "chromium-browser"))]
pub(super) use crate::chromium_browser::BrowserPane;
#[cfg(not(all(target_os = "macos", feature = "chromium-browser")))]
pub(super) type BrowserPane = tauri::Webview;

/// The pane a label names — after the mint check. Both halves matter: the set
/// proves the label is one THIS window handed out, and the lookup proves the
/// webview is still alive.
pub(super) fn browser_pane_of(
    app: &AppHandle,
    state: &AppState,
    label: &str,
) -> Result<BrowserPane, String> {
    if !state.browser_panes().contains(label) {
        return Err("이 창이 만든 브라우저 판이 아닙니다".to_string());
    }
    #[cfg(all(target_os = "macos", feature = "chromium-browser"))]
    {
        let pane = BrowserPane::new(app, label.to_string());
        pane.exists()
            .then_some(pane)
            .ok_or_else(|| "브라우저 판이 이미 닫혔습니다".to_string())
    }
    #[cfg(not(all(target_os = "macos", feature = "chromium-browser")))]
    app.get_webview(label)
        .ok_or_else(|| "브라우저 판이 이미 닫혔습니다".to_string())
}

/// What a page did, told to the window. `state` is `started` while the wheel
/// should spin and `finished` when it should stop — the window keeps the
/// address bar from these rather than asking.
#[derive(Clone, Serialize)]
pub(super) struct BrowserNav {
    pub(super) label: String,
    pub(super) url: String,
    pub(super) state: &'static str,
}

#[derive(Clone, Serialize)]
pub(super) struct BrowserTitle {
    pub(super) label: String,
    pub(super) title: String,
}

/// One download's milestones. `started` carries the reserved destination —
/// macOS never reports the finished path (tauri DownloadEvent docs), so the
/// window keeps the one this side chose. No progress: the platform hook has
/// no byte stream to offer (recorded deviation from Orca's progress row).
#[derive(Clone, Serialize)]
pub(super) struct BrowserDownload {
    pub(super) label: String,
    /// `started`, `finished` or `failed`.
    pub(super) state: &'static str,
    pub(super) url: String,
    pub(super) path: String,
    pub(super) file: String,
}

/// Mint the next label and hold it for a caller that will open the pane
/// later — the agents' door, which answers the label BEFORE the window has
/// opened anything (§2.2). Monotonic like every mint; a reservation the
/// window never claims is a name that was simply never used.
pub(super) fn reserve_label(
    born: &mut u64,
    reserved: &mut std::collections::HashSet<String>,
) -> String {
    *born += 1;
    let label = format!("browser-{born}");
    reserved.insert(label.clone());
    label
}

/// The label a new pane wears: a fresh mint when nobody asked for one, or
/// the reserved one when the window brings a reservation back. Anything
/// else is refused — a label cannot be guessed into existence, and a
/// reservation is claimed once.
pub(super) fn claim_label(
    born: &mut u64,
    reserved: &mut std::collections::HashSet<String>,
    asked: Option<&str>,
) -> Result<String, String> {
    match asked {
        None => {
            *born += 1;
            Ok(format!("browser-{born}"))
        }
        Some(label) if reserved.remove(label) => Ok(label.to_string()),
        Some(_) => Err("예약되지 않은 브라우저 라벨입니다".to_string()),
    }
}

/// An agent's `close` for one pane (`browser:agent-close`): the window walks
/// its ordinary close (hide→close→burn) and the door watches the mint.
#[derive(Clone, Serialize)]
pub(super) struct BrowserAgentClose {
    pub(super) label: String,
}

/// An agent's `viewport` for one pane (`browser:agent-viewport`): a preset
/// id or `WxH` as the tab keeps it, nothing for the default seat.
#[derive(Clone, Serialize)]
pub(super) struct BrowserAgentViewport {
    pub(super) label: String,
    pub(super) viewport: Option<String>,
}

/// A popup the page asked for. Denied as a WINDOW and handed to the webview
/// as a fact instead — Orca routes `browser:popup` into a new browser tab,
/// and a real popup window would be a surface nothing in the app addresses.
#[derive(Clone, Serialize)]
pub(super) struct BrowserPopup {
    pub(super) label: String,
    pub(super) url: String,
    /// The terminal whose agent asked (`zerocode-browser open`), when known.
    /// The window seats the tab in that terminal's checkout; a popup carries
    /// none and lands beside the page that spawned it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) term: Option<u32>,
}

/// A browser profile: its own cookie jar and storage under a
/// `WKWebsiteDataStore` identifier (macOS 14+), named by the person. The id
/// is 16 urandom bytes as hex, minted once — the data store lives under it,
/// so renaming a profile must never re-mint it. Names and ids only; no
/// secret ever belongs in this file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct BrowserProfile {
    pub(super) id: String,
    pub(super) name: String,
    /// 마지막으로 쿠키를 가져온 곳. 없으면 아직 한 번도 안 가져온 것이고,
    /// 옛 파일에는 이 칸이 없으므로 `default`가 그 판을 읽어 준다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) source: Option<BrowserProfileSource>,
}

/// 한 프로필의 쿠키가 온 자리 — 설정 페이지의 "마지막으로 가져온 곳" 한 줄.
/// 이름은 표에서 오므로 소스 브라우저를 지운 뒤에도 문장이 남는다.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BrowserProfileSource {
    pub(super) family: String,
    pub(super) label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) profile: Option<String>,
}

pub(super) fn browser_profiles_file(config_root: &Path) -> PathBuf {
    config_root.join("browser-profiles.json")
}

pub(super) fn save_browser_profiles(
    config_root: &Path,
    held: &[BrowserProfile],
) -> Result<(), String> {
    let text = serde_json::to_string_pretty(held).map_err(|error| error.to_string())?;
    std::fs::write(browser_profiles_file(config_root), text).map_err(|error| error.to_string())
}

/// 이 프로필이 방금 어디에서 마셨는지를 적어 둔다.
///
/// 실패는 가져오기의 실패가 아니다 — 쿠키는 이미 스테이징에 앉았고, 여기서
/// 못 적히는 것은 "어디에서 왔는지 한 줄"뿐이다. 그래서 결과를 삼킨다.
pub(super) fn remember_cookie_source(
    config_root: &Path,
    target: &str,
    family: &str,
    profile: Option<String>,
) {
    let Some(label) = browser_cookie_import::family_label(family) else {
        return;
    };
    let mut held = load_browser_profiles(config_root);
    let Some(one) = held.iter_mut().find(|held| held.id == target) else {
        return;
    };
    one.source = Some(BrowserProfileSource {
        family: family.to_string(),
        label: label.to_string(),
        profile,
    });
    let _ = save_browser_profiles(config_root, &held);
}

pub(super) fn load_browser_profiles(config_root: &Path) -> Vec<BrowserProfile> {
    let Ok(text) = std::fs::read_to_string(browser_profiles_file(config_root)) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

/// The sixteen bytes a profile id names, if it is one of ours. The shape
/// check IS the security check: the id reaches `data_store_identifier`, and
/// only 32 lowercase hex characters ever leave `create_browser_profile`.
pub(super) fn browser_profile_store(id: &str) -> Option<[u8; 16]> {
    if id.len() != 32 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut bytes = [0u8; 16];
    for (at, slot) in bytes.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&id[at * 2..at * 2 + 2], 16).ok()?;
    }
    Some(bytes)
}

/// 새 브라우저 탭이 기본으로 입는 프로필의 id를 담는 곁파일(sidecar).
///
/// `browser-profiles.json`은 날 배열이라 거기에 기본 id를 끼우면 옛 파일의
/// 마이그레이션이 생긴다 — 곁파일은 그 이사를 피한다. 없으면 내장 기본
/// 저장소(profile 없음)이고, 있으면 그 프로필이다. 값은 우리 mint가 지은
/// 32-hex뿐이라, 읽을 때 모양 검증이 곧 담장이다.
pub(super) fn browser_default_profile_file(config_root: &Path) -> PathBuf {
    config_root.join("browser-default-profile")
}

pub(super) fn load_default_browser_profile(config_root: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(browser_default_profile_file(config_root)).ok()?;
    let id = raw.trim().to_string();
    browser_profile_store(&id).map(|_| id)
}

pub(super) fn save_default_browser_profile(
    config_root: &Path,
    id: Option<&str>,
) -> Result<(), String> {
    let path = browser_default_profile_file(config_root);
    match id {
        Some(id) => std::fs::write(&path, id).map_err(|error| error.to_string()),
        None => match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        },
    }
}

/// 설정 페이지가 그리는 한 프로필 — 저장 struct(`BrowserProfile`)에 창이
/// 그릴 두 가지 파생 상태를 더한 것: 지금 스테이징에 마실 쿠키가 앉아 있는가
/// (`staged`, "적용 대기" 칩), 새 탭의 기본 프로필인가(`is_default`, "활성"
/// 배지). 저장 파일에는 넣지 않는다 — 파생값이라 물을 때마다 센다.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BrowserProfileView {
    pub(super) id: String,
    pub(super) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) source: Option<BrowserProfileSource>,
    pub(super) staged: bool,
    pub(super) is_default: bool,
}

/// 저장된 프로필들에 파생 상태(스테이징·기본)를 입혀 창에 건넨다.
pub(super) fn browser_profile_views(config_root: &Path) -> Vec<BrowserProfileView> {
    let default_id = load_default_browser_profile(config_root);
    load_browser_profiles(config_root)
        .into_iter()
        .map(|held| {
            let staged = browser_cookie_import::staging_file(config_root, &held.id).exists();
            let is_default = default_id.as_deref() == Some(held.id.as_str());
            BrowserProfileView {
                id: held.id,
                name: held.name,
                source: held.source,
                staged,
                is_default,
            }
        })
        .collect()
}

/// 프로필 하나를 잊는다 — 목록에서 지우고, 스테이징 파일을 지우고, 그것이
/// 기본이었으면 기본도 놓는다. WKWebsiteDataStore의 쿠키는 무작위 id 아래
/// 고아로 남지만(그 포맷은 비공개라 파일로 지울 길이 없다) 아무 프로필도 그
/// id를 더는 가리키지 않으므로 닿을 수 없다.
pub(super) fn remove_browser_profile(config_root: &Path, id: &str) -> Result<(), String> {
    if browser_profile_store(id).is_none() {
        return Err("프로필 id가 아닙니다".to_string());
    }
    let mut held = load_browser_profiles(config_root);
    let before = held.len();
    held.retain(|profile| profile.id != id);
    if held.len() == before {
        return Err("그 프로필이 없습니다".to_string());
    }
    save_browser_profiles(config_root, &held)?;
    let _ = std::fs::remove_file(browser_cookie_import::staging_file(config_root, id));
    if load_default_browser_profile(config_root).as_deref() == Some(id) {
        let _ = save_default_browser_profile(config_root, None);
    }
    Ok(())
}

/// 새 탭이 입을 기본 프로필을 고른다(None이면 내장 기본 저장소로 돌아간다).
/// 우리 mint가 지은, 실제로 존재하는 프로필만 기본이 될 수 있다.
pub(super) fn choose_default_browser_profile(
    config_root: &Path,
    id: Option<&str>,
) -> Result<(), String> {
    match id {
        Some(id) => {
            if browser_profile_store(id).is_none() {
                return Err("프로필 id가 아닙니다".to_string());
            }
            if !load_browser_profiles(config_root)
                .iter()
                .any(|profile| profile.id == id)
            {
                return Err("그 프로필이 없습니다".to_string());
            }
            save_default_browser_profile(config_root, Some(id))
        }
        None => save_default_browser_profile(config_root, None),
    }
}

/// 가져오기 요약 — Orca `BrowserCookieImportSummary`의 낱말 그대로
/// (browser-workspace-types.ts:139-160). 창은 camelCase로 읽는다.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BrowserCookieImportSummary {
    pub(super) total_cookies: usize,
    pub(super) imported_cookies: usize,
    pub(super) skipped_cookies: usize,
    pub(super) google_cookies_skipped: usize,
    pub(super) partition_skipped_cookies: usize,
    pub(super) domains: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) warning: Option<BrowserCookieImportWarning>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BrowserCookieImportWarning {
    pub(super) code: &'static str,
    pub(super) failed_cookies: usize,
}

/// 파일에서 온 쿠키의 자리를 프로필에 적는다. 브라우저 소스와 달리 family
/// 표(`family_label`)에 없는 자리라, 라벨을 직접 준다 — 카드가 그 한 줄을
/// "마지막으로 cookies.txt에서 가져왔습니다"로 읽는다.
pub(super) fn remember_file_cookie_source(config_root: &Path, target: &str) {
    let mut held = load_browser_profiles(config_root);
    let Some(one) = held.iter_mut().find(|held| held.id == target) else {
        return;
    };
    one.source = Some(BrowserProfileSource {
        family: "file".to_string(),
        label: "cookies.txt".to_string(),
        profile: None,
    });
    let _ = save_browser_profiles(config_root, &held);
}

/// 스테이징된 가져오기 쿠키를 갓 태어난 판에 앉힌다. 실패는 쿠키 하나의
/// 실패다 — 한 줄 남기고 나머지는 계속 앉는다(주입은 최선 노력, 원본의
/// 인메모리 길과 같은 태도). `staged`는 판을 짓기 전에 `peek`한 그 목록이고,
/// 주입이 끝나야 스테이징이 소거된다 — 사이에서 죽으면 다음 판이 다시
/// 시도한다. 판은 이 함수가 도는 동안 about:blank에 서 있어야 한다: 목적지의
/// 첫 요청이 쿠키를 들고 나가는 것이 이 주입의 존재 이유다.
///
/// 로그 한 줄은 개수만 적는다 — 쿠키 값·도메인은 어디에도 남지 않는다.
// The wry pane seats cookies this way; CEF stages them through
// `navigate_after_cookies`, so this is compiled-but-unused in the Chromium
// build (its caller, the wry body, is cfg'd out there — t-3621).
#[cfg_attr(
    all(target_os = "macos", feature = "chromium-browser"),
    allow(dead_code)
)]
pub(super) fn inject_staged_cookies(
    local_data_root: &Path,
    pane: &tauri::Webview,
    config_root: &Path,
    profile_id: &str,
    staged: Vec<browser_cookie_import::StagedCookie>,
) {
    if staged.is_empty() {
        return;
    }
    let total = staged.len();
    // 저장소가 영영 거절할 쿠키는 시도하지 않는다 — 같은 판단을 CEF 쪽도
    // 쓴다(`chromium_browser::navigate_after_cookies`).
    let (staged, dropped) =
        browser_cookie_import::keep_acceptable(staged, browser_cookie_import::now_unix());
    let mut refused = 0usize;
    for cookie in staged {
        let mut built = tauri::webview::cookie::CookieBuilder::new(cookie.name, cookie.value)
            .domain(cookie.domain)
            .path(if cookie.path.is_empty() {
                "/".to_string()
            } else {
                cookie.path
            })
            .secure(cookie.secure)
            .http_only(cookie.http_only);
        if let Some(at) = cookie.expires_unix.and_then(|seconds| {
            tauri::webview::cookie::time::OffsetDateTime::from_unix_timestamp(seconds).ok()
        }) {
            built = built.expires(at);
        }
        if pane.set_cookie(built.build()).is_err() {
            refused += 1;
        }
    }
    browser_cookie_import::discard_staged_cookies(config_root, profile_id);
    let applied = total - refused - dropped.total();
    note_window_event(
        local_data_root,
        &format!(
            "cookie staging: applied {applied}/{total} for profile {profile_id} ({refused} refused, dropped {})",
            dropped
        ),
    );
}

/// The page-find highlighter (1-g33): wraps every match in a `<mark>`, tracks a
/// current index, scrolls it into view, and answers `{count,index}` — Orca's
/// "1/5 + highlight all", reached the injected-JS way because wry has no
/// native findInPage. Installed once per page; text mutations invalidate the
/// cached matches, including mutations in the same task as the next find.
/// Highlight changes are not observed, so an unchanged page still cycles.
/// Clearing disconnects the observer until the next query. The query crosses
/// as a JSON string literal, so no
/// page text escapes the script. Marks are capped so a pathological page
/// cannot wrap a million nodes.
pub(super) const BROWSER_FIND_INSTALL: &str = r##"if(!window.__zerocodeFind){window.__zerocodeFind={marks:[],idx:-1,query:null,cs:false,dirty:false,observer:null,
clear:function(){if(this.observer)this.observer.disconnect();this.dirty=false;for(var i=0;i<this.marks.length;i++){var m=this.marks[i],pp=m.parentNode;if(pp){pp.replaceChild(document.createTextNode(m.textContent),m);pp.normalize();}}this.marks=[];this.idx=-1;this.query=null;},
highlight:function(q,cs){this.clear();if(!q)return;this.query=q;this.cs=cs;var needle=cs?q:q.toLowerCase();
var walker=document.createTreeWalker(document.body||document.documentElement,NodeFilter.SHOW_TEXT,{acceptNode:function(n){if(!n.nodeValue||!n.nodeValue.trim())return NodeFilter.FILTER_REJECT;var pp=n.parentNode;if(!pp)return NodeFilter.FILTER_REJECT;var tg=pp.nodeName;if(tg==='SCRIPT'||tg==='STYLE'||tg==='NOSCRIPT'||tg==='MARK')return NodeFilter.FILTER_REJECT;return NodeFilter.FILTER_ACCEPT;}});
var nodes=[],n;while((n=walker.nextNode()))nodes.push(n);var cap=2000;
for(var i=0;i<nodes.length&&this.marks.length<cap;i++){var node=nodes[i],text=node.nodeValue,hay=cs?text:text.toLowerCase();var positions=[],from=0,pos;while((pos=hay.indexOf(needle,from))!==-1&&this.marks.length+positions.length<cap){positions.push(pos);from=pos+needle.length;}if(!positions.length)continue;var parent=node.parentNode;if(!parent)continue;var f=document.createDocumentFragment(),cursor=0;for(var k=0;k<positions.length;k++){var start=positions[k];if(start>cursor)f.appendChild(document.createTextNode(text.slice(cursor,start)));var mk=document.createElement('mark');mk.setAttribute('data-zc-find','');mk.style.cssText='background:#ffe066;color:#000';mk.textContent=text.slice(start,start+needle.length);f.appendChild(mk);this.marks.push(mk);cursor=start+needle.length;}if(cursor<text.length)f.appendChild(document.createTextNode(text.slice(cursor)));parent.replaceChild(f,node);}var self=this;if(!this.observer)this.observer=new MutationObserver(function(){self.dirty=true;});this.observer.observe(document,{subtree:true,childList:true,characterData:true});},
run:function(q,fwd,cs){if(q!==this.query||cs!==this.cs||this.dirty||(this.observer&&this.observer.takeRecords().length)){this.highlight(q,cs);this.idx=this.marks.length?0:-1;}else if(this.marks.length){this.idx=fwd?(this.idx+1)%this.marks.length:(this.idx-1+this.marks.length)%this.marks.length;}for(var i=0;i<this.marks.length;i++){this.marks[i].style.background=(i===this.idx)?'#ff9f1c':'#ffe066';}if(this.idx>=0){try{this.marks[this.idx].scrollIntoView({block:'center',inline:'nearest'});}catch(e){}}return{count:this.marks.length,index:this.idx>=0?this.idx+1:0};}};}"##;

/// The element-grab installer, injected into a guest page (1-g5).
///
/// Orca's "grab page element" reaches its guest through an Electron preload
/// (`extractHoverPayload`); ours has no IPC back — the app-origin isolation
/// that keeps a visited site from reaching this window also keeps the page
/// from reaching it. So the payload is left on a page global and polled for
/// (`browser_grab_take`), the one road our eval-only channel allows.
///
/// Self-contained vanilla JS, because it runs in whatever engine the page
/// is: it paints a crosshair and a hover outline, and the first click
/// captures the element's identity — tag, a stable selector, role, name,
/// text, box and a few load-bearing styles — onto `window.__zerocodeGrab`,
/// then removes itself. Escape cancels. Idempotent: a second arm re-uses the
/// same overlay rather than stacking listeners.
/// The dimension tag both grab intents wear on hover — `div 1280x215` by
/// the ring, Orca's own affordance (실행 화면 #45·#46). Installed once at
/// arm like the harvest (1-g12): two copies is how the two modes' tags
/// would come to disagree.
pub(super) const GRAB_DIM_JS: &str = r##"(() => {
  if (window.__zerocodeDimTag) return "dimmed";
  let dim = null;
  window.__zerocodeDimTag = (el) => {
    if (!el || !el.getBoundingClientRect) return;
    if (!dim || !dim.isConnected) {
      dim = document.createElement("div");
      dim.style.cssText = "position:fixed;z-index:2147483646;pointer-events:none;background:#171717;color:#fafafa;font:11px -apple-system,sans-serif;padding:2px 6px;border-radius:4px;display:none";
      document.body.appendChild(dim);
    }
    const r = el.getBoundingClientRect();
    dim.textContent = el.tagName.toLowerCase() + " " + Math.round(r.width) + "x" + Math.round(r.height);
    dim.style.display = "block";
    dim.style.left = r.left + "px";
    dim.style.top = Math.max(2, r.top - 20) + "px";
  };
  window.__zerocodeDimHide = () => { if (dim) dim.style.display = "none"; };
  window.__zerocodeDimDrop = () => { if (dim) { dim.remove(); dim = null; } };
  return "dimmed";
})()"##;

pub(super) const BROWSER_GRAB_INSTALLER: &str = r##"(() => {
  if (window.__zerocodeGrabArmed) return "rearmed";
  window.__zerocodeGrabArmed = true;
  window.__zerocodeGrab = null;
  window.__zerocodeGrabCancelled = false;
  const prevCursor = document.documentElement.style.cursor;
  document.documentElement.style.cursor = "crosshair";
  const ring = document.createElement("div");
  ring.style.cssText = "position:fixed;z-index:2147483647;pointer-events:none;border:2px solid #f59e0b;background:rgba(245,158,11,0.12);border-radius:2px;transition:all 40ms ease;display:none";
  document.body.appendChild(ring);
  const outline = (el) => {
    if (!el) { ring.style.display = "none"; return; }
    const r = el.getBoundingClientRect();
    ring.style.display = "block";
    ring.style.left = r.left + "px"; ring.style.top = r.top + "px";
    ring.style.width = r.width + "px"; ring.style.height = r.height + "px";
  };
  const selectorOf = (el) => {
    if (el.id) return "#" + CSS.escape(el.id);
    const parts = [];
    let node = el;
    while (node && node.nodeType === 1 && parts.length < 5) {
      let part = node.tagName.toLowerCase();
      const parent = node.parentElement;
      if (parent) {
        const same = [...parent.children].filter((c) => c.tagName === node.tagName);
        if (same.length > 1) part += ":nth-of-type(" + (same.indexOf(node) + 1) + ")";
      }
      parts.unshift(part);
      node = node.parentElement;
    }
    return parts.join(" > ");
  };
  let over = null;
  const onMove = (e) => {
    over = e.target;
    outline(e.target);
    window.__zerocodeDimTag(e.target);
  };
  const cleanup = () => {
    window.__zerocodeGrabArmed = false;
    window.__zerocodeGrabCleanup = null;
    document.documentElement.style.cursor = prevCursor;
    ring.remove();
    window.__zerocodeDimDrop();
    document.removeEventListener("mousemove", onMove, true);
    document.removeEventListener("click", onClick, true);
    document.removeEventListener("keydown", onKey, true);
  };
  const pickOf = (el) => {
    const r = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    const nearby = [];
    let sib = el.parentElement ? el.parentElement.firstElementChild : null;
    while (sib && nearby.length < 4) {
      const t = (sib.innerText || "").trim();
      if (t && sib !== el) nearby.push(t.slice(0, 120));
      sib = sib.nextElementSibling;
    }
    return {
      url: location.href,
      tag: el.tagName.toLowerCase(),
      selector: selectorOf(el),
      role: el.getAttribute("role") || "",
      name: el.getAttribute("aria-label") || el.getAttribute("alt") || el.getAttribute("title") || "",
      text: (el.innerText || "").trim().slice(0, 500),
      rect: { w: Math.round(r.width), h: Math.round(r.height) },
      styles: {
        display: style.display, position: style.position,
        fontSize: style.fontSize, color: style.color, background: style.backgroundColor,
      },
      nearby,
      ...window.__zerocodeHarvest(el),
    };
  };
  const onClick = (e) => {
    e.preventDefault(); e.stopPropagation();
    window.__zerocodeGrab = pickOf(e.target);
    cleanup();
  };
  const onKey = (e) => {
    if (e.key === "Escape") { window.__zerocodeGrabCancelled = true; cleanup(); return; }
    // Orca's C: copy what the pointer is ON without clicking it — a click
    // can travel (a link, a button), and copying must not (배너 #46).
    if ((e.key === "c" || e.key === "C") && over) {
      e.preventDefault(); e.stopPropagation();
      window.__zerocodeGrab = { ...pickOf(over), via: "key-c" };
      cleanup();
    }
  };
  window.__zerocodeGrabCleanup = cleanup;
  document.addEventListener("mousemove", onMove, true);
  document.addEventListener("click", onClick, true);
  document.addEventListener("keydown", onKey, true);
  return "armed";
})()"##;

/// Take back the crosshair without a pick (the person left grab mode). The
/// installer hangs its own teardown on `__zerocodeGrabCleanup`, so the remover
/// calls it directly — no synthetic key event that a page could also hear.
pub(super) const BROWSER_GRAB_REMOVER: &str = r#"(() => {
  window.__zerocodeGrabCancelled = true;
  if (window.__zerocodeGrabCleanup) window.__zerocodeGrabCleanup();
  return "removed";
})()"#;

/// The annotate installer (1-g8) — Orca's second grab intent, measured from
/// its live card: element highlight with a dimension tag, then an inline
/// card AT the element (selector lines, a textarea, the two intents 변경 and
/// 질문, 취소/추가 with ⌘↩). The card lives IN the page for the same reason
/// the crosshair does: the native pane floats above this window's DOM, and
/// eval is the one road in. Adding leaves the pick on `__zerocodeGrab` with
/// `comment` and `intent` riding along; the window rearms for the next
/// element the way Orca's `grab.rearm()` does.
pub(super) const BROWSER_ANNOTATE_INSTALLER: &str = r##"(() => {
  if (window.__zerocodeGrabArmed) return "rearmed";
  window.__zerocodeGrabArmed = true;
  window.__zerocodeGrab = null;
  window.__zerocodeGrabCancelled = false;
  const prevCursor = document.documentElement.style.cursor;
  document.documentElement.style.cursor = "crosshair";
  const ring = document.createElement("div");
  ring.style.cssText = "position:fixed;z-index:2147483646;pointer-events:none;border:2px solid #f59e0b;background:rgba(245,158,11,0.12);border-radius:2px;transition:all 40ms ease;display:none";
  document.body.appendChild(ring);
  const selectorOf = (el) => {
    if (el.id) return "#" + CSS.escape(el.id);
    const parts = [];
    let node = el;
    while (node && node.nodeType === 1 && parts.length < 5) {
      let part = node.tagName.toLowerCase();
      const parent = node.parentElement;
      if (parent) {
        const same = [...parent.children].filter((c) => c.tagName === node.tagName);
        if (same.length > 1) part += ":nth-of-type(" + (same.indexOf(node) + 1) + ")";
      }
      parts.unshift(part);
      node = node.parentElement;
    }
    return parts.join(" > ");
  };
  const outline = (el) => {
    if (!el) { ring.style.display = "none"; window.__zerocodeDimHide(); return; }
    const r = el.getBoundingClientRect();
    ring.style.display = "block";
    ring.style.left = r.left + "px"; ring.style.top = r.top + "px";
    ring.style.width = r.width + "px"; ring.style.height = r.height + "px";
    window.__zerocodeDimTag(el);
  };
  let card = null;
  const closeCard = () => { if (card) { card.remove(); card = null; } };
  const cleanup = () => {
    window.__zerocodeGrabArmed = false;
    window.__zerocodeGrabCleanup = null;
    document.documentElement.style.cursor = prevCursor;
    ring.remove(); window.__zerocodeDimDrop(); closeCard();
    document.removeEventListener("mousemove", onMove, true);
    document.removeEventListener("click", onClick, true);
    document.removeEventListener("keydown", onKey, true);
  };
  const onMove = (e) => { if (!card) outline(e.target); };
  const onKey = (e) => {
    if (e.key !== "Escape") return;
    if (card) { closeCard(); return; }
    window.__zerocodeGrabCancelled = true; cleanup();
  };
  const onClick = (e) => {
    if (card && card.contains(e.target)) return;
    e.preventDefault(); e.stopPropagation();
    const el = e.target;
    const r = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    const nearby = [];
    let sib = el.parentElement ? el.parentElement.firstElementChild : null;
    while (sib && nearby.length < 4) {
      const t = (sib.innerText || "").trim();
      if (t && sib !== el) nearby.push(t.slice(0, 120));
      sib = sib.nextElementSibling;
    }
    const picked = {
      url: location.href,
      tag: el.tagName.toLowerCase(),
      selector: selectorOf(el),
      role: el.getAttribute("role") || "",
      name: el.getAttribute("aria-label") || el.getAttribute("alt") || el.getAttribute("title") || "",
      text: (el.innerText || "").trim().slice(0, 500),
      rect: { w: Math.round(r.width), h: Math.round(r.height) },
      styles: {
        display: style.display, position: style.position,
        fontSize: style.fontSize, color: style.color, background: style.backgroundColor,
      },
      nearby,
      ...window.__zerocodeHarvest(el),
    };
    ring.style.display = "none"; window.__zerocodeDimHide();
    closeCard();
    card = document.createElement("div");
    card.style.cssText = "position:fixed;z-index:2147483647;background:#171717;color:#fafafa;border:1px solid rgba(255,255,255,0.12);border-radius:10px;padding:10px;width:300px;font:12px -apple-system,sans-serif;box-shadow:0 8px 30px rgba(0,0,0,0.5)";
    card.style.left = Math.min(Math.max(8, r.left), innerWidth - 316) + "px";
    card.style.top = Math.min(Math.max(8, r.bottom + 8), innerHeight - 230) + "px";
    const head = document.createElement("div");
    head.style.cssText = "margin-bottom:6px;opacity:0.8";
    head.textContent = picked.tag;
    const sel = document.createElement("div");
    sel.style.cssText = "margin-bottom:8px;font-size:11px;opacity:0.55;overflow:hidden;text-overflow:ellipsis;white-space:nowrap";
    sel.textContent = picked.selector;
    const area = document.createElement("textarea");
    area.style.cssText = "width:100%;box-sizing:border-box;height:64px;background:#0a0a0a;color:#fafafa;border:1px solid rgba(255,255,255,0.12);border-radius:6px;padding:6px;font:12px -apple-system,sans-serif;resize:none;outline:none";
    let intent = "change";
    const row = document.createElement("div");
    row.style.cssText = "display:flex;gap:6px;margin:8px 0";
    const face = (btn, on) => {
      btn.style.background = on ? "#fafafa" : "transparent";
      btn.style.color = on ? "#0a0a0a" : "#fafafa";
    };
    const mk = (word, value) => {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.textContent = word;
      btn.style.cssText = "flex:1;border:1px solid rgba(255,255,255,0.2);border-radius:6px;padding:4px 0;font:12px -apple-system,sans-serif;cursor:pointer";
      btn.addEventListener("click", () => {
        intent = value;
        face(changeBtn, value === "change");
        face(askBtn, value === "question");
      });
      return btn;
    };
    const changeBtn = mk("변경", "change");
    const askBtn = mk("질문", "question");
    face(changeBtn, true); face(askBtn, false);
    row.append(changeBtn, askBtn);
    const foot = document.createElement("div");
    foot.style.cssText = "display:flex;justify-content:flex-end;gap:6px";
    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.textContent = "취소";
    cancel.style.cssText = "border:0;background:transparent;color:#a1a1a1;font:12px -apple-system,sans-serif;cursor:pointer;padding:4px 8px";
    cancel.addEventListener("click", () => closeCard());
    const add = document.createElement("button");
    add.type = "button";
    add.textContent = "추가 ⌘↩";
    add.style.cssText = "border:0;background:#fafafa;color:#0a0a0a;border-radius:6px;font:12px -apple-system,sans-serif;cursor:pointer;padding:4px 10px";
    const submit = () => {
      const said = area.value.trim();
      if (!said) return;
      window.__zerocodeGrab = { ...picked, comment: said, intent };
      cleanup();
    };
    add.addEventListener("click", submit);
    area.addEventListener("keydown", (ke) => {
      if (ke.key === "Enter" && (ke.metaKey || ke.ctrlKey)) { ke.preventDefault(); submit(); }
      ke.stopPropagation();
    });
    foot.append(cancel, add);
    card.append(head, sel, area, row, foot);
    document.body.appendChild(card);
    area.focus();
  };
  window.__zerocodeGrabCleanup = cleanup;
  document.addEventListener("mousemove", onMove, true);
  document.addEventListener("click", onClick, true);
  document.addEventListener("keydown", onKey, true);
  return "armed";
})()"##;

/// The page-side harvest both grab intents share: the element's own markup
/// and the computed styles that shaped it. Orca's Design Mode sends "its
/// HTML, CSS" with every pick (FeatureWallModal tile-05); two installers
/// each growing their own copy is how the two roads would come to differ,
/// so the harvest is installed once and both spread it. The screenshot
/// third of Orca's trio needs a webview capture API wry does not expose —
/// an explicit seam, not an oversight (1-g12).
pub(super) const GRAB_HARVEST_JS: &str = r##"(() => {
  window.__zerocodeHarvest = (el) => {
    const cap = 4000;
    let html = el.outerHTML || "";
    if (html.length > cap) html = html.slice(0, cap) + "…";
    const style = getComputedStyle(el);
    const wanted = [
      "display", "position", "width", "height", "margin", "padding", "gap",
      "flex-direction", "justify-content", "align-items",
      "grid-template-columns", "font-family", "font-size", "font-weight",
      "line-height", "text-align", "color", "background-color",
      "background-image", "border", "border-radius", "box-shadow",
      "opacity", "overflow",
    ];
    const bland = new Set(["none", "normal", "auto", "0px", "rgba(0, 0, 0, 0)", "visible"]);
    const css = wanted
      .map((key) => ({ key, said: style.getPropertyValue(key) }))
      .filter(({ said }) => said && !bland.has(said))
      .map(({ key, said }) => key + ": " + said + ";")
      .join("\n");
    return { html, css };
  };
  return "harvested";
})()"##;

/// The page-side half of the context menu (1-g13). wry gives a pane no
/// right-click event of its own, so the page keeps the answer where the
/// window can come for it — the same shape the grab road uses.
///
/// What Orca's main process reads off Chromium's `context-menu` params
/// (`setupGuestContextMenu`, main/index.js:147527) the page can read itself:
/// the link under the pointer, the selection, the address. Installed on
/// every main-document load rather than armed, because a right-click is
/// not a mode the person turns on.
pub(super) const BROWSER_MENU_JS: &str = r##"(() => {
  const state = window.__GUEST_STATE__;
  if (!state) return "missing";
  if (state.menuWatching) return "watching";
  state.menuWatching = true;
  state.menu = null;
  document.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    const link = e.target && e.target.closest ? e.target.closest("a[href]") : null;
    state.menu = {
      x: e.clientX,
      y: e.clientY,
      linkUrl: link ? link.href : "",
      selectionText: String(window.getSelection() || "").slice(0, 2000),
      pageUrl: location.href,
    };
  }, true);
  return "watching";
})()"##;

/// What one grab captured, or `None` while the person is still choosing.
/// `cancelled` tells the window the crosshair was dismissed with Escape so
/// its poll stops rather than spinning forever.
#[derive(Clone, Serialize)]
pub(super) struct GrabPoll {
    pub(super) picked: Option<serde_json::Value>,
    pub(super) cancelled: bool,
}

/// Hand a URL to the system browser.
///
/// Orca's `window.api.shell.openUrl`. The refusal is the interesting half: a
/// window that opens whatever string a webview hands it is a window that can be
/// made to run `file:///` or a custom scheme by anything that reaches this
/// channel, and check names and annotation bodies on this panel come from a
/// repository's CI. Two schemes, spelled out, and nothing else travels.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct OpenInLaunch {
    pub(super) program: String,
    pub(super) arguments: Vec<std::ffi::OsString>,
}

/// Build argv without a shell. The configured command may contain quoted
/// arguments, but the workspace itself is always appended as one OS string.
pub(super) fn open_in_application_launch(
    application: &OpenInApplication,
    workspace: &Path,
) -> Result<OpenInLaunch, String> {
    let mut words = split_command(&application.command);
    if words.is_empty() || words[0].trim().is_empty() {
        return Err("앱 명령을 입력하세요".to_string());
    }
    let program = words.remove(0);
    let is_cursor = Path::new(&program)
        .file_stem()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("cursor"));
    if is_cursor && !words.iter().any(|argument| argument == "--new-window") {
        words.push("--new-window".to_string());
    }
    let mut arguments: Vec<std::ffi::OsString> = words.into_iter().map(Into::into).collect();
    arguments.push(workspace.as_os_str().to_owned());
    Ok(OpenInLaunch { program, arguments })
}

pub(super) fn open_workspace_in_file_manager(workspace: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let launcher = "open";
    #[cfg(target_os = "linux")]
    let launcher = "xdg-open";
    #[cfg(target_os = "windows")]
    let launcher = "explorer";
    let mut opener = crate::proc::quiet_command(launcher);
    opener
        .arg(workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    zerocode_core::reap::spawn_forgotten(opener)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// The ONE launch road for a configured Open-in application, whatever the
/// target. The renderer sends only an id — the settings document supplies
/// the command, the caller supplies a path IT vouched for — the target rides
/// argv as its own item, PATH is hydrated and the child is reaped. Both
/// doors (workspace, file) walk this; a second copy is a second chance to
/// forget one of those four.
pub(super) fn spawn_open_in_application(
    repository: &settings::SettingsRepository,
    application_id: &str,
    target: &Path,
) -> Result<(), String> {
    let snapshot = load_settings(repository)?;
    let application = snapshot
        .document
        .open_in_applications
        .iter()
        .find(|application| application.id == application_id)
        .ok_or_else(|| "Open in 앱을 찾을 수 없습니다".to_string())?;
    let launch = open_in_application_launch(application, target)?;
    let mut opener = crate::proc::quiet_command(&launch.program);
    opener
        .args(&launch.arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(path) = shell_path::hydrated() {
        opener.env("PATH", path);
    }
    if zerocode_core::reap::spawn_forgotten(opener).is_ok() {
        return Ok(());
    }
    // The command is not on the PATH — which for these three is the ORDINARY
    // state of a machine, not a broken one. VS Code writes `code` only if you
    // ask it to from its own palette, and Zed keeps `zed` inside its bundle.
    // Reported live on a machine with Zed installed, its menu item doing
    // nothing whatsoever.
    //
    // So ask macOS for the application BY NAME, which is the door Finder
    // itself uses and wants nothing on any PATH. Second rather than first,
    // because a command somebody configured is a command they meant — a full
    // path, a wrapper, a flag — and the app of that name is only the answer
    // once that command turns out not to be there.
    open_in_application_by_name(macos_application_name(application), target)
}

/// Open an installed application BY NAME, with a path to open in it.
///
/// Waited on rather than forgotten, and that is the point rather than an
/// oversight: `open` is a launcher, so it returns as soon as the request is
/// accepted, and its exit code is the only thing that can tell "opened" from
/// "there is no such application here". The spawn above cannot report its own
/// failure at all, which is exactly why a menu item could look like it did
/// nothing.
#[cfg(target_os = "macos")]
pub(super) fn open_in_application_by_name(name: &str, target: &Path) -> Result<(), String> {
    let opened = crate::proc::quiet_command("open")
        .arg("-a")
        .arg(name)
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| error.to_string())?;
    if opened.success() {
        return Ok(());
    }
    Err(format!(
        "{name}을(를) 열지 못했습니다. 편집기의 명령줄 도구를 설치하거나 Open in 앱 설정에서 명령에 전체 경로를 적어 주세요"
    ))
}

/// Windows and Linux have no name-addressed launcher of their own: `explorer`
/// and `xdg-open` open a TARGET with whatever is registered for it, which is
/// the file-manager road and not this one. So the honest answer there is the
/// one the person can act on.
#[cfg(not(target_os = "macos"))]
pub(super) fn open_in_application_by_name(_name: &str, _target: &Path) -> Result<(), String> {
    Err(
        "명령을 PATH에서 찾지 못했습니다. Open in 앱 설정에서 명령에 전체 경로를 적어 주세요"
            .to_string(),
    )
}

/* ---- the review each board card is on (1-g78a) -----------------------------
 *
 * Orca's dashboard card wears a ReviewPill, and its snapshot builder fills it
 * from a per-worktree GitHub PR cache (`hostedReviewInfoFromGitHubPRInfo`).
 * The board asks for a set of checkouts at once and joins the answer against
 * its cards by the checkout's own path — the one identity a card's workspace
 * has that a person cannot rename. */
