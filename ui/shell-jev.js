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
/* When each judgment provider was applied from this window, by its identity
 * (`model_connection_checks`, t-76598) — `null` until the record is read. */
let jevApplied = null;
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
      let remembered;
      [held, jevDay, remembered] = await Promise.all([
        invoke("jev_summary", { ...(recent ? { recent: JEV_RECENT_ROWS } : {}), scope: jevNumbersAskingScope }),
        // The dashboard's strip reads the day's count beside the ledgers: a
        // file's length, so it rides every refresh; the card draws none.
        recent ? invoke("jev_day").catch(() => null) : jevDay,
        // Since when the model over the table counts: the window's record of
        // its applies, a small file — the dashboard's alone.
        recent ? invoke("remembered_model_connection_checks").catch(() => null) : null,
      ]);
      if (remembered?.applied) jevApplied = remembered.applied;
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
 * every folder and preserves saved choices; other features use their recommended mode. Off sends
 * nothing. The card's own step runner, so a refused press is said the same
 * way wherever it was made. */
function setJevEnabled(on) {
  return runKeyCardStep(JUDGMENT_SWITCH_CARD, () => invoke("set_jev_enabled", { on }), (state) => {
    const words = jevEnabledSaid(state);
    jevSayStatus(words);
    return words;
  });
}

/* What a press did, in the words of where Jev now stands. */
function jevEnabledSaid(state) {
  return state.jev?.on
    ? (state.switches?.some((row) => row.decision)
      ? t("jev.builtin.turnedOn", "켰습니다. 저장한 결정 설정과 삭제 상태를 유지합니다.")
      : t("settings.typesafe.turnedOn", "켰습니다. 모든 폴더에서 기능마다 권장 설정으로 판단을 요청합니다."))
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
  execution_quality: { key: "settings.typesafe.lineExecutionQuality", word: "같은 난이도에서 첫 시도 성공률이 낮아졌습니다" },
  execution_costs_missing: { key: "settings.typesafe.lineExecutionCostsMissing", word: "비교할 실행의 토큰·시간 측정이 부족합니다" },
  execution_savings: { key: "settings.typesafe.lineExecutionSavings", word: "첫 시도 성공률을 유지하며 토큰과 시간을 줄인 증거가 부족합니다" },
  apply_share: { key: "settings.typesafe.lineApplyShare", word: "확신도 기준을 넘는 답이 너무 적습니다" },
  proxy_only: { key: "settings.typesafe.lineProxyOnly", word: "정확도가 대용 라벨이나 출처 없는 라벨에만 기대고 있습니다", compares: true },
  calibration: { key: "settings.typesafe.lineCalibration", word: "행동 기준선이 아직 보정되지 않았거나 기록이 부족합니다" },
  bar: { key: "settings.typesafe.lineBar", word: "짝 기록이 승격 기준(표본·품질·비용·지연)을 넘지 못했습니다" },
  bundle: { key: "settings.typesafe.lineBundle", word: "고정된 사람 묶음이 없거나 고정되지 않았거나 카나리가 맞지 않습니다" },
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
 * file pick's `no_file_edited`, the claim check's `not_model_comparison`,
 * the mail triage's `superseded` and `unhandled`, t-9471, the tool text
 * guard's `ignored`) — and why a feature's record draws no confidence bar to
 * act from
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
  no_teacher: { key: "jev.reason.noTeacher", word: "비교할 깊이 지정 없음" },
  no_note_touched: { key: "jev.reason.noNoteTouched", word: "노트 안 씀" },
  unanswered: { key: "jev.reason.unanswered", word: "결과 없음" },
  no_file_edited: { key: "jev.reason.noFileEdited", word: "편집한 파일 없음" },
  not_model_comparison: { key: "jev.reason.notModelComparison", word: "모델 비교 아님" },
  ignored: { key: "jev.reason.ignored", word: "다음 걸음이 따르지 않음" },
  no_confidence: { key: "jev.reason.noConfidence", word: "답에 확신도가 없음" },
  whole: { key: "jev.reason.whole", word: "모든 답이 이미 기준을 넘음" },
  one_colour: { key: "jev.reason.oneColour", word: "확신도로 답이 나뉘지 않음" },
  non_monotone: { key: "jev.reason.nonMonotone", word: "확신도가 높은 답이 더 자주 틀림" },
  no_lift: { key: "jev.reason.noLift", word: "기준을 그어도 정확도가 거의 같음" },
  superseded: { key: "jev.reason.superseded", word: "새 편지가 대신함" },
  unhandled: { key: "jev.reason.unhandled", word: "코디네이터 행동이 모자람" },
});

/* A reason in words: this table's, else — a record that drew no bar because
 * one of the judge's own lines broke (`NoLine::Line`) — that line's, else the
 * token itself, so a word no table knows yet still says what zo said. */
function jevReasonWords(token) {
  const said = JEV_REASONS[token] ?? JEV_LINES[token];
  return said ? t(said.key, said.word) : token;
}

/* Why the window cannot ask a feature of an agent's work, or what is said
 * beside the agent's count — what of the feature's reading its hooks miss,
 * or that its own runtime asks (t-10916): the core's words
 * (`hook_guard::Unseen`), else the word itself. */
const JEV_UNSEEN = Object.freeze({
  own_runtime: { key: "jev.unseen.ownRuntime", word: "zo가 직접 물음" },
  no_hooks: { key: "jev.unseen.noHooks", word: "훅을 설치하지 않는 에이전트" },
  no_event_before: { key: "jev.unseen.noEventBefore", word: "실행 전에 알리지 않아 실행 뒤에 물음" },
  no_prompt_event: { key: "jev.unseen.noPromptEvent", word: "사람의 요청을 알리지 않음" },
  no_stop_flag: { key: "jev.unseen.noStopFlag", word: "멈춤을 알리지 않음" },
  no_context_road: { key: "jev.unseen.noContextRoad", word: "턴 시작에 안내를 받지 않음" },
});

function jevUnseenWords(token) {
  const said = JEV_UNSEEN[token];
  return said ? t(said.key, said.word) : token;
}

/* One agent's line under a feature a pane's hooks ask: that the window
 * cannot see its work and why, or its week's count with what is said beside
 * it. */
function jevAskerWords(one) {
  if (one.unseen) return t("jev.askers.unseen", "못 봄 — {{why}}", { why: jevUnseenWords(one.unseen) });
  return [t("jev.askers.count", "{{count}}건", { count: jevCount(one.requests) }), ...(one.notes ?? []).map(jevUnseenWords)].join(" · ");
}

/* Under a feature's week count, who asked it (t-10916): each agent with
 * requests this week, by name and count, and how many agents' work the
 * window cannot see — which, and why, as that chip's tip. Nothing for a
 * feature no pane's hooks ask. */
function jevAskersLine(held) {
  const askers = held.askers ?? [];
  const blind = askers.filter((one) => one.unseen);
  const chips = askers.filter((one) => one.requests > 0 && !one.unseen)
    .map((one) => jevFact("asker", `${one.label} ${jevCount(one.requests)}`, "jev-token"));
  if (blind.length > 0) {
    const chip = jevFact("unseen", t("jev.askers.unseenCount", "못 봄 {{count}}", { count: jevCount(blind.length) }), "jev-token");
    chip.dataset.tip = blind.map((one) => `${one.label} — ${jevUnseenWords(one.unseen)}`).join(" · ");
    chips.push(chip);
  }
  if (chips.length === 0) return null;
  const line = jevNode("div", "jev-tokens", ...chips);
  line.dataset.jevAskers = "";
  return line;
}

/* The version a change of version cut away from the judged window, when one
 * did — the reason a thin window gives for itself. */
function jevCutWords(held) {
  if (!held.verdict?.cutModel) return "";
  return t("settings.typesafe.seatCut", "이전 버전 {{cut}}의 기록은 제외", { cut: jevVersion(held.verdict.cutModel).short });
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
  no_key: { key: "jev.token.noKey", word: "API 키 없음", fix: "key", saidKey: "settings.typesafe.noKey", said: "키를 찾지 못했습니다 — 키를 저장한 뒤 다시 확인하세요." },
  unauthorized: { key: "jev.token.unauthorized", word: "키 거절", fix: "key", saidKey: "settings.typesafe.unauthorized", said: "키가 거절되었습니다 — 키를 다시 확인하세요." },
  not_consented: { key: "jev.token.notConsented", word: "동의 안 된 폴더", fix: "consent" },
  settings_changed: { key: "jev.token.settingsChanged", word: "설정 변경으로 취소됨", saidKey: "settings.judgment.changed", said: "판단 제공자 설정을 다시 확인하세요. 이전 모델의 응답은 사용하지 않았습니다." },
  provider_settings: { key: "jev.token.providerSettings", word: "판단 제공자 설정 오류", saidKey: "settings.judgment.changed", said: "판단 제공자 설정을 다시 확인하세요. 이전 모델의 응답은 사용하지 않았습니다." },
  provider_restart_required: { key: "jev.token.providerRestartRequired", word: "zo 재시작 필요", saidKey: "settings.judgment.restartRequired", said: "zo를 시작할 때 받은 키가 이전 모델 연결의 것입니다. 저장된 연결의 키로 zo를 다시 시작하세요." },
  provider_address: { key: "jev.token.providerAddress", word: "판단 제공자 주소 오류", saidKey: "settings.judgment.invalidAddress", said: "http:// 또는 https://로 시작하는 기본 URL을 입력하세요. 키는 주소 대신 API 키 칸에 입력하세요." },
  provider_not_loopback: { key: "jev.token.providerNotLoopback", word: "이 컴퓨터의 주소가 아님", saidKey: "settings.judgment.notLocal", said: "다른 컴퓨터의 서버는 ‘다른 판단 서버’를 선택하세요. 로컬 연결은 localhost 또는 루프백 IP를 사용합니다." },
  provider_https_required: { key: "jev.token.providerHttpsRequired", word: "HTTPS 주소 필요", saidKey: "settings.judgment.httpsRequired", said: "이 주소는 HTTPS가 필요합니다. 사설망의 HTTP 서버라면 호스트 이름 대신 사설 IP 주소를 입력하세요." },
  provider_model: { key: "jev.token.providerModel", word: "판단 모델 이름 오류", saidKey: "settings.judgment.invalidModel", said: "서버에서 모델을 고르거나 정확한 모델 ID를 입력하세요." },
  provider_request: { key: "jev.token.providerRequest", word: "판단 요청 형식 오류" },
  provider_response_too_large: { key: "jev.token.responseTooLarge", word: "판단 응답 크기 초과" },
  provider_context_truncated: { key: "jev.token.contextTruncated", word: "입력 일부 누락", saidKey: "settings.judgment.truncated", said: "모델이 입력 일부를 읽지 못했습니다. 더 긴 입력을 지원하는 판단 모델이나 서버 설정이 필요합니다." },
  provider_deadline_budget: { key: "jev.token.deadlineBudget", word: "남은 시간 부족" },
  provider_client_outdated: { key: "jev.token.clientOutdated", word: "판단 클라이언트 업데이트 필요", saidKey: "settings.judgment.outdated", said: "판단 제공자를 지원하는 최신 zo가 필요합니다. 앱과 zo의 업데이트 상태를 확인하세요." },
  rules_unavailable: { key: "jev.token.rulesUnavailable", word: "규칙 정의를 확인하거나 다시 준비해야 함" },
  budget: { key: "jev.token.budget", word: "하루 한도 초과", fix: "budget" },
  hourly_budget: { key: "jev.token.hourlyBudget", word: "시간당 한도 초과" },
  resting: { key: "jev.token.resting", word: "쉬는 중" },
  timeout: { key: "jev.token.timeout", word: "시간 초과" },
  schema: { key: "jev.token.schema", word: "형식 오류" },
  transport: { key: "jev.token.transport", word: "연결 실패" },
  overloaded: { key: "jev.token.overloaded", word: "서버 과부하" },
  rate_limited: { key: "jev.token.rateLimited", word: "요청 제한" },
  invalid_request: { key: "jev.token.invalidRequest", word: "잘못된 요청" },
  http: { key: "jev.token.http", word: "서버 오류" },
  http_402: { key: "jev.token.paymentRequired", word: "결제·이용 한도", saidKey: "settings.judgment.paymentRequired", said: "제공자가 결제·이용 한도로 거절했습니다. 계정 상태를 확인하거나 다른 판단 제공자를 선택하세요." },
});

/* A token's row: its own, or its family's — a ledger keeps an unnamed HTTP
 * failure with its status (`http_503`, zo's `ledger_token`) and a schema
 * refusal with the rule it broke (`schema_…`, the core's
 * `names_a_schema_failure`). `null` for a token no row words. */
function jevTokenRow(token) {
  if (Object.hasOwn(JEV_TOKENS, token)) return JEV_TOKENS[token];
  const family = token.startsWith("http_") ? "http" : token.startsWith("schema_") ? "schema" : token;
  return JEV_TOKENS[family] ?? null;
}

/* What zo's door rests a feature for (t-58193), by the word zo sends
 * (`jev_gate::RestCause::word`), each with the words a person reads for it,
 * never zo's word itself. `typesafe_settings.rs` holds this table to the
 * door's causes. */
const JEV_REST_CAUSES = Object.freeze({
  waiting: { key: "jev.rest.cause.waiting", word: "서버가 기다려 달라고 했습니다" },
  too_long: { key: "jev.rest.cause.tooLong", word: "가장 짧은 요청도 이 모델이 읽기에 깁니다" },
  cut: { key: "jev.rest.cause.cut", word: "모델이 읽는 길이를 넘음" },
  timeout: { key: "jev.rest.cause.timeout", word: "시간 초과" },
  unreachable: { key: "jev.rest.cause.unreachable", word: "서버에 닿지 못함" },
  refused: { key: "jev.rest.cause.refused", word: "서버가 거절함" },
});

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

/* Whether the mode zo names for a feature, for the folder it counted, asks:
 * for a person's own decisions it is the lower of their switch and their own
 * modes, which the settings answer cannot say. Read among the modes the
 * feature offers; a zo that names none leaves the switch to say it. */
function jevZoAsks(seat, held) {
  const named = held?.mode ? seat.modes?.find((choice) => choice.mode === held.mode) : null;
  return named?.asks !== false;
}

/* Whether a recording feature was measured and held under a line of its own
 * rather than still counting samples (`JEV_LINES`' `wants`). */
function jevUnderBar(held) {
  const line = held?.verdict?.line;
  return Boolean(line) && !JEV_LINES[line]?.wants && !held.applies;
}

/* The order the settings card lists features in (t-11989): the ones acting
 * first, then the ones waiting on a person, then the ones still recording —
 * the table's own order inside each. */
const JEV_STANDING_ORDER = Object.freeze(["applying", "blocked", "recording"]);

/* The settings card's line per feature while Jev is on (t-11989): where it
 * stands — acting, or recording with the samples its next judgment waits on,
 * or measured under its bar — read off the numbers zo answered (`zo jev
 * summary`, the table's own reading: `jevSeatStatus`, `jevOwed`), never off
 * a mode word. A feature whose mode asks nothing is left out, and the list is
 * hidden while the switch is off. The state is said in words; the dot beside
 * them only repeats it. */
function paintJevStandings(root = document) {
  const holder = root.querySelector("[data-jev-standings]");
  if (!holder) return;
  const on = Boolean(typesafeState?.jev?.on);
  holder.hidden = !on;
  if (!on) return;
  const heldOf = new Map((jevNumbers ?? []).map((held) => [held.id, held]));
  const rows = [];
  for (const seat of typesafeState.switches ?? []) {
    const choice = jevSeatChoice(typesafeState, seat.id);
    const held = heldOf.get(seat.id);
    if (!choice?.asks || !held || !jevZoAsks(seat, held)) continue;
    rows.push({ seat, held, status: jevSeatStatus(held, choice) });
  }
  rows.sort((a, b) => JEV_STANDING_ORDER.indexOf(a.status) - JEV_STANDING_ORDER.indexOf(b.status));
  const note = holder.querySelector("[data-jev-standings-note]");
  if (note) {
    note.hidden = rows.length > 0;
    note.textContent = rows.length > 0 ? ""
      : jevNumbers === null && jevNumbersError
        ? t("settings.typesafe.standingsUnanswered", "zo가 기능별 상태를 알려 주지 않았습니다.")
        : t("settings.typesafe.standingsAsking", "zo에게 기능별 상태를 묻는 중입니다.");
  }
  holder.querySelector("[data-jev-standings-list]")?.replaceChildren(...rows.map(jevStandingRow));
}

/* One feature's line on the card: its name, and its state with what the
 * state is waiting on — 「기록 중 · 13/53」 while the judge counts samples
 * (the sentence the table says, 「판정 표본 13/53건」, as its tip), 「기록 중
 * · 기준 미달」 once measured under its bar, the state alone otherwise. */
function jevStandingRow({ seat, held, status }) {
  const words = jevFeature(seat.id).name;
  const name = jevNode("span", "settings-jev-standing-name",
    document.createTextNode(words ? t(words.key, words.source) : seat.id));
  const state = JEV_STATUSES.find((one) => one.status === status);
  const word = state.waits?.[jevWaitsFor(held)] ?? state;
  const said = t(word.key, word.word);
  const owed = status === "recording" ? jevOwed(held, seat) : null;
  const counting = owed !== null && owed.wants !== "next";
  const detail = counting ? `${jevCount(owed.have)}/${jevCount(owed.want)}`
    : status === "recording" && jevUnderBar(held) ? t(JEV_UNDER.key, JEV_UNDER.word) : "";
  const standing = jevNode("span", "settings-jev-standing", document.createTextNode(detail
    ? t("settings.typesafe.standingWith", "{{state}} · {{detail}}", { state: said, detail })
    : said));
  standing.dataset.status = status;
  if (counting) standing.dataset.tip = jevSampleSaid(owed);
  const row = jevNode("li", "settings-jev-standing-row", name, standing);
  row.dataset.jevStanding = seat.id;
  return row;
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
  root.setAttribute("aria-label", t("jev.title", "의사결정 모델"));

  // The drawer a feature's row opens (t-6277 D6), first so it stays at the
  // top of the page while the page scrolls under it.
  root.append(jevNode("div", "jev-drawer-dock", jevBuildDrawer()));

  // The eyebrow is a name, the same in every language, so it takes no key.
  const heading = jevNode("div", "jev-heading",
    jevNode("p", "jev-eyebrow", document.createTextNode("TYPESAFE · JEV")),
    jevText("jev.title", "의사결정 모델", "h1", "jev-title"),
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
  if (mirror) {
    mirror.querySelector("[data-jev-switch-row]")?.append(jevCustomGuide());
    root.append(mirror);
  }

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
  root.append(charts, jevCustomPart());

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
  root.append(jevNode("div", "jev-table-wrap", table), jevBuiltinTrashPart());

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
  const fixed = jevText("jev.builtin.editable", "기본 결정도 수정하거나 삭제할 수 있습니다. 수정한 질문·판단 기준은 이 결정에 적용됩니다.", "p", "jev-drawer-fixed");
  fixed.dataset.jevDrawerFixed = "";
  const drawer = jevNode("aside", "jev-drawer", head,
    jevNode("p", "jev-drawer-summary"), fixed,
    jevText("jev.drawer.written", "설정 파일에 이 기능만 따로 정해 두어 스위치를 따르지 않습니다. 스위치를 다시 켜면 권장 설정으로 돌아갑니다.", "p", "jev-drawer-written"),
    part("why", "jev.drawer.why", "자동 적용 판정", jevNode("dl", "jev-drawer-facts"), whyHead, jevNode("ol", "jev-judgment")),
    part("days", "jev.drawer.days", "날마다", jevNode("div", "jev-days-picture"), jevNode("div", "jev-days-legend"),
      jevNode("table", "jev-days-table", jevNode("thead", "", jevNode("tr", "", ...JEV_DAY_COLUMNS.map((column) => {
        const head = jevText(column.key, column.word, "th");
        head.scope = "col";
        return head;
      }))), document.createElement("tbody"))),
    part("askers", "jev.drawer.askers", "누가 물었나", jevNode("dl", "jev-drawer-facts")),
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
    if (jevBuiltinPressed(event)) return;
    const chip = event.target.closest("[data-jev-fix]");
    if (chip) {
      jevFix(chip.dataset.jevFix);
      return;
    }
    const row = event.target.closest("[data-jev-dash-row]");
    if (row) jevToggleDrawer(host, row.dataset.jevDashRow);
  });
  host.querySelector(".jev-drawer-close").addEventListener("click", () => jevToggleDrawer(host, null));
  wireJevCustom(host);
  host.querySelector("[data-jev-builtin-trash]")?.addEventListener("click", jevBuiltinPressed);
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
  if (jevCustom === null) void loadJevCustom();
  void refreshJevDashboard();
}

async function refreshJevDashboard({ force = false } = {}) {
  const fresh = () => jevNumbers !== null && jevNumbersRecent && Date.now() - jevNumbersAt < JEV_POLL_MS;
  if (!force && fresh()) return;
  // The switches' standing comes with the settings answer, the numbers with
  // zo's; the card's own refresh asks both and hands the numbers here.
  if (!typesafeState) {
    await refreshTypeSafe();
    if (jevCustom === null) void loadJevCustom();
    if (!force && fresh()) return;
  }
  await Promise.all([loadJevNumbers({ recent: true }), loadJevCustom()]);
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
  // The settings card's standing lines read the same numbers (t-11989).
  paintJevStandings();
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
  const words = t(hint.key, hint.source, jevHintVars(jevSeat(typesafeState ?? {}, id)));
  const end = words.search(/[.!?](\s|$)|[。！？]/u);
  return end < 0 ? words : words.slice(0, end + 1).trim();
}

/* A feature's name, in the language in force: the settings markup's
 * (`jevFeature`), so the dashboard carries no second list of names. */
function jevSeatName(id) {
  const edited = jevBuiltinSaved(id).name;
  if (edited) return edited;
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

/* Milliseconds in a second, for the wall a feature's words name in seconds. */
const JEV_MS_PER_SECOND = 1_000;

/* What a feature's paragraph fills in from the feature's own row of the
 * backend's answer (`switches`): the wall its words name, in seconds, read
 * from the constant it is held to (`wallMs`, t-45961) rather than written
 * into five catalogs. Nothing for a row that names none. */
function jevHintVars(row) {
  const wall = row?.wallMs;
  return Number.isFinite(wall) ? { seconds: String(wall / JEV_MS_PER_SECOND) } : {};
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
      const one = byStart.get(day.startMs) ?? { startMs: day.startMs, tokens: 0, cost: 0, unmetered: 0 };
      one.tokens += tokens;
      one.unmetered += day.tally.unmeteredRequests ?? 0;
      if (one.unmetered > 0 || (rate === null && day.tally.rows > 0 && held.costUsd !== 0)) one.cost = null;
      else if (rate !== null && one.cost !== null) one.cost += tokens * rate;
      byStart.set(day.startMs, one);
    }
  }
  return [...byStart.values()].sort((a, b) => a.startMs - b.startMs);
}

function jevTotalCost(numbers) {
  if (!numbers.length) return null;
  let cost = 0;
  for (const held of numbers) {
    if (Number.isFinite(held.costUsd)) cost += held.costUsd;
    else if (held.week.rows > 0) return null;
  }
  return cost;
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
    const unmetered = days.some((day) => day.unmetered > 0);
    jevChartEmpty(card, counting, unmetered
      ? t("jev.chart.tokens.unmetered", "요청의 입력 토큰 사용량을 확인하지 못했습니다")
      : t("jev.chart.tokens.empty", "입력 토큰을 쓴 판단이 없습니다"));
    return;
  }
  const priced = days.every((day) => day.cost !== null);
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
    ? t("jev.chart.tokens.cost", "추정 비용 {{cost}}", { cost: jevCost(days.reduce((sum, day) => sum + day.cost, 0)) })
    : t("jev.chart.tokens.cost", "추정 비용 {{cost}}", { cost: "—" }));
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

/* How a judgment provider is said to a person (t-76598): where it is
 * reached, the model, and how it judges — never the provider's identity,
 * a hash nobody can read a connection or a model off. */
const JUDGMENT_WAYS = Object.freeze({
  system_one: { key: "jev.way.systemOne", word: "System One" },
  openai_json: { key: "jev.way.general", word: "일반 LLM" },
});

function judgmentWayWords(protocol) {
  const way = JUDGMENT_WAYS[protocol];
  return way ? t(way.key, way.word) : "";
}

/* 「연결 이름 · 모델 · 방식」 for the provider the settings name now, with
 * `model` in place of its own when an answer named another. */
function judgmentProviderWords(state = typesafeState, model = state?.provider?.model) {
  const provider = state?.provider;
  if (!provider) return "";
  const place = provider.kind === "jev" ? t("jev.place.jev", "Jev 서비스")
    : state.providerConnectionName || (provider.kind === "local" ? t("jev.place.local", "이 컴퓨터") : t("jev.place.remote", "다른 판단 서버"));
  return [place, model, judgmentWayWords(provider.protocol)].filter(Boolean).join(" · ");
}

/* An answered version as zo records it: `<identity>:<model>` for a provider
 * of the person's own (`judgment_provider::protocol`), the bare model for
 * Jev's. */
const JUDGMENT_VERSION = /^(judgment-[0-9a-f]+):(.+)$/;

/* What a version from the table is, for a person: `words` names the
 * provider judging now in full (the caption's), `short` only what tells it
 * from the one over the table (a row's, the drawer's, a cut's) — the model,
 * or 「이전 판단 모델」 for an identity the settings no longer name; `raw` is
 * what zo wrote, for the folded detail, when the words leave something out.
 * `current` is whether the settings name it now. */
function jevVersion(model) {
  const found = JUDGMENT_VERSION.exec(model ?? "");
  if (found) {
    const current = found[1] === typesafeState?.providerIdentity;
    const earlier = t("jev.model.earlier", "이전 판단 모델 · {{model}}", { model: found[2] });
    return {
      current,
      words: current ? judgmentProviderWords(typesafeState, found[2]) : earlier,
      short: current ? found[2] : earlier,
      raw: model,
    };
  }
  const current = typesafeState?.provider?.kind === "jev";
  return {
    current,
    words: current ? judgmentProviderWords(typesafeState, model) : model,
    short: model,
    raw: null,
  };
}

function paintJevCaption(view, numbers) {
  const caption = view.querySelector(".jev-caption");
  const model = jevHeadModel(numbers);
  const now = judgmentProviderWords();
  caption.hidden = !model && !now;
  if (caption.hidden) {
    caption.replaceChildren();
    return;
  }
  const version = model ? jevVersion(model) : null;
  const words = [];
  if (typesafeState?.model?.pinned && version) {
    words.push(t("settings.typesafe.seatVersionPinned", "고정 모델 {{model}}", { model: version.words }));
  } else if (!version || version.current) {
    words.push(t("jev.caption.now", "판단 모델: {{model}}", { model: version?.words ?? now }));
    const since = jevApplied?.[typesafeState?.providerIdentity];
    if (Number.isFinite(since)) words.push(t("jev.caption.since", "{{time}}에 바꿈 — 이 모델의 판단은 그때부터 셉니다", { time: jevSinceTime(since) }));
    if (!version) words.push(t("jev.caption.noAnswer", "아직 이 모델의 답이 없습니다"));
  } else {
    // Most features' newest answers name an earlier model. Whether this one
    // answered any is asked apart: once some features answer with it, the
    // table is a mix, and says so rather than 「아직 답이 없음」.
    const named = numbers.filter((held) => held.model);
    const answered = named.filter((held) => jevVersion(held.model).current).length;
    words.push(t("jev.caption.now", "판단 모델: {{model}}", { model: now }));
    if (answered) {
      const since = jevApplied?.[typesafeState?.providerIdentity];
      if (Number.isFinite(since)) words.push(t("jev.caption.since", "{{time}}에 바꿈 — 이 모델의 판단은 그때부터 셉니다", { time: jevSinceTime(since) }));
      words.push(t("jev.caption.mixedNumbers", "기능 {{answered}}개는 이 모델이 답했고, {{earlier}}개는 아직 {{model}}의 기록입니다",
        { answered: jevCount(answered), earlier: jevCount(named.length - answered), model: version.words }));
    } else {
      // Applied, and nothing answered since: the numbers are the earlier
      // model's, and the caption says both rather than the one zo last wrote.
      words.push(t("jev.caption.earlierNumbers", "아직 이 모델의 답이 없어 표의 숫자는 {{model}}의 것입니다", { model: version.words }));
    }
  }
  const said = jevNode("span", "jev-caption-words", document.createTextNode(words.join(" · ")));
  const parts = [said];
  const raw = version?.raw ?? null;
  if (raw) {
    const detail = jevNode("details", "jev-caption-detail");
    detail.append(jevNode("summary", "", document.createTextNode(t("jev.caption.detail", "자세한 값"))),
      jevNode("code", "", document.createTextNode(raw)));
    parts.push(detail);
  }
  const open = caption.querySelector(".jev-caption-detail")?.open ?? false;
  caption.replaceChildren(...parts);
  const detail = caption.querySelector(".jev-caption-detail");
  if (detail) detail.open = open;
}

/* A moment in the reader's own language and local time, to the minute. */
function jevSinceTime(ms) {
  try {
    return new Intl.DateTimeFormat(document.documentElement.lang || "ko", { dateStyle: "medium", timeStyle: "short" }).format(new Date(ms));
  } catch {
    return new Date(ms).toISOString();
  }
}

/* The rest zo's door holds a feature in (t-58193), while it lasts: `null`
 * once its end has passed, for a reading older than the moment drawn. */
function jevRestOf(held) {
  const rest = held?.resting;
  return rest && Number.isFinite(rest.untilMs) && rest.untilMs > Date.now() ? rest : null;
}

/* Why a feature rests and until when, in one sentence (t-58193): the end in
 * the person's local time by the window's own clock (`quotaWallClock`), the
 * cause in the language's words, and for a rest of its own how many of its
 * requests in a row went unanswered: the door's own number, which zo sends
 * (`unanswered`), never one spelled here. */
function jevRestWords(rest) {
  const time = quotaWallClock(rest.untilMs);
  const row = Object.hasOwn(JEV_REST_CAUSES, rest.cause) ? JEV_REST_CAUSES[rest.cause] : null;
  if (!row) return t("jev.rest.plain", "{{time}}까지 쉽니다", { time });
  const cause = t(row.key, row.word);
  return Number.isFinite(rest.unanswered)
    ? t("jev.rest.unanswered", "{{time}}까지 쉽니다 — 최근 요청 {{count}}건이 답을 받지 못했습니다 ({{cause}})",
      { time, count: jevCount(rest.unanswered), cause })
    : t("jev.rest.until", "{{time}}까지 쉽니다 — {{cause}}", { time, cause });
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
  // A feature the door rests says so beside its state (t-58193), in the
  // rest's own word: the chip and the sentence under it read one field.
  const rest = jevRestOf(held);
  if (rest) {
    const resting = jevNode("span", "jev-chip");
    resting.dataset.jevResting = rest.cause;
    resting.textContent = t(JEV_TOKENS.resting.key, JEV_TOKENS.resting.word);
    lead.append(resting);
  }
  if (counting) lead.append(jevNode("span", "jev-owed", document.createTextNode(jevOwedWords(owed, held))));
  const reason = across?.summed ? "" : jevStatusReason(held, status, choice, counting);
  if (reason) {
    const line = jevNode("p", "jev-status-reason", document.createTextNode(reason));
    // A feature measured under its bar says so in the tone of a miss.
    line.classList.toggle("is-under", status === "recording" && jevUnderBar(held));
    lead.append(line);
  }
  if (rest) {
    const line = jevNode("p", "jev-status-reason jev-rest", document.createTextNode(jevRestWords(rest)));
    line.dataset.jevRest = rest.cause;
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
  const version = held.model && held.model !== head ? jevVersion(held.model) : null;
  // An earlier provider's words already say it is a model.
  const other = !version ? "" : version.raw && !version.current ? version.short
    : t("settings.typesafe.seatVersion", "모델 {{model}}", { model: version.short });
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
  // The gate that holds the seat says why (t-26584); the line says it otherwise.
  const because = held.verdict ? jevLineWords(held.verdict.basis ?? held.verdict.line) : "";
  // A rise the evidence rules now hold says so beside the reason.
  const ruleHeld = held.verdict?.before === "rise" && held.verdict?.verdict !== "rise"
    ? t("jev.ruleHeld", "증거 규칙 전에는 승격이었지만 지금은 보류됩니다") : "";
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
  if (because) return counting ? "" : [ruleHeld, because].filter(Boolean).join(" · ");
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
  // The consent help stands beside the switch it is about; the budget's
  // beside the key card it belongs to.
  if (fix === "consent") {
    JUDGMENT_SWITCH_CARD.say(t("jev.help.consent", "동의한 폴더에서만 판단을 보냅니다. 「모든 폴더에서 사용」을 누르면 모든 폴더에서 보냅니다."));
  } else {
    paintTypeSafeStatus(t("jev.help.budget", "오늘 보낼 수 있는 판단 요청을 다 썼습니다. 하루 한도는 zo 설정 파일(settings.json)의 smart.jev.dailyRequests입니다."));
  }
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
  const builtIn = order.filter((id) => id !== jevCustomRules()?.feature && !jevBuiltinSaved(id).deleted);
  // A feature zo did not count stands with its dashes, never folded: only a
  // count of nothing says a feature was not used.
  const unused = builtIn.filter((id) => heldOf.get(id)?.week.rows === 0);
  const inUse = jevActivityOrder(builtIn.filter((id) => !unused.includes(id)), heldOf);
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
  paintJevCustom(view);
  paintJevBuiltinTrash(view);
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
  const open = builtIn.includes(jevDrawerSeat) ? jevDrawerSeat : null;
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
  if (drawer.hidden !== (id === null)) {
    drawer.hidden = id === null;
    sayDrawersDocked();
  }
  // Only the rows whose press opens or closes it change (t-9633 §3).
  for (const row of view.querySelectorAll("[data-jev-dash-row]")) {
    const open = row.dataset.jevDashRow === id;
    row.classList.toggle("is-open", open);
    const press = row.querySelector(".jev-row-open");
    if (press && press.getAttribute("aria-expanded") !== String(open)) press.setAttribute("aria-expanded", String(open));
  }
  if (id === null) return;
  jevReviewPart(drawer, id);
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
  drawer.querySelector(".jev-drawer-written").hidden = !standing?.written || Boolean(standing?.decision);
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

  // Who asked it this week, agent by agent — zo's own, every agent a pane
  // runs, and the ones whose work the window cannot see, with why (t-10916).
  const askers = held?.askers ?? [];
  part("askers").hidden = askers.length === 0;
  part("askers").querySelector("dl").replaceChildren(...askers.flatMap((one) => {
    const said = jevNode("dd", "", document.createTextNode(jevAskerWords(one)));
    said.dataset.jevAsker = one.agent;
    return [jevNode("dt", "", document.createTextNode(one.label)), said];
  }));

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
    const words = jevVersion(model).short;
    models.push({ key: "jev.drawer.model", word: "응답한 모델", said: typesafeState?.model?.pinned
      ? t("settings.typesafe.seatVersionPinned", "고정 모델 {{model}}", { model: words })
      : t("settings.typesafe.seatVersion", "모델 {{model}}", { model: words }) });
  }
  const cut = jevCutWords(held ?? {});
  if (cut) models.push({ key: "jev.drawer.cut", word: "버전 변경", said: cut });
  if (models.length === 0) {
    models.push({ key: "jev.drawer.model", word: "응답한 모델", said: t("jev.drawer.noModel", "아직 응답한 판단이 없습니다.") });
  }
  jevFacts(part("version").querySelector("dl"), models);

  part("sends").querySelector(".jev-drawer-text").textContent = feature.hint
    ? t(feature.hint.key, feature.hint.source, jevHintVars(standing))
    : "";
}

/* Evidence is loaded on an explicit press, never on the dashboard's poll.
 * A drawer generation owns its response and its buttons own the case they show. */
const jevReviewStates = new WeakMap();

function jevReviewPart(drawer, seat) {
  const scope = jevScopeChoice;
  const context = `${scope}:${jevScope?.workspace ?? ""}`;
  let state = jevReviewStates.get(drawer);
  if (!state) {
    const section = jevNode("section", "jev-drawer-part jev-review");
    const title = jevText("jev.review.title", "판단 검토", "h3", "jev-drawer-heading");
    const about = jevText("jev.review.about", "확신이 낮은 판단과 무작위 표본을 살펴보고 실제 결과를 남깁니다.", "p", "jev-drawer-text");
    const load = jevText("jev.review.load", "표본 불러오기", "button", "btn");
    load.type = "button";
    const status = jevNode("p", "jev-drawer-text");
    status.setAttribute("role", "status");
    const content = jevNode("div", "jev-review-cases");
    section.append(title, about, load, status, content);
    drawer.append(section);
    state = { seat, scope, context, generation: 0, seed: 0, load, status, content };
    jevReviewStates.set(drawer, state);
    load.addEventListener("click", () => { void jevLoadReviews(drawer); });
  }
  if (state.seat !== seat || state.context !== context) {
    state.seat = seat;
    state.scope = scope;
    state.context = context;
    state.generation += 1;
    state.load.disabled = false;
    state.status.textContent = "";
    state.content.replaceChildren();
  }
}

async function jevLoadReviews(drawer) {
  const state = jevReviewStates.get(drawer);
  if (!state || state.load.disabled) return;
  const generation = ++state.generation;
  const seat = state.seat;
  state.load.disabled = true;
  state.status.textContent = t("jev.review.loading", "검토 기록을 읽는 중…");
  try {
    const data = await invoke("jev_review", { seat, scope: state.scope, seed: state.seed++ });
    if (state.generation !== generation || state.seat !== seat) return;
    state.content.replaceChildren();
    const label = jevNode("label", "jev-review-toggle");
    const checkbox = document.createElement("input");
    checkbox.type = "checkbox";
    checkbox.checked = Boolean(data.enabled);
    label.append(checkbox, jevText("jev.review.capture", "검토용 입력·응답 저장"));
    checkbox.addEventListener("change", async () => {
      const wanted = checkbox.checked;
      checkbox.disabled = true;
      try {
        await invoke("set_jev_review_enabled", { on: wanted });
        if (state.generation === generation) state.status.textContent = t("jev.review.preferenceSaved", "검토 기록 저장 설정을 반영했습니다.");
      } catch (error) {
        checkbox.checked = !wanted;
        if (state.generation === generation) state.status.textContent = String(error);
      } finally { checkbox.disabled = false; }
    });
    state.content.append(label);
    const samples = Array.isArray(data.samples) ? data.samples : [];
    state.status.textContent = samples.length
      ? t("jev.review.count", "검토할 표본 {{count}}건", { count: samples.length })
      : t("jev.review.empty", "검토할 기록이 없습니다.");
    if (data.total >= data.capacity) {
      state.content.append(jevText("jev.review.full", "검토 기록이 저장 한도에 도달했습니다. 기록을 내보내고 보관 상태를 확인하세요.", "p", "jev-drawer-text"));
    }
    if (data.invalid > 0) {
      const warning = jevNode("p", "jev-drawer-text");
      warning.textContent = t("jev.review.invalid", "읽을 수 없는 기록 {{count}}건은 제외했습니다.", { count: data.invalid });
      state.content.append(warning);
    }
    for (const sample of samples) jevRenderReview(state.content, sample);
  } catch (error) {
    if (state.generation === generation) state.status.textContent = String(error);
  } finally {
    if (state.generation === generation) state.load.disabled = false;
  }
}

function jevRenderReview(host, sample) {
  const item = sample.case;
  if (!item || typeof item.id !== "string") return;
  const card = jevNode("article", "jev-review-case");
  const reason = sample.reason === "audit"
    ? t("jev.review.audit", "무작위 점검") : t("jev.review.uncertain", "불확실한 판단");
  const heading = jevNode("p", "jev-drawer-text");
  heading.textContent = `${reason} · ${item.model} · v${item.rubricVersion}`;
  const details = document.createElement("details");
  const summary = jevText("jev.review.evidence", "판단 당시 입력과 응답", "summary");
  const evidence = document.createElement("pre");
  evidence.tabIndex = 0;
  evidence.dataset.keyboardOwner = "jev-review";
  evidence.setAttribute("role", "region");
  evidence.setAttribute("aria-label", summary.textContent);
  evidence.textContent = JSON.stringify({ workspace: item.workspace, request: item.request, answers: item.answers }, null, 2);
  details.append(summary, evidence);
  card.append(heading, details);
  for (const question of sample.remaining ?? []) {
    const form = document.createElement("form");
    const label = jevNode("label", "jev-review-note");
    const name = jevNode("span");
    name.textContent = question;
    const note = document.createElement("textarea");
    note.maxLength = 1000;
    note.placeholder = t("jev.review.note", "실제 결과나 확인 근거");
    note.setAttribute("aria-label", `${question} · ${note.placeholder}`);
    label.append(name, note);
    const status = jevNode("p", "jev-drawer-text");
    status.setAttribute("role", "status");
    const buttons = [
      [true, jevText("jev.review.correct", "맞음", "button", "btn")],
      [false, jevText("jev.review.incorrect", "틀림", "button", "btn")],
    ].map(([correct, button]) => {
      button.type = "button";
      button.disabled = true;
      button.addEventListener("click", async () => {
        if (!note.value.trim() || form.dataset.saving) return;
        form.dataset.saving = "true";
        note.disabled = true;
        buttons.forEach((held) => { held.disabled = true; });
        try {
          await invoke("jev_review_outcome", { caseId: item.id, question, correct, note: note.value.trim() });
          status.textContent = t("jev.review.saved", "검토 결과를 저장했습니다.");
          form.dataset.saved = "true";
        } catch (error) {
          status.textContent = String(error);
          note.disabled = false;
          buttons.forEach((held) => { held.disabled = !note.value.trim(); });
        } finally { delete form.dataset.saving; }
      });
      return button;
    });
    note.addEventListener("input", () => {
      if (!form.dataset.saved && !form.dataset.saving) buttons.forEach((button) => { button.disabled = !note.value.trim(); });
    });
    form.addEventListener("submit", (event) => { event.preventDefault(); });
    form.append(label, jevNode("div", "jev-actions", ...buttons), status);
    card.append(form);
  }
  host.append(card);
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
    const actions = jevNode("span", "jev-builtin-actions");
    for (const [action, icon] of [["edit", "i-pencil"], ["delete", "i-trash"]]) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "jev-builtin-action";
      button.dataset[`jevBuiltin${action === "edit" ? "Edit" : "Delete"}`] = id;
      const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
      svg.setAttribute("class", "icon"); svg.setAttribute("aria-hidden", "true");
      const use = document.createElementNS("http://www.w3.org/2000/svg", "use");
      use.setAttribute("href", `#${icon}`); svg.append(use); button.append(svg); actions.append(button);
    }
    seat.replaceChildren(jevNode("span", "jev-builtin-heading", open, actions),
      jevNode("span", "jev-row-meta", jevNode("span", "jev-row-id")));
  }
  open.textContent = jevSeatName(id);
  for (const [selector, label] of [
    ["[data-jev-builtin-edit]", t("jev.builtin.editFor", "「{{name}}」 수정", { name: jevSeatName(id) })],
    ["[data-jev-builtin-delete]", t("jev.builtin.deleteFor", "「{{name}}」 삭제", { name: jevSeatName(id) })],
  ]) {
    const button = seat.querySelector(selector);
    button.setAttribute("aria-label", label); button.dataset.tip = label;
  }
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
  const askers = jevAskersLine(held);
  if (askers) rows.append(askers);
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
  said("cost", jevCost(jevTotalCost(counted)));
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

/* ---- A person's own decisions (t-62170 stage 3, DESIGN 3) ----
 *
 * The part between the charts and the built-in table that lists the
 * decisions a person wrote, the drawer they are written in, and the question
 * before one is deleted. Every bound and mode is the settings answer's
 * (`customDecisions`, read from core); the words for a point, an input, a
 * kind of answer, a field and a refusal are the markup's under core's own
 * word (`#jev-custom-words`); and what may be kept is core's to say — the
 * drawer asks the check, and the store checks again under its lock. With no
 * decision the part does not stand and 「새 결정」 waits on the switch line,
 * so the first screen keeps its rows (`FIRST_SCREEN_ROWS`). */

/* How long the undo after a delete stands (DESIGN 3): longer than a plain
 * toast (`TOAST_MS`), so it is held and taken away here. */
const JEV_CUSTOM_UNDO_MS = 8_000;

/* The pause after the last change in the drawer before core's check is
 * asked: one ask per pause in typing, not one per key. */
const JEV_CUSTOM_CHECK_MS = 250;

/* What each mode a decision offers is called, by what it does: the settings
 * answer names a mode with its flags (`ModeChoice`), and its word is never
 * matched here. */
const JEV_CUSTOM_MODE_WORDS = Object.freeze([
  Object.freeze({ asks: false, applies: false, key: "jev.custom.mode.off", word: "끔" }),
  Object.freeze({ asks: true, applies: false, key: "jev.custom.mode.shadow", word: "기록만" }),
  Object.freeze({ asks: true, applies: true, key: "jev.custom.mode.on", word: "켜짐" }),
]);

/* The store as `list_custom_decisions` last answered — `{ decisions, trash,
 * folders, everywhere }` — or `null` before it has. */
let jevCustom = null;
let jevCustomAsking = null;
/* The words of the last failure to read the store, said once: the poll asks
 * again, and the same failure is not said again each time. */
let jevCustomListFailed = "";
/* The drawer while it stands: the decision it edits (`null` for a new one),
 * the draft as it opened, the check owed and the last one answered, and
 * whether a close already found words unsaved. */
let jevCustomDrawer = null;
/* The decision the delete question is about. */
let jevCustomDeleting = null;
const jevCustomWordsHeld = new Map();

function jevCustomRules() {
  return typesafeState?.customDecisions ?? null;
}

/* The markup's node for core's `word` of `kind` — a point, what a point does
 * when on, an input, a kind of answer, a field or a refusal — or `null` for
 * one it does not word. */
function jevCustomWordNode(kind, word) {
  const at = `${kind}:${word}`;
  if (!jevCustomWordsHeld.has(at)) {
    jevCustomWordsHeld.set(at, el("jev-custom-words")?.content.querySelector(`[data-jev-${kind}="${word}"]`) ?? null);
  }
  return jevCustomWordsHeld.get(at);
}

/* Core's `word` of `kind` in the language in force, the markup's Korean when
 * no catalog says it, and the word itself when the markup does not either. */
function jevCustomSay(kind, word, vars) {
  const node = jevCustomWordNode(kind, word);
  return node ? t(node.dataset.i18n, node.textContent.replace(/\s+/g, " ").trim(), vars) : String(word);
}

function jevCustomModeWords(choice) {
  const words = JEV_CUSTOM_MODE_WORDS.find((one) => one.asks === choice.asks && one.applies === choice.applies);
  return words ? t(words.key, words.word) : choice.mode;
}

/* The bounds a sentence may name, each the settings answer's. */
function jevCustomBounds(rules) {
  return {
    nameMax: jevCount(rules.nameMaxChars), questionMax: jevCount(rules.questionMaxChars),
    meaningMax: jevCount(rules.meaningMaxChars), optionsLeast: jevCount(rules.options.least),
    optionsMost: jevCount(rules.options.most), hourlyLeast: jevCount(rules.hourly.least),
    hourlyMost: jevCount(rules.hourly.most), hourlyAll: jevCount(rules.hourlyAll),
    actLeast: jevPercent(rules.actFrom.least), actMost: jevPercent(rules.actFrom.most),
    decisionsMax: jevCount(rules.decisionsMax), days: jevCount(rules.trashKeptDays),
  };
}

/* What a refusal says: the markup's sentence for core's word, with the field
 * it names, the line, the option or level, and the bounds. */
function jevCustomRefusalWords(refusal, rules) {
  const level = refusal.level === null || refusal.level === undefined ? "" : jevCount(refusal.level + 1);
  const named = { option: refusal.option ?? "", level };
  return jevCustomSay("refusal", refusal.word, {
    ...jevCustomBounds(rules), ...named, line: jevCount(refusal.line),
    field: refusal.field ? jevCustomSay("field", refusal.field, named) : "",
  });
}

/* What a store's refusal or failure says: core's refusals by their words,
 * a decision the store no longer keeps, else the failure as it came back. */
function jevCustomFailure(failure) {
  const rules = jevCustomRules();
  if (rules && Array.isArray(failure?.refusals) && failure.refusals.length > 0) {
    return failure.refusals.map((refusal) => jevCustomRefusalWords(refusal, rules)).join(" ");
  }
  if (failure?.missing) return t("jev.custom.missing", "그 결정을 찾지 못했습니다. 목록을 다시 읽습니다.");
  return t("jev.custom.failed", "내가 만든 결정을 읽거나 쓰지 못했습니다 — {{error}}",
    { error: failure?.message ?? String(failure ?? "") });
}

/* Ask the store what it keeps, one ask at a time, and paint the answer. A
 * window whose settings name no editor (`customDecisions`) asks nothing. */
function loadJevCustom() {
  if (!jevCustomRules()) return Promise.resolve();
  jevCustomAsking ??= invoke("list_custom_decisions").then((store) => {
    jevCustom = store;
    jevCustomListFailed = "";
  }, (failure) => {
    const words = jevCustomFailure(failure);
    if (words !== jevCustomListFailed) toast(words, "halt");
    jevCustomListFailed = words;
  }).finally(() => {
    jevCustomAsking = null;
    paintJevViews();
  });
  return jevCustomAsking;
}

/* Hand a change to the store (`asking`, the door's own ask) and keep the
 * list it answers with; a refusal or a failure is said and the list in hand
 * stands — asked again when the store no longer keeps the decision. Whether
 * it took. */
async function jevCustomChange(asking) {
  try {
    jevCustom = await asking();
    return true;
  } catch (failure) {
    toast(jevCustomFailure(failure), "halt");
    if (failure?.missing) void loadJevCustom();
    return false;
  } finally {
    paintJevViews();
  }
}

/* 「새 결정」 on the switch line: all a person with no decisions sees of
 * them, on a line that already stands (DESIGN 3). */
function jevCustomGuide() {
  const guide = jevText("jev.custom.new", "＋ 새 결정", "button", "btn jev-custom-guide");
  guide.type = "button";
  guide.hidden = true;
  guide.dataset.jevCustomGuide = "";
  guide.dataset.keyboardOwner = "jev-custom";
  return jevHint("jev.custom.guideTip", "질문 하나를 직접 적어 Jev에 묻습니다. 새 결정은 기록만으로 시작합니다.", guide);
}

/* The part a person's decisions stand in: its head with the count and
 * 「새 결정」, the list, and the fold of deleted ones (`paintJevCustom`). */
function jevCustomPart() {
  const add = jevText("jev.custom.new", "＋ 새 결정", "button", "btn jev-custom-new");
  add.type = "button";
  add.dataset.jevCustomNew = "";
  const count = jevNode("p", "jev-custom-count");
  count.dataset.jevCustomCount = "";
  const list = jevNode("ul", "jev-custom-list");
  list.dataset.jevCustomList = "";
  const trash = jevNode("details", "jev-custom-trash", jevNode("summary"), jevNode("ul", "jev-custom-trash-list"));
  trash.dataset.jevCustomTrash = "";
  const part = jevNode("section", "jev-custom",
    jevNode("header", "jev-custom-head",
      jevNode("div", "jev-custom-heading", jevText("settings.typesafe.custom", "내가 만든 결정", "h2", "jev-custom-title"), count), add),
    list, trash);
  part.dataset.jevCustom = "";
  part.dataset.keyboardOwner = "jev-custom";
  part.dataset.i18nAria = "settings.typesafe.custom";
  part.setAttribute("aria-label", t("settings.typesafe.custom", "내가 만든 결정"));
  part.hidden = true;
  return part;
}

/* The part's hands on one view: 「새 결정」, a row's mode, 「편집」 and
 * 「삭제」, and 「되살리기」 — and the arrow keys that move a mode. */
function wireJevCustom(host) {
  host.querySelector("[data-jev-custom-guide]")?.addEventListener("click", (event) => {
    openJevCustomDrawer(null, event.currentTarget);
  });
  const part = host.querySelector("[data-jev-custom]");
  part.addEventListener("click", (event) => {
    const button = event.target.closest("button");
    if (!button || button.disabled) return;
    if (button.matches("[data-jev-custom-new]")) {
      openJevCustomDrawer(null, button);
      return;
    }
    if (button.dataset.jevCustomRestore) {
      const id = button.dataset.jevCustomRestore;
      void jevCustomChange(() => invoke("restore_custom_decision", { id }));
      return;
    }
    const id = button.closest("[data-jev-custom-row]")?.dataset.jevCustomRow;
    const kept = jevCustom?.decisions.find((one) => one.draft.id === id);
    if (!kept) return;
    if (button.matches("[data-jev-custom-edit]")) openJevCustomDrawer(kept, button);
    else if (button.matches("[data-jev-custom-delete]")) openJevCustomDelete(kept, button);
    else if (button.getAttribute("role") === "radio") void setJevCustomMode(kept, button.dataset.mode);
  });
  part.addEventListener("keydown", (event) => {
    const radio = event.target.closest("[data-jev-custom-mode] [role='radio']");
    const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[event.key];
    if (!radio || !step) return;
    event.preventDefault();
    const radios = [...radio.parentElement.querySelectorAll("[role='radio']")];
    const next = radios[(radios.indexOf(radio) + step + radios.length) % radios.length];
    next.focus();
    const kept = jevCustom?.decisions.find((one) => one.draft.id === radio.closest("[data-jev-custom-row]").dataset.jevCustomRow);
    if (kept) void setJevCustomMode(kept, next.dataset.mode);
  });
}

/* The part and the switch line's 「새 결정」 from the store in hand: the part
 * stands while the store keeps a decision, or a deleted one still to be
 * restored; with neither only the switch line's 「새 결정」 does. */
function paintJevCustom(view) {
  const rules = jevCustomRules();
  const store = rules ? jevCustom : null;
  const decisions = store?.decisions ?? [];
  const trash = store?.trash ?? [];
  const standing = decisions.length > 0 || trash.length > 0;
  const guide = view.querySelector("[data-jev-custom-guide]");
  if (guide) guide.hidden = !store || standing;
  const part = view.querySelector("[data-jev-custom]");
  part.hidden = !store || !standing;
  if (part.hidden || !jevMoved(part, JSON.stringify(store), JSON.stringify(rules), jevLanguage())) return;
  const bounds = jevCustomBounds(rules);
  jevSay(part.querySelector("[data-jev-custom-count]"), t("jev.custom.count",
    "{{count}}/{{decisionsMax}}개 · 모두 합쳐 한 시간에 {{hourlyAll}}회까지 묻습니다", { ...bounds, count: jevCount(decisions.length) }));
  const add = part.querySelector("[data-jev-custom-new]");
  add.disabled = decisions.length >= rules.decisionsMax;
  if (add.disabled) add.dataset.tip = t("jev.custom.full", "내가 만든 결정은 {{decisionsMax}}개까지입니다. 하나를 지우면 새로 만들 수 있습니다.", bounds);
  else delete add.dataset.tip;
  const list = part.querySelector("[data-jev-custom-list]");
  const rows = decisions.map((kept) => jevCustomRow(list, kept, rules));
  for (const row of list.querySelectorAll("[data-jev-custom-row]")) {
    if (!rows.includes(row)) row.remove();
  }
  if (!list.contains(document.activeElement)) {
    rows.forEach((row, at) => {
      if (list.children[at] !== row) list.insertBefore(row, list.children[at] ?? null);
    });
  }
  const fold = part.querySelector("[data-jev-custom-trash]");
  fold.hidden = trash.length === 0;
  jevSay(fold.querySelector("summary"), t("jev.custom.trash", "지운 결정 {{count}}개 · {{days}}일 동안 되살릴 수 있습니다",
    { ...bounds, count: jevCount(trash.length) }));
  fold.querySelector("ul").replaceChildren(...trash.map((one) => {
    const restore = jevText("jev.custom.restore", "되살리기", "button", "btn jev-custom-restore");
    restore.type = "button";
    restore.dataset.jevCustomRestore = one.id;
    return jevNode("li", "jev-custom-trashed", jevNode("span", "jev-custom-name", document.createTextNode(one.name)), restore);
  }));
}

/* One decision's line, built once and kept: its name, what it is and when
 * it asks, its mode as radios — never a select — and 「편집」 and 「삭제」. */
function jevCustomRow(list, kept, rules) {
  const id = kept.draft.id;
  let row = [...list.children].find((one) => one.dataset.jevCustomRow === id);
  if (!row) {
    const modes = jevNode("div", "jev-custom-mode", ...rules.modes.map((choice) => {
      const radio = jevNode("button", "jev-custom-mode-choice");
      radio.type = "button";
      radio.setAttribute("role", "radio");
      radio.dataset.mode = choice.mode;
      return radio;
    }));
    modes.setAttribute("role", "radiogroup");
    modes.dataset.jevCustomMode = "";
    const edit = jevText("jev.custom.edit", "편집", "button", "btn jev-custom-edit");
    edit.type = "button";
    edit.dataset.jevCustomEdit = "";
    const remove = jevText("app.delete", "삭제", "button", "btn btn--halt-quiet jev-custom-delete");
    remove.type = "button";
    remove.dataset.jevCustomDelete = "";
    row = jevNode("li", "jev-custom-row",
      jevNode("div", "jev-custom-about", jevNode("strong", "jev-custom-name"), jevNode("p", "jev-custom-facts")),
      modes, jevNode("div", "jev-custom-actions", edit, remove));
    row.dataset.jevCustomRow = id;
    list.append(row);
  }
  if (!jevMoved(row, JSON.stringify(kept), jevLanguage())) return row;
  jevSay(row.querySelector(".jev-custom-name"), kept.draft.name);
  row.querySelector(".jev-custom-facts").replaceChildren(...jevCustomFacts(kept.draft));
  const group = row.querySelector("[data-jev-custom-mode]");
  group.setAttribute("aria-label", t("jev.custom.modeOf", "{{name}} 모드", { name: kept.draft.name }));
  const radios = [...group.querySelectorAll("[role='radio']")];
  for (const radio of radios) {
    const choice = rules.modes.find((one) => one.mode === radio.dataset.mode);
    jevSay(radio, choice ? jevCustomModeWords(choice) : radio.dataset.mode);
    const on = radio.dataset.mode === kept.mode;
    radio.setAttribute("aria-checked", String(on));
    radio.tabIndex = on ? 0 : -1;
  }
  if (!radios.some((radio) => radio.tabIndex === 0) && radios.length > 0) radios[0].tabIndex = 0;
  return row;
}

/* What a decision is, fact by fact: its kind of answer, when it asks and
 * what it does there when on, the confidence it acts from — with the whole
 * rule as its tip — its hourly bound, and where it asks. */
function jevCustomFacts(draft) {
  const fact = (text) => jevNode("span", "jev-custom-fact", document.createTextNode(text));
  const share = { share: jevPercent(draft.actFrom) };
  const acting = fact(t("jev.custom.actsFrom", "확신 {{share}} 이상", share));
  acting.dataset.tip = t("jev.custom.actsFromTip",
    "확신이 {{share}} 이상인 답만 넘깁니다. 동률인 답은 기준과 상관없이 넘기지 않습니다. 동률은 가장 높은 확률을 보기 둘 이상이 나눠 가진 답입니다.", share);
  return [
    fact(jevCustomSay("kind", draft.answer.kind)),
    fact(`${jevCustomSay("point", draft.at)} — ${jevCustomSay("point-act", draft.at)}`),
    acting,
    fact(t("jev.custom.hourly", "한 시간에 {{count}}회까지", { count: jevCount(draft.maxPerHour) })),
    fact(draft.folders === null ? t("jev.custom.everyFolder", "동의한 모든 폴더")
      : t("jev.custom.someFolders", "고른 폴더 {{count}}곳", { count: jevCount(draft.folders.length) })),
  ];
}

/* Move `kept` to `mode` through the store; the radios stand where the
 * store's answer puts them. */
function setJevCustomMode(kept, mode) {
  if (!mode || kept.mode === mode) return Promise.resolve(false);
  return jevCustomChange(() => invoke("set_custom_decision_mode", { id: kept.draft.id, mode }));
}

/* A new decision's draft: the first kind, point and its every input, every
 * consented folder, and the settings answer's defaults. */
function jevCustomBlank(rules) {
  const point = rules.points[0];
  return {
    id: null, name: "", question: "", answer: jevCustomBlankAnswer(rules.kinds[0], rules), at: point.at,
    reads: [...point.inputs], folders: null, actFrom: rules.actFromDefault, maxPerHour: rules.hourlyDefault,
  };
}

/* An empty answer of `kind` in the window's draft (`AnswerDraft`): a yes
 * and a no, or as many options or levels as a decision needs at least. */
function jevCustomBlankAnswer(kind, rules) {
  const empty = Array.from({ length: rules.options.least }, () => "");
  if (kind === "choice") return { kind, options: empty.map(() => ({ name: "", means: "" })) };
  if (kind === "score") return { kind, levels: empty };
  return { kind, yes: "", no: "" };
}

/* The draft the drawer's fields hold now, in the window's shape
 * (`DecisionDraft`): the point's inputs in the point's own order, and no
 * folders for every consented one. */
function jevCustomDraftOf(form) {
  const rules = jevCustomRules();
  const value = (name) => form.querySelector(`[name="${name}"]`)?.value ?? "";
  const chosen = (name) => form.querySelector(`[name="${name}"]:checked`)?.value ?? null;
  const kind = chosen("kind") ?? rules.kinds[0];
  const point = rules.points.find((one) => one.at === chosen("at")) ?? rules.points[0];
  const ticked = new Set([...form.querySelectorAll("[name='reads']:checked")].map((one) => one.value));
  const rows = [...form.querySelectorAll("[data-jev-custom-option]")];
  const answer = kind === "choice"
    ? { kind, options: rows.map((row) => ({ name: row.querySelector("[name='option_name']").value, means: row.querySelector("[name='meaning']").value })) }
    : kind === "score" ? { kind, levels: rows.map((row) => row.querySelector("[name='level']").value) }
      : { kind, yes: value("yes"), no: value("no") };
  return {
    id: jevCustomDrawer?.kept?.draft.id ?? null, name: value("name"), question: value("question"), answer, at: point.at,
    reads: point.inputs.filter((input) => ticked.has(input)),
    folders: chosen("folders") === "chosen" ? [...form.querySelectorAll("[name='folder']:checked")].map((one) => one.value) : null,
    actFrom: Number(value("actFrom")), maxPerHour: Number(value("maxPerHour")),
  };
}

/* One group of the drawer — its words, a line about it, and its controls —
 * under the name a refusal finds it by. */
function jevCustomGroup(part, legend, about, ...controls) {
  const group = jevNode("fieldset", "jev-custom-field", legend, ...(about ? [about] : []), ...controls);
  group.dataset.jevCustomField = part;
  return group;
}

/* A sentence that names a bound, said again in the language in force. */
function jevCustomAbout(key, word) {
  const about = jevNode("p", "jev-custom-about-line");
  say(about, () => t(key, word, jevCustomBounds(jevCustomRules())));
  return about;
}

/* A radio or a checkbox with its words beside it, and a line under them. */
function jevCustomTick(type, name, value, checked, words, under = null) {
  const box = document.createElement("input");
  box.type = type;
  box.name = name;
  box.value = value;
  box.checked = checked;
  return jevNode("label", "jev-custom-choice", box, jevNode("span", "jev-custom-choice-words", words, ...(under ? [under] : [])));
}

/* The judgment model that answers every decision, as the settings card
 * names it: a decision has no model of its own. */
function jevCustomModelLine() {
  const line = jevNode("p", "jev-custom-about-line");
  say(line, () => {
    const provider = typesafeState?.provider;
    return provider
      ? t("jev.custom.model", "{{provider}} · {{model}} — 설정의 「모델 사용」을 따릅니다.",
        { provider: judgmentProviderController.providerName(provider), model: provider.model })
      : t("jev.custom.modelAbout", "설정에서 고른 판단 모델이 답합니다.");
  });
  return line;
}

/* The drawer's fields for `draft`, built afresh each time it opens: the
 * kinds, points and inputs are the settings answer's of the moment. */
function jevCustomFormOf(form, draft, rules) {
  const notice = jevText("jev.custom.version", "저장하면 새 판으로 기록이 나뉩니다. 지금까지의 기록은 이전 판으로 따로 남습니다.", "p", "jev-custom-version");
  notice.dataset.jevCustomVersion = "";
  notice.hidden = true;
  const refusals = jevNode("ul", "jev-custom-refusals");
  refusals.dataset.jevCustomRefusals = "";
  refusals.setAttribute("role", "alert");
  refusals.hidden = true;
  const text = (tag, name, value) => {
    const node = document.createElement(tag);
    node.name = name;
    node.value = value;
    node.className = tag === "textarea" ? "jev-custom-area" : "jev-custom-input";
    if (tag === "input") node.type = "text";
    node.spellcheck = false;
    return node;
  };
  const name = text("input", "name", draft.name);
  name.id = "jev-custom-name";
  const question = text("textarea", "question", draft.question);
  question.rows = 4;
  const kinds = jevCustomGroup("kind", jevText("jev.custom.kindLegend", "답의 종류", "legend"), null,
    ...rules.kinds.map((kind) => jevCustomTick("radio", "kind", kind, kind === draft.answer.kind, jevQuestionType(kind))));
  const answer = jevCustomGroup("answer", jevText("jev.custom.answerLegend", "보기", "legend"), null);
  const points = jevCustomGroup("at", jevText("jev.custom.atLegend", "언제 묻나", "legend"), null,
    ...rules.points.map((point) => jevCustomTick("radio", "at", point.at, point.at === draft.at, jevCustomSay("point", point.at),
      jevNode("small", "", document.createTextNode(jevCustomSay("point-about", point.at))))));
  const acts = jevNode("p", "jev-custom-act");
  acts.dataset.jevCustomAct = "";
  const reads = jevCustomGroup("reads", jevText("jev.custom.readsLegend", "읽는 입력", "legend"),
    jevText("jev.custom.readsAbout", "질문이 읽을 자료입니다. 보내기 전에 정해진 길이로 자르고, 비밀처럼 보이는 줄은 지웁니다.", "p", "jev-custom-about-line"));
  const actFrom = document.createElement("input");
  actFrom.type = "range";
  actFrom.name = "actFrom";
  actFrom.step = "any";
  actFrom.min = String(rules.actFrom.least);
  actFrom.max = String(rules.actFrom.most);
  actFrom.value = String(draft.actFrom);
  const shown = jevNode("output", "jev-custom-share");
  const hourly = document.createElement("input");
  hourly.type = "number";
  hourly.name = "maxPerHour";
  hourly.className = "jev-custom-input jev-custom-number";
  hourly.step = "1";
  hourly.min = String(rules.hourly.least);
  hourly.max = String(rules.hourly.most);
  hourly.value = String(draft.maxPerHour);
  const candidates = [...new Set([...(jevCustom?.folders ?? []), ...(draft.folders ?? [])])];
  const picks = jevNode("div", "jev-custom-folders", ...candidates.map((folder) =>
    jevCustomTick("checkbox", "folder", folder, draft.folders?.includes(folder) ?? false, folder)));
  const chosen = jevCustomTick("radio", "folders", "chosen", draft.folders !== null,
    jevText("jev.custom.chosenFolders", "고른 폴더만"), jevText("jev.custom.chosenFoldersAbout", "동의한 폴더 가운데서 고릅니다.", "small"));
  chosen.querySelector("input").disabled = candidates.length === 0;
  const line = jevNode("code", "jev-custom-line");
  line.dataset.jevCustomLine = "";
  const questions = jevNode("pre", "jev-custom-questions");
  questions.dataset.jevCustomQuestions = "";
  form.replaceChildren(notice, refusals,
    jevCustomGroup("name", jevText("jev.custom.nameLegend", "이름", "legend"),
      jevCustomAbout("jev.custom.nameAbout", "화면과 기록에 보이는 이름입니다. {{nameMax}}자까지 적습니다."), name),
    kinds,
    jevCustomGroup("question", jevText("jev.custom.questionLegend", "질문", "legend"),
      jevCustomAbout("jev.custom.questionAbout", "Jev에 묻는 글입니다. {{questionMax}}자까지 적습니다. 비밀번호나 키는 적지 마세요."), question),
    answer, points, acts, reads,
    jevCustomGroup("actFrom", jevText("jev.custom.actFromLegend", "확신도 기준", "legend"),
      jevCustomAbout("jev.custom.actFromAbout", "확신이 이보다 낮은 답은 기록만 하고 넘기지 않습니다. 동률인 답도 기준과 상관없이 넘기지 않습니다. 동률은 가장 높은 확률을 보기 둘 이상이 나눠 가진 답입니다. {{actLeast}}~{{actMost}} 사이에서 고릅니다."),
      jevNode("div", "jev-custom-range", actFrom, shown)),
    jevCustomGroup("folders", jevText("jev.custom.foldersLegend", "묻는 폴더", "legend"), null,
      jevCustomTick("radio", "folders", "consented", draft.folders === null, jevText("jev.custom.everyFolder", "동의한 모든 폴더"),
        jevText("jev.custom.everyFolderAbout", "설정의 폴더 동의를 그대로 따릅니다.", "small")),
      chosen, picks),
    jevCustomGroup("maxPerHour", jevText("jev.custom.hourlyLegend", "한 시간 상한", "legend"),
      jevCustomAbout("jev.custom.hourlyAbout", "이 결정이 한 시간에 보내는 요청 수입니다. {{hourlyLeast}}~{{hourlyMost}}회까지 정하고, 모든 결정을 합쳐 한 시간에 {{hourlyAll}}회, 하루 한도는 그 위에 그대로 걸립니다."),
      hourly),
    jevCustomGroup("model", jevText("jev.custom.modelLegend", "답하는 모델", "legend"), jevCustomModelLine()),
    jevCustomGroup("line", jevText("jev.custom.lineLegend", "에이전트가 읽는 줄", "legend"),
      jevText("jev.custom.lineAbout", "켠 결정의 답이 기준을 넘으면 에이전트가 이 줄을 읽습니다.", "p", "jev-custom-about-line"), line),
    jevNode("details", "jev-custom-preview", jevText("jev.custom.preview", "보내는 내용 미리보기", "summary"), questions));
  jevCustomAnswerFields(form, draft.answer, rules);
  const preview = jevInputPreview(form, () => {
    const draft = jevCustomDraftOf(form);
    const answer = draft.answer;
    const criteria = answer.kind === "choice" ? answer.options.map(option => [option.name, option.means])
      : answer.kind === "score" ? answer.levels.map((level, index) => [String(index), level])
        : [[t("jev.custom.yesMeans", "「예」의 뜻"), answer.yes], [t("jev.custom.noMeans", "「아니오」의 뜻"), answer.no]];
    return [{ type: answer.kind, instructions: draft.question, criteria }];
  });
  answer.after(preview);
  jevCustomPointFields(form, draft.at, draft.reads, rules);
  jevCustomFieldsShown(form);
}

/* The answer's fields for `answer`: a meaning for a yes and for a no, or a
 * list of options each named with its meaning, or levels lowest first — with
 * a press to add one up to the most and to take one away down to the least. */
function jevCustomAnswerFields(form, answer, rules) {
  const holder = form.querySelector("[data-jev-custom-field='answer']");
  const legend = holder.querySelector("legend");
  const area = (name, value) => {
    const node = document.createElement("textarea");
    node.className = "jev-custom-area";
    node.name = name;
    node.rows = 2;
    node.value = value;
    return node;
  };
  if (answer.kind !== "choice" && answer.kind !== "score") {
    holder.replaceChildren(legend,
      jevCustomAbout("jev.custom.yesNoAbout", "예와 아니오가 각각 무엇을 뜻하는지 적습니다. 각 {{meaningMax}}자까지입니다."),
      jevNode("label", "jev-custom-label", jevText("jev.custom.yesMeans", "「예」의 뜻"), area("yes", answer.yes)),
      jevNode("label", "jev-custom-label", jevText("jev.custom.noMeans", "「아니오」의 뜻"), area("no", answer.no)));
    jevCustomExamples(form, answer.kind);
    return;
  }
  const named = answer.kind === "choice";
  const rows = (named ? answer.options : answer.levels.map((means) => ({ means }))).map((option) => {
    const remove = jevText("jev.custom.removeOption", "빼기", "button", "btn jev-custom-remove");
    remove.type = "button";
    remove.dataset.jevCustomRemove = "";
    const label = jevNode("span", "jev-custom-option-label");
    const controls = named
      ? [jevNode("label", "jev-custom-label", label, (() => {
        const input = document.createElement("input");
        input.type = "text";
        input.className = "jev-custom-input";
        input.name = "option_name";
        input.value = option.name;
        return input;
      })()), jevNode("label", "jev-custom-label", jevText("jev.custom.optionMeans", "뜻"), area("meaning", option.means))]
      : [jevNode("label", "jev-custom-label", label, area("level", option.means))];
    const move = (direction, button) => {
      button.type = "button";
      button.dataset.jevCustomMove = direction; return button;
    };
    const actions = jevNode("div", "jev-custom-actions",
      ...(!named ? [
        move("up", jevText("jev.question.moveUp", "위로", "button", "btn")),
        move("down", jevText("jev.question.moveDown", "아래로", "button", "btn")),
      ] : []), remove);
    const row = jevNode("li", "jev-custom-option", ...controls, actions);
    row.dataset.jevCustomOption = "";
    return row;
  });
  const add = named
    ? jevText("jev.custom.addOption", "＋ 보기 더하기", "button", "btn jev-custom-add")
    : jevText("jev.custom.addLevel", "＋ 단계 더하기", "button", "btn jev-custom-add");
  add.type = "button";
  add.dataset.jevCustomAdd = answer.kind;
  holder.replaceChildren(legend,
    named
      ? jevCustomAbout("jev.custom.optionsAbout", "보기마다 이름과 뜻을 적습니다. 보기는 {{optionsLeast}}~{{optionsMost}}개, 뜻은 각 {{meaningMax}}자까지입니다. 어느 보기도 맞지 않으면 답은 그렇다고 말할 수 있습니다.")
      : jevCustomAbout("jev.custom.levelsAbout", "가장 낮은 단계부터 각 단계가 무엇을 뜻하는지 적습니다. {{optionsLeast}}~{{optionsMost}}단계, 뜻은 각 {{meaningMax}}자까지입니다."),
    jevNode("ol", "jev-custom-options", ...rows), add);
  jevCustomOptionsCounted(form, rules);
  jevCustomExamples(form, answer.kind);
}

/* Examples describe the chosen answer shape without becoming draft values.
 * Keep the source beside each placeholder so a locale change translates it
 * without replacing anything the person has written. */
function jevCustomExamples(form, kind) {
  const hint = (node, { key, source }) => {
    if (!node) return;
    node.dataset.i18nPlaceholder = key;
    node.dataset.i18nSourceplaceholder = source;
    node.placeholder = t(key, source);
  };
  const field = (name, words) => hint(form.querySelector(`[name="${name}"]`), words);
  field("name", { key: "jev.custom.example.name", source: "예: 추가 확인 필요, 요청 분류, 난이도 판단" });
  let example;
  if (kind === "choice") {
    field("question", { key: "jev.custom.example.choiceQuestion", source: "예: 이 요청에서 가장 먼저 할 일은 무엇인가요?" });
    example = jevText("jev.custom.example.choice", "예시 · 요청 분류: ‘코드 수정’은 기능을 추가하거나 바꾸는 요청, ‘원인 조사’는 오류의 이유를 찾는 요청입니다. 보기마다 구별되는 기준을 적어 주세요.", "p", "jev-custom-example");
    form.querySelectorAll("[data-jev-custom-option]").forEach((row) => {
      hint(row.querySelector("[name='option_name']"), { key: "jev.custom.example.optionName", source: "예: 코드 수정 / 원인 조사" });
      hint(row.querySelector("[name='meaning']"), { key: "jev.custom.example.optionMeaning", source: "예: ‘코드 수정’ — 기능을 추가하거나 기존 동작을 바꾸는 요청" });
    });
  } else if (kind === "score") {
    field("question", { key: "jev.custom.example.scoreQuestion", source: "예: 이 요청을 구현하는 난이도는 어느 정도인가요?" });
    example = jevText("jev.custom.example.score", "예시 · 난이도 판단: 1단계는 한 파일의 작은 수정, 2단계는 여러 파일의 동작 변경, 3단계는 구조를 바꾸는 작업입니다. 낮은 단계부터 기준을 적어 주세요.", "p", "jev-custom-example");
    form.querySelectorAll("[name='level']").forEach((node) => {
      hint(node, { key: "jev.custom.example.level", source: "예: 한 파일의 작은 수정 / 여러 파일의 동작 변경 / 구조 변경" });
    });
  } else {
    field("question", { key: "jev.custom.example.yesNoQuestion", source: "예: 이 요청을 진행하려면 사용자에게 정보를 더 물어봐야 하나요?" });
    field("yes", { key: "jev.custom.example.yes", source: "예: 목표나 조건이 빠져 있어 추가 확인이 필요하다" });
    field("no", { key: "jev.custom.example.no", source: "예: 목표와 조건이 충분해서 바로 진행할 수 있다" });
    example = jevText("jev.custom.example.yesNo", "예시 · 추가 확인 필요: 요청에 정보가 빠졌으면 ‘예’, 바로 진행할 수 있으면 ‘아니오’입니다. 두 답을 가르는 기준을 적어 주세요.", "p", "jev-custom-example");
  }
  const holder = form.querySelector("[data-jev-custom-field='answer']");
  holder.insertBefore(example, holder.children[2]);
}

/* The options' labels and presses after one was added or taken away: each
 * level named by its place, and no press past the bounds. */
function jevCustomOptionsCounted(form, rules) {
  const holder = form.querySelector("[data-jev-custom-field='answer']");
  const rows = [...holder.querySelectorAll("[data-jev-custom-option]")];
  const named = holder.querySelector("[data-jev-custom-add]")?.dataset.jevCustomAdd === "choice";
  rows.forEach((row, at) => {
    say(row.querySelector(".jev-custom-option-label"), () => (named
      ? t("jev.custom.optionName", "보기 {{count}}", { count: jevCount(at + 1) })
      : t("jev.custom.level", "{{count}}단계", { count: jevCount(at + 1) })));
    row.querySelector("[data-jev-custom-remove]").disabled = rows.length <= rules.options.least;
    const up = row.querySelector('[data-jev-custom-move="up"]');
    const down = row.querySelector('[data-jev-custom-move="down"]');
    if (up) up.disabled = at === 0;
    if (down) down.disabled = at === rows.length - 1;
  });
  const add = holder.querySelector("[data-jev-custom-add]");
  if (add) add.disabled = rows.length >= rules.options.most;
}

/* The inputs a point offers, ticked where the draft reads them, and what
 * the point does with an answer when the decision is on. */
function jevCustomPointFields(form, at, reads, rules) {
  const point = rules.points.find((one) => one.at === at) ?? rules.points[0];
  const holder = form.querySelector("[data-jev-custom-field='reads']");
  for (const tick of holder.querySelectorAll(".jev-custom-choice")) tick.remove();
  holder.append(...point.inputs.map((input) => jevCustomTick("checkbox", "reads", input, reads.includes(input), jevCustomSay("input", input))));
  say(form.querySelector("[data-jev-custom-act]"), () => t("jev.custom.act", "켜면 하는 일: {{act}}", { act: jevCustomSay("point-act", point.at) }));
}

/* What stands in the drawer for the choices made: the act-from line in
 * words, the folders to pick from only when the person picks, and each
 * count of what was written against its bound. */
function jevCustomFieldsShown(form) {
  const share = form.querySelector("[name='actFrom']");
  form.querySelector(".jev-custom-share").textContent = t("jev.custom.actsFrom", "확신 {{share}} 이상", { share: jevPercent(Number(share.value)) });
  form.querySelector(".jev-custom-folders").hidden = form.querySelector("[name='folders']:checked")?.value !== "chosen";
}

/* Open the drawer on `kept` — a decision the store keeps — or on a new one,
 * with the keyboard in its name; `opener` takes the keyboard back, and the
 * part's own 「새 결정」 does when the opener is gone. */
function openJevCustomDrawer(kept, opener) {
  const rules = jevCustomRules();
  if (!rules || jevCustomDrawer) return;
  const form = el("jev-custom-form");
  const view = opener?.closest(".jev-view") ?? null;
  jevCustomDrawer = { kept, view, opened: "", check: null, timer: null, wanted: 0, warned: false, saving: false };
  say(el("jev-custom-title"), () => (kept ? t("jev.custom.editTitle", "결정 고치기") : t("jev.custom.newTitle", "새 결정")));
  el("jev-custom-delete").hidden = !kept;
  el("jev-custom-unsaved").hidden = true;
  jevCustomFormOf(form, kept?.draft ?? jevCustomBlank(rules), rules);
  jevCustomDrawer.opened = JSON.stringify(jevCustomDraftOf(form));
  showModal(el("jev-custom-scrim"), {
    opener,
    fallback: () => view?.querySelector("[data-jev-custom-new]") ?? null,
  });
  jevCustomCheckSoon(0);
}

/* Close the drawer — 「닫기」's road, 「취소」's and Esc's (`MODAL_SPECS`):
 * words not saved hold it open once and say so, and a second close lets them
 * go. */
function closeJevCustomDrawer({ saved = false } = {}) {
  const drawer = jevCustomDrawer;
  if (!drawer) return;
  const unsaved = !saved && JSON.stringify(jevCustomDraftOf(el("jev-custom-form"))) !== drawer.opened;
  if (unsaved && !drawer.warned) {
    drawer.warned = true;
    el("jev-custom-unsaved").hidden = false;
    return;
  }
  window.clearTimeout(drawer.timer);
  jevCustomDrawer = null;
  hideModal(el("jev-custom-scrim"));
}

/* A change in the drawer: a kind or a point chosen anew redraws the fields
 * it decides, and core's check is asked again after a pause. */
function jevCustomEdited(event) {
  const drawer = jevCustomDrawer;
  if (!drawer) return;
  const form = el("jev-custom-form");
  const rules = jevCustomRules();
  const target = event.target;
  if (target.name === "kind") jevCustomAnswerFields(form, jevCustomBlankAnswer(target.value, rules), rules);
  if (target.name === "at") {
    const point = rules.points.find((one) => one.at === target.value);
    jevCustomPointFields(form, target.value, point ? [...point.inputs] : [], rules);
  }
  if (target.name === "actFrom") target.value = String(Math.round(Number(target.value) * 100) / 100);
  jevCustomFieldsShown(form);
  drawer.warned = false;
  el("jev-custom-unsaved").hidden = true;
  jevCustomCheckSoon();
}

/* Add an option or a level, or take one away. */
function jevCustomOptionPressed(event) {
  const button = event.target.closest("[data-jev-custom-add], [data-jev-custom-remove], [data-jev-custom-move]");
  const form = el("jev-custom-form");
  const rules = jevCustomRules();
  if (!button || !jevCustomDrawer || !rules) return;
  if (button.dataset.jevCustomMove) {
    const row = button.closest("[data-jev-custom-option]");
    if (button.dataset.jevCustomMove === "up" && row.previousElementSibling) row.previousElementSibling.before(row);
    else if (button.dataset.jevCustomMove === "down" && row.nextElementSibling) row.nextElementSibling.after(row);
    row.querySelector("textarea")?.focus();
  } else if (button.matches("[data-jev-custom-remove]")) {
    button.closest("[data-jev-custom-option]").remove();
  } else {
    const draft = jevCustomDraftOf(form);
    const answer = draft.answer.kind === "choice"
      ? { ...draft.answer, options: [...draft.answer.options, { name: "", means: "" }] }
      : { ...draft.answer, levels: [...draft.answer.levels, ""] };
    jevCustomAnswerFields(form, answer, rules);
    form.querySelector("[data-jev-custom-option]:last-of-type input, [data-jev-custom-option]:last-of-type textarea")?.focus();
  }
  jevCustomOptionsCounted(form, rules);
  jevCustomCheckSoon();
}

/* Ask core's check after `after` ms; the drawer is busy, and its save off,
 * until the answer to the newest change is painted. */
function jevCustomCheckSoon(after = JEV_CUSTOM_CHECK_MS) {
  const drawer = jevCustomDrawer;
  if (!drawer) return;
  paintJevInputPreview(el("jev-custom-form"));
  drawer.wanted += 1;
  const wanted = drawer.wanted;
  el("jev-custom-form").setAttribute("aria-busy", "true");
  el("jev-custom-save").disabled = true;
  window.clearTimeout(drawer.timer);
  drawer.timer = window.setTimeout(() => void jevCustomCheck(drawer, wanted), after);
}

async function jevCustomCheck(drawer, wanted) {
  const form = el("jev-custom-form");
  let check;
  try {
    check = await invoke("check_custom_decision", { draft: jevCustomDraftOf(form) });
  } catch (failure) {
    check = { refusals: [], version: null, questions: null, line: null, failure: jevCustomFailure(failure) };
  }
  if (jevCustomDrawer !== drawer || wanted !== drawer.wanted) return;
  drawer.check = check;
  paintJevCustomCheck(form, check);
  form.setAttribute("aria-busy", "false");
}

/* Core's check, said in the drawer: each refusal in words with the control
 * it points at marked, the new-version notice while an edit is a new
 * version, the line an agent reads and the question as core would send it —
 * and 「저장」 on only when nothing is refused. */
function paintJevCustomCheck(form, check) {
  const drawer = jevCustomDrawer;
  const rules = jevCustomRules();
  for (const marked of form.querySelectorAll("[aria-invalid]")) marked.removeAttribute("aria-invalid");
  for (const marked of form.querySelectorAll("[data-refused]")) delete marked.dataset.refused;
  const said = form.querySelector("[data-jev-custom-refusals]");
  said.replaceChildren(...check.refusals.map((refusal) => {
    const item = jevNode("li", "jev-custom-refusal");
    item.dataset.jevCustomRefusal = refusal.word;
    say(item, () => jevCustomRefusalWords(refusal, jevCustomRules() ?? rules));
    jevCustomMark(form, refusal);
    return item;
  }));
  if (check.failure) said.append(jevNode("li", "jev-custom-refusal", document.createTextNode(check.failure)));
  said.hidden = said.childElementCount === 0;
  form.querySelector("[data-jev-custom-version]").hidden = !drawer.kept || !check.version || check.version === drawer.kept.version;
  const line = form.querySelector("[data-jev-custom-line]");
  line.textContent = check.line ?? "";
  line.closest("[data-jev-custom-field]").hidden = !check.line;
  form.querySelector("[data-jev-custom-questions]").textContent = check.questions ? JSON.stringify(check.questions, null, 2) : "";
  el("jev-custom-save").disabled = check.refusals.length > 0 || Boolean(check.failure) || drawer.saving;
}

/* Mark the control a refusal points at — the field core names (its
 * `Field::word`, which the drawer's controls are named by), in the option or
 * level it names, else the control or the part of the drawer the markup puts
 * its rule under (`data-jev-refusal-at`). */
function jevCustomMark(form, refusal) {
  const rows = [...form.querySelectorAll("[data-jev-custom-option]")];
  const option = rows.find((row) => row.querySelector("[name='option_name']")?.value === refusal.option);
  const within = refusal.level !== null && refusal.level !== undefined ? rows[refusal.level] : option;
  const named = refusal.option !== null && refusal.option !== undefined ? "option_name" : null;
  const at = refusal.field ?? named ?? jevCustomWordNode("refusal", refusal.word)?.dataset.jevRefusalAt ?? "";
  if (!at) return;
  const control = (within ?? form).querySelector(
    `textarea[name="${at}"], input[name="${at}"]:not([type="checkbox"]):not([type="radio"])`);
  control?.setAttribute("aria-invalid", "true");
  const part = control?.closest("[data-jev-custom-field]") ?? form.querySelector(`[data-jev-custom-field="${at}"]`);
  if (part) part.dataset.refused = "";
}

/* Keep the drawer's draft: a new decision recording before anything acts on
 * it, an edit as its next version with its mode kept — both core's store's
 * doing. A refusal the store finds is said where the check's are. */
async function saveJevCustomDrawer() {
  const drawer = jevCustomDrawer;
  const save = el("jev-custom-save");
  if (!drawer || save.disabled) return;
  drawer.saving = true;
  save.disabled = true;
  const draft = jevCustomDraftOf(el("jev-custom-form"));
  try {
    jevCustom = await invoke("save_custom_decision", { draft });
  } catch (failure) {
    drawer.saving = false;
    if (jevCustomDrawer !== drawer) return;
    if (Array.isArray(failure?.refusals) && failure.refusals.length > 0) {
      drawer.check = { ...(drawer.check ?? { version: null, questions: null, line: null }), refusals: failure.refusals };
      paintJevCustomCheck(el("jev-custom-form"), drawer.check);
    } else {
      toast(jevCustomFailure(failure), "halt");
      save.disabled = false;
    }
    return;
  }
  drawer.saving = false;
  paintJevViews();
  if (jevCustomDrawer === drawer) closeJevCustomDrawer({ saved: true });
  toast(draft.id
    ? t("jev.custom.saved", "결정을 저장했습니다: 「{{name}}」", { name: draft.name })
    : t("jev.custom.made", "새 결정을 만들었습니다: 「{{name}}」. 기록만으로 시작합니다.", { name: draft.name }));
}

/* Ask once before `kept` is deleted (DESIGN 3): its records stay, and it
 * can be restored for the store's days. `opener` — the row's 「삭제」 or the
 * drawer's — takes the keyboard back if the person keeps it. */
function openJevCustomDelete(kept, opener) {
  jevCustomDeleting = { kept, view: opener?.closest(".jev-view") ?? jevCustomDrawer?.view ?? null };
  say(el("jev-custom-delete-title"), () => t("jev.custom.deleteTitle", "「{{name}}」 결정을 지울까요?", { name: kept.draft.name }));
  say(el("jev-custom-delete-body"), () => t("jev.custom.deleteBody",
    "지금까지의 기록은 남고, {{days}}일 동안 되살릴 수 있습니다.", jevCustomBounds(jevCustomRules())));
  showModal(el("jev-custom-delete-scrim"), { opener, animated: true });
}

/* Close the delete question and only it — the drawer under it stays. */
function closeJevCustomDelete() {
  jevCustomDeleting = null;
  hideModal(el("jev-custom-delete-scrim"));
}

/* 「지우기」: the drawer on the decision closes, the store deletes it, and
 * 「되돌리기」 stands for the design's time. */
async function confirmJevCustomDelete() {
  const asked = jevCustomDeleting;
  if (!asked) return;
  closeJevCustomDelete();
  if (asked.kind === "builtin") {
    await deleteJevBuiltin(asked);
    return;
  }
  if (jevCustomDrawer?.kept?.draft.id === asked.kept.draft.id) closeJevCustomDrawer({ saved: true });
  if (!(await jevCustomChange(() => invoke("delete_custom_decision", { id: asked.kept.draft.id })))) return;
  if (document.activeElement === null || document.activeElement === document.body) {
    asked.view?.querySelector("[data-jev-custom-new]")?.focus({ preventScroll: true });
  }
  jevCustomUndo(asked.kept);
}

/* 「되돌리기」 for the decision just deleted: it stands the design's time,
 * longer than a plain toast's, and leaves once used. */
function jevCustomUndo(kept) {
  const id = kept.draft.id;
  let note = null;
  const leave = () => {
    if (note?.isConnected) closing(note, () => note.remove());
  };
  note = toast(t("jev.custom.deleted", "결정을 지웠습니다: 「{{name}}」", { name: kept.draft.name }), "", {
    sticky: true,
    action: {
      label: t("jev.custom.undo", "되돌리기"),
      run: () => {
        leave();
        void jevCustomChange(() => invoke("restore_custom_decision", { id }));
      },
    },
  });
  if (!note) return;
  note.dataset.jevCustomUndo = id;
  window.setTimeout(leave, JEV_CUSTOM_UNDO_MS);
}

el("jev-custom-close").addEventListener("click", () => closeJevCustomDrawer());
el("jev-custom-cancel").addEventListener("click", () => closeJevCustomDrawer());
el("jev-custom-save").addEventListener("click", () => void saveJevCustomDrawer());
el("jev-custom-delete").addEventListener("click", (event) => {
  if (jevCustomDrawer?.kept) openJevCustomDelete(jevCustomDrawer.kept, event.currentTarget);
});
el("jev-custom-scrim").addEventListener("mousedown", (event) => {
  if (event.target === el("jev-custom-scrim")) closeJevCustomDrawer();
});
el("jev-custom-form").addEventListener("input", jevCustomEdited);
el("jev-custom-form").addEventListener("click", jevCustomOptionPressed);
el("jev-custom-form").addEventListener("submit", (event) => event.preventDefault());
el("jev-custom-delete-ok").addEventListener("click", () => void confirmJevCustomDelete());
el("jev-custom-delete-cancel").addEventListener("click", closeJevCustomDelete);
el("jev-custom-delete-scrim").addEventListener("mousedown", (event) => {
  if (event.target === el("jev-custom-delete-scrim")) closeJevCustomDelete();
});

/* Built-in definitions use the same settings authority as their execution. */
let jevBuiltinEditor = null;

function jevBuiltinSaved(id) {
  return jevSeat(typesafeState ?? {}, id)?.decision ?? {};
}

function jevBuiltinDefaultName(id) {
  const name = jevFeature(id).name;
  return name ? t(name.key, name.source) : id;
}

function jevBuiltinFailure(error) {
  const token = error?.message ?? String(error);
  const words = {
    builtin_decision_changed: t("jev.builtin.changed", "다른 창에서 이 결정을 바꿨습니다. 닫고 다시 열어 최신 내용을 확인하세요."),
    builtin_decision_missing: t("jev.builtin.missing", "이 기본 결정을 찾지 못했습니다. 목록을 새로 읽어 주세요."),
    builtin_decision_definition_unavailable: t("jev.builtin.unavailable", "기본 질문을 불러오지 못했습니다. 앱과 zo를 업데이트한 뒤 다시 열어 주세요."),
    builtin_decision_credential: t("jev.builtin.credential", "질문이나 기준에 비밀번호·키로 보이는 내용이 있습니다. 지운 뒤 저장하세요."),
    builtin_decision_name: t("jev.builtin.invalidName", "이름이 비어 있거나 너무 깁니다. 입력 칸의 길이 제한을 확인하세요."),
    builtin_decision_question: t("jev.builtin.invalidQuestion", "질문이 비어 있거나 너무 깁니다. 입력 칸의 길이 제한을 확인하세요."),
    builtin_decision_criteria: t("jev.builtin.invalidCriteria", "판단 기준이 비어 있거나 너무 깁니다. 입력 칸의 길이 제한을 확인하세요."),
    builtin_decision_question_schema: t("jev.question.schemaChanged", "저장된 문항과 현재 앱의 유형·보기가 맞지 않습니다. 기본값을 불러온 뒤 다시 수정하세요."),
    builtin_decision_question_content: t("jev.question.invalidContent", "질문·보기의 내용이나 길이를 확인하세요."),
    builtin_decision_options: t("jev.question.invalidOptions", "선택한 유형에 맞는 보기와 단계가 필요합니다."),
    builtin_decision_mode: t("jev.builtin.invalidMode", "이 결정이 지원하는 사용 방식을 골라 주세요. 수정한 기준은 기록만 또는 켜짐으로 사용할 수 있습니다."),
  };
  return words[token] ?? typesafeRefusal(error);
}

function jevBuiltinTrashPart() {
  const fold = jevNode("details", "jev-custom-trash jev-builtin-trash", jevNode("summary"), jevNode("ul", "jev-custom-trash-list"));
  fold.dataset.jevBuiltinTrash = "";
  fold.hidden = true;
  return fold;
}

function paintJevBuiltinTrash(view) {
  const fold = view.querySelector("[data-jev-builtin-trash]");
  const rows = (typesafeState?.switches ?? []).filter((row) => row.decision?.deleted);
  fold.hidden = rows.length === 0;
  if (!jevMoved(fold, JSON.stringify(rows), jevLanguage())) return;
  fold.querySelector("summary").textContent = t("jev.builtin.trash", "삭제한 기본 결정 {{count}}개", { count: jevCount(rows.length) });
  fold.querySelector("ul").replaceChildren(...rows.map((row) => {
    const restore = jevText("jev.custom.restore", "되살리기", "button", "btn");
    restore.type = "button";
    restore.dataset.jevBuiltinRestore = row.id;
    return jevNode("li", "jev-custom-trashed", jevNode("span", "jev-custom-name", document.createTextNode(jevSeatName(row.id))), restore);
  }));
}

function jevBuiltinPressed(event) {
  const button = event.target.closest("[data-jev-builtin-edit], [data-jev-builtin-delete], [data-jev-builtin-restore]");
  if (!button) return false;
  event.stopPropagation();
  if (button.disabled) return true;
  if (button.dataset.jevBuiltinEdit) void openJevBuiltinEditor(button.dataset.jevBuiltinEdit, button);
  else if (button.dataset.jevBuiltinDelete) openJevBuiltinDelete(button.dataset.jevBuiltinDelete, button);
  else {
    button.disabled = true;
    const id = button.dataset.jevBuiltinRestore;
    void changeJevBuiltin(id, "restore", jevBuiltinSaved(id)).catch((error) => {
      toast(jevBuiltinFailure(error), "halt");
    }).finally(() => { button.disabled = false; });
  }
  return true;
}

/* Jev's primitives keep their own criteria shape. Built-ins fix the keys
 * consumed by the app; custom decisions choose their type and option set. */
function jevQuestionType(kind) {
  if (kind === "noul" || kind === "yes_no") return t("jev.question.noul", "예/아니요 · Noul");
  if (kind === "choice") return t("jev.question.choice", "하나 고르기 · Choice");
  return t("jev.question.score", "단계 점수 · Score");
}

function jevQuestionSame(a, b) {
  const ordered = (value) => Array.isArray(value) ? value.map(ordered)
    : value && typeof value === "object" ? Object.fromEntries(Object.keys(value).sort().map((key) => [key, ordered(value[key])])) : value;
  return JSON.stringify(ordered(a)) === JSON.stringify(ordered(b));
}

/* This preview reads the form only. It never asks a model or writes a draft. */
function jevInputPreview(form, read) {
  let preview = form.querySelector("[data-jev-input-preview]");
  if (!preview) {
    preview = jevNode("details", "jev-custom-preview jev-input-preview",
      jevText("jev.question.inputPreview", "입력 형태 미리보기", "summary"),
      jevText("jev.question.previewAbout", "입력 형태만 미리 봅니다. 모델을 호출하지 않습니다. 문항 수정은 판단 모델을 바꾸지 않습니다.", "p", "jev-custom-about-line"),
      jevNode("div", "jev-input-preview-body"));
    preview.dataset.jevInputPreview = "";
    preview.addEventListener("toggle", () => paintJevInputPreview(form));
  }
  preview.readQuestions = read;
  return preview;
}

function paintJevInputPreview(form) {
  const preview = form?.querySelector("[data-jev-input-preview]");
  if (!preview?.open) return;
  const body = preview.querySelector(".jev-input-preview-body");
  const questions = preview.readQuestions();
  if (!questions) {
    body.replaceChildren(jevText("jev.question.invalidContent", "질문·보기의 내용이나 길이를 확인하세요.", "p", "jev-builtin-status is-error"));
    return;
  }
  const words = value => typeof value === "string" ? value : JSON.stringify(value, null, 2);
  body.replaceChildren(...questions.map(question => {
    const entries = question.criteria.map(([key, value]) => jevNode("li", "", jevNode("strong", "", document.createTextNode(key)),
      jevNode("p", "jev-custom-about-line", document.createTextNode(words(value) ?? ""))));
    const group = jevNode("section", "jev-input-preview-question", jevNode("h3", "", document.createTextNode(jevQuestionType(question.type))),
      jevNode("p", "", document.createTextNode(words(question.instructions) ?? "")), jevNode("ol", "", ...entries));
    group.dataset.previewType = question.type;
    return group;
  }));
}

function jevQuestionValue(label, value, cap) {
  const area = document.createElement("textarea");
  area.className = "jev-custom-area"; area.rows = 3;
  area.value = typeof value === "string" ? value : JSON.stringify(value, null, 2);
  area.maxLength = cap;
  const format = document.createElement("select");
  format.className = "jev-custom-input";
  format.setAttribute("aria-label", t("jev.question.formatFor", "{{name}}의 내용 형식", { name: label }));
  for (const [key, words] of [["text", t("jev.question.text", "텍스트")], ["json", t("jev.question.json", "구조화된 JSON")]]) {
    const option = document.createElement("option"); option.value = key; option.textContent = words; format.append(option);
  }
  format.value = typeof value === "string" ? "text" : "json";
  let previous = format.value;
  format.addEventListener("change", () => {
    if (format.value === previous) return;
    if (format.value === "json") area.value = JSON.stringify(area.value);
    else { try { const value = JSON.parse(area.value); area.value = typeof value === "string" ? value : JSON.stringify(value, null, 2); } catch { /* Keep an invalid draft visible. */ } }
    previous = format.value;
    area.dispatchEvent(new Event("input", { bubbles: true }));
  });
  const error = jevNode("small", "jev-builtin-status is-error");
  error.hidden = true; error.setAttribute("aria-live", "polite");
  const node = jevNode("div", "jev-custom-field jev-question-value",
    jevNode("label", "jev-custom-label", document.createTextNode(label), area), format, error);
  return { node, area, format, read() {
    let value = area.value;
    try {
      if (format.value === "json") value = JSON.parse(value);
      if (value !== null && typeof value !== "string" && typeof value !== "object") throw new Error("shape");
      if ([...JSON.stringify(value)].length > cap) throw new Error("length");
      area.setCustomValidity(""); area.removeAttribute("aria-invalid"); error.hidden = true; error.textContent = "";
      return { valid: true, value };
    } catch {
      const words = format.value === "json" ? t("jev.question.invalidJson", "문자열·객체·배열·null 중 하나를 길이 제한 안의 JSON으로 입력하세요.")
        : t("jev.question.invalidContent", "질문·보기의 내용이나 길이를 확인하세요.");
      area.setCustomValidity(words); area.setAttribute("aria-invalid", "true");
      error.textContent = words; error.hidden = false;
      return { valid: false };
    }
  } };
}

function buildJevBuiltinQuestions() {
  const editor = jevBuiltinEditor;
  const definition = editor.definition;
  editor.fields = [];
  const saved = editor.reset ? {} : (definition.saved?.questions ?? {});
  editor.unmatched = Object.keys(saved).some((id) => !(definition.questions ?? []).some((question) => question.id === id));
  if (editor.unmatched) {
    el("jev-builtin-status").textContent = t("jev.question.schemaChanged", "저장된 문항과 현재 앱의 유형·보기가 맞지 않습니다. 기본값을 불러온 뒤 다시 수정하세요.");
    el("jev-builtin-status").classList.add("is-error");
  } else if (editor.reset) { el("jev-builtin-status").textContent = ""; el("jev-builtin-status").classList.remove("is-error"); }
  el("jev-builtin-questions").replaceChildren(...(definition.questions ?? []).map((question, index) => {
    const edit = saved[question.id];
    const instructions = jevQuestionValue(t("jev.custom.questionLegend", "질문"), edit ? edit.instructions : question.instructions, definition.questionMax);
    instructions.area.dataset.jevQuestionInstructions = question.id;
    const criteria = question.type === "score" ? (edit?.criteria ?? question.criteria)
      : { ...(question.criteria ?? {}), ...(edit?.criteria ?? {}) };
    const keys = question.type === "noul" ? ["true", "false"] : Object.keys(criteria);
    const fields = keys.map((key) => {
      const label = question.type === "noul"
        ? key === "true" ? t("jev.custom.yesMeans", "「예」의 뜻") : t("jev.custom.noMeans", "「아니오」의 뜻")
        : question.type === "score" ? t("jev.question.level", "{{count}}단계 · 점수 {{score}}", { count: Number(key) + 1, score: key })
          : t("jev.question.option", "보기 · {{key}}", { key });
      const field = jevQuestionValue(label, criteria[key] ?? null, definition.criteriaMax);
      field.area.dataset.jevQuestionCriterion = key;
      return { key, ...field };
    });
    const help = jevText("jev.question.fixed", "기본 결정은 앱이 사용하는 답의 유형·보기 키·단계 순서를 유지합니다. 질문과 각 보기의 설명을 수정할 수 있습니다.", "p", "jev-custom-about-line");
    const heading = jevNode("legend", "", document.createTextNode(t("jev.question.number", "문항 {{count}} · {{type}}", { count: index + 1, type: jevQuestionType(question.type) })));
    const group = jevNode("fieldset", "jev-custom-field jev-builtin-question", heading,
      jevNode("code", "jev-custom-about-line", document.createTextNode(question.id)), help, instructions.node, ...fields.map((field) => field.node));
    group.dataset.jevQuestion = question.id;
    if (question.dynamicOptions) group.append(jevText("jev.question.dynamicOptions", "현재 작업의 후보로 보기가 만들어집니다. 여기서는 고정 보기만 수정하며, 실행 때 제공되지 않은 보기를 추가하지 않습니다.", "p", "jev-custom-example"));
    if (question.contextual) group.append(jevText("jev.question.contextual", "실행 때 각 문항이 가리키는 항목과 작업 상황은 유지됩니다.", "p", "jev-custom-about-line"));
    editor.fields.push({ question, instructions, criteria: fields });
    return group;
  }));
  const legacy = el("jev-builtin-legacy");
  legacy.hidden = !definition.canEditPrompt || (!definition.dynamic && !el("jev-builtin-question").value && !el("jev-builtin-criteria").value);
  legacy.open = !legacy.hidden;
  const form = el("jev-builtin-form");
  const preview = jevInputPreview(form, () => {
    const questions = [];
    for (const row of jevBuiltinEditor?.fields ?? []) {
      const instructions = row.instructions.read();
      const criteria = row.criteria.map(field => [field.key, field.read()]);
      if (!instructions.valid || criteria.some(([, field]) => !field.valid)) return null;
      questions.push({ type: row.question.type, instructions: instructions.value, criteria: criteria.map(([key, field]) => [key, field.value]) });
    }
    return questions;
  });
  legacy.after(preview);
}

function jevBuiltinQuestionDraft() {
  const questions = {};
  for (const row of jevBuiltinEditor.fields ?? []) {
    const instructions = row.instructions.read();
    const entries = row.criteria.map((field) => [field.key, field.read()]);
    if (!instructions.valid || entries.some(([, field]) => !field.valid)) return null;
    const original = row.question;
    const criteria = original.type === "score" ? entries.map(([, field]) => field.value)
      : Object.fromEntries(entries.filter(([key, field]) => !jevQuestionSame(field.value, original.criteria?.[key] ?? null)).map(([key, field]) => [key, field.value]));
    const changedCriteria = original.type === "score" ? !jevQuestionSame(criteria, original.criteria) : Object.keys(criteria).length > 0;
    if (!jevQuestionSame(instructions.value, original.instructions) || changedCriteria) {
      questions[original.id] = { type: original.type, instructions: instructions.value, criteria, ...(original.contextual ? { contextual: true } : {}) };
    }
  }
  return questions;
}

function jevBuiltinRaw() {
  return {
    name: el("jev-builtin-name").value,
    question: el("jev-builtin-question").value,
    criteria: el("jev-builtin-criteria").value,
    mode: jevBuiltinEditor?.mode ?? "",
    reset: Boolean(jevBuiltinEditor?.reset),
    questions: (jevBuiltinEditor?.fields ?? []).map((row) => [row.instructions, ...row.criteria].map((field) => [field.format.value, field.area.value])),
  };
}

function jevBuiltinDraft() {
  const editor = jevBuiltinEditor;
  if (!editor?.definition) return null;
  const raw = jevBuiltinRaw();
  const questions = jevBuiltinQuestionDraft();
  if (!questions) return null;
  const changed = (text, original) => text.trim() && text.trim() !== original.trim() ? text.trim() : null;
  return {
    name: changed(raw.name, editor.defaultName),
    question: changed(raw.question, ""),
    criteria: changed(raw.criteria, ""),
    questions,
    mode: raw.mode,
    deleted: false,
  };
}

function paintJevBuiltinEditor() {
  const editor = jevBuiltinEditor;
  if (!editor) return;
  paintJevInputPreview(el("jev-builtin-form"));
  const ready = Boolean(editor.definition);
  const draft = jevBuiltinDraft();
  const prior = editor.definition?.saved ?? {};
  const changedQuestion = ready && draft && !editor.reset && (draft.question !== (prior.question ?? null) || draft.criteria !== (prior.criteria ?? null) || !jevQuestionSame(draft.questions, prior.questions ?? {}));
  const edited = Boolean(draft?.question || draft?.criteria || Object.keys(draft?.questions ?? {}).length);
  const choices = editor.definition?.modes ?? [];
  const recording = choices.find((choice) => choice.asks && !choice.applies && !choice.automatic) ?? choices[0];
  if (changedQuestion && recording) editor.mode = recording.mode;
  el("jev-builtin-recording").hidden = !edited && !changedQuestion;
  for (const button of el("jev-builtin-mode").querySelectorAll("button")) {
    const choice = choices.find((choice) => choice.mode === button.dataset.mode);
    button.hidden = edited && Boolean(choice?.automatic);
    button.disabled = editor.busy || (changedQuestion && button.dataset.mode !== recording?.mode);
    const checked = button.dataset.mode === editor.mode;
    button.setAttribute("aria-checked", String(checked));
    button.tabIndex = checked ? 0 : -1;
  }
  for (const id of ["name", "question", "criteria"]) {
    el(`jev-builtin-${id}`).disabled = !ready || editor.busy || (id !== "name" && !editor.definition.canEditPrompt);
  }
  for (const control of el("jev-builtin-questions").querySelectorAll("textarea, select")) control.disabled = !ready || editor.busy;
  el("jev-builtin-form").setAttribute("aria-busy", String(!ready || editor.busy));
  el("jev-builtin-reset").disabled = !ready || editor.busy;
  el("jev-builtin-save").disabled = !ready || !draft || editor.unmatched || editor.busy || JSON.stringify(jevBuiltinRaw()) === editor.opened;
  el("jev-builtin-close").disabled = editor.busy;
  el("jev-builtin-cancel").disabled = editor.busy;
}

async function openJevBuiltinEditor(id, opener) {
  const editor = { id, opener, definition: null, busy: false, reset: false, mode: "", opened: "", warned: false, defaultName: jevBuiltinDefaultName(id) };
  jevBuiltinEditor = editor;
  const preview = el("jev-builtin-form").querySelector("[data-jev-input-preview]");
  if (preview) preview.open = false;
  for (const name of ["name", "question", "criteria"]) el(`jev-builtin-${name}`).value = "";
  el("jev-builtin-mode").replaceChildren();
  el("jev-builtin-questions").replaceChildren();
  el("jev-builtin-legacy").hidden = true;
  el("jev-builtin-help").textContent = t("jev.builtin.loading", "기본 질문과 저장한 내용을 읽는 중…");
  el("jev-builtin-status").textContent = "";
  el("jev-builtin-status").classList.remove("is-error");
  el("jev-builtin-unsaved").hidden = true;
  paintJevBuiltinEditor();
  showModal(el("jev-builtin-scrim"), { opener, animated: true });
  try {
    const definition = await invoke("builtin_decision_definition", { id });
    if (jevBuiltinEditor !== editor) return;
    editor.definition = definition;
    editor.mode = definition.mode;
    const saved = definition.saved ?? {};
    for (const [name, value, cap] of [["name", saved.name ?? editor.defaultName, definition.nameMax],
      ["question", saved.question ?? "", definition.legacyQuestionMax ?? definition.questionMax], ["criteria", saved.criteria ?? "", definition.criteriaMax]]) {
      const input = el(`jev-builtin-${name}`);
      input.value = value;
      input.maxLength = cap;
    }
    buildJevBuiltinQuestions();
    el("jev-builtin-help").textContent = !definition.canEditPrompt
      ? t("jev.builtin.noQuestion", "받은 답을 재사용하는 기능으로, 모델에 보내는 판단 질문은 없습니다. 이름과 사용 방식을 바꾸거나 삭제할 수 있습니다.")
      : definition.dynamic
        ? t("jev.builtin.dynamic", "작업마다 전달되는 질문을 사용합니다. 여기에 적은 질문·기준을 공통 판단 지침으로 적용하며, 비워 두면 작업의 질문을 따릅니다.")
        : t("jev.question.help", "실제로 보내는 문항과 유형별 보기입니다. 바꾼 내용은 해당 문항에만 적용되며, 기본값 불러오기로 원래 질문과 보기를 복원할 수 있습니다.");
    el("jev-builtin-mode").replaceChildren(...definition.modes.map((choice) => {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "jev-custom-mode-choice";
      button.setAttribute("role", "radio");
      button.dataset.mode = choice.mode;
      button.textContent = choice.automatic ? t("jev.builtin.automatic", "자동") : jevCustomModeWords(choice);
      return button;
    }));
    editor.opened = JSON.stringify(jevBuiltinRaw());
    paintJevBuiltinEditor();
    el("jev-builtin-name").focus({ preventScroll: true });
  } catch (error) {
    if (jevBuiltinEditor !== editor) return;
    el("jev-builtin-status").textContent = jevBuiltinFailure(error);
    el("jev-builtin-status").classList.add("is-error");
    el("jev-builtin-help").textContent = "";
  }
}

function closeJevBuiltinEditor({ saved = false } = {}) {
  const editor = jevBuiltinEditor;
  if (!editor || editor.busy) return;
  if (!saved && editor.definition && JSON.stringify(jevBuiltinRaw()) !== editor.opened && !editor.warned) {
    editor.warned = true;
    el("jev-builtin-unsaved").hidden = false;
    return;
  }
  jevBuiltinEditor = null;
  hideModal(el("jev-builtin-scrim"));
}

async function changeJevBuiltin(id, action, expected, draft = null) {
  const state = await invoke("update_builtin_decision", { id, action, expected, draft });
  paintTypeSafe(state);
  return jevBuiltinSaved(id);
}

async function saveJevBuiltinEditor() {
  const editor = jevBuiltinEditor;
  if (!editor?.definition || editor.busy || JSON.stringify(jevBuiltinRaw()) === editor.opened) return;
  const draft = jevBuiltinDraft();
  if (!draft || editor.unmatched || !el("jev-builtin-form").reportValidity()) return;
  editor.busy = true;
  paintJevBuiltinEditor();
  el("jev-builtin-status").textContent = "";
  try {
    await changeJevBuiltin(editor.id, editor.reset ? "reset" : "save", editor.definition.saved ?? {}, draft);
    editor.busy = false;
    if (jevBuiltinEditor === editor) closeJevBuiltinEditor({ saved: true });
  } catch (error) {
    if (jevBuiltinEditor !== editor) return;
    el("jev-builtin-status").textContent = jevBuiltinFailure(error);
    el("jev-builtin-status").classList.add("is-error");
  } finally {
    editor.busy = false;
    if (jevBuiltinEditor === editor) paintJevBuiltinEditor();
  }
}

function openJevBuiltinDelete(id, opener) {
  const name = jevSeatName(id);
  jevCustomDeleting = { kind: "builtin", id, name, expected: structuredClone(jevBuiltinSaved(id)), view: opener.closest(".jev-view") };
  say(el("jev-custom-delete-title"), () => t("jev.custom.deleteTitle", "「{{name}}」 결정을 지울까요?", { name }));
  say(el("jev-custom-delete-body"), () => t("jev.builtin.deleteBody", "삭제하면 이 결정의 판단 요청이 중지됩니다. 기록은 남고, 삭제한 결정에서 되살릴 수 있습니다. 전체 기능을 다시 켜도 이 결정은 삭제 상태를 유지합니다."));
  showModal(el("jev-custom-delete-scrim"), { opener, animated: true });
}

async function deleteJevBuiltin(asked) {
  try {
    const deleted = await changeJevBuiltin(asked.id, "delete", asked.expected);
    if (jevDrawerSeat === asked.id) jevDrawerSeat = null;
    const fold = asked.view?.querySelector("[data-jev-builtin-trash]");
    if (fold) { fold.open = true; fold.querySelector("summary").focus({ preventScroll: true }); }
    let note = null;
    const leave = () => { if (note?.isConnected) closing(note, () => note.remove()); };
    note = toast(t("jev.custom.deleted", "결정을 지웠습니다: 「{{name}}」", { name: asked.name }), "", {
      sticky: true,
      action: { label: t("jev.custom.undo", "되돌리기"), run: async () => {
        try { await changeJevBuiltin(asked.id, "restore", deleted); leave(); }
        catch (error) { toast(jevBuiltinFailure(error), "halt"); }
      } },
    });
    window.setTimeout(leave, JEV_CUSTOM_UNDO_MS);
  } catch (error) { toast(jevBuiltinFailure(error), "halt"); }
}

el("jev-builtin-close").addEventListener("click", () => closeJevBuiltinEditor());
el("jev-builtin-cancel").addEventListener("click", () => closeJevBuiltinEditor());
el("jev-builtin-save").addEventListener("click", () => void saveJevBuiltinEditor());
el("jev-builtin-scrim").addEventListener("mousedown", (event) => {
  if (event.target === el("jev-builtin-scrim")) closeJevBuiltinEditor();
});
el("jev-builtin-form").addEventListener("submit", (event) => { event.preventDefault(); void saveJevBuiltinEditor(); });
el("jev-builtin-form").addEventListener("input", () => {
  if (!jevBuiltinEditor) return;
  jevBuiltinEditor.reset = false; jevBuiltinEditor.warned = false;
  el("jev-builtin-unsaved").hidden = true; paintJevBuiltinEditor();
});
el("jev-builtin-mode").addEventListener("click", (event) => {
  const button = event.target.closest("[data-mode]");
  if (!button || button.disabled || !jevBuiltinEditor) return;
  jevBuiltinEditor.mode = button.dataset.mode; jevBuiltinEditor.reset = false;
  paintJevBuiltinEditor();
});
el("jev-builtin-mode").addEventListener("keydown", (event) => {
  const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[event.key];
  const choices = [...event.currentTarget.querySelectorAll("button")].filter((button) => !button.hidden && !button.disabled);
  if (!step || !choices.length) return;
  event.preventDefault();
  const next = choices[(choices.indexOf(document.activeElement) + step + choices.length) % choices.length];
  next.focus(); next.click();
});
el("jev-builtin-reset").addEventListener("click", () => {
  const editor = jevBuiltinEditor;
  if (!editor?.definition || editor.busy) return;
  el("jev-builtin-name").value = editor.defaultName;
  el("jev-builtin-question").value = "";
  el("jev-builtin-criteria").value = "";
  editor.mode = editor.definition.defaultMode; editor.reset = true; editor.warned = false;
  buildJevBuiltinQuestions();
  el("jev-builtin-unsaved").hidden = true; paintJevBuiltinEditor();
});
