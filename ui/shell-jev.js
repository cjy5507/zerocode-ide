/* ---- Jev: the ledgers' numbers, on the settings card and on the dashboard ----
 *
 * What zo last said each seat's ledger holds (`zo jev summary --json`,
 * docs/design/jev-settings-20260917.md §5) is one answer with two readers:
 * the settings card draws a line of it under every switch, and the dashboard
 * (docs/design/jev-dashboard-and-perfection-20260921.md §2) draws the table,
 * the recent decisions and the week's trend. Both read from here — one cache,
 * one ask, one set of words — so the number on the card is the number on the
 * dashboard. Neither counts the files: the judge that promotes a seat reads
 * them the same way zo does, and a screen free to disagree with the seat it
 * is drawing is worse than a screen with no numbers.
 *
 * The dashboard's ask is the card's ask with one more flag (`recent`), so a
 * dashboard beside an open card still costs one zo per refresh. */

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

/* The sparkline's own box; the stylesheet decides how large it draws. */
const JEV_SPARK_WIDTH = 64;
const JEV_SPARK_HEIGHT = 18;

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
/* When the last answer landed and what it cost, for the dashboard's own
 * freshness line — the cost is the exec boundary's, measured here rather
 * than promised. */
let jevNumbersAt = 0;
let jevNumbersCostMs = null;
/* What went wrong the last time zo was asked, or null. */
let jevNumbersError = null;
/* The seat whose recent decisions the dashboard lists. */
let jevRecentSeat = null;
let jevSettleTimer = null;
const jevViewsWired = new WeakSet();

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
    if (jevNumbersAskingRecent || !recent) return jevNumbersAsking;
    await jevNumbersAsking;
    return loadJevNumbers({ recent });
  }
  jevNumbersAskingRecent = recent;
  jevNumbersAsking = (async () => {
    const began = performance.now();
    let held = null;
    try {
      held = await invoke("jev_summary", recent ? { recent: JEV_RECENT_ROWS } : {});
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
    jevNumbers = held;
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

/* Move one seat's switch — the one door every surface's switch goes
 * through (`set_jev_mode`, the use's own name), and the card's own step
 * runner, so a refused move is said the same way wherever it was asked. */
function moveJevSeat(seat, mode, said) {
  return runTypeSafe(() => invoke("set_jev_mode", { use: seat, mode }), said);
}

/* The word for the line a judgment turned on. The tokens are the core's
 * (`promote::Line::token`); what each one MEANS to a person is this page's,
 * said once here rather than in seven rows of markup. */
function jevLineWords(line) {
  switch (line) {
    case "too_few_rows": return t("settings.typesafe.lineTooFewRows", "행이 더 쌓여야 합니다");
    case "answered": return t("settings.typesafe.lineAnswered", "답한 비율이 모자랍니다");
    case "latency": return t("settings.typesafe.lineLatency", "답이 너무 늦습니다");
    case "schema": return t("settings.typesafe.lineSchema", "형식이 깨진 답이 있었습니다");
    case "too_few_compared": return t("settings.typesafe.lineTooFewCompared", "프로브와 견줄 답이 더 쌓여야 합니다");
    case "agreement": return t("settings.typesafe.lineAgreement", "프로브와 다른 답이 너무 잦습니다");
    case "labels": return t("settings.typesafe.lineLabels", "라벨이 프로브 쪽을 가리킵니다");
    case "fallbacks": return t("settings.typesafe.lineFallbacks", "연달아 되돌아갔습니다");
    default: return "";
  }
}

/* One seat's numbers as words — what it did today, how it answered over the
 * week, and, for a seat whose `auto` may rise, what the judge said of that
 * window. The card's line and the dashboard's last column are this list,
 * joined; a seat nothing has asked yet is one sentence. */
function jevSeatWords(held) {
  if (held.week.rows === 0) {
    return [t("settings.typesafe.seatNeverAsked", "아직 아무것도 묻지 않았습니다.")];
  }
  const words = [t("settings.typesafe.seatCounts", "오늘 {{today}}건 · 7일 {{rows}}건 중 {{answered}} 답함", {
    today: String(held.today.rows),
    rows: String(held.week.rows),
    answered: String(held.week.answered),
  })];
  if (held.week.p95Ms !== null && held.week.p95Ms !== undefined) {
    words.push(t("settings.typesafe.seatP95", "p95 {{ms}} ms", { ms: String(held.week.p95Ms) }));
  }
  const version = jevVersionWords(held);
  if (version) words.push(version);
  if (held.verdict) {
    const because = jevLineWords(held.verdict.line);
    words.push(held.applies
      ? t("settings.typesafe.seatApplying", "근거가 서서 적용 중입니다.")
      : t("settings.typesafe.seatHolding", "아직 기록만 합니다 — {{because}}", { because }));
  }
  return words;
}

/* Which id the seat asked with and which version answered it (t-6187): the
 * pin or the alias beside the `model` the newest answer named. Empty for a
 * seat nothing answered, or from a zo older than the pin. The card and the
 * dashboard both read it here, so the two say it the same way. */
function jevModelWords(held) {
  if (!held.model || !held.askedModel) return "";
  return t("settings.typesafe.seatVersion", "물은 {{asked}} · 답한 {{model}}",
    { asked: held.askedModel, model: held.model });
}

/* The version a change of version cut away from the judged window, when one
 * did — the reason a thin window gives for itself. */
function jevCutWords(held) {
  if (!held.verdict?.cutModel) return "";
  return t("settings.typesafe.seatCut", "{{cut}} 행은 창에서 뺌", { cut: held.verdict.cutModel });
}

/* The card's version line: the models, the rows the judged window holds and
 * the version cut away. Before the verdict's words, which stay last. */
function jevVersionWords(held) {
  const models = jevModelWords(held);
  if (!models) return "";
  const words = [models];
  if (held.judged) {
    words.push(t("settings.typesafe.seatWindowRows", "창 안 {{rows}}행",
      { rows: String(held.judged.window.rows) }));
  }
  const cut = jevCutWords(held);
  if (cut) words.push(cut);
  return words.join(" · ");
}

/* The one line under a seat's switch, and the same line in the dashboard's
 * row: built here for both, so a seat added to the table gets its numbers
 * with it rather than an eighth copy of the same markup. `line` is whatever
 * holds the seat — the card's row or the dashboard's cell. */
function paintJevNumbers(line, seat) {
  const held = (jevNumbers ?? []).find((row) => row.id === seat) ?? null;
  let said = line.querySelector("[data-jev-numbers]");
  if (!held) {
    // Nothing counted: no line at all rather than an empty one, so a row with
    // no numbers reads as a row with no numbers and not as a silent zero.
    said?.remove();
    return;
  }
  if (!said) {
    said = document.createElement("p");
    said.className = "settings-row-desc settings-jev-numbers";
    said.setAttribute("data-jev-numbers", "");
    line.append(said);
  }
  said.textContent = jevSeatWords(held).join(" · ");
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

function jevNode(tag, className, ...children) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  node.append(...children);
  return node;
}

/* The dashboard's columns, in order: the key each head is read under and
 * the attribute the cell is found by. One table, so the head and the row
 * cannot disagree about what stands where. */
const JEV_COLUMNS = Object.freeze([
  { cell: "seat", key: "jev.col.seat", word: "자리" },
  { cell: "mode", key: "jev.col.mode", word: "모드" },
  { cell: "rows", key: "jev.col.rows", word: "행 (오늘 · 7일) · 거절" },
  { cell: "answered", key: "jev.col.answered", word: "답률 (95% 하한)" },
  { cell: "latency", key: "jev.col.latency", word: "p50 / p95 ms" },
  { cell: "acted", key: "jev.col.acted", word: "적용 · 일치" },
  { cell: "cost", key: "jev.col.cost", word: "비용 (USD)" },
  { cell: "why", key: "jev.col.why", word: "오르지 못하는 이유" },
  { cell: "trend", key: "jev.col.trend", word: "7일 추이 (답률 · 일치율 · p50)" },
]);

/* The three lines a seat's trend cell draws, each read off one day's count:
 * the share that answered, the share that agreed, and the p50. */
const JEV_TRENDS = Object.freeze([
  { line: "answered", key: "jev.trend.answered", word: "답률", read: (day) => day.tally.answeredShare ?? null, unit: "share" },
  { line: "agreement", key: "jev.trend.agreement", word: "일치율",
    read: (day) => (day.agreement?.compared ? day.agreement.agreed / day.agreement.compared : null), unit: "share" },
  { line: "p50", key: "jev.trend.p50", word: "p50", read: (day) => day.tally.p50Ms ?? null, unit: "ms" },
]);

function buildJevView() {
  const root = document.createElement("section");
  root.className = "file-view jev-view";
  // The first leaf's copy answers to this name, exactly as the surfaces the
  // markup declares do — `docHost` strips it from every clone after.
  root.id = "jev-view";
  root.hidden = true;
  root.dataset.i18nAria = "jev.title";
  root.setAttribute("aria-label", t("jev.title", "Jev 대시보드"));

  // The eyebrow is a name, the same in every language, so it takes no key.
  const heading = jevNode("div", "jev-heading",
    jevNode("p", "jev-eyebrow", document.createTextNode("TYPESAFE · JEV")),
    jevText("jev.title", "Jev 대시보드", "h1", "jev-title"),
    jevText("jev.about", "자리마다 무엇을 얼마나 물었고, 얼마나 맞았고, 왜 아직 오르지 못하는지를 원장 그대로 봅니다.", "p", "jev-about"));
  const freshness = jevNode("span", "jev-freshness");
  freshness.setAttribute("role", "status");
  const refresh = jevText("jev.refresh", "새로 고침", "button", "btn jev-refresh");
  refresh.type = "button";
  const settings = jevText("jev.openSettings", "설정에서 자세히", "button", "btn jev-settings-open");
  settings.type = "button";
  root.append(jevNode("header", "jev-head", heading, jevNode("div", "jev-actions", freshness, refresh, settings)));

  const status = jevNode("p", "jev-status");
  status.setAttribute("role", "status");
  status.hidden = true;
  const error = jevNode("p", "jev-error");
  error.setAttribute("role", "alert");
  error.hidden = true;
  root.append(status, error);

  const head = document.createElement("tr");
  for (const column of JEV_COLUMNS) {
    const cell = jevText(column.key, column.word, "th", `jev-col-${column.cell}`);
    cell.scope = "col";
    head.append(cell);
  }
  const body = document.createElement("tbody");
  body.dataset.jevRows = "";
  const table = jevNode("table", "jev-table", jevNode("thead", "", head), body);
  root.append(jevNode("div", "jev-table-wrap", table));

  const picker = document.createElement("select");
  picker.className = "settings-input jev-recent-seat";
  picker.dataset.i18nAria = "jev.recent.seat";
  picker.setAttribute("aria-label", t("jev.recent.seat", "자리 고르기"));
  const list = document.createElement("ol");
  list.className = "jev-recent-list";
  const empty = jevText("jev.recent.empty", "이 자리에는 아직 결정이 없습니다.", "p", "jev-recent-empty");
  empty.hidden = true;
  root.append(jevNode("section", "jev-recent",
    jevNode("header", "jev-recent-head", jevText("jev.recent.title", "최근 결정", "h2", "jev-recent-title"), picker),
    list, empty));
  return root;
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
    setSettingsOpen(true, "typesafe-routing-select");
  });
  host.querySelector(".jev-recent-seat").addEventListener("change", (event) => {
    jevRecentSeat = event.target.value;
    paintJevViews();
  });
  host.querySelector("[data-jev-rows]").addEventListener("change", (event) => {
    const select = event.target.closest("[data-jev-dash-seat]");
    if (!select) return;
    const seat = select.dataset.jevDashSeat;
    void moveJevSeat(seat, select.value, (state) => {
      const words = jevSwitchSaid(state, seat);
      jevSayStatus(words);
      return words;
    });
  });
}

/* What the dashboard says under its head: the last switch moved, until the
 * next paint of the numbers replaces it with the freshness line. */
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

/* The seat's name is the settings card's: the label above its switch, read
 * by the key it is written under, so the dashboard names a seat exactly as
 * the card does and carries no second list of names. */
function jevSeatName(id) {
  const label = document.querySelector(`[data-jev-seat="${id}"]`)
    ?.closest("[data-jev-row]")?.querySelector(".settings-label");
  if (!label) return id;
  return t(label.dataset.i18n, label.dataset.i18nSource ?? label.textContent);
}

function jevPercent(share) {
  if (share === null || share === undefined) return "—";
  return `${Math.round(share * 100)}%`;
}

function jevMs(ms) {
  return ms === null || ms === undefined ? "—" : String(ms);
}

/* A week of judgments costs cents: four places under a dollar, two above. */
function jevCost(usd) {
  if (usd === null || usd === undefined) return "—";
  return `$${usd.toFixed(usd < 1 ? 4 : 2)}`;
}

/* One polyline over the week's days, the newest at the right; a day with
 * nothing to say leaves a gap rather than a zero. The values ride on the
 * node for a reader or a test; the picture is the stylesheet's. */
function jevSpark(values, unit) {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("class", "jev-spark");
  svg.setAttribute("viewBox", `0 0 ${JEV_SPARK_WIDTH} ${JEV_SPARK_HEIGHT}`);
  svg.setAttribute("aria-hidden", "true");
  const known = values.filter((value) => value !== null);
  const top = unit === "share" ? 1 : Math.max(1, ...known);
  const step = values.length > 1 ? JEV_SPARK_WIDTH / (values.length - 1) : 0;
  const pad = 2;
  const points = [];
  values.forEach((value, at) => {
    if (value === null) return;
    const x = at * step;
    const y = JEV_SPARK_HEIGHT - pad - (value / top) * (JEV_SPARK_HEIGHT - pad * 2);
    points.push([x, y]);
  });
  if (points.length > 1) {
    const line = document.createElementNS("http://www.w3.org/2000/svg", "polyline");
    line.setAttribute("class", "jev-spark-line");
    line.setAttribute("points", points.map(([x, y]) => `${x.toFixed(1)},${y.toFixed(1)}`).join(" "));
    svg.append(line);
  }
  for (const [x, y] of points) {
    const dot = document.createElementNS("http://www.w3.org/2000/svg", "circle");
    dot.setAttribute("class", "jev-spark-dot");
    dot.setAttribute("cx", x.toFixed(1));
    dot.setAttribute("cy", y.toFixed(1));
    dot.setAttribute("r", "1.5");
    svg.append(dot);
  }
  svg.dataset.points = String(points.length);
  svg.dataset.values = values.map((value) => (value === null ? "" : String(value))).join(",");
  return svg;
}

function jevTrendCell(held) {
  const cell = jevNode("div", "jev-trends");
  for (const trend of JEV_TRENDS) {
    const values = (held.days ?? []).map((day) => trend.read(day));
    const last = values.filter((value) => value !== null).at(-1);
    const face = jevNode("span", "jev-trend-last");
    face.textContent = last === undefined ? "—" : (trend.unit === "share" ? jevPercent(last) : jevMs(last));
    // The line's name is the column head's and a reader's (`sr`); the cell
    // itself is three small lines with the latest value under each.
    const row = jevNode("div", `jev-trend jev-trend-${trend.line}`,
      jevText(trend.key, trend.word, "span", "sr"), jevSpark(values, trend.unit), face);
    cell.append(row);
  }
  return cell;
}

/* Why the seat is not acting, or that it is: the judge's line in words, the
 * window it was read on, and the rows still owed before the next reading.
 *
 * A seat a person switched to acting is acting by hand, whatever the judge
 * says — the card's line says "applying" of it, and this column says whose
 * doing that is and what the judge would have said: the one place a person
 * can see that their `on` is carrying a seat the evidence would not. `byHand`
 * is the switch's own word for its mode (a mode that applies unconditionally),
 * not a stand token respelled here. */
function jevWhyCell(held, byHand) {
  const cell = jevNode("div", "jev-why");
  const words = jevSeatWords(held);
  const verdict = jevNode("p", "jev-why-verdict");
  const because = held.verdict ? jevLineWords(held.verdict.line) : "";
  if (held.week.rows === 0) verdict.textContent = words[0];
  else if (held.applies && byHand && held.verdict) {
    verdict.textContent = t("jev.byHand", "직접 켜서 적용 중 — 판정은 「{{because}}」", { because });
  } else if (held.applies && byHand) {
    verdict.textContent = t("jev.byHandNoJudge", "직접 켜서 적용 중 — 이 자리는 승격하지 않습니다");
  } else if (!held.applies && !held.verdict) {
    verdict.textContent = t("jev.neverRises", "이 자리는 승격하지 않습니다 — 기록만");
  } else verdict.textContent = words.at(-1) ?? "";
  cell.append(verdict);
  if (held.week.rows === 0) return cell;
  const facts = [];
  const models = jevModelWords(held);
  if (models) facts.push(models);
  if (held.judged) {
    facts.push(t("jev.window", "창 {{rows}}/{{wanted}}행", {
      rows: String(held.judged.window.rows), wanted: String(held.judged.windowWanted) }));
  }
  const cut = jevCutWords(held);
  if (cut) facts.push(cut);
  if (held.clearsRiseFloor !== null && held.clearsRiseFloor !== undefined) {
    facts.push(held.clearsRiseFloor
      ? t("jev.clearsFloor", "답률 하한이 문턱({{floor}}‰)을 넘음", { floor: String(held.riseFloorPermille) })
      : t("jev.underFloor", "답률 하한이 문턱({{floor}}‰) 아래", { floor: String(held.riseFloorPermille) }));
  }
  if (held.rowsToNextJudgment !== null && held.rowsToNextJudgment !== undefined) {
    facts.push(t("jev.rowsToJudgment", "다음 판정까지 {{rows}}행", { rows: String(held.rowsToNextJudgment) }));
  }
  if (facts.length > 0) {
    const line = jevNode("p", "jev-why-facts");
    line.textContent = facts.join(" · ");
    cell.append(line);
  }
  return cell;
}

function jevRefusalWords(window) {
  const refusals = window.refusals ?? [];
  if (refusals.length === 0) return window.refused > 0 ? String(window.refused) : "—";
  return refusals.map((one) => `${one.token} ${one.rows}`).join(" · ");
}

function jevAgreementWords(held) {
  const agreement = held.judged?.agreement;
  if (!agreement || !agreement.compared) return "—";
  return `${agreement.agreed}/${agreement.compared} (${jevPercent(agreement.lowerBound)})`;
}

/* Draw the table and the recent list from what is in hand: the switches
 * from the settings answer, the numbers from zo's, in the use table's
 * order. Rows are keyed by seat and updated in place, so a repaint never
 * moves a switch somebody is using. */
function paintJevView(view) {
  wireJevView(view);
  const switches = typesafeState?.switches ?? [];
  const numbers = jevNumbers ?? [];
  const order = switches.length > 0 ? switches.map((row) => row.id) : numbers.map((row) => row.id);
  const body = view.querySelector("[data-jev-rows]");
  const seen = new Set();
  for (const id of order) {
    seen.add(id);
    let row = body.querySelector(`[data-jev-dash-row="${id}"]`);
    if (!row) {
      row = document.createElement("tr");
      row.dataset.jevDashRow = id;
      for (const column of JEV_COLUMNS) {
        const cell = document.createElement("td");
        cell.dataset.jevCell = column.cell;
        if (column.cell === "mode") {
          const select = document.createElement("select");
          select.className = "settings-input jev-mode";
          select.dataset.jevDashSeat = id;
          cell.append(select);
        }
        row.append(cell);
      }
      body.append(row);
    }
    const cell = (name) => row.querySelector(`[data-jev-cell="${name}"]`);
    cell("seat").textContent = jevSeatName(id);
    const held = numbers.find((one) => one.id === id) ?? null;
    const standing = switches.find((one) => one.id === id) ?? null;
    const select = cell("mode").querySelector("select");
    if (standing) {
      paintJevModes(select, standing.modes ?? []);
      select.value = standing.mode;
      select.disabled = typesafeBusy;
      select.hidden = false;
      select.setAttribute("aria-label", `${t("jev.col.mode", "모드")} · ${jevSeatName(id)}`);
    } else {
      select.hidden = true;
    }
    const byHand = standing ? Boolean(jevSeatChoice(typesafeState, id)?.applies) : false;
    const applying = held ? held.applies : byHand;
    row.classList.toggle("is-applying", Boolean(applying));
    row.classList.toggle("is-quiet", !held || held.week.rows === 0);
    if (!held) {
      for (const column of JEV_COLUMNS) {
        if (column.cell === "seat" || column.cell === "mode") continue;
        cell(column.cell).textContent = "—";
      }
      continue;
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
      fact("today", String(held.today.rows), "jev-fact-strong"), document.createTextNode(" · "),
      fact("week", String(held.week.rows)));
    if (held.week.refused > 0) {
      rows.append(document.createElement("br"), fact("refusals", jevRefusalWords(held.week), "jev-fact-mist"));
    }
    const share = cell("answered");
    share.replaceChildren();
    if (held.week.rows === 0) {
      share.textContent = "—";
    } else {
      share.append(fact("share", jevPercent(held.week.answeredShare), "jev-share"), document.createTextNode(" "),
        fact("bound", t("jev.bound", "하한 {{pct}}", { pct: jevPercent(held.week.answeredLowerBound) }), "jev-bound"));
    }
    cell("latency").textContent = `${jevMs(held.week.p50Ms)} / ${jevMs(held.week.p95Ms)}`;
    const acted = cell("acted");
    acted.replaceChildren(
      fact("applied", held.week.rows === 0 ? "—" : String(held.week.applied ?? 0)), document.createTextNode(" · "),
      fact("agreement", jevAgreementWords(held)));
    cell("cost").textContent = jevCost(held.costUsd);
    cell("trend").replaceChildren(jevTrendCell(held));
    cell("why").replaceChildren(jevWhyCell(held, byHand));
  }
  for (const row of body.querySelectorAll("[data-jev-dash-row]")) {
    if (!seen.has(row.dataset.jevDashRow)) row.remove();
  }
  paintJevFreshness(view);
  paintJevRecent(view, order);
}

function paintJevFreshness(view) {
  const line = view.querySelector(".jev-freshness");
  const error = view.querySelector(".jev-error");
  if (jevNumbersError) {
    error.textContent = t("jev.unavailable", "zo가 원장을 세지 못했습니다 — {{error}}", { error: jevNumbersError });
    error.hidden = false;
  } else {
    error.hidden = true;
    error.textContent = "";
  }
  if (jevNumbersAt === 0) {
    line.textContent = t("jev.loading", "원장을 세는 중…");
    return;
  }
  // "just now" is not a distance, so it does not take "ago".
  const ago = agoWord(jevNumbersAt, Date.now());
  const ms = String(jevNumbersCostMs ?? 0);
  line.textContent = ago === t("board.justNow", "방금")
    ? t("jev.freshNow", "방금 갱신 · zo {{ms}} ms", { ms })
    : t("jev.fresh", "{{ago}} 전 갱신 · zo {{ms}} ms", { ago, ms });
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
  const outcome = jevNode("span", "jev-decision-outcome");
  outcome.textContent = decision.outcome === "answered" ? t("jev.answered", "답함") : decision.outcome;
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
    marks.push(t("jev.confidence", "확신 {{pct}}", { pct: jevPercent(decision.confidence) }));
  }
  if (decision.applied === true) marks.push(t("jev.applied", "적용"));
  else if (decision.applied === false) marks.push(t("jev.recorded", "기록만"));
  if (decision.agreed === true) marks.push(t("jev.agreed", "일치"));
  else if (decision.agreed === false) marks.push(t("jev.disagreed", "불일치"));
  if (decision.followed) marks.push(t("jev.followed", "그 뒤 {{word}}", { word: decision.followed }));
  if (decision.elapsedMs !== null && decision.elapsedMs !== undefined) marks.push(`${decision.elapsedMs} ms`);
  const mark = jevNode("span", "jev-decision-marks");
  mark.textContent = marks.join(" · ");
  item.append(when, outcome, asked, answered, mark);
  return item;
}

function paintJevRecent(view, order) {
  const picker = view.querySelector(".jev-recent-seat");
  const numbers = jevNumbers ?? [];
  if (!order.includes(jevRecentSeat)) {
    // The first seat with anything to list, else the first seat there is.
    jevRecentSeat = order.find((id) => numbers.find((one) => one.id === id)?.recent?.length) ?? order[0] ?? null;
  }
  const options = order.map((id) => ({ id, name: jevSeatName(id) }));
  if ([...picker.options].map((option) => option.value).join("\u0000") !== order.join("\u0000")) {
    picker.replaceChildren(...options.map(({ id }) => {
      const option = document.createElement("option");
      option.value = id;
      return option;
    }));
  }
  for (const option of picker.options) {
    option.textContent = options.find((one) => one.id === option.value)?.name ?? option.value;
  }
  if (jevRecentSeat) picker.value = jevRecentSeat;
  const held = numbers.find((one) => one.id === jevRecentSeat) ?? null;
  const list = view.querySelector(".jev-recent-list");
  const empty = view.querySelector(".jev-recent-empty");
  const decisions = held?.recent ?? [];
  const now = Date.now();
  list.replaceChildren(...decisions.map((decision) => jevDecisionNode(decision, now)));
  empty.hidden = decisions.length > 0;
}
