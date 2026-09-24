/* ---- 브라우저 탭 (1-fy) --------------------------------------------------
 *
 * Orca's browser tab (`BrowserPane`, unsaved-close-queue-BHrI4TA0.js): a page
 * inside the workspace, with the toolbar this window draws and a native page
 * surface the platform draws. Orca embeds a guest WebContents; ours is a
 * Tauri child webview the backend positions over the leaf — so the layout
 * below is not CSS reaching the page but this window TELLING the backend
 * where the leaf is (`browser_bounds`/`browser_shown`), every time the answer
 * changes. The page cannot reach back: it has no IPC, and everything this
 * side knows arrives as `browser:*` events. */

/* Orca's blank page is a data URL (`ORCA_BROWSER_BLANK_URL`); OURS is the
 * engine's own empty page, because Tauri refuses a `data:` root without a
 * feature flag this build does not carry (gpt-sol review of 1-fy, finding 1
 * — the mock hid it). The address bar shows it as empty either way. */
const BROWSER_BLANK_URL = "about:blank";
/* Coalesce a navigation burst into one bounded history write. */
const BROWSER_VISITS_SAVE_MS = 800;
/* The zoom figure stays long enough to confirm a key press, then yields the
 * page its full viewport again. */
const BROWSER_PAGE_ZOOM_FEEDBACK_MS = 1400;

/* The search engines Orca ships (store-BgJxB0hr.js:1306035), with its own
 * default. A settings door can pick between them later; the table is the
 * measured data either way. */
const SEARCH_ENGINE_URLS = {
  google: "https://www.google.com/search?q=",
  duckduckgo: "https://duckduckgo.com/?q=",
  bing: "https://www.bing.com/search?q=",
  kagi: "https://kagi.com/search?q=",
};

/* The words the settings select already shows, for the suggestion row. */
const SEARCH_ENGINE_NAMES = {
  google: "Google",
  duckduckgo: "DuckDuckGo",
  bing: "Bing",
  kagi: "Kagi",
};

/* Where the address bar's suggestions come from — Orca's history entries,
 * written on page-title-updated (addBrowserHistoryEntry). Session-local for
 * now: the persisted ledger is the queued half (gap doc B02 ①), and one that
 * forgets on restart still ranks this run's pages the measured way. */
const browserVisits = new Map();
let browserVisitsSaver = 0;

/* The ledger goes to disk a breath after it moves — one write per burst of
 * navigation, most recent first, bounded to what the backend keeps. */
function rememberBrowserVisits() {
  clearTimeout(browserVisitsSaver);
  browserVisitsSaver = setTimeout(() => {
    const visits = [...browserVisits.entries()]
      .map(([url, one]) => ({ url, title: one.title, count: one.count, at: one.at }))
      .sort((left, right) => right.at - left.at)
      .slice(0, 200);
    void commitSetting("browser.visits", "set_browser_visits", { visits });
  }, BROWSER_VISITS_SAVE_MS);
}

/* A web address — the only kind the visits ledger and the tab-restore record
 * carry (the backend's `set_browser_visits` keeps http/https alone; its
 * `set_browser_open_tabs` admits these plus the blank page). */
function isWebAddress(url) {
  return /^https?:/.test(url);
}

function recordBrowserVisit(url, title) {
  if (!url || !isWebAddress(url)) return;
  const held = browserVisits.get(url) ?? { title: "", count: 0, at: 0 };
  browserVisits.set(url, {
    title: title || held.title,
    count: held.count + 1,
    at: Date.now(),
  });
  rememberBrowserVisits();
}

/* The list's whole law lives here, apart from the paint — Orca keeps the
 * same split (browser-address-bar-suggestions.ts holds the rows, the bar
 * only renders and arrows them). */
const BROWSER_SUGGEST_MAX_ROWS = 8; // browser-address-bar-suggestions.ts:13
const BROWSER_SUGGEST_QUERY_MAX_BYTES = 2 * 1024; // browser-address-bar-suggestions.ts:14

/* Orca's ranking, verbatim (scoreBrowserAddressBarSuggestion, :32-52): a
 * query in neither url nor title is a refusal; a url that starts with the
 * query — bare or behind https:// — earns 100; the visit count counts up
 * to 50; and up to 24 fade with each hour since the last visit. */
function scoreBrowserSuggestion(url, entry, said) {
  const lowerUrl = url.toLowerCase();
  const lowerTitle = (entry.title ?? "").toLowerCase();
  if (!lowerUrl.includes(said) && !lowerTitle.includes(said)) return -1;
  let score = 0;
  if (lowerUrl.startsWith(said) || lowerUrl.startsWith("https://" + said)) score += 100;
  score += Math.min(entry.count, 50);
  score += Math.max(0, 24 - (Date.now() - entry.at) / 3_600_000);
  return score;
}

/* The first row acts for the typed words themselves: a query gets the
 * engine's row, an address gets its normalized self. A scheme the ladder
 * refuses makes no synthetic row — the submit path's validation error must
 * answer, not a row that would hand the raw string to the pane
 * (browser-address-bar-suggestions.ts:91-122). */
function browserSuggestTopAction(needle) {
  if (looksLikeSearchQuery(needle)) {
    const engine = SEARCH_ENGINE_NAMES[browserPrefs.search_engine] ?? SEARCH_ENGINE_NAMES.google;
    return {
      kind: "search",
      url: searchUrlOf(needle),
      title: needle,
      subtitle: t("browser.suggest.search", "{{engine}} 검색", { engine }),
    };
  }
  const spoken = navigableUrlOf(needle);
  return spoken === null ? null : { kind: "open", url: spoken, title: needle };
}

/* Orca's buildBrowserAddressBarSuggestions, whole (:65-134): a query past
 * 2 KiB suppresses the list; an empty bar — or a blank page's address —
 * offers the most recent visits with no search row; otherwise the top
 * action leads at most seven ranked history rows, unless its url
 * duplicates one of them, in which case the history row keeps the spot —
 * it gives Enter the same target while showing real page metadata. */
function browserSuggestions(value) {
  if (new TextEncoder().encode(value).length > BROWSER_SUGGEST_QUERY_MAX_BYTES) return [];
  const needle = value.trim();
  if (needle === "" || needle === "about:blank" || needle.startsWith("data:")) {
    return [...browserVisits.entries()]
      .sort((left, right) => right[1].at - left[1].at)
      .slice(0, BROWSER_SUGGEST_MAX_ROWS)
      .map(([url, entry]) => ({ kind: "visit", url, title: entry.title }));
  }
  const said = needle.toLowerCase();
  const history = [...browserVisits.entries()]
    .map(([url, entry]) => ({ url, title: entry.title, score: scoreBrowserSuggestion(url, entry, said) }))
    .filter((one) => one.score >= 0)
    .sort((left, right) => right.score - left.score)
    .slice(0, BROWSER_SUGGEST_MAX_ROWS - 1)
    .map((one) => ({ kind: "visit", url: one.url, title: one.title }));
  const top = browserSuggestTopAction(needle);
  if (top === null || history.some((one) => one.url === top.url)) return history;
  return [top, ...history];
}
let browserPrefs = {
  home_page: "",
  search_engine: "google",
  restore_tabs: false,
  open_links_in_app: false,
  open_links_in_app_modifier_inverts: false,
  terminal_link_action_popover: true,
  default_zoom_level: 0,
  open_tabs: [],
  visits: [],
  user_agents: [],
};
let browserZoomSpec = null;

/* 크기만으로는 데스크톱 사이트가 좁게 접힐 뿐이다 — 판의 독자를 아이폰으로
 * 세워야 모바일 사이트가 온다(사실 문자열). 팔레트의 에뮬레이터 문과 탭
 * 복원(1-g25)이 같은 독자를 쓴다. */
const MOBILE_EMULATOR_UA =
  "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1";

/* 프로필은 자기 쿠키 단지다(1-g26): 백엔드가 이름·id를 지니고, 판은 태어날
 * 때 그 id의 데이터 스토어에 든다. 기본은 id 없음 — 늘 쓰던 공용 단지. */
/* 기계가 답한 시뮬레이터 목록의 캐시 — 팔레트를 여는 손보다 xcrun이 느리다.
 * null이면 아직 안 물어본 것; 실패는 빈 목록(행이 안 선다). */
let mobileEmulators = null;
let androidEmulators = null;

let browserProfiles = [];
/* 새 탭이 입는 기본 프로필의 id, 그리고 지금 쿠키를 읽고 있는 프로필의 id.
 * 둘 다 목록에서 파생되지만 목록 밖에서도 읽히므로(새 탭 여는 손, 카드 스핀)
 * 창 전역으로 든다 — refreshBrowserProfiles가 목록과 함께 늘 갱신한다. */
let browserDefaultProfileId = null;
let browserImportingId = null;
async function refreshBrowserProfiles() {
  try {
    browserProfiles = (await invoke("browser_profiles", {})) ?? [];
  } catch {
    browserProfiles = [];
  }
  browserDefaultProfileId = browserProfiles.find((one) => one.isDefault)?.id ?? null;
}
void refreshBrowserProfiles();

/* Orca's address classification, verbatim (store-BgJxB0hr.js:1305691-):
 * dev addresses get http, paths get file, everything with a scheme is taken
 * at its word, and what is left is a search or a bare host. */
const LOCAL_ADDRESS_PATTERN = /^(?:localhost|127(?:\.\d{1,3}){3}|0\.0\.0\.0|\[[0-9a-f:]+\])(?::\d+)?(?:[/?#].*)?$/i;
const LOOKS_LIKE_URL_PATTERN = /^[^\s]+\.[a-z]{2,}(\/.*)?$/i;
const WINDOWS_ABSOLUTE_PATH_PATTERN = /^[A-Za-z]:[\\/].*$/;
const WINDOWS_UNC_PATH_PATTERN = /^\\\\[^\s\\/]+[\\/][^\\/]+(?:[\\/].*)?$/;
const UNIX_ABSOLUTE_PATH_PATTERN = /^\/.*$/;

/* A space is a search; a dot or a colon without one is an address. Orca's
 * `looksLikeSearchQuery` exactly. */
function looksLikeSearchQuery(input) {
  if (input.includes(" ")) return true;
  if (LOOKS_LIKE_URL_PATTERN.test(input)) return false;
  if (input.includes(".") || input.includes(":")) return false;
  return true;
}

/* The engine's answer page for the words — Orca's `buildSearchUrl`, the
 * engine defaulted the same way the navigation ladder defaults it. */
function searchUrlOf(query) {
  const search = SEARCH_ENGINE_URLS[browserPrefs.search_engine] ?? SEARCH_ENGINE_URLS.google;
  return search + encodeURIComponent(query);
}

/* An absolute path as a file URL — the drive letter kept bare, every other
 * segment encoded (Orca's `absolutePathToFileUrl`). */
function pathAsFileUrl(filePath) {
  const normalized = filePath.replaceAll("\\", "/");
  const segments = normalized.split("/").map((segment, index) => {
    if (index === 0 && /^[A-Za-z]:$/.test(segment)) return segment;
    return encodeURIComponent(segment);
  });
  const joined = segments.join("/");
  return normalized.startsWith("/") ? "file://" + joined : "file:///" + joined;
}

function uncPathAsFileUrl(filePath) {
  const [host, ...rest] = filePath.replaceAll("\\", "/").replace(/^\/+/, "").split("/");
  return "file://" + host + "/" + rest.map(encodeURIComponent).join("/");
}

/* What the address bar's words mean — Orca's `normalizeBrowserNavigationUrl`
 * with search always on, engine defaulted. `null` means the words mean
 * nothing navigable (a scheme this window does not browse). */
function navigableUrlOf(raw) {
  const trimmed = raw.trim();
  if (trimmed.length === 0 || trimmed === "about:blank" || trimmed === "data:text/html,") {
    return BROWSER_BLANK_URL;
  }
  if (LOCAL_ADDRESS_PATTERN.test(trimmed)) {
    try {
      return new URL("http://" + trimmed).toString();
    } catch {
      /* not an address after all — fall through to the ladder below */
    }
  }
  if (WINDOWS_UNC_PATH_PATTERN.test(trimmed)) return uncPathAsFileUrl(trimmed);
  if (UNIX_ABSOLUTE_PATH_PATTERN.test(trimmed) || WINDOWS_ABSOLUTE_PATH_PATTERN.test(trimmed)) {
    return pathAsFileUrl(trimmed);
  }
  try {
    const parsed = new URL(trimmed);
    const spoken = parsed.protocol;
    return spoken === "http:" || spoken === "https:" || spoken === "file:" ? parsed.toString() : null;
  } catch {
    try {
      const withScheme = new URL("https://" + trimmed);
      if (!looksLikeSearchQuery(trimmed)) return withScheme.toString();
    } catch {
      /* not even with a scheme in front — a search, then */
    }
    return searchUrlOf(trimmed);
  }
}

/* What the "open in default browser" button may hand the system — Orca's
 * `normalizeExternalBrowserUrl`: never a local file, never the blank page. */
function externalBrowserUrl(raw) {
  const said = navigableUrlOf(raw ?? "");
  if (said === null || said === BROWSER_BLANK_URL) return null;
  if (said.startsWith("file:")) return null;
  return said;
}

/* Orca stores logarithmic half-step levels, not rounded percentages. Rust
 * owns the measured bounds and scale base; this window only projects that
 * spec into the factors Tauri's guest pane accepts. */
function browserZoomContract() {
  if (browserZoomSpec === null || typeof browserZoomSpec !== "object") return null;
  const contract = {
    min: Number(browserZoomSpec.min_level),
    max: Number(browserZoomSpec.max_level),
    step: Number(browserZoomSpec.step),
    defaultLevel: Number(browserZoomSpec.default_level),
    scaleBase: Number(browserZoomSpec.scale_base),
  };
  if (
    !Object.values(contract).every(Number.isFinite)
    || contract.step <= 0
    || contract.max < contract.min
    || contract.scaleBase <= 0
  ) return null;
  return contract;
}

function normalizeBrowserZoomLevel(level) {
  const contract = browserZoomContract();
  if (contract === null) return null;
  const numeric = Number(level);
  const wanted = Number.isFinite(numeric) ? numeric : contract.defaultLevel;
  const rounded = Math.round(wanted / contract.step) * contract.step;
  return Math.min(contract.max, Math.max(contract.min, rounded));
}

function browserZoomLevels() {
  const contract = browserZoomContract();
  if (contract === null) return [];
  const count = Math.round((contract.max - contract.min) / contract.step);
  return Array.from({ length: count + 1 }, (_, index) =>
    normalizeBrowserZoomLevel(contract.min + index * contract.step));
}

function browserZoomTarget(level) {
  const contract = browserZoomContract();
  const normalized = normalizeBrowserZoomLevel(level);
  if (contract === null || normalized === null) return null;
  return {
    level: normalized,
    scale: Math.pow(contract.scaleBase, normalized),
  };
}

function browserZoomPercent(level) {
  const target = browserZoomTarget(level);
  return target === null ? "" : String(Math.round(target.scale * 100)) + "%";
}

/* Orca's `scope: "browser"` as a question this window can ask: the front
 * tab is a browser AND nothing stands in front of the page. Both halves
 * matter — a chord aimed at a page nobody can see steers a hidden surface. */
function browserChordScope() {
  return currentTab()?.kind === "browser" && !browserCovered();
}

/* One rung up, one rung down, or back to the configured default — from
 * whatever the tab holds now, snapped to the nearest measured level first. */
/* "144%" for BROWSER_PAGE_ZOOM_FEEDBACK_MS = 1400ms (measured), then gone —
 * a zoom with no answer leaves the person counting presses. */
function sayBrowserZoom(tab, level) {
  const note = docHost(tab.pane, "browser")?.querySelector(".browser-zoom-note");
  if (!note) return;
  note.textContent = browserZoomPercent(level);
  note.hidden = false;
  clearTimeout(note._fade);
  note._fade = setTimeout(() => {
    note.hidden = true;
  }, BROWSER_PAGE_ZOOM_FEEDBACK_MS);
}

function browserZoomStep(tab, direction) {
  const levels = browserZoomLevels();
  const configured = browserZoomTarget(browserPrefs.default_zoom_level);
  if (levels.length === 0 || configured === null) return;
  const priorLevel = tab.zoomLevel;
  const priorScale = tab.zoom;
  const heldScale = Number.isFinite(priorScale) ? priorScale : configured.scale;
  const heldLevel = Number.isFinite(priorLevel)
    ? normalizeBrowserZoomLevel(priorLevel)
    : levels.reduce((best, level) => {
      const bestScale = browserZoomTarget(best)?.scale ?? configured.scale;
      const candidateScale = browserZoomTarget(level)?.scale ?? configured.scale;
      return Math.abs(candidateScale - heldScale) < Math.abs(bestScale - heldScale) ? level : best;
    }, levels[0]);
  const at = levels.indexOf(heldLevel);
  const nextLevel = direction === 0
    ? configured.level
    : levels[Math.min(levels.length - 1, Math.max(0, at + direction))];
  const next = browserZoomTarget(nextLevel);
  if (next === null || (next.level === heldLevel && next.scale === heldScale)) return;
  tab.zoomLevel = next.level;
  tab.zoom = next.scale;
  sayBrowserZoom(tab, next.level);
  // A refusal rolls the optimism back — a `tab.zoom` the pane never took
  // would make the NEXT step compute from a rung nobody is on, and a reset
  // that failed once could never be retried (`next === held` short-circuit).
  invoke("browser_zoom", { label: tab.label, scale: next.scale }).catch(() => {
    if (tab.zoomLevel !== next.level || tab.zoom !== next.scale) return;
    if (priorLevel === undefined) delete tab.zoomLevel;
    else tab.zoomLevel = priorLevel;
    if (priorScale === undefined) delete tab.zoom;
    else tab.zoom = priorScale;
  });
}

/* The geometry each pane was last told, so a repaint that moved nothing costs
 * no IPC — ResizeObserver and the stage both call in whenever anything
 * breathes. Cleared in `dropTab`, counted by the leak check. */
const browserSaid = new Map();

/* What a page said before its tab existed. `add_child` starts loading while
 * the open command is still in flight, so a fast page's nav/title events can
 * beat `openTab` (review finding 9) — they wait here, keyed by label, and
 * the open drains them. Bounded by in-flight opens; emptied on drain and on
 * `dropTab`. */
const browserEarly = new Map();

/* Hosts whose toolbar is wired. A WeakSet rather than a flag on the node:
 * `docHost` clones the template for a second leaf, and a clone carries the
 * flag it must not carry — listeners do not survive `cloneNode`. */
const browserWired = new WeakSet();

const browserBodyWatch = new ResizeObserver(() => syncBrowserPanes());

/* 좁은 판의 주소창(실측 browser-address-bar-expansion.ts): 다른 툴바
 * 컨트롤은 전부 flex:none이라 판이 좁아지면 짜부라드는 것은 주소창의
 * 슬롯뿐이다. 슬롯이 220px(:5) 밑이면 접힌 것이고(:7-9), 접힌 채 focus
 * 중이면 입력이 툴바 위 오버레이로 서서 URL 편집을 지킨다(:11-19,
 * issue #11090). */
const BROWSER_ADDRESS_MIN_INLINE_PX = 220; // browser-address-bar-expansion.ts:5

function browserAddressCollapsed(inlineWidth) {
  return inlineWidth !== null && inlineWidth < BROWSER_ADDRESS_MIN_INLINE_PX;
}

/* 폭과 focus가 함께 정한다 — 둘 다일 때만 오버레이. 재는 것이 입력이
 * 아니라 슬롯인 이유는 원본 주석 그대로다: 입력이 떠 있는 동안에도 슬롯은
 * flex 폭을 지키므로 측정이 오버레이와 진동하지 않는다
 * (BrowserAddressBar.tsx:45-46). */
function syncAddressOverlay(host) {
  if (!host) return;
  const slot = host.querySelector(".browser-address-slot");
  const address = host.querySelector(".browser-address");
  if (!slot || !address) return;
  const focused = document.activeElement === address;
  address.classList.toggle(
    "is-overlaying",
    focused && browserAddressCollapsed(slot.getBoundingClientRect().width),
  );
}

/* focus를 쥔 채 판이 좁아지는 손 — 창 크기 조절, 판 나누기 — 을 위한
 * 관찰. focus/blur의 손은 리스너가 직접 되묻는다. */
const browserAddressSlotWatch = new ResizeObserver((entries) => {
  for (const one of entries) syncAddressOverlay(one.target.closest(".browser-view"));
});

/* The browser surface, BUILT rather than found: the markup file is not this
 * window's to edit, and the view is a toolbar and a hole the native page
 * shows through. Same section shape as the markup's own doc views. */
function buildBrowserView() {
  const host = document.createElement("section");
  host.className = "file-view browser-view";
  host.hidden = true;
  const bar = document.createElement("div");
  bar.className = "browser-toolbar";
  // The KEY rides on the element, not just the words it produced now. This
  // view is built during load, before a stored language has been applied, so
  // the words baked in here are the Korean fallback — `applyLocale` is what
  // puts the chosen language on them, and it can only find an element that
  // says which key it wears.
  // The words arrive as a data row — `{ key, name }`, the same shape the
  // theme picker carries its names in. The KEY has to ride on the element,
  // not just the sentence it produced: this view is built during load, before
  // a stored language has been applied, so what is baked in here is the
  // Korean fallback and `applyLocale` is what later puts the chosen language
  // on it. It can only find an element that says which key it wears.
  const button = (name, glyph, words) => {
    const it = document.createElement("button");
    it.className = "browser-btn " + name;
    it.innerHTML = icon(glyph);
    // `data-tip`, not `title`: the tooltip this window draws is its own.
    it.dataset.i18nTitle = words.key;
    it.dataset.i18nAria = words.key;
    const said = t(words.key, words.name);
    it.dataset.tip = said;
    it.setAttribute("aria-label", said);
    return it;
  };
  bar.append(
    button("browser-back", "back", { key: "browser.back", name: "뒤로" }),
    button("browser-forward", "forward", { key: "browser.forward", name: "앞으로" }),
    button("browser-reload", "refresh", { key: "browser.reload", name: "새로고침" }),
  );
  const address = document.createElement("input");
  address.className = "browser-address";
  address.type = "text";
  address.spellcheck = false;
  address.dataset.i18nPlaceholder = "browser.address";
  address.placeholder = t("browser.address", "주소를 입력하거나 검색");
  // 슬롯이 입력의 flex 자리를 대신 쥔다 — 입력이 오버레이로 떠도 이웃
  // 단추가 밀리지 않고, 접힌 뒤에도 최소 폭이 남아 다시 여는 어포던스가
  // 된다(원본 min-w-11 슬롯, BrowserAddressBar.tsx:330-333).
  const addressSlot = document.createElement("div");
  addressSlot.className = "browser-address-slot";
  addressSlot.appendChild(address);
  // The zoom answer, said briefly where the eye already is. Orca floats a
  // pill over the page's top-right corner; our page is a NATIVE pane that
  // owns that rectangle, so the same words stand at the toolbar's edge
  // instead (recorded deviation) — same 1400ms stay (:11079).
  const zoomNote = document.createElement("span");
  zoomNote.className = "browser-zoom-note";
  zoomNote.hidden = true;
  zoomNote.setAttribute("role", "status");
  zoomNote.setAttribute("aria-live", "polite");
  bar.append(
    addressSlot,
    zoomNote,
    button("browser-grab", "wand", { key: "browser.grab", name: "요소 잡아 에이전트로 보내기" }),
    button("browser-annotate", "pencil", { key: "browser.annotate", name: "요소에 주석 달기" }),
    button("browser-markup", "pen-tool", { key: "browser.markup", name: "스크린샷에 그리기" }),
    button("browser-find", "eye", { key: "browser.find", name: "페이지에서 찾기" }),
    button("browser-devtools", "wrench", { key: "browser.devtools", name: "개발자 도구" }),
    button("browser-external", "external", { key: "browser.external", name: "기본 브라우저로 열기" }),
    button("browser-menu", "gear", { key: "browser.menu", name: "브라우저 메뉴" }),
  );
  // 주석 수 뱃지 — Orca가 annotate 컨트롤 모서리에 다는 그 수. 뱃지 자체가
  // 보내기 문이다: 단추는 모드 토글이므로, 모인 것을 보내는 손은 따로 산다.
  const annotate = bar.querySelector(".browser-annotate");
  const badge = document.createElement("span");
  badge.className = "browser-badge";
  badge.hidden = true;
  annotate.appendChild(badge);
  // 모인 주석의 소비 문 — Orca의 annotation 배너. 뱃지는 세고, 이 줄이
  // 보내고 비운다: 14px 뱃지 하나에 숨은 문은 아무도 못 찾는다(라이브 보고
  // 2026-08-14: "쌓이고 소비가 없음"). 네이티브 판은 .browser-body의 사각형을
  // 따라가므로, 이 줄이 서면 판이 스스로 내려앉는다.
  const notes = document.createElement("div");
  notes.className = "browser-notes";
  notes.hidden = true;
  const count = document.createElement("span");
  count.className = "browser-notes-count";
  const sendNotes = document.createElement("button");
  sendNotes.type = "button";
  sendNotes.className = "browser-notes-send";
  sendNotes.textContent = t("browser.annotate.send", "에이전트로 보내기");
  const clearNotes = document.createElement("button");
  clearNotes.type = "button";
  clearNotes.className = "browser-notes-clear";
  clearNotes.textContent = t("browser.annotate.clear", "비우기");
  notes.append(count, sendNotes, clearNotes);
  // 무장 중일 때만 서는 배너 — Orca의 grab 안내줄(실행 화면 #46). 판이
  // .browser-body 사각형을 따라가므로 이 줄도 서면 판이 내려앉는다.
  const grabbar = document.createElement("div");
  grabbar.className = "browser-grabbar";
  grabbar.hidden = true;
  const grabWords = document.createElement("span");
  grabWords.className = "browser-grabbar-words";
  const grabCancel = document.createElement("button");
  grabCancel.type = "button";
  grabCancel.className = "browser-grabbar-cancel";
  grabCancel.textContent = t("browser.grab.cancel", "취소");
  grabbar.append(grabWords, grabCancel);
  // 페이지 내 찾기(1-g32): 네이티브 판은 창 DOM 위에 뜨므로 그 위에 뜬
  // 입력은 판에 삼켜진다 — grab 배너처럼 판을 밀어내는 형제 스트립으로 서고,
  // 입력은 창 DOM이라 칠 수 있다. 명령은 판에 window.find를 주입한다.
  const findbar = document.createElement("div");
  findbar.className = "browser-findbar";
  findbar.hidden = true;
  const findInput = document.createElement("input");
  findInput.className = "browser-find-input";
  findInput.type = "text";
  findInput.spellcheck = false;
  findInput.placeholder = t("browser.find.placeholder", "페이지에서 찾기");
  const findCount = document.createElement("span");
  findCount.className = "browser-find-count";
  const findPrev = document.createElement("button");
  findPrev.type = "button";
  findPrev.className = "browser-find-prev";
  findPrev.innerHTML = icon("back");
  findPrev.dataset.tip = t("browser.find.prev", "이전");
  findPrev.setAttribute("aria-label", t("browser.find.prev", "이전"));
  const findNext = document.createElement("button");
  findNext.type = "button";
  findNext.className = "browser-find-next";
  findNext.innerHTML = icon("forward");
  findNext.dataset.tip = t("browser.find.next", "다음");
  findNext.setAttribute("aria-label", t("browser.find.next", "다음"));
  const findClose = document.createElement("button");
  findClose.type = "button";
  findClose.className = "browser-find-close";
  findClose.textContent = t("browser.find.close", "닫기");
  // Orca의 구분 막대: 개수와 화살표, 화살표와 닫기 사이에 한 줄씩 선다.
  const findSepA = document.createElement("span");
  findSepA.className = "browser-find-sep";
  const findSepB = document.createElement("span");
  findSepB.className = "browser-find-sep";
  findbar.append(findInput, findCount, findSepA, findPrev, findNext, findSepB, findClose);
  // 주소 히스토리 제안(B02 ①): Orca는 주소창 아래 팝오버로 띄우지만 우리
  // 페이지는 네이티브 판이라 DOM이 그 위에 설 수 없다 — 찾기 스트립처럼
  // 판을 밀어내는 형제로 선다(기록된 이탈, 1-g83).
  const suggest = document.createElement("div");
  suggest.className = "browser-suggest";
  suggest.hidden = true;
  // 다운로드 행(B02 ③): Orca의 알림 행처럼 툴바 밑에 서고, 같은 네이티브-판
  // 이유로 판을 밀어내는 형제다. 진행률은 없다 — tauri의 다운로드 훅은
  // 바이트 흐름을 주지 않는다(기록된 이탈).
  const dlbar = document.createElement("div");
  dlbar.className = "browser-dlbar";
  dlbar.hidden = true;
  const body = document.createElement("div");
  body.className = "browser-body";
  const failure = document.createElement("section");
  failure.className = "browser-failure";
  failure.hidden = true;
  failure.setAttribute("role", "status");
  const failureTitle = document.createElement("h2");
  failureTitle.className = "browser-failure-title";
  const failureUrl = document.createElement("p");
  failureUrl.className = "browser-failure-url";
  const failureCopy = document.createElement("p");
  failureCopy.className = "browser-failure-copy";
  const failureActions = document.createElement("div");
  failureActions.className = "browser-failure-actions";
  for (const name of ["retry", "auto"]) {
    const action = document.createElement("button");
    action.type = "button";
    action.className = `btn browser-failure-${name}`;
    failureActions.appendChild(action);
  }
  failure.append(failureTitle, failureUrl, failureCopy, failureActions);
  body.appendChild(failure);
  // 아티팩트 페이지의 머리띠(t-3233 §5): 갤러리에서 연 페이지에만 서고, 판이
  // .browser-body 사각형을 따라가므로 이 줄도 서면 판이 내려앉는다.
  host.append(bar, artifactStripNode(), suggest, dlbar, notes, grabbar, findbar, body);
  return host;
}

/* Give a host its hands, once per NODE — the clone a second leaf gets arrives
 * with dead buttons, and this is what brings them to life. The handlers read
 * `host._browserTab`, which every paint refreshes: a listener holding the tab
 * it was born beside would steer a tab that moved leaves long ago. */
function wireBrowserHost(host) {
  if (browserWired.has(host)) return;
  browserWired.add(host);
  // The first pane to be wired starts the right-click watch; it costs
  // nothing while no pane is in front (1-g13).
  watchBrowserMenu();
  browserBodyWatch.observe(host.querySelector(".browser-body"));
  const held = () => host._browserTab;
  host.querySelector(".browser-failure-retry").addEventListener("click", () => {
    const tab = held();
    if (tab?.navFailure) void retryBrowserFailure(tab, { manual: true });
  });
  host.querySelector(".browser-failure-auto").addEventListener("click", () => {
    const tab = held();
    if (!tab?.navFailure) return;
    tab.navFailure.paused = !tab.navFailure.paused;
    cancelBrowserRetry(tab);
    paintBrowserView(tab);
    syncBrowserPanes();
  });
  host.querySelector(".browser-back").addEventListener("click", () => {
    const tab = held();
    if (tab) invoke("browser_history", { label: tab.label, delta: -1 }).catch(() => {});
  });
  host.querySelector(".browser-forward").addEventListener("click", () => {
    const tab = held();
    if (tab) invoke("browser_history", { label: tab.label, delta: 1 }).catch(() => {});
  });
  // Orca's same button: loading → stop, else reload.
  host.querySelector(".browser-reload").addEventListener("click", () => {
    const tab = held();
    if (!tab) return;
    if (tab.navFailure) {
      if (tab.loading) {
        tab.navFailure.paused = true;
        tab.loading = false;
        cancelBrowserRetry(tab);
        void invoke("browser_stop", { label: tab.label }).catch(() => {});
        paintBrowserView(tab);
      } else void retryBrowserFailure(tab, { manual: true });
      return;
    }
    tab.navigationStopped = tab.loading === true;
    invoke(tab.loading ? "browser_stop" : "browser_reload", { label: tab.label }).catch(() => {});
  });
  host.querySelector(".browser-external").addEventListener("click", () => {
    const out = externalBrowserUrl(held()?.url);
    if (out) openExternal(out);
  });
  host.querySelector(".browser-devtools").addEventListener("click", () => {
    const tab = held();
    if (!tab) return;
    invoke("browser_devtools", { label: tab.label }).catch(() => {});
    // macOS의 인스펙터는 이 창 안에 도킹하며 창을 다시 나눈다 — 문서가
    // 줄어드는 동안 판이 옛 자리에 떠 있으면 인스펙터를 덮는 흰 면이 된다
    // (라이브 보고 2026-08-14 #63). 반영이 끝날 때까지 몇 번 다시 잰다;
    // 닫힘도 같은 버튼이므로 같은 버스트가 되돌린다.
    for (const wait of [200, 600, 1200]) {
      setTimeout(syncBrowserPanes, wait);
    }
  });
  // Design mode: arm the crosshair in the guest, poll for the pick, hand what
  // it grabbed to an agent terminal (1-g5). The button is a toggle — a second
  // click stands the crosshair down.
  host.querySelector(".browser-grab").addEventListener("click", () => {
    if (held()) toggleBrowserGrab(host, "copy");
  });
  host.querySelector(".browser-grabbar-cancel").addEventListener("click", () => {
    stopBrowserGrab(true);
  });
  host.querySelector(".browser-find").addEventListener("click", () => {
    const tab = held();
    if (tab) toggleBrowserFind(host, tab);
  });
  const findInput = host.querySelector(".browser-find-input");
  // 치는 동안에도 찾는다(1-g34) — Enter를 기다리지 않는다.
  findInput.addEventListener("input", () => {
    const tab = held();
    if (tab) scheduleBrowserFind(tab, findInput.value);
  });
  findInput.addEventListener("keydown", (event) => {
    // Orca의 handleKeyDown 첫 줄과 같다: 위젯 경계에서 키를 세운다. 여기의
    // Esc가 창까지 오르면 '읽기 표면은 Esc로 닫는다'가 탭째로 닫아버린다.
    event.stopPropagation();
    const tab = held();
    if (!tab) return;
    if (event.key === "Enter") {
      event.preventDefault();
      pressBrowserFind(tab, findInput.value, !event.shiftKey);
    } else if (event.key === "Escape") {
      event.preventDefault();
      closeBrowserFind(host, tab);
    }
  });
  host.querySelector(".browser-find-next").addEventListener("click", () => {
    const tab = held();
    if (tab) pressBrowserFind(tab, findInput.value, true);
  });
  host.querySelector(".browser-find-prev").addEventListener("click", () => {
    const tab = held();
    if (tab) pressBrowserFind(tab, findInput.value, false);
  });
  host.querySelector(".browser-find-close").addEventListener("click", () => {
    const tab = held();
    if (tab) closeBrowserFind(host, tab);
  });
  // Orca의 ⚙ 메뉴: 프로필(1-g26 — 판은 자기 쿠키 단지에서 다시 선다),
  // 뷰포트 크기(1-g19), 쿠키 가져오기(지시서 C3 — 심기 문이 섰다:
  // `import_browser_cookies`), 설정 문.
  host.querySelector(".browser-menu").addEventListener("click", async (event) => {
    const tab = held();
    if (!tab) return;
    const box = event.currentTarget.getBoundingClientRect();
    const check = (chosen, label) => (chosen ? "✓ " + label : label);
    // 목록은 문을 열 때마다 새로 읽는다 — 다른 창이 만든 프로필도 이 메뉴의
    // 것이다.
    await refreshBrowserProfiles();
    const items = [
      { label: t("browser.profile", "프로필"), disabled: true, run: () => {} },
      {
        label: check(!tab.profile, t("browser.profileDefault", "기본")),
        run: () => {
          if (tab.profile) void reopenBrowserPane(tab, { profile: null });
        },
      },
      ...browserProfiles.map((profile) => ({
        label: check(tab.profile === profile.id, profile.name),
        run: () => {
          if (tab.profile !== profile.id) void reopenBrowserPane(tab, { profile: profile.id });
        },
      })),
      {
        label: t("browser.newProfile", "새 프로필…"),
        run: () => askNewBrowserProfile(box, tab),
      },
      {
        label: t("browser.importCookies", "브라우저에서 가져오기…"),
        run: () => void askCookieImport(box, tab.profile ?? null),
      },
      { separator: true },
      { label: t("browser.viewport", "뷰포트 크기"), disabled: true, run: () => {} },
      {
        label: check(!tab.viewport, t("browser.viewportDefault", "기본")),
        run: () => {
          tab.viewport = null;
          syncBrowserPanes();
          rememberBrowserOpenTabs();
        },
      },
      ...BROWSER_VIEWPORT_PRESETS.map((preset) => ({
        label: check(tab.viewport === preset.id, preset.label),
        run: () => {
          tab.viewport = preset.id;
          syncBrowserPanes();
          rememberBrowserOpenTabs();
        },
      })),
      { separator: true },
      {
        label: t("browser.openSettings", "브라우저 설정…"),
        run: () => {
          setSettingsOpen(true);
          showSettingsPane("browser");
        },
      },
    ];
    openSidebarMenu(box.left, box.bottom + 4, items);
  });
  // Annotate: the same crosshair with Orca's card at the end of it. The
  // BADGE is the send door — the button stays a mode toggle.
  host.querySelector(".browser-annotate").addEventListener("click", (event) => {
    const tab = held();
    if (!tab) return;
    if (event.target.closest(".browser-badge")) {
      void deliverAnnotations(host, tab);
      return;
    }
    toggleBrowserGrab(host, "annotate");
  });
  // 마크업(B02 ②): 캡처가 먼저 오고, 편집기가 판의 자리에 선다.
  host.querySelector(".browser-markup").addEventListener("click", () => {
    const tab = held();
    if (tab) void startBrowserMarkup(tab);
  });
  host.querySelector(".browser-notes-send").addEventListener("click", () => {
    const tab = held();
    if (tab) void deliverAnnotations(host, tab);
  });
  host.querySelector(".browser-notes-clear").addEventListener("click", () => {
    const tab = held();
    if (!tab) return;
    browserAnnotations.delete(tab.label);
    refreshGrabButtons();
  });
  const address = host.querySelector(".browser-address");
  const suggest = host.querySelector(".browser-suggest");
  // What was typed before ↑/↓ began previewing — Esc puts it back (Orca's
  // address bar holds the typed text apart from the previewed one).
  let addressDraft = null;
  let suggestAt = -1;
  const closeSuggest = () => {
    if (suggest.hidden) return;
    suggest.hidden = true;
    suggest.replaceChildren();
    addressDraft = null;
    suggestAt = -1;
    syncBrowserPanes();
  };
  const suggestRows = () => [...suggest.querySelectorAll(".browser-suggest-row")];
  const markSuggest = () => {
    // Row 0 glows even before any arrow — the typed words in the input are
    // that row's own meaning (BrowserAddressBar.tsx:153-157, 241-242).
    const lit = Math.max(suggestAt, 0);
    suggestRows().forEach((row, at) => row.classList.toggle("is-active", at === lit));
  };
  // Selecting a row previews its url into the input; the row without a url
  // is the search row, and picking it is what Enter already does with the
  // typed words — so the input goes back to them and the preview state ends
  // (selectSuggestionAtIndex, BrowserAddressBar.tsx:111-128).
  const selectSuggest = (at) => {
    const row = suggestRows()[at];
    if (!row) return;
    suggestAt = at;
    if (row.dataset.url === undefined) {
      if (addressDraft !== null) address.value = addressDraft;
      addressDraft = null;
    } else {
      if (addressDraft === null) addressDraft = address.value;
      address.value = row.dataset.url;
    }
    markSuggest();
  };
  const paintSuggest = () => {
    const tab = held();
    if (!tab || document.activeElement !== address) {
      closeSuggest();
      return;
    }
    // The whole law — empty bar → recents, top action, seven ranked rows,
    // duplicate top dropped — lives in browserSuggestions; this hand only
    // renders what it says.
    const rows = browserSuggestions(address.value);
    if (rows.length === 0) {
      closeSuggest();
      return;
    }
    const wasHidden = suggest.hidden;
    suggest.replaceChildren();
    // New letters are the new typed truth — any preview draft is void now.
    addressDraft = null;
    suggestAt = -1;
    for (const one of rows) {
      const row = document.createElement("button");
      row.type = "button";
      row.className = "browser-suggest-row";
      row.innerHTML = icon(one.kind === "search" ? "search" : "globe");
      // Only the search row carries no url — its preview is the typed words.
      if (one.kind !== "search") row.dataset.url = one.url;
      const words = document.createElement("span");
      words.className = "browser-suggest-words";
      words.textContent = one.kind === "visit" ? (one.title || one.url) : one.title;
      row.appendChild(words);
      // The subtitle speaks Orca's: the engine's words on the search row,
      // the url under a titled visit, nothing under a normalized address
      // (subtitle '', browser-address-bar-suggestions.ts:97-116).
      const sub = one.kind === "search" ? one.subtitle
        : one.kind === "visit" && one.title ? one.url : "";
      if (sub) {
        const where = document.createElement("span");
        where.className = "browser-suggest-url";
        where.textContent = sub;
        row.appendChild(where);
      }
      // mousedown, so the pick lands before the input's blur closes the
      // strip under the pointer.
      row.addEventListener("mousedown", (event) => {
        event.preventDefault();
        const held2 = held();
        if (!held2) return;
        submitBrowserAddress(held2, one.url);
        closeSuggest();
        address.blur();
      });
      suggest.appendChild(row);
    }
    suggest.hidden = false;
    markSuggest();
    if (wasHidden) syncBrowserPanes();
  };
  // 접힌 슬롯의 여백을 눌러도 입력이 열린다 — 짜부라진 입력은 0폭이라
  // 클릭이 슬롯에 앉기 때문이다(원본 form onClick → input.focus(),
  // BrowserAddressBar.tsx:364-365). 관찰자는 focus를 쥔 채 좁아지는 판을
  // 따라간다.
  const addressSlot = host.querySelector(".browser-address-slot");
  browserAddressSlotWatch.observe(addressSlot);
  addressSlot.addEventListener("click", () => address.focus());
  // The whole address on focus — Orca's handleFocus calls select(), because
  // the common next act is replacing it, not appending to it — and focus
  // alone opens the strip (setOpen(true), BrowserAddressBar.tsx:159-169):
  // an empty bar's recent visits stand this way.
  address.addEventListener("focus", () => {
    syncAddressOverlay(host);
    address.select();
    paintSuggest();
  });
  address.addEventListener("input", paintSuggest);
  address.addEventListener("blur", () => {
    // 오버레이는 focus의 것 — 손이 떠나면 입력은 슬롯의 인라인 자리로
    // 돌아간다(shouldOverlayBrowserAddressBar의 focused 항).
    syncAddressOverlay(host);
    closeSuggest();
  });
  address.addEventListener("keydown", (event) => {
    const tab = held();
    if (!tab) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      if (suggest.hidden) return;
      event.preventDefault();
      event.stopPropagation();
      const rows = suggestRows();
      if (rows.length === 0) return;
      // Row 0 is spoken for by the input itself, so the walk starts there:
      // the first ↓ advances to the next row, and both ends wrap
      // (BrowserAddressBar.tsx:237-246, 249-262).
      const from = Math.max(suggestAt, 0);
      if (event.key === "ArrowDown") {
        selectSuggest(from < rows.length - 1 ? from + 1 : 0);
      } else if (addressDraft === null) {
        // ↑ before any preview wraps to the far end (:252-256).
        selectSuggest(from > 0 ? from - 1 : rows.length - 1);
      } else if (from <= 0) {
        // ↑ at row 0 while previewing restores the typed words (:257-260).
        address.value = addressDraft;
        addressDraft = null;
        suggestAt = -1;
        markSuggest();
      } else {
        selectSuggest(from - 1);
      }
    } else if (event.key === "Enter") {
      event.preventDefault();
      submitBrowserAddress(tab, address.value);
      closeSuggest();
      address.blur();
    } else if (event.key === "Escape") {
      event.preventDefault();
      if (!suggest.hidden) {
        // First Esc restores what was typed and puts the list away; the
        // second leaves the bar (Orca's ladder). Held here — bubbling on
        // would reach the window's own Escape ladder, which pulls focus,
        // and a list closed by the FIRST press must leave the bar typing.
        event.stopPropagation();
        if (addressDraft !== null) address.value = addressDraft;
        closeSuggest();
        return;
      }
      address.value = tab.url === BROWSER_BLANK_URL ? "" : (tab.url ?? "");
      address.blur();
    }
  });
}

function submitBrowserAddress(tab, raw) {
  const said = navigableUrlOf(raw);
  if (said === null) return;
  cancelBrowserRetry(tab);
  tab.navFailure = null;
  tab.navigationStopped = false;
  tab.url = said;
  tab.loading = true;
  // A new voyage drops the old page's name from the strip — keeping it would
  // caption the next page with the last one's title.
  tab.pageTitle = null;
  invoke("browser_navigate", { label: tab.label, url: said }).catch((error) => {
    // The refusal undoes the optimism: a spinner for a voyage that never
    // left would spin until the next real one.
    tab.loading = false;
    markBrowserFailure(tab);
    if (stillShowing(tab)) paintBrowserView(tab);
    showError(String(error));
  });
  renderTabs();
  if (stillShowing(tab)) paintBrowserView(tab);
}

/* ---- 기기 프레임 맞춤(1-g45) ----
 *
 * CSS에 맡길 수 없는 자리다: 베젤은 가운데 정렬된 flex 상자라 제 높이가
 * 내용으로 정해지고(indefinite), 그 안의 화면이 든 max-height:100%는
 * 명세대로 무시된다 — 화면이 제 원본 픽셀(1080×2400)로 서던 실기기 버그
 * 두 건(Android·iOS)의 전부. 그래서 Orca처럼 판을 재서 픽셀로 말한다
 * (fitDeviceFrameToPane 실측 이식). */

function clampBetween(value, low, high) {
  return Math.min(high, Math.max(low, value));
}

/* 이름이 말하면 이름을 믿고(iPad·iPhone·tablet), 모르는 이름은 화면의
 * 비율로 가른다 — 정사각에 가까우면(0.62~1.62) 태블릿이다. */
function resolveEmulatorFrameKind(deviceName, aspectRatio) {
  if (/ipad|tablet/i.test(deviceName ?? "")) return "tablet";
  if (/iphone/i.test(deviceName ?? "")) return "phone";
  return aspectRatio > 0.62 && aspectRatio < 1.62 ? "tablet" : "phone";
}

/* 첫 그림이 오기 전의 화면꼴. 기기가 제 크기를 말하기 전에도 껍데기는 서야
 * 하므로 — 서지 않으면 사람은 베젤 없는 빈 사각형을 본다 — 꼴마다 하나씩
 * 둔다. 원본은 꼴을 가리지 않고 9:19 하나로 세우지만(`emulator-device-frame-
 * layout.ts:64-65`의 `?? 9 / ?? 19`), 그 값은 아이패드 판을 폰 모양으로 세운다.
 * 첫 그림이 오는 순간 진짜 비율이 이 값을 대체하고, 그때 껍데기는 자리를
 * 지킨 채 비율만 바뀐다. */
const EMULATOR_FALLBACK_ASPECT = Object.freeze({ phone: 9 / 19, tablet: 3 / 4 });

/* 폰 옆구리의 하드웨어 버튼이 앉는 자리 — `top`은 껍데기 높이에 대한 비율,
 * 길이는 그 높이에 비례하되(`ratio`) 제 띠 안에 죈다(`min`/`max`).
 *
 * 값은 실측된 것이고 원본과 일치하므로 한 자도 바꾸지 않는다. 여기로 옮긴
 * 이유는 둘이다: 여덟 개의 무명 숫자가 이름을 얻고, 이 표가 레이아웃마다
 * 새로 태어나지 않는다 — 분할을 끄는 동안 초당 예순 개가 만들어지던 자리다. */
const EMULATOR_SIDE_KEY_SEATS = Object.freeze({
  action: Object.freeze({ top: 0.16, ratio: 0.04, min: 18, max: 34 }),
  "volume-up": Object.freeze({ top: 0.24, ratio: 0.08, min: 34, max: 64 }),
  "volume-down": Object.freeze({ top: 0.33, ratio: 0.08, min: 34, max: 64 }),
  power: Object.freeze({ top: 0.24, ratio: 0.095, min: 42, max: 76 }),
});

/* 기기가 설 수 있는 자리 — 판의 사각형에서 그 판이 입은 여백을 뺀 것.
 *
 * 여백의 값을 여기 적지 않는 이유는 그것이 이미 `--device-stage-inset-*`에
 * 있고, 같은 수를 두 곳에 적으면 한쪽만 고쳐지는 날이 오기 때문이다. 판은
 * 토큰을 padding으로 입고, 이 함수는 입은 것을 되읽는다.
 *
 * `getBoundingClientRect()`가 이미 스타일을 최신으로 만든 뒤이므로 여기의
 * 계산된 값 읽기는 두 번째 리플로를 부르지 않는다. 그리고 이 함수는 프레임마다
 * 아니라 판이 크기를 바꾸거나 화면꼴이 바뀔 때만 돈다. */
function emulatorStageRoom(body) {
  const rect = body.getBoundingClientRect();
  const worn = getComputedStyle(body);
  const inset = (side) => Number.parseFloat(worn.getPropertyValue(`padding-${side}`)) || 0;
  return {
    width: Math.floor(rect.width) - inset("left") - inset("right"),
    height: Math.floor(rect.height) - inset("top") - inset("bottom"),
  };
}

/* contain 수학: 판이 화면꼴보다 넓으면 높이에, 아니면 너비에 맞춘다. */
function fitEmulatorScreenToPane(pane, aspectRatio) {
  if (!pane || pane.width <= 0 || pane.height <= 0 || aspectRatio <= 0) return null;
  if (pane.width / pane.height > aspectRatio) {
    return { width: Math.max(1, pane.height * aspectRatio), height: pane.height };
  }
  return { width: pane.width, height: Math.max(1, pane.width / aspectRatio) };
}

/* 화면 크기에 비례하는 껍데기 치수(실측): 베젤 두께, 폰의 하드웨어 버튼이
 * 설 바깥 여유, 그 버튼의 두께. */
function measureEmulatorChrome(screen, kind) {
  const short = Math.min(screen.width, screen.height);
  return {
    bezel:
      kind === "phone" ? clampBetween(short * 0.021, 7, 15) : clampBetween(short * 0.026, 8, 22),
    hardwareOutset: kind === "phone" ? clampBetween(short * 0.012, 3, 7) : 0,
    sideButtonThickness: kind === "phone" ? clampBetween(short * 0.01, 3, 6) : 0,
  };
}

/* 판 안에 화면과 껍데기가 함께 서도록 맞춘다. 껍데기는 화면 크기에
 * 비례하고 화면은 껍데기를 뺀 자리에 맞으므로 서로를 부른다 — Orca처럼
 * 네 번 되먹여 수렴시킨다. */
function fitEmulatorFrameToPane(pane, aspectRatio, kind) {
  if (!pane || pane.width <= 0 || pane.height <= 0 || aspectRatio <= 0) return null;
  let screen = fitEmulatorScreenToPane(pane, aspectRatio);
  for (let pass = 0; pass < 4; pass += 1) {
    if (!screen) return null;
    const chrome = measureEmulatorChrome(screen, kind);
    screen = fitEmulatorScreenToPane(
      {
        width: Math.max(1, pane.width - chrome.hardwareOutset * 2 - chrome.bezel * 2 - 0.5),
        height: Math.max(1, pane.height - chrome.bezel * 2 - 0.5),
      },
      aspectRatio,
    );
  }
  if (!screen) return null;
  const { bezel, hardwareOutset, sideButtonThickness } = measureEmulatorChrome(screen, kind);
  const short = Math.min(screen.width, screen.height);
  const outerRadius =
    kind === "phone" ? clampBetween(short * 0.135, 44, 92) : clampBetween(short * 0.065, 24, 56);
  const innerRadius =
    kind === "phone"
      ? clampBetween(outerRadius - bezel, 34, 82)
      : clampBetween(outerRadius - bezel * 0.7, 18, 48);
  const shellWidth = screen.width + bezel * 2;
  const shellHeight = screen.height + bezel * 2;
  return {
    kind,
    width: shellWidth + hardwareOutset * 2,
    height: shellHeight,
    shellWidth,
    shellHeight,
    bezel,
    hardwareOutset,
    outerRadius,
    innerRadius,
    sideButtonThickness,
  };
}

/* 판을 재서 껍데기와 화면을 픽셀로 앉힌다 — 스트림 크기를 아직 모르면
 * 인라인을 걷어 CSS 폴백이 서게 둔다. 버튼 자리(실측): 왼쪽에 액션·볼륨
 * 둘, 오른쪽에 전원 — 높이는 껍데기 높이에 비례하고 저마다의 띠 안에
 * 죈다. */
/* 백엔드와 같은 눈금. ios.rs의 LONG_EDGE_QUANTUM_PX와 짝이어야 한다 —
 * 눈금이 어긋나면 보낸 값과 백엔드가 고른 값이 달라져, 매번 다르다고 판단해
 * 헬퍼의 인코더를 계속 다시 만든다. */
const EMULATOR_VIEWPORT_QUANTUM_PX = 64;

/* 드래그가 멈추기를 기다리는 시간. 드래그 한 번은 이 자리를 초당 수십 번
 * 지나가고, 그때마다 보내면 헬퍼가 크기마다 VTCompressionSession을 버리고 새로
 * 만든다. */
const EMULATOR_VIEWPORT_SETTLE_MS = 150;

/* 판이 실제로 칠하는 긴 변 — CSS 상자 × 디스플레이 배율.
 *
 * 배율을 곱하는 것이 핵심이다. 레티나에서 300pt 판은 600픽셀을 칠하고, 300을
 * 보내면 백엔드가 정확히 절반 크기의 그림을 보내 판이 그것을 두 배로 늘려
 * 그린다 — 그게 바로 흐림이다. */
function emulatorViewportLongEdge(layout) {
  const scale = window.devicePixelRatio || 1;
  // 재는 것은 껍데기가 아니라 화면이다 — 베셸은 그림이 앚지 않는 테두리다.
  const width = Math.max(1, layout.shellWidth - layout.bezel * 2);
  const height = Math.max(1, layout.shellHeight - layout.bezel * 2);
  const longEdge = Math.max(width, height) * scale;
  return Math.ceil(longEdge / EMULATOR_VIEWPORT_QUANTUM_PX) * EMULATOR_VIEWPORT_QUANTUM_PX;
}

/* 백엔드에 누가 보고 있는지 알린다 — 값이 바뀔 때만. 스트림이 새로 살아날 때는
 * 지난 스트림의 기억을 지우고 지금 상태를 다시 말한다: 백엔드의 새 제어는
 * '주의 있음'으로 태어나므로, 포인터가 밖에 있는 판은 그 말을 들어야 낮아진다. */
function reportEmulatorAttention(host, engaged) {
  const tab = host?._emulatorTab;
  if (!tab?.stream || tab.engaged === engaged) return;
  tab.engaged = engaged;
  void invoke("set_emulator_stream_engaged", { stream: tab.stream, engaged }).catch(() => {});
}

function emulatorAttentionOf(host) {
  return host.matches(":hover") || host.contains(document.activeElement);
}

/* 백엔드에 판의 크기를 알린다 — 눈금을 넘을 때만, 그리고 손이 멈춘 뒤에. */
function reportEmulatorViewport(tab, layout) {
  if (!tab || tab.kind !== "emulator" || !layout) return;
  const longEdgePx = emulatorViewportLongEdge(layout);
  if (tab.viewportLongEdge === longEdgePx) return;
  // 스트림이 아직 없는 판도 치수는 기억해둔다 — start_emulator_stream이 그걸
  // 실어 보내야 첫 그림부터 제 해상도로 온다.
  tab.viewportLongEdge = longEdgePx;
  if (tab.viewportAt) clearTimeout(tab.viewportAt);
  if (!tab.stream) return;
  tab.viewportAt = setTimeout(() => {
    tab.viewportAt = null;
    if (!tab.stream) return;
    void invoke("set_emulator_stream_viewport", {
      stream: tab.stream,
      viewport: { longEdgePx },
    }).catch(() => {});
  }, EMULATOR_VIEWPORT_SETTLE_MS);
}

function layoutEmulatorScreen(host, tab) {
  const body = host.querySelector(".emulator-body");
  const shell = host.querySelector(".emulator-shell");
  const bezel = host.querySelector(".emulator-bezel");
  const frame = host.querySelector(".emulator-frame");
  const screen = host.querySelector(".emulator-canvas");
  // 기다리는 판은 화면이 설 자리에 같은 치수로 서기 때문에, 치수를 매기는 이곳이
  // 그 판을 세우고 눕히는 곳이기도 하다. 두 화면 가운데 하나라도 보이면 픽셀이
  // 이미 도착한 것이므로 판은 물러난다.
  const waiting = host.querySelector(".emulator-skeleton");
  waiting.hidden = frame.hidden === false || screen.hidden === false;
  const media = [frame, screen, waiting];
  const keys = [...host.querySelectorAll(".emulator-side-key")];
  const size = tab?.screenSize;
  const kind = resolveEmulatorFrameKind(tab?.deviceName, size ? size.width / size.height : 0);
  // 아직 크기를 모르는 판도 껍데기는 세운다 — 기다리는 동안 보이는 것이
  // 빈 사각형이 아니라 그 기기여야 한다.
  const aspect = size ? size.width / size.height : EMULATOR_FALLBACK_ASPECT[kind];
  bezel.classList.toggle("is-tablet", kind === "tablet");
  const layout = fitEmulatorFrameToPane(emulatorStageRoom(body), aspect, kind);
  if (!layout) {
    for (const one of [shell, bezel]) one.removeAttribute("style");
    // 화면에는 내가 얹은 것만 걷는다 — 표식 없이 걷으면 다른 손(하네스의
    // 가짜 프레임 크기 같은)의 인라인까지 걷어 간다.
    for (const one of media) {
      if (one.dataset.fitted === undefined) continue;
      delete one.dataset.fitted;
      one.style.removeProperty("border-radius");
      one.style.removeProperty("width");
      one.style.removeProperty("height");
    }
    for (const key of keys) key.hidden = true;
    return;
  }
  // 재자리가 이곳이므로 판의 크기도 여기서 백엔드에 간다 — 분할 드래그, 창 크기
  // 변경, 사이드바 접기가 전부 이 함수를 지난다.
  reportEmulatorViewport(tab, layout);
  shell.style.width = `${layout.width}px`;
  shell.style.height = `${layout.height}px`;
  // 버튼이 설 왼쪽 여유만큼 베젤을 민다 — 오른쪽 여유는 껍데기 너비가
  // 이미 품고 있다.
  bezel.style.marginLeft = `${layout.hardwareOutset}px`;
  bezel.style.width = `${layout.shellWidth}px`;
  bezel.style.height = `${layout.shellHeight}px`;
  bezel.style.padding = `${layout.bezel}px`;
  bezel.style.borderRadius = `${layout.outerRadius}px`;
  // 잰 내경을 화면이 가득 쓴다 — CSS의 max-*:100%는 축소만 알아서, 내경보다
  // 작은 원본(iOS 120×256 같은)은 그냥 제 크기로 남는다. 베젤이 인라인 px로
  // 선(definite) 동안만 %가 유효하므로 이 두 줄도 layout과 같은 손이 얹고
  // 걷는다.
  for (const one of media) {
    // 위 걷기의 기준 — 이 표식이 있어야 내가 앉힌 인라인이다.
    one.dataset.fitted = "1";
    one.style.borderRadius = `${layout.innerRadius}px`;
    one.style.width = "100%";
    one.style.height = "100%";
  }
  const inset = layout.hardwareOutset - layout.sideButtonThickness;
  for (const key of keys) {
    const seat = EMULATOR_SIDE_KEY_SEATS[key.dataset.key];
    key.hidden =
      layout.kind !== "phone" || !seat || (key.dataset.key === "action" && tab?.platform !== "ios");
    if (!seat) continue;
    key.style.top = `${layout.shellHeight * seat.top}px`;
    key.style.height = `${clampBetween(layout.shellHeight * seat.ratio, seat.min, seat.max)}px`;
    key.style.width = `${layout.sideButtonThickness}px`;
    if (key.dataset.side === "right") key.style.right = `${inset}px`;
    else key.style.left = `${inset}px`;
  }
}

/* 시뮬레이터 거울(1-g28): bytes가 objectURL이 되어 img에 앉고, 앞 프레임의
 * URL은 그 자리에서 해제된다. 입력 capability가 선 기기는 같은 표면의
 * 정규화 좌표·IME 문을 각 플랫폼 어댑터로 보낸다. */
/* Orca의 툴바 실측 그대로(EmulatorPaneToolbar): 기기 글리프+이름, 상태
 * (Connected/Working…/Not connected), 기기 Select, 살아 있으면 Shutdown —
 * 아니면 Attach. Home·Rotate는 backend가 실제 입력 capability를 답한 경우에만
 * 선다 — iOS는 자체 SimulatorKit HID helper, Android는 adb가 그 문이다. */
function buildEmulatorView() {
  const root = document.createElement("div");
  root.className = "emulator-view";
  root.hidden = true;
  const bar = document.createElement("div");
  bar.className = "emulator-toolbar";
  const glyph = document.createElement("span");
  glyph.className = "emulator-glyph";
  glyph.innerHTML = icon("phone");
  const name = document.createElement("span");
  name.className = "emulator-name";
  const status = document.createElement("span");
  status.className = "emulator-status";
  const gap = document.createElement("span");
  gap.className = "emulator-gap";
  const pick = document.createElement("select");
  pick.className = "emulator-pick";
  pick.setAttribute("aria-label", t("emulator.choose", "기기 선택"));
  const power = document.createElement("button");
  power.className = "icon-btn emulator-power";
  power.type = "button";
  const outward = document.createElement("button");
  outward.className = "icon-btn emulator-outward";
  outward.type = "button";
  outward.innerHTML = icon("external");
  labelButton(outward, t("emulator.openApp", "Simulator에서 열기"));
  // Android의 하드웨어 3버튼(Orca BUTTON_KEYCODES) — iOS 미러엔 없다.
  const keys = document.createElement("span");
  keys.className = "emulator-keys";
  keys.hidden = true;
  const install = document.createElement("button");
  install.className = "icon-btn emulator-install";
  install.type = "button";
  install.innerHTML = icon("upload");
  labelButton(install, t("emulator.install", "앱 설치…"));
  const rotate = document.createElement("button");
  rotate.className = "icon-btn emulator-rotate";
  rotate.type = "button";
  rotate.innerHTML = icon("forward");
  labelButton(rotate, t("emulator.rotate", "회전"));
  keys.appendChild(rotate);
  for (const [name, glyphName, label] of [
    ["back", "arrow-left", t("emulator.back", "뒤로")],
    ["home", "circle-dashed", t("emulator.home", "홈")],
    ["recents", "columns", t("emulator.recents", "최근")],
  ]) {
    const key = document.createElement("button");
    key.className = "icon-btn emulator-key";
    key.type = "button";
    key.dataset.button = name;
    key.innerHTML = icon(glyphName);
    labelButton(key, label);
    keys.appendChild(key);
  }
  bar.append(glyph, name, status, gap, keys, install, pick, power, outward);
  const body = document.createElement("div");
  body.className = "emulator-body";
  // 주의(attention): 포인터가 화면 위에 있거나 키보드 포커스가 안에 있는 동안,
  // 그리고 기기 입력(사람의 터치·에이전트의 탭) 직후 몇 초는 헬퍼가 60fps
  // 상한 그대로 밀고, 그 밖에 거울처럼 놓여 있을 때만 12fps다. 2026-09-02
  // 실측 — 애니메이션 배경화면 하나가 1122×2436을 초당 42~50장 밀어 GPU 31%·
  // WindowServer 40%를 먹고 옆 터미널 타자를 버벅이게 했다. 보는 것과 쓰는
  // 것은 다른 값이고, 쓰는 동안의 매끄러움은 시뮬레이터 그대로다.
  for (const [name, engaged] of [
    ["pointerenter", true],
    ["pointerleave", false],
    ["focusin", true],
    ["focusout", false],
  ]) {
    body.addEventListener(name, () => reportEmulatorAttention(root, engaged));
  }
  // 껍데기 한 겹(1-g45): 레이아웃이 잰 픽셀은 여기 앉고, 폰 옆구리의
  // 하드웨어 버튼 장식이 베젤 밖 여유에 선다.
  const shell = document.createElement("div");
  shell.className = "emulator-shell";
  const bezel = document.createElement("div");
  bezel.className = "emulator-bezel";
  const frame = document.createElement("img");
  frame.className = "emulator-frame";
  frame.alt = "";
  frame.hidden = true;
  frame.draggable = false;
  bezel.appendChild(frame);
  // H264 터널이 서면 그림은 여기 앉는다(1-g35) — <img>는 screencap의 자리고,
  // 둘 중 하나만 보인다.
  const screen = document.createElement("canvas");
  screen.className = "emulator-canvas";
  screen.hidden = true;
  bezel.appendChild(screen);
  /* 화면이 아직 오지 않은 동안 그 자리를 지키는 판(1-g45). 안내 글귀는 몸통 아래에
   * 서므로 화면이 설 자리는 비어 있게 되는데, 거기에 화면과 똑같은 치수의 판을
   * 세워 두면 첫 프레임이 도착할 때 자리가 흔들리지 않는다. */
  const waiting = document.createElement("div");
  waiting.className = "emulator-skeleton";
  bezel.appendChild(waiting);
  shell.appendChild(bezel);
  // 폰 껍데기의 실제 하드웨어 입력. 모양과 누름은 같은 요소가 소유해
  // 레이아웃을 복제하지 않고 iOS HID/Android keyevent로만 갈라진다.
  for (const [name, side, label] of [
    ["action", "left", t("emulator.action", "동작 버튼")],
    ["volume-up", "left", t("emulator.volumeUp", "음량 높이기")],
    ["volume-down", "left", t("emulator.volumeDown", "음량 낮추기")],
    ["power", "right", t("emulator.power", "전원")],
  ]) {
    const key = document.createElement("button");
    key.className = "emulator-side-key";
    key.type = "button";
    key.dataset.key = name;
    key.dataset.side = side;
    labelButton(key, label);
    key.hidden = true;
    shell.appendChild(key);
  }
  const note = document.createElement("div");
  note.className = "emulator-note";
  note.textContent = t("emulator.waking", "기기를 깨우는 중…");
  body.append(shell, note);
  root.append(bar, body);
  return root;
}

const EMULATOR_INPUT_ERROR_THROTTLE_MS = 5_000;

/* 글자가 아닌 키가 기기에서 불리는 이름. 표를 모듈에 두는 이유는 이것이
 * 상수이기 때문이다 — 핸들러 안에 두면 사람이 누르는 키마다 표가 하나씩
 * 태어난다. */
const EMULATOR_NAMED_KEYS = Object.freeze({
  Enter: "enter",
  Backspace: "del",
  Delete: "forward_del",
  Escape: "escape",
  Tab: "tab",
  ArrowUp: "up",
  ArrowDown: "down",
  ArrowLeft: "left",
  ArrowRight: "right",
});

function emulatorErrorMessage(key) {
  if (key === "emulator.inputFailed") return t("emulator.inputFailed", "에뮬레이터에 입력을 보내지 못했습니다.");
  if (key === "emulator.shutdownFailed") return t("emulator.shutdownFailed", "에뮬레이터를 종료하지 못했습니다.");
  if (key === "emulator.installFailed") return t("emulator.installFailed", "앱을 설치하지 못했습니다.");
  return t("emulator.connectionFailed", "에뮬레이터에 연결하지 못했습니다.");
}

/* 부팅 자막이 흐른 초를 고쳐 쓰는 주기(D6). */
const EMULATOR_BOOT_CAPTION_TICK_MS = 1_000;

/* 「부팅 중 · <기기> · n초」.
 *
 * 기기 이름은 스트림이 답해야 오는데(콜드 부팅이면 그게 수십 초다) 자막은
 * 판이 서는 순간 서야 하므로, 아직 이름이 없는 동안은 이름 칸이 없는 쪽을
 * 말한다 — 빈 칸에 가운뎃점만 남는 「부팅 중 ·  · 3초」가 되지 않게. */
function emulatorBootWords(tab, now = Date.now()) {
  const seconds = Math.max(0, Math.round((now - tab.bootingSince) / 1_000));
  const device = tab.deviceName || tab.deviceId || "";
  return device
    ? t("emulator.booting", "부팅 중 · {{device}} · {{seconds}}초", { device, seconds })
    : t("emulator.bootingUnnamed", "부팅 중 · {{seconds}}초", { seconds });
}

/* 이 탭이 제 말을 써도 되는 쪽지 칸, 아니면 null.
 *
 * 한 그룹의 에뮬레이터 화면은 그 그룹의 탭들이 나눠 쓰고, 그룹 0은 모든
 * 체크아웃의 것이다 — 다른 체크아웃에 선 거울(t-6379)도 같은 번호를 든다.
 * 다른 탭이 잡고 있는 화면에 자막이나 쪽지를 쓰면 사람이 보는 기기 밑에
 * 남의 기기 말이 선다. 임자가 없는 화면은 받는다 — 프레임이 그러듯
 * (`receiveEmulatorFrame`). */
function emulatorNoteOf(tab) {
  const host = groupOf(tab.pane)?.emulatorView;
  if (!host || (host._emulatorTab && host._emulatorTab !== tab)) return null;
  return host.querySelector(".emulator-note");
}

/* 자막을 지금 한 번 쓴다 — 부팅 중인 탭에만. */
function paintEmulatorBootCaption(tab) {
  if (tab.bootingSince == null) return;
  const note = emulatorNoteOf(tab);
  if (!note) return;
  note.textContent = emulatorBootWords(tab);
  note.hidden = false;
}

/* 자막을 세운다: 지금 한 번, 그리고 1초마다.
 *
 * 판이 서는 그 턴에 불리므로 첫 글자는 프레임 하나 안에 선다 — 스트림이
 * 대답하기를 기다리면 콜드 부팅 동안 판이 빈칸으로 남는다(실측 42.3 s). */
function standEmulatorBootCaption(tab) {
  dropEmulatorBootCaption(tab);
  tab.bootingSince = Date.now();
  paintEmulatorBootCaption(tab);
  tab.bootCaptionAt = setInterval(() => {
    if (!tabs.includes(tab) || tab.live) dropEmulatorBootCaption(tab);
    else paintEmulatorBootCaption(tab);
  }, EMULATOR_BOOT_CAPTION_TICK_MS);
}

/* 자막을 눕힌다 — 첫 프레임, 다른 소식, 닫힌 탭. */
function dropEmulatorBootCaption(tab) {
  if (tab.bootCaptionAt) clearInterval(tab.bootCaptionAt);
  tab.bootCaptionAt = null;
  tab.bootingSince = null;
}

/* 판 아래 한 줄이 자막 말고 다른 말을 한다 — 실패든 복구든 꺼짐이든.
 *
 * 네 자리가 저마다 세 줄로 같은 일을 적고 있었고, 자막이 붙은 지금은 그
 * 세 줄에 「자막을 먼저 눕힌다」가 더해진다. 빠뜨린 한 자리는 1초 뒤 자막이
 * 그 소식을 덮어쓰는 것으로 나타난다. */
function sayInEmulatorNote(tab, words) {
  dropEmulatorBootCaption(tab);
  const note = emulatorNoteOf(tab);
  if (!note) return;
  note.textContent = words;
  note.hidden = false;
}

/* 기기가 판 아래에서 사라졌다는 한 낱말. 쪽지도, 거절당한 탭도 이것을 들고
 * 오므로 두 자리가 서로 다른 말을 할 수 없다(백엔드 `EmulatorNoteCode::code`). */
const EMULATOR_DEVICE_OFFLINE = "device-offline";

function emulatorNoteMessage(code, fallback = "") {
  if (code === EMULATOR_DEVICE_OFFLINE) return t("emulator.deviceOffline", "기기가 꺼졌습니다 — 다시 켜지면 이어집니다.");
  if (code === "frame-unavailable") return t("emulator.frameUnavailable", "화면이 오지 않습니다. 기기가 멈춰 있으면 재부팅해 보세요.");
  if (code === "stream-ended") return t("emulator.streamEnded", "화면 연결이 종료됐습니다.");
  if (code === "video-start-failed") return t("emulator.videoStartFailed", "Android 비디오 인코더를 시작하지 못했습니다.");
  return fallback;
}

function reportEmulatorError(tab, error, key, throttle = false) {
  const detail = String(error);
  const message = emulatorErrorMessage(key);
  const now = Date.now();
  if (!throttle || !tab || now - (tab.inputErrorAt ?? 0) >= EMULATOR_INPUT_ERROR_THROTTLE_MS) {
    if (tab) tab.inputErrorAt = now;
    void invoke("log_window_error", { message: `emulator: ${detail}` }).catch(() => {});
    showError(message);
  }
  return message;
}

/* 한 탭이 가리키는 기기의 문. 안드로이드는 실행 중인 serial로, iOS는 udid로
 * 불린다 — 그 한 줄의 갈림이 이 블록에 아홉 번 적혀 있었다. */
function emulatorDoor(tab) {
  return tab.platform === "android" ? { serial: tab.udid } : { udid: tab.udid };
}

/* 그 기기에 낱말 하나를 보낸다.
 *
 * 명령의 이름은 플랫폼이 앞에 붙은 같은 동사이고, 실패는 어느 자리에서 나든
 * 같은 문장으로 말한다. 그 두 가지가 여덟 자리에 저마다 다른 줄바꿈으로
 * 적혀 있었고, 그중 하나만 고쳐지는 날이 오는 것이 이 함수가 막는 일이다. */
function sendEmulatorInput(tab, verb, extra = {}, { throttle = false } = {}) {
  const prefix = tab.platform === "android" ? "android" : "ios";
  return invoke(`${prefix}_${verb}`, { ...emulatorDoor(tab), ...extra }).catch((error) => {
    /* 기기가 꺼져서 거절된 입력은 「보내지 못했습니다」가 아니다 — 거절이
     * 쪽지의 코드를 그대로 들고 오므로, 판 아래에 이미 서 있는 그 문장을
     * 다시 말한다. 토스트를 띄우면 펌프가 조용히 기다리는 동안 사람은 제
     * 손이 고장 난 줄 안다. */
    if (String(error).includes(EMULATOR_DEVICE_OFFLINE)) {
      sayInEmulatorNote(tab, emulatorNoteMessage(EMULATOR_DEVICE_OFFLINE));
      return undefined;
    }
    return reportEmulatorError(tab, error, "emulator.inputFailed", throttle);
  });
}

/* 이 판이 지금 만질 수 있는 탭 — 아니면 아무것도 아니다. 일곱 자리가 같은
 * 세 조건을 물었다. (설치 버튼의 `appWorking`과 옆 버튼의 `disabled`는 이
 * 물음이 아니므로 그 자리에 그대로 남는다.) */
function liveEmulatorTab(host) {
  const tab = host._emulatorTab;
  return tab && tab.interactive && tab.live ? tab : null;
}

/* 버튼이 제 이름을 두 번 말한다 — 손끝의 말풍선에게 한 번, 보조기술에게 한 번.
 * 두 번 말하는 것은 맞지만 두 곳에 적을 일은 아니다. */
function labelButton(node, text) {
  node.dataset.tip = text;
  node.setAttribute("aria-label", text);
}

/* 그룹≠0의 판은 템플릿의 clone이라 리스너가 없다 — 브라우저 뷰의
 * `wireBrowserHost` 관용구 그대로, 첫 paint가 배선한다(중복 없음). */
function wireEmulatorHost(host) {
  if (host._emulatorWired) return;
  host._emulatorWired = true;
  // 프레임 그림의 디코드는 비동기로 — 초당 수십 장이 앉는 <img>이고, 동기
  // 디코드는 그 모두를 메인 스레드의 페인트에 얹는다. 에이전트 넷이 도는
  // 창에서 그 스레드는 이미 바쁘고, 여기서 밀린 만큼이 곧 미러의 프레임이다.
  host.querySelector(".emulator-frame").decoding = "async";
  host.querySelector(".emulator-outward").addEventListener("click", () => {
    const tab = host._emulatorTab;
    if (!tab || tab.platform !== "ios") return;
    void invoke("open_mobile_emulator", { udid: tab.udid }).catch((error) =>
      reportEmulatorError(
        tab,
        error,
        "emulator.connectionFailed",
      ),
    );
  });
  const pick = host.querySelector(".emulator-pick");
  pick.addEventListener("change", () => {
    const tab = host._emulatorTab;
    if (!tab || !pick.value) return;
    const [platform, ...rest] = pick.value.split(":");
    const id = rest.join(":");
    if (platform === tab.platform && id === (tab.deviceId ?? tab.udid)) return;
    void switchEmulatorDevice(tab, platform, id);
  });
  host.querySelector(".emulator-power").addEventListener("click", () => {
    const tab = host._emulatorTab;
    if (!tab) return;
    if (tab.live) void shutdownEmulatorTab(tab);
    else void attachEmulatorTab(tab);
  });
  host.querySelector(".emulator-install").addEventListener("click", async () => {
    const tab = host._emulatorTab;
    if (!tab || !tab.interactive || !tab.live || tab.appWorking) return;
    const epoch = tab.emulatorEpoch ?? 0;
    tab.appWorking = true;
    paintEmulatorView(tab);
    try {
      const path = await invoke("choose_emulator_app", { platform: tab.platform });
      if (!path) return;
      if (!tabs.includes(tab) || tab.emulatorEpoch !== epoch || !tab.live) return;
      const prefix = tab.platform === "android" ? "android" : "ios";
      await invoke(`${prefix}_install_app`, {
        ...emulatorDoor(tab),
        path,
        reinstall: tab.platform === "android",
      });
      toast(t("emulator.installDone", "앱을 설치했습니다"), "done");
    } catch (error) {
      reportEmulatorError(tab, error, "emulator.installFailed");
    } finally {
      tab.appWorking = false;
      if (tabs.includes(tab)) paintEmulatorView(tab);
    }
  });
  for (const key of host.querySelectorAll(".emulator-key")) {
    key.addEventListener("click", () => {
      const tab = liveEmulatorTab(host);
      if (!tab) return;
      const name = key.dataset.button;
      if (tab.platform === "ios" && name !== "home") return;
      void sendEmulatorInput(tab, "button", { name });
    });
  }
  host.querySelector(".emulator-rotate").addEventListener("click", () => {
    const tab = liveEmulatorTab(host);
    if (!tab) return;
    tab.rotation = tab.rotation === 1 ? 0 : 1;
    void sendEmulatorInput(tab, "rotate", { rotation: tab.rotation });
  });
  for (const key of host.querySelectorAll(".emulator-side-key")) {
    key.addEventListener("click", () => {
      const tab = liveEmulatorTab(host);
      if (!tab || key.disabled) return;
      const name = key.dataset.key;
      if (tab.platform === "android" && name === "action") return;
      void sendEmulatorInput(tab, "button", { name });
    });
  }
  // 화면을 만지면 그 손이 기기에 닿는다 — 그 화면은 screencap의 <img>이거나
  // H264 터널의 <canvas>다(1-g35). 어느 쪽이 앞에 서 있든 손은 같으므로 둘 다
  // 같은 손을 맨다.
  bindEmulatorTouch(host, host.querySelector(".emulator-frame"));
  bindEmulatorTouch(host, host.querySelector(".emulator-canvas"));
  // 미러가 활성일 때의 타이핑은 플랫폼 어댑터 하나로 흘러간다. 글자는 text,
  // 엔터·백스페이스는 HID/keyevent 버튼이다.
  host._emulatorKeydown = (event) => {
    const tab = liveEmulatorTab(host);
    if (!tab) return;
    const named = EMULATOR_NAMED_KEYS[event.key];
    if (named) {
      void sendEmulatorInput(tab, "button", { name: named }, { throttle: true });
    } else if (event.key.length === 1 && !event.metaKey && !event.ctrlKey) {
      void sendEmulatorInput(tab, "text", { text: event.key }, { throttle: true });
    } else {
      return;
    }
    event.preventDefault();
    event.stopImmediatePropagation?.();
  };
  // 판이 늘고 줄면 화면도 다시 잰다 — Orca처럼 rAF로 한 박자에 한 번만
  // (분할 드래그가 프레임마다 발화해도 계산은 그리기 박자를 따른다).
  let refitAt = null;
  host._emulatorResizeObserver = new ResizeObserver(() => {
    if (refitAt !== null) cancelAnimationFrame(refitAt);
    refitAt = requestAnimationFrame(() => {
      refitAt = null;
      if (host._emulatorTab) layoutEmulatorScreen(host, host._emulatorTab);
    });
  });
  host._emulatorResizeObserver.observe(host.querySelector(".emulator-body"));
}

const EMULATOR_WHEEL_GESTURE_IDLE_MS = 80;
const EMULATOR_WHEEL_MAX_TRAVEL = 0.65;

/* 한 표면에 손을 매단다: 누른 점과 뗀 점이 같으면 탭, 멀면
 * 스와이프 — 좌표는 그 표면 안에서 정규화(0..1)한다. */
function bindEmulatorTouch(host, surface) {
  const normal = (event) => {
    const rect = surface.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return null;
    return {
      x: Math.min(1, Math.max(0, (event.clientX - rect.left) / rect.width)),
      y: Math.min(1, Math.max(0, (event.clientY - rect.top) / rect.height)),
    };
  };
  let down = null;
  const pointers = new Map();
  let pointerContext = null;
  let multiTouch = null;
  let wheel = null;
  /* iOS 한 손가락은 실시간으로 중계된다 — begin이 누르는 순간에, move가
   * 움직이는 동안에, end가 놓는 순간에 기기로 간다. 놓을 때 260ms짜리
   * 스와이프 하나로 재생하던 옛 길은 두 가지를 부쉈다: 기기는 제스처가 다
   * 끝난 뒤에야 듣기 시작했고(홈을 끄는 손과 화면이 따로 놀았다 — "색이
   * 덮어지는것같고"의 절반은 뜻하지 않게 뽑힌 Spotlight/앱 보관함의 블러다),
   * 그 재생이 헬퍼의 한 줄 서기를 통째로 차지하는 동안 프레임 요청까지 뒤에
   * 줄을 섰다("체감상 시뮬레이터에 비해 현저히 느려"). 속도의 뜻도 이제
   * 사람의 손이다 — 천천히 끌면 천천히 끌린다.
   *
   * 사슬 하나로 순서를 지킨다: invoke는 도착 순서를 약속하지 않으므로, end가
   * move를 앞지르면 기기에 손가락이 낀다. move는 사슬에 하나만 태우고 보낼
   * 때 마지막 좌표를 읽는다 — 밀린 중간 좌표는 버려지는 것이 맞다. */
  let iosTouch = null;
  let touchChain = Promise.resolve();
  const sendTouchPhase = (stream, phase, point) => {
    touchChain = touchChain
      .then(() =>
        invoke("ios_touch", {
          udid: stream.udid,
          phase,
          x: point.x,
          y: point.y,
        }),
      )
      .catch((error) => {
        const tab = host._emulatorTab;
        if (tab) reportEmulatorError(tab, error, "emulator.inputFailed", true);
      });
  };
  const queueTouchMove = (stream) => {
    if (stream.moveQueued) return;
    stream.moveQueued = true;
    touchChain = touchChain
      .then(() => {
        stream.moveQueued = false;
        if (iosTouch !== stream) return undefined;
        return invoke("ios_touch", {
          udid: stream.udid,
          phase: "move",
          x: stream.last.x,
          y: stream.last.y,
        });
      })
      .catch(() => {
        stream.moveQueued = false;
      });
  };
  /* 시작한 터치는 반드시 끝난다 — end를 잃은 begin은 기기에 낀 손가락이고,
   * 낀 홈 제스처는 화면 전체를 반쯤 뽑힌 블러로 덮는다. 그래서 end는 문맥이
   * 어긋나도(기기를 갈아탔어도) begin이 갔던 그 udid로 간다. */
  const endIosTouch = (point = null) => {
    if (!iosTouch) return;
    const stream = iosTouch;
    iosTouch = null;
    if (point) stream.last = point;
    sendTouchPhase(stream, "end", stream.last);
  };
  const contextFor = (tab) => ({
    tab,
    epoch: tab.emulatorEpoch ?? 0,
    platform: tab.platform,
    udid: tab.udid,
  });
  const contextMatches = (context, tab) =>
    Boolean(context) && context.tab === tab && context.epoch === (tab?.emulatorEpoch ?? 0) &&
    context.platform === tab?.platform && context.udid === tab?.udid;
  const clearPointers = () => {
    endIosTouch();
    down = null;
    multiTouch = null;
    pointerContext = null;
    pointers.clear();
  };
  surface.addEventListener("pointerdown", (event) => {
    const tab = host._emulatorTab;
    if (!tab || !tab.interactive || !tab.live) return;
    if (pointerContext && !contextMatches(pointerContext, tab)) clearPointers();
    pointerContext ??= contextFor(tab);
    const point = normal(event);
    if (point) {
      const pointerId = event.pointerId ?? 0;
      pointers.set(pointerId, point);
      if (tab.platform === "ios" && pointers.size === 2) {
        // 둘째 손가락이 오면 흐르던 한 손가락은 그 자리에서 든다 — 두 손가락
        // 제스처는 새로 시작하는 것이고, 기기에 낀 첫 손가락 위에 얹는 것이
        // 아니다.
        endIosTouch();
        const pair = [...pointers.entries()];
        multiTouch = {
          ids: [pair[0][0], pair[1][0]],
          start: [pair[0][1], pair[1][1]],
          current: [pair[0][1], pair[1][1]],
        };
        down = null;
      } else if (pointers.size === 1) {
        if (tab.platform === "ios") {
          iosTouch = { pointerId, udid: tab.udid, last: point, moveQueued: false };
          sendTouchPhase(iosTouch, "begin", point);
        } else {
          down = { pointerId, point };
        }
      }
      try {
        surface.setPointerCapture?.(event.pointerId);
      } catch {
        // Synthetic accessibility/test pointers have no native capture slot.
      }
    }
    keySink.focus();
  });
  surface.addEventListener("pointermove", (event) => {
    const pointerId = event.pointerId ?? 0;
    if (!pointers.has(pointerId)) return;
    const point = normal(event);
    if (!point) return;
    pointers.set(pointerId, point);
    if (iosTouch && iosTouch.pointerId === pointerId) {
      iosTouch.last = point;
      queueTouchMove(iosTouch);
    }
    if (multiTouch) {
      multiTouch.current = multiTouch.ids.map((id, index) => pointers.get(id) ?? multiTouch.current[index]);
    }
  });
  surface.addEventListener("pointerup", (event) => {
    const tab = host._emulatorTab;
    const pointerId = event.pointerId ?? 0;
    const point = normal(event);
    if (point && pointers.has(pointerId)) pointers.set(pointerId, point);
    if (multiTouch && contextMatches(pointerContext, tab) && tab?.platform === "ios" && tab.interactive && tab.live) {
      multiTouch.current = multiTouch.ids.map((id, index) => pointers.get(id) ?? multiTouch.current[index]);
      const [start1, start2] = multiTouch.start;
      const [end1, end2] = multiTouch.current;
      void invoke("ios_multi_touch", {
        udid: tab.udid,
        x1: start1.x, y1: start1.y,
        x2: start2.x, y2: start2.y,
        x3: end1.x, y3: end1.y,
        x4: end2.x, y4: end2.y,
        ms: 260,
      }).catch((error) => reportEmulatorError(tab, error, "emulator.inputFailed"));
      clearPointers();
      return;
    }
    pointers.delete(pointerId);
    // 흐르던 iOS 손가락은 여기서 든다 — 문맥이 어긋났어도(기기를 갈아탔어도)
    // end는 간다: begin이 갔던 기기에 손가락이 끼는 것이 더 나쁜 결과다.
    if (iosTouch && iosTouch.pointerId === pointerId) {
      endIosTouch(point ?? null);
      if (pointers.size === 0) pointerContext = null;
      return;
    }
    if (!contextMatches(pointerContext, tab) || !tab?.interactive || !tab.live || !down || down.pointerId !== pointerId) {
      if (pointers.size === 0 || !contextMatches(pointerContext, tab)) clearPointers();
      return;
    }
    const up = point ?? down.point;
    const far = Math.hypot(up.x - down.point.x, up.y - down.point.y) > 0.02;
    const prefix = tab.platform === "android" ? "android" : "ios";
    const target = tab.platform === "android" ? { serial: tab.udid } : { udid: tab.udid };
    if (far) {
      void invoke(`${prefix}_swipe`, {
        ...target,
        x1: down.point.x, y1: down.point.y, x2: up.x, y2: up.y, ms: 260,
      }).catch((error) => reportEmulatorError(
        tab, error, "emulator.inputFailed",
      ));
    } else {
      void invoke(`${prefix}_tap`, { ...target, x: down.point.x, y: down.point.y }).catch((error) =>
        reportEmulatorError(tab, error, "emulator.inputFailed"),
      );
    }
    down = null;
    if (pointers.size === 0) pointerContext = null;
  });
  surface.addEventListener("pointercancel", (event) => {
    const pointerId = event.pointerId ?? 0;
    pointers.delete(pointerId);
    if (iosTouch && iosTouch.pointerId === pointerId) endIosTouch();
    if (down?.pointerId === pointerId) down = null;
    if (multiTouch?.ids.includes(pointerId)) {
      clearPointers();
    } else if (pointers.size === 0) {
      pointerContext = null;
    }
  });
  surface.addEventListener("wheel", (event) => {
    const tab = host._emulatorTab;
    if (!tab || !tab.interactive || !tab.live) return;
    const start = normal(event);
    const rect = surface.getBoundingClientRect();
    if (!start || rect.width === 0 || rect.height === 0) return;
    event.preventDefault();
    if (wheel && !contextMatches(wheel.context, tab) && wheel.timer !== null) {
      clearTimeout(wheel.timer);
    }
    if (!wheel || !contextMatches(wheel.context, tab)) {
      wheel = { start, dx: 0, dy: 0, timer: null, context: contextFor(tab) };
    }
    wheel.dx += event.deltaX / rect.width;
    wheel.dy += event.deltaY / rect.height;
    if (wheel.timer !== null) clearTimeout(wheel.timer);
    wheel.timer = setTimeout(() => {
      const pending = wheel;
      wheel = null;
      const current = host._emulatorTab;
      if (!pending || !contextMatches(pending.context, current) || !tab.live) return;
      const travel = (value) => Math.max(-EMULATOR_WHEEL_MAX_TRAVEL, Math.min(EMULATOR_WHEEL_MAX_TRAVEL, value));
      const end = {
        x: Math.max(0, Math.min(1, pending.start.x - travel(pending.dx))),
        y: Math.max(0, Math.min(1, pending.start.y - travel(pending.dy))),
      };
      if (Math.hypot(end.x - pending.start.x, end.y - pending.start.y) < 0.01) return;
      const prefix = tab.platform === "android" ? "android" : "ios";
      const target = tab.platform === "android" ? { serial: tab.udid } : { udid: tab.udid };
      void invoke(`${prefix}_swipe`, {
        ...target,
        x1: pending.start.x,
        y1: pending.start.y,
        x2: end.x,
        y2: end.y,
        ms: 160,
      }).catch((error) => reportEmulatorError(tab, error, "emulator.inputFailed"));
    }, EMULATOR_WHEEL_GESTURE_IDLE_MS);
  }, { passive: false });
}

function paintEmulatorView(tab) {
  const host = docHost(tab.pane, "emulator");
  wireEmulatorHost(host);
  host._emulatorTab = tab;
  host.querySelector(".emulator-name").textContent = tab.deviceName ?? "";
  const status = host.querySelector(".emulator-status");
  // 낱말 옆의 점은 같은 세 갈래를 형태로 되풀이한다. 색만으로 말하지 않기 위한
  // 것이므로(`docs/design/direction.md` 1항) 낱말과 다른 판단에서 나오면 안 되고,
  // 그래서 갈래를 한 번만 정해 낱말과 점이 함께 그것을 읽는다.
  status.dataset.state = tab.working ? "working" : tab.live ? "live" : "off";
  status.textContent = tab.working
    ? t("emulator.working", "작업 중…")
    : tab.live
      ? t("emulator.connected", "연결됨")
      : t("emulator.detached", "연결 안 됨");
  const power = host.querySelector(".emulator-power");
  power.innerHTML = icon(tab.live ? "circle-x" : "forward");
  const said = tab.live
    ? t("emulator.shutdown", "에뮬레이터 끄기")
    : t("emulator.attach", "연결");
  labelButton(power, said);
  power.disabled = tab.working === true || tab.appWorking === true;
  const install = host.querySelector(".emulator-install");
  const installLabel = tab.appWorking
    ? t("emulator.installing", "앱을 설치하는 중…")
    : t("emulator.install", "앱 설치…");
  labelButton(install, installLabel);
  install.disabled = !tab.interactive || !tab.live || tab.appWorking === true;
  const keys = host.querySelector(".emulator-keys");
  keys.hidden = !tab.interactive;
  host.querySelector(".emulator-rotate").hidden = !tab.interactive;
  for (const key of host.querySelectorAll(".emulator-key")) {
    key.hidden = !tab.interactive || (tab.platform === "ios" && key.dataset.button !== "home");
  }
  for (const key of host.querySelectorAll(".emulator-side-key")) {
    key.disabled = !tab.interactive || !tab.live;
  }
  host.querySelector(".emulator-outward").hidden = tab.platform !== "ios";
  const canTouch = tab.interactive === true && tab.live;
  const frame = host.querySelector(".emulator-frame");
  frame.classList.toggle("is-interactive", canTouch);
  host.querySelector(".emulator-canvas").classList.toggle("is-interactive", canTouch);
  const pick = host.querySelector(".emulator-pick");
  pick.disabled = tab.working === true || tab.appWorking === true;
  if (document.activeElement !== pick) {
    const fleet = Array.isArray(mobileEmulatorList) ? mobileEmulatorList : [];
    const signature = JSON.stringify(fleet.map(({ platform, id, name }) => [platform, id, name]));
    if (pick._painted !== signature) {
      pick._painted = signature;
      pick.replaceChildren(...fleet.map((device) => {
        const option = document.createElement("option");
        option.value = `${device.platform}:${device.id}`;
        option.textContent = device.name + (device.platform === "android" ? " · Android" : "");
        return option;
      }));
    }
    pick.value = `${tab.platform}:${tab.deviceId ?? tab.udid ?? ""}`;
  }
  // 첫 프레임이 `live` 를 세우는 곳이 바로 여기다 — 그러니 자막이 눕는
  // 자리도 여기고, 그림이 아직 없는 동안은 이 판을 다시 그릴 때마다 자막이
  // 제 자리에 남는다(판을 갈아 끼우면 note 는 새 것이다).
  if (tab.live) dropEmulatorBootCaption(tab);
  else paintEmulatorBootCaption(tab);
  // 프레임 베젤은 이름이 말하는 꼴을 입고, 화면은 판을 재서 앉는다(1-g45).
  layoutEmulatorScreen(host, tab);
}

/* 기기 갈아타기: 옛 펌프를 멈추고 고른 기기로 새 스트림 — 탭은 그대로,
 * 이름표만 그 기기의 것이 된다.
 *
 * `focus`는 이 여는 일이 화면을 가져가도 되는가다. 다른 체크아웃에 선
 * 에이전트의 거울(t-6379)은 안 된다: 그 탭은 화면에 없으니 그리지 않고,
 * 같은 기기의 스트림이 이미 있어 그 탭으로 합쳐질 때도 사람의 화면을 그
 * 탭으로 넘기지 않는다.
 *
 * `borrower`는 이 여는 일을 부탁한 에이전트의 판이다(t-6336) — 에이전트의
 * open만 싣는다. 백엔드는 이 시작이 기기를 부팅했을 때만 그 판에 빌려 준
 * 것으로 적고, 그 판의 일이 끝나면 끈다. 사람이 고른 기기, 사람이 다시 붙인
 * 기기는 누구의 빌림도 아니다. */
async function switchEmulatorDevice(tab, platform, id, { focus = true, borrower = null } = {}) {
  const epoch = (tab.emulatorEpoch ?? 0) + 1;
  tab.emulatorEpoch = epoch;
  tab.working = true;
  tab.live = false;
  if (stillShowing(tab)) paintEmulatorView(tab);
  // 자막은 여기서 선다(D6) — 판이 서는 것과 같은 턴이고, 기기를 갈아타도
  // 새 기기의 초부터 다시 센다.
  standEmulatorBootCaption(tab);
  const previous = tab.stream;
  dropEmulatorBinaryDoor(tab);
  tab.video = false;
  tab.streamPaused = undefined;
  // 크기도 옛 기기의 것이다 — 새 기기의 첫 그림이 제 크기를 다시 잰다(1-g45).
  tab.screenSize = null;
  await teardownEmulatorStream(previous);
  let failure = null;
  const binaryDoor = makeEmulatorBinaryDoor(
    "frame",
    platform === "android" ? "image/png" : "image/jpeg",
  );
  try {
    // 기기를 대지 않고 부를 수 있다 — 새 탭이 그 길로 온다. 그때 고르는 일은
    // 백엔드의 몫이고(부팅된 것 우선), 창은 그것이 답한 이름을 입는다.
    const lent = borrower === null ? {} : { borrower };
    const said =
      platform === "android"
        ? await invoke("start_android_stream", {
            ...(id ? { avd: id } : {}),
            ...(binaryDoor ? { onFrame: binaryDoor.channel } : {}),
            ...lent,
          })
        : await invoke("start_emulator_stream", {
            ...(id ? { udid: id } : {}),
            ...(tab.viewportLongEdge ? { viewport: { longEdgePx: tab.viewportLongEdge } } : {}),
            ...(binaryDoor ? { onFrame: binaryDoor.channel } : {}),
            ...lent,
          });
    if (tab.emulatorEpoch !== epoch || !tabs.includes(tab)) {
      binaryDoor?.dispose();
      if (said?.stream) void invoke("stop_emulator_stream", { stream: said.stream }).catch(() => {});
      return;
    }
    const existing = said?.reused
      ? tabs.find((one) => one !== tab && one.kind === "emulator" && one.stream === said.stream)
      : null;
    if (existing) {
      binaryDoor?.dispose();
      closeTab(tab.id);
      if (focus) setActiveTab(existing.id);
      return;
    }
    tab.platform = platform;
    tab.stream = said.stream;
    binaryDoor?.bind(said.stream);
    tab.binaryDoor = binaryDoor;
    // iOS: udid IS the device id. Android: the device id is the AVD name, and
    // `udid` becomes the running serial the input doors need.
    tab.udid = said.udid;
    tab.deviceId = platform === "android" ? said.name : said.udid;
    tab.deviceName = said.name;
    tab.interactive = platform === "android" ? said.interactive !== false : said.interactive === true;
    // 판이 서자마자 H264 터널로 갈아탄다 — 문이 닫혀 있으면 screencap 그대로.
    await standEmulatorVideo(tab, epoch);
  } catch (error) {
    binaryDoor?.dispose();
    failure = reportEmulatorError(
      tab,
      error,
      "emulator.connectionFailed",
    );
  }
  if (tab.emulatorEpoch !== epoch || !tabs.includes(tab)) return;
  tab.working = false;
  if (stillShowing(tab)) {
    const host = docHost(tab.pane, "emulator");
    const frame = host.querySelector(".emulator-frame");
    frame.hidden = true;
    // 갈아탄 기기는 제 첫 그림부터 다시 시작한다 — 옛 터널이 남긴 그림은
    // 이 기기의 것이 아니다.
    host.querySelector(".emulator-canvas").hidden = true;
    // 기기가 섰다고 그림이 온 것은 아니다 — 실패만 자막의 자리를 가져간다.
    if (failure === null) paintEmulatorBootCaption(tab);
    else sayInEmulatorNote(tab, failure);
    paintEmulatorView(tab);
  }
  syncEmulatorStreamVisibility();
  renderTabs();
}

async function attachEmulatorTab(tab) {
  await switchEmulatorDevice(tab, tab.platform, tab.deviceId ?? tab.udid);
}

async function shutdownEmulatorTab(tab) {
  tab.emulatorEpoch = (tab.emulatorEpoch ?? 0) + 1;
  tab.working = true;
  paintEmulatorView(tab);
  dropEmulatorBinaryDoor(tab);
  tab.video = false;
  await teardownEmulatorStream(tab.stream);
  tab.streamPaused = undefined;
  let failure = null;
  try {
    if (tab.platform === "android") {
      await invoke("shutdown_android_emulator", { serial: tab.udid });
    } else {
      await invoke("shutdown_mobile_emulator", { udid: tab.udid });
    }
  } catch (error) {
    failure = reportEmulatorError(
      tab,
      error,
      "emulator.shutdownFailed",
    );
  }
  tab.working = false;
  tab.live = false;
  // 꺼진 화면의 크기는 없다 — 다시 붙는 기기가 제 크기를 새로 잰다(1-g45).
  tab.screenSize = null;
  // 끈 판은 빈 판으로 남지 않는다: 같은 자리에 빈 셸이 선다 ("ios를 끄면
  // 화면이 비[어] … 자동으로 터미널창이 켜져야함"). 갈아끼우는 순서가 뜻이다 —
  // 셸이 먼저 자리에 앉고 에뮬레이터 탭이 나중에 빠지므로, 그 자리(분할 칸)가
  // 비는 프레임이 없다. 실패한 끄기는 그대로 남는다: 기기가 아직 살아 있고,
  // 판을 갈아치우면 그 사실도 다시 시도할 전원 단추도 함께 사라진다.
  if (failure === null && tabs.includes(tab)) {
    let term = null;
    try {
      term = await invoke("open_term_tab", { rows: 24, cols: 96, plain: true });
    } catch {
      term = null;
    }
    if (term !== null) {
      const seat = tab.pane;
      const shell = {
        id: termTabId(term),
        kind: "term",
        term,
        worktree: activeWorktreePath,
        layout: paneLeaf(term),
        activePane: term,
        ...(seat == null ? {} : { pane: seat }),
      };
      renderPanes(shell);
      openTab(shell, { focus: stillShowing(tab) });
      dropTab(tab.id);
      if (seat != null) persistStageLayouts();
      termViews.get(term)?.measure();
      resizeTermTab(term);
      return;
    }
  }
  if (stillShowing(tab)) {
    const host = docHost(tab.pane, "emulator");
    host.querySelector(".emulator-frame").hidden = true;
    host.querySelector(".emulator-canvas").hidden = true;
    sayInEmulatorNote(tab, t("emulator.off", "에뮬레이터가 꺼졌습니다"));
    paintEmulatorView(tab);
  }
}

/* 스트림별 앞 프레임의 objectURL — 해제는 다음 프레임이 앉는 순간이다. */
const emulatorFrames = new Map();

/* 그 스트림이 들고 있던 그림을 놓는다.
 *
 * objectURL은 문서가 살아 있는 한 회수되지 않으므로, 잊는 것과 놓는 것은
 * 반드시 같은 동작이어야 한다 — 네 자리가 저마다 세 줄로 그 둘을 적고 있었고,
 * 한 자리에서 `delete`만 하고 `revoke`를 빠뜨리는 것이 이 함수가 막는 일이다. */
function releaseEmulatorFrame(stream) {
  const held = emulatorFrames.get(stream);
  if (held) URL.revokeObjectURL(held);
  emulatorFrames.delete(stream);
}

/* 스트림 하나를 완전히 내린다 — 비디오 문, 백엔드 펌프, 붙잡고 있던 그림.
 *
 * 같은 세 줄이 다섯 자리에 합어져 있었고 자리마다 하나씩 빠져 있었다 — 비디오
 * 복구는 그림을 놓지 않았고, 비디오 승격은 문을 걷지 않았다. 셋 다 없는
 * 스트림에는 아무 일도 하지 않으므로 한 자리에서 셋을 다 부르는 것이 안전하다. */
async function teardownEmulatorStream(stream) {
  if (!stream) return;
  dropEmulatorVideo(stream);
  releaseEmulatorFrame(stream);
  await invoke("stop_emulator_stream", { stream }).catch(() => {});
}
const EMULATOR_BINARY_HEADER_BYTES = 8;

function makeEmulatorBinaryDoor(kind, mime = null) {
  if (typeof TauriChannel !== "function") return null;
  const channel = new TauriChannel();
  const door = { channel, stream: null, pending: null };
  const deliver = ({ sequence, bytes }) => {
    if (kind === "video") receiveEmulatorVideo(door.stream, bytes, sequence);
    else receiveEmulatorFrame(door.stream, bytes, sequence, mime);
  };
  channel.onmessage = (message) => {
    const packet = new Uint8Array(message);
    if (packet.byteLength < EMULATOR_BINARY_HEADER_BYTES) return;
    const sequence = Number(new DataView(
      packet.buffer,
      packet.byteOffset,
      EMULATOR_BINARY_HEADER_BYTES,
    ).getBigUint64(0));
    const payload = {
      sequence,
      bytes: packet.subarray(EMULATOR_BINARY_HEADER_BYTES),
    };
    if (door.stream === null) door.pending = payload;
    else deliver(payload);
  };
  door.bind = (stream) => {
    door.stream = stream;
    if (door.pending) {
      const pending = door.pending;
      door.pending = null;
      deliver(pending);
    }
  };
  door.dispose = () => {
    door.pending = null;
    door.stream = null;
    channel.onmessage = () => {};
    channel.cleanupCallback?.();
  };
  return door;
}

function dropEmulatorBinaryDoor(tab) {
  tab?.binaryDoor?.dispose?.();
  if (tab) tab.binaryDoor = null;
}

/* 숨은 탭은 화면을 소비하지 않는다. 백엔드도 같은 사실을 알아야 캡처·인코더가
 * 메모리와 CPU를 쓰지 않는다. 스트림마다 마지막 답을 기억해 stage repaint가
 * 같은 IPC를 반복하지 않게 한다. */
function syncEmulatorStreamVisibility() {
  for (const tab of tabs) {
    if (tab.kind !== "emulator" || !tab.stream) continue;
    const host = groupOf(tab.pane)?.emulatorView;
    // `host.hidden`은 그룹이 에뮬레이터를 보여주는지만 말한다. 한 그룹에
    // 에뮬레이터 탭이 둘이면 뒤에 가려진 쪽도 같은 보이는 자리를 가리키므로
    // 저 물음만으로는 영영 멈추지 않고, 캡처와 인코더가 아무도 안 보는
    // 그림을 계속 만든다.
    const paused =
      document.hidden ||
      !host ||
      host.hidden ||
      (host._emulatorTab != null && host._emulatorTab !== tab);
    if (tab.streamPaused === paused) continue;
    tab.streamPaused = paused;
    void invoke("set_emulator_stream_paused", { stream: tab.stream, paused }).catch(() => {});
  }
}

document.addEventListener("visibilitychange", syncEmulatorStreamVisibility);

function base64Bytes(bytes) {
  // Current WebView engines decode straight into the destination buffer,
  // avoiding `atob`'s full-size UTF-16-ish intermediary and the JS byte loop.
  // Older engines keep the compatible path below.
  if (typeof Uint8Array.fromBase64 === "function") return Uint8Array.fromBase64(bytes);
  const raw = atob(bytes);
  const held = new Uint8Array(raw.length);
  for (let at = 0; at < raw.length; at += 1) held[at] = raw.charCodeAt(at);
  return held;
}

/* ---- H264 비디오 문(1-g35) ----
 *
 * WebCodecs가 있으면 screencap 폴링(~1.5fps)을 내리고 기기 인코더의 스트림을
 * canvas에 그린다. 오는 것은 Annex-B라 시작 코드로 갈라 NAL로 만들고, 매개변수
 * 셋(SPS·PPS)이 모인 뒤 첫 IDR에서 디코더가 선다. 회전은 재spawn이 새 SPS로
 * 답하고 — 그때 디코더는 눕고 새 크기로 다시 선다 — 정지 화면은 스트림이
 * 조용한 것이지 죽은 게 아니다. */
const EMULATOR_NAL_SLICE = 1;
const EMULATOR_NAL_IDR = 5;
const EMULATOR_NAL_SPS = 7;
const EMULATOR_NAL_PPS = 8;
const EMULATOR_MAX_NAL_CARRY_BYTES = 2 * 1024 * 1024;
const EMULATOR_MAX_DECODE_QUEUE = 2;

/* 읽기 한 번이 NAL 경계에서 끝난다는 보장은 없다: 시작 코드(3바이트 00 00 01,
 * 또는 앞에 0이 하나 더 붙은 4바이트)로 갈라 온전한 것만 내주고, 마지막 시작
 * 코드부터는 아직 열린 NAL로 보아 코드째 남긴다 — 다음 먹이가 그 자리를 잇는다. */
function splitAnnexBNals(held) {
  const nals = [];
  let codeAt = -1;
  let from = -1;
  let at = 0;
  while (at + 2 < held.length) {
    if (held[at] !== 0 || held[at + 1] !== 0) {
      at += 1;
      continue;
    }
    const size = held[at + 2] === 1 ? 3 : held[at + 2] === 0 && held[at + 3] === 1 ? 4 : 0;
    if (size === 0) {
      at += 1;
      continue;
    }
    if (from >= 0 && at > from) nals.push(held.subarray(from, at));
    codeAt = at;
    from = at + size;
    at = from;
  }
  return { nals, rest: codeAt < 0 ? held : held.subarray(codeAt) };
}

/* WebCodecs의 avc1은 avcC 상자로 말문을 연다 — 프로필·호환·레벨은 SPS의 1..3
 * 바이트가 쥐고 있고, 디코더는 그것을 보고서야 선다. */
function avccOfParameterSets(sps, pps) {
  const held = new Uint8Array(11 + sps.length + pps.length);
  held.set([1, sps[1], sps[2], sps[3], 0xff, 0xe1, (sps.length >> 8) & 0xff, sps.length & 0xff]);
  held.set(sps, 8);
  held.set([1, (pps.length >> 8) & 0xff, pps.length & 0xff], 8 + sps.length);
  held.set(pps, 11 + sps.length);
  return held;
}

/* avcC로 선 디코더는 시작 코드가 아니라 길이 접두(4바이트 빅엔디안)를 먹는다. */
function lengthPrefixedNal(nal) {
  const held = new Uint8Array(4 + nal.length);
  held[0] = (nal.length >>> 24) & 0xff;
  held[1] = (nal.length >>> 16) & 0xff;
  held[2] = (nal.length >>> 8) & 0xff;
  held[3] = nal.length & 0xff;
  held.set(nal, 4);
  return held;
}

/* 한 스트림의 문: 바이트를 먹고 프레임을 뱉는다. `paint`는 선 프레임을 받고,
 * `fell`은 디코더가 넘어졌을 때 그 말을 받는다. */
function makeEmulatorVideoDoor(paint, fell) {
  const door = {
    paint,
    fell,
    carry: new Uint8Array(0),
    sps: null,
    pps: null,
    decoder: null,
    tick: 0,
    down: false,
  };
  door.feed = (bytes) => {
    if (door.down) return;
    if (door.carry.length + bytes.length > EMULATOR_MAX_NAL_CARRY_BYTES) {
      door.down = true;
      door.fell(t("emulator.videoOverflow", "비디오 프레임 경계가 너무 큽니다"));
      return;
    }
    const held = new Uint8Array(door.carry.length + bytes.length);
    held.set(door.carry);
    held.set(bytes, door.carry.length);
    const { nals, rest } = splitAnnexBNals(held);
    // 남은 꼬리는 제 복사본으로 — subarray는 방금 만든 판을 붙들고 있다.
    door.carry = rest.slice();
    for (const nal of nals) {
      const kind = nal[0] & 0x1f;
      if (kind === EMULATOR_NAL_SPS) {
        door.sps = nal.slice();
        // 회전 뒤의 새 SPS — 옛 디코더를 눕히고 새 크기로 다시 선다.
        door.close();
        continue;
      }
      if (kind === EMULATOR_NAL_PPS) {
        door.pps = nal.slice();
        continue;
      }
      if (kind !== EMULATOR_NAL_IDR && kind !== EMULATOR_NAL_SLICE) continue;
      // 키프레임 전의 조각은 그릴 수 없다 — 매개변수 셋과 IDR을 기다린다.
      if (!door.decoder && (!door.sps || !door.pps || kind !== EMULATOR_NAL_IDR)) continue;
      try {
        if (!door.decoder) {
          const codec =
            "avc1." +
            [...door.sps.slice(1, 4)].map((one) => one.toString(16).padStart(2, "0")).join("");
          door.decoder = new VideoDecoder({
            output: (frame) => {
              door.paint(frame);
              frame.close();
            },
            error: (error) => {
              door.down = true;
              door.fell(String(error));
            },
          });
          // 미러는 만지는 판이다 — 디코더가 프레임을 몇 장 쥐고 있다가
          // 내보내면 그만큼 손이 늦는다. `optimizeForLatency`는 오는 대로
          // 내보내라는 말(실측: 이것 없이는 첫 장만 나오고 나머지는 버퍼에
          // 머문다).
          door.decoder.configure({
            codec,
            description: avccOfParameterSets(door.sps, door.pps),
            optimizeForLatency: true,
          });
        }
        // WebCodecs가 그리기보다 입력을 늦게 먹으면 오래된 delta를 버린다.
        // 키프레임은 복구점이라 절대 버리지 않는다.
        if (
          kind !== EMULATOR_NAL_IDR &&
          door.decoder.decodeQueueSize > EMULATOR_MAX_DECODE_QUEUE
        ) continue;
        door.decoder.decode(
          new EncodedVideoChunk({
            type: kind === EMULATOR_NAL_IDR ? "key" : "delta",
            timestamp: door.tick * 1000,
            data: lengthPrefixedNal(nal),
          }),
        );
        door.tick += 1;
      } catch (error) {
        // 넘어진 문은 다시 세우지 않는다 — 다음 attach가 새 문을 연다.
        door.down = true;
        door.fell(String(error));
        return;
      }
    }
  };
  door.close = () => {
    if (!door.decoder) return;
    try {
      door.decoder.close();
    } catch {
      // 이미 누운 디코더 — 닫는 일에 두 번은 없다.
    }
    door.decoder = null;
  };
  door.dispose = () => {
    door.down = true;
    door.close();
    door.carry = new Uint8Array(0);
    door.sps = null;
    door.pps = null;
  };
  return door;
}

/* 스트림별로 열려 있는 비디오 문. */
const emulatorVideoDoors = new Map();

/* screencap에서 H264로 갈아탄다: 창이 WebCodecs를 가졌고 판이 Android일 때만
 * 두 번째 문을 두드린다. 문이 열리면 옛 스트림을 내리고 canvas가 <img>를
 * 대신하며, 열리지 않으면 아무 일도 없었던 듯 screencap이 그대로 흐른다. */
async function recoverEmulatorVideo(tab, failedStream, error) {
  if (!tabs.includes(tab) || tab.stream !== failedStream || tab.platform !== "android") return;
  await teardownEmulatorStream(failedStream);
  dropEmulatorBinaryDoor(tab);
  const epoch = tab.emulatorEpoch ?? 0;
  const binaryDoor = makeEmulatorBinaryDoor("frame", "image/png");
  try {
    const said = await invoke("start_android_stream", {
      avd: tab.deviceId,
      ...(binaryDoor ? { onFrame: binaryDoor.channel } : {}),
    });
    if (!tabs.includes(tab) || tab.emulatorEpoch !== epoch) {
      binaryDoor?.dispose();
      if (said?.stream) void invoke("stop_emulator_stream", { stream: said.stream }).catch(() => {});
      return;
    }
    tab.stream = said.stream;
    binaryDoor?.bind(said.stream);
    tab.binaryDoor = binaryDoor;
    tab.video = false;
    tab.streamPaused = undefined;
    tab.interactive = said.interactive !== false;
    tab.live = false;
    const host = groupOf(tab.pane)?.emulatorView;
    if (host) {
      host.querySelector(".emulator-canvas").hidden = true;
      sayInEmulatorNote(tab, t("emulator.videoFallback", "비디오 연결을 복구하는 중…"));
      paintEmulatorView(tab);
    }
    syncEmulatorStreamVisibility();
  } catch (fallbackError) {
    binaryDoor?.dispose();
    sayInEmulatorNote(
      tab,
      reportEmulatorError(tab, `${error} · ${fallbackError}`, "emulator.connectionFailed"),
    );
  }
}

async function standEmulatorVideo(tab, epoch = tab.emulatorEpoch ?? 0) {
  if (tab.platform !== "android" || typeof VideoDecoder === "undefined") return;
  let said;
  const binaryDoor = makeEmulatorBinaryDoor("video");
  try {
    said = await invoke("start_emulator_video", {
      serial: tab.udid,
      ...(binaryDoor ? { onVideo: binaryDoor.channel } : {}),
    });
  } catch {
    binaryDoor?.dispose();
    // 폴백: 이 기계엔 터널이 없다 — 폴링이 계속 그린다.
    return;
  }
  // 대답이 비어도 같은 폴백이다 — 문 없는 백엔드가 null로 답할 수 있다.
  if (!said?.stream) {
    binaryDoor?.dispose();
    return;
  }
  if (!tabs.includes(tab) || tab.emulatorEpoch !== epoch) {
    binaryDoor?.dispose();
    void invoke("stop_emulator_stream", { stream: said.stream }).catch(() => {});
    return;
  }
  const shed = tab.stream;
  const shedBinary = tab.binaryDoor;
  const videoStream = said.stream;
  binaryDoor?.bind(videoStream);
  const door = makeEmulatorVideoDoor(
    (frame) => {
      const host = groupOf(tab.pane)?.emulatorView;
      const screen = host?.querySelector(".emulator-canvas");
      if (!screen) return;
      if (screen.width !== frame.displayWidth || screen.height !== frame.displayHeight) {
        screen.width = frame.displayWidth;
        screen.height = frame.displayHeight;
        // 크기는 첫 프레임과 회전(재spawn의 새 SPS)만 바꾼다 — 그때만
        // 판을 다시 잰다(1-g45).
        tab.screenSize = { width: frame.displayWidth, height: frame.displayHeight };
        layoutEmulatorScreen(host, tab);
      }
      screen.getContext("2d")?.drawImage(frame, 0, 0);
      if (screen.hidden) {
        screen.hidden = false;
        host.querySelector(".emulator-frame").hidden = true;
      }
      const note = host.querySelector(".emulator-note");
      if (note) note.hidden = true;
      if (!tab.live) {
        tab.live = true;
        paintEmulatorView(tab);
      }
    },
    (error) => {
      void recoverEmulatorVideo(tab, videoStream, error);
    },
  );
  emulatorVideoDoors.set(videoStream, door);
  tab.video = true;
  tab.stream = videoStream;
  tab.binaryDoor = binaryDoor;
  tab.streamPaused = undefined;
  await teardownEmulatorStream(shed);
  shedBinary?.dispose?.();
  syncEmulatorStreamVisibility();
}

/* 터널을 걷는다 — 스트림이 죽는 자리마다(탭 닫기·기기 갈아타기·전원 끄기). */
function dropEmulatorVideo(stream) {
  const door = emulatorVideoDoors.get(stream);
  if (!door) return;
  door.dispose();
  emulatorVideoDoors.delete(stream);
}

function receiveEmulatorVideo(stream, bytes, sequence) {
  const door = emulatorVideoDoors.get(stream);
  try {
    if (door) door.feed(bytes);
  } finally {
    if (Number.isSafeInteger(sequence)) {
      void invoke("acknowledge_emulator_payload", { stream, sequence }).catch(() => {});
    }
  }
}

listen("emulator:video", (event) => {
  const { stream, bytes, sequence } = event.payload;
  receiveEmulatorVideo(stream, base64Bytes(bytes), sequence);
});

/* 슬롯은 바이트가 도착한 그 자리에서 돌려준다 — 그림이 화면에 앉기를
 * 기다리지 않는다.
 *
 * ack가 `frame.onload`에 매달려 있으면 그것은 역압(backpressure) 신호가
 * 아니라 지연(latency) 신호다. 펌프는 고리의 맨 위에서 슬롯부터
 * 기다리고(ios.rs:533 `wait_for_payload_slot`), 그 다음에야 시계를 켜
 * `FRAME_CEILING`을 잰다 — 그러니 ack가 늦은 만큼은 16ms 안에 흡수되는
 * 것이 아니라 16ms 위에 그대로 얹힌다. 그리고 그 ack는 그림이 다
 * 읽힐 때까지 기다린 뒤에야 두 번째 IPC 왕복(macOS에서 invoke는 ipc
 * 커스텀 프로토콜로 가는 fetch다)을 통째로 더 태운다.
 *
 * 창이 하나보다 넓어지는 순간 onload는 신뢰할 수 없는 자리이기도 하다:
 * 같은 <img>에 src를 두 번 연달아 얹으면 WebKit은 앞 장의 로드를
 * 중단하고 마지막 것에만 load를 발화한다. `onerror`도 같은 이유로
 * 사라진다 — 그것이 하던 일은 ack를 흘리지 않는 것뿐이었고, 이제 ack는
 * 그림보다 먼저 떠난다. 바이트가 깨진 그림은 안 앉아도 ack되고, 펌프는
 * 멈추는 대신 계속 간다 — onerror가 바로 그것을 주선하고 있었다. */
function receiveEmulatorFrame(stream, bytes, sequence, mime = "image/jpeg") {
  if (Number.isSafeInteger(sequence)) {
    void invoke("acknowledge_emulator_payload", { stream, sequence }).catch(() => {});
  }
  const tab = tabs.find((one) => one.kind === "emulator" && one.stream === stream);
  if (!tab) return;
  const host = groupOf(tab.pane)?.emulatorView;
  const frame = host?.querySelector(".emulator-frame");
  if (!frame) return;
  // 이 자리는 그룹의 것이고, 같은 그룹의 에뮬레이터 탭들이 그것 하나를 나눠
  // 쓴다(22731과 같은 사실). 가려진 탭의 프레임을 여기에 앉히면 앞에 있는
  // 탭의 화면이 남의 기기로 바뀌고, 아래의 `frame.hidden = false`와 쪽지
  // 감추기가 그 탭이 아직 기다리고 있는 말을 대신 지운다. 임자가 없는
  // 자리는 아무도 보고 있지 않으므로 그대로 받는다 — 첫 그림이 첫
  // paintEmulatorView보다 빨리 와도 잃지 않기 위해서다.
  if (host._emulatorTab && host._emulatorTab !== tab) return;
  const was = emulatorFrames.get(stream);
  let url;
  try {
    url = URL.createObjectURL(new Blob([bytes], { type: mime }));
  } catch {
    return;
  }
  emulatorFrames.set(stream, url);
  // onload 속성이라 프레임마다 갈아끼워도 리스너가 쌓이지 않는다 — 크기가
  // 같은 다음 그림은 그냥 앉고, 첫 그림과 회전만 판을 다시 잰다(1-g45).
  //
  // 이제 이 자리가 지는 것은 치수뿐이다. 다음 장이 앞 장의 로드를 끊으면 이
  // 되불림은 그 한 장에 대해 건너뛰어지고, 실제로 앉는 다음 그림이 같은
  // 조건을 다시 재므로 스스로 낫는다 — 다만 첫 치수가 반드시 첫 장에서
  // 온다는 보장은 이제 없다.
  frame.onload = () => {
    if (
      frame.naturalWidth > 0 &&
      (tab.screenSize?.width !== frame.naturalWidth ||
        tab.screenSize?.height !== frame.naturalHeight)
    ) {
      tab.screenSize = { width: frame.naturalWidth, height: frame.naturalHeight };
      layoutEmulatorScreen(host, tab);
    }
  };
  frame.src = url;
  frame.hidden = false;
  const note = host.querySelector(".emulator-note");
  if (note) note.hidden = true;
  if (was) URL.revokeObjectURL(was);
  if (!tab.live) {
    tab.live = true;
    paintEmulatorView(tab);
    tab.engaged = undefined;
    reportEmulatorAttention(host, emulatorAttentionOf(host));
  }
}

listen("emulator:frame", (event) => {
  const { stream, bytes, sequence, mime = "image/jpeg" } = event.payload;
  try {
    receiveEmulatorFrame(stream, base64Bytes(bytes), sequence, mime);
  } catch {
    if (Number.isSafeInteger(sequence)) {
      void invoke("acknowledge_emulator_payload", { stream, sequence }).catch(() => {});
    }
  }
});

/* 입력 문이 열렸다는 소식. 스트림을 여는 대답에 실려 오지 않는 이유는 그것을
 * 알아내는 값이 헬퍼의 콜드 스타트이고, 첫 그림은 그 값을 기다릴 이유가 없기
 * 때문이다 — 판은 그동안 그리고 있고, 만질 수 있게 되는 순간 이 말이 온다. */
listen("emulator:capability", (event) => {
  const { stream, interactive } = event.payload;
  const tab = tabs.find((one) => one.kind === "emulator" && one.stream === stream);
  if (!tab || tab.interactive === interactive) return;
  tab.interactive = interactive;
  paintEmulatorView(tab);
});

listen("emulator:note", (event) => {
  const { stream, code, note } = event.payload;
  const message = emulatorNoteMessage(code, note);
  const tab = tabs.find((one) => one.kind === "emulator" && one.stream === stream);
  if (!tab) return;
  /* 기기가 꺼진 것은 스트림의 병이 아니다. 펌프는 그 자리에서 기다리다가
   * 기기가 돌아오면 같은 판에 새 스트림을 잇는다 — 여기서 비디오 복구로
   * 넘어가면 멀쩡한 스트림을 헐고 느린 녹화 길로 내려앉는다. 판은 말만
   * 바꾸고, 첫 그림이 오는 순간 `live`가 스스로 되살아난다. */
  if (code === EMULATOR_DEVICE_OFFLINE) {
    sayInEmulatorNote(tab, message);
    if (tab.live) {
      tab.live = false;
      paintEmulatorView(tab);
    }
    return;
  }
  if (tab.video) {
    void recoverEmulatorVideo(tab, stream, message);
    return;
  }
  sayInEmulatorNote(tab, message);
  if (tab.live) {
    tab.live = false;
    paintEmulatorView(tab);
  }
});

/* 팔레트의 에뮬레이터 문(1-g28): 스트림을 열고 그 거울을 탭으로 세운다 —
 * Orca처럼 창 '안'이다 (라이브 보고 2026-08-15 "orca는 창안에뜸"). */
let mobileEmulatorList = [];

/* 두 플랫폼을 한 목록으로: 선택기가 iOS 시뮬레이터와 Android AVD를 함께
 * 든다. value는 `<platform>:<id>` — id는 udid(iOS)이거나 AVD 이름(Android). */
async function refreshEmulatorFleet() {
  const [ios, android] = await Promise.all([
    invoke("mobile_emulators", {}).catch(() => []),
    invoke("android_emulators", {}).catch(() => []),
  ]);
  mobileEmulatorList = [
    ...(Array.isArray(ios) ? ios : []).map((device) => ({
      platform: "ios",
      id: device.udid,
      name: device.name,
    })),
    ...(Array.isArray(android) ? android : []).map((device) => ({
      platform: "android",
      id: device.avd,
      name: device.avd,
    })),
  ];
  return mobileEmulatorList;
}

/* 이 창이 주는 에뮬레이터 탭 이름. 스트림이 아니라 창이 이름을 주는 이유는
 * 스트림이 기기를 갈아탈 때마다 바뀌기 때문이다 — 스트림을 찾는 자리는 전부
 * `tab.stream`을 보므로 이름에서 스트림을 읽어 가는 손은 없다. */
let emulatorTabSeq = 0;

/* 새 에뮬레이터 탭: 판을 먼저 세우고, 붙는 일은 기기 갈아타기와 같은 문으로
 * 보낸다.
 *
 * 판을 먼저 세우는 이유는 원본이 그 이유를 자기 주석에 적어 두었다 —
 * `open-mobile-emulator-tab.ts:88`의 동기 마운트가 `:100`의 attach await보다
 * 앞이고, 주석은 "the pane is visible but inert while serve-sim settles"라고
 * 말한다. 기다린 뒤에 세우면 그 사이 사람은 빈 무대를 본다(실측 609ms).
 *
 * 붙는 절차 — 이진 문 만들기·스트림 시작·이미 붙어 있던 스트림을 그 탭에
 * 넘겨주기·이름표 갈아입기·H264 터널로 갈아타기 — 는 `switchEmulatorDevice`가
 * 이미 전부 들고 있다. 여기서 다시 쓰지 않는다. */
async function openEmulatorTab(platform = "ios", device, { from = null, agent = false } = {}) {
  // 에이전트가 연 거울은 부른 판의 체크아웃에 선다(t-6379): 사람이 그
  // 체크아웃을 보고 있으면 부른 판 옆에, 다른 곳을 보고 있으면 부른 판의
  // 그룹에 탭으로만 서고 체크아웃·탭·초점·나눔은 사람의 것 그대로다(09-23
  // 14:14·14:34·14:49, 워커의 거울 셋이 사람이 보던 dl 스테이지에 섰다).
  // 부른 판을 모르면 — 판 키 없는 셸, 이 창에 없는 판 — 지금처럼 초점 옆에
  // 세우고 그렇게 됐다고 한 줄 말한다. 팔레트는 지금 그대로다.
  const caller = from === null ? null : tabOfTerm(from);
  const away = caller !== null && caller.worktree !== activeWorktreePath;
  if (agent && caller === null) {
    toast(t(
      "emulator.agentOpenUnseated",
      "에이전트가 에뮬레이터를 열었지만 요청한 터미널을 알 수 없어, 지금 보고 있는 화면 옆에 열었습니다.",
    ));
  }
  const stood = away ? null : caller !== null ? standBeside(caller.pane) : standBesideFocused();
  if (stood !== null) optimizeRichStageGroup(stood);
  emulatorTabSeq += 1;
  const id = `emulator:${emulatorTabSeq}`;
  openTab({
    id,
    kind: "emulator",
    platform,
    deviceId: device ?? null,
    interactive: false,
    working: true,
    live: false,
    emulatorEpoch: 0,
    ...(away ? { worktree: caller.worktree, pane: caller.pane } : {}),
    ...(stood === null ? {} : { pane: stood }),
    // 누구의 거울인가(t-6336): 그 판의 일이 끝나 기기가 반납되면 이 탭이 걷힌다.
    ...(caller === null ? {} : { borrower: from }),
  }, { focus: !away });
  if (stood !== null) persistStageLayouts();
  const tab = tabs.find((one) => one.id === id);
  if (!tab) return;
  // 기기 목록은 첫 그림과 인과가 없다 — 고르는 상자를 채우려는 것뿐이므로
  // 붙는 길을 막지 않고 따로 채운다.
  void refreshEmulatorFleet()
    .then(() => {
      if (tabs.includes(tab) && stillShowing(tab)) paintEmulatorView(tab);
    })
    .catch(() => {});
  await switchEmulatorDevice(tab, platform, device, {
    focus: !away,
    borrower: caller === null ? null : from,
  });
}

/* A Computer Use agent opens the exact same in-window surface as the palette.
 * Revalidate the event at the frontend boundary: emitted payloads are data,
 * never instructions or a model-specific device shortcut. The backend fleet
 * lookup decides which installed device is available when `device` is absent.
 * `term` is the pane whose door asked (t-6379): the mirror is seated in that
 * pane's checkout, and only a whole number names one. */
listen("emulator:agent-open", (event) => {
  if (isPopout) return;
  const payload = event?.payload ?? {};
  const platform = payload.platform;
  if (platform !== "ios" && platform !== "android") return;
  const candidate = typeof payload.device === "string" ? payload.device : "";
  const device = candidate.trim() && candidate.length <= 512 ? candidate : undefined;
  const from = Number.isInteger(payload.term) && payload.term >= 0 ? payload.term : null;
  void openEmulatorTab(platform, device, { from, agent: true });
});

/* A device an agent borrowed was returned (t-6336): its borrower's work
 * ended, and the backend has already stopped its streams and put it down.
 * The mirrors that agent opened of it go with it — a mirror of a device that
 * is off stood for three hours on 09-23 (14:49–18:00). A mirror the person
 * opened is theirs and stays. */
listen("emulator:loan-returned", (event) => {
  if (isPopout) return;
  const { platform, device } = event?.payload ?? {};
  if (typeof device !== "string" || device === "") return;
  for (const tab of tabs.filter((one) => one.kind === "emulator"
    && one.borrower != null
    && one.platform === platform
    && (one.deviceId === device || one.udid === device))) {
    closeTab(tab.id);
  }
});

/* 「빌린 기기 n · 마지막 사용」 — how many devices agents' panes borrowed and
 * when one was last used (t-6336). Standing only while something is lent,
 * and read again once a minute while it stands, because a borrowed device's
 * last use moves with every door verb and nothing announces that. */
const EMULATOR_LOANS_TICK_MS = 60_000;
let emulatorLoans = { count: 0, lastUsedMs: null };

/* The loan book's one sentence, for the status bar's chip and for the task
 * board's machine strip (t-6588) — one state, one sentence, two places it
 * stands. Empty while nothing is lent. */
function emulatorLoansWords(now) {
  const { count, lastUsedMs } = emulatorLoans;
  if (count === 0) return "";
  return lastUsedMs === null || now - lastUsedMs < 60_000
    ? t("emulator.loansNow", "빌린 기기 {{count}} · 방금 사용", { count })
    : t("emulator.loans", "빌린 기기 {{count}} · 마지막 사용 {{ago}} 전", {
        count,
        ago: agoWord(lastUsedMs, now),
      });
}

function paintEmulatorLoans(summary) {
  const count = Number.isSafeInteger(summary?.count) && summary.count > 0 ? summary.count : 0;
  const lastUsedMs = Number.isFinite(summary?.lastUsedMs) ? summary.lastUsedMs : null;
  emulatorLoans = { count, lastUsedMs };
  const chip = el("sb-loans");
  chip.hidden = count === 0;
  if (count > 0) say(el("sb-loans-words"), () => emulatorLoansWords(Date.now()));
  emulatorLoansTick.sync();
}

function refreshEmulatorLoans() {
  void invoke("emulator_loans").then(paintEmulatorLoans).catch(() => {});
}

const emulatorLoansTick = idlePoller({
  wanted: () => emulatorLoans.count > 0,
  every: EMULATOR_LOANS_TICK_MS,
  tick: refreshEmulatorLoans,
  onResume: refreshEmulatorLoans,
});

listen("emulator:loans", (event) => {
  if (isPopout) return;
  paintEmulatorLoans(event?.payload);
});
if (!isPopout) refreshEmulatorLoans();

/* `zerocode-ssh open` — the shell is already connected when this arrives.
 *
 * The backend dials, because that is what lets `open` answer the agent with a
 * pane id at once; the only thing left is the seat, which is this window's to
 * give. So this mounts a shell that EXISTS rather than starting one, and a
 * payload naming a term this window never made simply finds no screen —
 * `mountTermTab` builds the view, and the backend is the only thing that can
 * feed it. Same revalidation as the emulator door: emitted payloads are data,
 * never instructions. */
listen("ssh:agent-open", (event) => {
  if (isPopout) return;
  const payload = event?.payload ?? {};
  const term = payload.term;
  if (!Number.isInteger(term) || term < 0) return;
  const label = typeof payload.label === "string" && payload.label.trim()
    ? payload.label.slice(0, 200)
    : undefined;
  const remoteHost = typeof payload.remoteHost === "string" && payload.remoteHost.trim()
    ? payload.remoteHost.slice(0, 512)
    : undefined;
  mountTermTab(term, {
    ...(label === undefined ? {} : { label }),
    ...(remoteHost === undefined ? {} : { remoteHost }),
  }, { placement: "tab" });
});

/* ---- 스크린샷 마크업 (B02 ②) --------------------------------------------
 *
 * Orca의 마크업 편집기, 실측 그대로(unsaved-close-queue-Cm9a2nZJ.js): 색 일곱,
 * 굵기 셋, 글자 다섯 단, 도구 여섯(펜·형광펜·화살표·사각형·타원·텍스트),
 * 확정 레이어 + 진행 셰이프의 두 겹 캔버스, past/future의 undo 문서. 페이지
 * 사각형은 네이티브 판의 것이므로 편집기는 판을 눕히고 그 자리에 선다 —
 * 캡처(browser_snapshot)가 먼저, 그 다음 판이 눕는다. */
const MARKUP_COLORS = ["#ef4444", "#f97316", "#eab308", "#22c55e", "#3b82f6", "#111827", "#ffffff"];
const MARKUP_WIDTHS = [2, 4, 8];
const MARKUP_FONT_SIZES = [14, 18, 24, 32, 48];
const MARKUP_DOWNSCALE_STEPS = [1, 0.85, 0.7, 0.55, 0.4, 0.3];
const MARKUP_HIGHLIGHT_ALPHA = 0.35;
const MARKUP_ARROW_HEAD_ANGLE = 0.45;
const MARKUP_TEXT_FONT =
  'ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, Helvetica, Arial, sans-serif';
/* 클립보드 한도(clipboard-image-ddN0rPTV.js): base64 24MiB자 → 원본 18MiB,
 * 그리고 32Mi 픽셀. */
const MARKUP_MAX_SOURCE_BYTES = Math.floor(((24 * 1024 * 1024) / 4) * 3);
const MARKUP_MAX_PIXELS = 32 * 1024 * 1024;

function markupNormalizeRect(from, to) {
  return {
    x: Math.min(from.x, to.x),
    y: Math.min(from.y, to.y),
    width: Math.abs(to.x - from.x),
    height: Math.abs(to.y - from.y),
  };
}

function markupHaloColor(color) {
  return color.toLowerCase() === "#ffffff" ? "rgba(0,0,0,0.65)" : "rgba(255,255,255,0.85)";
}

function markupStrokePolyline(ctx, points, width) {
  if (points.length === 0) return;
  if (points.length === 1) {
    const point = points[0];
    ctx.beginPath();
    ctx.arc(point.x, point.y, Math.max(width / 2, 1), 0, Math.PI * 2);
    ctx.fill();
    return;
  }
  ctx.beginPath();
  ctx.moveTo(points[0].x, points[0].y);
  for (let i = 1; i < points.length; i += 1) ctx.lineTo(points[i].x, points[i].y);
  ctx.stroke();
}

function markupArrowHead(from, to, width) {
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  if (dx === 0 && dy === 0) return null;
  const angle = Math.atan2(dy, dx);
  const size = Math.max(10, width * 3.5);
  const leftAngle = angle + Math.PI - MARKUP_ARROW_HEAD_ANGLE;
  const rightAngle = angle + Math.PI + MARKUP_ARROW_HEAD_ANGLE;
  return {
    left: { x: to.x + size * Math.cos(leftAngle), y: to.y + size * Math.sin(leftAngle) },
    right: { x: to.x + size * Math.cos(rightAngle), y: to.y + size * Math.sin(rightAngle) },
  };
}

function markupDrawShape(ctx, shape) {
  ctx.save();
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  ctx.strokeStyle = shape.color;
  ctx.fillStyle = shape.color;
  if (shape.kind === "pen") {
    ctx.lineWidth = shape.width;
    markupStrokePolyline(ctx, shape.points, shape.width);
  } else if (shape.kind === "highlight") {
    ctx.globalAlpha = MARKUP_HIGHLIGHT_ALPHA;
    const width = shape.width * 4;
    ctx.lineWidth = width;
    markupStrokePolyline(ctx, shape.points, width);
  } else if (shape.kind === "arrow") {
    ctx.lineWidth = shape.width;
    ctx.beginPath();
    ctx.moveTo(shape.from.x, shape.from.y);
    ctx.lineTo(shape.to.x, shape.to.y);
    ctx.stroke();
    const head = markupArrowHead(shape.from, shape.to, shape.width);
    if (head) {
      ctx.beginPath();
      ctx.moveTo(head.left.x, head.left.y);
      ctx.lineTo(shape.to.x, shape.to.y);
      ctx.lineTo(head.right.x, head.right.y);
      ctx.stroke();
    }
  } else if (shape.kind === "rect") {
    ctx.lineWidth = shape.width;
    const rect = markupNormalizeRect(shape.from, shape.to);
    ctx.strokeRect(rect.x, rect.y, rect.width, rect.height);
  } else if (shape.kind === "ellipse") {
    ctx.lineWidth = shape.width;
    const rect = markupNormalizeRect(shape.from, shape.to);
    ctx.beginPath();
    ctx.ellipse(
      rect.x + rect.width / 2,
      rect.y + rect.height / 2,
      rect.width / 2,
      rect.height / 2,
      0,
      0,
      Math.PI * 2,
    );
    ctx.stroke();
  } else if (shape.kind === "text") {
    ctx.font = `600 ${shape.fontSize}px ${MARKUP_TEXT_FONT}`;
    ctx.textBaseline = "top";
    ctx.lineWidth = Math.max(shape.fontSize / 6, 2);
    ctx.strokeStyle = markupHaloColor(shape.color);
    ctx.strokeText(shape.text, shape.at.x, shape.at.y);
    ctx.fillText(shape.text, shape.at.x, shape.at.y);
  }
  ctx.restore();
}

function markupScaleShape(shape, scale) {
  const point = (one) => ({ x: one.x * scale, y: one.y * scale });
  if (shape.kind === "pen" || shape.kind === "highlight") {
    return { ...shape, width: shape.width * scale, points: shape.points.map(point) };
  }
  if (shape.kind === "text") {
    return { ...shape, fontSize: shape.fontSize * scale, at: point(shape.at) };
  }
  return { ...shape, width: shape.width * scale, from: point(shape.from), to: point(shape.to) };
}

/* Orca의 undo 문서: 셰이프 배열의 스냅숏이 past로, redo가 future로. */
function markupCommit(held, shape) {
  held.past.push(held.shapes.slice());
  held.future.length = 0;
  held.shapes.push(shape);
}

function markupSetShapes(held, shapes) {
  held.past.push(held.shapes.slice());
  held.future.length = 0;
  held.shapes = shapes;
}

function markupUndo(held) {
  const prev = held.past.pop();
  if (prev === undefined) return;
  held.future.unshift(held.shapes);
  held.shapes = prev;
}

function markupRedo(held) {
  const next = held.future.shift();
  if (next === undefined) return;
  held.past.push(held.shapes);
  held.shapes = next;
}

/* 캡처 → 편집기. 실패는 토스트로 말하고 아무것도 남기지 않는다(실측 문장). */
async function startBrowserMarkup(tab) {
  if (tab.marking) return;
  const host = groupOf(tab.pane)?.browserView;
  const body = host?.querySelector(".browser-body");
  const rect = body?.getBoundingClientRect();
  if (!rect || rect.width <= 0 || rect.height <= 0) {
    toast(t("browser.markup.unavailable", "이 페이지에서는 스크린샷 마크업을 사용할 수 없습니다"), "halt");
    return;
  }
  let base;
  try {
    base = await invoke("browser_snapshot", { label: tab.label });
  } catch {
    toast(t("browser.markup.captureFailed", "그릴 페이지를 캡처하지 못했습니다"), "halt");
    return;
  }
  tab.marking = {
    base,
    cssWidth: rect.width,
    cssHeight: rect.height,
    outputScale: window.devicePixelRatio || 1,
    shapes: [],
    past: [],
    future: [],
    inProgress: null,
    tool: "pen",
    color: MARKUP_COLORS[0],
    width: 4,
    fontSize: 18,
    pendingText: null,
    busy: false,
    node: null,
  };
  if (stillShowing(tab)) paintBrowserView(tab);
  syncBrowserPanes();
}

function closeBrowserMarkup(tab) {
  if (!tab.marking) return;
  tab.marking = null;
  if (stillShowing(tab)) paintBrowserView(tab);
  syncBrowserPanes();
}

/* 확정 레이어에 shapes를, 화면 캔버스에 확정+진행을 — 실측 renderCommittedLayer
 * / blitMarkupScene의 결. DPR은 [1,4]로 클램프(clampMarkupScale). */
function markupRepaint(held) {
  const canvas = held.node?.querySelector(".browser-markup-canvas");
  if (!canvas) return;
  const scale = Math.min(Math.max(window.devicePixelRatio || 1, 1), 4);
  const width = Math.max(1, Math.round(held.cssWidth * scale));
  const height = Math.max(1, Math.round(held.cssHeight * scale));
  if (held.layer.width !== width || held.layer.height !== height) {
    held.layer.width = width;
    held.layer.height = height;
    held.layerDirty = true;
  }
  if (held.layerDirty) {
    const layerCtx = held.layer.getContext("2d");
    layerCtx.setTransform(scale, 0, 0, scale, 0, 0);
    layerCtx.clearRect(0, 0, held.cssWidth, held.cssHeight);
    for (const shape of held.shapes) markupDrawShape(layerCtx, shape);
    held.layerDirty = false;
  }
  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width;
    canvas.height = height;
  }
  const ctx = canvas.getContext("2d");
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.clearRect(0, 0, width, height);
  if (held.layer.width > 0) ctx.drawImage(held.layer, 0, 0, width, height);
  ctx.setTransform(scale, 0, 0, scale, 0, 0);
  if (held.inProgress) markupDrawShape(ctx, held.inProgress);
  const undo = held.node.querySelector(".browser-markup-undo");
  const redo = held.node.querySelector(".browser-markup-redo");
  const wipe = held.node.querySelector(".browser-markup-clear");
  undo.disabled = held.past.length === 0;
  redo.disabled = held.future.length === 0;
  wipe.disabled = held.past.length === 0 && held.future.length === 0;
  const dot = held.node.querySelector(".browser-markup-dot");
  dot.style.backgroundColor = held.color;
  const sizeSaid = held.node.querySelector(".browser-markup-fontsize-said");
  sizeSaid.textContent = String(held.fontSize);
  for (const one of held.node.querySelectorAll(".browser-markup-tool")) {
    one.setAttribute("aria-pressed", one.dataset.tool === held.tool ? "true" : "false");
  }
  for (const one of held.node.querySelectorAll(".browser-markup-swatch")) {
    one.setAttribute("aria-pressed", one.dataset.color === held.color ? "true" : "false");
  }
  for (const one of held.node.querySelectorAll(".browser-markup-width")) {
    one.setAttribute("aria-pressed", Number(one.dataset.width) === held.width ? "true" : "false");
  }
  for (const one of held.node.querySelectorAll(".browser-markup-size")) {
    one.setAttribute(
      "aria-pressed",
      Number(one.dataset.size) === held.fontSize ? "true" : "false",
    );
  }
}

/* 진행 중 텍스트: 캔버스 위 그 자리의 input — Enter가 확정, Escape가 물린다
 * (실측). 빈(공백뿐인) 글은 셰이프가 되지 않는다. */
function markupCommitPendingText(tab, value) {
  const held = tab.marking;
  if (!held?.pendingText) return;
  const at = { x: held.pendingText.x, y: held.pendingText.y };
  held.pendingText = null;
  held.node.querySelector(".browser-markup-text")?.remove();
  const trimmed = value.trim();
  if (trimmed.length > 0) {
    markupCommit(held, {
      kind: "text",
      color: held.color,
      at,
      text: trimmed,
      fontSize: held.fontSize,
    });
    held.layerDirty = true;
  }
  markupRepaint(held);
}

function markupPlacePendingText(tab, point) {
  const held = tab.marking;
  if (held.pendingText) return;
  held.pendingText = { x: point.x, y: point.y };
  const field = document.createElement("input");
  field.className = "browser-markup-text";
  field.type = "text";
  field.spellcheck = false;
  field.setAttribute("aria-label", t("browser.markup.textInput", "주석 텍스트"));
  field.style.left = `${point.x}px`;
  field.style.top = `${point.y}px`;
  field.style.fontSize = `${held.fontSize}px`;
  field.style.color = held.color;
  field.style.textShadow =
    held.color.toLowerCase() === "#ffffff"
      ? "0 0 3px rgba(0,0,0,0.7)"
      : "0 0 3px rgba(255,255,255,0.9), 0 0 2px rgba(255,255,255,0.9)";
  field.addEventListener("pointerdown", (event) => event.stopPropagation());
  field.addEventListener("keydown", (event) => {
    event.stopPropagation();
    if (event.key === "Enter" && !event.isComposing) {
      event.preventDefault();
      markupCommitPendingText(tab, field.value);
    } else if (event.key === "Escape") {
      event.preventDefault();
      held.pendingText = null;
      field.remove();
    }
  });
  field.addEventListener("blur", () => {
    if (tab.marking?.pendingText) markupCommitPendingText(tab, field.value);
  });
  held.node.querySelector(".browser-markup-stage").appendChild(field);
  requestAnimationFrame(() => field.focus());
}

function buildMarkupEditor(tab) {
  const held = tab.marking;
  const root = document.createElement("div");
  root.className = "browser-markup-editor";
  held.node = root;
  held.layer = document.createElement("canvas");
  held.layerDirty = true;

  const stage = document.createElement("div");
  stage.className = "browser-markup-stage";
  const image = document.createElement("img");
  image.className = "browser-markup-img";
  image.alt = "";
  image.draggable = false;
  image.src = held.base;
  const canvas = document.createElement("canvas");
  canvas.className = "browser-markup-canvas";
  stage.append(image, canvas);
  root.appendChild(stage);

  const pointAt = (event) => {
    const box = canvas.getBoundingClientRect();
    return { x: event.clientX - box.left, y: event.clientY - box.top };
  };
  canvas.addEventListener("pointerdown", (event) => {
    if (held.busy || event.button !== 0) return;
    const point = pointAt(event);
    if (held.tool === "text") {
      event.preventDefault();
      markupPlacePendingText(tab, point);
      return;
    }
    canvas.setPointerCapture(event.pointerId);
    held.inProgress =
      held.tool === "pen" || held.tool === "highlight"
        ? { kind: held.tool, color: held.color, width: held.width, points: [point] }
        : { kind: held.tool, color: held.color, width: held.width, from: point, to: point };
    markupRepaint(held);
  });
  canvas.addEventListener("pointermove", (event) => {
    const going = held.inProgress;
    if (!going) return;
    const point = pointAt(event);
    if (going.kind === "pen" || going.kind === "highlight") going.points.push(point);
    else going.to = point;
    markupRepaint(held);
  });
  const settle = () => {
    if (!held.inProgress) return;
    markupCommit(held, held.inProgress);
    held.inProgress = null;
    held.layerDirty = true;
    markupRepaint(held);
  };
  canvas.addEventListener("pointerup", settle);
  canvas.addEventListener("pointercancel", settle);

  // ---- 아래 가운데의 두 줄: 도구 막대, 그리고 확인 줄(실측 배치) ----
  const dock = document.createElement("div");
  dock.className = "browser-markup-dock";
  const bar = document.createElement("div");
  bar.className = "browser-markup-bar";
  const tools = [
    ["pen", "pencil", t("browser.markup.pen", "펜")],
    ["highlight", "highlighter", t("browser.markup.highlight", "형광펜")],
    ["arrow", "arrow-up-right", t("browser.markup.arrow", "화살표")],
    ["rect", "square", t("browser.markup.rect", "사각형")],
    ["ellipse", "circle", t("browser.markup.ellipse", "타원")],
    ["text", "type", t("browser.markup.text", "텍스트")],
  ];
  for (const [kind, glyph, label] of tools) {
    const one = document.createElement("button");
    one.type = "button";
    one.className = "browser-markup-tool";
    one.dataset.tool = kind;
    one.dataset.tip = label;
    one.setAttribute("aria-label", label);
    one.innerHTML = icon(glyph);
    one.addEventListener("click", () => {
      held.tool = kind;
      markupRepaint(held);
    });
    bar.appendChild(one);
  }
  const sep = () => {
    const line = document.createElement("span");
    line.className = "browser-markup-sep";
    return line;
  };
  bar.appendChild(sep());
  // 색·두께 팝오버(실측: 스와치 일곱 + 두께 셋)와 글자 크기 팝오버(다섯 단).
  const styleWrap = document.createElement("span");
  styleWrap.className = "browser-markup-pop-wrap";
  const styleDoor = document.createElement("button");
  styleDoor.type = "button";
  styleDoor.className = "browser-markup-tool browser-markup-style";
  styleDoor.dataset.tip = t("browser.markup.style", "색상 및 두께");
  styleDoor.setAttribute("aria-label", t("browser.markup.style", "색상 및 두께"));
  styleDoor.innerHTML = '<span class="browser-markup-dot"></span>';
  const stylePop = document.createElement("div");
  stylePop.className = "browser-markup-pop";
  stylePop.hidden = true;
  const swatches = document.createElement("div");
  swatches.className = "browser-markup-swatches";
  for (const color of MARKUP_COLORS) {
    const one = document.createElement("button");
    one.type = "button";
    one.className = "browser-markup-swatch";
    one.dataset.color = color;
    one.setAttribute("aria-label", color);
    one.style.backgroundColor = color;
    one.addEventListener("click", () => {
      held.color = color;
      markupRepaint(held);
    });
    swatches.appendChild(one);
  }
  const widths = document.createElement("div");
  widths.className = "browser-markup-widths";
  for (const width of MARKUP_WIDTHS) {
    const one = document.createElement("button");
    one.type = "button";
    one.className = "browser-markup-width";
    one.dataset.width = String(width);
    one.setAttribute("aria-label", t("browser.markup.widthOption", "{{n}} px", { n: width }));
    one.innerHTML = `<span style="width:${width + 2}px;height:${width + 2}px"></span>`;
    one.addEventListener("click", () => {
      held.width = width;
      markupRepaint(held);
    });
    widths.appendChild(one);
  }
  stylePop.append(swatches, widths);
  styleDoor.addEventListener("click", () => {
    stylePop.hidden = !stylePop.hidden;
    sizePop.hidden = true;
  });
  styleWrap.append(styleDoor, stylePop);
  bar.appendChild(styleWrap);
  const sizeWrap = document.createElement("span");
  sizeWrap.className = "browser-markup-pop-wrap";
  const sizeDoor = document.createElement("button");
  sizeDoor.type = "button";
  sizeDoor.className = "browser-markup-tool browser-markup-fontsize";
  sizeDoor.dataset.tip = t("browser.markup.fontSize", "글자 크기");
  sizeDoor.setAttribute("aria-label", t("browser.markup.fontSize", "글자 크기"));
  sizeDoor.innerHTML = `${icon("type")}<span class="browser-markup-fontsize-said"></span>`;
  const sizePop = document.createElement("div");
  sizePop.className = "browser-markup-pop";
  sizePop.hidden = true;
  const sizes = document.createElement("div");
  sizes.className = "browser-markup-sizes";
  for (const size of MARKUP_FONT_SIZES) {
    const one = document.createElement("button");
    one.type = "button";
    one.className = "browser-markup-size";
    one.dataset.size = String(size);
    one.setAttribute("aria-label", t("browser.markup.widthOption", "{{n}} px", { n: size }));
    one.textContent = String(size);
    one.addEventListener("click", () => {
      held.fontSize = size;
      markupRepaint(held);
    });
    sizes.appendChild(one);
  }
  sizePop.appendChild(sizes);
  sizeDoor.addEventListener("click", () => {
    sizePop.hidden = !sizePop.hidden;
    stylePop.hidden = true;
  });
  sizeWrap.append(sizeDoor, sizePop);
  bar.appendChild(sizeWrap);
  bar.appendChild(sep());
  const undo = document.createElement("button");
  undo.type = "button";
  undo.className = "browser-markup-tool browser-markup-undo";
  undo.dataset.tip = t("browser.markup.undo", "실행 취소");
  undo.setAttribute("aria-label", t("browser.markup.undo", "실행 취소"));
  undo.innerHTML = icon("undo-2");
  undo.addEventListener("click", () => {
    markupUndo(held);
    held.layerDirty = true;
    markupRepaint(held);
  });
  const redo = document.createElement("button");
  redo.type = "button";
  redo.className = "browser-markup-tool browser-markup-redo";
  redo.dataset.tip = t("browser.markup.redo", "다시 실행");
  redo.setAttribute("aria-label", t("browser.markup.redo", "다시 실행"));
  redo.innerHTML = icon("redo-2");
  redo.addEventListener("click", () => {
    markupRedo(held);
    held.layerDirty = true;
    markupRepaint(held);
  });
  const wipe = document.createElement("button");
  wipe.type = "button";
  wipe.className = "browser-markup-tool browser-markup-clear";
  wipe.dataset.tip = t("browser.markup.clear", "모두 지우기");
  wipe.setAttribute("aria-label", t("browser.markup.clear", "모두 지우기"));
  wipe.innerHTML = icon("trash");
  wipe.addEventListener("click", () => {
    held.pendingText = null;
    held.node.querySelector(".browser-markup-text")?.remove();
    held.inProgress = null;
    if (held.shapes.length > 0) markupSetShapes(held, []);
    held.layerDirty = true;
    markupRepaint(held);
  });
  bar.append(undo, redo, wipe);
  dock.appendChild(bar);

  const confirm = document.createElement("div");
  confirm.className = "browser-markup-confirm";
  const hint = document.createElement("span");
  hint.className = "browser-markup-hint";
  hint.textContent = t(
    "browser.markup.hint",
    "페이지에 그린 다음 마크업을 복사해 에이전트에 붙여넣으세요.",
  );
  const cancel = document.createElement("button");
  cancel.type = "button";
  cancel.className = "browser-markup-act";
  cancel.innerHTML = `${icon("x")}<span>${t("browser.markup.cancel", "취소")}</span>`;
  cancel.addEventListener("click", () => closeBrowserMarkup(tab));
  const copy = document.createElement("button");
  copy.type = "button";
  copy.className = "browser-markup-act browser-markup-copy";
  copy.innerHTML = `${icon("check")}<span>${t("browser.markup.copy", "마크업 복사")}</span>`;
  copy.addEventListener("click", () => void copyBrowserMarkup(tab));
  confirm.append(hint, cancel, copy);
  dock.appendChild(confirm);
  root.appendChild(dock);

  markupRepaint(held);
  return root;
}

/* 합성과 배달(실측 composeMarkupDataUrl): 바탕을 cssW×scale로 눕히고 셰이프를
 * 같은 자로 얹은 뒤, 한도(원본 18MiB·32Mi픽셀)에 들 때까지 사다리를 내려간다.
 * 클립보드에는 픽셀(RGBA)로 — PNG 인코딩은 한도 셈에만 쓴다. */
function markupDataUrlBytes(dataUrl) {
  const comma = dataUrl.indexOf(",");
  if (comma === -1) return 0;
  const payload = dataUrl.slice(comma + 1);
  const padding = payload.endsWith("==") ? 2 : payload.endsWith("=") ? 1 : 0;
  return Math.max(0, Math.floor((payload.length * 3) / 4) - padding);
}

/* 한도 셈에 쓰는 PNG 크기 — blob이 제 크기를 이미 아니 data URL을 왕복할
 * 이유가 없다(Orca는 dataUrl 길이로 셌다 — 값은 같고 길은 짧다). */
function markupPngBytes(canvas) {
  return new Promise((resolve) => {
    if (typeof canvas.toBlob === "function") {
      canvas.toBlob((blob) => {
        if (blob) resolve(blob.size);
        else resolve(markupDataUrlBytes(canvas.toDataURL("image/png")));
      }, "image/png");
      return;
    }
    resolve(markupDataUrlBytes(canvas.toDataURL("image/png")));
  });
}

function markupBase64(bytes) {
  let out = "";
  const CHUNK = 0x8000;
  for (let i = 0; i < bytes.length; i += CHUNK) {
    out += String.fromCharCode.apply(null, bytes.subarray(i, i + CHUNK));
  }
  return btoa(out);
}

async function copyBrowserMarkup(tab) {
  const held = tab.marking;
  if (!held || held.busy) return;
  const image = held.node?.querySelector(".browser-markup-img");
  if (!image || !image.complete) return;
  held.busy = true;
  held.node.querySelector(".browser-markup-copy").disabled = true;
  try {
    const area = held.cssWidth * held.cssHeight;
    const wanted = Math.min(Math.max(held.outputScale, 1), 4);
    const scale = area > 0 ? Math.min(wanted, Math.sqrt(MARKUP_MAX_PIXELS / area)) : wanted;
    const composite = document.createElement("canvas");
    composite.width = Math.max(1, Math.floor(held.cssWidth * scale));
    composite.height = Math.max(1, Math.floor(held.cssHeight * scale));
    const ctx = composite.getContext("2d");
    ctx.drawImage(image, 0, 0, composite.width, composite.height);
    for (const shape of held.shapes) markupDrawShape(ctx, markupScaleShape(shape, scale));
    let chosen = null;
    for (const step of MARKUP_DOWNSCALE_STEPS) {
      let canvas = composite;
      if (step < 1) {
        canvas = document.createElement("canvas");
        canvas.width = Math.max(1, Math.round(composite.width * step));
        canvas.height = Math.max(1, Math.round(composite.height * step));
        canvas.getContext("2d").drawImage(composite, 0, 0, canvas.width, canvas.height);
      }
      const bytes = await markupPngBytes(canvas);
      chosen = canvas;
      if (bytes <= MARKUP_MAX_SOURCE_BYTES) break;
    }
    const pixels = chosen
      .getContext("2d")
      .getImageData(0, 0, chosen.width, chosen.height);
    await invoke("set_clipboard_image", {
      rgba: markupBase64(new Uint8Array(pixels.data.buffer)),
      width: chosen.width,
      height: chosen.height,
    });
    toast(
      t("browser.markup.copied", "마크업을 복사했습니다 — 에이전트에 붙여넣으세요 ({{keys}})", {
        keys: "⌘V",
      }),
    );
    closeBrowserMarkup(tab);
  } catch (error) {
    toast(t("browser.markup.attachFailed", "마크업 스크린샷을 첨부하지 못했습니다"), "halt");
    held.busy = false;
    const door = held.node?.querySelector(".browser-markup-copy");
    if (door) door.disabled = false;
  }
}

/* One download's line: the mark for its state, the filename with the state
 * word and the origin, then the hands — 열기/표시 only once it is a file,
 * and 닫기 always (Orca's row; no cancel — the hook gives no handle). */
function browserDownloadRow(tab, row) {
  const line = document.createElement("div");
  line.className = "browser-dl";
  line.dataset.state = row.state;
  const glyph =
    row.state === "finished" ? "circle-check" : row.state === "failed" ? "octagon-x" : "download";
  line.innerHTML = icon(glyph);
  const words = document.createElement("span");
  words.className = "browser-dl-words";
  let origin = "";
  try {
    origin = new URL(row.url).host;
  } catch {
    /* a URL the page invented — the filename still names the row */
  }
  const state =
    row.state === "finished"
      ? t("browser.dl.done", "다운로드 완료")
      : row.state === "failed"
        ? t("browser.dl.failed", "다운로드 실패")
        : t("browser.dl.going", "다운로드 중");
  words.textContent = origin ? `${row.file} — ${state} · ${origin}` : `${row.file} — ${state}`;
  line.appendChild(words);
  const acts = document.createElement("div");
  acts.className = "browser-dl-acts";
  if (row.state === "finished") {
    const open = document.createElement("button");
    open.type = "button";
    open.className = "browser-dl-act";
    open.textContent = t("browser.dl.open", "열기");
    open.addEventListener("click", () => {
      invoke("open_download", { path: row.path }).catch(showError);
    });
    const show = document.createElement("button");
    show.type = "button";
    show.className = "browser-dl-act";
    show.textContent = t("browser.dl.show", "표시");
    show.addEventListener("click", () => {
      invoke("show_download", { path: row.path }).catch(showError);
    });
    acts.append(open, show);
  }
  const dismiss = document.createElement("button");
  dismiss.type = "button";
  dismiss.className = "browser-dl-act";
  dismiss.textContent = t("browser.dl.dismiss", "닫기");
  dismiss.addEventListener("click", () => {
    const rows = browserDownloads.get(tab.label) ?? [];
    browserDownloads.set(tab.label, rows.filter((one) => one !== row));
    if (stillShowing(tab)) paintBrowserView(tab);
  });
  acts.appendChild(dismiss);
  line.appendChild(acts);
  return line;
}

function paintBrowserView(tab) {
  const host = docHost(tab.pane, "browser");
  wireBrowserHost(host);
  host._browserTab = tab;
  paintBrowserFailure(host, tab);
  paintArtifactStrip(host.querySelector(".artifact-strip"), tab, (version) => {
    submitBrowserAddress(tab, pathAsFileUrl(version.path));
  });
  const reload = host.querySelector(".browser-reload");
  // The face is markup, so it is only rebuilt when it CHANGES — a paint per
  // frame rewriting identical SVG would cost layout for nothing.
  const face = tab.loading ? "loader" : "refresh";
  if (reload.dataset.face !== face) {
    reload.dataset.face = face;
    reload.innerHTML = icon(face);
    const said = tab.loading ? t("browser.stop", "중지") : t("browser.reload", "새로고침");
    reload.dataset.tip = said;
    reload.setAttribute("aria-label", said);
  }
  reload.classList.toggle("is-loading", tab.loading === true);
  const address = host.querySelector(".browser-address");
  // Never over somebody's typing: the bar being edited belongs to the person
  // until they leave it (Orca's address bar holds its own draft the same way).
  if (document.activeElement !== address) {
    address.value = tab.url === BROWSER_BLANK_URL ? "" : (tab.url ?? "");
  }
  host.querySelector(".browser-external").disabled = externalBrowserUrl(tab.url) === null;
  // A blank page has nothing to grab; Orca greys its crosshair the same way.
  const grab = host.querySelector(".browser-grab");
  const blank = !tab.url || tab.url === BROWSER_BLANK_URL;
  grab.disabled = blank;
  grab.classList.toggle(
    "is-armed",
    browserGrab !== null && browserGrab.label === tab.label && browserGrab.mode === "copy",
  );
  const annotate = host.querySelector(".browser-annotate");
  annotate.disabled = blank;
  annotate.classList.toggle(
    "is-armed",
    browserGrab !== null && browserGrab.label === tab.label && browserGrab.mode === "annotate",
  );
  const held = browserAnnotations.get(tab.label) ?? [];
  const badge = annotate.querySelector(".browser-badge");
  badge.hidden = held.length === 0;
  badge.textContent = String(held.length);
  const notes = host.querySelector(".browser-notes");
  const wasHidden = notes.hidden;
  notes.hidden = held.length === 0;
  notes.querySelector(".browser-notes-count").textContent = t(
    "browser.annotate.count",
    "주석 {{n}}개",
    { n: held.length },
  );
  // 무장한 판의 배너: 모드가 제 문장을 말하고, 방금의 C 복사는 잠깐 제
  // 확인을 말한다 (1-g18).
  const grabbar = host.querySelector(".browser-grabbar");
  const grabbarWas = grabbar.hidden;
  const armedHere = browserGrab !== null && browserGrab.label === tab.label;
  grabbar.hidden = !armedHere;
  if (armedHere) {
    const words = grabbar.querySelector(".browser-grabbar-words");
    if (Date.now() < browserGrabCopiedUntil) {
      words.textContent = t("browser.grab.copied", "복사되었습니다");
    } else if (browserGrab.mode === "annotate") {
      words.textContent = t(
        "browser.grab.bannerAnnotate",
        "선택한 요소에 대한 피드백을 추가합니다 — Esc 취소",
      );
    } else {
      words.textContent = t(
        "browser.grab.bannerCopy",
        "요소를 클릭해 에이전트로 보내거나, 가리킨 채 C를 눌러 복사합니다 — Esc 취소",
      );
    }
  }
  // 찾기 스트립: tab.finding이 세우고 눕힌다 — grab 배너와 같은 형제라
  // 서면 판이 스스로 내려앉는다.
  const dlbar = host.querySelector(".browser-dlbar");
  const downloadsHeld = browserDownloads.get(tab.label) ?? [];
  dlbar.replaceChildren();
  dlbar.hidden = downloadsHeld.length === 0;
  for (const row of downloadsHeld) dlbar.appendChild(browserDownloadRow(tab, row));
  // 마크업(B02 ②): 편집기가 판의 자리(.browser-body)에 선다 — 판은
  // syncBrowserPanes가 marking을 보고 눕힌다. 캔버스 상태는 세션 소유라
  // 노드는 한 번 지어 지니고 다닌다.
  const bodyBox = host.querySelector(".browser-body");
  if (tab.marking) {
    const editor = tab.marking.node ?? buildMarkupEditor(tab);
    if (editor.parentNode !== bodyBox) bodyBox.appendChild(editor);
  } else {
    bodyBox.querySelector(".browser-markup-editor")?.remove();
  }
  const findbar = host.querySelector(".browser-findbar");
  findbar.hidden = tab.finding !== true;
  const findCount = host.querySelector(".browser-find-count");
  if (!tab.findCount) {
    findCount.textContent = "";
  } else if (tab.findCount.count === 0) {
    findCount.textContent = t("browser.find.none", "일치 없음");
  } else {
    findCount.textContent = `${tab.findCount.index} / ${tab.findCount.count}`;
  }
  // 줄이 서고 눕는 것은 판의 사각형이 변하는 일 — 곧장 다시 잰다.
  if (wasHidden !== notes.hidden || grabbarWas !== grabbar.hidden) syncBrowserPanes();
  syncBrowserPanes();
}

/* ⌘F는 앞에 선 브라우저 판에서 찾기를 연다 — 네이티브 판이 키를 쥔 동안엔
 * 창이 못 듣지만, 주소창·툴바에 초점이 있을 때와 재열림 직후엔 여기로 온다.
 * 판이 쥔 상태의 확실한 문은 툴바의 찾기 버튼이다. */
document.addEventListener("keydown", (event) => {
  if (!(event.metaKey || event.ctrlKey) || event.key.toLowerCase() !== "f") return;
  if (!el("task-view").hidden) {
    const source = document.querySelector(".task-tab.is-active")?.dataset.source;
    const field = source === "github"
      ? el("task-gh-search")
      : source === "jira"
        ? el("jira-search")
        : null;
    if (field) {
      event.preventDefault();
      field.focus();
    }
    return;
  }
  const tab = currentTab();
  if (tab?.kind !== "browser" || browserCovered()) return;
  const host = groupOf(tab.pane)?.browserView;
  if (!host || host.hidden) return;
  event.preventDefault();
  if (!tab.finding) toggleBrowserFind(host, tab);
  else host.querySelector(".browser-find-input").focus();
});

/* 타이핑이 멎고 스스로 찾기까지의 사이, 그리고 질의로 쳐주는 크기의 끝 —
 * Orca의 살아있는 찾기(1-g34)가 재는 두 값. */
const BROWSER_FIND_DEBOUNCE_MS = 200;
const BROWSER_FIND_QUERY_MAX_BYTES = 2 * 1024;

/* ---- design mode: grab an element, send it to an agent (1-g5) ----
 *
 * Orca's "Grab page element" reaches its guest through an Electron preload
 * (`extractHoverPayload`, `formatGrabPayloadAsText`) and its result rides the
 * agent-send popover (`openAgentSendPopoverTargetMode`, source
 * "browser-annotations"). Our guest has NO IPC back — the same app-origin
 * isolation that keeps a visited site out of this window keeps the page from
 * pushing us a pick. So the backend arms a crosshair in the page
 * (`browser_grab_arm`), the page leaves what it captured on a global, and this
 * side POLLS for it (`browser_grab_take`) — the one road an eval-only channel
 * allows. What comes back is put into words and handed to the same send
 * picker the review notes use. */
const BROWSER_GRAB_POLL_MS = 250;
/* Refresh once just after the copy-confirmation deadline so the grab banner
 * cannot keep stale success text. */
const GRAB_REFRESH_MS = 1600;

/* One grab in flight at a time: the crosshair lives in the guest page, and a
 * second arm would stack overlays there. `label` is the pane being grabbed,
 * `timer` the poll that watches for a pick, `button` the anchor the send
 * picker opens beside. */
let browserGrab = null;

/* 판마다 모인 주석 — Orca의 `browserAnnotations`. 라벨이 키, 전송·탭 닫기가
 * 비운다. */
const browserAnnotations = new Map();

/* C 복사의 확인이 배너에 사는 마감 — 지나면 배너가 제 문장으로 돌아간다.
 * 상태로 두는 이유: 배너는 paint가 그리고, paint는 아무 때나 다시 온다. */
let browserGrabCopiedUntil = 0;

/* Orca의 뷰포트 프리셋(BROWSER_VIEWPORT_PRESETS, unsaved-close-queue:3448) —
 * 실측값 그대로. deviceScaleFactor와 모바일 UA는 Electron 에뮬레이션의
 * 몫이라 wry에는 문이 없다: 여기서는 **크기**가 전부고, 그래서 라벨도
 * 크기만 말한다 (1-g19). */
const BROWSER_VIEWPORT_PRESETS = [
  { id: "mobile-s", label: "Mobile S — 320 × 568", width: 320, height: 568 },
  { id: "mobile-m", label: "Mobile M — 375 × 667", width: 375, height: 667 },
  { id: "mobile-l", label: "Mobile L — 425 × 812", width: 425, height: 812 },
  { id: "tablet", label: "Tablet — 768 × 1024", width: 768, height: 1024 },
  { id: "laptop", label: "Laptop — 1024 × 768", width: 1024, height: 768 },
  { id: "laptop-l", label: "Laptop L — 1440 × 900", width: 1440, height: 900 },
  { id: "desktop", label: "Desktop — 1920 × 1080", width: 1920, height: 1080 },
];

/* 판을 프리셋 상자 안으로 — 가로는 중앙, 세로는 위에 붙인다(devtools의
 * 자세). 몸보다 큰 프리셋은 몸에 맞춰 줄어든다: 창보다 큰 판은 거짓말이다. */
/* An agent's `viewport <label> WxH` is a box no preset names: the tab keeps
 * it as the word itself ("500x400"), and it is placed like a preset. */
function customViewportOf(word) {
  const found = /^(\d{3,4})x(\d{3,4})$/.exec(typeof word === "string" ? word : "");
  return found ? { id: word, width: Number(found[1]), height: Number(found[2]) } : null;
}

function viewportBox(rect, presetId) {
  const preset = BROWSER_VIEWPORT_PRESETS.find((one) => one.id === presetId)
    ?? customViewportOf(presetId);
  if (!preset) return rect;
  const width = Math.min(preset.width, rect.width);
  const height = Math.min(preset.height, rect.height);
  return {
    x: rect.x + (rect.width - width) / 2,
    y: rect.y,
    width,
    height,
  };
}

function refreshGrabButtons() {
  for (const view of document.querySelectorAll(".browser-view")) {
    const tab = view._browserTab;
    if (tab) paintBrowserView(tab);
  }
}

function stopBrowserGrab(disarm) {
  if (browserGrab === null) return;
  const { label, timer } = browserGrab;
  clearTimeout(timer);
  browserGrab = null;
  // Only tell the pane to drop its crosshair when WE are standing it down; a
  // pick or a cancel already tore the overlay down on the page's side.
  if (disarm) invoke("browser_grab_disarm", { label }).catch(() => {});
  refreshGrabButtons();
}

async function pollBrowserGrab(label) {
  if (browserGrab === null || browserGrab.label !== label) return;
  let poll;
  try {
    poll = await invoke("browser_grab_take", { label });
  } catch {
    // The pane went away — closed, or navigated out from under us. Give up
    // quietly rather than poll a hole forever.
    stopBrowserGrab(false);
    return;
  }
  // The world may have moved while the eval was in flight (toggled off, or the
  // crosshair jumped to another pane).
  if (browserGrab === null || browserGrab.label !== label) return;
  if (poll.cancelled) {
    stopBrowserGrab(false);
    return;
  }
  if (poll.picked) {
    if (browserGrab.mode === "annotate") {
      // 주석은 모인다: 카드가 보탠 한 건을 쌓고, Orca처럼 크로스헤어를 다시
      // 세워 다음 요소를 기다린다. 보내는 손은 뱃지에 있다.
      const held = browserAnnotations.get(label) ?? [];
      held.push(poll.picked);
      browserAnnotations.set(label, held);
      refreshGrabButtons();
      armBrowserGrab(label, "annotate");
      return;
    }
    if (poll.picked.via === "key-c") {
      // C는 클립보드로 — 클릭 없이, 피커 없이 (Orca 배너 #46). 크로스헤어는
      // 다시 서서 다음 복사를 기다리고, 배너가 잠깐 확인을 말한다.
      void clipboardText.write(formatGrabText(poll.picked));
      browserGrabCopiedUntil = Date.now() + 1500;
      refreshGrabButtons();
      setTimeout(refreshGrabButtons, GRAB_REFRESH_MS);
      armBrowserGrab(label, "copy");
      return;
    }
    const button = browserGrab.button;
    stopBrowserGrab(false);
    void deliverGrab(button, poll.picked);
    return;
  }
  browserGrab.timer = setTimeout(() => pollBrowserGrab(label), BROWSER_GRAB_POLL_MS);
}

/* Orca's pane menu (1-g13), in this window's own menu vocabulary. The rows
 * and their order are Orca's (`BrowserPane`, unsaved-close-queue:8767): the
 * link trio when the pointer was over a link, the selection's copy when
 * there was one, then the three navigations, then the two page rows —
 * separated exactly where Orca separates them. */
function browserMenuItems(tab, said) {
  const items = [];
  const linkUrl = typeof said.linkUrl === "string" ? said.linkUrl : "";
  if (linkUrl) {
    items.push(
      {
        label: t("browser.menu.openHere", "이 브라우저에서 링크 열기"),
        run: () => openBrowserTab(linkUrl, { beside: true }),
      },
      {
        label: t("browser.menu.openOut", "기본 브라우저에서 링크 열기"),
        run: () => {
          const out = externalBrowserUrl(linkUrl);
          if (out) openExternal(out);
        },
      },
      {
        label: t("browser.menu.copyLink", "링크 주소 복사"),
        run: () => void clipboardText.write(linkUrl),
      },
      { separator: true },
    );
  }
  const selection = typeof said.selectionText === "string" ? said.selectionText.trim() : "";
  if (selection) {
    items.push(
      {
        label: t("browser.menu.copy", "복사"),
        run: () => void clipboardText.write(selection),
      },
      { separator: true },
    );
  }
  items.push(
    {
      label: t("browser.back", "뒤로"),
      run: () => invoke("browser_history", { label: tab.label, delta: -1 }),
    },
    {
      label: t("browser.forward", "앞으로"),
      run: () => invoke("browser_history", { label: tab.label, delta: 1 }),
    },
    {
      label: t("browser.reload", "새로고침"),
      run: () => invoke("browser_reload", { label: tab.label }),
    },
    { separator: true },
    {
      label: t("browser.menu.openPageOut", "기본 브라우저에서 페이지 열기"),
      run: () => {
        const out = externalBrowserUrl(tab.url);
        if (out) openExternal(out);
      },
    },
    {
      label: t("browser.menu.copyPage", "페이지 주소 복사"),
      run: () => void clipboardText.write(tab.url ?? ""),
    },
  );
  return items;
}

/* The window asks the page whether it was right-clicked, and only while a
 * pane is actually in front: a native pane's clicks never reach this DOM, so
 * asking is the only way to hear one, and asking a hidden pane would be a
 * heartbeat nobody is listening to. */
const BROWSER_MENU_POLL_MS = 300;
const browserMenuPoll = idlePoller({
  wanted: () => currentTab()?.kind === "browser",
  every: BROWSER_MENU_POLL_MS,
  tick: takeBrowserMenu,
});

function watchBrowserMenu() {
  browserMenuPoll.sync();
}

function takeBrowserMenu() {
  const tab = currentTab();
  if (tab?.kind !== "browser" || browserCovered() || !sidebarMenu.hidden) return;
  invoke("browser_menu_take", { label: tab.label })
    .then((said) => {
      if (!said) return;
      // The page's coordinates are the BODY's; the menu lives in the
      // window, so the pane's own corner is the offset.
      const view = [...document.querySelectorAll(".browser-view")].find(
        (one) => one._browserTab === tab,
      );
      const body = view?.querySelector(".browser-body");
      const box = body?.getBoundingClientRect();
      openSidebarMenu(
        (box?.left ?? 0) + (said.x ?? 0),
        (box?.top ?? 0) + (said.y ?? 0),
        browserMenuItems(tab, said),
      );
    })
    .catch(() => {});
}

/* 페이지 내 찾기(1-g32·1-g34). tab.finding이 스트립의 생사를 쥐고, 명령은 판에
 * window.find를 주입한다(백엔드가 질의를 JSON 문자열로 이스케이프). 질의는
 * 살아 있다(1-g34) — 타이핑이 멎고 200ms 뒤 스스로 감기고, 비면 기다리지 않고
 * 그 자리에서 걷힌다. */
function toggleBrowserFind(host, tab) {
  if (tab.finding) {
    closeBrowserFind(host, tab);
    return;
  }
  tab.finding = true;
  if (stillShowing(tab)) paintBrowserView(tab);
  const input = host.querySelector(".browser-find-input");
  input.value = tab.findQuery ?? "";
  input.focus();
  input.select();
  // 다시 열리면 기억한 질의를 그 자리에서 다시 감는다 — Orca의 재열림.
  const query = findableBrowserQuery(input.value);
  if (query && query !== tab.findLast) void runBrowserFind(tab, query, true);
}

/* 질의의 공용 경계(1-g34·1-g38): 비었거나 maxBytes(UTF-8)를 넘으면 질의가
 * 아니다. 바이트로 재는 이유는 UTF-16 단위가 사람이 친 글자의 크기가 아니어서고,
 * 이 창에서 질의를 받는 상자가 둘이 된 뒤로 그 자를 두 벌 두지 않는다. */
function boundedQuery(raw, maxBytes) {
  const query = raw ?? "";
  if (!query) return "";
  return new TextEncoder().encode(query).length > maxBytes ? "" : query;
}

/* 질의의 경계: 빈 것과 2KB(UTF-8 바이트)를 넘는 것은 질의가 아니다.
 * 그 밖에는 공백까지 그대로 — Orca는 질의를 다듬지 않는다. */
function findableBrowserQuery(raw) {
  return boundedQuery(raw, BROWSER_FIND_QUERY_MAX_BYTES);
}

/* 타이핑의 문: 타이핑이 멎고 200ms 뒤 스스로 찾는다. 이미 감긴 질의면
 * 잠자코, 비거나 과대한 질의는 기다리지 않고 즉시 걷는다. */
function scheduleBrowserFind(tab, raw) {
  if (tab.findTimer) {
    clearTimeout(tab.findTimer);
    tab.findTimer = null;
  }
  tab.findQuery = raw ?? "";
  const query = findableBrowserQuery(raw);
  if (!query) {
    dropBrowserFindMarks(tab);
    return;
  }
  if (query === tab.findLast) return;
  tab.findTimer = setTimeout(() => {
    tab.findTimer = null;
    void runBrowserFind(tab, query, true);
  }, BROWSER_FIND_DEBOUNCE_MS);
}

/* Enter·버튼의 문: 미룬 타이핑 찾기를 접고 그 자리에서 찾는다 — 같은
 * 질의면 다음·이전으로 돌고, 새 질의면 처음부터 감는다. */
function pressBrowserFind(tab, raw, forward) {
  if (tab.findTimer) {
    clearTimeout(tab.findTimer);
    tab.findTimer = null;
  }
  void runBrowserFind(tab, raw, forward);
}

async function runBrowserFind(tab, query, forward) {
  const needle = findableBrowserQuery(query);
  tab.findQuery = query ?? "";
  if (!needle) {
    dropBrowserFindMarks(tab);
    return;
  }
  try {
    const hit = await invoke("browser_find", {
      label: tab.label,
      query: needle,
      forward,
      matchCase: false,
    });
    tab.findLast = needle;
    tab.findCount = hit ? { count: hit.count ?? 0, index: hit.index ?? 0 } : { count: 0, index: 0 };
  } catch (error) {
    showError(String(error));
    return;
  }
  if (stillShowing(tab)) paintBrowserView(tab);
}

/* 걷기: 하이라이트·개수·마지막 질의를 함께 지운다 — 걷을 것이 있을 때만
 * 판을 부른다. */
function dropBrowserFindMarks(tab) {
  const hadMarks = tab.findLast != null || tab.findCount != null;
  tab.findLast = null;
  tab.findCount = null;
  if (hadMarks) void invoke("browser_find_clear", { label: tab.label }).catch(() => {});
  if (stillShowing(tab)) paintBrowserView(tab);
}

function closeBrowserFind(host, tab) {
  if (!tab.finding) return;
  tab.finding = false;
  if (tab.findTimer) {
    clearTimeout(tab.findTimer);
    tab.findTimer = null;
  }
  tab.findLast = null;
  tab.findCount = null;
  void invoke("browser_find_clear", { label: tab.label }).catch(() => {});
  if (stillShowing(tab)) paintBrowserView(tab);
  keySink.focus();
}

function toggleBrowserGrab(host, mode = "copy") {
  const tab = host._browserTab;
  if (!tab) return;
  // A second click on the armed pane's SAME mode stands the crosshair down;
  // the other mode takes the crosshair over.
  if (browserGrab !== null && browserGrab.label === tab.label && browserGrab.mode === mode) {
    stopBrowserGrab(true);
    return;
  }
  if (browserGrab !== null) stopBrowserGrab(true);
  const button = host.querySelector(mode === "annotate" ? ".browser-annotate" : ".browser-grab");
  browserGrab = { label: tab.label, mode, timer: 0, button };
  refreshGrabButtons();
  armBrowserGrab(tab.label, mode);
}

/* Arm (and re-arm after each added annotation — Orca's `grab.rearm()`). */
function armBrowserGrab(label, mode) {
  invoke("browser_grab_arm", { label, mode })
    .then(() => {
      if (browserGrab !== null && browserGrab.label === label) {
        browserGrab.timer = setTimeout(() => pollBrowserGrab(label), BROWSER_GRAB_POLL_MS);
      }
    })
    .catch((error) => {
      stopBrowserGrab(false);
      showError(String(error));
    });
}

/* One grabbed element, put into words for an agent. The field set and order
 * are Orca's `formatGrabPayloadAsText` — the interface the agent has learned
 * to read — in this window's own text. Untrusted page strings, so the whole
 * thing is data the agent quotes, never markup this window renders. */
function formatGrabText(picked) {
  const lines = [];
  lines.push(t("browser.grab.header", "브라우저에서 잡은 요소 — {{url}}", { url: picked.url ?? "" }));
  lines.push("");
  lines.push(`${t("browser.grab.tag", "태그")}: ${picked.tag ?? ""}`);
  if (picked.name) lines.push(`${t("browser.grab.name", "이름")}: "${picked.name}"`);
  if (picked.role) lines.push(`${t("browser.grab.role", "역할")}: ${picked.role}`);
  if (picked.selector) lines.push(`${t("browser.grab.selector", "선택자")}: ${picked.selector}`);
  const rect = picked.rect;
  if (rect && (rect.w || rect.h)) {
    lines.push(`${t("browser.grab.size", "크기")}: ${Math.round(rect.w)}x${Math.round(rect.h)}`);
  }
  if (picked.text) {
    lines.push("", `${t("browser.grab.text", "텍스트")}:`, picked.text);
  }
  const nearby = Array.isArray(picked.nearby) ? picked.nearby : [];
  if (nearby.length > 0) {
    lines.push("", `${t("browser.grab.nearby", "주변 맥락")}:`);
    for (const near of nearby) lines.push(`- ${near}`);
  }
  const styles = picked.styles || {};
  const styleLines = [];
  if (styles.display && styles.display !== "inline") styleLines.push(`display: ${styles.display}`);
  if (styles.position && styles.position !== "static") styleLines.push(`position: ${styles.position}`);
  if (styles.fontSize) styleLines.push(`font-size: ${styles.fontSize}`);
  if (styles.color) styleLines.push(`color: ${styles.color}`);
  if (styles.background && styles.background !== "rgba(0, 0, 0, 0)") {
    styleLines.push(`background: ${styles.background}`);
  }
  // Orca's Design Mode ships the pick's CSS whole; the five-line digest
  // stays only for payloads harvested before the page carried the door.
  if (picked.css) {
    lines.push("", `${t("browser.grab.css", "CSS (계산값)")}:`, "```css", picked.css, "```");
  } else if (styleLines.length > 0) {
    lines.push("", `${t("browser.grab.styles", "계산된 스타일")}:`);
    for (const styleLine of styleLines) lines.push(`  ${styleLine}`);
  }
  if (picked.html) {
    lines.push("", `${t("browser.grab.html", "HTML")}:`, "```html", picked.html, "```");
  }
  return lines.join("\n").trimEnd();
}

/* 모인 주석을 한 묶음의 말로 — 의도와 코멘트가 머리, 요소 맥락이 몸. 아티팩트의
 * 판을 보며 단 주석이면 그 판과 고칠 원본이 머리 바로 아래에 선다(t-3952). */
function formatAnnotationsText(label, url, context = "") {
  const held = browserAnnotations.get(label) ?? [];
  const lines = [t("browser.annotate.header", "브라우저 주석 — {{url}}", { url: url ?? "" })];
  if (context) lines.push(context);
  held.forEach((one, at) => {
    const intent =
      one.intent === "question"
        ? t("browser.annotate.ask", "질문")
        : t("browser.annotate.change", "변경");
    lines.push("", `${at + 1}. [${intent}] ${one.comment ?? ""}`);
    lines.push(formatGrabText(one));
  });
  return lines.join("\n").trimEnd();
}

async function deliverAnnotations(host, tab) {
  const held = browserAnnotations.get(tab.label) ?? [];
  if (held.length === 0) return;
  if (browserGrab !== null && browserGrab.label === tab.label) stopBrowserGrab(true);
  const button = host.querySelector(".browser-annotate");
  await openSendToAgent(
    button,
    formatAnnotationsText(tab.label, tab.url, artifactFeedbackContext(tab)),
    async () => {
      // 전달된 주석은 떠난다 — 뱃지가 비고, 다음 묶음이 새로 모인다.
      browserAnnotations.delete(tab.label);
      refreshGrabButtons();
    },
    { submit: false },
  );
}

async function deliverGrab(button, picked) {
  // Into the agent's INPUT and no further: a grabbed element is context the
  // person usually wants to write on top of, so the Enter stays theirs
  // (live report 2026-08-14: "바로 엔터치지 않고").
  await openSendToAgent(button, formatGrabText(picked), null, { submit: false });
}

/* Everything the window can stand in FRONT of a browser pane. The scrims are
 * gathered from the markup rather than named four at a time — the review
 * caught this list at 4 of 25 (finding 5), and a permission dialog buried
 * under a native page is a dialog nobody can answer. The four full-page
 * views cover the stage without a scrim, so they are named alongside. */
const browserShades = [...document.querySelectorAll('[id$="-scrim"]')];

function browserCovered() {
  if (!termFloat.hidden) return true;
  if (!el("workspace-board").hidden) return true;
  if (browserShades.some((shade) => !shade.hidden)) return true;
  // Floating menus live in the DOM; the native pane floats ABOVE the DOM, so a
  // menu that overlaps the browser body would be swallowed by the page — live
  // report 2026-08-14: the `+` palette and the grab send-picker both vanished
  // behind NAVER. The send picker and the `+` palette are the ones that open
  // over the body, and both wear `.note-pop` and toggle the `hidden` attribute
  // (through `showing`/`closing`), so one query catches both without the false
  // positives a CSS-hidden panel would bring.
  if (document.querySelector(".note-pop:not([hidden])")) return true;
  // The pane menu is one of these too — it opens right over the body it was
  // asked from (1-g13), and a menu swallowed by the page is a menu whose
  // rows cannot be clicked.
  if (!sidebarMenu.hidden) return true;
  return ["space-view", "settings-view", "task-view", "auto-view"].some(
    (page) => el(page)?.hidden === false,
  );
}

/* The placement commands, in the order they were decided. One chain for all
 * panes: a hide that raced an older show and lost put the page back on top
 * of a dialog (finding 8), and a queue is how two async answers keep the
 * order they were asked in. A failed placement forgets what it thought it
 * said, so the next sync retries instead of trusting a lie. */
let browserPlacing = Promise.resolve();
let restoringBrowserTabs = false;

/* What the restore record may carry — exactly what the door
 * (`set_browser_open_tabs` → `normalized_browser_url`) accepts: a web
 * address or the blank page. A `file://` tab opens fine (an artifact's
 * published address, the tree's 「ZeroCode 브라우저에서 열기」) but does not
 * cross a restart: the settings file never holds a path of this machine, and
 * a record the door refused failed the WHOLE save and toasted on every tab
 * change (t-4088). */
function rememberedBrowserAddress(url) {
  return url === "" || url === BROWSER_BLANK_URL || isWebAddress(url);
}

function browserOpenTabRecords() {
  return tabs
    .filter((tab) => tab.kind === "browser" && typeof tab.url === "string"
      && rememberedBrowserAddress(tab.url))
    .map((tab) => ({
      url: tab.url,
      viewport: tab.viewport ?? null,
      // 독자는 문자열로 적는다(t-3043 §2.5): 아이폰만이 아니라 어떤 독자든
      // 재시작을 건넌다. 표의 행을 입고 태어난 판은 독자가 없다 — 다음
      // 부팅에도 표가 입히므로, 기록이 그 이름을 굳히면 안 된다.
      reader: tab.reader ?? null,
      profile: tab.profile ?? null,
    }));
}

/* 옛 파일은 주소 문자열만 적었고(1-g25 이전), 그다음 철자는 `mobile` 불리언
 * (아이폰 독자)이었다 — 비교와 복원 둘 다 두 철자를 아직 읽고, 독자는
 * 문자열 하나로 넓힌다(t-3043 §2.5). 쓰는 철자는 `reader` 뿐이다. */
function storedTabRecordOf(one) {
  if (typeof one === "string") {
    return { url: one, viewport: null, reader: null, profile: null };
  }
  const reader = typeof one.reader === "string" && one.reader !== ""
    ? one.reader
    : one.mobile === true ? MOBILE_EMULATOR_UA : null;
  return {
    url: one.url,
    viewport: one.viewport ?? null,
    reader,
    profile: one.profile ?? null,
  };
}

function sameTabRecord(left, right) {
  return (
    left.url === right.url &&
    (left.viewport ?? null) === (right.viewport ?? null) &&
    (left.reader ?? null) === (right.reader ?? null) &&
    (left.profile ?? null) === (right.profile ?? null)
  );
}

function rememberBrowserOpenTabs() {
  if (restoringBrowserTabs) return;
  const records = browserOpenTabRecords();
  const held = (browserPrefs.open_tabs ?? []).map(storedTabRecordOf);
  if (
    records.length === held.length &&
    records.every((one, at) => sameTabRecord(one, held[at]))
  ) {
    return;
  }
  browserPrefs = { ...browserPrefs, open_tabs: records };
  void commitSetting("browser.open_tabs", "set_browser_open_tabs", { tabs: records });
}

async function restoreBrowserTabs() {
  if (!browserPrefs.restore_tabs) return;
  const stored = (browserPrefs.open_tabs ?? []).map(storedTabRecordOf);
  if (stored.length === 0) return;
  restoringBrowserTabs = true;
  let dressed = false;
  try {
    for (const record of stored) {
      const before = tabs.at(-1);
      await openBrowserTab(record.url, {
        userAgent: record.reader,
        profile: record.profile ?? null,
      });
      const born = tabs.at(-1);
      // 실패한 열기는 아무도 입히지 않는다 — at(-1)은 그때 옛 탭이다.
      if (born !== before && born?.kind === "browser" && record.viewport) {
        born.viewport = record.viewport;
        dressed = true;
      }
    }
  } finally {
    restoringBrowserTabs = false;
  }
  if (dressed) syncBrowserPanes();
}

/* 판을 같은 자리·같은 주소로 다시 세운다 — 프로필(쿠키 단지)과 독자(UA)는
 * 생성 시에만 입으므로(wry), 갈아입기는 곧 재생성이다(1-g26). 탭 객체는
 * 그대로 두고 이름표만 바꿔, 스트립·본-순서·주석이 제자리를 지킨다. */
async function reopenBrowserPane(tab, { profile = tab.profile ?? null } = {}) {
  const was = tab.label;
  const body = groupOf(tab.pane)?.browserView?.querySelector(".browser-body");
  const seat = body?.getBoundingClientRect() ?? { x: 0, y: 0, width: 800, height: 600 };
  let label;
  try {
    label = await invoke("open_browser_pane", {
      url: tab.url ?? BROWSER_BLANK_URL,
      x: seat.x,
      y: seat.y,
      width: Math.max(seat.width, 1),
      height: Math.max(seat.height, 1),
      ...(tab.reader ? { userAgent: tab.reader } : {}),
      ...(profile ? { profile } : {}),
    });
  } catch (error) {
    showError(String(error));
    return false;
  }
  void invoke("close_browser_pane", { label: was }).catch(() => {});
  browserSaid.delete(was);
  // 무장과 주석은 판 이름에 걸려 있다 — 무장은 내리고, 주석은 새 이름으로.
  if (browserGrab?.label === was) stopBrowserGrab(true);
  if (browserAnnotations.has(was)) {
    browserAnnotations.set(label, browserAnnotations.get(was));
    browserAnnotations.delete(was);
  }
  const oldId = tab.id;
  tab.label = label;
  tab.id = "browser:" + label;
  tab.profile = profile ?? undefined;
  tab.loading = true;
  tab.pageTitle = null;
  if (activeTabId === oldId) activeTabId = tab.id;
  // 이름을 아는 장부들: 본-순서와 워크스페이스별 활성 탭.
  for (const [, order] of groupRecents) {
    const at = order.indexOf(oldId);
    if (at >= 0) order[at] = tab.id;
  }
  for (const [worktree, id] of activeTabByWorktree) {
    if (id === oldId) activeTabByWorktree.set(worktree, tab.id);
  }
  renderTabs();
  updateStage();
  syncBrowserPanes();
  rememberBrowserOpenTabs();
  return true;
}

/* 새 프로필의 이름을 묻는 작은 팝. 판에서 물으면 그 판이 곧장 새 단지로
 * 갈아입고, 설정 페이지에서 물으면 갈아입을 판이 없다 — 목록만 는다. */
const profilePop = document.createElement("div");
profilePop.className = "note-pop tc-pop profile-pop";
profilePop.hidden = true;
const profileInput = document.createElement("input");
profileInput.className = "tc-input";
profileInput.type = "text";
profileInput.spellcheck = false;
profilePop.appendChild(profileInput);
document.body.appendChild(profilePop);
let profileAsking = null;

function closeProfilePop() {
  closing(profilePop);
  profileAsking = null;
}

function askNewBrowserProfile(at, tab) {
  profileAsking = tab ?? null;
  profileInput.value = "";
  profileInput.placeholder = t("browser.profileName", "프로필 이름");
  showing(profilePop);
  const width = profilePop.getBoundingClientRect().width;
  profilePop.style.left = `${Math.min(Math.max(8, at.left), window.innerWidth - width - 8)}px`;
  profilePop.style.top = `${Math.min(at.bottom + 6, window.innerHeight - 80)}px`;
  profileInput.focus();
}

profileInput.addEventListener("keydown", (event) => {
  if (event.key === "Escape") {
    event.preventDefault();
    closeProfilePop();
    return;
  }
  if (event.key !== "Enter") return;
  event.preventDefault();
  const name = profileInput.value.trim();
  const tab = profileAsking;
  if (!name) {
    closeProfilePop();
    return;
  }
  void invoke("create_browser_profile", { name })
    .then(async (made) => {
      closeProfilePop();
      await refreshBrowserProfiles();
      paintBrowserProfiles();
      if (made?.id && tab) await reopenBrowserPane(tab, { profile: made.id });
    })
    .catch((error) => showError(String(error)));
});
dismissable(profilePop, closeProfilePop, ".browser-menu");

/* ---- 쿠키 가져오기 (지시서 C3) --------------------------------------------
 *
 * Orca의 'imported' 프로필 흐름을 우리 프로필 단지 위에 얹은 것. 세 걸음이
 * 전부 기존 사이드바 메뉴 기계로 걷는다: 소스 브라우저 → (여럿일 때만) 소스
 * 프로필 → 대상 프로필. 대상은 우리가 mint한 프로필만이다 — 기본 저장소는
 * C2의 담장(`browser_profile_store`) 밖이고, 그 담장이 곧 보안 검사다.
 *
 * 주입 시점 계약(메모 19): 가져온 쿠키는 스테이징에 앉고, 그 프로필의 판이
 * 다음에 태어날 때 마신다. 그래서 요약 카드가 "다시 열면 적용" 문장을 들고,
 * 그 프로필의 판이 이미 열려 있으면 그 자리에서 다시 여는 단추를 내민다. */
const cookieImportPop = document.createElement("div");
cookieImportPop.className = "note-pop tc-pop cookie-import-pop";
cookieImportPop.id = "cookie-import-pop";
cookieImportPop.hidden = true;
document.body.appendChild(cookieImportPop);

function closeCookieImportPop() {
  closing(cookieImportPop);
}

async function askCookieImport(box, target) {
  let sources;
  try {
    sources = await invoke("cookie_sources");
  } catch (error) {
    showError(String(error));
    return;
  }
  const at = () => [box.left, box.bottom + 4];
  const pickTarget = (family, profile) => {
    // 대상을 아는 채로 불렸으면 마지막 질문이 없다 — 설정 페이지에서는 누른
    // 행이 곧 대상이고, 도구모음에서는 프로필 위에 서 있는 판이 그렇다
    // (메모 19). 둘 다 아니면 어느 단지로 갈지 여기서 묻는다.
    if (target) {
      void runCookieImport(family, profile, target, box);
      return;
    }
    if (!browserProfiles.length) {
      toast(
        t("browser.importNeedProfile", "먼저 새 프로필을 만들어 주세요 — 가져오기는 프로필 단지에만 앉습니다"),
        "halt",
      );
      return;
    }
    openSidebarMenu(...at(), [
      { label: t("browser.importTarget", "어느 프로필로 가져올까요"), disabled: true, run: () => {} },
      ...browserProfiles.map((held) => ({
        label: held.name,
        run: () => void runCookieImport(family, profile, held.id, box),
      })),
    ]);
  };
  const pickProfile = (source) => {
    // 프로필이 하나뿐이거나 아예 없으면(사파리) 물을 것이 없다.
    if ((source.profiles?.length ?? 0) <= 1) {
      pickTarget(source.family, source.profiles?.[0]?.directory ?? null);
      return;
    }
    openSidebarMenu(...at(), [
      { label: source.label, disabled: true, run: () => {} },
      ...source.profiles.map((held) => ({
        label: held.name,
        run: () => pickTarget(source.family, held.directory),
      })),
    ]);
  };
  const sourceRows = (sources ?? []).map((source) => ({
    label: source.label,
    run: () => pickProfile(source),
  }));
  // 파일에서 가져오기는 대상 프로필을 아는 자리(설정 카드)에서만 선다 —
  // 브라우저가 하나도 없어도 파일은 언제나 있을 수 있다(원본의 "From File…").
  const fileRow = target
    ? [{
      label: t("browser.importFromFile", "파일에서 가져오기…"),
      run: () => void runCookieFileImport(target, box),
    }]
    : [];
  if (!sourceRows.length && !fileRow.length) {
    toast(t("browser.importNoSources", "가져올 브라우저를 찾지 못했습니다"), "halt");
    return;
  }
  openSidebarMenu(...at(), [
    { label: t("browser.importFrom", "브라우저에서 가져오기"), disabled: true, run: () => {} },
    ...sourceRows,
    ...fileRow,
  ]);
}

async function runCookieImport(family, profile, target, box) {
  // macOS에서 Chromium계는 이 호출이 키체인 프롬프트를 띄운다 — 조용히
  // 멈춘 것처럼 보이는 시간에 이름을 붙여 둔다.
  toast(t("browser.importBusy", "쿠키를 읽는 중 — 키체인 확인이 필요할 수 있습니다"));
  // 설정 카드의 이 프로필 줄이 "가져오는 중…"으로 잠긴다 — 키체인 프롬프트와
  // SQLite 사본이 도는 시간에 이름을 준다(원본의 importState.importing).
  browserImportingId = target;
  paintBrowserProfiles();
  let summary;
  try {
    summary = await invoke("import_browser_cookies", { family, profile, target });
  } catch (error) {
    browserImportingId = null;
    paintBrowserProfiles();
    showError(String(error));
    return;
  }
  // 그 프로필의 "마지막으로 가져온 곳"이 방금 달라졌다. 목록은 한 곳에서만
  // 그려지므로 설정 페이지가 열려 있든 아니든 여기서 따라 그린다.
  browserImportingId = null;
  await refreshBrowserProfiles();
  paintBrowserProfiles();
  showCookieImportSummary(target, summary, box);
}

/* 파일(Netscape cookies.txt)에서 가져오기 — 시스템 손이 파일 선택을 열고
 * (import_cookie_file), 나머지는 브라우저 가져오기와 같은 자리로 걷는다.
 * 선택을 닫으면(취소) summary가 null이라 조용히 물러난다(원본 계약). */
async function runCookieFileImport(target, box) {
  browserImportingId = target;
  paintBrowserProfiles();
  let summary;
  try {
    summary = await invoke("import_cookie_file", { target });
  } catch (error) {
    browserImportingId = null;
    paintBrowserProfiles();
    showError(String(error));
    return;
  }
  browserImportingId = null;
  await refreshBrowserProfiles();
  paintBrowserProfiles();
  if (summary === null) return;
  showCookieImportSummary(target, summary, box);
}

function showCookieImportSummary(target, summary, box) {
  cookieImportPop.replaceChildren();
  const line = (words, quiet) => {
    const said = document.createElement("p");
    said.className = quiet ? "cookie-import-quiet" : "cookie-import-line";
    said.textContent = words;
    cookieImportPop.appendChild(said);
  };
  line(
    t("browser.importDone", "쿠키 {{imported}}/{{total}}개 가져옴 · 도메인 {{domains}}곳", {
      imported: summary.importedCookies,
      total: summary.totalCookies,
      domains: summary.domains?.length ?? 0,
    }),
  );
  // 숨기지 않는 세 가지 — Orca도 요약에 같은 셋을 신는다
  // (browser-workspace-types.ts:139-160).
  if (summary.googleCookiesSkipped > 0) {
    line(
      t("browser.importGoogleSkipped", "구글 계정 결속 쿠키 {{count}}개는 옮기지 않음", {
        count: summary.googleCookiesSkipped,
      }),
      true,
    );
  }
  if (summary.partitionSkippedCookies > 0) {
    line(
      t("browser.importPartitionSkipped", "파티션 저장소 쿠키 {{count}}개는 충실히 옮길 수 없어 건너뜀", {
        count: summary.partitionSkippedCookies,
      }),
      true,
    );
  }
  if (summary.warning?.code === "cookies-undecryptable") {
    line(
      t("browser.importUndecryptable", "{{count}}개는 풀 수 없었습니다 — 소스 브라우저를 완전히 종료하고 다시 시도해 보세요", {
        count: summary.warning.failedCookies,
      }),
      true,
    );
  }
  line(t("browser.importReopenHint", "이 프로필의 브라우저를 다시 열면 적용됩니다"), true);
  // 그 프로필로 이미 열린 판이 있으면, 그 자리에서 들이켜게 한다.
  const drinking = tabs.filter((one) => one.kind === "browser" && one.profile === target);
  if (drinking.length) {
    const now = document.createElement("button");
    now.type = "button";
    now.className = "btn cookie-import-reopen";
    now.textContent = t("browser.importReopenNow", "지금 다시 열기");
    now.addEventListener("click", () => {
      for (const one of drinking) void reopenBrowserPane(one, { profile: target });
      closeCookieImportPop();
    });
    cookieImportPop.appendChild(now);
  }
  showing(cookieImportPop);
  const width = cookieImportPop.getBoundingClientRect().width;
  cookieImportPop.style.left = `${Math.min(Math.max(8, box.left), window.innerWidth - width - 8)}px`;
  cookieImportPop.style.top = `${Math.min(box.bottom + 6, window.innerHeight - 200)}px`;
}

dismissable(cookieImportPop, closeCookieImportPop, ".browser-menu", ".browser-profile-import");

/* Tell the backend where every browser pane is and whether it may be seen —
 * the ONE place that answer is computed. The native view floats above the
 * window's surface, so every overlay must push it out of the way; the
 * observer below fires this on each of those doors without owning any of
 * them. */
function syncBrowserPanes() {
  const covered = browserCovered();
  for (const tab of tabs) {
    if (tab.kind !== "browser") continue;
    const host = groupOf(tab.pane)?.browserView;
    const visible = !covered && host && !host.hidden && !tab.marking &&
      tab.worktree === activeWorktreePath && activeTabIn(tab.pane)?.id === tab.id;
    if (visible && !document.hidden) scheduleBrowserRetry(tab);
    else cancelBrowserRetry(tab);
    const body =
      !covered &&
      host &&
      !host.hidden &&
      // 마크업 중엔 편집기가 그 사각형의 주인이다 — 판은 눕는다.
      !tab.marking &&
      !tab.navFailure &&
      tab.worktree === activeWorktreePath &&
      activeTabIn(tab.pane)?.id === tab.id
        ? host.querySelector(".browser-body")
        : null;
    const raw = body?.getBoundingClientRect();
    // 무대 밖은 없다: 어떤 낡은 좌표도 무대 사각형을 넘어 칠하지 못한다 —
    // 넘친 판은 파일 레일을 덮는 흰 면이 됐다(라이브 보고 2026-08-14 #64).
    const yard = stage.getBoundingClientRect();
    const fenced = raw
      ? {
          x: Math.max(raw.x, yard.x),
          y: Math.max(raw.y, yard.y),
          width: Math.max(0, Math.min(raw.right, yard.right) - Math.max(raw.x, yard.x)),
          height: Math.max(0, Math.min(raw.bottom, yard.bottom) - Math.max(raw.y, yard.y)),
        }
      : null;
    const rect =
      fenced && fenced.width > 0 && fenced.height > 0
        ? viewportBox(fenced, tab.viewport ?? null)
        : null;
    // 프리셋도 열쇠의 일부 — 같은 몸에서 프리셋만 바뀐 판은 다시 놓여야 한다.
    const said = rect
      ? [Math.round(rect.x), Math.round(rect.y), Math.round(rect.width), Math.round(rect.height), tab.viewport ?? ""].join(",")
      : "hidden";
    if (browserSaid.get(tab.label) === said) continue;
    browserSaid.set(tab.label, said);
    const label = tab.label;
    const place = rect
      ? { label, x: rect.x, y: rect.y, width: rect.width, height: rect.height, shown: true }
      : { label, x: 0, y: 0, width: 1, height: 1, shown: false };
    browserPlacing = browserPlacing.then(() =>
      invoke("browser_place", place).catch(() => {
        browserSaid.delete(label);
      }),
    );
  }
}

/* A rough seat for the pane's first frame — the leaf's rect, minus a toolbar
 * measured properly one paint later. The sync corrects any error before the
 * page has pixels to show, so this only decides where the FIRST flash lands. */
const BROWSER_TOOLBAR_GUESS = 37;

/* The group already standing to the RIGHT of this one: the nearest ancestor
 * split whose left-right FIRST half holds it has a right-hand neighbour, and
 * that neighbour's first leaf is the seat (`findReusableRightSplitTarget`,
 * shutdown-checkpoint-guard-DjeYIuhi.js:270). A group already rightmost has
 * no such seat and answers null. */
function stageRightOf(node, target) {
  if (node.type === "leaf") return { holds: node.group === target, seat: null };
  const first = stageRightOf(node.first, target);
  if (first.holds) {
    return {
      holds: true,
      seat:
        first.seat ??
        (node.direction === "horizontal" ? stageGroups(node.second)[0] : null),
    };
  }
  return stageRightOf(node.second, target);
}

/* Add a stage neighbour to `group` on the requested layout axis. Used when
 * Setup must split beside a lane, which is a terminal surface but not a leaf
 * in a terminal tab's internal pane tree. */
function splitStageBeside(group, direction) {
  if (!activeTabIn(group)) return null;
  const box = groupOf(group)?.el?.getBoundingClientRect();
  if (!box || box.width === 0 || box.height === 0) return null;
  const added = nextGroupId;
  nextGroupId += 1;
  setStageTree(
    splitStageLeaf(stageTree(), group, added, direction, "second"),
  );
  return added;
}

function splitStageBesideFocused(direction) {
  return splitStageBeside(focusedPane, direction);
}

/* The stage seats a newcomer the way Orca's rightSplit placement does:
 * an existing seat to the RIGHT of `group` is reused — the new surface joins
 * that group as a tab instead of carving another sliver (live report
 * 2026-08-14: "3분활로 열리는데 보기가 힘들어") — and only a rightmost group
 * builds a new split, rightward, the newcomer second. Returns the group id,
 * or null when the stage has nothing to divide — an empty stage just opens a
 * tab. Beside the focus for what the person opens; beside the pane that
 * asked for what an agent opens (t-6379). */
function standBeside(group) {
  if (!activeTabIn(group)) return null;
  const reused = stageRightOf(stageTree(), group).seat;
  if (reused !== null) return reused;
  return splitStageBeside(group, "horizontal");
}

function standBesideFocused() {
  return standBeside(focusedPane);
}

const RICH_STAGE_WEIGHT = 1.7;

function preferredStageWeight(node, preferred) {
  if (node.type === "leaf") return node.group === preferred ? RICH_STAGE_WEIGHT : 1;
  return preferredStageWeight(node.first, preferred)
    + preferredStageWeight(node.second, preferred);
}

function stageTreePreferringGroup(node, preferred) {
  if (node.type === "leaf") return node;
  const first = stageTreePreferringGroup(node.first, preferred);
  const second = stageTreePreferringGroup(node.second, preferred);
  if (node.direction !== "horizontal") return { ...node, first, second };
  const firstWeight = preferredStageWeight(node.first, preferred);
  const secondWeight = preferredStageWeight(node.second, preferred);
  const ratio = Math.min(
    1 - STAGE_MIN_RATIO,
    Math.max(STAGE_MIN_RATIO, firstWeight / (firstWeight + secondWeight)),
  );
  return { ...node, ratio, first, second };
}

/* A browser or device mirror needs a viewport, not another terminal-width
 * column. With three or more leaves, give the rich surface roughly 46% of a
 * three-way stage and share the rest fairly. Two-way layouts remain 50/50 and
 * every divider is still draggable afterwards. */
function optimizeRichStageGroup(group) {
  if (group === null || stageGroups().length < 3 || !stageGroups().includes(group)) return false;
  setStageTree(stageTreePreferringGroup(stageTree(), group));
  return true;
}

async function openBrowserTab(
  rawUrl,
  {
    beside = false,
    userAgent = null,
    profile = null,
    expectedWorktree = null,
    from = null,
    label: reserved = null,
    focus = true,
  } = {},
) {
  // The pop-out runs this same script, but a pane it opened would attach to
  // the MAIN window — a surface it cannot see or close. The backend refuses
  // too; this spares it the knock.
  if (isPopout) return;
  const defaultZoom = browserZoomTarget(browserPrefs.default_zoom_level);
  if (defaultZoom === null) {
    showError(t(
      "settings.browser.zoomUnavailable",
      "브라우저 확대 설정을 읽을 수 없습니다.",
    ));
    return;
  }
  // Every open walks the address bar's own ladder (`navigableUrlOf`) — the
  // home-page setting and any caller may say a bare host ("naver.com"), and
  // a raw string handed to the backend was a voyage that never left (live
  // report 2026-08-14: "http 안붙여도 자동으로 붙게" — the configured site
  // never loaded). A word the ladder cannot place becomes the blank page.
  const url = navigableUrlOf(rawUrl ?? (browserPrefs.home_page || "")) ?? BROWSER_BLANK_URL;
  const seat = groupOf(focusedPane)?.el?.getBoundingClientRect() ?? {
    x: 0,
    y: 0,
    width: 800,
    height: 600,
  };
  // 새 탭은 설정에서 고른 기본 프로필을 입는다("활성" 배지의 값). 복원과
  // 재생성은 제 프로필을 그대로 들고 오고(그 길은 expectedWorktree를 든다),
  // 거기엔 손대지 않는다 — 내장 기본 저장소에 있던 탭이 기본 프로필로 옮겨
  // 가면 안 된다. 그래서 명시 profile이 없고, 또 새로 여는 것일 때만 default.
  if (profile === null && expectedWorktree === null && browserDefaultProfileId === null) {
    await refreshBrowserProfiles();
  }
  const jar = profile ?? (expectedWorktree === null ? browserDefaultProfileId : null);
  // An agent's `zerocode-browser open` names the pane it ran in. The tab
  // belongs to THAT pane's checkout: opened into the focused group it landed
  // in whatever project the person had turned to, on a stage the agent's own
  // project never showed (live report 2026-09-03: "다른 프로젝트를 보고 있는데
  // 그 창에서 브라우저가 열림"). Seated at home, and never focused by the
  // agent's word (`focus: false` from its listener): the stage is the
  // person's, in the agent's project as much as in any other.
  const home = from === null ? null : (tabOfTerm(from)?.worktree ?? null);
  const away = home !== null && home !== activeWorktreePath;
  let label;
  try {
    label = await invoke("open_browser_pane", {
      url,
      x: seat.x,
      y: seat.y + BROWSER_TOOLBAR_GUESS,
      width: Math.max(seat.width, 1),
      height: Math.max(seat.height - BROWSER_TOOLBAR_GUESS, 1),
      // 판의 독자(user agent)와 프로필(쿠키 단지)은 생성 시에만 정해진다
      // (wry) — 에뮬레이터 문과 프로필 문만 넘긴다.
      ...(userAgent ? { userAgent } : {}),
      ...(jar ? { profile: jar } : {}),
      // An agent's `open` reserved its label at the bridge and answered it
      // before this open ran; the backend accepts exactly that reservation.
      ...(reserved ? { label: reserved } : {}),
    });
  } catch (error) {
    showError(String(error));
    return;
  }
  if (expectedWorktree !== null && activeWorktreePath !== expectedWorktree) {
    browserEarly.delete(label);
    await invoke("close_browser_pane", { label }).catch(() => {});
    return;
  }
  try {
    // The pane stays hidden until `openTab` paints it, so a new or restored
    // tab cannot flash at 100% before taking the configured default.
    await invoke("browser_zoom", { label, scale: defaultZoom.scale });
  } catch (error) {
    browserEarly.delete(label);
    await invoke("close_browser_pane", { label }).catch(() => {});
    showError(String(error));
    return;
  }
  if (expectedWorktree !== null && activeWorktreePath !== expectedWorktree) {
    browserEarly.delete(label);
    await invoke("close_browser_pane", { label }).catch(() => {});
    return;
  }
  // `beside` is the + palette's word only: a restored session or a popup
  // replays MANY opens, and each splitting the stage would shatter it.
  const stood = beside && !away ? standBesideFocused() : null;
  if (stood !== null) optimizeRichStageGroup(stood);
  openTab({
    id: "browser:" + label,
    kind: "browser",
    label,
    url,
    pageTitle: null,
    loading: true,
    zoomLevel: defaultZoom.level,
    zoom: defaultZoom.scale,
    ...(expectedWorktree === null ? {} : { worktree: expectedWorktree }),
    ...(home === null ? {} : { worktree: home }),
    // 독자·프로필은 생성 시에만 정해지니(wry) 탭이 평생 기억한다 — 복원
    // 기록과 재생성 문이 이 표시를 읽는다(1-g25·1-g26).
    ...(userAgent ? { reader: userAgent } : {}),
    ...(jar ? { profile: jar } : {}),
    ...(stood === null ? {} : { pane: stood }),
  }, { focus: focus && !away });
  if (stood !== null) persistStageLayouts();
  rememberBrowserOpenTabs();
  // What the page said while the open was still in flight.
  const early = browserEarly.get(label);
  browserEarly.delete(label);
  if (early) {
    const held = tabs.find((one) => one.id === "browser:" + label);
    if (held) {
      if (early.nav) {
        applyBrowserNavigation(held, early.nav);
      }
      if (early.title !== undefined) held.pageTitle = early.title;
      renderTabs();
      if (stillShowing(held)) paintBrowserView(held);
    }
  }
  // Orca's `focusAddressBar: true`, and only for the tab a PERSON opened —
  // a popup's tab arrives with an address already meant.
  if (rawUrl === undefined) {
    const tab = tabs.find((one) => one.id === "browser:" + label);
    if (tab) docHost(tab.pane, "browser").querySelector(".browser-address")?.focus();
  }
  // The label, so a caller that has more to say about the tab (the artifact
  // page's header strip, t-3233) can find it on the strip. Truthy, as the
  // `true` it replaced was.
  return label;
}

/* 브라우저를 여는 두 자리(옆에 분할 / 단일 탭) 중 마지막 선택 — 다음 + 팔레트의
 * 기본값, 곧 먼저 서는 행이 된다. 찢어지거나 닫힌 저장소는 기록이 없는 것과
 * 같다(floatTriggerLoad의 태도). */
const BROWSER_PLACEMENT_KEY = "zerocode.browser-open-placement.v1";

function lastBrowserPlacement() {
  try {
    return localStorage.getItem(BROWSER_PLACEMENT_KEY) === "single" ? "single" : "split";
  } catch {
    return "split";
  }
}

function rememberBrowserPlacement(choice) {
  try {
    localStorage.setItem(BROWSER_PLACEMENT_KEY, choice);
  } catch {
    // 저장 못 하는 저장소에서는 기본값이 계속 "옆에 분할"일 뿐이다.
  }
}

/* Failed navigation is already a backend fact (`dead`, with a generation
 * guard). Keep the recovery UI on screen while a hidden native pane retries,
 * so a refused origin cannot flash white between attempts. */
const BROWSER_RETRY_POLICY = Object.freeze({ firstMs: 1000, maxMs: 30_000, factor: 2 });

function cancelBrowserRetry(tab) {
  if (tab.retryTimer != null) clearTimeout(tab.retryTimer);
  tab.retryTimer = null;
}

function markBrowserFailure(tab) {
  if (tab.navFailure?.url !== tab.url) {
    tab.navFailure = { url: tab.url, attempts: 0, paused: tab.navigationStopped === true };
  }
}

function browserRetryDelay(tab) {
  const attempts = tab.navFailure?.attempts ?? 0;
  return Math.min(BROWSER_RETRY_POLICY.maxMs,
    BROWSER_RETRY_POLICY.firstMs * BROWSER_RETRY_POLICY.factor ** attempts);
}

function scheduleBrowserRetry(tab) {
  if (!tab.navFailure || tab.navFailure.paused || tab.loading || tab.retryTimer != null) return;
  const failure = tab.navFailure;
  tab.retryTimer = setTimeout(() => {
    tab.retryTimer = null;
    if (!tabs.includes(tab) || tab.navFailure !== failure || document.hidden || !stillShowing(tab) || browserCovered()) return;
    void retryBrowserFailure(tab);
  }, browserRetryDelay(tab));
}

async function retryBrowserFailure(tab, { manual = false } = {}) {
  const failure = tab.navFailure;
  if (!failure || tab.loading || !tabs.includes(tab)) return;
  cancelBrowserRetry(tab);
  if (manual) { failure.attempts = 0; failure.paused = false; tab.navigationStopped = false; }
  failure.attempts += 1;
  tab.loading = true;
  if (stillShowing(tab)) paintBrowserView(tab);
  try {
    await invoke("browser_navigate", { label: tab.label, url: failure.url });
  } catch {
    if (tab.navFailure !== failure || !tabs.includes(tab)) return;
    tab.loading = false;
    if (stillShowing(tab)) paintBrowserView(tab);
    syncBrowserPanes();
  }
}

function paintBrowserFailure(host, tab) {
  const box = host.querySelector(".browser-failure");
  box.hidden = !tab.navFailure;
  if (!tab.navFailure) return;
  writeTextContent(box.querySelector(".browser-failure-title"), t("browser.failure.title", "페이지에 연결하지 못했습니다"));
  writeTextContent(box.querySelector(".browser-failure-url"), tab.navFailure.url);
  writeTextContent(box.querySelector(".browser-failure-copy"), tab.loading
    ? t("browser.failure.retrying", "다시 연결하는 중입니다…")
    : tab.navFailure.paused ? t("browser.failure.paused", "주소와 서버 상태를 확인한 뒤 다시 시도해 주세요.")
      : t("browser.failure.next", "{{seconds}}초 간격으로 다시 연결합니다.", { seconds: browserRetryDelay(tab) / 1000 }));
  const retry = box.querySelector(".browser-failure-retry");
  retry.textContent = t("browser.failure.retry", "다시 시도");
  retry.disabled = tab.loading === true;
  const auto = box.querySelector(".browser-failure-auto");
  auto.textContent = tab.navFailure.paused
    ? t("browser.failure.resume", "자동 재시도 켜기") : t("browser.failure.pause", "자동 재시도 중지");
}

function applyBrowserNavigation(tab, said) {
  // A delayed old failure cannot replace a newer address the person entered.
  if ((said.state === "dead" || said.state === "failed") && tab.url && tab.url !== said.url) return;
  tab.url = said.url;
  tab.loading = said.state === "started";
  if (said.state === "dead" || said.state === "failed") markBrowserFailure(tab);
  else if (said.state === "finished" || tab.navFailure?.url !== said.url) {
    cancelBrowserRetry(tab);
    tab.navFailure = null;
  }
}

listen("browser:nav", (event) => {
  if (isPopout) return;
  const said = event.payload;
  const tab = tabs.find((one) => one.kind === "browser" && one.label === said.label);
  if (!tab) {
    browserEarly.set(said.label, { ...browserEarly.get(said.label), nav: said });
    return;
  }
  // Only a Started spins. `finished` is the page's word; `dead` is the
  // backend clock's — a Started with no Finished for NAV_DEAD_AFTER_SECS
  // (wry never reports a failed provisional navigation, and a dead dev
  // server's tab span forever, 2026-09-07). Either one stops the wheel.
  applyBrowserNavigation(tab, said);
  if (!tab.loading) rememberBrowserOpenTabs();
  renderTabs();
  if (stillShowing(tab)) paintBrowserView(tab);
  syncBrowserPanes();
});

listen("browser:cookie-import-error", (event) => {
  if (isPopout || typeof event.payload?.message !== "string") return;
  showError(event.payload.message);
});

listen("browser:title", (event) => {
  if (isPopout) return;
  const said = event.payload;
  const tab = tabs.find((one) => one.kind === "browser" && one.label === said.label);
  if (!tab) {
    browserEarly.set(said.label, { ...browserEarly.get(said.label), title: said.title });
    return;
  }
  tab.pageTitle = said.title;
  recordBrowserVisit(tab.url, said.title);
  renderTabs();
});

/* A link that asked for a new window walks IN THE SAME PANE — Orca's link
 * ROUTING default (live report 2026-08-14: "링크가 그 브라우저에서
 * 이동되야함"). The backend already refused the WINDOW; what arrives here is
 * the source pane and an address, and it walks the address bar's own
 * validated door. A popup whose pane has already gone still becomes a tab —
 * the address deserves a surface. (The routing MODIFIER arrives with the
 * browser-settings slice.) */
/* 탭마다 최근 다운로드 셋 — started가 예약한 경로를 들고, macOS의 빈
 * finished 경로는 그 예약을 그대로 쓴다(백엔드 BrowserDownload 주석). */
const browserDownloads = new Map();

listen("browser:download", (event) => {
  if (isPopout) return;
  const said = event.payload;
  const rows = browserDownloads.get(said.label) ?? [];
  if (said.state === "started") {
    rows.unshift({ url: said.url, file: said.file, path: said.path, state: "started" });
    if (rows.length > 3) rows.length = 3;
  } else {
    const row = rows.find((one) => one.url === said.url && one.state === "started");
    if (row) {
      row.state = said.state;
      if (said.path) row.path = said.path;
    }
  }
  browserDownloads.set(said.label, rows);
  const tab = tabs.find((one) => one.kind === "browser" && one.label === said.label);
  if (tab && stillShowing(tab)) paintBrowserView(tab);
});

/* 마크업의 키(실측 useMarkupKeyboardShortcuts): Escape는 진행 중 텍스트를
 * 먼저 물리고 그 다음에 편집기를 닫는다; ⌘Z/⇧⌘Z는 undo/redo. 캡처 단계 —
 * 창의 전역 Escape 사다리보다 먼저 서야 이 겹의 키가 된다(주소창 선례). */
window.addEventListener(
  "keydown",
  (event) => {
    const tab = currentTab?.();
    const held = tab?.marking;
    if (!held) return;
    if (event.key === "Escape") {
      if (event.target.closest?.(".browser-markup-text")) return;
      event.preventDefault();
      event.stopPropagation();
      closeBrowserMarkup(tab);
      return;
    }
    const primary = navigator.platform?.includes("Mac") ? event.metaKey : event.ctrlKey;
    if (primary && event.key.toLowerCase() === "z") {
      const typing = event.target instanceof HTMLElement &&
        (event.target.tagName === "INPUT" || event.target.tagName === "TEXTAREA");
      if (typing) return;
      event.preventDefault();
      event.stopPropagation();
      if (event.shiftKey) markupRedo(held);
      else markupUndo(held);
      held.layerDirty = true;
      markupRepaint(held);
    }
  },
  true,
);

listen("browser:popup", (event) => {
  if (isPopout) return;
  const said = event.payload;
  const tab = tabs.find((one) => one.kind === "browser" && one.label === said.label);
  if (tab) submitBrowserAddress(tab, said.url);
  else void openBrowserTab(said.url);
});

/* A label an agent's event names, or null: only a pane's own name shape
 * (`browser-N`) is passed on to a road that would look it up. */
const agentPaneLabel = (label) =>
  (typeof label === "string" && /^browser-\d+$/.test(label) ? label : null);

/* An agent asked for a tab (`zerocode-browser open`, 1-g4). The same door a
 * popup walks: the URL was validated at the bridge AND will be validated
 * again by the open — two doors, because the emit crosses a trust boundary.
 * The label rode along: the bridge reserved it and already answered it to
 * the agent, so this open claims that reservation rather than minting.
 *
 * Opened onto the strip, never onto the stage. The agent reads, clicks and
 * navigates by label and needs no seat in front of the person; taking one
 * hid whatever the person was reading and unwatched its terminals, so an
 * agent researching in seven tabs flickered the person's panes seven times
 * (2026-09-17 00:29–00:33: watch [2, 9] → [] in the second of every open). */
listen("browser:agent-open", (event) => {
  if (isPopout) return;
  const label = agentPaneLabel(event.payload?.label);
  void openBrowserTab(event.payload.url, {
    from: event.payload.term ?? null,
    label,
    focus: false,
  });
});

/* An agent asked to close a tab (`zerocode-browser close`). The ordinary
 * close — `closeTab`, hide→close→burn — and nothing shorter, so the bridge
 * sees the label leave the mint the way a person's close makes it leave. */
listen("browser:agent-close", (event) => {
  if (isPopout) return;
  const label = agentPaneLabel(event.payload?.label);
  if (label === null) return;
  const tab = tabs.find((one) => one.kind === "browser" && one.label === label);
  if (tab) closeTab(tab.id);
});

/* An agent asked for a viewport (`zerocode-browser viewport`): a preset id
 * or `WxH` as the gear menu would have set it, null for the default seat.
 * The same road the menu walks — `tab.viewport`, then the placement sync. */
listen("browser:agent-viewport", (event) => {
  if (isPopout) return;
  const label = agentPaneLabel(event.payload?.label);
  if (label === null) return;
  const tab = tabs.find((one) => one.kind === "browser" && one.label === label);
  if (!tab) return;
  const asked = event.payload?.viewport;
  const known = typeof asked === "string"
    && (BROWSER_VIEWPORT_PRESETS.some((one) => one.id === asked) || customViewportOf(asked) !== null);
  tab.viewport = known ? asked : null;
  syncBrowserPanes();
  rememberBrowserOpenTabs();
  if (stillShowing(tab)) paintBrowserView(tab);
});

/* The overlays' own doors stay untouched: watching the `hidden` attribute on
 * every scrim, the full-page views and the float is what keeps this feature
 * from owning a hook in every dialog. */
const browserOverlayWatch = new MutationObserver(() => syncBrowserPanes());
for (const shade of browserShades) {
  browserOverlayWatch.observe(shade, { attributes: true, attributeFilter: ["hidden"] });
}
for (const page of ["space-view", "settings-view", "task-view", "auto-view"]) {
  const held = el(page);
  if (held) browserOverlayWatch.observe(held, { attributes: true, attributeFilter: ["hidden"] });
}
// The send picker and the `+` palette duck the pane too (live report
// 2026-08-14): watched the same way so the pane bows the instant one opens
// over it.
// 컨텍스트·기어 메뉴(1-g13/1-g19)도 판 위에 뜨는 DOM — 닫힘을 지켜보는
// 자가 없으면 메뉴가 물러나도 판이 영영 엎드려 있다(1-g19의 하네스가 이
// 구멍을 실측으로 잡았다: 메뉴로 프리셋을 고른 순간의 sync가 "가려짐"을
// 보고 hidden을 놓은 뒤, 아무도 다시 세우지 않았다).
// `el("sidebar-menu")`로 직접: 이 등록은 선언 순서상 `sidebarMenu` 상수보다
// 먼저 달리고, TDZ는 관찰자를 세우기도 전에 창을 눕힌다.
for (const float of [notePop, tabCreatePop, profilePop, el("sidebar-menu")]) {
  if (float) browserOverlayWatch.observe(float, { attributes: true, attributeFilter: ["hidden"] });
}
browserOverlayWatch.observe(termFloat, { attributes: true, attributeFilter: ["hidden"] });
window.addEventListener("resize", () => syncBrowserPanes());

/* A pointer landing in this DOM is a claim on the keyboard. The native pane
 * never forwards its clicks here, so any pointer that DOES arrive means the
 * person left the page — and macOS does not always move first responder off
 * a child webview on its own (live report 2026-08-14: typing ran into NAVER
 * while the terminal looked focused). Asked only while a browser pane
 * exists; one knock per pointer-down is cheap. */
document.addEventListener(
  "pointerdown",
  () => {
    if (tabs.some((tab) => tab.kind === "browser")) {
      void invoke("focus_main").catch(() => {});
    }
  },
  true,
);

/* And the pointer decides which GROUP holds the keyboard, the way it already
 * decides which pane inside a tab is active. The strip was the only road that
 * moved `focusedPane`, so on a split stage typing followed a click on a
 * pane's TOP edge and nothing else (live report 2026-08-14: "창을 정확하게
 * 찍어야 인식됨"). Delegated from the stage: whichever group's element the
 * pointer went down in is the focused one. */
stage.addEventListener(
  "pointerdown",
  (event) => {
    for (const group of stageGroups()) {
      if (groups.get(group)?.el?.contains(event.target)) {
        focusedPane = group;
        // And the group's front tab becomes THE active tab — keystrokes
        // route by `activeTabId`, so a focus that moved without it left
        // typing running at the other pane (live report 2026-08-14: only
        // the header worked).
        const front = activeTabIn(group);
        if (front && front.id !== activeTabId) setActiveTab(front.id);
        return;
      }
    }
  },
  true,
);

/* The 새 에이전트 탭 chord, spent.
 *
 * The palette's own agent rows already know how to do this
 * ([`launchPaletteAgent`]) — the chord's whole job is deciding WHICH agent,
 * which is [`newAgentTabAgent`]. The catalog may not have been read yet when
 * this is the first thing pressed, the same ask [`openTermTab`] makes.
 *
 * Nothing installed is not an error and not a silent nothing: the original's
 * pick returns `null` there too, and the honest answer to "open a tab for the
 * agent" when there is no agent is the terminal this window can actually open.
 */
async function launchNewAgentTab() {
  if (agentRows.length === 0) await refreshAgents();
  const chosen = installedAgents().find((row) => row.id === newAgentTabAgent());
  if (!chosen) return openTermTab({ door: "terminal" });
  return launchPaletteAgent(chosen);
}

async function launchPaletteAgent(row) {
  try {
    const term = await launchAgentTab({
      agent: row.id,
      prompt: "",
      rows: 24,
      cols: 96,
    });
    mountTermTab(term, { agent: row.name });
  } catch (error) {
    showError(error);
  }
}

async function paintTabCreate() {
  const query = tcInput.value.trim();
  const stamp = ++tcStamp;
  const acts = [];
  // 두 박자로 그린다: 동기 행이 선 순간 한 번, 비동기 답이 다 든 뒤 한 번.
  // 한 번에 그리던 판은 뒷단이 느린 창에서 mobile_emulators·refreshAgents가
  // 돌아올 때까지 45px 빈 껍데기로 서 있었고(일지 실측 다섯 번, 최장 1.3초+),
  // 사람은 행이 있어야 할 자리 — 빈 판 밑 — 를 눌러 dismiss로 끝냈다.
  // 터미널 행은 동기라 첫 박자에 이미 누를 수 있다. 첫 박자의 빈 결과는
  // 문장을 세우지 않는다: 아직 다 듣지 않은 "결과 없음"은 거짓말이다.
  const paint = (settled) => {
    if (stamp !== tcStamp) return;
    // 빈 결과는 빈 판이 아니라 문장이다 — Orca의 EntryStatusRow. 다만 첫
    // 박자의 빈 결과는 아무것도 바꾸지 않는다: 서 있는 옛 행들이 tcActions와
    // 함께 그대로 남아, 답을 기다리는 동안에도 눌리는 채로 있는다.
    if (acts.length === 0) {
      if (!settled) return;
      tcActions = acts;
      tcSelected = 0;
      const none = document.createElement("div");
      none.className = "tc-none";
      none.textContent = t("tabs.createNone", "결과 없음");
      tcRows.replaceChildren(none);
      return;
    }
    tcActions = acts;
    tcSelected = 0;
    tcRows.replaceChildren(...acts.map((row, at) => tcRowEl(row, at)));
  };
  const terminalRow = {
    glyph: "terminal",
    label: t("tabs.newTerminal", "새 터미널"),
    hint: optionalShortcutLabel("terminal.newTab") ?? "",
    words: ["terminal", "shell", "터미널", t("tabs.newTerminal", "새 터미널")],
    run: () => {
      openTermTab({ door: "terminal" });
    },
  };
  if (!query || tcMatches(query, terminalRow.words)) acts.push(terminalRow);
  // Orca's own keyword set for this row (tab.create.menu.options: markdown,
  // md, new markdown, new file, mark).
  const markdownRow = {
    glyph: "file",
    label: t("tabs.newMarkdown", "새 마크다운"),
    hint: "",
    words: ["markdown", "md", "new file", "mark", "마크다운", t("tabs.newMarkdown", "새 마크다운")],
    run: () => void newMarkdownTab(),
  };
  if (!query || tcMatches(query, markdownRow.words)) acts.push(markdownRow);
  // 토큰 사용량 화면. Orca 의 `ClaudeUsagePane` 자리이지만 우리 쪽 문은
  // 팔레트다 — 자주 여는 화면이 아니라 궁금할 때 찾는 화면이므로.
  const tokensRow = {
    glyph: "columns",
    label: t("tokens.tab", "토큰 사용량"),
    hint: "",
    // 검색어는 낱말이지 라벨이 아니다 — 그래서 카탈로그를 탄다. 일본어를 쓰는
    // 사람은 일본어로 찾을 수 있어야 하고, `tabs.simKeyword*` 도 같은 이유로
    // 번역된다.
    words: [
      "tokens",
      "usage",
      "cost",
      t("tokens.keywords", "토큰 사용량 비용"),
      t("tokens.tab", "토큰 사용량"),
    ],
    run: () => openTokensTab(),
  };
  if (!query || tcMatches(query, tokensRow.words)) acts.push(tokensRow);
  // Orca's keyword set for its browser row (tab.create.menu.options:
  // browser, new browser, browser tab, web) — 행 두 개다: 열리기 전에 자리를
  // 고른다(라이브 보고 2026-08-26 "반갈라서 열릴지 독립적으로 열릴지 열리기
  // 전에 선택"). 옆에 분할은 스테이지를 가르고, 단일 탭은 가르지 않고 지금
  // 판의 탭으로 선다 — 둘 다 IDE 안이다(별도 OS 창이 아니다: "ide에서
  // 열게 시켰는데 다른 창에 뜸"). 마지막 선택이 기억되어 먼저 선다.
  const browserSplitRow = {
    glyph: "external",
    label: t("tabs.newBrowserSplit", "새 브라우저 탭 — 옆에 분할"),
    hint: "",
    words: [
      "browser", "new browser", "browser tab", "web", "split",
      "브라우저", "분할", t("tabs.newBrowser", "새 브라우저 탭"),
      t("tabs.newBrowserSplit", "새 브라우저 탭 — 옆에 분할"),
    ],
    run: () => {
      rememberBrowserPlacement("split");
      void openBrowserTab(undefined, { beside: true });
    },
  };
  const browserSingleRow = {
    glyph: "external",
    label: t("tabs.newBrowserSingle", "새 브라우저 탭 — 단일 창(분할 없음)"),
    hint: "",
    words: [
      "browser", "new browser", "browser tab", "web", "single", "full",
      "브라우저", "단일", t("tabs.newBrowser", "새 브라우저 탭"),
      t("tabs.newBrowserSingle", "새 브라우저 탭 — 단일 창(분할 없음)"),
    ],
    run: () => {
      rememberBrowserPlacement("single");
      void openBrowserTab(undefined);
    },
  };
  const browserRows = lastBrowserPlacement() === "single"
    ? [browserSingleRow, browserSplitRow]
    : [browserSplitRow, browserSingleRow];
  for (const row of browserRows) {
    if (!query || tcMatches(query, row.words)) acts.push(row);
  }
  paint(false);
  // Orca의 시뮬레이터 행(hasSimulator·new/goto 한 쌍): 우리의 모바일
  // 에뮬레이터는 뷰포트 상자를 입은 브라우저 판이다(1-g19 프리셋 — DPR·
  // 모바일 UA는 명시된 wry 이음새 그대로). 하나 있으면 "이동"으로 바뀌고
  // 실행은 그 탭을 앞세운다 — Orca와 같은 단독성. 키워드는 실측 일곱.
  // Orca의 에뮬레이터는 브라우저가 아니라 진짜 iOS 시뮬레이터다(simctl —
  // 라이브 보고 2026-08-15 "왜 웹브라우저가 열려"). 행은 기계가 시뮬레이터를
  // 답할 때만 서고(hasSimulator), 실행은 기본 기기를 부팅해 Simulator 앱을
  // 앞세운다 — 판 안 스트림 내장은 명시된 다음 이음새. 모바일 '크기'는 기어
  // 메뉴의 뷰포트가 계속 맡는다.
  if (mobileEmulators === null) {
    mobileEmulators = invoke("mobile_emulators", {}).catch(() => []);
  }
  const simulators = await mobileEmulators;
  if (stamp !== tcStamp) return;
  if (Array.isArray(simulators) && simulators.length > 0) {
    const simulatorRow = {
      glyph: "phone",
      label: t("tabs.newSimulator", "새 모바일 에뮬레이터"),
      hint: "iOS",
      words: [
        "mobile emulator",
        "emulator",
        "simulator",
        "ios simulator",
        "iphone",
        "ipad",
        "mobile",
        t("tabs.simKeywordMobile", "모바일"),
        t("tabs.simKeywordEmulator", "에뮬레이터"),
        t("tabs.simKeywordIphone", "아이폰"),
        t("tabs.simKeywordIpad", "아이패드"),
        t("tabs.simKeywordSim", "시뮬레이터"),
        t("tabs.newSimulator", "새 모바일 에뮬레이터"),
      ],
      run: () => void openEmulatorTab(),
    };
    if (!query || tcMatches(query, simulatorRow.words)) acts.push(simulatorRow);
  }
  // Android 에뮬레이터: 기계가 AVD를 답할 때만 선다. iOS 미러와 달리 진짜
  // 인터랙티브 터널(adb tap/type) — 라이브 보고 2026-08-15 "실제로 터널과
  // 통신하면서".
  if (androidEmulators === null) {
    androidEmulators = invoke("android_emulators", {}).catch(() => []);
  }
  const avds = await androidEmulators;
  if (stamp !== tcStamp) return;
  if (Array.isArray(avds) && avds.length > 0) {
    const androidRow = {
      glyph: "phone",
      label: t("tabs.newAndroid", "새 Android 에뮬레이터"),
      hint: "Android",
      words: [
        "android emulator",
        "android",
        "avd",
        "emulator",
        "mobile",
        t("tabs.simKeywordAndroid", "안드로이드"),
        t("tabs.simKeywordEmulator", "에뮬레이터"),
        t("tabs.newAndroid", "새 Android 에뮬레이터"),
      ],
      run: () => void openEmulatorTab("android"),
    };
    if (!query || tcMatches(query, androidRow.words)) acts.push(androidRow);
  }
  if (agentRows.length === 0) await refreshAgents();
  if (stamp !== tcStamp) return;
  // Claude Agent Teams — Orca 카탈로그의 그 항목이 우리 자신의 문으로
  // 들어온다: 직접 실행된 claude에는 이 창의 팀 장비(가짜 tmux·실험
  // 플래그)가 스폰 로드에서 이미 붙으므로, 이 행은 같은 claude를 팀의
  // 이름으로 띄운다. 팀 모드를 끈 창에는 행도 없다 — 없는 것은 없는 것.
  const claudeSeat = installedAgents().find((row) => row.id === "claude");
  if (agentTeamsMode !== "off" && claudeSeat) {
    const teamsRow = {
      face: agentIcon(claudeSeat),
      label: "Claude Agent Teams",
      hint: t("tabs.launchAgent", "에이전트 실행"),
      words: [
        "claude agent teams",
        "agent teams",
        "claude teams",
        "teams",
        "team",
        t("tabs.teamsKeyword", "팀"),
        t("tabs.teamsKeywordLong", "에이전트 팀"),
      ],
      run: () => void launchPaletteAgent({ ...claudeSeat, name: "Claude Agent Teams" }),
    };
    if (!query || tcMatches(query, teamsRow.words)) acts.push(teamsRow);
  }
  for (const row of installedAgents()) {
    if (query && !tcMatches(query, [row.name, row.id])) continue;
    acts.push({
      face: agentIcon(row),
      label: row.name,
      hint: t("tabs.launchAgent", "에이전트 실행"),
      run: () => void launchPaletteAgent(row),
    });
  }
  if (query) {
    // Open tabs by their shown labels — switching is what naming one means.
    for (const held of tabs) {
      const label = tabLabel(held);
      if (!tcMatches(query, [label])) continue;
      acts.push({
        glyph: "file",
        label,
        hint: t("tabs.openTab", "열린 탭"),
        run: () => setActiveTab(held.id),
      });
      if (acts.length >= 20) break;
    }
    // Repository files, through the same door the finder asks.
    try {
      const hits = await invoke("search_files", { query });
      if (stamp !== tcStamp) return;
      for (const hit of (hits ?? []).slice(0, 4)) {
        acts.push({
          glyph: "file",
          label: basename(hit),
          hint: hit,
          run: () => openPath(hit, { preview: true }),
        });
      }
    } catch {
      // Files are one column of this palette, not its floor.
    }
  } else {
    acts.push({
      glyph: "gear",
      label: t("review.agentSettings", "에이전트 설정…"),
      hint: "",
      run: () => {
        setSettingsOpen(true);
        showSettingsPane("agents");
      },
    });
  }
  paint(true);
}

tcInput.addEventListener("input", () => void paintTabCreate());
tcInput.addEventListener("keydown", (event) => {
  if (event.key === "ArrowDown" || event.key === "ArrowUp") {
    event.preventDefault();
    if (tcActions.length === 0) return;
    const step = event.key === "ArrowDown" ? 1 : tcActions.length - 1;
    tcSelected = (tcSelected + step) % tcActions.length;
    [...tcRows.children].forEach((node, at) =>
      node.classList.toggle("is-active", at === tcSelected),
    );
    return;
  }
  if (event.key === "Enter") {
    event.preventDefault();
    const chosen = tcActions[tcSelected];
    closeTabCreate("enter");
    chosen?.run();
    return;
  }
  if (event.key === "Escape") {
    event.preventDefault();
    closeTabCreate("escape");
  }
});

/* The strip's own leaf was already recorded by the click (`focusedPane`), so
 * the palette needs only its anchor: the row a choice opens into is the row
 * the `+` lives on, exactly as the old direct door behaved. */
function openTabCreate(button, road = "open") {
  tcOpenedAt = Date.now();
  setTabCreateTerminalPaintPaused(true);
  tcInput.value = "";
  tcInput.placeholder = t("tabs.createSearch", "열린 탭, 파일, agent 검색…");
  tabCreatePop.setAttribute("aria-label", tcInput.placeholder);
  void paintTabCreate();
  showModal(tabCreatePop, { animated: true, opener: button });
  const at = button.getBoundingClientRect();
  const width = tabCreatePop.getBoundingClientRect().width;
  const left = Math.min(Math.max(8, at.left), window.innerWidth - width - 8);
  tabCreatePop.style.left = `${left}px`;
  tabCreatePop.style.top = `${Math.min(at.bottom + 6, window.innerHeight - 80)}px`;
  // 닻이 0×0이면 팔레트가 타이틀바 밑에서 번쩍인다 — 그 좌표째로 일지에 남긴다.
  noteTabCreate(
    `${road} anchor=${Math.round(at.left)},${Math.round(at.bottom)} at=${Math.round(left)},`
      + `${Math.round(Math.min(at.bottom + 6, window.innerHeight - 80))}`,
  );
  // 그리고 한 박자 뒤의 두 번째 판독 — 진입은 150ms짜리라, 400ms 뒤에도
  // opacity가 0이면 애니메이션이 첫 프레임에서 멎어 있는 것이다. rAF가 아니라
  // 타이머인 이유가 곧 가설이다: 멎은 창에서는 rAF도 함께 멎는다.
  //
  // 둘째 판독(04:55)은 "opacity 1로 서 있는데 안 보인다"를 남겼다. 그럼
  // 물음은 하나로 준다: 그 픽셀의 주인이 누구인가 — elementFromPoint가
  // 팔레트 밖의 이름을 답하면 그 이름이 곧 덮개다.
  window.setTimeout(() => {
    if (tabCreatePop.hidden) return;
    const box = tabCreatePop.getBoundingClientRect();
    const owner = document.elementFromPoint(box.left + 24, box.top + 24);
    const named = owner
      ? `${owner.tagName.toLowerCase()}${owner.id ? `#${owner.id}` : ""}.${[...owner.classList].join(".")}`
      : "null";
    noteTabCreate(
      `open+400ms rect=${Math.round(box.left)},${Math.round(box.top)},`
        + `${Math.round(box.width)}x${Math.round(box.height)}`
        + ` owner=${tabCreatePop.contains(owner) ? "self" : named}`
        // 다섯 번째 실종이 온다면 제 이야기를 스스로 가르게: top layer 승격이
        // 실제로 섰는가(pop), 그리고 네이티브 브라우저 판 몇이 아직 화면을
        // 딛고 있는가(panes=서있는/전체) — 안 보이는 팔레트의 남은 두 갈래가
        // 이 두 값으로 갈린다.
        + ` pop=${tabCreatePop.matches(":popover-open") ? 1 : 0}`
        + ` panes=${[...browserSaid.values()].filter((said) => said !== "hidden").length}`
        + `/${tabs.filter((tab) => tab.kind === "browser").length}`,
    );
  }, TAB_CREATE_SETTLE_MS);
}

// 바깥 판정이 닫을 때는 그 바깥이 어디였는지도 적는다 — 01:49의 세 번은
// 일지 곁의 IME 테이프(keySink focus의 동시각)로만 터미널임을 읽어낼 수
// 있었다. 다음부터는 닫은 손가락이 제 자리를 제 이름으로 남긴다.
dismissable(
  tabCreatePop,
  (event) => {
    const hit = event?.target;
    const named = hit
      ? `${hit.tagName?.toLowerCase?.() ?? "?"}${hit.id ? `#${hit.id}` : ""}${
        hit.classList?.length ? `.${hit.classList[0]}` : ""}`
      : "";
    closeTabCreate(named ? `dismiss@${named}` : "dismiss");
  },
  ".tab-new",
  // ＋ 곁 몇 px의 빗나간 재누름은 바깥이 아니다 (여섯 번째 실종, 일지의
  // `dismiss@div#tabstrip.tabstrip`). 드래그 가드와 같은 슬하(GRIP_MISS_SLOP)를
  // dismiss 판정에도 준다 — 판은 서 있고, 그 클릭은 아무것도 아니게 된다.
  (event) => {
    const strip = event?.target instanceof Element && event.target.id === "tabstrip"
      ? event.target
      : null;
    const add = strip?.querySelector(".tab-new");
    if (!add) return false;
    const box = add.getBoundingClientRect();
    return (
      event.clientX >= box.left - GRIP_MISS_SLOP && event.clientX <= box.right + GRIP_MISS_SLOP
      && event.clientY >= box.top - GRIP_MISS_SLOP && event.clientY <= box.bottom + GRIP_MISS_SLOP
    );
  },
);

/* ---- the terminal's right-click menu ----
 *
 * Orca's `TerminalContextMenu`: copy and paste with their chords, the
 * quick-commands submenu, then the pane verbs this window has. What is
 * missing here is missing on purpose and ledgered — session forking and
 * pane titles are features this window does not have yet, and a menu row
 * that goes nowhere is worse than no row. */

const termMenu = el("term-menu");
const termMenuSub = el("term-menu-sub");
let menuTerm = null;

function closeTermMenu() {
  closing(termMenu);
  termMenuSub.hidden = true;
  menuTerm = null;
}

/* One menu row, for whichever menu asked for it.
 *
 * `close` is which popover this row belongs to — the terminal menu when
 * nobody says otherwise, because that is the menu this row was written for.
 * A second menu reusing the anatomy has to be able to say so, or picking one
 * of its rows dismisses a surface that was never open. */
function menuRow(host, { said, glyph, chord, run, disabled, badge, close }) {
  const pick = document.createElement("button");
  pick.className = "note-pop-row term-menu-row";
  pick.type = "button";
  pick.disabled = Boolean(disabled);
  if (glyph) pick.innerHTML = icon(glyph);
  const name = document.createElement("span");
  name.className = "note-pop-name";
  name.textContent = said;
  pick.appendChild(name);
  if (badge) {
    const tag = document.createElement("span");
    tag.className = "term-menu-chord";
    tag.textContent = badge;
    pick.appendChild(tag);
  } else if (chord) {
    const keys = document.createElement("span");
    keys.className = "term-menu-chord";
    keys.textContent = chord;
    pick.appendChild(keys);
  }
  pick.addEventListener("click", () => {
    (close ?? closeTermMenu)();
    run();
  });
  host.appendChild(pick);
  return pick;
}

function chordSaid(actionId, fallback) {
  const chords = chordsForId(actionId);
  return chords.length > 0 ? chordLabel(chords[0]) : fallback;
}

/* The submenu, beside its parent row: the checkout's commands under the
 * project's name, the global ones under `전역`, and the composer door. */
async function openQuickCommands(anchor, term) {
  let rows = [];
  try {
    rows = (await invoke("list_quick_commands")) ?? [];
  } catch {
    rows = [];
  }
  const host = el("term-menu-sub-body");
  host.replaceChildren();
  const label = (said) => {
    const line = document.createElement("div");
    line.className = "note-pop-label";
    line.textContent = said;
    host.appendChild(line);
  };
  const runRow = (command) => {
    const spec = command.agent ? agentRows.find((one) => one.id === command.agent) : null;
    const pick = document.createElement("button");
    pick.className = "note-pop-row term-menu-row";
    pick.type = "button";
    if (spec) pick.appendChild(agentIcon(spec));
    else pick.innerHTML = icon("play");
    const name = document.createElement("span");
    name.className = "note-pop-name";
    name.textContent = command.label;
    pick.appendChild(name);
    // A command that stops short of Enter is a template — Orca badges it.
    if (!command.agent && !command.append_enter) {
      const tag = document.createElement("span");
      tag.className = "term-menu-chord";
      tag.textContent = t("quick.insert", "입력만");
      pick.appendChild(tag);
    }
    pick.addEventListener("click", () => {
      closeTermMenu();
      runQuickCommand(command, term);
    });
    // The way out. Shown on hover like every quiet row control; it deletes
    // the SAVED command, so it asks the backend and redraws the submenu from
    // the answer rather than trusting its own memory of the list.
    const drop = document.createElement("button");
    drop.className = "term-menu-drop";
    drop.type = "button";
    drop.dataset.tip = t("quick.delete", "빠른 명령 삭제");
    drop.setAttribute("aria-label", drop.dataset.tip);
    drop.innerHTML = icon("trash");
    drop.addEventListener("click", async (event) => {
      // The row runs the command; the trash must not run it on the way out.
      event.stopPropagation();
      try {
        await invoke("delete_quick_command", { id: command.id });
      } catch (error) {
        showError(error);
        return;
      }
      await openQuickCommands(anchor, term);
    });
    pick.appendChild(drop);
    host.appendChild(pick);
  };
  // "이 프로젝트"는 프로젝트다 — 서 있는 워크트리가 아니라. 명령의 자리도
  // 지금 자리도 저마다의 프로젝트 본체로 올려 세워 비교한다: 워크트리에서
  // 저장한 명령이 본체와 형제 워크트리에서도 서고, 본체 경로로 적힌 명령은
  // 워크트리 안에서도 선다. 워크트리 경로 정확일치는 명령을 그 워크트리에
  // 가뒀고, 걷힌 워크트리에 적힌 명령은 설정에는 있는데 메뉴 어디에도 다시
  // 서지 못했다 (라이브 보고 2026-08-26 "설정에 있는 빠른 명령이 오른쪽
  // 버튼 눌러도 나오지 않는"). 카탈로그가 모르는 자리는 적힌 그대로
  // 비교한다 — 지어낸 소속보다 낫다.
  const projectPathOf = (path) => projectOfWorktree(path)?.path ?? path;
  const home = activeWorktreePath ? projectPathOf(activeWorktreePath) : null;
  // With no checkout open there IS no "this project" group — and matching
  // null against null must not drag every global command into it twice.
  const mine = home
    ? rows.filter((one) => one.workspace != null && projectPathOf(one.workspace) === home)
    : [];
  const global = rows.filter((one) => one.workspace == null);
  // Every command the settings pane lists stands here too. A command saved
  // for another project (or a checkout that has since been reclaimed) used
  // to fall through both groups above and vanish from every menu while the
  // settings list still showed it ("설정창에 설정된게 오른쪽 버튼눌러도 표시
  // 안되는", 2026-09-15). Those stand under their own project's name, after
  // this project's and the global ones — the body runs the same either way.
  const elsewhere = new Map();
  for (const one of rows) {
    if (one.workspace == null || (home && projectPathOf(one.workspace) === home)) continue;
    const place = projectPathOf(one.workspace);
    if (!elsewhere.has(place)) elsewhere.set(place, []);
    elsewhere.get(place).push(one);
  }
  if (mine.length === 0 && global.length === 0 && elsewhere.size === 0) {
    const none = document.createElement("div");
    none.className = "note-pop-none";
    none.textContent = t("quick.none", "등록된 빠른 명령이 없습니다");
    host.appendChild(none);
  } else {
    if (mine.length > 0) {
      // 무리의 이름도 프로젝트의 것 — 워크트리 폴더명이 아니라.
      label(
        projectOfWorktree(activeWorktreePath)?.name
          ?? (basename(home ?? "") || t("quick.thisProject", "이 프로젝트")),
      );
      mine.forEach(runRow);
    }
    if (global.length > 0) {
      if (mine.length > 0 || elsewhere.size > 0) label(t("quick.everywhere", "모든 프로젝트"));
      global.forEach(runRow);
    }
    for (const [place, held] of elsewhere) {
      label(projectOfWorktree(place)?.name ?? basename(place));
      held.forEach(runRow);
    }
  }
  const rule = document.createElement("div");
  rule.className = "note-pop-rule";
  host.appendChild(rule);
  menuRow(host, {
    said: t("quick.add", "빠른 명령 추가"),
    glyph: "plus",
    run: () => openQuickCommandEditor(term),
  });
  termMenuSub.hidden = false;
  const at = anchor.getBoundingClientRect();
  const wide = termMenuSub.getBoundingClientRect().width;
  const left = at.right + wide + 8 <= window.innerWidth ? at.right + 2 : at.left - wide - 2;
  termMenuSub.style.left = `${Math.max(8, left)}px`;
  termMenuSub.style.top = `${Math.min(at.top, window.innerHeight - 240)}px`;
}

function runQuickCommand(command, term) {
  if (command.agent) {
    // A prompt for an agent starts one, the prompt riding the launch — the
    // same road the notes menu takes, plus the one fact only this road has:
    // this menu belongs to a pane, so the agent it starts was started BY that
    // pane. That is what the board's subagent tree is drawn from, and it is
    // recorded at the launch because after it there is nothing left to say
    // which of two running agents came first.
    launchAgentTab({
      agent: command.agent,
      prompt: command.body,
      rows: 24,
      cols: 96,
      parent: term,
    })
      .then((opened) => {
        const spec = agentRows.find((one) => one.id === command.agent);
        mountTermTab(opened, { agent: spec?.name ?? command.agent });
      })
      .catch(showError);
    return;
  }
  const text = command.append_enter ? `${command.body}\r` : command.body;
  invoke("term_text", { term, text }).catch(showError);
}

function openTermMenu(x, y, term) {
  menuTerm = term;
  const host = el("term-menu-body");
  host.replaceChildren();
  // This pane's own selection, from the grid model — a right-click elsewhere
  // in the pane does not collapse it the way the browser's used to, and
  // another pane's selection is not this menu's to copy.
  const key = `term:${term}`;
  menuRow(host, {
    said: t("terminal.copy", "복사"),
    glyph: "copy",
    chord: chordLabel("mod+c"),
    disabled: !terminalSelectionStands(key),
    run: () => void copyTerminalSelection(key),
  });
  menuRow(host, {
    said: t("terminal.paste", "붙여넣기"),
    glyph: "clipboard",
    chord: chordLabel("mod+v"),
    run: () => void pasteClipboardAt({ kind: "term", term, key: `term:${term}` }),
  });
  const quick = menuRow(host, {
    said: t("quick.title", "빠른 명령"),
    glyph: "play",
    badge: "▸",
    run: () => {},
  });
  quick.addEventListener("pointerenter", () => openQuickCommands(quick, term));
  quick.addEventListener("click", (event) => {
    event.stopImmediatePropagation();
    openQuickCommands(quick, term);
  });
  // Only where a conversation lives — Orca's menu draws this row behind the
  // same predicate its pane header uses (`canContinueAgentSessionInNewSession`),
  // and a row that could only toast "no context" teaches nothing by being there.
  const menuTab = currentTab();
  if (menuTab?.kind === "term" && paneHoldsAgent(menuTab, term)) {
    menuRow(host, {
      said: t("continue.continueInNewSession", "새 세션에서 계속…"),
      glyph: "message-plus",
      run: () => void openContinueDialog(menuTab, term),
    });
  }
  const rule = document.createElement("div");
  rule.className = "note-pop-rule";
  host.appendChild(rule);
  menuRow(host, {
    said: t("terminal.splitRight", "터미널 오른쪽으로 분할"),
    glyph: "columns",
    chord: chordSaid("terminal.splitRight", chordLabel("mod+d")),
    run: () => splitActivePane("vertical"),
  });
  menuRow(host, {
    said: t("terminal.splitDown", "터미널 아래로 분할"),
    glyph: "rows",
    chord: chordSaid("terminal.splitDown", chordLabel("mod+shift+d")),
    run: () => splitActivePane("horizontal"),
  });
  // The two pane-shape verbs, shown only when there is a shape to change.
  // Orca hides them rather than disabling them: a row that can never do
  // anything on a single-pane tab is a row that teaches nothing by being
  // there. Two tests rather than one, because Orca asks two — equalize also
  // goes away while a pane is expanded, since the ratios it would even out
  // are not the ones on screen.
  const tab = currentTab();
  if (canEqualizePanes(tab)) {
    menuRow(host, {
      said: t("terminal.equalize", "페인 크기 균등"),
      glyph: "columns",
      run: () => equalizeActivePanes(tab),
    });
  }
  if (canExpandPane(tab)) {
    menuRow(host, {
      said:
        expandedPaneOf(tab) === term
          ? t("terminal.collapsePane", "페인 접기")
          : t("terminal.expandPane", "페인 펼치기"),
      glyph: "rows",
      run: () => togglePaneExpanded(tab, term),
    });
  }
  const shape = document.createElement("div");
  shape.className = "note-pop-rule";
  host.appendChild(shape);
  menuRow(host, {
    said: t("terminal.setTitle", "이름 지정…"),
    glyph: "pencil",
    run: () => {
      if (tab) startPaneRename(tab, term);
    },
  });
  if (tab && paneTitleOf(tab, term)) {
    menuRow(host, {
      said: t("terminal.clearPaneTitle", "페인 이름 지우기"),
      glyph: "trash",
      run: () => clearPaneTitle(tab, term),
    });
  }
  // The shell's id, for the person who needs to name this pane to something
  // outside the window — a log line, a `kill`, a bug report. Orca offers both
  // a terminal id and a pane id; ours are the same number, because a leaf owns
  // exactly one pty and a second identifier would be a second name for it.
  menuRow(host, {
    said: t("terminal.copyId", "터미널 ID 복사"),
    glyph: "copy",
    run: () => void clipboardText.write(String(term)),
  });
  if (paneAgents.has(term) || paneSessions.has(term)) {
    menuRow(host, {
      said: t("coordinator.title", "코디네이터와 인계"), glyph: "orbit",
      run: () => void openCoordinatorPanel(term),
    });
  }
  const tail = document.createElement("div");
  tail.className = "note-pop-rule";
  host.appendChild(tail);
  menuRow(host, {
    said: t("terminal.closePane", "페인 닫기"),
    glyph: "x",
    chord: chordSaid("tab.close", chordLabel("mod+w")),
    run: () => {
      if (!closeActivePane()) closeTab(currentTab()?.id);
    },
  });
  const clearing = document.createElement("div");
  clearing.className = "note-pop-rule";
  host.appendChild(clearing);
  menuRow(host, {
    said: t("terminal.clear", "화면 지우기"),
    glyph: "ban",
    chord: "\u2303L",
    // ⌃L to the shell, which owns its own screen — the window clearing a
    // buffer a child is drawing would fight it.
    run: () => invoke("term_text", { term, text: "\u000c" }).catch(showError),
  });
  showing(termMenu);
  termMenuSub.hidden = true;
  const wide = termMenu.getBoundingClientRect().width;
  const tall = termMenu.getBoundingClientRect().height;
  termMenu.style.left = `${Math.min(x, window.innerWidth - wide - 8)}px`;
  termMenu.style.top = `${Math.min(y, window.innerHeight - tall - 8)}px`;
}

document.addEventListener("contextmenu", (event) => {
  const slot = event.target.closest?.("[data-term]");
  const tab = currentTab();
  if (!slot || tab?.kind !== "term") return;
  event.preventDefault();
  const term = Number(slot.dataset.term);
  // The clicked pane becomes the active one, so the pane verbs act on what
  // was pointed at — a menu acting on a different pane is a misfire.
  if (tab.activePane !== term) {
    tab.activePane = term;
    renderPanes(tab);
  }
  openTermMenu(event.clientX, event.clientY, term);
});

dismissable(termMenu, closeTermMenu, termMenuSub);
