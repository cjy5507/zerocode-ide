/* ---- 지식 그래프의 라이브 층 (t-2931) -------------------------------------
 *
 * 그림이 사진이 아니라 지금인가를 잰다. 네 절, 네 단언:
 *
 *  1. LIVE REFRESH — 워처 레인의 `second-brain:changed`가 바닥(1.5초) 안에 한
 *     번만 다시 읽고, 새 점·새 선이 한 번 맥동한다(움직임을 줄인 판에서는 맥동
 *     없이 낱말만). 판이 서 있지 않으면 묻지 않는다.
 *  2. RECALL TRACE — 창 안에서 회상된 페이지가 활동 고리를 두르고, 인스펙터의
 *     BUS LOG 띠가 최신순·상한만큼 선다. `live`가 없는 옛 답도 여전히 그려진다.
 *  3. ACTIVITY LENS — 「살아 있는 것만」이 창 안에서 회상되거나 바뀐 페이지만
 *     남기고, 건강 카드가 「오늘 회상」·「회상된 적 없음」을 센다.
 *  4. DEDUPE LENS — 백엔드가 준 merge 후보 쌍이 점선 `merge?` 선으로 서고,
 *     건강 카드에 한 줄, 범례에 한 줄. JS는 쌍을 다시 계산하지 않는다.
 *
 * 수치는 전부 `live.limits`(Rust의 한 표)에서 읽는다 — 이 파일도 창도 자기
 * 숫자를 갖지 않는다. */

const NOW = Date.now();
const MINUTE = 60_000;
const HOUR = 3_600_000;

const LIVE_LIMITS = {
  activity_window_minutes: 30,
  today_hours: 24,
  trace_lines_max: 2000,
  trace_rotate_bytes: 1048576,
  trace_read_bytes_max: 524288,
  trace_pages_per_line_max: 8,
  bus_rows_max: 4,
  bus_note_chars: 120,
  log_tail_lines: 20,
  log_tail_bytes_max: 65536,
  merge_overlap_min_percent: 60,
  merge_title_tokens_min: 2,
  merge_token_pages_max: 64,
  merge_pairs_max: 64,
  watch_dirs_max: 64,
  watch_files_max: 64,
};

const emptyLive = () => ({ now_ms: NOW, recalled: [], bus: [], merge: [], limits: LIVE_LIMITS });

const NEW_KEYS = [
  "knowledge.aliveOnly", "knowledge.aliveShort", "knowledge.mergeOnly", "knowledge.mergeShort",
  "knowledge.busLog", "knowledge.busCreated", "knowledge.busUpdated", "knowledge.busLinked",
  "knowledge.busRecalled", "knowledge.busIngested", "knowledge.lintRecalledToday",
  "knowledge.lintNeverRecalled", "knowledge.lintMerge", "knowledge.nodeRecalled",
  "knowledge.edgeMerge",
];

export async function testKnowledgeLive(page, ok) {
  /* 판을 처음부터: 열두 쪽, 빈 라이브 층, 탭 열림. 앞 절이 남긴 상태(검색어·렌즈·
     선택)는 여기서 놓는다. */
  await page.evaluate(async ({ now, limits }) => {
    knowledgeQuery = "";
    knowledgeOrphansOnly = false;
    knowledgeGhostsOnly = false;
    knowledgeTypedOnly = false;
    knowledgeShowSources = false;
    knowledgeTagsPicked.clear();
    knowledgeSelectedKey = null;
    window.__VAULT__ = {
      pages: 12, linksPer: 2, ghosts: 3, tags: ["core", "reading"],
      live: { now_ms: now, recalled: [], bus: [], merge: [], limits },
    };
    secondBrainVault = "/vault";
    paintKnowledgeEntry();
    dropTab("knowledge");
    el("nav-knowledge").click();
    await new Promise((done) => setTimeout(done, 400));
  }, { now: NOW, limits: LIVE_LIMITS });

  /* ---- 1. LIVE REFRESH ---- */
  const refresh = await page.evaluate(async () => {
    const viewOf = () => [...document.querySelectorAll(".file-view")]
      .find((one) => one.classList.contains("knowledge-view") && !one.hidden);
    const fire = () => {
      for (const listener of window.__LISTENERS__["second-brain:changed"] ?? []) {
        listener({ payload: { events: [{ kind: "update", path: "wiki/" }] } });
      }
    };
    const wired = (window.__LISTENERS__["second-brain:changed"] ?? []).length;
    /* 바닥은 이 절이 스스로 연다 — 탭을 열 때와 같은 강제 읽기로. 앞 evaluate가
       연 질의에 기대면 그것과 이 알림 사이가 부하로 벌어질 때 알림이 바닥 밖에
       떨어져, 세 번이 한 번으로 모이는지를 묻지 못한다(사이 1.1 s에서 asksAtOnce 2). */
    await refreshKnowledgeGraph({ force: true });
    const asksBefore = window.__GRAPH_ASKS__;
    const pulsesBefore = knowledgePulses;
    /* 볼트가 한 쪽 자란다 — 워처가 들은 변화가 이것이다. 세 번 울려도 바닥 안이면
       한 번만 다시 읽어야 한다. */
    window.__VAULT__ = { ...window.__VAULT__, pages: 13 };
    const freshNode = () => viewOf()?.querySelector('.knowledge-node[data-graph-key="wiki/Page-0012.md"]');
    fire();
    fire();
    fire();
    const windowEnds = performance.now() + 2060;
    await new Promise((done) => setTimeout(done, 60));
    const asksAtOnce = window.__GRAPH_ASKS__;
    /* 맥동은 그것이 서는 프레임에서 센다. 다시 읽기는 바닥을 연 질의에 매여 바닥
     * 끝에 오는데, 이 알림 뒤의 벽시계로 세면 둘 사이가 부하로 벌어진 만큼 맥동이
     * 일찍 서서 1200 ms 맥동이 측정 전에 끝났다(창 하네스 전체 실행에서 프레임
     * 간격 80~141 ms일 때 animations 0; 통과한 실행의 여유는 3 ms — 맥동 866 ms
     * + 1200 ms 대 측정 2063 ms). 창이 끝날 때까지는 그대로 기다린다: 바닥 안의
     * 세 번이 다시 읽기 한 번인지를 본다. */
    const pulsedNode = await new Promise((done) => {
      const look = () => {
        if (knowledgePulses > pulsesBefore) done(freshNode() ?? null);
        else if (performance.now() >= windowEnds) done(null);
        else requestAnimationFrame(look);
      };
      look();
    });
    const animations = pulsedNode?.getAnimations?.().length ?? 0;
    await new Promise((done) => setTimeout(done, Math.max(0, windowEnds - performance.now())));
    const asksAfter = window.__GRAPH_ASKS__;
    const fresh = freshNode();
    const freshKeys = [...knowledgeFreshKeys];
    const pulses = knowledgePulses - pulsesBefore;
    // 맥동한 점이 창 끝까지 그 점이다 — 다시 그리기가 갈아 끼운 점은 맥동을 잃는다.
    const pulsedStays = pulsedNode !== null && pulsedNode === fresh;
    /* 서 있지 않은 판은 묻지 않는다 — 다음에 열릴 때 다시 읽을 뿐. */
    dropTab("knowledge");
    fire();
    await new Promise((done) => setTimeout(done, 1700));
    const asksHidden = window.__GRAPH_ASKS__;
    el("nav-knowledge").click();
    await new Promise((done) => setTimeout(done, 400));
    return { wired, asksBefore, asksAtOnce, asksAfter, fresh: fresh !== null && fresh !== undefined,
      freshKeys, pulses, animations, pulsedStays, asksHidden, nodes: viewOf().querySelectorAll(".knowledge-node").length };
  });
  ok(
    "a vault change heard on the watcher lane re-reads the graph once within the floor and pulses what is new",
    refresh.wired === 1 &&
      refresh.asksAtOnce === refresh.asksBefore &&
      refresh.asksAfter === refresh.asksBefore + 1 &&
      refresh.fresh &&
      refresh.freshKeys.includes("wiki/Page-0012.md") &&
      refresh.pulses > 0 &&
      refresh.animations > 0 &&
      refresh.pulsedStays &&
      refresh.asksHidden === refresh.asksAfter &&
      // 열세 쪽과 유령 셋: 다시 연 판이 자란 볼트를 그린다.
      refresh.nodes === 16,
    JSON.stringify(refresh),
  );

  /* 통째로 바뀐 볼트는 맥동하지 않는다 — 천 개의 애니메이션이 한 프레임을 먹는
     대신 새 위상의 도착으로 스며든다(실측 540ms). 낱말은 그대로 남는다. */
  const wholesale = await page.evaluate(async () => {
    const pulsesBefore = knowledgePulses;
    window.__VAULT__ = { ...window.__VAULT__, pages: 200 };
    knowledgeAskedAt = 0;
    for (const listener of window.__LISTENERS__["second-brain:changed"] ?? []) {
      listener({ payload: { events: [{ kind: "create", path: "wiki/" }] } });
    }
    await new Promise((done) => setTimeout(done, 500));
    const fresh = knowledgeFreshKeys.size;
    const pulses = knowledgePulses - pulsesBefore;
    /* 그리고 열세 쪽으로 돌아온다 — 그 작은 차이는 다시 맥동해도 된다. */
    window.__VAULT__ = { ...window.__VAULT__, pages: 13 };
    knowledgeAskedAt = 0;
    for (const listener of window.__LISTENERS__["second-brain:changed"] ?? []) {
      listener({ payload: { events: [{ kind: "update", path: "wiki/" }] } });
    }
    await new Promise((done) => setTimeout(done, 500));
    return { fresh, pulses };
  });
  ok(
    "a wholesale change arrives as a new picture and pulses nothing",
    wholesale.fresh > 64 && wholesale.pulses === 0,
    JSON.stringify(wholesale),
  );

  /* 움직임을 줄이라는 판: 새 것은 여전히 낱말로 남지만 맥동은 없다. */
  await page.emulateMedia({ reducedMotion: "reduce" });
  const stillRefresh = await page.evaluate(async () => {
    const viewOf = () => [...document.querySelectorAll(".file-view")]
      .find((one) => one.classList.contains("knowledge-view") && !one.hidden);
    const pulsesBefore = knowledgePulses;
    window.__VAULT__ = { ...window.__VAULT__, pages: 14 };
    knowledgeAskedAt = 0;
    for (const listener of window.__LISTENERS__["second-brain:changed"] ?? []) {
      listener({ payload: { events: [{ kind: "create", path: "wiki/" }] } });
    }
    await new Promise((done) => setTimeout(done, 400));
    const fresh = viewOf().querySelector('.knowledge-node[data-graph-key="wiki/Page-0013.md"]');
    return {
      fresh: fresh !== null && fresh !== undefined,
      keys: [...knowledgeFreshKeys].includes("wiki/Page-0013.md"),
      pulses: knowledgePulses - pulsesBefore,
      animations: fresh?.getAnimations?.().length ?? 0,
    };
  });
  await page.emulateMedia({ reducedMotion: "no-preference" });
  ok(
    "under reduced motion a change is still named but nothing pulses",
    stillRefresh.fresh && stillRefresh.keys && stillRefresh.pulses === 0 && stillRefresh.animations === 0,
    JSON.stringify(stillRefresh),
  );

  /* ---- 2. RECALL TRACE ---- */
  const trace = await page.evaluate(async ({ now, minute, hour, limits, keys }) => {
    const viewOf = () => [...document.querySelectorAll(".file-view")]
      .find((one) => one.classList.contains("knowledge-view") && !one.hidden);
    window.__VAULT__ = {
      pages: 12, linksPer: 2, ghosts: 3, tags: ["core", "reading"],
      live: {
        now_ms: now,
        recalled: [
          { id: "wiki/Page-0001.md", at_ms: now - minute, count: 2 },
          { id: "wiki/Page-0002.md", at_ms: now - 3 * hour, count: 1 },
        ],
        bus: [
          { at_ms: now - 1000, kind: "recalled", page: "wiki/Page-0001.md", note: "term-3" },
          { at_ms: now - 2000, kind: "created", page: "wiki/Page-0005.md", note: "개념 5" },
          { at_ms: now - 3000, kind: "ingested", page: "wiki/Page-0007.md", note: "취합 → [[Page-0007]]" },
          { at_ms: now - 500, kind: "linked", page: "wiki/Page-0002.md", note: "개념 2 → 개념 3" },
          { at_ms: now - 4000, kind: "updated", page: null, note: "사라진 페이지" },
        ],
        merge: [],
        limits,
      },
    };
    knowledgeAskedAt = 0;
    viewOf().querySelector(".knowledge-refresh").click();
    await new Promise((done) => setTimeout(done, 400));
    const view = viewOf();
    const ringed = [...view.querySelectorAll(".knowledge-node.is-recalled")].map((one) => one.dataset.graphKey);
    const halo = view.querySelector(".knowledge-node.is-recalled .knowledge-halo");
    /* 값은 지금 읽는다 — `getComputedStyle`은 살아 있는 객체라, 아래의 옛 답
       라운드가 지나간 뒤에 읽으면 그 라운드의 옷을 읽는다. */
    const haloStyle = halo ? getComputedStyle(halo) : null;
    const haloDisplay = haloStyle?.display ?? "";
    const haloStroke = haloStyle?.stroke ?? "";
    const haloFill = haloStyle?.fill ?? "";
    const strip = view.querySelector(".knowledge-overview-bus");
    const stripHidden = strip?.hidden ?? null;
    const rows = [...(strip?.querySelectorAll(".knowledge-bus-row") ?? [])];
    const kinds = rows.map((row) => row.dataset.busKind);
    const notes = rows.map((row) => row.querySelector(".knowledge-inspector-note")?.textContent ?? "");
    const words = rows.map((row) => row.querySelector(".knowledge-inspector-row")?.textContent ?? "");
    /* 줄을 누르면 그 점이 골라진다 — 목록의 다른 줄들과 같은 손. */
    rows[0]?.querySelector("[data-knowledge-key]")?.click();
    await new Promise((done) => setTimeout(done, 60));
    const selected = view.querySelector(".knowledge-node.is-selected")?.dataset.graphKey ?? null;
    selectKnowledgeNode(view, null);
    const catalogs = ["en", "ja", "zh", "es"].map((code) => [
      code, keys.filter((key) => typeof CATALOG[code]?.[key] !== "string" || CATALOG[code][key] === ""),
    ]).filter(([, missing]) => missing.length > 0);
    /* 그리고 `live`가 없는 옛 답. 고리도 띠도 없이 그림만 선다. */
    window.__VAULT__ = { pages: 12, linksPer: 2, ghosts: 3, tags: ["core", "reading"] };
    knowledgeAskedAt = 0;
    viewOf().querySelector(".knowledge-refresh").click();
    await new Promise((done) => setTimeout(done, 400));
    const old = {
      nodes: viewOf().querySelectorAll(".knowledge-node").length,
      ringed: viewOf().querySelectorAll(".knowledge-node.is-recalled").length,
      stripHidden: viewOf().querySelector(".knowledge-overview-bus")?.hidden ?? null,
      error: viewOf().querySelector(".knowledge-error").hidden,
    };
    return {
      ringed, haloDisplay, haloStroke, haloFill, stripHidden, kinds, notes, words, selected, catalogs, old,
      recalledWord: t("knowledge.busRecalled", "회상"),
    };
  }, { now: NOW, minute: MINUTE, hour: HOUR, limits: LIVE_LIMITS, keys: NEW_KEYS });
  ok(
    "a page recalled within the window wears an activity ring and the inspector's bus log stands newest first, bounded",
    trace.ringed.length === 1 &&
      trace.ringed[0] === "wiki/Page-0001.md" &&
      trace.haloDisplay !== "none" &&
      trace.haloStroke !== "none" &&
      trace.haloFill === "none" &&
      trace.stripHidden === false &&
      // 다섯 줄을 줬고 표의 상한은 넷: 최신순으로 넷.
      trace.kinds.length === 4 &&
      trace.kinds.join(",") === "linked,recalled,created,ingested" &&
      trace.notes[1].includes(trace.recalledWord) &&
      trace.words[0].includes("개념 2") &&
      trace.selected === "wiki/Page-0002.md" &&
      trace.catalogs.length === 0 &&
      trace.old.nodes === 15 &&
      trace.old.ringed === 0 &&
      trace.old.stripHidden === true &&
      trace.old.error,
    JSON.stringify(trace),
  );

  /* ---- 3. ACTIVITY LENS ---- */
  const alive = await page.evaluate(async ({ now, minute, hour, limits }) => {
    const viewOf = () => [...document.querySelectorAll(".file-view")]
      .find((one) => one.classList.contains("knowledge-view") && !one.hidden);
    const count = () => viewOf().querySelectorAll(".knowledge-node").length;
    const flag = (name) => viewOf().querySelector(`[data-knowledge-flag="${name}"]`);
    const lintRow = (name) => viewOf().querySelector(`.knowledge-health-row[data-knowledge-lint="${name}"]`);
    window.__VAULT__ = {
      pages: 12, linksPer: 2, ghosts: 3, tags: ["core", "reading"],
      // 4번은 오 분 전에 고쳐졌다: 회상되지 않았어도 살아 있다.
      modified: { 4: now - 5 * minute },
      live: {
        now_ms: now,
        recalled: [
          { id: "wiki/Page-0001.md", at_ms: now - minute, count: 2 },
          { id: "wiki/Page-0002.md", at_ms: now - 3 * hour, count: 1 },
        ],
        bus: [],
        merge: [],
        limits,
      },
    };
    knowledgeAskedAt = 0;
    viewOf().querySelector(".knowledge-refresh").click();
    await new Promise((done) => setTimeout(done, 400));
    const asks = window.__GRAPH_ASKS__;
    const all = count();
    flag("alive").click();
    await new Promise((done) => setTimeout(done, 80));
    const shown = [...viewOf().querySelectorAll(".knowledge-node")].map((one) => one.dataset.graphKey).sort();
    const pressed = flag("alive").getAttribute("aria-pressed");
    const stat = viewOf().querySelector(".knowledge-stat").textContent;
    const edgesWhileAlive = viewOf().querySelectorAll(".knowledge-edge").length;
    flag("alive").click();
    await new Promise((done) => setTimeout(done, 80));
    const restored = count();
    const health = {
      recalledToday: lintRow("recalledToday")?.querySelector(".knowledge-inspector-note")?.textContent ?? null,
      neverRecalled: lintRow("neverRecalled")?.querySelector(".knowledge-inspector-note")?.textContent ?? null,
      todayWord: lintRow("recalledToday")?.querySelector("button")?.textContent ?? "",
      neverWord: lintRow("neverRecalled")?.querySelector("button")?.textContent ?? "",
      todayClean: lintRow("recalledToday")?.classList.contains("is-clean") ?? null,
    };
    /* 건강 카드의 줄이 렌즈를 켠다 — 고아·유령 줄과 같은 손. */
    lintRow("recalledToday")?.querySelector("button")?.click();
    await new Promise((done) => setTimeout(done, 80));
    const viaRow = { nodes: count(), pressed: flag("alive").getAttribute("aria-pressed"),
      rowActive: lintRow("recalledToday")?.querySelector("button")?.classList.contains("is-active") };
    lintRow("recalledToday")?.querySelector("button")?.click();
    await new Promise((done) => setTimeout(done, 80));
    /* 「회상된 적 없음」 줄은 그 페이지들만 남긴다. */
    lintRow("neverRecalled")?.querySelector("button")?.click();
    await new Promise((done) => setTimeout(done, 80));
    const cold = count();
    lintRow("neverRecalled")?.querySelector("button")?.click();
    await new Promise((done) => setTimeout(done, 80));
    return { asks, all, shown, pressed, stat, edgesWhileAlive, restored, health, viaRow, cold,
      asksAfter: window.__GRAPH_ASKS__, finally: count() };
  }, { now: NOW, minute: MINUTE, hour: HOUR, limits: LIVE_LIMITS });
  ok(
    "the activity lens keeps what was recalled or changed within the window, and the health card counts today's recalls and the never-recalled",
    alive.all === 15 &&
      alive.shown.join(",") === "wiki/Page-0001.md,wiki/Page-0004.md" &&
      alive.pressed === "true" &&
      alive.stat.includes("2/12") &&
      alive.edgesWhileAlive === 0 &&
      alive.restored === 15 &&
      alive.health.recalledToday === "2" &&
      alive.health.neverRecalled === "10" &&
      alive.health.todayWord.length > 0 &&
      alive.health.neverWord.length > 0 &&
      alive.health.todayClean === false &&
      alive.viaRow.nodes === 2 &&
      alive.viaRow.pressed === "true" &&
      alive.viaRow.rowActive === true &&
      alive.cold === 10 &&
      alive.asksAfter === alive.asks &&
      alive.finally === 15,
    JSON.stringify(alive),
  );

  /* ---- 4. DEDUPE LENS ---- */
  const dedupe = await page.evaluate(async ({ now, limits }) => {
    const viewOf = () => [...document.querySelectorAll(".file-view")]
      .find((one) => one.classList.contains("knowledge-view") && !one.hidden);
    const count = () => viewOf().querySelectorAll(".knowledge-node").length;
    const flag = (name) => viewOf().querySelector(`[data-knowledge-flag="${name}"]`);
    const lintRow = (name) => viewOf().querySelector(`.knowledge-health-row[data-knowledge-lint="${name}"]`);
    window.__VAULT__ = {
      pages: 12, linksPer: 2, ghosts: 3, tags: ["core", "reading"],
      live: {
        now_ms: now, recalled: [], bus: [],
        merge: [
          { left: 0, right: 1, reason: "title_overlap", score: 80 },
          { left: 2, right: 3, reason: "same_links", score: 100 },
        ],
        limits,
      },
    };
    knowledgeAskedAt = 0;
    viewOf().querySelector(".knowledge-refresh").click();
    await new Promise((done) => setTimeout(done, 400));
    const asks = window.__GRAPH_ASKS__;
    const before = {
      nodes: count(),
      mergeEdges: viewOf().querySelectorAll(".knowledge-edge.kind-merge").length,
      stat: viewOf().querySelector(".knowledge-stat").textContent,
      links: viewOf().querySelector('[data-knowledge-count="links"]')?.textContent ?? "",
    };
    flag("merge").click();
    await new Promise((done) => setTimeout(done, 80));
    const view = viewOf();
    const mergeEdges = [...view.querySelectorAll(".knowledge-edge.kind-merge")];
    const dash = mergeEdges[0] ? getComputedStyle(mergeEdges[0]).strokeDasharray : "";
    const typedWorn = mergeEdges.filter((line) => line.classList.contains("is-typed")).length;
    const shown = [...view.querySelectorAll(".knowledge-node")].map((one) => one.dataset.graphKey).sort();
    /* 후보 선의 낱말은 밝을 때 선다: 한 끝을 짚으면 「merge?」. */
    const seat = view.querySelector('.knowledge-node[data-graph-key="wiki/Page-0000.md"]');
    litKnowledge(view, knowledgeLayouts.get(view), seat?.dataset.graphKey ?? null);
    const labels = [...view.querySelectorAll(".knowledge-edge-label")].map((one) => one.textContent);
    litKnowledge(view, knowledgeLayouts.get(view), null);
    const health = {
      merge: lintRow("merge")?.querySelector(".knowledge-inspector-note")?.textContent ?? null,
      word: lintRow("merge")?.querySelector("button")?.textContent ?? "",
      active: lintRow("merge")?.querySelector("button")?.classList.contains("is-active") ?? null,
    };
    const legend = view.querySelectorAll('.knowledge-legend [data-edge-kind="merge"]').length;
    /* 고른 점의 카드도 후보를 관계로 말한다. */
    selectKnowledgeNode(view, "wiki/Page-0000.md");
    await new Promise((done) => setTimeout(done, 60));
    const cardRows = [...view.querySelectorAll(".knowledge-relations-outgoing .knowledge-inspector-note")]
      .map((one) => one.textContent);
    selectKnowledgeNode(view, null);
    flag("merge").click();
    await new Promise((done) => setTimeout(done, 80));
    const restored = { nodes: count(), mergeEdges: viewOf().querySelectorAll(".knowledge-edge.kind-merge").length };
    /* 건강 카드의 줄로도 켜진다. */
    lintRow("merge")?.querySelector("button")?.click();
    await new Promise((done) => setTimeout(done, 80));
    const viaRow = { pressed: flag("merge").getAttribute("aria-pressed"), nodes: count() };
    lintRow("merge")?.querySelector("button")?.click();
    await new Promise((done) => setTimeout(done, 80));
    return { asks, before, mergeEdges: mergeEdges.length, dash, typedWorn, shown, labels, health, legend,
      cardRows, restored, viaRow, asksAfter: window.__GRAPH_ASKS__, mergeWord: t("knowledge.edgeMerge", "merge?") };
  }, { now: NOW, limits: LIVE_LIMITS });
  ok(
    "merge candidates from the backend stand as dashed merge? edges under the dedupe lens, with a health row and a legend row",
    dedupe.before.nodes === 15 &&
      dedupe.before.mergeEdges === 0 &&
      dedupe.mergeEdges === 2 &&
      dedupe.dash !== "none" && dedupe.dash !== "" &&
      dedupe.typedWorn === 0 &&
      dedupe.shown.join(",") === "wiki/Page-0000.md,wiki/Page-0001.md,wiki/Page-0002.md,wiki/Page-0003.md" &&
      dedupe.labels.includes(dedupe.mergeWord) &&
      dedupe.health.merge === "2" &&
      dedupe.health.word.length > 0 &&
      dedupe.health.active === true &&
      dedupe.legend === 1 &&
      // 낱말 칸은 관계 키로 시작하고 그 뒤에 쉬운 말이 붙는다(09-16).
      dedupe.cardRows.some((row) => row.startsWith(dedupe.mergeWord)) &&
      dedupe.restored.nodes === 15 &&
      dedupe.restored.mergeEdges === 0 &&
      dedupe.viaRow.pressed === "true" &&
      dedupe.viaRow.nodes === 4 &&
      dedupe.asksAfter === dedupe.asks,
    JSON.stringify(dedupe),
  );

  /* 뒤에 오는 절들을 위해 판을 원래대로. */
  await page.evaluate(async () => {
    window.__VAULT__ = { pages: 12, linksPer: 2, ghosts: 3, tags: ["core", "reading"] };
    knowledgeAliveOnly = false;
    knowledgeColdOnly = false;
    knowledgeMergeOnly = false;
    knowledgeAskedAt = 0;
    dropTab("knowledge");
  });
}
