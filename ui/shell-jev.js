/* ---- Jev: the one switch, and the ledgers' numbers on the dashboard ----
 *
 * The switch a person turns Jev on and off with (2026-09-23,
 * docs/design/jev-settings-20260917.md §6.1) stands on the settings card and
 * over the dashboard's table: one command, one state, painted here for both.
 *
 * What zo last said each feature's ledger holds (`zo jev summary --json`,
 * §5) is the dashboard's (docs/design/jev-dashboard-and-perfection-20260921.md
 * §2): the table, each feature's recent judgments and the week's trend. It
 * does not count the files: the judge that promotes a feature reads them the
 * same way zo does, and a screen free to disagree with the feature it is
 * drawing is worse than a screen with no numbers. */

/* The tab the dashboard lives in — a document surface like the skills view,
 * built here rather than declared in index.html (`DOC_VIEWS.jev`). */
const JEV_TAB = Object.freeze({ id: "jev", kind: "jev" });

/* How many of each seat's last requests the dashboard lists. */
const JEV_RECENT_ROWS = 12;

/* The dashboard's slow beat while it is on stage. Chosen from the cost of
 * one refresh, which is one zo process, measured (10 runs each, 2026-09-22,
 * t-5807 report): 101 ms p50 / 124 ms p95 over this machine's live ledgers
 * (1,132 rows in the week, the largest file 1,005 rows and 1.47 MB), 68 ms
 * p50 / 98 ms p95 over a synthetic 3,000-row ledger (1.03 MB), against a
 * 13 ms p50 floor for the process itself. Every thirty seconds that is a
 * third of a percent of one core, and the seats zo writes (routing, recall)
 * are what the beat is for — the seats this window writes announce
 * themselves through the ledger event below and never wait for it. Not the
 * 1 s ledger poll: that beat is a SQLite read, this one is a process. */
const JEV_POLL_MS = 30_000;

/* After the ledger moved, how long the dashboard lets the seat's row land
 * before asking zo — a summon and its placement are written around the
 * event, not before it. One ask per burst, however many events. */
const JEV_EVENT_SETTLE_MS = 1_500;

/* The trend picture's own box, the room kept above and below its lines and
 * the size of a day's dot, in the box's own units; the stylesheet decides how
 * large the box draws. */
const JEV_SPARK_WIDTH = 64;
const JEV_SPARK_HEIGHT = 18;
const JEV_SPARK_PAD = 2;
const JEV_SPARK_DOT = 1.5;

/* The fewest days with a value a feature's week is drawn from: two points
 * are a line with no shape, three the fewest that can show a turn (t-6243
 * D4). Fewer, and the cell says how many days there are instead. */
const JEV_TREND_DAYS = 3;

/* What zo last said each seat's ledger holds. `null` until zo answers, and
 * it stays null on a zo too old to know the verb — the switches then stand
 * without numbers rather than not at all. */
let jevNumbers = null;
/* Whether the answer in hand carries the recent list — the dashboard's
 * ask; the card's answer does not. */
let jevNumbersRecent = false;
/* The ask in flight, so the card and the dashboard opening together cost
 * one zo, and what it was asked for. */
let jevNumbersAsking = null;
let jevNumbersAskingRecent = false;
let jevNumbersAskingScope = null;
/* When the last answer landed and what it cost, for the dashboard's own
 * freshness line — the cost is the exec boundary's, measured here rather
 * than promised. */
let jevNumbersAt = 0;
let jevNumbersCostMs = null;
/* What went wrong the last time zo was asked, or null. */
let jevNumbersError = null;
/* The feature whose drawer stands open (t-6277 D6), or null. */
let jevDrawerSeat = null;
let jevSettleTimer = null;
const jevViewsWired = new WeakSet();
/* The day's requests from this machine against the person's limit
 * (`jev_day`), `null` until asked or from a window too old to answer. */
let jevDay = null;

/* Whether the features nothing asked all week stand unfolded under their
 * row — the person's last choice, kept in this browser (t-6243 D1). A store
 * that cannot be read or written is a fold that starts closed. */
const JEV_UNUSED_OPEN_KEY = "zerocode.jev-unused-open.v1";

function jevReadUnusedOpen() {
  try {
    return localStorage.getItem(JEV_UNUSED_OPEN_KEY) === "1";
  } catch {
    return false;
  }
}

function jevRememberUnusedOpen(open) {
  try {
    localStorage.setItem(JEV_UNUSED_OPEN_KEY, open ? "1" : "0");
  } catch {
    // Unsaved, the fold is only closed again on the next start.
  }
}

let jevUnusedOpen = jevReadUnusedOpen();

/* Which numbers the dashboard counts (t-9091): this checkout — zo is asked
 * about the one the person is looking at, and that stays the default — or
 * every project with zo records, summed (`jev_scope::read`). The words are
 * the switch's; `scope` is the answer's own word for each. The person's last
 * choice is kept in this browser like the fold's, and a store that cannot be
 * read is this checkout. */
const JEV_SCOPES = Object.freeze([
  { scope: "workspace", key: "jev.scope.workspace", word: "이 작업 공간" },
  { scope: "projects", key: "jev.scope.projects", word: "모든 프로젝트" },
]);
const JEV_SCOPE_KEY = "zerocode.jev-scope.v1";

function jevReadScope() {
  try {
    const kept = localStorage.getItem(JEV_SCOPE_KEY);
    return JEV_SCOPES.some((one) => one.scope === kept) ? kept : JEV_SCOPES[0].scope;
  } catch {
    return JEV_SCOPES[0].scope;
  }
}

function jevRememberScope(scope) {
  try {
    localStorage.setItem(JEV_SCOPE_KEY, scope);
  } catch {
    // Unsaved, the next start counts this checkout again.
  }
}

let jevScopeChoice = jevReadScope();
/* What the numbers in hand counted (`jev_scope::JevScope`): the checkout by
 * name, whether it has zo records of its own, and the projects that do.
 * `null` until zo has answered. */
let jevScope = null;

function jevViewsShowing() {
  return tabs.some((tab) => tab.kind === "jev" && stillShowing(tab));
}

/* Ask zo for the ledgers' numbers and repaint every surface that draws them.
 *
 * One ask at a time: a second caller shares the one in flight when it asks
 * for no more than that one did, and waits behind it otherwise (a card's
 * ask without the recent list, then the dashboard's with it). `recent`
 * defaults to whether a dashboard is on stage, so the card's own refresh
 * feeds the dashboard when both are open. */
async function loadJevNumbers({ recent = jevViewsShowing() } = {}) {
  if (jevNumbersAsking) {
    if ((jevNumbersAskingRecent || !recent) && jevNumbersAskingScope === jevScopeChoice) return jevNumbersAsking;
    await jevNumbersAsking;
    return loadJevNumbers({ recent });
  }
  jevNumbersAskingRecent = recent;
  jevNumbersAskingScope = jevScopeChoice;
  jevNumbersAsking = (async () => {
    const began = performance.now();
    let held = null;
    try {
      [held, jevDay] = await Promise.all([
        invoke("jev_summary", { ...(recent ? { recent: JEV_RECENT_ROWS } : {}), scope: jevNumbersAskingScope }),
        // The dashboard's strip reads the day's count beside the ledgers: a
        // file's length, so it rides every refresh; the card draws none.
        recent ? invoke("jev_day").catch(() => null) : jevDay,
      ]);
      jevNumbersError = null;
    } catch (error) {
      held = null;
      jevNumbersError = String(error);
    }
    jevNumbersCostMs = Math.round(performance.now() - began);
    jevNumbersAt = Date.now();
    // Nothing new to draw — a zo that cannot count leaves the card exactly
    // as the settings answer left it, and the dashboard with the last
    // numbers it had and a line saying zo did not answer, rather than a
    // table of dashes over a count that was true a minute ago.
    if (held === null) {
      paintJevViews();
      return;
    }
    // The reading names what it counted (t-9091): the seats, and the scope
    // the dashboard says it counted them over.
    jevNumbers = held.seats;
    jevScope = held.scope;
    jevNumbersRecent = recent;
    if (typesafeState) paintTypeSafe(typesafeState);
    else paintJevViews();
  })();
  try {
    return await jevNumbersAsking;
  } finally {
    jevNumbersAsking = null;
  }
}

/* ---- the one switch ----
 * `smart.jev.enabled`, pressed on the settings card or over the dashboard's
 * table: one command (`set_jev_enabled`), one state (`typesafeState.jev`),
 * and every surface that wears the switch painted from it here. */

/* Turn Jev on or off from any surface that wears the switch: on consents
 * every folder and hands every feature its recommended mode; off sends
 * nothing. The card's own step runner, so a refused press is said the same
 * way wherever it was made. */
function setJevEnabled(on) {
  return runTypeSafe(() => invoke("set_jev_enabled", { on }), (state) => {
    const words = jevEnabledSaid(state);
    jevSayStatus(words);
    return words;
  });
}

/* What a press did, in the words of where Jev now stands. */
function jevEnabledSaid(state) {
  return state.jev?.on
    ? t("settings.typesafe.turnedOn", "켰습니다. 모든 폴더에서 기능마다 권장 설정으로 판단을 요청합니다.")
    : t("settings.typesafe.turnedOffAll", "껐습니다. 아무것도 보내지 않습니다.");
}

/* Every switch under `root` — the card's and each dashboard's — painted from
 * the one state: thrown while Jev is in use here, still while a step runs, and
 * the line under it shown while only some folders are consented, with how
 * many. */
function paintJevSwitches(root = document) {
  const jev = typesafeState?.jev ?? null;
  for (const input of root.querySelectorAll("[data-jev-switch]")) {
    input.checked = Boolean(jev?.on);
    input.disabled = typesafeBusy || !jev;
  }
  const partial = Boolean(jev?.on) && !jev.everywhere;
  for (const line of root.querySelectorAll("[data-jev-partial]")) {
    line.hidden = !partial;
    const said = line.querySelector("[data-jev-partial-said]");
    if (said) {
      said.textContent = partial
        ? t("settings.typesafe.partial", "일부 폴더({{count}}개)에서만 사용 중입니다.", { count: jevCount(jev.folders) })
        : "";
    }
    const press = line.querySelector("[data-jev-everywhere]");
    if (press) press.disabled = typesafeBusy;
  }
}

/* The card's switch row and the line under it, as a copy a dashboard wears:
 * the card's own markup, so its words are the card's, with no ids (the card
 * keeps its own) and no label pointing at the card's input — a press on the
 * copy throws the copy. */
function jevSwitchMirror() {
  const card = el("typesafe-card");
  const row = card?.querySelector("[data-jev-switch-row]");
  if (!row) return null;
  const holder = jevNode("div", "jev-switch", row.cloneNode(true));
  const partial = card.querySelector("[data-jev-partial]");
  if (partial) holder.append(partial.cloneNode(true));
  for (const node of holder.querySelectorAll("[id], [for]")) {
    node.removeAttribute("id");
    node.removeAttribute("for");
  }
  return holder;
}

/* A feature's words as the settings markup keeps them (`#jev-features`): its
 * name, one line of what it does and the paragraph of what it sends, each by
 * the catalog key it is written under — the markup is the one place the
 * Korean is written. `null` for a part the markup does not hold. */
function jevFeature(id) {
  const holder = el("jev-features")?.content.querySelector(`[data-jev-feature="${id}"]`) ?? null;
  const part = (name) => {
    const node = holder?.querySelector(`[data-jev-${name}]`);
    return node ? { key: node.dataset.i18n, source: node.textContent.replace(/\s+/g, " ").trim() } : null;
  };
  return { name: part("name"), summary: part("summary"), hint: part("hint") };
}

/* The line a judgment turned on, by the core's token
 * (`promote::Line::token`): what each one MEANS to a person is this page's,
 * said once here. A line that `wants` more samples holds a seat that has not
 * been measured yet; every other line holds one measured under its bar. */
const JEV_LINES = Object.freeze({
  too_few_rows: { key: "settings.typesafe.lineTooFewRows", word: "판단 기록이 더 쌓여야 합니다", wants: "rows" },
  answered: { key: "settings.typesafe.lineAnswered", word: "응답률이 기준에 못 미칩니다" },
  latency: { key: "settings.typesafe.lineLatency", word: "응답이 너무 느립니다" },
  schema: { key: "settings.typesafe.lineSchema", word: "형식이 잘못된 응답이 있었습니다" },
  too_few_compared: { key: "settings.typesafe.lineTooFewCompared", word: "정확도를 비교할 표본이 더 필요합니다", wants: "compared" },
  agreement: { key: "settings.typesafe.lineAgreement", word: "정확도가 기준에 못 미칩니다" },
  unlabeled: { key: "settings.typesafe.lineUnlabeled", word: "정확도를 잴 결과가 아직 없습니다", wants: "compared" },
  one_sided: { key: "settings.typesafe.lineOneSided", word: "틀렸다는 결과가 거의 없어 정확도를 믿을 수 없습니다" },
  too_few_baseline: { key: "settings.typesafe.lineTooFewBaseline", word: "가장 단순한 방식과 비교할 표본이 더 필요합니다", wants: "baseline" },
  baseline: { key: "settings.typesafe.lineBaseline", word: "가장 단순한 방식보다 낫지 않습니다" },
  labels: { key: "settings.typesafe.lineLabels", word: "사람의 평가에서 기존 방식이 더 나았습니다" },
  fallbacks: { key: "settings.typesafe.lineFallbacks", word: "연속으로 기존 방식으로 되돌아갔습니다" },
});

function jevLineWords(line) {
  const said = JEV_LINES[line];
  return said ? t(said.key, said.word) : "";
}

/* The version a change of version cut away from the judged window, when one
 * did — the reason a thin window gives for itself. */
function jevCutWords(held) {
  if (!held.verdict?.cutModel) return "";
  return t("settings.typesafe.seatCut", "이전 버전 {{cut}}의 기록은 제외", { cut: held.verdict.cutModel });
}

/* ---- the dashboard ---- */

function openJevView() {
  leavePagesForStage();
  openTab({ ...JEV_TAB });
}

/* An element whose words are a catalog key's, re-read by `applyLocale` when
 * the language moves. */
function jevText(key, fallback, tag = "span", className = "") {
  const node = document.createElement(tag);
  if (className) node.className = className;
  node.dataset.i18n = key;
  node.dataset.i18nSource = fallback;
  node.textContent = t(key, fallback);
  return node;
}

/* A tip whose words are a catalog key's — the attribute twin of `jevText`:
 * `applyLocale` re-reads it from the key when the language moves. */
function jevHint(key, fallback, node) {
  node.dataset.i18nTitle = key;
  node.dataset.i18nSourcedatatip = fallback;
  node.dataset.tip = t(key, fallback);
  return node;
}

function jevNode(tag, className, ...children) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  node.append(...children);
  return node;
}

/* The dashboard's columns, in order: the key each head is read under and
 * the attribute the cell is found by. One table, so the head and the row
 * cannot disagree about what stands where. A head whose number needs its
 * meaning said once carries that sentence as its tip (`tipKey`, `tip`). */
const JEV_COLUMNS = Object.freeze([
  { cell: "seat", key: "jev.col.seat", word: "기능" },
  { cell: "rows", key: "jev.col.rows", word: "판단 요청 (오늘 / 7일)" },
  { cell: "answered", key: "jev.col.answered", word: "응답률 (신뢰 하한)", tipKey: "jev.col.answeredTip", tip: "응답률은 판단을 요청한 것 중 응답을 받은 비율이고, 신뢰 하한은 표본이 적을 때를 감안한 보수적인 값입니다." },
  { cell: "latency", key: "jev.col.latency", word: "응답 시간" },
  { cell: "acted", key: "jev.col.acted", word: "실제 적용 · 정확도", tipKey: "jev.col.actedTip", tip: "정확도는 판단이 기존 방식의 판단이나 나중에 확인된 결과와 같았던 비율입니다." },
  { cell: "cost", key: "jev.col.cost", word: "비용 (USD)" },
  { cell: "status", key: "jev.col.why", word: "상태와 다음 단계" },
  { cell: "trend", key: "jev.col.trend", word: "지난 7일 (응답률 · 정확도)" },
]);

/* The two lines a feature's week draws, each read off one day's count: the
 * share that answered and the share that matched. Both are shares, so one
 * picture holds both on one scale; how long the answers took has its own
 * column (t-6243 D4). */
const JEV_TRENDS = Object.freeze([
  { line: "answered", key: "jev.trend.answered", word: "응답률", read: (day) => day.tally.answeredShare ?? null },
  { line: "agreement", key: "jev.trend.agreement", word: "정확도",
    read: (day) => (day.agreement?.compared ? day.agreement.agreed / day.agreement.compared : null) },
]);

/* Where a feature stands — one chip per row, one of four (t-6277 D8):
 * dormant (its mode asks nothing), recording, applying, or waiting on a
 * person, in the tone the stylesheet gives the state (`--jev-tone-*`). A feature that waits on a person wears
 * what it waits for — the key, or a folder's consent — and the strip over
 * the table counts the two as one (`word`). Read off what the switch does
 * and the numbers zo answered, never off a mode word (`jevSeatStatus`). */
const JEV_STATUSES = Object.freeze([
  { status: "applying", key: "jev.status.applying", word: "적용 중", counted: true },
  { status: "recording", key: "jev.status.recording", word: "기록 중", counted: true },
  { status: "blocked", key: "jev.status.blocked", word: "키·동의 필요", counted: true, waits: {
    key: { key: "jev.status.needsKey", word: "키 필요" },
    consent: { key: "jev.status.needsConsent", word: "동의 필요" },
  } },
  { status: "dormant", key: "jev.status.dormant", word: "꺼짐", counted: false },
]);

/* Of the features recording, the ones a judgment measured and held under a
 * line of their own — not a chip of its own (a feature under its bar is still
 * recording, and its sentence says why, in the tone of a miss), but a count
 * the strip keeps beside the states (t-6243 D1). */
const JEV_UNDER = Object.freeze({ stat: "under", key: "jev.status.under", word: "기준 미달" });

/* The strip's sums over every feature, before the states' counts. */
const JEV_TOTALS = Object.freeze([
  { stat: "today", key: "jev.stat.today", word: "오늘 요청" },
  { stat: "week", key: "jev.stat.week", word: "7일 요청" },
  { stat: "cost", key: "jev.stat.cost", word: "7일 비용" },
]);

/* The outcome tokens a feature's week can carry — zo's wire failures
 * (`SystemOneFailure::token`) and the door's refusals
 * (`zerocode_core::jev::door::Refused::token`) — each with its short word, the
 * sentence the card's key check says for it, and what a person does to clear
 * it: the key (`key`), the folder's consent (`consent`) or the day's limit
 * (`budget`). One table: the dashboard's chips and the card's check read it
 * (t-6243 D5), and `typesafe_settings.rs` holds it to zo's and the door's
 * tokens. The door's `off` is a mode word this page never spells. */
const JEV_TOKENS = Object.freeze({
  no_key: { key: "jev.token.noKey", word: "API 키 없음", fix: "key", saidKey: "settings.typesafe.noKey", said: "zo가 키를 찾지 못했습니다 — 키를 저장한 뒤 다시 확인하세요." },
  unauthorized: { key: "jev.token.unauthorized", word: "키 거절", fix: "key", saidKey: "settings.typesafe.unauthorized", said: "키가 거절되었습니다 — 키를 다시 확인하세요." },
  not_consented: { key: "jev.token.notConsented", word: "동의 안 된 폴더", fix: "consent" },
  budget: { key: "jev.token.budget", word: "하루 한도 초과", fix: "budget" },
  timeout: { key: "jev.token.timeout", word: "시간 초과" },
  schema: { key: "jev.token.schema", word: "형식 오류" },
  transport: { key: "jev.token.transport", word: "연결 실패" },
  overloaded: { key: "jev.token.overloaded", word: "서버 과부하" },
  rate_limited: { key: "jev.token.rateLimited", word: "요청 제한" },
  invalid_request: { key: "jev.token.invalidRequest", word: "잘못된 요청" },
  http: { key: "jev.token.http", word: "서버 오류" },
});

/* A token's row: its own, or its family's — a ledger keeps an unnamed HTTP
 * failure with its status (`http_503`, zo's `ledger_token`) and a schema
 * refusal with the rule it broke (`schema_…`, the core's
 * `names_a_schema_failure`). `null` for a token no row words. */
function jevTokenRow(token) {
  const family = token.startsWith("http_") ? "http" : token.startsWith("schema_") ? "schema" : token;
  return JEV_TOKENS[family] ?? null;
}

/* What a feature waits on a person for — the key (missing or refused) or a
 * folder's consent, whichever the door refused more often — when it was
 * refused for them at least as often as anything answered; `null` when it
 * waits on nobody. The day's limit clears itself tomorrow. */
function jevWaitsFor(held) {
  const fixes = { key: 0, consent: 0 };
  for (const one of held?.week.failures ?? []) {
    const fix = jevTokenRow(one.token)?.fix;
    if (fix in fixes) fixes[fix] += one.rows;
  }
  const refused = fixes.key + fixes.consent;
  if (refused === 0 || refused < held.week.answered) return null;
  return fixes.key >= fixes.consent ? "key" : "consent";
}

/* Where a feature stands, given the switch's `choice` (what its mode does)
 * and what zo counted (`held`, null when it has not): dormant when its mode
 * asks nothing; waiting on a person when the door refused it for the key or for
 * consent; applying when it acts — a person's `on`, or `auto` its evidence
 * raised; recording otherwise. */
function jevSeatStatus(held, choice) {
  if (choice && !choice.asks) return "dormant";
  if (jevWaitsFor(held)) return "blocked";
  return (held ? held.applies : choice?.applies) ? "applying" : "recording";
}

/* Whether a recording feature was measured and held under a line of its own
 * rather than still counting samples (`JEV_LINES`' `wants`). */
function jevUnderBar(held) {
  const line = held?.verdict?.line;
  return Boolean(line) && !JEV_LINES[line]?.wants && !held.applies;
}

/* The busiest first: today's requests, then the week's, then the card's own
 * order — the order a person reading "what is running" wants, and the one
 * the card keeps when nothing tells two features apart. */
function jevActivityOrder(ids, heldOf) {
  const at = new Map(ids.map((id, index) => [id, index]));
  const count = (id, window) => heldOf.get(id)?.[window].rows ?? 0;
  return [...ids].sort((a, b) =>
    count(b, "today") - count(a, "today") || count(b, "week") - count(a, "week") || at.get(a) - at.get(b));
}

function buildJevView() {
  const root = document.createElement("section");
  root.className = "file-view jev-view";
  // The first leaf's copy answers to this name, exactly as the surfaces the
  // markup declares do — `docHost` strips it from every clone after.
  root.id = "jev-view";
  root.hidden = true;
  root.dataset.i18nAria = "jev.title";
  root.setAttribute("aria-label", t("jev.title", "Jev 대시보드"));

  // The drawer a feature's row opens (t-6277 D6), first so it stays at the
  // top of the page while the page scrolls under it.
  root.append(jevNode("div", "jev-drawer-dock", jevBuildDrawer()));

  // The eyebrow is a name, the same in every language, so it takes no key.
  const heading = jevNode("div", "jev-heading",
    jevNode("p", "jev-eyebrow", document.createTextNode("TYPESAFE · JEV")),
    jevText("jev.title", "Jev 대시보드", "h1", "jev-title"),
    jevText("jev.about", "기능별로 AI에게 판단을 몇 번 맡겼고, 얼마나 정확했고, 자동 적용까지 무엇이 남았는지 보여 줍니다.", "p", "jev-about"));
  const freshness = jevNode("span", "jev-freshness");
  freshness.setAttribute("role", "status");
  const refresh = jevText("jev.refresh", "새로 고침", "button", "btn jev-refresh");
  refresh.type = "button";
  const settings = jevText("jev.openSettings", "설정에서 자세히", "button", "btn jev-settings-open");
  settings.type = "button";
  // What the numbers count (t-9091), beside the refresh that asks for
  // them: the checkout by name — or every project — and the one switch
  // between the two, in the room the heading's lines already take.
  const scopeName = jevNode("strong", "jev-scope-name");
  const scopeLine = jevNode("p", "jev-scope-line", jevText("jev.scope.label", "범위"), document.createTextNode(" "), scopeName);
  scopeLine.dataset.jevScopeLine = "";
  const choices = jevNode("div", "jev-scope-switch", ...JEV_SCOPES.map((one) => {
    const choice = jevText(one.key, one.word, "button", "jev-scope-choice");
    choice.type = "button";
    choice.dataset.jevScopeChoice = one.scope;
    return choice;
  }));
  choices.setAttribute("role", "group");
  choices.dataset.i18nAria = "jev.scope.choose";
  choices.setAttribute("aria-label", t("jev.scope.choose", "집계 범위"));
  root.append(jevNode("header", "jev-head", heading, jevNode("div", "jev-head-side",
    jevNode("div", "jev-actions", freshness, refresh, settings), jevNode("div", "jev-scope", scopeLine, choices))));
  // The settings card's switch, worn over the table (§6.1).
  const mirror = jevSwitchMirror();
  if (mirror) root.append(mirror);

  const status = jevNode("p", "jev-status");
  status.setAttribute("role", "status");
  status.hidden = true;
  const error = jevNode("p", "jev-error");
  error.setAttribute("role", "alert");
  error.hidden = true;
  root.append(status, error);

  // When the checkout has no zo records of its own, the projects that do
  // (t-9091), over the strip whose sums would otherwise read as nothing.
  const scopeNote = jevNode("p", "jev-scope-note");
  scopeNote.dataset.jevScopeNote = "";
  scopeNote.setAttribute("role", "status");
  scopeNote.hidden = true;
  root.append(scopeNote);

  // The strip over the table: the sums, how many features stand where, and
  // the day's requests against the limit (t-6243 D1).
  const summary = document.createElement("dl");
  summary.className = "jev-summary";
  summary.dataset.i18nAria = "jev.summary";
  summary.setAttribute("aria-label", t("jev.summary", "요약"));
  // Three groups: the sums, where the features stand, and the day's limit —
  // the second set apart from the first (t-6277 D10).
  const stat = (name, label, group) => {
    const holder = jevNode("div", "jev-stat", label, document.createElement("dd"));
    holder.dataset.jevStat = name;
    holder.dataset.group = group;
    return holder;
  };
  for (const total of JEV_TOTALS) summary.append(stat(total.stat, jevText(total.key, total.word, "dt"), "totals"));
  for (const [at, state] of [...JEV_STATUSES.filter((one) => one.counted), { ...JEV_UNDER, status: JEV_UNDER.stat }].entries()) {
    const holder = stat(state.status, jevText(state.key, state.word, "dt"), "states");
    holder.dataset.status = state.status;
    holder.classList.toggle("jev-stat--lead", at === 0);
    summary.append(holder);
  }
  const day = stat("day", jevText("jev.stat.day", "하루 한도", "dt"), "day");
  jevHint("jev.stat.dayTip", "이 컴퓨터가 오늘 Jev에 보낸 요청 수와 하루 한도(smart.jev.dailyRequests)입니다.", day);
  day.hidden = true;
  summary.append(day);
  root.append(summary);

  const head = document.createElement("tr");
  for (const column of JEV_COLUMNS) {
    const cell = jevText(column.key, column.word, "th", `jev-col-${column.cell}`);
    cell.scope = "col";
    if (column.tipKey) jevHint(column.tipKey, column.tip, cell);
    head.append(cell);
  }
  const body = document.createElement("tbody");
  body.dataset.jevRows = "";
  // The version the table's judgments come from, named once over it
  // (t-6243 D2); a row names one only when it is another.
  const caption = jevNode("caption", "jev-caption");
  caption.hidden = true;
  const table = jevNode("table", "jev-table", caption, jevNode("thead", "", head), body);
  root.append(jevNode("div", "jev-table-wrap", table));

  return root;
}

/* The drawer's frame, built once per view: the feature's name and id, one
 * line of what it does, and its parts — the last judgments, the comparisons,
 * what a screen feature's guards stopped, the model its numbers come from,
 * and what it sends (t-6277 D6). Filled by `paintJevDrawer`. */
function jevBuildDrawer() {
  const close = jevText("jev.drawer.close", "닫기", "button", "btn jev-drawer-close");
  close.type = "button";
  const head = jevNode("header", "jev-drawer-head",
    jevNode("div", "jev-drawer-name", jevNode("h2", "jev-drawer-title"), jevNode("p", "jev-drawer-id")), close);
  const part = (name, key, word, ...body) => {
    const section = jevNode("section", "jev-drawer-part", jevText(key, word, "h3", "jev-drawer-heading"), ...body);
    section.dataset.jevDrawerPart = name;
    return section;
  };
  const drawer = jevNode("aside", "jev-drawer", head,
    jevNode("p", "jev-drawer-summary"),
    jevText("jev.drawer.written", "설정 파일에 이 기능만 따로 정해 두어 스위치를 따르지 않습니다. 스위치를 다시 켜면 권장 설정으로 돌아갑니다.", "p", "jev-drawer-written"),
    part("recent", "jev.recent.title", "최근 판단", jevNode("ol", "jev-recent-list"),
      jevText("jev.recent.empty", "아직 판단 기록이 없습니다.", "p", "jev-recent-empty")),
    part("agreement", "jev.drawer.agreement", "정확도 비교", jevNode("dl", "jev-drawer-facts")),
    part("guards", "jev.drawer.guards", "화면 안전장치", jevNode("dl", "jev-drawer-facts")),
    part("version", "jev.drawer.version", "모델", jevNode("dl", "jev-drawer-facts")),
    part("sends", "jev.drawer.sends", "무엇을 보내나", jevNode("p", "jev-drawer-text")));
  drawer.hidden = true;
  return drawer;
}

/* The hands on one host, wired once: the leaf's own clone has its own
 * buttons, and a listener on the template would answer for no clone. */
function wireJevView(host) {
  if (jevViewsWired.has(host)) return;
  jevViewsWired.add(host);
  host.querySelector(".jev-refresh").addEventListener("click", () => {
    void refreshJevDashboard({ force: true });
  });
  host.querySelector(".jev-settings-open").addEventListener("click", () => {
    setSettingsOpen(true, "jev-enabled");
  });
  host.querySelector("[data-jev-switch]")?.addEventListener("change", (event) => {
    void setJevEnabled(event.target.checked);
  });
  host.querySelector("[data-jev-everywhere]")?.addEventListener("click", () => {
    void setJevEnabled(true);
  });
  host.querySelector(".jev-scope-switch").addEventListener("click", (event) => {
    const choice = event.target.closest("[data-jev-scope-choice]");
    if (choice) void setJevScope(choice.dataset.jevScopeChoice);
  });
  host.querySelector("[data-jev-rows]").addEventListener("click", (event) => {
    const chip = event.target.closest("[data-jev-fix]");
    if (chip) {
      jevFix(chip.dataset.jevFix);
      return;
    }
    const row = event.target.closest("[data-jev-dash-row]");
    if (row) jevToggleDrawer(host, row.dataset.jevDashRow);
  });
  host.querySelector(".jev-drawer-close").addEventListener("click", () => jevToggleDrawer(host, null));
  host.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && jevDrawerSeat !== null) {
      event.stopPropagation();
      jevToggleDrawer(host, null);
    }
  });
  // The drawer is as tall as the page's window onto the table, whatever the
  // window's size: one measure per resize, none per paint.
  new ResizeObserver(() => {
    host.style.setProperty("--jev-drawer-height", `${host.clientHeight}px`);
  }).observe(host);
}

/* Count another scope (t-9091): kept for the next open, drawn as chosen at
 * once, and asked for — the numbers in hand stand until the answer lands. */
function setJevScope(scope) {
  if (scope === jevScopeChoice || !JEV_SCOPES.some((one) => one.scope === scope)) return undefined;
  jevScopeChoice = scope;
  jevRememberScope(scope);
  paintJevViews();
  return loadJevNumbers({ recent: jevViewsShowing() });
}

/* The line over the strip: what the numbers in hand counted — the checkout
 * by name, its whole path as the tip, or how many projects and which — the
 * switch as chosen, and the note under it when the checkout has no zo
 * records of its own: which projects do, in the last days zo counts. */
function paintJevScope(view) {
  for (const choice of view.querySelectorAll("[data-jev-scope-choice]")) {
    choice.setAttribute("aria-pressed", String(choice.dataset.jevScopeChoice === jevScopeChoice));
  }
  const line = view.querySelector("[data-jev-scope-line]");
  const note = view.querySelector("[data-jev-scope-note]");
  line.hidden = jevScope === null;
  note.hidden = true;
  if (jevScope === null) return;
  const projects = jevScope.projects ?? [];
  const summed = jevScope.scope === "projects" && projects.length > 0;
  line.querySelector(".jev-scope-name").textContent = summed
    ? t("jev.scope.counted", "프로젝트 {{count}}곳", { count: jevCount(projects.length) })
    : jevScope.workspaceName;
  line.dataset.tip = summed ? projects.map((one) => one.path).join("\n") : jevScope.workspace;
  if (summed || jevScope.recorded) return;
  const days = jevCount(jevScope.windowDays);
  note.textContent = projects.length > 0
    ? t("jev.scope.elsewhere", "이 작업 공간에는 zo 판단 기록이 없습니다 — 최근 {{days}}일 기록이 있는 프로젝트 {{count}}곳: {{names}}", {
      days, count: jevCount(projects.length), names: projects.map((one) => one.name).join(", "),
    })
    : t("jev.scope.nowhere", "이 작업 공간에는 zo 판단 기록이 없습니다 — 최근 {{days}}일 기록이 있는 프로젝트도 없습니다", { days });
  note.hidden = false;
}

/* What the dashboard says under its head: the last press of the switch. */
function jevSayStatus(words) {
  for (const view of document.querySelectorAll(".jev-view")) {
    const status = view.querySelector(".jev-status");
    status.textContent = words;
    status.hidden = !words;
  }
}

/* The stage's door: paint what is in hand, then ask zo once — on the first
 * open, and again when what is in hand is older than the beat or was the
 * card's answer without the recent list. A tab switched back to within the
 * beat costs no process. */
function paintJevStage(tab) {
  const view = docHost(tab.pane, "jev");
  paintJevView(view);
  void refreshJevDashboard();
}

async function refreshJevDashboard({ force = false } = {}) {
  const stale = Date.now() - jevNumbersAt >= JEV_POLL_MS;
  if (!force && jevNumbers !== null && jevNumbersRecent && !stale) return;
  // The switches' standing comes with the settings answer, the numbers with
  // zo's; the card's own refresh asks both and hands the numbers here.
  if (!typesafeState) await refreshTypeSafe();
  await loadJevNumbers({ recent: true });
}

/* The dashboard's slow beat while it is on stage (the constant says why
 * thirty seconds); off stage or with the window hidden it stops. */
const jevPoll = idlePoller({
  wanted: jevViewsShowing,
  every: JEV_POLL_MS,
  tick: () => void loadJevNumbers({ recent: true }),
  onResume: () => void refreshJevDashboard(),
});

/* The seats this window writes (summon, placement, stall, effort) write
 * their rows around the orchestration ledger's own events, so the event is
 * the moment to read them back — settled, because the rows land after it. */
listen("ledger:changed", () => {
  if (!jevViewsShowing()) return;
  window.clearTimeout(jevSettleTimer);
  jevSettleTimer = window.setTimeout(() => {
    jevSettleTimer = null;
    if (jevViewsShowing()) void loadJevNumbers({ recent: true });
  }, JEV_EVENT_SETTLE_MS);
});

function paintJevViews() {
  for (const tab of tabs) {
    if (tab.kind !== "jev") continue;
    const host = groups.get(tab.pane)?.jevView;
    if (host) paintJevView(host);
  }
}

/* What a feature judges, in one sentence: the first of the paragraph its
 * drawer shows whole (`…Hint`, the same key, in the language in force), cut
 * where a sentence ends in any of the catalog's languages — a stop and a
 * space or the end (ko, en, es), or an ideographic stop (ja, zh). A version
 * like 1.5 or a host like api.typesafe.ai is not an end (t-6277 D7). */
function jevHintLead(id) {
  const hint = jevFeature(id).hint;
  if (!hint) return "";
  const words = t(hint.key, hint.source);
  const end = words.search(/[.!?](\s|$)|[。！？]/u);
  return end < 0 ? words : words.slice(0, end + 1).trim();
}

/* A feature's name, in the language in force: the settings markup's
 * (`jevFeature`), so the dashboard carries no second list of names. */
function jevSeatName(id) {
  const name = jevFeature(id).name;
  return name ? t(name.key, name.source) : id;
}

/* Under this many rows a lower bound is the width of its interval rather
 * than a measurement — one answer in one bounds at 21%, which a person reads
 * as a failing feature — so the dashboard says the sample instead of a share
 * (t-6243 D3, the plan's "하한은 n≥5부터"). */
const JEV_SAMPLE_FLOOR = 5;

/* A sample too small for a share, said as its size. */
function jevSampleWords(count) {
  return t("jev.sample", "표본 {{count}}건", { count: jevCount(count) });
}

function jevPercent(share) {
  if (share === null || share === undefined) return "—";
  return `${Math.round(share * 100)}%`;
}

/* A seat's line for acting by itself, which the core keeps per thousand
 * (`riseFloorPermille`), said as the percent a person reads; a line between
 * two whole percents keeps its one decimal rather than being rounded across. */
function jevFloor(permille) {
  return `${Number((permille / 10).toFixed(1))}%`;
}

/* A week of judgments costs cents, so a cost keeps three significant digits
 * whatever its size (plan §시각화 의미 정합): $0.0173, $0.00109, $12.3. */
const JEV_COST_DIGITS = 3;
/* The number formats of the language in force, made once per language: a
 * formatter is not free to build, and a paint formats every cell. */
let jevNumbersLang = null;
let jevCountFormat = null;
let jevCostFormat = null;

function jevFormats() {
  const lang = document.documentElement.lang || undefined;
  if (jevCountFormat === null || lang !== jevNumbersLang) {
    jevNumbersLang = lang;
    jevCountFormat = new Intl.NumberFormat(lang);
    jevCostFormat = new Intl.NumberFormat(lang, { maximumSignificantDigits: JEV_COST_DIGITS });
  }
  return { count: jevCountFormat, cost: jevCostFormat };
}

/* A count as the language in force groups its digits (1,321). */
function jevCount(value) {
  return value === null || value === undefined ? "—" : jevFormats().count.format(value);
}

function jevMs(ms) {
  return ms === null || ms === undefined ? "—" : jevCount(ms);
}

function jevCost(usd) {
  if (usd === null || usd === undefined) return "—";
  return `$${jevFormats().cost.format(usd)}`;
}

/* How long a seat's answers take: the typical wait, and the slow one beside
 * it when there is one. */
function jevLatencyWords(week) {
  if (week.p50Ms === null || week.p50Ms === undefined) return "—";
  if (week.p95Ms === null || week.p95Ms === undefined) return `${jevMs(week.p50Ms)} ms`;
  return t("jev.latency", "{{p50}} ms (느릴 때 {{p95}})", { p50: jevMs(week.p50Ms), p95: jevMs(week.p95Ms) });
}

/* The week as one picture, every line on the same 0–100% scale over the
 * days, the newest at the right; a day with nothing to say leaves a gap
 * rather than a zero. Each line's values ride on its group for a reader or a
 * test; the picture is the stylesheet's. */
function jevTrendPicture(series) {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("class", "jev-spark");
  svg.setAttribute("viewBox", `0 0 ${JEV_SPARK_WIDTH} ${JEV_SPARK_HEIGHT}`);
  svg.setAttribute("aria-hidden", "true");
  const days = Math.max(0, ...series.map(({ values }) => values.length));
  const step = days > 1 ? JEV_SPARK_WIDTH / (days - 1) : 0;
  const pad = JEV_SPARK_PAD;
  for (const { line, values } of series) {
    const group = document.createElementNS("http://www.w3.org/2000/svg", "g");
    group.setAttribute("class", `jev-spark-${line}`);
    const points = [];
    values.forEach((value, at) => {
      if (value === null) return;
      points.push([at * step, JEV_SPARK_HEIGHT - pad - value * (JEV_SPARK_HEIGHT - pad * 2)]);
    });
    if (points.length > 1) {
      const polyline = document.createElementNS("http://www.w3.org/2000/svg", "polyline");
      polyline.setAttribute("class", "jev-spark-line");
      polyline.setAttribute("points", points.map(([x, y]) => `${x.toFixed(1)},${y.toFixed(1)}`).join(" "));
      group.append(polyline);
    }
    for (const [x, y] of points) {
      const dot = document.createElementNS("http://www.w3.org/2000/svg", "circle");
      dot.setAttribute("class", "jev-spark-dot");
      dot.setAttribute("cx", x.toFixed(1));
      dot.setAttribute("cy", y.toFixed(1));
      dot.setAttribute("r", String(JEV_SPARK_DOT));
      group.append(dot);
    }
    group.dataset.line = line;
    group.dataset.points = String(points.length);
    group.dataset.values = values.map((value) => (value === null ? "" : String(value))).join(",");
    svg.append(group);
  }
  return svg;
}

/* A feature's week: the picture and each line's latest value under it, once
 * there are enough days to draw; how many days there are, until then. */
function jevTrendCell(held) {
  const cell = jevNode("div", "jev-trends");
  const days = held.days ?? [];
  const known = days.filter((day) => day.tally.rows > 0).length;
  if (known < JEV_TREND_DAYS) {
    cell.textContent = known === 0 ? "—" : t("jev.trendShort", "{{days}}일치만 있음", { days: jevCount(known) });
    return cell;
  }
  const series = JEV_TRENDS.map((trend) => ({ line: trend.line, values: days.map((day) => trend.read(day)) }));
  const legend = jevNode("div", "jev-trend-legend");
  for (const [at, trend] of JEV_TRENDS.entries()) {
    const last = series[at].values.filter((value) => value !== null).at(-1);
    const item = jevNode("span", `jev-trend-last jev-trend-${trend.line}`);
    item.textContent = `${t(trend.key, trend.word)} ${last === undefined ? "—" : jevPercent(last)}`;
    legend.append(item);
  }
  cell.append(jevTrendPicture(series), legend);
  return cell;
}

/* The samples a feature still owes before the judge can speak, by the line
 * that wants them: the judged window while it fills, then the compared marks.
 * Keyed by `JEV_LINES`' `wants`. */
const JEV_SAMPLES = Object.freeze({
  rows: { key: "jev.window", word: "판정 표본 {{rows}}/{{wanted}}건", owedKey: "jev.owed.rows", owed: "판정까지 {{count}}건" },
  compared: { key: "jev.compared", word: "비교 표본 {{rows}}/{{wanted}}건", owedKey: "jev.owed.compared", owed: "정확도 판정까지 {{count}}건" },
  baseline: { key: "jev.baselineCompared", word: "기준 비교 표본 {{rows}}/{{wanted}}건", owedKey: "jev.owed.baseline", owed: "기준 비교까지 {{count}}건" },
});

/* The cheapest reader each feature is held against, by the table's word
 * (`Baseline::kind`, t-6342): what it would have scored had it always given
 * one answer, or had it been today's rule. A feature with no such reader
 * names none. */
const JEV_BASELINES = Object.freeze({
  always_same: { key: "jev.baseline.always_same", word: "늘 같은 답" },
  todays_rule: { key: "jev.baseline.todays_rule", word: "지금의 규칙" },
});

/* The version the table's judgments come from: the pin when the person
 * pinned one, else the version most features' newest answers named. `null`
 * while nothing has answered. */
function jevHeadModel(numbers) {
  if (typesafeState?.model?.pinned) return typesafeState.model.model;
  const tally = new Map();
  for (const held of numbers) if (held.model) tally.set(held.model, (tally.get(held.model) ?? 0) + 1);
  let best = null;
  for (const [model, count] of tally) if (best === null || count > tally.get(best)) best = model;
  return best;
}

function paintJevCaption(view, numbers) {
  const caption = view.querySelector(".jev-caption");
  const model = jevHeadModel(numbers);
  caption.hidden = !model;
  caption.textContent = !model ? "" : typesafeState?.model?.pinned
    ? t("settings.typesafe.seatVersionPinned", "고정 모델 {{model}}", { model })
    : t("settings.typesafe.seatVersion", "모델 {{model}}", { model });
}

/* A feature's state as a person reads it (t-6243 D2): one chip in the
 * state's tone and, from its line on, the samples still owed while the judge
 * wants some and one sentence of why, unless the bar already is the why; and
 * a version only when it is not the one over the table — each said once, in
 * as few lines as a row can hold. */
function jevStatusCell(held, choice, standing, head) {
  const status = jevSeatStatus(held, choice);
  const state = JEV_STATUSES.find((one) => one.status === status);
  const said = state.waits?.[jevWaitsFor(held)] ?? state;
  const chip = jevNode("span", "jev-chip");
  chip.dataset.status = status;
  chip.textContent = t(said.key, said.word);
  // Summed over projects, a feature has no one judgment to wait on or cite:
  // each project judges its own rows (t-9091), and the line says in how
  // many of them it acts instead.
  const across = held.across ?? null;
  const owed = across?.summed ? null : jevSamplesOwed(held, standing);
  const lead = jevNode("div", "jev-status-lead", chip);
  if (owed) lead.append(owed);
  const reason = across?.summed ? "" : jevStatusReason(held, status, choice, owed !== null);
  if (reason) {
    const line = jevNode("p", "jev-status-reason", document.createTextNode(reason));
    // A feature measured under its bar says so in the tone of a miss.
    line.classList.toggle("is-under", status === "recording" && jevUnderBar(held));
    lead.append(line);
  }
  if (across) {
    const acts = jevNode("p", "jev-status-reason jev-across");
    acts.dataset.jevAcross = "";
    acts.textContent = t("jev.scope.across", "{{projects}}곳 중 {{applying}}곳 적용 중", {
      projects: jevCount(across.projects), applying: jevCount(across.applying),
    });
    lead.append(acts);
  }
  const cell = jevNode("div", "jev-status", lead);
  const other = held.model && held.model !== head
    ? t("settings.typesafe.seatVersion", "모델 {{model}}", { model: held.model }) : "";
  for (const fact of [other, jevCutWords(held)]) {
    if (fact) cell.append(jevNode("p", "jev-status-fact", document.createTextNode(fact)));
  }
  return cell;
}

/* The one sentence under the chip: what keeps the feature where it is. A
 * switch a person turned on applies whatever the judge says, so its sentence
 * says whose doing that is and what the judge would have said — except where
 * the samples still owed are `counting`: the bar says that, once. */
function jevStatusReason(held, status, choice, counting) {
  const because = held.verdict ? jevLineWords(held.verdict.line) : "";
  if (status === "dormant") {
    return held.week.rows > 0 ? t("jev.switchedOff", "꺼져 있어 새 판단을 요청하지 않습니다") : "";
  }
  if (status === "blocked") return jevBlockedWords(held);
  // Switched on and asked nothing all week: the row's dashes are that, not
  // a feature failing.
  if (held.week.rows === 0) return t("jev.noRequests", "지난 7일 판단 요청이 없었습니다");
  if (status === "applying") {
    if (!choice?.applies) return t("jev.risen", "근거가 충분해 자동으로 켜졌습니다");
    if (!held.verdict) return t("jev.byHandNoJudge", "직접 켰습니다 — 이 기능은 자동 적용 대상이 아닙니다");
    if (counting) return t("jev.byHandShort", "직접 켰습니다");
    return because
      ? t("jev.byHand", "직접 켰습니다 — {{because}}", { because })
      : t("jev.byHandClear", "직접 켰습니다 — 자동 적용 기준도 넘었습니다");
  }
  if (because) return counting ? "" : because;
  if (!held.verdict) return t("jev.neverRises", "자동 적용 대상이 아니라 기록만 합니다");
  // The judge found nothing to hold it on: it acts at the next judgment
  // under `auto`, and never under a switch that only records.
  return choice?.automatic
    ? t("jev.risingNext", "기준을 넘었습니다 — 다음 판정에서 자동 적용됩니다")
    : t("jev.risingHeld", "기준을 넘었지만 기록만으로 설정되어 있습니다");
}

/* Why the door refused a feature that waits on a person, in a sentence:
 * the key, or the folder's consent (`jevWaitsFor`). */
function jevBlockedWords(held) {
  return jevWaitsFor(held) === "consent"
    ? t("jev.needs.consent", "동의하지 않은 폴더라 판단을 보내지 않았습니다")
    : t("jev.needs.key", "API 키가 없거나 거절되어 판단을 요청하지 못했습니다");
}

/* The samples a feature still owes before the judge can speak, as a bar and
 * how many remain until the check: the judged window while it fills, then the
 * compared marks.
 * Nothing once the judge has what it wants — the sentence then says what it
 * found. The countdown is the core's while the window fills
 * (`rowsToNextJudgment`), so the bar and the judgment land on the same row. */
function jevSamplesOwed(held, standing) {
  const wants = JEV_LINES[held.verdict?.line ?? ""]?.wants;
  const sample = JEV_SAMPLES[wants];
  if (!sample || !held.judged) return null;
  const [have, want] = wants === "rows"
    ? [held.judged.window.rows, held.judged.windowWanted]
    : wants === "baseline"
      ? [held.judged.agreement?.baselineCompared ?? 0, standing?.agreementRowsWanted]
      : [held.judged.agreement?.compared ?? 0, standing?.agreementRowsWanted];
  if (!want) return null;
  const owed = wants === "rows" ? (held.rowsToNextJudgment ?? Math.max(0, want - have)) : Math.max(0, want - have);
  const bar = jevNode("div", "jev-progress", jevNode("span", "jev-progress-fill"));
  bar.setAttribute("role", "progressbar");
  bar.setAttribute("aria-valuemin", "0");
  bar.setAttribute("aria-valuemax", String(want));
  bar.setAttribute("aria-valuenow", String(Math.min(have, want)));
  const said = t(sample.key, sample.word, { rows: jevCount(have), wanted: jevCount(want) });
  bar.setAttribute("aria-label", said);
  bar.dataset.tip = said;
  bar.style.setProperty("--jev-filled", String(Math.min(1, have / want)));
  const label = jevNode("span", "jev-progress-label");
  label.textContent = t(sample.owedKey, sample.owed, { count: jevCount(owed) });
  return jevNode("div", "jev-owed", bar, label);
}

/* One token's chip: its words and how many rows carried it; a button to the
 * fix when a person clears it, a word otherwise. A token no row words shows
 * as itself. */
function jevTokenChip(one) {
  const row = jevTokenRow(one.token);
  const chip = document.createElement(row?.fix ? "button" : "span");
  chip.className = "jev-token";
  chip.dataset.token = one.token;
  if (row?.fix) {
    chip.type = "button";
    chip.dataset.jevFix = row.fix;
  }
  chip.textContent = `${row ? t(row.key, row.word) : one.token} ${jevCount(one.rows)}`;
  return chip;
}

/* Take a person to where a token is cleared: the key's own field for a key
 * missing or refused; the switch for a folder not consented, whose line
 * offers every folder in one press (§6.1); the key card, saying what to
 * change, for a day's limit spent — it has no field of its own. */
function jevFix(fix) {
  if (fix === "key") {
    setSettingsOpen(true, "typesafe-key-input");
    return;
  }
  setSettingsOpen(true, fix === "consent" ? "jev-enabled" : "typesafe-status");
  paintTypeSafeStatus(fix === "consent"
    ? t("jev.help.consent", "동의한 폴더에서만 판단을 보냅니다. 「모든 폴더에서 사용」을 누르면 모든 폴더에서 보냅니다.")
    : t("jev.help.budget", "오늘 보낼 수 있는 판단 요청을 다 썼습니다. 하루 한도는 zo 설정 파일(settings.json)의 smart.jev.dailyRequests입니다."));
}

/* How often the judgment matched, over the judged window — or, while the
 * comparisons are under half of what the judge wants before accuracy may
 * speak, how many there are: 0/3 is a feature not yet judged, not a failing
 * one (t-6243 D3). */
function jevAgreementWords(held, standing) {
  const agreement = held.judged?.agreement;
  if (!agreement || !agreement.compared) return "—";
  const wanted = standing?.agreementRowsWanted;
  if (wanted && agreement.compared * 2 < wanted) return jevSampleWords(agreement.compared);
  return `${jevCount(agreement.agreed)}/${jevCount(agreement.compared)} (${jevPercent(agreement.lowerBound)})`;
}

/* Draw the strip, the table and the recent list from what is in hand: the
 * switches from the settings answer, the numbers from zo's. The features in
 * use lead, busiest first; the ones nothing asked all week fold into one row
 * after them, unfolded when the person last left them so (t-6243 D1). Rows
 * are keyed by feature and updated in place, and the table is not reordered
 * while something in it has focus, so a repaint never moves a switch
 * somebody is using. */
function paintJevView(view) {
  wireJevView(view);
  const switches = typesafeState?.switches ?? [];
  const numbers = jevNumbers ?? [];
  const order = switches.length > 0 ? switches.map((row) => row.id) : numbers.map((row) => row.id);
  const heldOf = new Map(numbers.map((one) => [one.id, one]));
  // A feature zo did not count stands with its dashes, never folded: only a
  // count of nothing says a feature was not used.
  const unused = order.filter((id) => heldOf.get(id)?.week.rows === 0);
  const inUse = jevActivityOrder(order.filter((id) => !unused.includes(id)), heldOf);
  const body = view.querySelector("[data-jev-rows]");
  const drawn = [];
  for (const id of [...inUse, ...(jevUnusedOpen ? unused : [])]) {
    const row = jevDashRow(body, id);
    paintJevRow(row, id, heldOf.get(id) ?? null, switches.find((one) => one.id === id) ?? null, jevHeadModel(numbers));
    drawn.push(row);
  }
  const fold = unused.length > 0 ? jevFoldRow(body, unused.length) : null;
  if (fold) drawn.splice(inUse.length, 0, fold);
  // Nothing in use all week: the table says why before the fold does
  // (t-6277 D10) — Jev is off, or on and nothing asked.
  const empty = inUse.length === 0 && numbers.length > 0 ? jevEmptyRow(body) : null;
  if (empty) drawn.unshift(empty);
  for (const row of body.querySelectorAll("[data-jev-dash-row], [data-jev-fold], [data-jev-empty]")) {
    if (!drawn.includes(row)) row.remove();
  }
  jevArrange(body, drawn);
  paintJevSwitches(view);
  paintJevScope(view);
  paintJevCaption(view, numbers);
  paintJevSummary(view, order, heldOf);
  paintJevFreshness(view);
  // A feature the table no longer names has no drawer to stand open.
  const open = order.includes(jevDrawerSeat) ? jevDrawerSeat : null;
  paintJevDrawer(view, open, open === null ? null : heldOf.get(open) ?? null);
}

/* Open a feature's drawer, or close it: the row that is open closes it, and
 * `null` closes whatever is open. What it draws is in hand — opening asks zo
 * nothing (t-6277 D6). Focus goes to the drawer's close and comes back to
 * the row that opened it. */
function jevToggleDrawer(view, id) {
  const was = jevDrawerSeat;
  jevDrawerSeat = id === null || id === was ? null : id;
  paintJevViews();
  if (jevDrawerSeat !== null) {
    view.querySelector(".jev-drawer-close")?.focus({ preventScroll: true });
  } else if (was !== null) {
    view.querySelector(`[data-jev-dash-row="${was}"] .jev-row-open`)?.focus({ preventScroll: true });
  }
}

/* One `<dl>` of facts: a term per row, by its catalog key, and what the
 * feature's numbers say for it. Rebuilt whole: a handful of rows. */
function jevFacts(list, facts) {
  list.replaceChildren(...facts.flatMap(({ key, word, said }) => [
    jevText(key, word, "dt"), jevNode("dd", "", document.createTextNode(said)),
  ]));
}

/* The open feature's drawer (`id`, or null for none), from what is in hand:
 * the settings answer's standing, zo's numbers (`held`, null until zo has
 * counted) and the markup's words. */
function paintJevDrawer(view, id, held) {
  const drawer = view.querySelector(".jev-drawer");
  drawer.hidden = id === null;
  for (const row of view.querySelectorAll("[data-jev-dash-row]")) {
    const open = row.dataset.jevDashRow === id;
    row.classList.toggle("is-open", open);
    row.querySelector(".jev-row-open")?.setAttribute("aria-expanded", String(open));
  }
  if (id === null) return;
  const feature = jevFeature(id);
  const standing = jevSeat(typesafeState ?? {}, id);
  const name = jevSeatName(id);
  drawer.setAttribute("aria-label", name);
  drawer.querySelector(".jev-drawer-title").textContent = name;
  drawer.querySelector(".jev-drawer-id").textContent = id;
  drawer.querySelector(".jev-drawer-summary").textContent = feature.summary ? t(feature.summary.key, feature.summary.source) : "";
  drawer.querySelector(".jev-drawer-written").hidden = !standing?.written;
  const part = (name) => drawer.querySelector(`[data-jev-drawer-part="${name}"]`);

  const decisions = held?.recent ?? [];
  const now = Date.now();
  part("recent").querySelector(".jev-recent-list").replaceChildren(...decisions.map((decision) => jevDecisionNode(decision, now)));
  part("recent").querySelector(".jev-recent-empty").hidden = decisions.length > 0;

  // How often its judgment matched: over the window the judge reads, and
  // over the week — with the control rows the comparison borrowed.
  const matched = (agreement) => t("jev.drawer.matched", "{{agreed}}/{{compared}}건 일치", {
    agreed: jevCount(agreement.agreed), compared: jevCount(agreement.compared),
  });
  const judged = held?.judged?.agreement;
  const week = held?.agreementWeek;
  const comparisons = [];
  if (judged?.compared) {
    const bound = judged.lowerBound === null || judged.lowerBound === undefined ? ""
      : ` · ${t("jev.bound", "신뢰 하한 {{pct}}", { pct: jevPercent(judged.lowerBound) })}`;
    const control = judged.controlRows
      ? ` · ${t("jev.drawer.controlRows", "대조 표본 {{count}}건 포함", { count: jevCount(judged.controlRows) })}` : "";
    comparisons.push({ key: "jev.drawer.judged", word: "판정 표본", said: `${matched(judged)}${bound}${control}` });
  }
  if (week?.compared) comparisons.push({ key: "jev.drawer.week", word: "지난 7일", said: matched(week) });
  if (comparisons.length === 0) {
    comparisons.push({ key: "jev.drawer.judged", word: "판정 표본", said: t("jev.drawer.noComparison", "아직 비교한 판단이 없습니다.") });
  }
  // The cheapest reader over the same marks — the bar a feature has to beat,
  // not only its own — and the judgments whose outcome could not tell right
  // from wrong (t-6342).
  const reader = JEV_BASELINES[held?.baseline];
  const beside = judged?.baselineCompared ? judged : week?.baselineCompared ? week : null;
  if (reader && beside) {
    comparisons.push({ key: "jev.drawer.baseline", word: "가장 단순한 방식", said: t("jev.drawer.baselineSaid", "{{kind}}: {{agreed}}/{{compared}}건 일치", {
      kind: t(reader.key, reader.word), agreed: jevCount(beside.baselineAgreed), compared: jevCount(beside.baselineCompared),
    }) });
  }
  if (week?.notCompared) {
    comparisons.push({ key: "jev.drawer.notCompared", word: "비교하지 않은 판단", said: t("jev.drawer.notComparedSaid", "{{count}}건 (맞았는지 가를 결과가 없음)", {
      count: jevCount(week.notCompared),
    }) });
  }
  jevFacts(part("agreement").querySelector("dl"), comparisons);

  // A feature whose rows name controls presses them: what its guards stopped
  // and what it handed to the person. One that names none shows no part.
  const guards = held?.week.guards;
  const controls = held?.week.controls;
  part("guards").hidden = !controls?.named;
  if (controls?.named) {
    jevFacts(part("guards").querySelector("dl"), [
      { key: "jev.drawer.instructed", word: "화면의 글이 지시해 멈춤", said: jevCount(guards?.instructed ?? 0) },
      { key: "jev.drawer.walled", word: "로그인·오류 화면이라 멈춤", said: jevCount(guards?.walled ?? 0) },
      { key: "jev.drawer.held", word: "되돌릴 수 없는 조작을 사람에게", said: t("jev.drawer.heldOf", "{{held}}건 (조작 판단 {{named}}건 중)", {
        held: jevCount(controls.destructiveHeld ?? 0), named: jevCount(controls.named),
      }) },
    ]);
  }

  // The model its numbers come from, and the version a change cut away.
  const models = [];
  const model = typesafeState?.model?.pinned ? held?.askedModel : held?.model;
  if (model) {
    models.push({ key: "jev.drawer.model", word: "응답한 모델", said: typesafeState?.model?.pinned
      ? t("settings.typesafe.seatVersionPinned", "고정 모델 {{model}}", { model })
      : t("settings.typesafe.seatVersion", "모델 {{model}}", { model }) });
  }
  const cut = jevCutWords(held ?? {});
  if (cut) models.push({ key: "jev.drawer.cut", word: "버전 변경", said: cut });
  if (models.length === 0) {
    models.push({ key: "jev.drawer.model", word: "응답한 모델", said: t("jev.drawer.noModel", "아직 응답한 판단이 없습니다.") });
  }
  jevFacts(part("version").querySelector("dl"), models);

  part("sends").querySelector(".jev-drawer-text").textContent = feature.hint ? t(feature.hint.key, feature.hint.source) : "";
}

/* One feature's row, built once: a cell per column. */
function jevDashRow(body, id) {
  let row = body.querySelector(`[data-jev-dash-row="${id}"]`);
  if (row) return row;
  row = document.createElement("tr");
  row.dataset.jevDashRow = id;
  for (const column of JEV_COLUMNS) {
    const cell = document.createElement("td");
    cell.dataset.jevCell = column.cell;
    row.append(cell);
  }
  body.append(row);
  return row;
}

function paintJevRow(row, id, held, standing, head) {
  const cell = (name) => row.querySelector(`[data-jev-cell="${name}"]`);
  // The name opens the feature's drawer — a button, so the row opens from
  // the keyboard as well as from a click anywhere on it (t-6277 D6) — with
  // the feature's id under it, small, and as its tip what the feature judges
  // in one sentence (D7).
  const seat = cell("seat");
  let open = seat.querySelector(".jev-row-open");
  if (!open) {
    open = document.createElement("button");
    open.type = "button";
    open.className = "jev-row-open";
    seat.replaceChildren(open, jevNode("span", "jev-row-meta", jevNode("span", "jev-row-id")));
  }
  open.textContent = jevSeatName(id);
  open.dataset.tip = jevHintLead(id);
  seat.querySelector(".jev-row-id").textContent = id;
  // A feature whose records this computer keeps in one place reads the same
  // numbers whichever scope is counted, and says so (t-9091).
  let reach = seat.querySelector("[data-jev-reach]");
  if (held?.reach === "machine" && !reach) {
    reach = jevHint("jev.scope.machineTip", "이 기능의 기록은 이 컴퓨터 한곳에 쌓여, 범위를 바꿔도 숫자가 같습니다.",
      jevText("jev.scope.machine", "프로젝트와 무관", "span", "jev-reach"));
    reach.dataset.jevReach = "machine";
    seat.querySelector(".jev-row-meta").append(reach);
  } else if (held?.reach !== "machine") {
    reach?.remove();
  }
  const choice = standing ? jevSeatChoice(typesafeState, id) : null;
  const applying = held ? held.applies : Boolean(choice?.applies);
  row.classList.toggle("is-applying", Boolean(applying));
  row.classList.toggle("is-quiet", !held || held.week.rows === 0);
  if (!held) {
    for (const column of JEV_COLUMNS) {
      if (column.cell === "seat") continue;
      cell(column.cell).textContent = "—";
    }
    return;
  }
  // Facts inside a cell wear their own names, so a reader (or a test)
  // finds "today" whether or not it shares a column with "week".
  const fact = (name, text, className = "") => {
    const node = jevNode("span", className);
    node.dataset.jevFact = name;
    node.textContent = text;
    return node;
  };
  const rows = cell("rows");
  rows.replaceChildren(
    fact("today", jevCount(held.today.rows), "jev-fact-strong"), document.createTextNode(" · "),
    fact("week", jevCount(held.week.rows)));
  // What did not come back, by token, in words — the ones a person clears
  // are buttons to where they are cleared (t-6243 D5).
  const failures = held.week.failures ?? [];
  if (failures.length > 0) {
    rows.append(jevNode("div", "jev-tokens", ...failures.map(jevTokenChip)));
  } else if (held.week.refused > 0) {
    rows.append(jevNode("div", "jev-tokens", fact("refused", t("jev.refusedCount", "거절 {{count}}건", { count: jevCount(held.week.refused) }), "jev-token")));
  }
  const share = cell("answered");
  share.replaceChildren();
  if (held.week.rows === 0) {
    share.textContent = "—";
  } else if (held.week.answered === 0 && held.week.refused > 0) {
    // Nothing answered because the door refused everything: that, not a
    // share of zero and an empty bound, is the fact (t-6243 D3).
    share.append(fact("refused", t("jev.refusedCount", "거절 {{count}}건", { count: jevCount(held.week.refused) }), "jev-refused"));
  } else if (held.week.rows < JEV_SAMPLE_FLOOR) {
    share.append(fact("sample", jevSampleWords(held.week.rows), "jev-sample"));
  } else {
    const bound = fact("bound", t("jev.bound", "신뢰 하한 {{pct}}", { pct: jevPercent(held.week.answeredLowerBound) }), "jev-bound");
    // Where that bound stands against the line for acting by itself, said
    // as its tip — the number a person hovers, not a fact beside it.
    if (held.clearsRiseFloor !== null && held.clearsRiseFloor !== undefined && held.riseFloorPermille) {
      const floor = jevFloor(held.riseFloorPermille);
      bound.dataset.tip = held.clearsRiseFloor
        ? t("jev.clearsFloor", "응답률 신뢰 하한이 자동 적용 기준({{floor}}) 이상", { floor })
        : t("jev.underFloor", "응답률 신뢰 하한이 자동 적용 기준({{floor}}) 미만", { floor });
      bound.classList.toggle("is-under", !held.clearsRiseFloor);
    }
    share.append(fact("share", jevPercent(held.week.answeredShare), "jev-share"), document.createTextNode(" "), bound);
  }
  cell("latency").textContent = jevLatencyWords(held.week);
  const acted = cell("acted");
  acted.replaceChildren(
    fact("applied", held.week.rows === 0 ? "—" : jevCount(held.week.applied ?? 0)), document.createTextNode(" · "),
    fact("agreement", jevAgreementWords(held, standing)));
  cell("cost").textContent = jevCost(held.costUsd);
  cell("trend").replaceChildren(jevTrendCell(held));
  cell("status").replaceChildren(jevStatusCell(held, choice, standing, head));
}

/* The row that stands first when no feature was asked all week: whether Jev
 * is off — the switch over the table turns it on — or on and quiet. */
function jevEmptyRow(body) {
  let row = body.querySelector("[data-jev-empty]");
  if (!row) {
    const cell = jevNode("td", "", jevNode("span", "jev-empty"));
    cell.colSpan = JEV_COLUMNS.length;
    row = jevNode("tr", "jev-empty-row", cell);
    row.dataset.jevEmpty = "";
    body.prepend(row);
  }
  row.querySelector(".jev-empty").textContent = typesafeState?.jev && !typesafeState.jev.on
    ? t("jev.empty.off", "Jev가 꺼져 있어 판단을 요청하지 않습니다. 위의 스위치로 켤 수 있습니다.")
    : t("jev.empty.quiet", "지난 7일 판단 요청이 없었습니다. 기능이 판단을 요청하면 여기에 쌓입니다.");
  return row;
}

/* The row the unused features fold into: one press shows or hides them all,
 * and the choice is kept for the next open. */
function jevFoldRow(body, count) {
  let row = body.querySelector("[data-jev-fold]");
  if (!row) {
    const press = document.createElement("button");
    press.type = "button";
    press.className = "jev-fold";
    press.addEventListener("click", () => {
      jevUnusedOpen = !jevUnusedOpen;
      jevRememberUnusedOpen(jevUnusedOpen);
      paintJevViews();
    });
    const cell = jevNode("td", "", press);
    cell.colSpan = JEV_COLUMNS.length;
    row = jevNode("tr", "jev-fold-row", cell);
    row.dataset.jevFold = "";
    body.append(row);
  }
  const press = row.querySelector(".jev-fold");
  press.setAttribute("aria-expanded", String(jevUnusedOpen));
  press.textContent = jevUnusedOpen
    ? t("jev.unused.hide", "사용 안 한 기능 {{count}}개 접기", { count: jevCount(count) })
    : t("jev.unused.show", "사용 안 한 기능 {{count}}개 보기", { count: jevCount(count) });
  return row;
}

/* Put the table's rows in `drawn`'s order, moving only the rows out of place
 * — and none while focus is inside the table, where moving a row would take
 * the focus with it; the next paint puts them right. */
function jevArrange(body, drawn) {
  if (body.contains(document.activeElement)) return;
  drawn.forEach((row, at) => {
    if (body.children[at] !== row) body.insertBefore(row, body.children[at] ?? null);
  });
}

/* The strip: the sums over every feature the card names, how many stand in
 * each state, and the day's requests against the limit. */
function paintJevSummary(view, order, heldOf) {
  const said = (name, text) => {
    const holder = view.querySelector(`[data-jev-stat="${name}"]`);
    holder.querySelector("dd").textContent = text;
    return holder;
  };
  const counted = order.map((id) => heldOf.get(id)).filter(Boolean);
  const sum = (read) => counted.reduce((total, held) => total + read(held), 0);
  said("today", counted.length ? jevCount(sum((held) => held.today.rows)) : "—");
  said("week", counted.length ? jevCount(sum((held) => held.week.rows)) : "—");
  const costs = counted.filter((held) => held.costUsd !== null && held.costUsd !== undefined);
  said("cost", costs.length ? jevCost(costs.reduce((total, held) => total + held.costUsd, 0)) : "—");
  // The states are counted over the features in use this week: a feature
  // switched on that nothing asked is where its switch puts it, and is not
  // one more feature applying (t-6277 D8).
  const inUse = order.map((id) => [id, heldOf.get(id)]).filter(([, held]) => held?.week.rows > 0);
  const standing = inUse.map(([id, held]) => [jevSeatStatus(held, jevSeatChoice(typesafeState, id)), held]);
  const tally = (count) => (counted.length ? jevCount(count) : "—");
  for (const state of JEV_STATUSES.filter((one) => one.counted)) {
    said(state.status, tally(standing.filter(([status]) => status === state.status).length));
  }
  said(JEV_UNDER.stat, tally(standing.filter(([status, held]) => status === "recording" && jevUnderBar(held)).length));
  const day = said("day", !jevDay ? "" : jevDay.most === null || jevDay.most === undefined
    ? t("jev.stat.dayOpen", "{{sent}} / 제한 없음", { sent: jevCount(jevDay.sent) })
    : `${jevCount(jevDay.sent)} / ${jevCount(jevDay.most)}`);
  day.hidden = !jevDay;
}

function paintJevFreshness(view) {
  const line = view.querySelector(".jev-freshness");
  const error = view.querySelector(".jev-error");
  if (jevNumbersError) {
    error.textContent = t("jev.unavailable", "기록을 집계하지 못했습니다 — {{error}}", { error: jevNumbersError });
    error.hidden = false;
  } else {
    error.hidden = true;
    error.textContent = "";
  }
  if (jevNumbersAt === 0) {
    line.textContent = t("jev.loading", "기록을 집계하는 중…");
    return;
  }
  // "just now" is not a distance, so it does not take "ago".
  const ago = agoWord(jevNumbersAt, Date.now());
  const ms = String(jevNumbersCostMs ?? 0);
  line.textContent = ago === t("board.justNow", "방금")
    ? t("jev.freshNow", "방금 갱신 · 집계 {{ms}} ms", { ms })
    : t("jev.fresh", "{{ago}} 전 갱신 · 집계 {{ms}} ms", { ago, ms });
}

/* One decision as a line: when, what became of it, what it was asked (the
 * row's own facts, by their names), what it answered, and what the seat's
 * writer later said. The keys are the ledger's — a reader who greps the
 * file finds the same words. */
function jevDecisionNode(decision, now) {
  const item = document.createElement("li");
  item.className = "jev-decision";
  item.dataset.outcome = decision.outcome;
  const when = jevNode("span", "jev-decision-when");
  when.textContent = agoWord(decision.at, now);
  when.dataset.tip = new Date(decision.at).toLocaleString();
  // What became of it, in words: an answer, or the token's own word from the
  // one table the chips read (`JEV_TOKENS`) — a token no row words shows as
  // itself.
  const outcome = jevNode("span", "jev-decision-outcome");
  const token = jevTokenRow(decision.outcome);
  outcome.textContent = decision.outcome === "answered" ? t("jev.answered", "응답")
    : token ? t(token.key, token.word) : decision.outcome;
  const asked = jevNode("span", "jev-decision-asked");
  asked.textContent = Object.entries(decision.asked ?? {}).map(([key, value]) => `${key}: ${value}`).join(" · ");
  const answered = jevNode("span", "jev-decision-answer");
  const answer = decision.answered;
  if (answer === null || answer === undefined) answered.textContent = "—";
  else if (typeof answer === "object") {
    answered.textContent = Object.entries(answer).map(([key, value]) => `${key}=${value}`).join(" · ");
  } else answered.textContent = String(answer);
  const marks = [];
  if (decision.confidence !== null && decision.confidence !== undefined) {
    marks.push(t("jev.confidence", "확신도 {{pct}}", { pct: jevPercent(decision.confidence) }));
  }
  if (decision.applied === true) marks.push(t("jev.applied", "적용됨"));
  else if (decision.applied === false) marks.push(t("jev.recorded", "기록만 (미적용)"));
  if (decision.agreed === true) marks.push(t("jev.agreed", "일치"));
  else if (decision.agreed === false) marks.push(t("jev.disagreed", "불일치"));
  if (decision.followed) marks.push(t("jev.followed", "이후 {{word}}", { word: decision.followed }));
  if (decision.elapsedMs !== null && decision.elapsedMs !== undefined) marks.push(`${decision.elapsedMs} ms`);
  const mark = jevNode("span", "jev-decision-marks");
  mark.textContent = marks.join(" · ");
  item.append(when, outcome, asked, answered, mark);
  return item;
}
