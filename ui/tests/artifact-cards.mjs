/* The Artifacts tab reads as one task (t-36910, stage 1): the cards and the drawer, on the rows the
 * catalog already holds.
 *
 * What is measured here is what a person sees at the tab's real width — 809px in a 1440 window with
 * both side panels open, measured in this harness and pinned below — with a backend that answers as
 * the runtime does: it leaves out the rows whose files are gone when it is asked for present rows, it
 * counts them by kind, and a preview of an evidence file carries its digest.
 *
 * Everything in the fixture is synthetic. Every read of the page is defensive (`?.`, `?? null`): on a
 * tree that has not built the feature the checks fail at their assertions, one line each, and the
 * suite still runs to its end. */
import { openWindowTestPage } from "./window-boot.mjs";

/* The window a person has open: 1440 wide, both side panels standing. */
const REAL_WINDOW = Object.freeze({ width: 1440, height: 900 });
/* The tab's width in that window, as measured in this harness on main (51789dbc6), and how far a
 * later change of the side panels may move it before this file is looked at again. */
const REAL_TAB_WIDTH = 809;
const REAL_TAB_SLACK = 24;
/* The drawer the design asks for, and how far a scrollbar or a rounded pixel may move it. */
const DRAWER_WIDTH = 560;
const DRAWER_SLACK = 16;
const BAR_HEIGHT = 40;
const TEXT_MIN_PX = 12;
const PRESS_MIN_PX = 28;

/* The words a card or the drawer says, by catalog key — every one of them must stand in the four
 * translated catalogs (Korean is the word the code carries). */
const NEW_KEYS = Object.freeze([
  "artifacts.subtype.report", "artifacts.subtype.review", "artifacts.subtype.brief",
  "artifacts.subtype.handover", "artifacts.subtype.proposal",
  "artifacts.pick.task", "artifacts.task.unlinked", "artifacts.task.filterTip", "artifacts.task.emptied",
  "artifacts.detail.more", "artifacts.detail.when", "artifacts.meta.landing",
  "artifacts.attempts.label", "artifacts.attempts.final", "artifacts.attempts.nth",
  "artifacts.outline.toggle", "artifacts.outline.label",
  "artifacts.status.pass", "artifacts.status.fail",
  "artifacts.digest.steps", "artifacts.digest.fold", "artifacts.digest.foldFailed", "artifacts.digest.cut",
  "artifacts.digest.skipped", "artifacts.digest.raw", "artifacts.digest.others",
  "artifacts.state.actions", "artifacts.state.last", "artifacts.state.failures", "artifacts.state.stuck",
  "artifacts.state.moving", "artifacts.state.unchanged", "artifacts.state.recent",
  "artifacts.facts.records", "artifacts.facts.list", "artifacts.facts.record", "artifacts.facts.nothing",
  "artifacts.facts.yes", "artifacts.facts.no", "artifacts.facts.more",
]);

/* The synthetic catalog, built in the page so the fake backend and the checks read one set of rows. */
function standCatalog(page) {
  return page.evaluate(async () => {
    const now = Date.now();
    const hour = 60 * 60 * 1000;
    const kept = "/tmp/zerocode-window-test/wt-kept";
    const row = (id, kind, at, fields = {}) => ({
      id, kind, title: fields.title ?? id, path: fields.path ?? `/data/artifacts/run/${id}/${id}.md`,
      bytes: fields.bytes ?? 4096, created_ms: now - at * hour, modified_ms: now - at * hour,
      origin: fields.origin ?? {}, tags: [], preview: fields.preview ?? { kind: "none" },
      source: fields.source ?? "worker_report",
      ...(fields.subtype ? { subtype: fields.subtype } : {}),
      ...(fields.version ? { version: fields.version, url: `file://${fields.path}` } : {}),
    });
    const worker = (task, id, more = {}) => ({
      run: "run-9", task, worker: id, pane: `team-1/%${id.slice(2)}`, agent: "claude", model: "claude-sonnet-5-5",
      work_summary: "큐 닫기", ...more,
    });
    const session = "/tmp/zerocode-window-test/computer-use/sessions/one";
    const evidence = (id, name, kind = "evidence") => row(id, kind, 3, {
      title: name, path: `${session}/${name}`, origin: { automation: "computer-use" }, source: "evidence",
    });
    const rows = [
      row("r-final", "report", 1, {
        title: "큐 닫기 재시도 — 닫을 때 남은 일을 비운다", subtype: "report", bytes: 7_300,
        origin: worker("t-501", "w-12", { worktree: kept }),
      }),
      row("r-first", "report", 5, {
        title: "큐 닫기 — 진행 보고", subtype: "report", origin: worker("t-501", "w-11", { worktree: kept }),
      }),
      row("r-review", "report", 2, {
        title: "검증 보고 — 큐가 비워지는가", subtype: "review",
        origin: worker("t-502", "w-13", { agent: "codex", model: "gpt-6-astra", worktree: "/tmp/zerocode-window-test/wt-gone" }),
      }),
      row("r-brief", "report", 4, { title: "다음 작업 브리프", subtype: "brief" }),
      row("r-old", "report", 6, { title: "옛 빌드가 적은 보고서", origin: worker("t-503", "w-14") }),
      row("p-linked", "page", 1, {
        title: "큐 닫기 전후 그림", path: "/data/artifacts/pages/p-linked/index.html", version: 2, source: "manual",
        origin: { pane: "term-4", agent: "zo", model: "claude-opus-5-5", run: "run-9", worker: "w-12", task: "t-501" },
      }),
      row("p-loose", "page", 2, {
        title: "누구의 작업도 아닌 페이지", path: "/data/artifacts/pages/p-loose/index.html", version: 1, source: "manual",
      }),
      row("d-loose", "document", 3, {
        title: "설계 메모", path: "/tmp/zerocode-window-test/project/notes.md", source: "agent_page",
        origin: { agent: "claude", session: "s-1", project: "/tmp/zerocode-window-test/project" },
        preview: { kind: "markdown", text: "# 설계 메모\n\n첫 문단" },
      }),
      evidence("e-steps", "steps.jsonl"),
      evidence("e-state", "state.json"),
      evidence("e-log", "run.log"),
      evidence("s-shot", "001.png", "screenshot"),
      evidence("g-steps", "gone-steps.jsonl"),
      evidence("g-shot-1", "gone-001.png", "screenshot"),
      evidence("g-shot-2", "gone-002.png", "screenshot"),
    ];
    const gone = new Set(["g-steps", "g-shot-1", "g-shot-2"]);
    const count = (held, pick) => {
      const counted = {};
      for (const one of held) if (pick(one)) counted[one.kind] = (counted[one.kind] ?? 0) + 1;
      return counted;
    };
    const seen = (window.__CARDS__ = { asks: [], previews: [] });
    // The runtime's listing, in small: `present` leaves the vanished rows out and they are counted by
    // kind either way; the rows no task is linked to are counted by kind.
    window.__ANSWER__.artifacts_list = (args) => {
      const filter = args?.filter ?? {};
      seen.asks.push(JSON.parse(JSON.stringify(filter)));
      const held = rows.filter((one) => filter.present !== true || !gone.has(one.id));
      const missing = held.filter((one) => gone.has(one.id)).map((one) => one.id);
      return {
        rows: held, total: held.length, truncated: false, missing, missing_total: gone.size,
        missing_by_kind: count(rows, (one) => gone.has(one.id)),
        unlinked_by_kind: count(rows, (one) => !gone.has(one.id) && !one.origin.task),
        by_kind: count(held, () => true),
        retention_days: 30, thumb: { width: 320, height: 240, queue_max: 2 },
      };
    };
    window.__ANSWER__.artifact_counts = () => ({
      total: rows.length, by_worker: {}, by_task: {}, by_run: {}, by_worktree: {}, by_automation: {}, report_by_worker: {},
    });
    window.__ANSWER__.artifact_thumbnail = () => ({ data_url: null, cached: false });
    // A report long enough to need an outline: eight sections under its title, each several screens
    // of the drawer's lines when taken together.
    const sections = ["결과", "커밋", "고친 것", "빨강과 초록", "미리 돌려 본 것", "돌리지 않은 것", "알아 둘 것", "증거 파일"];
    const paragraph = "닫을 때 큐에 남은 일을 비우고, 비운 수를 돌려준다. 호출한 쪽은 그 수를 기록에 남긴다. ";
    const long = [`# ${rows[0].title}`, "", "한 줄 요약: 닫을 때 남은 일을 비운다.", ""]
      .concat(sections.flatMap((name, at) => [`## ${at + 1}. ${name}`, "", paragraph.repeat(9), "", `### ${name}의 세부`, "", paragraph.repeat(6), ""]))
      .join("\n");
    const step = (n, verb, ok, more = {}) => ({
      row: "step", n, at_ms: now - 3 * hour + n * 1000, tool: "browser", verb, target: "browser-1 the go button", ok, ms: 4 + n, ...more,
    });
    const previews = {
      "r-final": { kind: "markdown", text: long, bytes: long.length, truncated: false },
      "r-first": { kind: "markdown", text: "# 큐 닫기 — 진행 보고\n\n아직 짓지 않았다.\n\n## 다음\n\n시험을 쓴다.", bytes: 80, truncated: false },
      "e-steps": {
        kind: "text", text: '{"n":1,"verb":"open","ok":true}\n{"n":2,"verb":"click","ok":true}\n', bytes: 9_000, truncated: false,
        digest: {
          kind: "steps", total: 62, failed: 2, skipped: 1, cut: false,
          started_ms: now - 3 * hour, ended_ms: now - 3 * hour + 432_000,
          verbs: [{ verb: "click", steps: 40 }, { verb: "wait", steps: 12 }], other_verbs: 10,
          rows: [
            step(1, "open", true), step(2, "click", true),
            { row: "fold", steps: 2446, failed: 0 },
            step(2449, "click", true),
            step(2450, "wait", false, { error: "the selector never showed", code: "timeout" }),
            step(2451, "click", true),
            { row: "fold", steps: 9, failed: 1 },
            step(2461, "close", true, { framed: true }),
          ],
        },
      },
      "e-state": {
        kind: "text", text: '{"actions":3,"lastVerb":"click","lastOk":false}', bytes: 300, truncated: false,
        digest: {
          kind: "state", actions: 3, last_verb: "click", last_ok: false, last_error: "the selector never showed", last_at_ms: now - hour,
          consecutive_failures: 2, unchanged_looks: 4, stuck: true, recent: ["look", "click browser-1"],
        },
      },
      "e-log": { kind: "text", text: "test result: ok. 3 passed; 0 failed\n", bytes: 40, truncated: false },
    };
    window.__ANSWER__.artifact_preview = (args) => {
      seen.previews.push(args.id);
      return previews[args.id] ?? { kind: "none", bytes: 0, truncated: false };
    };
    // The checkout the window still holds, as git last answered for it: in the compare ref. It
    // rides the project catalog, the way the runtime sends it, so a later re-read keeps it.
    const catalog = window.__ANSWER__.project_catalog;
    window.__ANSWER__.project_catalog = () => {
      const projects = catalog();
      projects[0]?.worktrees.push({
        path: kept, branch: "wt/t-501", is_main: false, active: false, is_folder: false,
        ownership: "zerocode-managed", external_hidden: false,
        landing: {
          state: "landed", detached: false, ahead: 0, dirty: false, ignored: false, compare_ref: "origin/main",
          ref_updated_ms: now - hour, landed_in: { sha: "0123456789abcdef0123456789abcdef01234567", time_ms: now - 2 * hour },
        },
      });
      return projects;
    };
    if (typeof refreshWorktrees === "function") await refreshWorktrees();
    return { sections: sections.length, headings: 1 + sections.length * 2 };
  });
}

/* Open the tab fresh, as a person does from the sidebar. */
async function openTab(page) {
  await page.evaluate(async () => {
    dropTab("artifacts");
    artifactFilter.tab = "pages";
    artifactFilter.origin = null;
    artifactFilter.showMissing = false;
    artifactTabPicked = false;
    artifactSelectedId = null;
    el("nav-artifacts").click();
    await window.__PAINTED__();
    await new Promise((done) => setTimeout(done, 200));
  });
}

export async function testArtifactCards(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.setViewportSize(REAL_WINDOW);
    const fixture = await standCatalog(page);
    await openTab(page);

    /* ---- the width the design stands on, and the vanished rows (item 3) ------------------- */
    const first = await page.evaluate(async () => {
      const view = artifactsView();
      const tab = (name) => view?.querySelector(`[data-artifact-tab="${name}"]`) ?? null;
      const seen = { width: view ? Math.round(view.getBoundingClientRect().width) : null };
      seen.firstAsk = window.__CARDS__.asks[0] ?? null;
      seen.heldGone = ["g-steps", "g-shot-1", "g-shot-2"].filter((id) => artifactRows.has(id));
      seen.tabs = Object.fromEntries(["pages", "reports", "evidence"].map((name) => [
        name, tab(name)?.querySelector(".artifacts-tab-count")?.textContent ?? null,
      ]));
      tab("evidence")?.click();
      await new Promise((done) => setTimeout(done, 120));
      const line = view?.querySelector(".artifacts-missing");
      seen.hiddenLine = line && !line.hidden ? line.querySelector(".artifacts-missing-word")?.textContent ?? "" : null;
      const asksBefore = window.__CARDS__.asks.length;
      view?.querySelector(".artifacts-missing-toggle")?.click();
      await new Promise((done) => setTimeout(done, 250));
      seen.showAsks = window.__CARDS__.asks.length - asksBefore;
      seen.showAsk = window.__CARDS__.asks.at(-1) ?? null;
      seen.shownGone = ["g-steps", "g-shot-1", "g-shot-2"].filter((id) => artifactOrder.includes(id));
      seen.shownLine = line && !line.hidden ? line.querySelector(".artifacts-missing-word")?.textContent ?? "" : null;
      view?.querySelector(".artifacts-missing-toggle")?.click();
      await new Promise((done) => setTimeout(done, 250));
      seen.hideAsk = window.__CARDS__.asks.at(-1) ?? null;
      seen.heldGoneAfter = ["g-steps", "g-shot-1", "g-shot-2"].filter((id) => artifactRows.has(id));
      return seen;
    });
    ok(
      `the tab is the width the design stands on: about ${REAL_TAB_WIDTH}px in a 1440 window with both side panels open`,
      first.width !== null && Math.abs(first.width - REAL_TAB_WIDTH) <= REAL_TAB_SLACK,
      JSON.stringify({ width: first.width }),
    );
    ok(
      "the window asks for the rows whose files are there: its first ask says so, no vanished row is held, and each tab counts what stands",
      first.firstAsk?.present === true && first.heldGone.length === 0
        && first.tabs.pages === "3" && first.tabs.reports === "5" && first.tabs.evidence === "4",
      JSON.stringify({ firstAsk: first.firstAsk, heldGone: first.heldGone, tabs: first.tabs }),
    );
    ok(
      "the line under the list still says how many vanished rows the tab hides — counted by the runtime, not from rows the window holds",
      typeof first.hiddenLine === "string" && first.hiddenLine.includes("3"),
      JSON.stringify({ hiddenLine: first.hiddenLine }),
    );
    ok(
      "「보기」 asks once more, without `present`, and the vanished rows stand in the list; 「숨기기」 asks for present rows again and they are gone",
      first.showAsks === 1 && first.showAsk?.present !== true && first.shownGone.length === 3
        && typeof first.shownLine === "string" && first.shownLine.includes("3")
        && first.hideAsk?.present === true && first.heldGoneAfter.length === 0,
      JSON.stringify({
        showAsks: first.showAsks, showAsk: first.showAsk, shownGone: first.shownGone, shownLine: first.shownLine,
        hideAsk: first.hideAsk, heldGoneAfter: first.heldGoneAfter,
      }),
    );

    /* ---- the card: subtype, task, landed (items 1 and 4) ------------------------------------ */
    const cards = await page.evaluate(async () => {
      const view = artifactsView();
      view?.querySelector('[data-artifact-tab="reports"]')?.click();
      await new Promise((done) => setTimeout(done, 150));
      const card = (id) => view?.querySelector(`.artifact-card[data-id="${id}"]`) ?? null;
      const shown = (node) => Boolean(node) && !node.hidden && node.getClientRects().length > 0;
      const text = (node) => (shown(node) ? node.textContent.trim() : null);
      const read = (id) => {
        const node = card(id);
        const task = node?.querySelector(".artifact-card-task") ?? null;
        const landing = node?.querySelector(".artifact-card-landing") ?? null;
        return {
          subtype: text(node?.querySelector(".artifact-card-subtype")),
          task: text(task),
          taskTag: task?.tagName ?? null,
          taskTabIndex: task?.tabIndex ?? null,
          taskBorder: shown(task) ? getComputedStyle(task).borderTopColor : null,
          landing: text(landing),
          landingTip: shown(landing) ? landing.dataset.tip ?? "" : null,
          title: text(node?.querySelector(".artifact-card-title")),
          faceShown: shown(node?.querySelector(".artifact-card-face")),
          height: node ? Math.round(node.getBoundingClientRect().height) : null,
        };
      };
      // The two colours a chip's outline is weighed against: the tab's neutral chip edge, and the
      // amber the selected card wears.
      const probe = document.createElement("span");
      probe.style.color = "var(--artifact-selected-edge)";
      view?.appendChild(probe);
      const selectedEdge = getComputedStyle(probe).color;
      probe.style.color = "var(--artifact-chip-edge, transparent)";
      const rule = getComputedStyle(probe).color;
      probe.remove();
      return {
        final: read("r-final"), review: read("r-review"), brief: read("r-brief"), old: read("r-old"),
        selectedEdge, rule,
      };
    });
    ok(
      "a report card wears its kind of report as a chip — 보고서 · 리뷰 · 브리프 — and a row that says none wears 보고서",
      cards.final.subtype === "보고서" && cards.review.subtype === "리뷰" && cards.brief.subtype === "브리프" && cards.old.subtype === "보고서",
      JSON.stringify({ final: cards.final.subtype, review: cards.review.subtype, brief: cards.brief.subtype, old: cards.old.subtype }),
    );
    ok(
      "a card says its task in a chip with a neutral outline — a button that is outside the tab order — and a row with no task wears none",
      cards.final.task === "t-501" && cards.review.task === "t-502" && cards.brief.task === null
        && cards.final.taskTag === "BUTTON" && cards.final.taskTabIndex === -1
        && cards.final.taskBorder === cards.rule && cards.final.taskBorder !== cards.selectedEdge,
      JSON.stringify({
        final: cards.final.task, review: cards.review.task, brief: cards.brief.task, tag: cards.final.taskTag,
        tabIndex: cards.final.taskTabIndex, border: cards.final.taskBorder, rule: cards.rule, selected: cards.selectedEdge,
      }),
    );
    ok(
      "a card whose checkout the window still holds says its landed state — 반영됨 on the chip, the commit in its tooltip — and a card whose checkout is gone says nothing",
      cards.final.landing === "반영됨" && (cards.final.landingTip ?? "").includes("012345678")
        && cards.review.landing === null && cards.brief.landing === null,
      JSON.stringify({ final: cards.final.landing, tip: cards.final.landingTip, review: cards.review.landing, brief: cards.brief.landing }),
    );
    ok(
      "a report card is its words, not a glyph two thirds of its height: no icon face, and the title is the report's own",
      cards.final.faceShown === false && cards.final.title === "큐 닫기 재시도 — 닫을 때 남은 일을 비운다",
      JSON.stringify({ face: cards.final.faceShown, title: cards.final.title, height: cards.final.height }),
    );

    /* ---- the drawer: one head row, details folded, the body flowing (item 4) ---------------- */
    const drawer = await page.evaluate(async () => {
      const view = artifactsView();
      selectArtifact(view, "r-final", { reveal: true });
      await window.__PAINTED__();
      await new Promise((done) => setTimeout(done, 350));
      const pane = view?.querySelector(".artifacts-drawer") ?? null;
      const shown = (node) => Boolean(node) && !node.hidden && node.getClientRects().length > 0;
      const text = (node) => (shown(node) ? node.textContent.trim().replace(/\s+/g, " ") : null);
      const bar = pane?.querySelector(".artifact-detail-bar") ?? null;
      const box = (node) => (shown(node) ? node.getBoundingClientRect() : null);
      const seen = {};
      seen.width = pane ? Math.round(pane.getBoundingClientRect().width) : null;
      seen.grid = Math.round(view?.querySelector(".artifacts-grid")?.getBoundingClientRect().width ?? 0);
      seen.barHeight = box(bar) ? Math.round(box(bar).height) : null;
      seen.barParts = bar ? [...bar.children].filter(shown).map((one) => String(one.className).split(" ")[0]) : [];
      seen.task = text(bar?.querySelector(".artifact-detail-task"));
      seen.landing = text(bar?.querySelector(".artifact-detail-landing"));
      seen.when = text(bar?.querySelector(".artifact-detail-when"));
      seen.maker = text(bar?.querySelector(".artifact-detail-maker"));
      const attempts = bar?.querySelector(".artifact-detail-attempts") ?? null;
      seen.attempts = shown(attempts) ? [...attempts.options].map((one) => one.textContent.trim()) : null;
      seen.attemptsTag = attempts?.tagName ?? null;
      // What stands behind 「세부 정보」: closed first, and then it names the run, the worker and the size.
      const more = pane?.querySelector(".artifact-detail-more") ?? null;
      const facts = pane?.querySelector(".artifact-detail-facts") ?? null;
      seen.moreWord = text(more);
      seen.factsClosed = more?.getAttribute("aria-expanded") === "false" && !shown(facts);
      seen.metaShownClosed = [...(pane?.querySelectorAll(".artifact-meta dt") ?? [])].filter(shown).length;
      more?.click();
      await new Promise((done) => setTimeout(done, 60));
      seen.factsOpen = more?.getAttribute("aria-expanded") === "true" && shown(facts);
      seen.facts = text(facts) ?? "";
      more?.click();
      await new Promise((done) => setTimeout(done, 60));
      // The title once; the body with the pane, no box of its own.
      const title = "큐 닫기 재시도 — 닫을 때 남은 일을 비운다";
      seen.titles = [...(pane?.querySelectorAll("h1, h2, h3") ?? [])].filter((one) => shown(one) && one.textContent.trim() === title).length;
      const preview = pane?.querySelector(".artifact-preview") ?? null;
      const style = preview ? getComputedStyle(preview) : null;
      seen.previewMaxHeight = style?.maxHeight ?? null;
      seen.previewOverflow = style?.overflowY ?? null;
      seen.drawerScrolls = pane ? pane.scrollHeight > pane.clientHeight + 200 : null;
      const md = pane?.querySelector(".artifact-preview-md") ?? null;
      seen.readWidth = md ? Math.round(md.getBoundingClientRect().width) : null;
      seen.readPx = md ? parseFloat(getComputedStyle(md).fontSize) : null;
      const sticky = (node) => (node ? getComputedStyle(node).position : null);
      seen.barSticky = sticky(bar);
      return seen;
    });
    ok(
      `the drawer is about ${DRAWER_WIDTH}px at the real width and the list keeps a column of cards beside it`,
      drawer.width !== null && Math.abs(drawer.width - DRAWER_WIDTH) <= DRAWER_SLACK && drawer.grid >= 200,
      JSON.stringify({ drawer: drawer.width, grid: drawer.grid }),
    );
    ok(
      `the drawer's head is one row ${BAR_HEIGHT}px high that stays in view: task · landed · what and when · agent and model · the task's reports`,
      drawer.barHeight === BAR_HEIGHT && drawer.barSticky === "sticky"
        && drawer.task === "t-501" && drawer.landing === "반영됨"
        && typeof drawer.when === "string" && drawer.when.startsWith("보고서 ")
        && drawer.maker === "claude sonnet-5-5",
      JSON.stringify({
        height: drawer.barHeight, sticky: drawer.barSticky, parts: drawer.barParts, task: drawer.task,
        landing: drawer.landing, when: drawer.when, maker: drawer.maker,
      }),
    );
    ok(
      "the reports of one task are one select in the head, the newest first: the final report of two, then the first attempt with its time",
      drawer.attemptsTag === "SELECT" && Array.isArray(drawer.attempts) && drawer.attempts.length === 2
        && drawer.attempts[0].includes("최종 보고") && drawer.attempts[0].includes("2")
        && drawer.attempts[1].includes("1번째 시도"),
      JSON.stringify({ attempts: drawer.attempts }),
    );
    ok(
      "the run, the worker, the size and the commit stand behind 「세부 정보」: folded first, and one press shows them",
      drawer.moreWord === "세부 정보" && drawer.factsClosed === true && drawer.metaShownClosed === 0 && drawer.factsOpen === true
        && drawer.facts.includes("run-9") && drawer.facts.includes("w-12") && drawer.facts.includes("KB")
        && drawer.facts.includes("012345678"),
      JSON.stringify({
        word: drawer.moreWord, closed: drawer.factsClosed, metaShownClosed: drawer.metaShownClosed, open: drawer.factsOpen,
        facts: drawer.facts.slice(0, 300),
      }),
    );
    ok(
      "the report's title is shown once, and its body flows with the drawer: no height cap and no scroll box of its own",
      drawer.titles === 1 && drawer.previewMaxHeight === "none" && drawer.previewOverflow === "visible" && drawer.drawerScrolls === true,
      JSON.stringify({
        titles: drawer.titles, maxHeight: drawer.previewMaxHeight, overflow: drawer.previewOverflow, drawerScrolls: drawer.drawerScrolls,
        readWidth: drawer.readWidth, readPx: drawer.readPx,
        hangulPerLine: drawer.readWidth && drawer.readPx ? Math.floor(drawer.readWidth / drawer.readPx) : null,
      }),
    );

    /* ---- the outline of a long report (item 4) ---------------------------------------------- */
    const outline = await page.evaluate(async () => {
      const view = artifactsView();
      const pane = view?.querySelector(".artifacts-drawer") ?? null;
      const shown = (node) => Boolean(node) && !node.hidden && node.getClientRects().length > 0;
      const strip = pane?.querySelector(".artifact-outline") ?? null;
      const toggle = strip?.querySelector(".artifact-outline-toggle") ?? null;
      const seen = { shown: shown(strip), sticky: strip ? getComputedStyle(strip).position : null };
      seen.toggleWord = shown(toggle) ? toggle.textContent.trim() : null;
      seen.closed = toggle?.getAttribute("aria-expanded") === "false";
      toggle?.click();
      await new Promise((done) => setTimeout(done, 60));
      const items = [...(strip?.querySelectorAll(".artifact-outline-list button") ?? [])].filter(shown);
      seen.items = items.map((one) => one.textContent.trim());
      seen.itemHeights = items.map((one) => Math.round(one.getBoundingClientRect().height));
      const before = pane?.scrollTop ?? 0;
      items[8]?.click();
      await new Promise((done) => setTimeout(done, 200));
      seen.moved = (pane?.scrollTop ?? 0) - before;
      seen.now = strip?.querySelector(".artifact-outline-now")?.textContent.trim() ?? null;
      seen.closedAfterPick = toggle?.getAttribute("aria-expanded") === "false";
      // A short report of the same task has no outline.
      selectArtifact(view, "r-first", { reveal: true });
      await window.__PAINTED__();
      await new Promise((done) => setTimeout(done, 350));
      seen.shortShown = shown(pane?.querySelector(".artifact-outline"));
      seen.shortWhen = pane?.querySelector(".artifact-detail-when")?.textContent.trim() ?? null;
      return seen;
    });
    ok(
      "a long report gets an outline that stays in view and lists every heading, each a row tall enough to press; picking one goes there and says where the reader is",
      outline.shown === true && outline.sticky === "sticky" && outline.closed === true
        && outline.items.length === fixture.headings
        && outline.items[1] === "1. 결과" && outline.items[8] === "빨강과 초록의 세부"
        && outline.itemHeights.every((height) => height >= PRESS_MIN_PX)
        && outline.moved > 200 && outline.now === "빨강과 초록의 세부" && outline.closedAfterPick === true,
      JSON.stringify({ ...outline, items: outline.items.slice(0, 10), itemHeights: outline.itemHeights.slice(0, 4) }),
    );
    ok(
      "a report under two screens has no outline",
      outline.shortShown === false,
      JSON.stringify({ shortShown: outline.shortShown }),
    );

    /* ---- evidence as steps and plain words (item 4) ----------------------------------------- */
    const evidence = await page.evaluate(async () => {
      const view = artifactsView();
      view?.querySelector('[data-artifact-tab="evidence"]')?.click();
      await new Promise((done) => setTimeout(done, 150));
      const pane = view?.querySelector(".artifacts-drawer") ?? null;
      const shown = (node) => Boolean(node) && !node.hidden && node.getClientRects().length > 0;
      const text = (node) => (shown(node) ? node.textContent.trim().replace(/\s+/g, " ") : null);
      const open = async (id) => {
        selectArtifact(view, id, { reveal: true });
        await window.__PAINTED__();
        await new Promise((done) => setTimeout(done, 300));
        const digest = pane?.querySelector(".artifact-digest") ?? null;
        const raw = pane?.querySelector(".artifact-digest-raw") ?? null;
        const visible = [...(pane?.querySelectorAll("*") ?? [])]
          .filter((one) => shown(one) && !one.closest("details:not([open])") && one.children.length === 0)
          .map((one) => one.textContent).join("\n");
        return {
          digestShown: shown(digest),
          summary: text(digest?.querySelector(".artifact-digest-summary")),
          steps: [...(digest?.querySelectorAll(".artifact-step") ?? [])].filter(shown).map((one) => ({
            ok: one.dataset.ok ?? null,
            words: one.textContent.trim().replace(/\s+/g, " "),
            numberHeight: Math.round(one.querySelector(".artifact-step-n")?.getBoundingClientRect().height ?? 0),
            lineHeight: Math.round(parseFloat(getComputedStyle(one.querySelector(".artifact-step-n") ?? one).lineHeight) || 0),
          })),
          folds: [...(digest?.querySelectorAll(".artifact-step-fold") ?? [])].filter(shown).map((one) => one.textContent.trim().replace(/\s+/g, " ")),
          facts: text(digest?.querySelector(".artifact-digest-facts")),
          rawTag: raw?.tagName ?? null,
          rawOpen: raw?.open ?? null,
          rawWord: text(raw?.querySelector("summary")),
          preShown: shown(pane?.querySelector(".artifact-preview-text")),
          showsJson: visible.includes('{"n"') || visible.includes('"actions"'),
        };
      };
      return { steps: await open("e-steps"), state: await open("e-state"), log: await open("e-log") };
    });
    const failed = evidence.steps.steps.find((one) => one.ok === "false") ?? null;
    const passed = evidence.steps.steps.find((one) => one.ok === "true") ?? null;
    ok(
      "a step log opens as its steps in plain words — how many ran and failed, each step with 통과 or 실패 beside it, the folded runs counted — and never as raw JSON",
      evidence.steps.digestShown === true && evidence.steps.showsJson === false && evidence.steps.preShown === false
        && typeof evidence.steps.summary === "string" && evidence.steps.summary.includes("62") && evidence.steps.summary.includes("2")
        && evidence.steps.steps.length === 6
        && Boolean(failed) && failed.words.includes("✕") && failed.words.includes("실패") && failed.words.includes("the selector never showed")
        && Boolean(passed) && passed.words.includes("✓") && passed.words.includes("통과")
        && evidence.steps.folds.length === 2 && evidence.steps.folds[0].includes("2,446") && evidence.steps.folds[1].includes("9") && evidence.steps.folds[1].includes("1"),
      JSON.stringify({
        shown: evidence.steps.digestShown, json: evidence.steps.showsJson, pre: evidence.steps.preShown, summary: evidence.steps.summary,
        steps: evidence.steps.steps.length, failed: failed?.words ?? null, passed: passed?.words ?? null, folds: evidence.steps.folds,
      }),
    );
    ok(
      "the raw text stands behind 「원문 보기」, closed, for whoever wants it",
      evidence.steps.rawTag === "DETAILS" && evidence.steps.rawOpen === false && evidence.steps.rawWord === "원문 보기",
      JSON.stringify({ tag: evidence.steps.rawTag, open: evidence.steps.rawOpen, word: evidence.steps.rawWord }),
    );
    ok(
      "a step's number is one unbroken piece: it does not wrap in the drawer",
      evidence.steps.steps.length > 0 && evidence.steps.steps.every((one) => one.numberHeight > 0 && one.numberHeight <= one.lineHeight + 2),
      JSON.stringify(evidence.steps.steps.map((one) => [one.numberHeight, one.lineHeight])),
    );
    ok(
      "the operator's state record opens as what it says — how many actions, the last one and how it went, whether it is stuck — in words",
      evidence.state.digestShown === true && evidence.state.showsJson === false
        && typeof evidence.state.summary === "string" && evidence.state.summary.includes("3")
        && typeof evidence.state.facts === "string" && evidence.state.facts.includes("click")
        && evidence.state.facts.includes("실패") && evidence.state.facts.includes("막힘"),
      JSON.stringify({
        shown: evidence.state.digestShown, json: evidence.state.showsJson, summary: evidence.state.summary, facts: evidence.state.facts,
      }),
    );
    ok(
      "a text log that holds no record is shown as the text it is",
      evidence.log.digestShown === false && evidence.log.preShown === true,
      JSON.stringify({ digest: evidence.log.digestShown, pre: evidence.log.preShown }),
    );

    /* ---- a task chip filters every tab (item 5) ---------------------------------------------- */
    const filtered = await page.evaluate(async () => {
      const view = artifactsView();
      const tab = (name) => view?.querySelector(`[data-artifact-tab="${name}"]`) ?? null;
      const shown = (node) => Boolean(node) && !node.hidden && node.getClientRects().length > 0;
      const text = (node) => (shown(node) ? node.textContent.trim().replace(/\s+/g, " ") : null);
      const counts = () => Object.fromEntries(["pages", "reports", "evidence"].map((name) => [
        name, tab(name)?.querySelector(".artifacts-tab-count")?.textContent ?? null,
      ]));
      const settle = () => new Promise((done) => setTimeout(done, 150));
      const seen = { asksBefore: window.__CARDS__.asks.length };
      tab("reports")?.click();
      await settle();
      view?.querySelector('.artifact-card[data-id="r-final"] .artifact-card-task')?.click();
      await settle();
      seen.afterChip = counts();
      seen.asksOnChip = window.__CARDS__.asks.length - seen.asksBefore;
      seen.chip = text(view?.querySelector(".artifacts-origin-chip"));
      const pick = view?.querySelector('[data-artifact-pick="task"]') ?? null;
      seen.pickTag = pick?.tagName ?? null;
      seen.pickValue = pick?.value ?? null;
      seen.pickOptions = pick ? [...pick.options].map((one) => one.textContent.trim()) : [];
      seen.reportIds = [...artifactOrder];
      // The evidence tab has nothing of this task: it says what is not linked instead of nothing.
      tab("evidence")?.click();
      await settle();
      const line = view?.querySelector(".artifacts-unlinked") ?? null;
      seen.emptiedLine = text(line);
      seen.emptyBlock = shown(view?.querySelector(".artifacts-empty"));
      const show = line?.querySelector("button") ?? null;
      seen.showTag = show?.tagName ?? null;
      seen.showHeight = shown(show) ? Math.round(show.getBoundingClientRect().height) : null;
      show?.click();
      await settle();
      seen.unlinkedIds = [...artifactOrder].sort();
      seen.unlinkedChip = text(view?.querySelector(".artifacts-origin-chip"));
      seen.unlinkedCounts = counts();
      // The same filter from the keyboard's road: the head's select, with the drawer never opened.
      if (pick) {
        pick.value = "t-502";
        pick.dispatchEvent(new Event("change", { bubbles: true }));
      }
      await settle();
      seen.afterPick = counts();
      seen.pickChip = text(view?.querySelector(".artifacts-origin-chip"));
      view?.querySelector(".artifacts-origin-clear")?.click();
      await settle();
      seen.afterClear = counts();
      seen.pickAfterClear = pick?.value ?? null;
      return seen;
    });
    ok(
      "pressing a card's task chip filters every tab to that task without asking the runtime, and each tab says its count",
      filtered.afterChip.pages === "1" && filtered.afterChip.reports === "2" && filtered.afterChip.evidence === "0"
        && filtered.asksOnChip === 0 && typeof filtered.chip === "string" && filtered.chip.includes("t-501")
        && filtered.reportIds.join(",") === "r-final,r-first",
      JSON.stringify({ counts: filtered.afterChip, asks: filtered.asksOnChip, chip: filtered.chip, rows: filtered.reportIds }),
    );
    ok(
      "a tab the task emptied says what is not linked — 「작업에 연결 안 된 것 4」 with 「보기」 — instead of an empty list, and 「보기」 shows those rows",
      typeof filtered.emptiedLine === "string" && filtered.emptiedLine.includes("0") && filtered.emptiedLine.includes("4")
        && filtered.emptiedLine.includes("작업에 연결 안 된 것") && filtered.emptyBlock === false
        && filtered.showTag === "BUTTON" && filtered.showHeight >= PRESS_MIN_PX
        && filtered.unlinkedIds.join(",") === "e-log,e-state,e-steps,s-shot"
        && typeof filtered.unlinkedChip === "string" && filtered.unlinkedChip.includes("작업에 연결 안 된 것")
        && filtered.unlinkedCounts.reports === "1" && filtered.unlinkedCounts.pages === "2",
      JSON.stringify({
        line: filtered.emptiedLine, emptyBlock: filtered.emptyBlock, show: [filtered.showTag, filtered.showHeight],
        rows: filtered.unlinkedIds, chip: filtered.unlinkedChip, counts: filtered.unlinkedCounts,
      }),
    );
    ok(
      "the same filter is reached from the keyboard without the drawer: the head's 작업 select lists every task with its count and the rows no task is linked to, and follows the chip",
      filtered.pickTag === "SELECT" && filtered.pickValue === "t-501"
        && filtered.pickOptions.some((one) => one.includes("t-501") && one.includes("3"))
        && filtered.pickOptions.some((one) => one.includes("작업에 연결 안 된 것"))
        && filtered.afterPick.reports === "1" && typeof filtered.pickChip === "string" && filtered.pickChip.includes("t-502"),
      JSON.stringify({ value: filtered.pickValue, options: filtered.pickOptions, afterPick: filtered.afterPick, chip: filtered.pickChip }),
    );
    ok(
      "✕ clears the task and every tab counts all of its rows again",
      filtered.afterClear.pages === "3" && filtered.afterClear.reports === "5" && filtered.afterClear.evidence === "4" && filtered.pickAfterClear === "",
      JSON.stringify({ counts: filtered.afterClear, pick: filtered.pickAfterClear }),
    );

    /* ---- the screen rules (item 6) ------------------------------------------------------------ */
    const rules = await page.evaluate(async () => {
      const view = artifactsView();
      const shown = (node) => Boolean(node) && !node.closest("[hidden]") && node.getClientRects().length > 0;
      view?.querySelector('[data-artifact-tab="reports"]')?.click();
      await new Promise((done) => setTimeout(done, 150));
      selectArtifact(view, "r-final", { reveal: true });
      await window.__PAINTED__();
      await new Promise((done) => setTimeout(done, 350));
      view?.querySelector(".artifact-detail-more")?.click();
      view?.querySelector(".artifact-outline-toggle")?.click();
      await new Promise((done) => setTimeout(done, 80));
      const name = (node) => String(node.className || node.tagName).split(" ").slice(0, 2).join(".");
      // An agent's mark is a picture with a letter in it, not a line of text.
      const picture = (node) => Boolean(node.closest('[class*="agent-ico"], svg'));
      const small = new Map();
      for (const node of view?.querySelectorAll("*") ?? []) {
        if (!shown(node) || picture(node)) continue;
        const own = [...node.childNodes].some((child) => child.nodeType === 3 && child.textContent.trim() !== "");
        if (!own) continue;
        const px = parseFloat(getComputedStyle(node).fontSize);
        if (px < 12) small.set(name(node), px);
      }
      // A link inside a report's prose is a word of its sentence, not a control of the tab.
      const short = new Map();
      for (const node of view?.querySelectorAll('button, select, input, summary, a[href], [role="tab"], [role="button"]') ?? []) {
        if (!shown(node) || node.closest(".md-body")) continue;
        const height = Math.round(node.getBoundingClientRect().height);
        if (height < 28) short.set(name(node), height);
      }
      // Amber — the colour of the selected card's edge — on anything that is not a selected thing.
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
        const edges = ["Top", "Right", "Bottom", "Left"]
          .filter((side) => parseFloat(style[`border${side}Width`]) > 0)
          .map((side) => style[`border${side}Color`]);
        const own = [...node.childNodes].some((child) => child.nodeType === 3 && child.textContent.trim() !== "");
        if (edges.includes(amber) || (own && style.color === amber)) loud.add(name(node));
      }
      const selectedCard = view?.querySelector(".artifact-card.is-selected") ?? null;
      const seen = {
        small: [...small].map(([key, px]) => `${key}:${px}`),
        short: [...short].map(([key, px]) => `${key}:${px}`),
        amber,
        loud: [...loud],
        selectedEdge: selectedCard ? getComputedStyle(selectedCard).borderTopColor : null,
      };
      view?.querySelector(".artifact-outline-toggle")?.click();
      view?.querySelector(".artifact-detail-more")?.click();
      return seen;
    });
    ok(
      `every line of text in the tab is ${TEXT_MIN_PX}px or more — the list, the cards, the drawer with its details and its outline open`,
      rules.small.length === 0,
      JSON.stringify(rules.small),
    );
    ok(
      `everything a person presses in the tab is at least ${PRESS_MIN_PX}px high`,
      rules.short.length === 0,
      JSON.stringify(rules.short),
    );
    ok(
      "amber is worn by the selected card's edge and by nothing that is not selected",
      rules.selectedEdge === rules.amber && rules.loud.length === 0,
      JSON.stringify({ amber: rules.amber, selectedEdge: rules.selectedEdge, loud: rules.loud }),
    );

    /* Keyboard reach and a visible focus for every new control: Tab from the list walks into them. */
    const reach = await page.evaluate(() => {
      const view = artifactsView();
      view?.querySelector(".artifacts-grid")?.focus();
      return Boolean(view);
    });
    const walked = [];
    if (reach) {
      for (let at = 0; at < 14; at += 1) {
        await page.keyboard.press("Tab");
        walked.push(await page.evaluate(() => {
          const node = document.activeElement;
          if (!node || !node.closest(".artifacts-view")) return null;
          const style = getComputedStyle(node);
          return {
            name: String(node.className || node.tagName).split(" ")[0],
            ring: (style.outlineStyle !== "none" && parseFloat(style.outlineWidth) > 0) || style.boxShadow !== "none",
          };
        }));
      }
    }
    const reached = walked.filter(Boolean);
    const wanted = ["artifact-detail-task", "artifact-detail-attempts", "artifact-detail-more", "artifact-outline-toggle"];
    ok(
      "Tab from the list reaches the drawer's new controls — the task chip, the reports select, 세부 정보, the outline — and each shows its focus",
      wanted.every((one) => reached.some((held) => held.name === one && held.ring)),
      JSON.stringify(reached.map((held) => `${held.name}${held.ring ? "" : " (no ring)"}`)),
    );
    await page.emulateMedia({ reducedMotion: "reduce" });
    const still = await page.evaluate(() => {
      const view = artifactsView();
      const moving = [];
      for (const selector of [".artifacts-drawer", ".artifact-card", ".artifact-card-task", ".artifact-detail-bar", ".artifact-outline", ".artifact-outline-list"]) {
        const node = view?.querySelector(selector);
        if (!node) continue;
        const style = getComputedStyle(node);
        const seconds = style.transitionDuration.split(",").map((one) => parseFloat(one));
        if (seconds.some((one) => one > 0) || style.scrollBehavior === "smooth") moving.push(selector);
      }
      return { moving, drawerBehavior: getComputedStyle(view?.querySelector(".artifacts-drawer") ?? document.body).scrollBehavior };
    });
    ok(
      "with reduced motion asked for, nothing in the cards or the drawer animates and the outline jumps instead of gliding",
      still.moving.length === 0 && still.drawerBehavior !== "smooth",
      JSON.stringify(still),
    );
    await page.emulateMedia({ reducedMotion: "no-preference" });

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
      "every new word stands in the four translated catalogs beside its Korean",
      catalogs.length === 0,
      JSON.stringify(catalogs.slice(0, 12)) + (catalogs.length > 12 ? ` … ${catalogs.length}` : ""),
    );
    ok("the window raised no errors", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}
