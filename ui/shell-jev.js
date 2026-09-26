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

/* The pictures' one table (t-9633): every number a picture is drawn with —
 * each box in its own units, the room kept inside it, the days a trend needs
 * and the sample a day's share is read from — while every colour a mark
 * wears is the stylesheet's (`--jev-chart-*`, in both treatments) and how
 * large a box draws is the stylesheet's too (`--jev-plot-*`).
 *
 *   trendDays    the fewest days with a value a day-by-day picture is drawn
 *                from: two points are a line with no shape, three the fewest
 *                that can show a turn (t-6243 D4). Fewer, and the cell says
 *                how many days there are instead.
 *   sampleFloor  under this many rows a share is the width of its interval
 *                rather than a measurement — one answer in one bounds at 21%,
 *                which a person reads as a failing feature — so a cell says
 *                the sample instead of a share (t-6243 D3, the plan's "하한은
 *                n≥5부터"), and a day graded on fewer marks is drawn faint.
 *   trend        a row's accuracy picture (b) and, under it on the same days,
 *                its strip of days (a), `strip` tall with `gap` between its
 *                cells: the days fill `width` less the `now` slot at the
 *                right end, where the strip marks where the feature stands;
 *                `pad` keeps a line's stroke inside the box.
 *   trendWide    the drawer's accuracy picture: the same days, larger, with
 *                the days' comparisons as columns `column` of their day's
 *                width in the `samples` under the lines.
 *   latency      a row's latency picture (d).
 *   tokens       the week's input tokens (e); past `dense` days its columns
 *                draw thinner, so a month still stands apart day by day.
 *   judgment     one feature's bar toward its next judgment (c), and in its
 *                drawer each reason's share of the rows that compared nothing.
 *   reasonsShown how many of the reasons its week's rows compared nothing a
 *                row names under its state's chip, the most frequent first
 *                (t-9935): two stand on one line in the state column's
 *                width, where the cell keeps to its fourteen words and a
 *                1080p screen to its seven rows (t-6243 D2, t-9633); the
 *                drawer lists every one.
 *
 * A share is said in whole percents (`jevPercent`) and a rate the core keeps
 * per thousand in the whole per-thousands it sends (`jevPermille`), so no
 * picture or sentence rounds a number of its own. And a 1080p screen holds
 * seven rows under the charts before any scroll (t-9633, the harness's
 * `FIRST_SCREEN_ROWS`): t-6243 D1's nine, less the two rows' room the charts
 * took — a decision kept, not a room to win back. */
const JEV_CHART = Object.freeze({
  trendDays: 3,
  sampleFloor: 5,
  reasonsShown: 2,
  trend: Object.freeze({ width: 64, height: 18, pad: 2, now: 6, strip: 10, gap: 1 }),
  trendWide: Object.freeze({ width: 360, height: 96, pad: 4, now: 0, samples: 24, column: 0.5 }),
  latency: Object.freeze({ width: 72, height: 20, pad: 2 }),
  tokens: Object.freeze({ width: 280, height: 64, dense: 14 }),
  judgment: Object.freeze({ width: 100, height: 6 }),
});

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
/* What each row, chart and drawer was last drawn from (t-9633 §3), so a
 * paint draws again only what moved — the lesson of t-6323's poll, which
 * rebuilt every row on every beat. Keyed by the node drawn, so a node that
 * leaves the page takes its entry with it. */
const jevDrawnFrom = new WeakMap();
/* Whether each view's width shows its rows' latency pictures, as its size
 * watcher last read it (`jevLatencyShown`); none until it has read. */
const jevLatencyAt = new WeakMap();
/* The words one feature's numbers say, once per answer: zo's answer is new
 * objects each time it is asked, and the same objects between the paints of
 * one answer. */
const jevHeldWords = new WeakMap();
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

const jevFeatureWords = new Map();

/* A feature's words as the settings markup keeps them (`#jev-features`): its
 * name, one line of what it does and the paragraph of what it sends, each by
 * the catalog key it is written under — the markup is the one place the
 * Korean is written. `null` for a part the markup does not hold. */
function jevFeature(id) {
  let words = jevFeatureWords.get(id);
  if (words === undefined) {
    const holder = el("jev-features")?.content.querySelector(`[data-jev-feature="${id}"]`) ?? null;
    const part = (name) => {
      const node = holder?.querySelector(`[data-jev-${name}]`);
      return node ? { key: node.dataset.i18n, source: node.textContent.replace(/\s+/g, " ").trim() } : null;
    };
    words = { name: part("name"), summary: part("summary"), hint: part("hint") };
    // The markup's words are the page's own and never change, so each
    // feature's are read once (t-9633 §3: every row read them twice a paint).
    if (holder) jevFeatureWords.set(id, words);
  }
  return words;
}

/* The line a judgment turned on, by the core's token
 * (`promote::Line::token`): what each one MEANS to a person is this page's,
 * said once here. A line that `wants` more samples holds a seat that has not
 * been measured yet; every other line holds one measured under its bar. A
 * line that `compares` holds a seat on the marks its rows left — too few, none,
 * or none that ever said wrong — so its rows that compared nothing, and why,
 * are the reason to name beside it (t-9935). */
const JEV_LINES = Object.freeze({
  too_few_rows: { key: "settings.typesafe.lineTooFewRows", word: "판단 기록이 더 쌓여야 합니다", wants: "rows" },
  answered: { key: "settings.typesafe.lineAnswered", word: "응답률이 기준에 못 미칩니다" },
  latency: { key: "settings.typesafe.lineLatency", word: "응답이 너무 느립니다" },
  schema: { key: "settings.typesafe.lineSchema", word: "형식이 잘못된 응답이 있었습니다" },
  too_few_compared: { key: "settings.typesafe.lineTooFewCompared", word: "정확도를 비교할 표본이 더 필요합니다", wants: "compared", compares: true },
  agreement: { key: "settings.typesafe.lineAgreement", word: "정확도가 기준에 못 미칩니다" },
  unlabeled: { key: "settings.typesafe.lineUnlabeled", word: "정확도를 잴 결과가 아직 없습니다", wants: "compared", compares: true },
  one_sided: { key: "settings.typesafe.lineOneSided", word: "틀렸다는 결과가 거의 없어 정확도를 믿을 수 없습니다", compares: true },
  too_few_baseline: { key: "settings.typesafe.lineTooFewBaseline", word: "가장 단순한 방식과 비교할 표본이 더 필요합니다", wants: "baseline" },
  baseline: { key: "settings.typesafe.lineBaseline", word: "가장 단순한 방식보다 낫지 않습니다" },
  labels: { key: "settings.typesafe.lineLabels", word: "사람의 평가에서 기존 방식이 더 나았습니다" },
  fallbacks: { key: "settings.typesafe.lineFallbacks", word: "연속으로 기존 방식으로 되돌아갔습니다" },
  apply_share: { key: "settings.typesafe.lineApplyShare", word: "확신도 기준을 넘는 답이 너무 적습니다" },
});

function jevLineWords(line) {
  const said = JEV_LINES[line];
  return said ? t(said.key, said.word) : "";
}

/* Why a feature's rows compared nothing, by the word each writer put in the
 * row's place of a mark (`notComparedBy`, t-9556: the placement seat's
 * `unseen` and `not_carried`, the effort seats' `not_carried` and
 * `same_as_rule`, the stall seat's `unknown` and `worker_died`, the notice
 * seat's `away`, the summons' and the mention ranking's `not_offered`, the
 * summons' `pinned`, recall's `no_note_touched`, routing's `unanswered`, the
 * file pick's `no_file_edited`, the claim check's `not_model_comparison`) —
 * and why a feature's record draws no confidence bar to act from
 * (`calibration.reason`, the core's `threshold::NoLine::token`). What each
 * means to a person is said once here (t-9935); `jevReasonWords` reads it. */
const JEV_REASONS = Object.freeze({
  away: { key: "jev.reason.away", word: "사람 없음" },
  unseen: { key: "jev.reason.unseen", word: "아무도 안 봄" },
  not_carried: { key: "jev.reason.notCarried", word: "실행 안 됨" },
  same_as_rule: { key: "jev.reason.sameAsRule", word: "규칙과 같음" },
  unknown: { key: "jev.reason.unknown", word: "원인 모름" },
  worker_died: { key: "jev.reason.workerDied", word: "터미널 닫힘" },
  not_offered: { key: "jev.reason.notOffered", word: "보기에 없음" },
  pinned: { key: "jev.reason.pinned", word: "모델 고정" },
  no_note_touched: { key: "jev.reason.noNoteTouched", word: "노트 안 씀" },
  unanswered: { key: "jev.reason.unanswered", word: "결과 없음" },
  no_file_edited: { key: "jev.reason.noFileEdited", word: "편집한 파일 없음" },
  not_model_comparison: { key: "jev.reason.notModelComparison", word: "모델 비교 아님" },
  no_confidence: { key: "jev.reason.noConfidence", word: "답에 확신도가 없음" },
  whole: { key: "jev.reason.whole", word: "모든 답이 이미 기준을 넘음" },
  one_colour: { key: "jev.reason.oneColour", word: "확신도로 답이 나뉘지 않음" },
  non_monotone: { key: "jev.reason.nonMonotone", word: "확신도가 높은 답이 더 자주 틀림" },
  no_lift: { key: "jev.reason.noLift", word: "기준을 그어도 정확도가 거의 같음" },
});

/* A reason in words: this table's, else — a record that drew no bar because
 * one of the judge's own lines broke (`NoLine::Line`) — that line's, else the
 * token itself, so a word no table knows yet still says what zo said. */
function jevReasonWords(token) {
  const said = JEV_REASONS[token] ?? JEV_LINES[token];
  return said ? t(said.key, said.word) : token;
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
  { cell: "rows", key: "jev.col.rows", word: "요청 (오늘 · 7일)" },
  { cell: "answered", key: "jev.col.answered", word: "응답률", tipKey: "jev.col.answeredTip", tip: "응답률은 판단을 요청한 것 중 응답을 받은 비율이고, 신뢰 하한은 표본이 적을 때를 감안한 보수적인 값입니다." },
  { cell: "latency", key: "jev.col.latency", word: "응답 시간" },
  { cell: "acted", key: "jev.col.acted", word: "적용 · 정확도", tipKey: "jev.col.actedTip", tip: "정확도는 판단이 기존 방식의 판단이나 나중에 확인된 결과와 같았던 비율입니다." },
  { cell: "cost", key: "jev.col.cost", word: "비용 (USD)" },
  { cell: "status", key: "jev.col.why", word: "상태와 다음 단계" },
  { cell: "trend", key: "jev.col.trend", word: "지난 7일", tipKey: "jev.col.trendTip", tip: "선은 날마다 판단이 맞은 비율(실선)과 가장 단순한 방식의 비율(점선)이고, 띠는 그 비율의 신뢰 하한까지입니다. 아래 막대는 날마다 한 일입니다: 높이는 그날의 요청 수이고, 진한 막대는 비교함, 옅은 막대는 비교가 너무 적음, 회색 막대는 요청만 있음, 주황 막대는 단순 방식이 같거나 나았던 날이고, 끝의 점은 지금 상태입니다." },
]);

/* Where each column stands in a row. */
const JEV_COLUMN_AT = new Map(JEV_COLUMNS.map((column, at) => [column.cell, at]));

/* The lines a feature's week draws in its accuracy picture (t-6243 D4,
 * t-9633 (b)), each read off one day (`jevDaysOf`): the share of the
 * judgments that matched, and the simplest method's share over the same
 * marks (t-6342). Both are shares, so one picture holds both on one scale;
 * the response rate has its own column and how long the answers took its own
 * picture (d). */
const JEV_TRENDS = Object.freeze([
  { line: "agreement", key: "jev.trend.agreement", word: "정확도", read: (day) => day.share },
  { line: "baseline", key: "jev.trend.baseline", word: "단순 방식", read: (day) => day.base },
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

  // The charts over the table (t-9633): the week's input tokens and what
  // they cost, and the features nearest their next judgment.
  const charts = jevNode("section", "jev-charts", ...JEV_CHARTS.map(jevChartCard));
  charts.dataset.jevCharts = "";
  charts.dataset.i18nAria = "jev.charts";
  charts.setAttribute("aria-label", t("jev.charts", "지난 7일 그래프"));
  root.append(charts);

  // Each part of the table says what it is (`role`): a narrow window lays
  // the rows out as cards (t-9633), and a table that stops displaying as one
  // can lose what it is to a screen reader.
  const head = jevNode("tr");
  head.setAttribute("role", "row");
  for (const column of JEV_COLUMNS) {
    const cell = jevText(column.key, column.word, "th", `jev-col-${column.cell}`);
    cell.scope = "col";
    cell.setAttribute("role", "columnheader");
    if (column.tipKey) jevHint(column.tipKey, column.tip, cell);
    head.append(cell);
  }
  const body = document.createElement("tbody");
  body.dataset.jevRows = "";
  body.setAttribute("role", "rowgroup");
  const heads = jevNode("thead", "", head);
  heads.setAttribute("role", "rowgroup");
  // The version the table's judgments come from, named once over it
  // (t-6243 D2); a row names one only when it is another.
  const caption = jevNode("caption", "jev-caption");
  caption.hidden = true;
  const table = jevNode("table", "jev-table", caption, heads, body);
  table.setAttribute("role", "table");
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
  // What stands between the feature and automatic use (t-9935): the check's
  // result and the confidence bar as facts, then why its week's rows
  // compared nothing, reason by reason.
  const whyHead = jevNode("p", "jev-drawer-text");
  whyHead.dataset.jevWhyHead = "";
  const drawer = jevNode("aside", "jev-drawer", head,
    jevNode("p", "jev-drawer-summary"),
    jevText("jev.drawer.written", "설정 파일에 이 기능만 따로 정해 두어 스위치를 따르지 않습니다. 스위치를 다시 켜면 권장 설정으로 돌아갑니다.", "p", "jev-drawer-written"),
    part("why", "jev.drawer.why", "자동 적용 판정", jevNode("dl", "jev-drawer-facts"), whyHead, jevNode("ol", "jev-judgment")),
    part("days", "jev.drawer.days", "날마다", jevNode("div", "jev-days-picture"), jevNode("div", "jev-days-legend"),
      jevNode("table", "jev-days-table", jevNode("thead", "", jevNode("tr", "", ...JEV_DAY_COLUMNS.map((column) => {
        const head = jevText(column.key, column.word, "th");
        head.scope = "col";
        return head;
      }))), document.createElement("tbody"))),
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
    // A width that shows the rows' latency pictures where they were not (or
    // hides them) is a paint: they are drawn only where they stand. Read
    // here, after the page is laid out, and never in a paint, where reading
    // a style would lay out a page half drawn.
    const shown = jevLatencyShown(host);
    if (shown === (jevLatencyAt.get(host) ?? false)) return;
    jevLatencyAt.set(host, shown);
    paintJevView(host);
  }).observe(host);
}

/* Whether a row's latency picture stands at the width this view has — the
 * stylesheet's to say (`--jev-latency-shown`, t-9633) — so that one it hides
 * is not drawn. */
function jevLatencyShown(view) {
  const table = view.querySelector(".jev-table");
  return Boolean(table) && getComputedStyle(table).getPropertyValue("--jev-latency-shown").trim() === "1";
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

/* The words a feature's numbers say, for `jevMoved`: its days as the
 * pictures read them (`jevDaysOf`, kept for the paint that draws them) —
 * the fields a picture reads and none of the rest of a day, a tenth of its
 * size — and apart from them the rest a row draws from. Neither holds the
 * recent list: the drawer's alone, and new with every request. */
function jevWordsOf(held) {
  if (!held) return { days: "", rest: "", read: [] };
  let words = jevHeldWords.get(held);
  if (words === undefined) {
    // Copies without the list rather than a replacer: a replacer is called
    // for every key, and costs the check its fast road.
    const { recent, days, ...rest } = held;
    const read = jevDaysOf(held);
    words = { days: JSON.stringify(read.flatMap(Object.values)), rest: JSON.stringify(rest), read };
    jevHeldWords.set(held, words);
  }
  return words;
}

/* Whether what `node` is drawn from moved since it was last drawn; when it
 * did, what it is drawn from now is kept for the next paint to compare. */
function jevMoved(node, ...inputs) {
  const words = inputs.join("\u0001");
  if (jevDrawnFrom.get(node) === words) return false;
  jevDrawnFrom.set(node, words);
  return true;
}

/* Words written into a node only when they are not its words already: a
 * paint that writes what stands moved nothing and still changes the page. */
function jevSay(node, text) {
  if (node.textContent !== text) node.textContent = text;
}

function jevLanguage() {
  return jevRoot.lang ?? "";
}

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

/* A sample too small for a share (`JEV_CHART.sampleFloor`), said as its size. */
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
/* The number and day formats of the language in force, made once per
 * language: a formatter is not free to build, and a paint formats every
 * cell. */
const jevRoot = document.documentElement;
let jevNumbersLang = null;
let jevCountFormat = null;
let jevCostFormat = null;
let jevDayFormat = null;
/* And what they said: a picture names each of its days, and a table of
 * features names the same seven over and over — each is formatted once per
 * language (t-9633 §3: formatting a day was a fifth of a full repaint). */
let jevDayNames = new Map();
let jevStateWords = new Map();
let jevColumnWords = null;
let jevCounted = new Map();
/* How many grouped counts are kept before they are all let go: a month of
 * twenty-seven features' days and their numbers several times over. */
const JEV_COUNTS_KEPT = 4096;

function jevFormats() {
  const lang = jevRoot.lang || undefined;
  if (jevCountFormat === null || lang !== jevNumbersLang) {
    jevNumbersLang = lang;
    jevCountFormat = new Intl.NumberFormat(lang);
    jevCostFormat = new Intl.NumberFormat(lang, { maximumSignificantDigits: JEV_COST_DIGITS });
    jevDayFormat = new Intl.DateTimeFormat(lang, { month: "short", day: "numeric" });
    jevDayNames = new Map();
    jevStateWords = new Map();
    jevColumnWords = null;
    jevCounted = new Map();
  }
  return { count: jevCountFormat, cost: jevCostFormat, day: jevDayFormat };
}

/* A day as the language in force names it: "9월 21일", "Sep 21". */
function jevDayName(ms) {
  const day = jevFormats().day;
  let name = jevDayNames.get(ms);
  if (name === undefined) {
    name = day.format(new Date(ms));
    jevDayNames.set(ms, name);
  }
  return name;
}

/* A count as the language in force groups its digits (1,321). */
function jevCount(value) {
  if (value === null || value === undefined) return "—";
  const count = jevFormats().count;
  // The same counts come back paint after paint — a day's tokens, a
  // latency — and each is grouped once per language.
  let said = jevCounted.get(value);
  if (said === undefined) {
    if (jevCounted.size >= JEV_COUNTS_KEPT) jevCounted.clear();
    said = count.format(value);
    jevCounted.set(value, said);
  }
  return said;
}

function jevMs(ms) {
  return ms === null || ms === undefined ? "—" : jevCount(ms);
}

function jevCost(usd) {
  if (usd === null || usd === undefined) return "—";
  return `$${jevFormats().cost.format(usd)}`;
}

/* ---- the pictures (t-9633) ----
 * Five, each one SVG drawn by one function from what zo answered: a row's
 * strip of days (a, `jevTimelinePicture`), its accuracy against the simplest
 * method (b, `jevTrendPicture`) and its latency (d, `jevLatencyPicture`); and
 * over the table how far each feature is from its next judgment (c,
 * `jevJudgmentPicture`) and the week's input tokens with what they cost (e,
 * `jevTokensPicture`). Every number a picture is drawn with is `JEV_CHART`'s;
 * every colour is a mark's class, which the stylesheet inks from
 * `--jev-chart-*` in both treatments, so no picture carries a colour of its
 * own. Each is an image named with its numbers, and each chart over the
 * table keeps a table of them for a reader who cannot see it. */

const JEV_SVG = "http://www.w3.org/2000/svg";

/* An element of a picture, with its attributes. */
function jevSvg(tag, attributes = {}) {
  const node = document.createElementNS(JEV_SVG, tag);
  for (const [name, value] of Object.entries(attributes)) node.setAttribute(name, String(value));
  return node;
}

/* A picture's frame: an image in a box of its own units, named with its
 * numbers (`label`), and the kind of picture it is (`data-jev-picture`). A
 * `stretched` box fills the width it is given, its marks' strokes and dots
 * keeping their size (the stylesheet's `non-scaling-stroke`). */
function jevPicture(kind, box, label, className, stretched = false) {
  const svg = jevSvg("svg", { class: className, viewBox: `0 0 ${box.width} ${box.height}`, role: "img", "aria-label": label });
  if (stretched) svg.setAttribute("preserveAspectRatio", "none");
  svg.dataset.jevPicture = kind;
  return svg;
}

/* One mark of a picture: a path, the line it draws (`data-line`) and the
 * values it was drawn from (`data-values`), for a reader or a test. */
function jevMark(className, d, line = null, values = null) {
  const path = jevSvg("path", { class: className, d });
  if (line) path.dataset.line = line;
  if (values) path.dataset.values = values.map((value) => (value === null || value === undefined ? "" : String(value))).join(",");
  return path;
}

/* A coordinate to the hundredth of a unit: finer than a screen shows, and
 * short to write into a path. */
const jevFixed = (value) => Math.round(value * 100) / 100;

/* Where a day stands across `width`: the middle of its share of the width,
 * so a point, a cell and a column of the same day line up in every picture
 * that draws the same days. */
function jevDayX(at, count, width) {
  return ((at + 0.5) * width) / Math.max(1, count);
}

/* A line through the points, each run of days with a value a subpath of its
 * own: a day with nothing to say is a gap, never a zero. */
function jevLinePath(points) {
  let d = "";
  let open = false;
  for (const point of points) {
    if (!point) {
      open = false;
      continue;
    }
    d += `${open ? "L" : "M"}${jevFixed(point[0])} ${jevFixed(point[1])}`;
    open = true;
  }
  return d;
}

/* A dot at each point, all in one path: a stroke of no length, which the
 * round cap the stylesheet gives it draws as a dot. */
function jevDotsPath(points) {
  return points.filter(Boolean).map(([x, y]) => `M${jevFixed(x)} ${jevFixed(y)}h0`).join("");
}

/* The area between two lines, one polygon per run of days that has both. */
function jevBandPath(upper, lower) {
  let d = "";
  let run = [];
  const close = () => {
    if (run.length > 1) {
      const edge = (points, order) => order.map((at) => `${jevFixed(points[at][0])} ${jevFixed(points[at][1])}`).join("L");
      d += `M${edge(upper, run)}L${edge(lower, [...run].reverse())}Z`;
    }
    run = [];
  };
  upper.forEach((point, at) => {
    if (point && lower[at]) run.push(at);
    else close();
  });
  close();
  return d;
}

/* A feature's days as the pictures read them: each day's requests, its
 * graded marks and their share with that share's lower bound, the simplest
 * method's share over the same marks, its latency and the input tokens it
 * billed — zo's counts as zo gave them, the share read as agreed over
 * compared as the week's picture always has. */
function jevDaysOf(held) {
  return (held?.days ?? []).map((day) => {
    const marks = day.agreement ?? {};
    const compared = marks.compared ?? 0;
    const share = compared > 0 ? marks.agreed / compared : null;
    return {
      startMs: day.startMs,
      rows: day.tally.rows ?? 0,
      compared,
      agreed: marks.agreed ?? 0,
      share,
      bound: share === null ? null : marks.lowerBound ?? null,
      baseCompared: marks.baselineCompared ?? 0,
      baseAgreed: marks.baselineAgreed ?? 0,
      base: marks.baselineCompared ? marks.baselineShare ?? null : null,
      p50: day.tally.p50Ms ?? null,
      p95: day.tally.p95Ms ?? null,
      tokens: day.tally.inputTokens ?? 0,
    };
  });
}

/* What a day of a feature's week did, as its strip draws it (t-9633 (a)),
 * with its words: nothing asked; asked and not graded; graded on fewer marks
 * than the sample floor; graded; or graded on a day the simplest method did
 * as well — the one a person looks for. */
const JEV_DAY_STATES = Object.freeze([
  { state: "idle", key: "jev.day.idle", word: "요청 없음" },
  { state: "recorded", key: "jev.day.recorded", word: "요청만 있음" },
  { state: "thin", key: "jev.day.thin", word: "비교가 너무 적음" },
  { state: "measured", key: "jev.day.measured", word: "비교함" },
  { state: "behind", key: "jev.day.behind", word: "단순 방식이 같거나 나음" },
]);

function jevDayState(day) {
  if (day.compared > 0) {
    if (day.compared < JEV_CHART.sampleFloor) return "thin";
    return day.base !== null && day.base >= day.share ? "behind" : "measured";
  }
  return day.rows > 0 ? "recorded" : "idle";
}

/* A list of a picture's days for its name, "9월 21일 75%, 9월 22일 88%": the
 * days `say` has words for. */
function jevDaysSaid(days, say) {
  return days.map((day) => {
    const said = say(day);
    return said === null ? null : `${jevDayName(day.startMs)} ${said}`;
  }).filter(Boolean).join(", ");
}

/* The week's accuracy as one picture (t-6243 D4, t-9633 (b)), every line on
 * one 0–100% scale over the days, the newest at the right: each graded day's
 * share of the judgments that matched, the band down to that day's lower
 * bound under it — how far the share may be trusted — and, dashed, the
 * simplest method's share over the same marks. A day graded on fewer marks
 * than the sample floor is a faint dot: its share is the width of its
 * interval. Each line carries its values (`data-values`) and how many days
 * drew it (`data-points`). The drawer's box (`samples`) adds each day's
 * comparisons as a column under the lines, and the sample floor across them. */
function jevTrendPicture(days, label, box = JEV_CHART.trend) {
  const wide = Boolean(box.samples);
  const svg = jevPicture("trend", box, label, wide ? "jev-spark jev-spark-wide" : "jev-spark", wide);
  const plot = box.height - (box.samples ?? 0);
  const width = box.width - box.now;
  const y = (share) => plot - box.pad - share * (plot - box.pad * 2);
  const at = (read) => days.map((day, index) => {
    const value = read(day);
    return value === null ? null : [jevDayX(index, days.length, width), y(value)];
  });
  const bounds = days.map((day) => (day.share === null ? null : day.bound));
  const shares = at((day) => day.share);
  const lower = at((day) => (day.share === null ? null : day.bound));
  const bases = at((day) => day.base);
  const counted = (mark, points) => {
    mark.dataset.points = String(points.filter(Boolean).length);
    return mark;
  };
  const floor = JEV_CHART.sampleFloor;
  svg.append(
    counted(jevMark("jev-spark-band", jevBandPath(shares, lower), "band", bounds), lower),
    counted(jevMark("jev-spark-baseline", jevLinePath(bases), "baseline", days.map((day) => day.base)), bases),
    counted(jevMark("jev-spark-line", jevLinePath(shares), "agreement", days.map((day) => day.share)), shares),
    jevMark("jev-spark-dots", jevDotsPath(shares.map((point, index) => (days[index].compared >= floor ? point : null)))),
    jevMark("jev-spark-dots is-thin", jevDotsPath(shares.map((point, index) => (days[index].compared < floor ? point : null)))),
  );
  if (wide) {
    const most = Math.max(floor * 2, ...days.map((day) => day.compared));
    const slot = width / Math.max(1, days.length);
    const columns = days.map((day, index) => {
      if (!day.compared) return "";
      const top = box.height - (day.compared / most) * box.samples;
      const left = jevDayX(index, days.length, width) - (slot * box.column) / 2;
      return `M${jevFixed(left)} ${box.height}V${jevFixed(top)}h${jevFixed(slot * box.column)}V${box.height}Z`;
    }).join("");
    const floorAt = jevFixed(box.height - (floor / most) * box.samples);
    svg.append(
      jevMark("jev-spark-samples", columns, "samples", days.map((day) => day.compared || null)),
      jevMark("jev-spark-floor", `M0 ${floorAt}H${width}`, "floor", [floor]),
    );
  }
  return svg;
}

/* A feature's week as a strip of days (t-9633 (a)) under its accuracy
 * picture, on the same days: an empty cell for a day nothing asked, a quiet
 * one for a day asked and not graded, a faint one for a day graded on fewer
 * marks than the sample floor, a full one for a graded day — in the tone of
 * a miss when the simplest method did as well that day — and, after the
 * last day, a dot in the tone of where the feature stands now. The days'
 * states ride on the picture (`data-states`), one mark per state. */
function jevTimelinePicture(days, status, label, box = JEV_CHART.trend) {
  const svg = jevPicture("timeline", { width: box.width, height: box.strip }, label, "jev-strip");
  const width = box.width - box.now;
  const slot = width / Math.max(1, days.length);
  // Each day's cell is a bar as tall as that day's requests against the
  // busiest day's, so a week of requests that compared nothing still shows
  // its shape (a day with any at all stands at least one unit tall).
  const most = Math.max(1, ...days.map((day) => Math.max(day.rows, day.compared)));
  const cell = (at, rows) => {
    const tall = Math.max(1, Math.round((rows / most) * box.strip * 10) / 10);
    return `M${jevFixed(at * slot + box.gap / 2)} ${jevFixed(box.strip - tall)}h${jevFixed(slot - box.gap)}v${jevFixed(tall)}h${-jevFixed(slot - box.gap)}Z`;
  };
  const states = days.map(jevDayState);
  svg.dataset.states = states.join(",");
  svg.dataset.rows = days.map((day) => day.rows).join(",");
  // Every day's faint cell as one stroke across the days, dashed a cell and
  // a gap at a time: a month costs what a week does.
  const track = jevMark("jev-strip-track", `M${jevFixed(box.gap / 2)} ${jevFixed(box.strip / 2)}H${jevFixed(width)}`);
  track.setAttribute("stroke-width", String(box.strip));
  track.setAttribute("stroke-dasharray", `${jevFixed(slot - box.gap)} ${jevFixed(box.gap)}`);
  svg.append(track);
  const cells = new Map();
  states.forEach((state, at) => {
    if (state !== "idle") cells.set(state, (cells.get(state) ?? "") + cell(at, Math.max(days[at].rows, days[at].compared)));
  });
  for (const { state } of JEV_DAY_STATES) {
    if (!cells.has(state)) continue;
    const mark = jevMark("jev-strip-day", cells.get(state));
    mark.dataset.state = state;
    svg.append(mark);
  }
  const now = jevMark("jev-strip-now", `M${jevFixed(box.width - box.now / 2)} ${jevFixed(box.strip / 2)}h0`);
  now.dataset.status = status;
  svg.append(now);
  return svg;
}

/* A feature's week of answers as one picture (t-9633 (d)): each day's typical
 * answer (the median) as a line with a dot on it, and a whisker from it up to
 * the day's slow answer (the 95th percentile), on a scale from zero to the
 * week's slowest — this feature's own, so its shape shows. The numbers ride on
 * the picture (`data-p50`, `data-p95`) and in its name. */
function jevLatencyPicture(days, label, box = JEV_CHART.latency) {
  const svg = jevPicture("latency", box, label, "jev-latency-picture");
  const most = Math.max(1, ...days.map((day) => day.p95 ?? day.p50 ?? 0));
  const x = (at) => jevDayX(at, days.length, box.width);
  const y = (ms) => box.height - box.pad - (ms / most) * (box.height - box.pad * 2);
  const typical = days.map((day, at) => (day.p50 === null ? null : [x(at), y(day.p50)]));
  const whiskers = days.map((day, at) => (day.p50 === null || day.p95 === null ? ""
    : `M${jevFixed(x(at))} ${jevFixed(y(day.p50))}V${jevFixed(y(day.p95))}`)).join("");
  svg.dataset.p50 = days.map((day) => day.p50 ?? "").join(",");
  svg.dataset.p95 = days.map((day) => day.p95 ?? "").join(",");
  svg.append(
    jevMark("jev-latency-range", whiskers, "p95"),
    jevMark("jev-latency-line", jevLinePath(typical), "p50"),
    jevMark("jev-latency-dots", jevDotsPath(typical)),
  );
  return svg;
}

/* The week's input tokens day by day, every feature's added (t-9633 (e)):
 * one column per day, from the baseline up to its share of the week's
 * busiest day, as thick as the stylesheet says whatever width the chart is
 * given (a stroke that keeps its size) — thinner past `dense` days. The
 * values ride on the columns (`data-values`), and each day's slot, under
 * them, is where a pointer finds that day's numbers (its `tip`). */
function jevTokensPicture(days, label, box = JEV_CHART.tokens) {
  const svg = jevPicture("tokens", box, label, "jev-chart-picture", true);
  if (days.length > box.dense) svg.dataset.dense = "";
  const most = Math.max(1, ...days.map((day) => day.tokens));
  const slot = box.width / Math.max(1, days.length);
  const columns = days.map((day, at) => (day.tokens
    ? `M${jevFixed(jevDayX(at, days.length, box.width))} ${box.height}V${jevFixed(box.height - (day.tokens / most) * box.height)}`
    : "")).join("");
  svg.append(jevMark("jev-chart-rule", `M0 ${box.height}H${box.width}`));
  for (const [at, day] of days.entries()) {
    const hit = jevSvg("rect", { class: "jev-chart-hit", x: jevFixed(at * slot), y: 0, width: jevFixed(slot), height: box.height });
    hit.dataset.tip = day.tip;
    svg.append(hit);
  }
  svg.append(jevMark("jev-chart-columns", columns, null, days.map((day) => day.tokens)));
  return svg;
}

/* One feature's way to its next judgment (t-9633 (c)): a bar filled to the
 * share of what the judge wants that is in hand (`data-values`, have/want),
 * on the track the stylesheet lays behind it — and, in a feature's drawer,
 * one reason's share of the rows that compared nothing (t-9935). */
function jevJudgmentPicture(entry, label, box = JEV_CHART.judgment) {
  const svg = jevPicture("judgment", box, label, "jev-judgment-bar", true);
  const filled = Math.min(1, entry.have / entry.want) * box.width;
  svg.append(jevMark("jev-judgment-fill", filled > 0 ? `M0 0H${jevFixed(filled)}V${box.height}H0Z` : "", null,
    [`${entry.have}/${entry.want}`]));
  return svg;
}

/* A feature's week in its row (t-6243 D4, t-9633 (a)(b)): the accuracy
 * picture once three days were graded, and under it the strip of every day
 * ending where the feature stands (`status`); beside them the latest share
 * of each line, or how many graded days there are until there are enough. A
 * feature nothing asked all week has no week to draw. */
function jevTrendCell(days, status) {
  const cell = jevNode("div", "jev-trends");
  if (!days.some((day) => day.rows > 0 || day.compared > 0)) {
    cell.append(jevNode("span", "jev-trend-short", document.createTextNode("—")));
    return cell;
  }
  const graded = days.filter((day) => day.share !== null).length;
  const drawn = graded >= JEV_CHART.trendDays;
  const pictures = jevNode("div", "jev-trend-pictures");
  if (drawn) {
    const said = JEV_TRENDS.map((trend) => {
      const values = jevDaysSaid(days, (day) => (trend.read(day) === null ? null : jevPercent(trend.read(day))));
      return values ? t("jev.chart.series", "{{name}} — {{values}}", { name: t(trend.key, trend.word), values }) : "";
    }).filter(Boolean).join("; ");
    pictures.append(jevTrendPicture(days, said));
  }
  // The strip is named by how many days stood in each state — the days one
  // by one are the drawer's table — and where the feature stands now.
  const standing = JEV_STATUSES.find((one) => one.status === status);
  const tally = new Map();
  for (const day of days) {
    const state = jevDayState(day);
    tally.set(state, (tally.get(state) ?? 0) + 1);
  }
  pictures.append(jevTimelinePicture(days, status, t("jev.chart.strip", "날마다 한 일 — {{days}} · 지금 {{standing}}", {
    days: JEV_DAY_STATES.filter(({ state }) => tally.has(state))
      .map(({ state }) => t("jev.chart.stripDays", "{{state}} {{count}}일", { state: jevDayWords(state), count: jevCount(tally.get(state)) })).join(", "),
    standing: standing ? t(standing.key, standing.word) : "",
  })));
  cell.append(pictures);
  if (!drawn) {
    cell.append(jevNode("span", "jev-trend-short", document.createTextNode(graded === 0
      ? t("jev.trend.noCompared", "비교한 판단 없음")
      : t("jev.trendShort", "{{days}}일치만 있음", { days: jevCount(graded) }))));
    return cell;
  }
  // Each line's latest value beside its key — the line's name is the
  // column's head, in the same order, and the key's tip; a screen reader
  // has both in the picture's own name, so the key is not read twice.
  const legend = jevNode("div", "jev-trend-legend");
  legend.setAttribute("aria-hidden", "true");
  for (const trend of JEV_TRENDS) {
    const last = days.map(trend.read).filter((value) => value !== null).at(-1);
    if (last === undefined) continue;
    const item = jevNode("span", `jev-trend-last jev-trend-${trend.line}`);
    item.textContent = jevPercent(last);
    item.dataset.tip = t(trend.key, trend.word);
    legend.append(item);
  }
  cell.append(legend);
  return cell;
}

/* The table's column names (`JEV_COLUMNS`), once per language. */
function jevColumnNames() {
  jevFormats();
  jevColumnWords ??= JEV_COLUMNS.map((column) => t(column.key, column.word));
  return jevColumnWords;
}

/* Each column's name, said once on the table for every cell of it to wear
 * where the rows stand as cards with no head over them (t-9633) — a string
 * the stylesheet reads (`--jev-label-<cell>`), written again only when the
 * language moved, and never cell by cell. */
const jevNamedIn = new WeakMap();

function jevNameColumns(view) {
  const table = view.querySelector(".jev-table");
  const names = jevColumnNames();
  if (jevNamedIn.get(table) === names) return;
  jevNamedIn.set(table, names);
  JEV_COLUMNS.forEach((column, at) => table.style.setProperty(`--jev-label-${column.cell}`, JSON.stringify(names[at])));
}

/* A day's state in words (`JEV_DAY_STATES`), once per language. */
function jevDayWords(state) {
  jevFormats();
  let words = jevStateWords.get(state);
  if (words === undefined) {
    const said = JEV_DAY_STATES.find((one) => one.state === state);
    words = said ? t(said.key, said.word) : state;
    jevStateWords.set(state, words);
  }
  return words;
}

/* How long a feature's answers take (t-9633 (d)): the week's typical wait and
 * its slow one beside it, and — where the width shows it (`drawn`) — the
 * picture of its days once three answered. A sum of projects carries no
 * percentile — each project measures its own — and says so rather than a
 * dash. */
function jevLatencyCell(held, days, drawn) {
  const cell = jevNode("div", "jev-latency");
  const words = jevNode("div", "jev-latency-words");
  const week = held.week;
  if (week.p50Ms === null || week.p50Ms === undefined) {
    words.append(held.across?.summed
      ? jevHint("jev.latency.summedTip", "응답 시간은 프로젝트마다 따로 재서 더하지 않습니다.",
        jevFact("p50", t("jev.latency.summed", "합산 안 함"), "jev-fact-mist"))
      : jevFact("p50", "—"));
  } else {
    words.append(jevFact("p50", `${jevMs(week.p50Ms)} ms`, "jev-fact-strong"));
    if (week.p95Ms !== null && week.p95Ms !== undefined) {
      words.append(jevFact("p95", t("jev.latency.slow", "느릴 때 {{p95}}", { p95: jevMs(week.p95Ms) }), "jev-fact-mist"));
    }
  }
  if (drawn && days.filter((day) => day.p50 !== null).length >= JEV_CHART.trendDays) {
    // Each day says its two numbers, typical then slow; the words for the
    // two are said once, at the head of the name.
    cell.append(jevLatencyPicture(days, t("jev.chart.latency", "날마다 응답 시간 (보통 · 느릴 때) — {{days}}", {
      days: jevDaysSaid(days, (day) => (day.p50 === null ? null
        : `${jevMs(day.p50)}${day.p95 === null ? "" : ` · ${jevMs(day.p95)}`} ms`)),
    })));
  }
  cell.append(words);
  return cell;
}

/* What the pieces of a row's cell are, by their own names, so a reader (or a
 * test) finds "today" whether or not it shares a cell with "week". */
function jevFact(name, text, className = "") {
  const node = jevNode("span", className);
  node.dataset.jevFact = name;
  node.textContent = text;
  return node;
}

/* The samples in hand for the judge, by the line that wants them: the judged
 * window, the compared marks, the simplest method's. Keyed by `JEV_LINES`'
 * `wants`. */
const JEV_SAMPLES = Object.freeze({
  rows: { key: "jev.window", word: "판정 표본 {{rows}}/{{wanted}}건" },
  compared: { key: "jev.compared", word: "비교 표본 {{rows}}/{{wanted}}건" },
  baseline: { key: "jev.baselineCompared", word: "기준 비교 표본 {{rows}}/{{wanted}}건" },
});

/* What remains before the judge speaks (t-6243 D2, t-9633 (c)), by what it
 * waits on: the samples a line that `wants` more still owes, or — once the
 * judged window is full (`next`) — the requests until the next judgment, the
 * window itself being the sample in hand. And, for a record with too few
 * misses to trust (`one_sided`), the misses it needs (`negativesWanted`). */
const JEV_OWED = Object.freeze({
  rows: { sample: "rows", key: "jev.owed.rows", word: "판정까지 {{count}}건" },
  compared: { sample: "compared", key: "jev.owed.compared", word: "정확도 판정까지 {{count}}건" },
  baseline: { sample: "baseline", key: "jev.owed.baseline", word: "기준 비교까지 {{count}}건" },
  next: { sample: "rows", key: "jev.owed.next", word: "다음 판정까지 {{count}}건" },
});
const JEV_NEGATIVES = Object.freeze({ key: "jev.owed.negatives", word: "틀린 결과 {{count}}건 필요" });

/* What the judge waits on for a feature, and how much of it is in hand: the
 * judged window while it fills (the core's own countdown,
 * `rowsToNextJudgment`, so the bar and the judgment land on the same row),
 * then the compared marks, then the simplest method's; once the window is
 * full, the requests until the next judgment. `null` for a feature the judge
 * does not read, for a sum of projects — each judges its own rows (t-9091) —
 * and for a count zo did not give. */
function jevOwed(held, standing) {
  if (!held?.judged || !held.verdict || held.across?.summed) return null;
  const wants = JEV_LINES[held.verdict.line ?? ""]?.wants ?? "next";
  const agreement = held.judged.agreement ?? {};
  const [have, want] = wants === "compared" ? [agreement.compared ?? 0, standing?.agreementRowsWanted]
    : wants === "baseline" ? [agreement.baselineCompared ?? 0, standing?.agreementRowsWanted]
      : [held.judged.window.rows, held.judged.windowWanted];
  if (!want) return null;
  const counted = held.rowsToNextJudgment ?? null;
  const owed = wants === "next" ? counted : wants === "rows" ? counted ?? Math.max(0, want - have) : Math.max(0, want - have);
  if (owed === null) return null;
  return { wants, have: Math.min(have, want), want, owed };
}

/* What remains, in words — and the misses a one-sided record still needs. */
function jevOwedWords(owed, held) {
  const said = JEV_OWED[owed.wants];
  const words = t(said.key, said.word, { count: jevCount(owed.owed) });
  if (held.verdict?.line !== "one_sided" || !held.negativesWanted) return words;
  return `${words} · ${t(JEV_NEGATIVES.key, JEV_NEGATIVES.word, { count: jevCount(held.negativesWanted) })}`;
}

/* The samples in hand, in words ("판정 표본 25/25건"). */
function jevSampleSaid(owed) {
  const sample = JEV_SAMPLES[JEV_OWED[owed.wants].sample];
  return t(sample.key, sample.word, { rows: jevCount(owed.have), wanted: jevCount(owed.want) });
}

/* The charts over the table (t-9633 (c)(e)), each a card: its title, the
 * figure it is read for and a line under it, the picture, a line in its
 * place when there is nothing to draw, and — for a reader who cannot see it
 * — a table of the same numbers under the heads given here. Built once per
 * view (`jevChartCard`); drawn by `paintJevCharts`. */
const JEV_CHARTS = Object.freeze([
  { chart: "tokens", key: "jev.chart.tokens.title", word: "일별 입력 토큰", noteKey: "jev.chart.tokens.costTip", note: "기능마다 7일 비용을 날마다의 입력 토큰 몫으로 나눈 추정입니다. 날짜를 모두 더하면 위 요약의 7일 비용과 같습니다.", heads: [
    { key: "jev.chart.head.day", word: "날짜" },
    { key: "jev.chart.head.tokens", word: "입력 토큰" },
    { key: "jev.chart.head.cost", word: "추정 비용" },
  ] },
  { chart: "judgment", key: "jev.chart.judgment.title", word: "다음 판정까지", heads: [
    { key: "jev.col.seat", word: "기능" },
    { key: "jev.chart.head.filled", word: "채운 표본" },
    { key: "jev.chart.head.owed", word: "남은 요청" },
  ] },
]);

/* How many features the judgment chart lists, nearest first: the ones about
 * to be judged are what it is read for, and the rest stand in the table's
 * state column with the same words. */
const JEV_JUDGMENT_ROWS = 4;

function jevChartCard(chart) {
  const card = jevNode("figure", "jev-chart");
  card.dataset.jevChart = chart.chart;
  const sub = jevNode("span", "jev-chart-sub");
  // What the line under the figure means, as its tip, when it needs saying.
  if (chart.noteKey) jevHint(chart.noteKey, chart.note, sub);
  const head = jevNode("figcaption", "jev-chart-head",
    jevText(chart.key, chart.word, "span", "jev-chart-title"), jevNode("span", "jev-chart-figure"), sub);
  const empty = jevNode("p", "jev-chart-empty");
  empty.hidden = true;
  const heads = jevNode("tr", "", ...chart.heads.map((one) => {
    const cell = jevText(one.key, one.word, "th");
    cell.scope = "col";
    return cell;
  }));
  const table = jevNode("table", "sr jev-chart-table",
    jevText(chart.key, chart.word, "caption"), jevNode("thead", "", heads), document.createElement("tbody"));
  card.append(head, jevNode("div", "jev-chart-body"), empty, table);
  return card;
}

/* One row of a chart's table: the first cell heads it. */
function jevChartTableRow(first, ...rest) {
  const head = jevNode("th", "", document.createTextNode(first));
  head.scope = "row";
  return jevNode("tr", "", head, ...rest.map((one) => jevNode("td", "", document.createTextNode(one))));
}

/* The week's input tokens by day, every feature's added (t-9633 (e)), with
 * what each day cost as an estimate: a feature's bill for the week
 * (`costUsd`) spread over its days by their share of its input tokens, so the
 * days add up to the bill the strip counts. zo prices the week alone
 * (`jev_summary`'s `cost_of`, one rate per model from the one price table),
 * and a second price table here would be a second answer. A feature zo did
 * not price adds its tokens and no cost. */
function jevTokenDays(numbers) {
  const byStart = new Map();
  for (const held of numbers) {
    const days = held.days ?? [];
    const billed = days.reduce((total, day) => total + (day.tally.inputTokens ?? 0), 0);
    const rate = held.costUsd !== null && held.costUsd !== undefined && billed > 0 ? held.costUsd / billed : null;
    for (const day of days) {
      const tokens = day.tally.inputTokens ?? 0;
      const one = byStart.get(day.startMs) ?? { startMs: day.startMs, tokens: 0, cost: null };
      one.tokens += tokens;
      if (rate !== null) one.cost = (one.cost ?? 0) + tokens * rate;
      byStart.set(day.startMs, one);
    }
  }
  return [...byStart.values()].sort((a, b) => a.startMs - b.startMs);
}

/* The features in use that wait on a judgment (t-9633 (c)), nearest first —
 * the activity order among equals — each with what is in hand and what
 * remains, in words. */
function jevJudgmentEntries(ids, heldOf) {
  const entries = [];
  for (const id of ids) {
    const held = heldOf.get(id);
    const owed = jevOwed(held, jevSeat(typesafeState ?? {}, id));
    if (owed) entries.push({ id, ...owed, words: jevOwedWords(owed, held), sample: jevSampleSaid(owed) });
  }
  return entries.sort((a, b) => a.owed - b.owed);
}

/* The charts over the table, from what is in hand, each drawn again only
 * when what it draws moved: while zo has not answered each frame says it is
 * counting; with nothing to draw, why. */
function paintJevCharts(view, inUse, heldOf, numbers) {
  const counting = jevNumbers === null && !jevNumbersError;
  const language = jevLanguage();
  const tokens = view.querySelector('[data-jev-chart="tokens"]');
  const days = jevTokenDays(numbers);
  if (jevMoved(tokens, language, String(counting), JSON.stringify(days))) paintJevTokens(tokens, days, counting);
  const judgment = view.querySelector('[data-jev-chart="judgment"]');
  const entries = jevJudgmentEntries(inUse, heldOf);
  const summed = numbers.some((held) => held.across?.summed);
  if (jevMoved(judgment, language, String(counting), String(summed), JSON.stringify(entries))) {
    paintJevJudgment(judgment, entries, counting, summed);
  }
}

/* A chart with nothing to draw keeps its frame and says why: zo is still
 * counting, or `why`. */
function jevChartEmpty(card, counting, why) {
  const empty = card.querySelector(".jev-chart-empty");
  empty.textContent = counting ? t("jev.loading", "기록을 집계하는 중…") : why;
  empty.hidden = false;
  jevSay(card.querySelector(".jev-chart-figure"), "");
  jevSay(card.querySelector(".jev-chart-sub"), "");
  card.querySelector(".jev-chart-body").replaceChildren();
  card.querySelector("tbody").replaceChildren();
}

/* A chart about to draw: the line that stood in its place goes, words and
 * all. */
function jevChartDrawn(card) {
  const empty = card.querySelector(".jev-chart-empty");
  empty.hidden = true;
  jevSay(empty, "");
}

/* (e): the week's input tokens day by day, their sum, and what they cost. */
function paintJevTokens(card, days, counting) {
  const total = days.reduce((sum, day) => sum + day.tokens, 0);
  if (counting || total === 0) {
    jevChartEmpty(card, counting, t("jev.chart.tokens.empty", "입력 토큰을 쓴 판단이 없습니다"));
    return;
  }
  const priced = days.some((day) => day.cost !== null);
  const costs = new Map(days.map((day) => [day, day.cost === null ? "—" : jevCost(day.cost)]));
  const cost = (day) => costs.get(day);
  for (const day of days) {
    day.tip = t("jev.chart.tokens.day", "{{day}} · 입력 토큰 {{tokens}} · 추정 비용 {{cost}}", {
      day: jevDayName(day.startMs), tokens: jevCount(day.tokens), cost: cost(day),
    });
  }
  jevChartDrawn(card);
  jevSay(card.querySelector(".jev-chart-figure"), jevCount(total));
  jevSay(card.querySelector(".jev-chart-sub"), priced
    ? t("jev.chart.tokens.cost", "추정 비용 {{cost}}", { cost: jevCost(days.reduce((sum, day) => sum + (day.cost ?? 0), 0)) }) : "");
  const label = t("jev.chart.tokens.label", "일별 입력 토큰 — {{days}}", {
    days: jevDaysSaid(days, (day) => jevCount(day.tokens)),
  });
  const axis = jevNode("div", "jev-chart-axis",
    jevNode("span", "", document.createTextNode(jevDayName(days[0].startMs))),
    jevNode("span", "", document.createTextNode(jevDayName(days.at(-1).startMs))));
  card.querySelector(".jev-chart-body").replaceChildren(jevTokensPicture(days, label), axis);
  card.querySelector("tbody").replaceChildren(...days.map((day) => jevChartTableRow(jevDayName(day.startMs), jevCount(day.tokens), cost(day))));
}

/* (c): the features nearest their next judgment, each with its bar. */
function paintJevJudgment(card, entries, counting, summed) {
  if (counting || entries.length === 0) {
    jevChartEmpty(card, counting, summed
      ? t("jev.chart.judgment.summed", "판정은 프로젝트마다 따로 합니다 — 이 작업 공간을 고르면 보입니다")
      : t("jev.chart.judgment.empty", "판정을 기다리는 기능이 없습니다"));
    return;
  }
  const shown = entries.slice(0, JEV_JUDGMENT_ROWS);
  jevChartDrawn(card);
  jevSay(card.querySelector(".jev-chart-figure"), jevCount(entries.length));
  jevSay(card.querySelector(".jev-chart-sub"), entries.length > shown.length
    ? t("jev.chart.judgment.nearest", "판정을 기다리는 기능 · 가까운 {{count}}개", { count: jevCount(shown.length) })
    : t("jev.chart.judgment.sub", "판정을 기다리는 기능"));
  const list = jevNode("ol", "jev-judgment", ...shown.map((entry) => {
    const name = jevSeatName(entry.id);
    const row = jevNode("li", "jev-judgment-row",
      jevNode("span", "jev-judgment-name", document.createTextNode(name)),
      jevNode("span", "jev-judgment-owed", document.createTextNode(entry.words)),
      jevJudgmentPicture(entry, `${name} — ${entry.sample}, ${entry.words}`));
    row.dataset.jevJudgment = entry.id;
    return row;
  }));
  card.querySelector(".jev-chart-body").replaceChildren(list);
  card.querySelector("tbody").replaceChildren(...shown.map((entry) =>
    jevChartTableRow(jevSeatName(entry.id), `${jevCount(entry.have)}/${jevCount(entry.want)}`, jevCount(entry.owed))));
}

/* The drawer's table of a feature's days (t-9633): what each day asked, how
 * its judgments and the simplest method's compared, how long it took and the
 * input tokens it billed — the numbers the row's pictures draw. */
const JEV_DAY_COLUMNS = Object.freeze([
  { key: "jev.chart.head.day", word: "날짜" },
  { key: "jev.chart.head.rows", word: "요청" },
  { key: "jev.trend.agreement", word: "정확도" },
  { key: "jev.trend.baseline", word: "단순 방식" },
  { key: "jev.col.latency", word: "응답 시간" },
  { key: "jev.chart.head.tokens", word: "입력 토큰" },
]);

/* The drawer's week (t-9633): the accuracy picture large, with the days'
 * comparisons over the sample floor under it, the key to its marks, and the
 * table of the days. */
function paintJevDays(part, days) {
  const said = JEV_TRENDS.map((trend) => {
    const values = jevDaysSaid(days, (day) => (trend.read(day) === null ? null : jevPercent(trend.read(day))));
    return values ? t("jev.chart.series", "{{name}} — {{values}}", { name: t(trend.key, trend.word), values }) : "";
  }).filter(Boolean).join("; ");
  const compared = jevDaysSaid(days, (day) => (day.compared ? jevCount(day.compared) : null));
  part.querySelector(".jev-days-picture").replaceChildren(jevTrendPicture(days,
    [said, compared ? t("jev.chart.compared", "비교 표본 — {{values}}", { values: compared }) : ""].filter(Boolean).join("; "),
    JEV_CHART.trendWide));
  const key = (className, words) => jevNode("span", `jev-days-key ${className}`, document.createTextNode(words));
  part.querySelector(".jev-days-legend").replaceChildren(
    key("is-agreement", t("jev.trend.agreement", "정확도")),
    key("is-band", t("jev.chart.band", "신뢰 하한까지")),
    key("is-baseline", t("jev.trend.baseline", "단순 방식")),
    key("is-samples", t("jev.chart.floor", "비교 표본 (바닥 {{floor}}건)", { floor: jevCount(JEV_CHART.sampleFloor) })));
  const marks = (agreed, compared) => (compared ? `${jevCount(agreed)}/${jevCount(compared)}` : "—");
  part.querySelector(".jev-days-table tbody").replaceChildren(...days.map((day) => {
    const latency = day.p50 === null ? "—" : day.p95 === null ? `${jevMs(day.p50)} ms`
      : t("jev.chart.latencyDay", "{{p50}} ms (느릴 때 {{p95}})", { p50: jevMs(day.p50), p95: jevMs(day.p95) });
    return jevChartTableRow(jevDayName(day.startMs), day.rows ? jevCount(day.rows) : "—",
      marks(day.agreed, day.compared), marks(day.baseAgreed, day.baseCompared), latency, day.tokens ? jevCount(day.tokens) : "—");
  }));
}

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
 * wants some — in words; the bar filling toward the judgment is the chart's
 * over the table (t-9633 (c)) — and one sentence of why, unless the owed
 * samples already are the why; and a version only when it is not the one
 * over the table — each said once, in as few lines as a row can hold. */
function jevStatusCell(held, choice, standing, head) {
  const status = jevSeatStatus(held, choice);
  const state = JEV_STATUSES.find((one) => one.status === status);
  const said = state.waits?.[jevWaitsFor(held)] ?? state;
  const chip = jevNode("span", "jev-chip");
  chip.dataset.status = status;
  chip.textContent = t(said.key, said.word);
  // Summed over projects, a feature has no one judgment to wait on or cite:
  // each project judges its own rows (t-9091), and the line says in how
  // many of them it acts instead (`jevOwed` answers nothing for a sum).
  const across = held.across ?? null;
  const owed = jevOwed(held, standing);
  // A feature switched off asks nothing, so nothing counts toward its
  // next judgment: the chip says off, and no countdown stands beside it.
  const counting = owed !== null && owed.wants !== "next" && status !== "dormant";
  const lead = jevNode("div", "jev-status-lead", chip);
  if (counting) lead.append(jevNode("span", "jev-owed", document.createTextNode(jevOwedWords(owed, held))));
  const reason = across?.summed ? "" : jevStatusReason(held, status, choice, counting);
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
  // Why its rows compared nothing, on a line of its own under the chip: beside
  // the chip it would widen the column the whole table shares (t-9935).
  const why = jevWhyWords(held, status);
  if (why) {
    const said = jevHint("jev.why.tip", "비교하지 못한 판단의 까닭 — 지난 7일, 많은 순", jevNode("p", "jev-why", document.createTextNode(why)));
    said.dataset.jevWhy = "";
    cell.append(said);
  }
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
    if (!choice?.applies) {
      // The judged window's comparisons are the evidence: said with the
      // sentence, so a week whose strip compared nothing does not read as
      // a feature applying on nothing.
      const agreement = held.judged?.agreement;
      return agreement?.compared
        ? t("jev.risenWith", "근거가 충분해 자동으로 켜졌습니다 — 비교 {{count}}건, 신뢰 하한 {{pct}}",
          { count: jevCount(agreement.compared), pct: jevPercent(agreement.lowerBound) })
        : t("jev.risen", "근거가 충분해 자동으로 켜졌습니다");
    }
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

/* A feature's reasons its rows compared nothing (`notComparedBy`), the most
 * frequent first, each with how many rows gave it. */
function jevReasonsOf(by) {
  return Object.entries(by ?? {}).sort(([a, one], [b, other]) => other - one || a.localeCompare(b));
}

/* Why a feature held on a line that `compares` has too few marks, under its
 * state's chip (t-9935): its week's most frequent reasons its rows compared
 * nothing (`JEV_CHART.reasonsShown` of them), each with its count — "" for a
 * feature held on another line or on none, for one a person has to clear
 * first (the key, a folder's consent) or that asks nothing, and for a sum of
 * projects, which has no one judgment to explain (t-9091). */
function jevWhyWords(held, status) {
  if (status === "dormant" || status === "blocked" || held.across?.summed) return "";
  if (!JEV_LINES[held.verdict?.line]?.compares) return "";
  return jevReasonsOf(held.agreementWeek?.notComparedBy).slice(0, JEV_CHART.reasonsShown)
    .map(([token, count]) => t("jev.why.count", "{{reason}} {{count}}건", { reason: jevReasonWords(token), count: jevCount(count) }))
    .join(", ");
}

/* Why the door refused a feature that waits on a person, in a sentence:
 * the key, or the folder's consent (`jevWaitsFor`). */
function jevBlockedWords(held) {
  return jevWaitsFor(held) === "consent"
    ? t("jev.needs.consent", "동의하지 않은 폴더라 판단을 보내지 않았습니다")
    : t("jev.needs.key", "API 키가 없거나 거절되어 판단을 요청하지 못했습니다");
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
  jevNameColumns(view);
  const head = jevHeadModel(numbers);
  const latency = jevLatencyAt.get(view) ?? false;
  const drawn = [];
  for (const id of [...inUse, ...(jevUnusedOpen ? unused : [])]) {
    const row = jevDashRow(body, id);
    paintJevRow(row, id, heldOf.get(id) ?? null, switches.find((one) => one.id === id) ?? null, head, latency);
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
  paintJevCharts(view, inUse, heldOf, numbers);
  // The scope just chosen is being counted: what stands counted the other
  // one, and says so by stepping back until the answer lands (t-9633).
  const stale = jevScope !== null && jevScope.scope !== jevScopeChoice;
  view.toggleAttribute("data-jev-stale", stale);
  if (view.getAttribute("aria-busy") !== String(stale)) view.setAttribute("aria-busy", String(stale));
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
 * feature's numbers say for it — with a tip, by its own key, where what it
 * says needs its meaning said. Rebuilt whole: a handful of rows. */
function jevFacts(list, facts) {
  list.replaceChildren(...facts.flatMap(({ key, word, said, tip }) => {
    const value = jevNode("dd", "", document.createTextNode(said));
    return [jevText(key, word, "dt"), tip ? jevHint(tip.key, tip.word, value) : value];
  }));
}

/* A rate the core keeps per thousand, as the whole per-thousand it sends —
 * the sign is the catalog's — or a dash for none. */
function jevPermille(permille) {
  if (permille === null || permille === undefined) return "—";
  return t("jev.permille", "{{count}}‰", { count: jevCount(permille) });
}

/* What a sum of projects cannot say — each project judges its own rows
 * (t-9091) — standing where the fact would, with why as its tip. */
const JEV_PER_PROJECT = Object.freeze({
  said: "—",
  tip: { key: "jev.drawer.perProject", word: "프로젝트마다 따로 판정하는 값이라 합산하지 않습니다 — 이 작업 공간을 고르면 보입니다" },
});

/* The drawer's check and confidence bar (t-9935): what the check found — the
 * line the feature is held on, every bar cleared, or no check to clear — and
 * the bar its answers act from: the table's (with what it applies and how
 * often that is wrong beside the simplest method), one its record draws that
 * is not read yet, why its record draws none, or none read at all. A sum of
 * projects says both are each project's own. */
function jevWhyFacts(held) {
  const check = { key: "jev.drawer.check", word: "판정 결과" };
  const bar = { key: "jev.drawer.act", word: "확신도 기준" };
  if (held.across?.summed) return [{ ...check, ...JEV_PER_PROJECT }, { ...bar, ...JEV_PER_PROJECT }];
  const line = held.verdict?.line;
  const facts = [{ ...check, said: !held.verdict ? t("jev.neverRises", "자동 적용 대상이 아니라 기록만 합니다")
    : line ? jevLineWords(line) || line : t("jev.drawer.cleared", "모든 기준을 넘었습니다") }];
  const calibration = held.calibration;
  if (!calibration) return facts;
  const at = (from, numbers) => [
    { ...bar, said: from },
    { key: "jev.drawer.applyShare", word: "적용 몫", said: jevPercent(numbers.applyShare) },
    { key: "jev.drawer.appliedError", word: "적용한 답의 오류", said: jevPermille(numbers.errorPermille) },
    { key: "jev.drawer.baselineError", word: "단순 방식의 오류", said: jevPermille(numbers.baselineErrorPermille) },
  ];
  if (!calibration.readsActLine) {
    return [...facts, { ...bar, said: t("jev.act.unread", "쓰지 않음 — 확신도와 상관없이 답을 모두 적용합니다") }];
  }
  if (calibration.tableLine !== null && calibration.tableLine !== undefined) {
    return [...facts, ...at(t("jev.act.from", "{{line}} 이상", { line: jevFloor(calibration.tableLine) }), {
      applyShare: held.applyShare, errorPermille: held.appliedErrorPermille, baselineErrorPermille: held.baselineErrorPermille,
    })];
  }
  if (calibration.actFromPermille !== null && calibration.actFromPermille !== undefined) {
    return [...facts, ...at(t("jev.act.drawn", "{{line}} 이상 — 기록이 가리키지만 아직 쓰지 않음", {
      line: jevFloor(calibration.actFromPermille),
    }), calibration.drawn ?? {})];
  }
  return [...facts, { ...bar, said: calibration.reason
    ? t("jev.act.none", "없음 — {{why}}", { why: jevReasonWords(calibration.reason) }) : t("jev.act.unset", "없음") }];
}

/* The drawer's reasons its rows compared nothing (t-9935): the week's, the
 * most frequent first, each with its count, its share of them all and
 * today's count, over a bar of that share — or nothing, for a week whose rows
 * all compared or said nothing of why. */
function paintJevWhy(part, held) {
  const reasons = jevReasonsOf(held.agreementWeek?.notComparedBy);
  const today = held.days?.at(-1)?.agreement?.notComparedBy ?? {};
  const total = reasons.reduce((sum, [, count]) => sum + count, 0);
  const head = part.querySelector("[data-jev-why-head]");
  head.hidden = total === 0;
  head.textContent = total === 0 ? "" : t("jev.drawer.notComparedBy", "비교하지 못한 판단 {{count}}건 — 지난 7일, 까닭별", { count: jevCount(total) });
  part.querySelector(".jev-judgment").replaceChildren(...reasons.map(([token, count]) => {
    const words = jevReasonWords(token);
    const said = t("jev.drawer.reasonCount", "{{count}}건 ({{share}}) · 오늘 {{today}}건", {
      count: jevCount(count), share: jevPercent(count / total), today: jevCount(today[token] ?? 0),
    });
    const row = jevNode("li", "jev-judgment-row",
      jevNode("span", "jev-judgment-name", document.createTextNode(words)),
      jevNode("span", "jev-judgment-owed", document.createTextNode(said)),
      jevJudgmentPicture({ have: count, want: total }, `${words} — ${said}`));
    row.dataset.jevReason = token;
    return row;
  }));
}

/* The open feature's drawer (`id`, or null for none), from what is in hand:
 * the settings answer's standing, zo's numbers (`held`, null until zo has
 * counted) and the markup's words. */
function paintJevDrawer(view, id, held) {
  const drawer = view.querySelector(".jev-drawer");
  if (drawer.hidden !== (id === null)) drawer.hidden = id === null;
  // Only the rows whose press opens or closes it change (t-9633 §3).
  for (const row of view.querySelectorAll("[data-jev-dash-row]")) {
    const open = row.dataset.jevDashRow === id;
    row.classList.toggle("is-open", open);
    const press = row.querySelector(".jev-row-open");
    if (press && press.getAttribute("aria-expanded") !== String(open)) press.setAttribute("aria-expanded", String(open));
  }
  if (id === null) return;
  const standing = jevSeat(typesafeState ?? {}, id);
  // Drawn again when its feature's numbers, its switch or the language moved
  // — or a minute passed, which moves how long ago its last judgments were.
  if (!jevMoved(drawer, id, JSON.stringify(held), JSON.stringify(standing), String(typesafeState?.model?.pinned ?? ""),
    jevLanguage(), String(Math.floor(Date.now() / 60_000)))) return;
  const feature = jevFeature(id);
  const name = jevSeatName(id);
  drawer.setAttribute("aria-label", name);
  drawer.querySelector(".jev-drawer-title").textContent = name;
  drawer.querySelector(".jev-drawer-id").textContent = id;
  drawer.querySelector(".jev-drawer-summary").textContent = feature.summary ? t(feature.summary.key, feature.summary.source) : "";
  drawer.querySelector(".jev-drawer-written").hidden = !standing?.written;
  const part = (name) => drawer.querySelector(`[data-jev-drawer-part="${name}"]`);

  // What stands between it and automatic use, once zo has counted it
  // (t-9935).
  part("why").hidden = !held;
  if (held) {
    jevFacts(part("why").querySelector("dl"), jevWhyFacts(held));
    paintJevWhy(part("why"), held);
  }

  // Its week, large, with the table of its days (t-9633).
  const days = jevWordsOf(held).read;
  const asked = days.some((day) => day.rows > 0 || day.compared > 0);
  part("days").hidden = !asked;
  if (asked) paintJevDays(part("days"), days);

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
  row.setAttribute("role", "row");
  for (const column of JEV_COLUMNS) {
    const cell = document.createElement("td");
    cell.dataset.jevCell = column.cell;
    cell.setAttribute("role", "cell");
    row.append(cell);
  }
  body.append(row);
  return row;
}

/* One feature's row, drawn from what is in hand — and only when that moved
 * since it was last drawn (`jevMoved`): its numbers, its switch, the version
 * over the table and the language, each of which a cell reads. */
function paintJevRow(row, id, held, standing, head, latency) {
  const choice = standing ? jevSeatChoice(typesafeState, id) : null;
  const words = jevWordsOf(held);
  const language = jevLanguage();
  if (!jevMoved(row, words.rest, words.days, JSON.stringify(standing), JSON.stringify(choice), head ?? "", String(latency), language)) return;
  // A row's cells stand in `JEV_COLUMNS`' order (`jevDashRow`).
  const cell = (name) => row.cells[JEV_COLUMN_AT.get(name)];
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
  const applying = held ? held.applies : Boolean(choice?.applies);
  row.classList.toggle("is-applying", Boolean(applying));
  row.classList.toggle("is-quiet", !held || held.week.rows === 0);
  if (!held) {
    for (const column of JEV_COLUMNS) {
      if (column.cell === "seat") continue;
      cell(column.cell).textContent = "—";
      // A dash stands where a picture was: the next numbers draw it anew.
      jevDrawnFrom.delete(cell(column.cell));
    }
    return;
  }
  const rows = cell("rows");
  rows.replaceChildren(
    jevFact("today", jevCount(held.today.rows), "jev-fact-strong"), document.createTextNode(" · "),
    jevFact("week", jevCount(held.week.rows)));
  // What did not come back, by token, in words — the ones a person clears
  // are buttons to where they are cleared (t-6243 D5).
  const failures = held.week.failures ?? [];
  if (failures.length > 0) {
    rows.append(jevNode("div", "jev-tokens", ...failures.map(jevTokenChip)));
  } else if (held.week.refused > 0) {
    rows.append(jevNode("div", "jev-tokens", jevFact("refused", t("jev.refusedCount", "거절 {{count}}건", { count: jevCount(held.week.refused) }), "jev-token")));
  }
  const share = cell("answered");
  share.replaceChildren();
  if (held.week.rows === 0) {
    share.textContent = "—";
  } else if (held.week.answered === 0 && held.week.refused > 0) {
    // Nothing answered because the door refused everything: that, not a
    // share of zero and an empty bound, is the fact (t-6243 D3).
    share.append(jevFact("refused", t("jev.refusedCount", "거절 {{count}}건", { count: jevCount(held.week.refused) }), "jev-refused"));
  } else if (held.week.rows < JEV_CHART.sampleFloor) {
    share.append(jevFact("sample", jevSampleWords(held.week.rows), "jev-sample"));
  } else {
    const bound = jevFact("bound", t("jev.bound", "신뢰 하한 {{pct}}", { pct: jevPercent(held.week.answeredLowerBound) }), "jev-bound");
    // Where that bound stands against the line for acting by itself, said
    // as its tip — the number a person hovers, not a fact beside it.
    if (held.clearsRiseFloor !== null && held.clearsRiseFloor !== undefined && held.riseFloorPermille) {
      const floor = jevFloor(held.riseFloorPermille);
      bound.dataset.tip = held.clearsRiseFloor
        ? t("jev.clearsFloor", "응답률 신뢰 하한이 자동 적용 기준({{floor}}) 이상", { floor })
        : t("jev.underFloor", "응답률 신뢰 하한이 자동 적용 기준({{floor}}) 미만", { floor });
      bound.classList.toggle("is-under", !held.clearsRiseFloor);
    }
    share.append(jevFact("share", jevPercent(held.week.answeredShare), "jev-share"), document.createTextNode(" "), bound);
  }
  // The row's pictures draw its days, and are drawn again only when those,
  // or what a cell says beside them, moved — a count that moved alone draws
  // its own cell.
  const status = jevSeatStatus(held, choice);
  const summed = String(Boolean(held.across?.summed));
  if (jevMoved(cell("latency"), words.days, String(held.week.p50Ms), String(held.week.p95Ms), summed, String(latency), language)) {
    cell("latency").replaceChildren(jevLatencyCell(held, words.read, latency));
  }
  const acted = cell("acted");
  acted.replaceChildren(
    jevFact("applied", held.week.rows === 0 ? "—" : jevCount(held.week.applied ?? 0)), document.createTextNode(" · "),
    jevFact("agreement", jevAgreementWords(held, standing)));
  cell("cost").textContent = jevCost(held.costUsd);
  if (jevMoved(cell("trend"), words.days, status, language)) cell("trend").replaceChildren(jevTrendCell(words.read, status));
  cell("status").replaceChildren(jevStatusCell(held, choice, standing, head));
}

/* The row that stands first when no feature was asked all week: whether Jev
 * is off — the switch over the table turns it on — or on and quiet. */
function jevEmptyRow(body) {
  let row = body.querySelector("[data-jev-empty]");
  if (!row) {
    const cell = jevNode("td", "", jevNode("span", "jev-empty"));
    cell.colSpan = JEV_COLUMNS.length;
    cell.setAttribute("role", "cell");
    row = jevNode("tr", "jev-empty-row", cell);
    row.setAttribute("role", "row");
    row.dataset.jevEmpty = "";
    body.prepend(row);
  }
  jevSay(row.querySelector(".jev-empty"), typesafeState?.jev && !typesafeState.jev.on
    ? t("jev.empty.off", "Jev가 꺼져 있어 판단을 요청하지 않습니다. 위의 스위치로 켤 수 있습니다.")
    : t("jev.empty.quiet", "지난 7일 판단 요청이 없었습니다. 기능이 판단을 요청하면 여기에 쌓입니다."));
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
    cell.setAttribute("role", "cell");
    row = jevNode("tr", "jev-fold-row", cell);
    row.setAttribute("role", "row");
    row.dataset.jevFold = "";
    body.append(row);
  }
  const press = row.querySelector(".jev-fold");
  if (press.getAttribute("aria-expanded") !== String(jevUnusedOpen)) press.setAttribute("aria-expanded", String(jevUnusedOpen));
  jevSay(press, jevUnusedOpen
    ? t("jev.unused.hide", "사용 안 한 기능 {{count}}개 접기", { count: jevCount(count) })
    : t("jev.unused.show", "사용 안 한 기능 {{count}}개 보기", { count: jevCount(count) }));
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
    jevSay(holder.querySelector("dd"), text);
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
  if (day.hidden !== !jevDay) day.hidden = !jevDay;
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
