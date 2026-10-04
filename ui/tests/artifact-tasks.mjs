/* The Artifacts tab read by task (t-36910, stage 2): one line for each task, and a reader that holds
 * everything the task left — its final report, the other attempts, the before-and-after pictures, the
 * evidence with its counts, its pages.
 *
 * The backend here answers as the runtime does: `artifact_tasks` gives the lines (and applies the
 * search to them), `artifact_bundle` gives one task whole, `commit_landings` says whether the commit a
 * task handed in is in the compare ref when its checkout is gone. What is measured is what a person
 * sees at the tab's real width, and then at the two widths where the layout changes.
 *
 * Everything in the fixture is synthetic. Every read of the page is defensive (`?.`, `?? null`): on a
 * tree that has not built the tab the checks fail at their assertions, one line each, and the suite
 * still runs to its end. */
import { openWindowTestPage } from "./window-boot.mjs";
import { REAL_WINDOW, standCatalog } from "./artifact-cards.mjs";

/* The tab's width in the window a person has open (measured in stage 1), and the slack a scrollbar or
 * a later change of the side panels is given. */
const REAL_TAB_WIDTH = 809;
const WIDTH_SLACK = 24;
/* The list beside an open reader, and the widths at which the layout changes. */
const STRIP_WIDTH = 200;
const RAIL_TIER = 1100;
/* A window wide enough for the evidence column, and one too narrow for two panes. */
const WIDE_WINDOW = Object.freeze({ width: 1800, height: 900 });
const NARROW_WINDOW = Object.freeze({ width: 1024, height: 900 });
const TEXT_MIN_PX = 12;
const PRESS_MIN_PX = 28;
/* How many more tasks stand under the four the checks read: enough that a list which built every
 * line would be told apart from one that builds the lines in view. */
const FILLER_TASKS = 60;

const NEW_KEYS = Object.freeze([
  "artifacts.tab.tasks", "artifacts.tasks.list", "artifacts.tasks.empty", "artifacts.tasks.none", "artifacts.tasks.reports",
  "artifacts.tasks.pages", "artifacts.tasks.pictures", "artifacts.tasks.logs", "artifacts.tasks.other",
  "artifacts.tasks.attempts", "artifacts.tasks.files", "artifacts.tasks.sessions", "artifacts.tasks.missing",
  "artifacts.tasks.when", "artifacts.tasks.back", "artifacts.status.intended", "artifacts.status.unknown",
  "artifacts.loose.reports", "artifacts.loose.pages", "artifacts.loose.files", "artifacts.loose.noOrigin",
  "artifacts.loose.sessions", "artifacts.compare.slide", "artifacts.compare.side", "artifacts.compare.modes",
  "artifacts.compare.close", "artifacts.compare.before", "artifacts.compare.after", "artifacts.compare.range",
  "artifacts.compare.name", "artifacts.compare.trouble", "artifacts.bundle.whyAsk", "artifacts.bundle.why",
  "artifacts.bundle.noReport", "artifacts.bundle.lostWithFolder", "artifacts.bundle.lost",
  "artifacts.bundle.reportsKept", "artifacts.bundle.pair", "artifacts.bundle.pairSaid", "artifacts.bundle.deleted",
  "artifacts.bundle.evidence", "artifacts.bundle.tally", "artifacts.bundle.intended", "artifacts.bundle.pages",
  "artifacts.bundle.steps", "artifacts.bundle.unread", "artifacts.tests.passed", "artifacts.tests.failed",
  "artifacts.tests.ignored", "artifacts.tests.rc", "artifacts.landing.unknown", "artifacts.digest.tasked",
]);

/* The tasks of the synthetic catalog, laid over the rows `standCatalog` stands. */
export function standTasks(page) {
  return page.evaluate((fillers) => {
    const now = Date.now();
    const hour = 60 * 60 * 1000;
    const kept = "/tmp/zerocode-window-test/wt-kept";
    const gone = "/tmp/zerocode-window-test/wt-gone";
    const landedCommit = "89abcdef0123456789abcdef0123456789abcdef";
    const strangerCommit = "fedcba9876543210fedcba9876543210fedcba98";
    const parts = (held = {}) => ({ reports: 0, pages: 0, files: 0, pictures: 0, logs: 0, other: 0, missing: 0, ...held });
    const origin = (task, worker, more = {}) => ({
      run: "run-9", task, worker, agent: "claude", model: "claude-sonnet-5-5", work_summary: "큐 닫기", ...more,
    });
    const file = (id, kind, name, at, fields = {}) => ({
      id, kind, title: fields.title ?? name, path: `/data/artifacts/run/${id}/${name}`, bytes: fields.bytes ?? 2048,
      created_ms: now - at * hour, modified_ms: now - at * hour, origin: fields.origin ?? origin("t-501", "w-12", { worktree: kept }),
      tags: fields.tags ?? [], preview: { kind: "none" }, source: fields.source ?? "worker_evidence",
    });
    const bundleRows = {
      "b-before": file("b-before", "screenshot", "drawer-before.png", 1.2),
      "b-after": file("b-after", "screenshot", "drawer-after.png", 1.1),
      "b-single": file("b-single", "screenshot", "overview.png", 1.3),
      "b-red-log": file("b-red-log", "evidence", "red-rust.log", 1.5),
      "b-green-log": file("b-green-log", "evidence", "green-rust.log", 1.4),
      "b-gone-1": file("b-gone-1", "evidence", "window-full.log", 2.4, { origin: origin("t-502", "w-13", { worktree: gone, commit: landedCommit }) }),
      "b-gone-2": file("b-gone-2", "screenshot", "final-dark.png", 2.3, { origin: origin("t-502", "w-13", { worktree: gone, commit: landedCommit }) }),
      "p-demo": {
        id: "p-demo", kind: "page", title: "작업 묶음 시안", path: "/data/artifacts/pages/p-demo/index.html", bytes: 9000,
        created_ms: now - 3 * hour, modified_ms: now - 3 * hour, version: 1, url: "file:///data/artifacts/pages/p-demo/index.html",
        origin: { pane: "term-4", agent: "zo", model: "claude-opus-5-5", run: "run-9", worker: "w-15", task: "t-504", work_summary: "작업 묶음 시안" },
        tags: [], preview: { kind: "none" }, source: "manual",
      },
    };
    const lines = [
      {
        task: "t-501", work: "큐 닫기", modified_ms: now - hour, lead: "r-final", title: "큐 닫기 재시도 — 닫을 때 남은 일을 비운다",
        parts: parts({ reports: 2, pages: 1, files: 6, pictures: 3, logs: 2, other: 1 }), attempts: 2, sessions: 1, worktree: kept,
      },
      {
        task: "t-502", work: "큐 검증", modified_ms: now - 2 * hour, lead: "r-review", title: "검증 보고 — 큐가 비워지는가",
        parts: parts({ reports: 1, missing: 2 }), attempts: 1, sessions: 0, worktree: gone, commit: landedCommit,
      },
      {
        task: "t-504", work: "작업 묶음 시안", modified_ms: now - 3 * hour, lead: "p-demo", title: "작업 묶음 시안",
        parts: parts({ pages: 1 }), attempts: 0, sessions: 0,
      },
      {
        task: "t-503", work: "옛 작업", modified_ms: now - 6 * hour, lead: "r-old", title: "옛 빌드가 적은 보고서",
        parts: parts({ reports: 1 }), attempts: 1, sessions: 0, commit: strangerCommit,
      },
    ];
    for (let at = 0; at < fillers; at += 1) {
      lines.push({
        task: `t-${700 + at}`, work: `채움 작업 ${at}`, modified_ms: now - (10 + at) * hour, lead: null, title: `채움 작업 ${at}`,
        parts: parts({ reports: 1 }), attempts: 1, sessions: 0,
      });
    }
    const tests = (held) => ({ passed: 0, failed: 0, ignored: 0, filtered: 0, suites: 1, failed_names: [], more: 0, cut: false, verdict: "unknown", ...held });
    const red = tests({ failed: 3, failed_names: ["store::closes_the_queue", "store::counts_what_it_dropped"], more: 1, rc: 0, verdict: "intended" });
    const green = tests({ passed: 12, ignored: 1, verdict: "pass" });
    const row = (id) => bundleRows[id] ?? null;
    const bundles = {
      "t-501": {
        line: lines[0],
        reports: [{ id: "r-final", attempt: 2 }, { id: "r-first", attempt: 1 }],
        pictures: [{ name: "drawer", before: "b-before", after: "b-after" }, { name: "", single: "b-single" }],
        evidence: [{ id: "b-green-log", tests: green }, { id: "b-red-log", tests: red }, { id: "e-steps", steps: { of_task: 7, total: 62, failed: 2 } }],
        tally: { runs: 2, passed: 12, failed: 3, ignored: 1, pass: 1, fail: 0, intended: 1, unknown: 0 },
        unread: 0, pages: ["p-linked"],
        rowIds: ["r-final", "r-first", "p-linked", "b-before", "b-after", "b-single", "b-green-log", "b-red-log", "e-steps"],
        missing: [],
      },
      "t-502": {
        line: lines[1], reports: [{ id: "r-review", attempt: 1 }], pictures: [], evidence: [],
        tally: { runs: 0, passed: 0, failed: 0, ignored: 0, pass: 0, fail: 0, intended: 0, unknown: 0 },
        unread: 0, pages: [], rowIds: ["r-review", "b-gone-1", "b-gone-2"], missing: ["b-gone-1", "b-gone-2"],
      },
      "t-504": {
        line: lines[2], reports: [], pictures: [], evidence: [],
        tally: { runs: 0, passed: 0, failed: 0, ignored: 0, pass: 0, fail: 0, intended: 0, unknown: 0 },
        unread: 0, pages: ["p-demo"], rowIds: ["p-demo"], missing: [],
      },
    };
    const seen = (window.__TASKS__ = { asks: [], bundles: [], commits: [], lines: lines.length });
    // The runtime's listing by task, in small: the words are asked of the line.
    window.__ANSWER__.artifact_tasks = (args) => {
      const filter = args?.filter ?? {};
      seen.asks.push(JSON.parse(JSON.stringify(filter)));
      const needle = String(filter.query ?? "").toLowerCase().split(/\s+/).filter(Boolean);
      const held = lines.filter((line) => {
        const hay = `${line.task}\n${line.work}\n${line.title}`.toLowerCase();
        return needle.every((word) => hay.includes(word)) && (!filter.agent || filter.agent === "claude");
      });
      return {
        tasks: held, total: held.length, truncated: false,
        unlinked: {
          parts: parts({ reports: 2, pages: 2, files: 4, pictures: 1, logs: 1, other: 2, missing: 3 }),
          reports_without_origin: 1, session_files: 4, sessions: 1,
        },
      };
    };
    const listed = window.__ANSWER__.artifacts_list;
    window.__ANSWER__.artifact_bundle = (args) => {
      seen.bundles.push(args.task);
      const bundle = bundles[args.task];
      if (!bundle) return null;
      // The rows the window already holds come from its own listing; the bundle brings the rest.
      const known = new Map(listed({ filter: {} }).rows.map((one) => [one.id, one]));
      const { rowIds, ...rest } = bundle;
      return { ...rest, rows: rowIds.map((id) => known.get(id) ?? row(id)).filter(Boolean) };
    };
    window.__ANSWER__.commit_landings = (args) => {
      seen.commits.push([...(args?.commits ?? [])]);
      const said = {};
      for (const commit of args?.commits ?? []) {
        said[commit] = commit === landedCommit
          ? { state: "landed", detached: true, ahead: 0, dirty: false, ignored: false, compare_ref: "origin/main", landed_in: { sha: commit, time_ms: now - hour } }
          : { state: "unknown", detached: true, ahead: 0, dirty: false, ignored: false };
      }
      return said;
    };
    // Two pictures that differ, and the two logs as the runtime digests them.
    const picture = (fill) => `data:image/svg+xml,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" width="640" height="360"><rect width="640" height="360" fill="${fill}"/></svg>`)}`;
    const previews = {
      "b-before": { kind: "image", data_url: picture("#334455"), bytes: 200, truncated: false },
      "b-after": { kind: "image", data_url: picture("#557799"), bytes: 200, truncated: false },
      "b-red-log": { kind: "text", text: "test store::closes_the_queue ... FAILED\n", bytes: 60, truncated: false, digest: { kind: "tests", ...red } },
      "b-green-log": { kind: "text", text: "test result: ok. 12 passed\n", bytes: 40, truncated: false, digest: { kind: "tests", ...green } },
    };
    const previewed = window.__ANSWER__.artifact_preview;
    window.__ANSWER__.artifact_preview = (args) => previews[args.id] ?? previewed(args);
    return { lines: lines.length, landedCommit, strangerCommit };
  }, FILLER_TASKS);
}

/* Open the tab fresh, as a person does from the sidebar: nothing picked, nothing filtered. */
async function openFresh(page) {
  await page.evaluate(async () => {
    dropTab("artifacts");
    Object.assign(artifactFilter, { query: "", origin: null, project: null, agent: null, period: "all", showMissing: false });
    if (typeof ARTIFACT_TASKS_TAB === "string") artifactFilter.tab = ARTIFACT_TASKS_TAB;
    artifactTabPicked = false;
    artifactSelectedId = null;
    if (typeof closeArtifactTask === "function") closeArtifactTask();
    el("nav-artifacts").click();
    await window.__PAINTED__();
    await new Promise((done) => setTimeout(done, 250));
  });
}

/* Open one task's line and wait for its bundle to be drawn. */
async function openTask(page, task) {
  await page.evaluate(async (wanted) => {
    const view = artifactsView();
    const line = [...(view?.querySelectorAll(".artifact-task") ?? [])].find((one) => one.dataset.task === wanted);
    line?.click();
    await window.__PAINTED__();
    await new Promise((done) => setTimeout(done, 300));
  }, task);
}

export async function testArtifactTasks(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.setViewportSize(REAL_WINDOW);
    await standCatalog(page);
    const fixture = await standTasks(page);
    await openFresh(page);

    /* ---- the tab row and the list ------------------------------------------------------------ */
    const list = await page.evaluate(async () => {
      const view = artifactsView();
      const shown = (node) => Boolean(node) && !node.closest("[hidden]") && node.getClientRects().length > 0;
      const seen = {};
      seen.tabs = [...(view?.querySelectorAll("[data-artifact-tab]") ?? [])].filter((one) => !one.hidden)
        .map((one) => `${one.dataset.artifactTab}:${one.querySelector(".artifacts-tab-count")?.textContent ?? ""}`).join(",");
      seen.tabWords = [...(view?.querySelectorAll("[data-artifact-tab]") ?? [])].filter((one) => !one.hidden)
        .map((one) => one.firstElementChild?.textContent ?? "").join(" · ");
      seen.selectedTab = view?.querySelector('[role="tab"][aria-selected="true"]')?.dataset.artifactTab ?? null;
      seen.asks = window.__TASKS__.asks.length;
      const rows = [...(view?.querySelectorAll(".artifact-task") ?? [])].filter(shown)
        .sort((a, b) => a.getBoundingClientRect().top - b.getBoundingClientRect().top);
      seen.built = view?.querySelectorAll(".artifact-task").length ?? 0;
      seen.order = rows.slice(0, 4).map((one) => one.dataset.task).join(",");
      const first = rows[0] ?? null;
      seen.first = first ? {
        id: first.querySelector(".artifact-task-id")?.textContent ?? "",
        title: first.querySelector(".artifact-task-title")?.textContent ?? "",
        when: first.querySelector(".artifact-task-when")?.textContent ?? "",
        parts: first.querySelector(".artifact-task-parts")?.textContent ?? "",
        height: Math.round(first.getBoundingClientRect().height),
      } : null;
      const chip = (task) => {
        const node = rows.find((one) => one.dataset.task === task)?.querySelector(".artifact-task-landing") ?? null;
        return node && !node.hidden ? node.textContent : null;
      };
      // The commits are asked in one batch, a moment after the lines are drawn.
      await new Promise((done) => setTimeout(done, 200));
      seen.chips = { kept: chip("t-501"), commit: chip("t-502"), stranger: chip("t-503"), none: chip("t-504") };
      seen.commitAsks = window.__TASKS__.commits.map((one) => one.slice().sort().join("+"));
      const loose = view?.querySelector(".artifacts-loose") ?? null;
      seen.loose = shown(loose) ? loose.textContent.replace(/\s+/g, " ").trim() : null;
      seen.looseButtons = [...(loose?.querySelectorAll("button") ?? [])].filter(shown).map((one) => one.textContent);
      seen.gridShown = shown(view?.querySelector(".artifacts-grid"));
      seen.taskPickShown = shown(view?.querySelector('[data-artifact-pick="task"]'));
      seen.drawerShown = shown(view?.querySelector(".artifacts-drawer"));
      seen.listWidth = Math.round(view?.querySelector(".artifacts-tasks")?.getBoundingClientRect().width ?? 0);
      seen.viewWidth = Math.round(view?.getBoundingClientRect().width ?? 0);
      return seen;
    });
    ok(
      "the tabs read 작업별 · 보고서 · 증거 · 페이지·문서, each with its count, and the tab opens on 작업별 — the lines asked for once, beside the rows",
      list.tabs === `tasks:${fixture.lines},reports:5,evidence:4,pages:3` && list.tabWords.startsWith("작업별 · 보고서 · 증거 · 페이지·문서")
        && list.selectedTab === "tasks" && list.asks === 1,
      JSON.stringify({ tabs: list.tabs, words: list.tabWords, selected: list.selectedTab, asks: list.asks }),
    );
    ok(
      "there is one line for each task, newest first, and a line says its task, when it last produced something, its title and what it holds",
      list.order === "t-501,t-502,t-504,t-503" && list.first?.id === "t-501" && list.first.title.includes("큐 닫기 재시도")
        && /\d/.test(list.first.when) && list.first.parts.includes("보고서 2") && list.first.parts.includes("시도 2")
        && list.first.parts.includes("페이지 1") && list.gridShown === false && list.taskPickShown === false,
      JSON.stringify({ order: list.order, first: list.first, grid: list.gridShown, pick: list.taskPickShown }),
    );
    const said = /파일 (\d+) \(([^)]*)\)/.exec(list.first?.parts ?? "");
    const inside = said ? [...said[2].matchAll(/(\d+)/g)].map((one) => Number(one[1])) : [];
    ok(
      "a line's files are said with their parts, and the parts add up to the number before them: 「파일 6 (그림 3 · 로그 2 · 그 밖 1)」",
      said !== null && inside.length === 3 && inside.reduce((sum, n) => sum + n, 0) === Number(said[1]) && Number(said[1]) === 6,
      JSON.stringify({ parts: list.first?.parts ?? null }),
    );
    ok(
      "only the lines in view are built: of the tasks answered, the list holds a screenful of nodes",
      list.built > 0 && list.built < fixture.lines / 2 && list.viewWidth > 0
        && Math.abs(list.viewWidth - REAL_TAB_WIDTH) <= WIDTH_SLACK && Math.abs(list.listWidth - list.viewWidth) <= 2 && list.drawerShown === false,
      JSON.stringify({ built: list.built, lines: fixture.lines, view: list.viewWidth, list: list.listWidth, drawer: list.drawerShown }),
    );
    ok(
      "a task wears its landed state: from its checkout while the window holds it, from the commit it handed in once the checkout is gone, 「확인 안 됨」 when no repository knows the commit, and nothing when it handed none in — the commits asked for in one batch",
      list.chips.kept === "반영됨" && list.chips.commit === "반영됨" && typeof list.chips.stranger === "string"
        && list.chips.stranger.includes("확인 안 됨") && list.chips.none === null
        && list.commitAsks.length === 1 && list.commitAsks[0] === [fixture.landedCommit, fixture.strangerCommit].sort().join("+"),
      JSON.stringify({ chips: list.chips, asks: list.commitAsks }),
    );
    ok(
      "what no task is linked to is said at the foot of the list — the reports with the ones whose origin is empty, the pages, and the evidence as files and as sessions",
      typeof list.loose === "string" && list.loose.includes("작업에 연결 안 된 것") && list.loose.includes("보고서 2")
        && list.loose.includes("출처가 빈 것 1") && list.loose.includes("페이지·문서 2") && list.loose.includes("증거 4")
        && list.loose.includes("파일 4개") && list.loose.includes("세션 1개") && list.looseButtons.length === 3,
      JSON.stringify({ loose: list.loose, buttons: list.looseButtons }),
    );

    /* ---- the reader: one task at the real width ------------------------------------------------ */
    await openTask(page, "t-501");
    const reader = await page.evaluate(() => {
      const view = artifactsView();
      const shown = (node) => Boolean(node) && !node.closest("[hidden]") && node.getClientRects().length > 0;
      const drawer = view?.querySelector(".artifacts-drawer") ?? null;
      const box = (node) => (node ? node.getBoundingClientRect() : null);
      const seen = {};
      seen.detail = drawer?.querySelector(".artifacts-detail")?.dataset.artifactId ?? null;
      seen.stripWidth = Math.round(box(view?.querySelector(".artifacts-main"))?.width ?? 0);
      seen.readerWidth = Math.round(box(drawer)?.width ?? 0);
      seen.viewWidth = Math.round(box(view)?.width ?? 0);
      seen.selectedLine = view?.querySelector(".artifact-task.is-selected")?.dataset.task ?? null;
      const bar = drawer?.querySelector(".artifact-detail-bar") ?? null;
      seen.bar = bar ? [...bar.children].filter(shown).map((one) => one.className.split(" ")[0]) : [];
      seen.barTask = bar?.querySelector(".artifact-detail-task")?.textContent ?? null;
      seen.attempts = [...(bar?.querySelector(".artifact-detail-attempts")?.options ?? [])].map((one) => one.textContent);
      const bundle = drawer?.querySelector(".artifact-bundle") ?? null;
      seen.bundleShown = shown(bundle);
      const cards = [...(bundle?.querySelectorAll(".artifact-evidence") ?? [])].filter(shown);
      seen.cards = cards.map((one) => ({
        name: one.querySelector(".artifact-evidence-name")?.textContent ?? "",
        said: one.querySelector(".artifact-evidence-said")?.textContent ?? "",
        verdict: one.dataset.verdict ?? "",
        tag: one.tagName,
        height: Math.round(one.getBoundingClientRect().height),
        color: getComputedStyle(one.querySelector(".artifact-evidence-said") ?? one).color,
      }));
      seen.head = bundle?.querySelector(".artifact-bundle-evidence .artifact-bundle-head")?.textContent ?? null;
      const probe = document.createElement("span");
      view?.appendChild(probe);
      const colorOf = (name) => {
        probe.style.color = `var(${name})`;
        return getComputedStyle(probe).color;
      };
      seen.halt = colorOf("--signal-halt-ink");
      seen.ready = colorOf("--signal-ready-ink");
      probe.remove();
      // The strip: the cards stand in one row under the head, above the report.
      const strip = bundle?.querySelector(".artifact-bundle-cards") ?? null;
      const tops = new Set(cards.map((one) => Math.round(one.getBoundingClientRect().top)));
      seen.oneRow = tops.size === 1;
      seen.stripAboveBody = (box(strip)?.bottom ?? Infinity) <= (box(drawer?.querySelector(".artifact-preview"))?.top ?? -Infinity) + 1;
      seen.rail = view?.classList.contains("is-rail") ?? null;
      seen.pages = [...(bundle?.querySelectorAll(".artifact-bundle-page") ?? [])].filter(shown).map((one) => one.textContent);
      seen.pagesHead = bundle?.querySelector(".artifact-bundle-pages .artifact-bundle-head")?.textContent ?? null;
      seen.banner = shown(bundle?.querySelector(".artifact-bundle-banner"));
      seen.bundleAsks = window.__TASKS__.bundles.join(",");
      seen.overflowX = (view?.querySelector(".artifacts-tasks")?.scrollWidth ?? 0) - (view?.querySelector(".artifacts-tasks")?.clientWidth ?? 0);
      return seen;
    });
    ok(
      "opening a task reads its final report in two panes at the real width: the list becomes a strip and the reader takes the rest",
      reader.detail === "r-final" && reader.selectedLine === "t-501" && Math.abs(reader.stripWidth - STRIP_WIDTH) <= 4
        && reader.readerWidth >= reader.viewWidth - STRIP_WIDTH - 6 && reader.readerWidth >= 560 && reader.overflowX <= 0
        && reader.bundleAsks === "t-501",
      JSON.stringify({ detail: reader.detail, line: reader.selectedLine, strip: reader.stripWidth, reader: reader.readerWidth, view: reader.viewWidth, asks: reader.bundleAsks }),
    );
    ok(
      "the reader's head is the one row of stage 1, and the task's other attempts are one select: 「최종 보고 · 2편 중」 and 「1번째 시도」",
      reader.barTask === "t-501" && reader.bar.includes("artifact-detail-attempts") && reader.attempts.length === 2
        && reader.attempts[0].includes("최종 보고") && reader.attempts[0].includes("2") && reader.attempts[1].includes("1번째 시도"),
      JSON.stringify({ bar: reader.bar, attempts: reader.attempts }),
    );
    const card = (name) => reader.cards.find((one) => one.name.includes(name)) ?? null;
    ok(
      "the task's evidence stands under the head as a strip of cards, every file its own card with a plain status word beside its numbers",
      reader.bundleShown === true && reader.cards.length === 5 && reader.oneRow === true && reader.stripAboveBody === true && reader.rail === false
        && reader.cards.every((one) => one.tag === "BUTTON" && one.height >= PRESS_MIN_PX)
        && (card("green-rust.log")?.said ?? "").startsWith("✓ 통과") && (card("green-rust.log")?.said ?? "").includes("통과 12")
        && (card("green-rust.log")?.said ?? "").includes("무시됨 1") && card("green-rust.log")?.color === reader.ready,
      JSON.stringify({ shown: reader.bundleShown, cards: reader.cards, oneRow: reader.oneRow, above: reader.stripAboveBody, rail: reader.rail }),
    );
    ok(
      "a failure the run was for is a grey ✕ 「의도한 실패」 with its exit code — never the red of a failure",
      card("red-rust.log")?.verdict === "intended" && (card("red-rust.log")?.said ?? "").startsWith("✕ 의도한 실패")
        && (card("red-rust.log")?.said ?? "").includes("실패 3") && (card("red-rust.log")?.said ?? "").includes("끝 코드 0")
        && card("red-rust.log")?.color !== reader.halt && card("red-rust.log")?.color !== reader.ready,
      JSON.stringify({ card: card("red-rust.log"), halt: reader.halt }),
    );
    const headNumbers = [...(reader.head ?? "").matchAll(/(\d[\d,]*)/g)].map((one) => Number(one[1].replace(/,/g, "")));
    ok(
      "a sum is said only beside its parts: 「이 작업의 증거 6 · 시험 통과 12 · 실패 3 · 무시됨 1 · 의도한 실패 1건」 is the sum of the cards under it",
      typeof reader.head === "string" && reader.head.includes("이 작업의 증거 6") && reader.head.includes("통과 12")
        && reader.head.includes("실패 3") && reader.head.includes("무시됨 1") && reader.head.includes("의도한 실패 1건") && headNumbers.length === 5,
      JSON.stringify({ head: reader.head }),
    );
    ok(
      "a Computer Use session's card says how many of its steps were this task's, beside all of them",
      (card("steps.jsonl")?.said ?? "").includes("이 작업의 단계 7") && (card("steps.jsonl")?.said ?? "").includes("전체 62"),
      JSON.stringify({ card: card("steps.jsonl") }),
    );
    ok(
      "the task's pages stand in the reader under their own count",
      reader.pages.length === 1 && reader.pages[0].includes("큐 닫기 전후 그림") && (reader.pagesHead ?? "").includes("페이지·문서 1") && reader.banner === false,
      JSON.stringify({ pages: reader.pages, head: reader.pagesHead, banner: reader.banner }),
    );

    /* ---- a card reads its file; a test log is its verdict and the names that failed ----------- */
    const log = await page.evaluate(async () => {
      const view = artifactsView();
      const drawer = view?.querySelector(".artifacts-drawer") ?? null;
      const cards = [...(drawer?.querySelectorAll("button.artifact-evidence") ?? [])];
      cards.find((one) => (one.textContent ?? "").includes("red-rust.log"))?.click();
      await window.__PAINTED__();
      await new Promise((done) => setTimeout(done, 250));
      const digest = drawer?.querySelector(".artifact-digest") ?? null;
      const seen = {};
      seen.detail = drawer?.querySelector(".artifacts-detail")?.dataset.artifactId ?? null;
      seen.task = typeof artifactTaskOpen === "string" ? artifactTaskOpen : null;
      seen.summary = digest?.querySelector(".artifact-digest-summary")?.textContent ?? null;
      seen.names = [...(digest?.querySelectorAll(".artifact-digest-fact") ?? [])].map((one) => one.textContent);
      seen.rawShown = Boolean(drawer?.querySelector(".artifact-preview-text")) && !drawer.querySelector(".artifact-preview-text").hidden;
      seen.selectedCards = [...(drawer?.querySelectorAll(".artifact-evidence.is-selected") ?? [])].map((one) => one.querySelector(".artifact-evidence-name")?.textContent ?? "");
      seen.current = drawer?.querySelector(".artifact-evidence.is-selected")?.getAttribute("aria-current") ?? null;
      seen.stripStill = [...(drawer?.querySelectorAll("button.artifact-evidence") ?? [])].length;
      // And a page of the task reads the same way.
      drawer?.querySelector(".artifact-bundle-page")?.click();
      await window.__PAINTED__();
      await new Promise((done) => setTimeout(done, 150));
      seen.pageDetail = drawer?.querySelector(".artifacts-detail")?.dataset.artifactId ?? null;
      seen.pageSelected = drawer?.querySelector(".artifact-bundle-page.is-selected") !== null;
      return seen;
    });
    ok(
      "pressing an evidence card reads that file in the reader while the task stays open, and the card wears the selection",
      log.detail === "b-red-log" && log.task === "t-501" && log.selectedCards.join(",") === "red-rust.log" && log.current === "true" && log.stripStill === 5
        && log.pageDetail === "p-linked" && log.pageSelected === true,
      JSON.stringify(log),
    );
    const named = log.names.filter((one) => one.startsWith("✕")).length;
    const others = Number(/(\d+)/.exec(log.names.find((one) => one.includes("그 밖")) ?? "")?.[1] ?? 0);
    ok(
      "a test log opens as its verdict and the tests that failed, never as raw text — and the failed count is the names shown plus 「그 밖 N」",
      typeof log.summary === "string" && log.summary.includes("의도한 실패") && log.summary.includes("실패 3")
        && named === 2 && others === 1 && named + others === 3 && log.rawShown === false,
      JSON.stringify({ summary: log.summary, names: log.names, raw: log.rawShown }),
    );

    /* ---- before and after ---------------------------------------------------------------------- */
    await openTask(page, "t-501");
    const compared = await page.evaluate(async () => {
      const view = artifactsView();
      const shown = (node) => Boolean(node) && !node.closest("[hidden]") && node.getClientRects().length > 0;
      const drawer = view?.querySelector(".artifacts-drawer") ?? null;
      const pair = [...(drawer?.querySelectorAll("button.artifact-evidence") ?? [])].find((one) => one.dataset.before) ?? null;
      const seen = { pairName: pair?.querySelector(".artifact-evidence-name")?.textContent ?? null };
      pair?.click();
      await window.__PAINTED__();
      await new Promise((done) => setTimeout(done, 250));
      const compare = drawer?.querySelector(".artifact-compare") ?? null;
      const stage = compare?.querySelector(".artifact-compare-stage") ?? null;
      const range = compare?.querySelector(".artifact-compare-range") ?? null;
      seen.open = shown(compare);
      seen.name = compare?.querySelector(".artifact-compare-name")?.textContent ?? null;
      seen.mode = stage?.dataset.compareMode ?? null;
      seen.width = Math.round(compare?.getBoundingClientRect().width ?? 0);
      seen.readerInner = Math.round(drawer?.querySelector(".artifact-preview")?.getBoundingClientRect().width ?? 0);
      seen.pictures = [...(stage?.querySelectorAll("img") ?? [])].filter((one) => one.getAttribute("src")).length;
      seen.at = stage ? getComputedStyle(stage).getPropertyValue("--artifact-compare-at").trim() : null;
      seen.clip = stage?.querySelector(".is-before") ? getComputedStyle(stage.querySelector(".is-before")).clipPath : null;
      seen.rangeTag = range?.tagName ?? null;
      seen.rangeType = range?.type ?? null;
      seen.rangeLabel = range?.getAttribute("aria-label") ?? null;
      seen.handle = shown(stage?.querySelector(".artifact-compare-handle"));
      range?.focus();
      return seen;
    });
    for (let at = 0; at < 3; at += 1) await page.keyboard.press("ArrowRight");
    // A drag on the picture with the real pointer: pressed at a quarter of its width, let go at
    // three quarters.
    const stageBox = await page.evaluate(() => {
      const stage = artifactsView()?.querySelector(".artifact-compare-stage") ?? null;
      if (!stage || stage.getClientRects().length === 0) return null;
      const seen = {
        afterKeys: getComputedStyle(stage).getPropertyValue("--artifact-compare-at").trim(),
        rangeValue: artifactsView().querySelector(".artifact-compare-range")?.value ?? null,
      };
      stage.scrollIntoView({ block: "center" });
      const box = stage.getBoundingClientRect();
      return { ...seen, left: box.left, top: box.top, width: box.width, height: box.height };
    });
    if (stageBox && stageBox.width > 0) {
      const y = stageBox.top + Math.max(4, stageBox.height / 2);
      await page.mouse.move(stageBox.left + stageBox.width * 0.25, y);
      await page.mouse.down();
      await page.mouse.move(stageBox.left + stageBox.width * 0.75, y, { steps: 4 });
      await page.mouse.up();
    }
    const moved = await page.evaluate(async (keyed) => {
      const view = artifactsView();
      const compare = view?.querySelector(".artifact-compare") ?? null;
      const stage = compare?.querySelector(".artifact-compare-stage") ?? null;
      const seen = { afterKeys: keyed?.afterKeys ?? null, rangeValue: keyed?.rangeValue ?? null };
      const at = () => (stage ? getComputedStyle(stage).getPropertyValue("--artifact-compare-at").trim() : null);
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      seen.afterDrag = at();
      // Side by side.
      compare?.querySelector('[data-compare-mode="side"]')?.click();
      await window.__PAINTED__();
      const before = stage?.querySelector(".is-before")?.getBoundingClientRect() ?? null;
      const after = stage?.querySelector(".is-after")?.getBoundingClientRect() ?? null;
      seen.side = before && after
        ? { beforeFirst: before.right <= after.left + 1, sameRow: Math.abs(before.top - after.top) <= 1, mode: stage.dataset.compareMode }
        : null;
      seen.rangeHiddenInSide = compare?.querySelector(".artifact-compare-range")?.hidden ?? null;
      seen.modes = [...(compare?.querySelectorAll("button[data-compare-mode]") ?? [])].map((one) => `${one.dataset.compareMode}:${one.getAttribute("aria-pressed")}`).join(",");
      seen.stagePressed = stage?.getAttribute("aria-pressed") ?? null;
      compare?.querySelector(".artifact-compare-close")?.click();
      await window.__PAINTED__();
      seen.closed = compare ? compare.hidden : null;
      return seen;
    }, stageBox);
    ok(
      "a before-and-after pair opens a comparison as wide as the reader's own text, the two pictures over each other with a boundary in the middle",
      compared.pairName !== null && compared.pairName.includes("drawer") && compared.open === true && (compared.name ?? "").includes("drawer")
        && compared.mode === "slide" && compared.pictures === 2 && compared.at === "50%" && compared.handle === true
        && compared.width > 0 && Math.abs(compared.width - compared.readerInner) <= 2,
      JSON.stringify(compared),
    );
    ok(
      "the boundary is moved from the keyboard with a range input, and follows a drag on the picture",
      compared.rangeTag === "INPUT" && compared.rangeType === "range" && typeof compared.rangeLabel === "string" && compared.rangeLabel !== ""
        && moved.afterKeys === "56%" && moved.rangeValue === "56" && moved.afterDrag === "75%",
      JSON.stringify({ range: [compared.rangeTag, compared.rangeType, compared.rangeLabel], keys: moved.afterKeys, value: moved.rangeValue, drag: moved.afterDrag }),
    );
    ok(
      "「나란히」 sets the two pictures in one row, before first, with no boundary to move — and closing gives the report back",
      moved.side !== null && moved.side.beforeFirst && moved.side.sameRow && moved.side.mode === "side" && moved.rangeHiddenInSide === true
        && moved.modes === "slide:false,side:true" && moved.stagePressed === null && moved.closed === true,
      JSON.stringify({ side: moved.side, range: moved.rangeHiddenInSide, modes: moved.modes, stagePressed: moved.stagePressed, closed: moved.closed }),
    );

    /* ---- what is gone, and a task with a page and no report ------------------------------------ */
    await openTask(page, "t-502");
    const lost = await page.evaluate(async () => {
      const view = artifactsView();
      const shown = (node) => Boolean(node) && !node.closest("[hidden]") && node.getClientRects().length > 0;
      const bundle = view?.querySelector(".artifact-bundle") ?? null;
      const seen = {};
      seen.banner = shown(bundle?.querySelector(".artifact-bundle-banner")) ? bundle.querySelector(".artifact-bundle-banner-word").textContent : null;
      const cards = [...(bundle?.querySelectorAll(".artifact-evidence") ?? [])].filter(shown);
      seen.cards = cards.map((one) => ({
        name: one.querySelector(".artifact-evidence-name")?.textContent ?? "",
        said: one.querySelector(".artifact-evidence-said")?.textContent ?? "",
        struck: getComputedStyle(one.querySelector(".artifact-evidence-name") ?? one).textDecorationLine,
      }));
      const why = bundle?.querySelector(".artifact-bundle-why") ?? null;
      seen.whyAsk = why?.querySelector("summary")?.textContent ?? null;
      seen.whyClosed = why ? !why.open : null;
      why?.querySelector("summary")?.click();
      await window.__PAINTED__();
      seen.whyWord = why?.open ? why.querySelector(".artifact-bundle-why-word")?.textContent ?? null : null;
      seen.head = bundle?.querySelector(".artifact-bundle-evidence .artifact-bundle-head")?.textContent ?? null;
      seen.landing = view?.querySelector(".artifact-detail-landing")?.textContent ?? null;
      return seen;
    });
    ok(
      "a task whose files are gone says so in the product's words — what is missing, why, and what is left — with 「왜?」 behind it",
      typeof lost.banner === "string" && lost.banner.includes("증거 2개가 없습니다") && lost.banner.includes("작업 폴더를 정리할 때 함께 삭제")
        && lost.banner.includes("보고서는 남아 있습니다") && lost.whyAsk === "왜?" && lost.whyClosed === true
        && typeof lost.whyWord === "string" && lost.whyWord.length > 20,
      JSON.stringify({ banner: lost.banner, why: [lost.whyAsk, lost.whyClosed, lost.whyWord] }),
    );
    ok(
      "every lost file is still named — struck through, with 「삭제됨」 — and the reader's head wears the landed state the commit gave",
      lost.cards.length === 2 && lost.cards.every((one) => one.struck.includes("line-through") && one.said === "삭제됨")
        && (lost.head ?? "").includes("이 작업의 증거 2") && lost.landing === "반영됨",
      JSON.stringify({ cards: lost.cards, head: lost.head, landing: lost.landing }),
    );
    await openTask(page, "t-504");
    const demo = await page.evaluate(() => {
      const view = artifactsView();
      const shown = (node) => Boolean(node) && !node.closest("[hidden]") && node.getClientRects().length > 0;
      const drawer = view?.querySelector(".artifacts-drawer") ?? null;
      return {
        detail: drawer?.querySelector(".artifacts-detail")?.dataset.artifactId ?? null,
        when: drawer?.querySelector(".artifact-detail-when")?.textContent ?? null,
        pages: [...(drawer?.querySelectorAll(".artifact-bundle-page") ?? [])].filter(shown).map((one) => one.textContent),
        none: shown(drawer?.querySelector(".artifact-bundle-none")) ? drawer.querySelector(".artifact-bundle-none").textContent : null,
        evidence: shown(drawer?.querySelector(".artifact-bundle-evidence")),
      };
    });
    ok(
      "a task that left a page and no report opens on its page: the page is listed, and the reader says the task has no report",
      demo.detail === "p-demo" && (demo.when ?? "").includes("페이지") && demo.pages.length === 1 && demo.pages[0].includes("작업 묶음 시안")
        && demo.none === "이 작업의 보고서가 없습니다." && demo.evidence === false,
      JSON.stringify(demo),
    );

    /* ---- the filter: asked again only when the question changed; the reader closes with its line */
    await openTask(page, "t-501");
    const filtered = await page.evaluate(async () => {
      const view = artifactsView();
      const shown = (node) => Boolean(node) && !node.closest("[hidden]") && node.getClientRects().length > 0;
      const seen = {};
      const asks = () => window.__TASKS__.asks.length;
      const before = asks();
      view?.querySelector('[data-artifact-tab="reports"]')?.click();
      await new Promise((done) => setTimeout(done, 120));
      seen.readerAfterLeaving = typeof artifactTaskOpen === "undefined" ? "undefined" : artifactTaskOpen;
      view?.querySelector('[data-artifact-tab="tasks"]')?.click();
      await new Promise((done) => setTimeout(done, 120));
      seen.asksOnTabs = asks() - before;
      const line = [...(view?.querySelectorAll(".artifact-task") ?? [])].find((one) => one.dataset.task === "t-501");
      line?.click();
      await new Promise((done) => setTimeout(done, 250));
      const query = view?.querySelector(".artifacts-query") ?? null;
      if (query) {
        query.value = "검증";
        query.dispatchEvent(new Event("input", { bubbles: true }));
      }
      await new Promise((done) => setTimeout(done, 500));
      seen.asksOnQuery = asks() - before - seen.asksOnTabs;
      seen.lastQuery = window.__TASKS__.asks.at(-1)?.query ?? null;
      seen.lines = [...(view?.querySelectorAll(".artifact-task") ?? [])].filter(shown).map((one) => one.dataset.task).join(",");
      seen.open = typeof artifactTaskOpen === "undefined" ? "undefined" : artifactTaskOpen;
      seen.drawerShown = shown(view?.querySelector(".artifacts-drawer"));
      seen.tabCount = view?.querySelector('[data-artifact-tab="tasks"] .artifacts-tab-count')?.textContent ?? null;
      if (query) {
        query.value = "아무 작업에도 없는 말";
        query.dispatchEvent(new Event("input", { bubbles: true }));
      }
      await new Promise((done) => setTimeout(done, 500));
      const empty = view?.querySelector(".artifacts-tasks-empty") ?? null;
      seen.empty = shown(empty) ? empty.textContent : null;
      if (query) {
        query.value = "";
        query.dispatchEvent(new Event("input", { bubbles: true }));
      }
      await new Promise((done) => setTimeout(done, 500));
      seen.linesBack = view?.querySelector('[data-artifact-tab="tasks"] .artifacts-tab-count')?.textContent ?? null;
      return seen;
    });
    ok(
      "the lines are asked for again only when the question changed — not when a tab is switched — and the search goes to the lines",
      filtered.asksOnTabs === 0 && filtered.asksOnQuery === 1 && filtered.lastQuery === "검증" && filtered.lines === "t-502" && filtered.tabCount === "1",
      JSON.stringify({ tabs: filtered.asksOnTabs, query: filtered.asksOnQuery, last: filtered.lastQuery, lines: filtered.lines, count: filtered.tabCount }),
    );
    ok(
      "the reader closes when its line is filtered out, and when the tab is left; a filter no task matches says so",
      filtered.readerAfterLeaving === null && filtered.open === null && filtered.drawerShown === false
        && filtered.empty === "이 거르개에 맞는 작업이 없습니다." && filtered.linesBack === String(fixture.lines),
      JSON.stringify({ left: filtered.readerAfterLeaving, open: filtered.open, drawer: filtered.drawerShown, empty: filtered.empty, back: filtered.linesBack }),
    );
    const loose = await page.evaluate(async () => {
      const view = artifactsView();
      view?.querySelector('.artifacts-loose [data-loose-tab="reports"]')?.click();
      await new Promise((done) => setTimeout(done, 200));
      const seen = {
        tab: artifactFilter.tab,
        chip: view?.querySelector(".artifacts-origin-chip")?.textContent ?? null,
        cards: [...(view?.querySelectorAll(".artifact-card") ?? [])].map((one) => one.dataset.id).sort().join(","),
      };
      view?.querySelector(".artifacts-origin-clear")?.click();
      view?.querySelector('[data-artifact-tab="tasks"]')?.click();
      await new Promise((done) => setTimeout(done, 200));
      return seen;
    });
    ok(
      "pressing a line of the foot opens that tab filtered to what no task is linked to",
      loose.tab === "reports" && (loose.chip ?? "").includes("작업에 연결 안 된 것") && loose.cards === "r-brief",
      JSON.stringify(loose),
    );

    /* ---- the screen rules, with a task open ------------------------------------------------------ */
    await openTask(page, "t-501");
    const rules = await page.evaluate(async () => {
      const view = artifactsView();
      const shown = (node) => Boolean(node) && !node.closest("[hidden]") && node.getClientRects().length > 0;
      [...(view?.querySelectorAll("button.artifact-evidence") ?? [])].find((one) => one.dataset.before)?.click();
      await new Promise((done) => setTimeout(done, 250));
      const name = (node) => String(node.className || node.tagName).split(" ").slice(0, 2).join(".");
      const picture = (node) => Boolean(node.closest('[class*="agent-ico"], svg'));
      const small = new Map();
      for (const node of view?.querySelectorAll("*") ?? []) {
        if (!shown(node) || picture(node)) continue;
        const own = [...node.childNodes].some((child) => child.nodeType === 3 && child.textContent.trim() !== "");
        if (own && parseFloat(getComputedStyle(node).fontSize) < 12) small.set(name(node), parseFloat(getComputedStyle(node).fontSize));
      }
      const short = new Map();
      for (const node of view?.querySelectorAll('button, select, input, summary, a[href], [role="tab"], [role="option"]') ?? []) {
        if (!shown(node) || node.closest(".md-body")) continue;
        const height = Math.round(node.getBoundingClientRect().height);
        if (height < 28) short.set(name(node), height);
      }
      const probe = document.createElement("span");
      probe.style.color = "var(--artifact-selected-edge)";
      view?.appendChild(probe);
      const amber = getComputedStyle(probe).color;
      probe.remove();
      const selected = (node) => Boolean(node.closest('.is-selected, [aria-selected="true"], [aria-current="true"], [aria-current="location"]'));
      const loud = new Set();
      for (const node of view?.querySelectorAll(".artifacts-head *, .artifacts-body *") ?? []) {
        if (!shown(node) || selected(node)) continue;
        const style = getComputedStyle(node);
        const edges = ["Top", "Right", "Bottom", "Left"].filter((side) => parseFloat(style[`border${side}Width`]) > 0).map((side) => style[`border${side}Color`]);
        const own = [...node.childNodes].some((child) => child.nodeType === 3 && child.textContent.trim() !== "");
        if (edges.includes(amber) || (own && style.color === amber)) loud.add(name(node));
      }
      const line = view?.querySelector(".artifact-task.is-selected") ?? null;
      const seen = {
        small: [...small].map(([key, px]) => `${key}:${px}`),
        short: [...short].map(([key, px]) => `${key}:${px}`),
        amber, loud: [...loud],
        selectedEdge: line ? getComputedStyle(line).borderLeftColor : null,
      };
      view?.querySelector(".artifact-compare-close")?.click();
      return seen;
    });
    ok(
      `every line of text in the tab by task is ${TEXT_MIN_PX}px or more — the list, the reader, the evidence, the comparison`,
      rules.small.length === 0 && typeof rules.selectedEdge === "string",
      JSON.stringify(rules.small),
    );
    ok(
      `everything a person presses there is at least ${PRESS_MIN_PX}px high — a task's line, an evidence card, a page, the comparison's controls`,
      rules.short.length === 0 && typeof rules.selectedEdge === "string",
      JSON.stringify(rules.short),
    );
    ok(
      "amber is worn by the selected line's edge and by nothing that is not selected — not by a chip, and not by the boundary of a comparison",
      rules.selectedEdge === rules.amber && rules.loud.length === 0,
      JSON.stringify({ amber: rules.amber, edge: rules.selectedEdge, loud: rules.loud }),
    );

    /* Keyboard: the arrows walk the lines, Tab walks into the reader's new controls, each shows its focus. */
    const reach = await page.evaluate(() => {
      const view = artifactsView();
      view?.querySelector(".artifacts-tasks")?.focus();
      return Boolean(view?.querySelector(".artifacts-tasks"));
    });
    if (reach) await page.keyboard.press("ArrowDown");
    const walkedTo = await page.evaluate(async () => {
      await new Promise((done) => setTimeout(done, 250));
      return {
        open: typeof artifactTaskOpen === "undefined" ? "undefined" : artifactTaskOpen,
        focus: document.activeElement?.className ?? null,
      };
    });
    if (reach) await page.keyboard.press("ArrowUp");
    await page.evaluate(() => new Promise((done) => setTimeout(done, 250)));
    // Every stop is written down, the ones outside the tab marked with `!`. The walk is as long as the
    // reader's controls (the head, the actions, the outline, every evidence card) and ends when both
    // wanted kinds were reached or when the focus has left the tab after being in it.
    const TAB_WALK_MAX = 40;
    const wantedKinds = ["artifact-evidence", "artifact-bundle-page"];
    const walked = [];
    if (reach) {
      for (let at = 0; at < TAB_WALK_MAX; at += 1) {
        await page.keyboard.press("Tab");
        const stop = await page.evaluate(() => {
          const node = document.activeElement;
          const style = node ? getComputedStyle(node) : null;
          return {
            name: String(node?.className || node?.tagName || "").split(" ")[0] || null,
            inside: Boolean(node?.closest(".artifacts-view")),
            ring: style !== null && ((style.outlineStyle !== "none" && parseFloat(style.outlineWidth) > 0) || style.boxShadow !== "none"),
          };
        });
        walked.push(stop);
        const inside = walked.filter((held) => held.inside);
        if (wantedKinds.every((one) => inside.some((held) => held.name === one && held.ring))) break;
        if (!stop.inside && inside.length > 0) break;
      }
    }
    const reached = walked.filter((held) => held.inside);
    ok(
      "the arrows walk the lines and open each; Tab from the list reaches the reader's evidence cards and the task's pages, and each shows its focus",
      walkedTo.open === "t-502" && String(walkedTo.focus).includes("artifacts-tasks")
        && wantedKinds.every((one) => reached.some((held) => held.name === one && held.ring)),
      JSON.stringify({ walkedTo, path: walked.map((held) => `${held.inside ? "" : "!"}${held.name}${held.ring ? "" : " (no ring)"}`) }),
    );
    await page.emulateMedia({ reducedMotion: "reduce" });
    const still = await page.evaluate(() => {
      const view = artifactsView();
      const moving = [];
      const read = [".artifact-task", ".artifact-evidence", ".artifact-bundle-page", ".artifact-compare-stage", ".artifact-compare-handle", ".artifacts-tasks"];
      let found = 0;
      for (const selector of read) {
        const node = view?.querySelector(selector);
        if (!node) continue;
        found += 1;
        const style = getComputedStyle(node);
        const seconds = `${style.transitionDuration},${style.animationDuration}`.split(",").map((one) => parseFloat(one));
        if (seconds.some((one) => one > 0) || style.scrollBehavior === "smooth") moving.push(selector);
      }
      return { moving, found, wanted: read.length };
    });
    ok(
      "with reduced motion asked for, nothing in the list, the evidence or the comparison animates",
      still.found === still.wanted && still.moving.length === 0,
      JSON.stringify(still),
    );
    await page.emulateMedia({ reducedMotion: "no-preference" });

    /* ---- the two widths where the layout changes ------------------------------------------------ */
    await page.setViewportSize(WIDE_WINDOW);
    await page.evaluate(() => new Promise((done) => setTimeout(done, 250)));
    await openTask(page, "t-501");
    const wide = await page.evaluate(() => {
      const view = artifactsView();
      const shown = (node) => Boolean(node) && !node.closest("[hidden]") && node.getClientRects().length > 0;
      const drawer = view?.querySelector(".artifacts-drawer") ?? null;
      const evidence = drawer?.querySelector(".artifact-bundle-evidence") ?? null;
      const body = drawer?.querySelector(".artifact-preview") ?? null;
      const cards = [...(evidence?.querySelectorAll(".artifact-evidence") ?? [])].filter(shown);
      const lefts = new Set(cards.map((one) => Math.round(one.getBoundingClientRect().left)));
      return {
        bodyWidth: Math.round(view?.querySelector(".artifacts-body")?.getBoundingClientRect().width ?? 0),
        rail: view?.classList.contains("is-rail") ?? null,
        beside: evidence && body ? evidence.getBoundingClientRect().left >= body.getBoundingClientRect().right - 1 : null,
        column: cards.length > 1 && lefts.size === 1,
        list: Math.round(view?.querySelector(".artifacts-main")?.getBoundingClientRect().width ?? 0),
      };
    });
    ok(
      `in a tab ${RAIL_TIER}px wide or more the evidence stands as a column beside the report, and the list stays a strip`,
      wide.bodyWidth >= RAIL_TIER && wide.rail === true && wide.beside === true && wide.column === true && Math.abs(wide.list - STRIP_WIDTH) <= 4,
      JSON.stringify(wide),
    );
    await page.setViewportSize(NARROW_WINDOW);
    await page.evaluate(() => new Promise((done) => setTimeout(done, 250)));
    const narrow = await page.evaluate(async () => {
      const view = artifactsView();
      const shown = (node) => Boolean(node) && !node.closest("[hidden]") && node.getClientRects().length > 0;
      const drawer = view?.querySelector(".artifacts-drawer") ?? null;
      const back = drawer?.querySelector(".artifact-detail-back") ?? null;
      const seen = {
        bodyWidth: Math.round(view?.querySelector(".artifacts-body")?.getBoundingClientRect().width ?? 0),
        covers: drawer ? Math.round(drawer.getBoundingClientRect().width) : 0,
        back: shown(back) ? back.textContent : null,
        backHeight: back ? Math.round(back.getBoundingClientRect().height) : 0,
      };
      back?.click();
      await window.__PAINTED__();
      seen.openAfter = typeof artifactTaskOpen === "undefined" ? "undefined" : artifactTaskOpen;
      seen.listShown = shown(view?.querySelector(".artifact-task"));
      seen.readerShown = shown(drawer?.querySelector(".artifacts-detail"));
      return seen;
    });
    ok(
      "in a tab too narrow for two panes the reader covers the list, and 「목록」 in its head gives the list back",
      narrow.bodyWidth > 0 && narrow.bodyWidth < 600 && narrow.covers >= narrow.bodyWidth - 2 && narrow.back === "목록"
        && narrow.backHeight >= PRESS_MIN_PX && narrow.openAfter === null && narrow.listShown === true && narrow.readerShown === false,
      JSON.stringify(narrow),
    );
    await page.setViewportSize(REAL_WINDOW);

    /* ---- a line is a pooled node: walking the list builds no more of them ---------------------- */
    const pooled = await page.evaluate(async () => {
      const view = artifactsView();
      const tasks = view?.querySelector(".artifacts-tasks") ?? null;
      if (!tasks || typeof artifactTaskRowCreations !== "number") return null;
      await window.__PAINTED__();
      const before = artifactTaskRowCreations;
      for (let round = 0; round < 6; round += 1) {
        tasks.scrollTop = round % 2 === 0 ? tasks.scrollHeight : 0;
        await window.__PAINTED__();
      }
      const lastShown = [...view.querySelectorAll(".artifact-task")].map((one) => one.dataset.task);
      tasks.scrollTop = tasks.scrollHeight;
      await window.__PAINTED__();
      return {
        made: artifactTaskRowCreations - before,
        nodes: view.querySelectorAll(".artifact-task").length,
        bottom: [...view.querySelectorAll(".artifact-task")].some((one) => one.dataset.task === `t-${700 + window.__TASKS__.lines - 5}`),
        top: lastShown.includes("t-501"),
      };
    });
    ok(
      "scrolling the list to its end and back builds no new line: the nodes are a pool, and the last task is reached",
      pooled !== null && pooled.made <= 2 && pooled.nodes < fixture.lines / 2 && pooled.bottom === true && pooled.top === true,
      JSON.stringify(pooled),
    );

    /* The list is painted on every scroll event: lines that did not change are not written into. */
    const quiet = await page.evaluate(async () => {
      const view = artifactsView();
      const rows = view?.querySelector(".artifacts-task-rows") ?? null;
      if (!rows || typeof paintArtifactTaskRows !== "function") return null;
      view.querySelector(".artifacts-tasks").scrollTop = 0;
      await window.__PAINTED__();
      const written = [];
      const watch = new MutationObserver((records) => written.push(...records.map((one) => one.attributeName ?? one.type)));
      watch.observe(rows, { subtree: true, childList: true, characterData: true, attributes: true });
      for (let at = 0; at < 3; at += 1) paintArtifactTaskRows(view);
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      written.push(...watch.takeRecords().map((one) => one.attributeName ?? one.type));
      watch.disconnect();
      return { lines: rows.querySelectorAll(".artifact-task").length, writes: written.length, written: written.slice(0, 8) };
    });
    ok(
      "repainting lines that did not change writes nothing into them",
      quiet !== null && quiet.lines > 0 && quiet.writes === 0,
      JSON.stringify(quiet),
    );

    const catalogs = await page.evaluate((keys) => {
      const missing = [];
      for (const language of ["en", "ja", "zh", "es"]) {
        for (const key of keys) {
          if (typeof CATALOG?.[language]?.[key] !== "string" || CATALOG[language][key].trim() === "") missing.push(`${language}:${key}`);
        }
      }
      return missing;
    }, NEW_KEYS);
    ok(
      "every new word of the tab by task stands in the four translated catalogs beside its Korean",
      catalogs.length === 0,
      JSON.stringify(catalogs.slice(0, 12)) + (catalogs.length > 12 ? ` … ${catalogs.length}` : ""),
    );
    ok("the window raised no errors", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}
