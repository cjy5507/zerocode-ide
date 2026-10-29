import { openWindowTestPage } from "./window-boot.mjs";

/* One task, one sidebar row (t-45336).
 *
 * Reported by the person on 10-11 03:5x and again on 10-14: 「네비바에 완벽하게
 * 끝난 과업들은 … 정리해 너무 많아」, 「정리할때 워커와 브런치도 같이 정리 해야해
 * 그래야 안햇갈림」. One task (t-26584) stood in the sidebar four times — the
 * `before`, `jev-evidence`, `jev-green` and `verify` checkouts its worker cut
 * beside its own — each row saying 「미반영 N · 에이전트 창 없음」.
 *
 * The scene is a synthetic catalog and ledger, the window's own roads
 * (`project_catalog`, `ledger_agents`, `hook:agent`), nobody's real checkout:
 *   ① a task stands as one row — the checkout its head was handed in from —
 *      and the checkouts beside it fold under that row;
 *   ② the row's chip says how many are folded and names them, and a press
 *      shows them and folds them again;
 *   ③ a checkout an agent runs in never folds;
 *   ④ the row says where the work stands and, in one word, what holds it
 *      back: the ledger's late-landing reason, or the seat's own health;
 *   ⑤ every new sentence reads in all five catalogues.
 *
 * `ok` details carry the numbers the report quotes: the rows the catalog
 * holds (what the sidebar listed before) and the rows it lists now. */

const NOW = Date.now();
const HOUR = 3_600_000;
const HEAD = "4e5f60718293a4b5c6d7e8f9a0b1c29f3c2a1b7d";
const NOTHING = { verified: false, merged: false, deployed: false, written: false };
const WALL = {
  wall: "quota_walled", observed_at_ms: NOW - 60_000, resets_at_ms: NOW + 2 * HOUR,
  reset_waitable: true, stands_until_ms: NOW + 2 * HOUR + 60_000,
};

const OWN = "/r/t-26584-sidebar-fold";
const BESIDE = ["/r/t-26584-before", "/r/t-26584-jev-evidence", "/r/t-26584-jev-green", "/r/t-26584-verify"];
const BUSY_LEAD = "/r/t-26591";
const BUSY_BESIDE = "/r/t-26591-red";
const WALLED = "/r/t-26592-walled";
/* A task's verify, Windows and red copies fold under its row by their names (t-60498): the folder of the row they
 * copy plus the suffix. One task whose rows carry no task id, one whose rows do, and one whose copy an agent runs in. */
const NAMED = "/r/wt-n1";
const NAMED_COPIES = ["/r/wt-n1-verify", "/r/wt-n1-windows"];
const IDENTIFIED = "/r/t-26600-both";
const IDENTIFIED_COPIES = ["/r/t-26600-both-verify", "/r/t-26600-both-windows"];
const WORKED = "/r/wt-n2";
const WORKED_COPY = "/r/wt-n2-red";

/* The catalog's rows, as `project_catalog` ships them: each named for its task (`task_id`). */
const ROWS = [
  { path: OWN, branch: "wt/t-26584/sidebar-fold", task_id: "t-26584", state: "unlanded", ahead: 3 },
  { path: BESIDE[0], branch: null, task_id: "t-26584", state: "no_commits", ahead: 0 },
  { path: BESIDE[1], branch: null, task_id: "t-26584", state: "unlanded", ahead: 1 },
  { path: BESIDE[2], branch: "wt/t-26584-jev-green", task_id: "t-26584", state: "unlanded", ahead: 2 },
  { path: BESIDE[3], branch: null, task_id: "t-26584", state: "unlanded", ahead: 3 },
  { path: BUSY_LEAD, branch: "wt/t-26591", task_id: "t-26591", state: "unlanded", ahead: 1 },
  { path: BUSY_BESIDE, branch: null, task_id: "t-26591", state: "unlanded", ahead: 1 },
  { path: WALLED, branch: "wt/t-26592/walled", task_id: "t-26592", state: "unlanded", ahead: 1 },
  { path: NAMED, branch: "wt/n1", state: "unlanded", ahead: 2 },
  { path: NAMED_COPIES[0], branch: null, state: "unlanded", ahead: 2 },
  { path: NAMED_COPIES[1], branch: null, state: "no_commits", ahead: 0 },
  { path: IDENTIFIED, branch: "wt/t-26600/both", task_id: "t-26600", state: "unlanded", ahead: 1 },
  { path: IDENTIFIED_COPIES[0], branch: null, task_id: "t-26600", state: "unlanded", ahead: 1 },
  { path: IDENTIFIED_COPIES[1], branch: null, task_id: "t-26600", state: "no_commits", ahead: 0 },
  { path: WORKED, branch: "wt/n2", state: "unlanded", ahead: 1 },
  { path: WORKED_COPY, branch: null, state: "unlanded", ahead: 1 },
];

export async function testSidebarTaskFold({ browser, origin, ok, faults }) {
  const { page } = await openWindowTestPage(browser, origin, { faults });
  try {
    await page.setViewportSize({ width: 1280, height: 1600 });
    await page.waitForFunction(() => projectsRead);
    await page.evaluate(() => { setEveryProjectClosed(false); });
    await page.evaluate(async (scene) => {
      setLocale("ko", { persist: false });
      openedLandingFolds.add("/r"); // Inspect task-level folds inside the expanded finished group.
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      const row = (over) => ({
        run: "run-1", worker: `w-${over.tag}`, agent: "claude", state: "working", ledger: "active",
        hearing: "pending", hearing_at: 1, checkout: over.checkout, task: "", task_id: "",
        reported: false, failed: false, settled: false, session: null, at: 1, review: { ...scene.nothing },
        review_since_ms: null, closed: null, asking: false, wall: null, model: null, effort: null, handed_in: null,
        kept: null, ...over,
      });
      // The worker that handed t-26584 in, released: its checkout is the one the ledger names, and the ledger
      // has told the coordinator the review is late.
      const ledger = [
        row({
          tag: "own", checkout: scene.own, settled: true, reported: true, state: "released", ledger: "released",
          task: "사이드바 정리", task_id: "t-26584", model: "claude-opus-5-5", at: Date.now() - 3 * 3_600_000,
          handed_in: scene.head, review_since_ms: Date.now() - 3 * 3_600_000,
          progress: { stage: "report_only", source: scene.head }, stalled: ["no_review"],
        }),
        // t-26591's own checkout, handed in and verified.
        row({
          tag: "busy", checkout: scene.busyLead, settled: true, reported: true, state: "released", ledger: "released",
          task: "바쁜 과업", task_id: "t-26591", at: Date.now() - 3_600_000, handed_in: scene.head,
          review: { ...scene.nothing, verified: true, written: true, author: "coordinator" },
          progress: { stage: "verified", source: scene.head },
        }),
        // A live seat at a quota wall.
        row({ tag: 9702, term: 9702, checkout: scene.walled, task: "한도 과업", task_id: "t-26592", wall: scene.wall }),
      ];
      // An agent the person runs in a checkout beside t-26591's: no ledger row of its own.
      tabs.push({ kind: "term", worktree: scene.busyBeside, id: "tab-9701", layout: { type: "leaf", term: 9701 } });
      paneAgents.set(9701, "claude");
      tell("hook:agent", { term: 9701, state: "working", agent: "claude", session: "s-9701", model: "claude-opus-5-5" });
      tabs.push({ kind: "term", worktree: scene.workedCopy, id: "tab-9703", layout: { type: "leaf", term: 9703 } });
      paneAgents.set(9703, "claude");
      tell("hook:agent", { term: 9703, state: "working", agent: "claude", session: "s-9703", model: "claude-opus-5-5" });
      tabs.push({ kind: "term", worktree: scene.walled, id: "tab-9702", layout: { type: "leaf", term: 9702 } });
      paneAgents.set(9702, "claude");
      tell("hook:agent", { term: 9702, state: "done", agent: "claude", session: "s-9702", model: "claude-opus-5-5" });
      window.__LEDGER__ = ledger;
      window.__ANSWER__.project_catalog = () => [{
        name: "r", path: "/r",
        external: { visibility: "show", legacy: false, authoritative: true, shown: 0, hidden: [], prompt: false, inbox: [] },
        worktrees: scene.rows.map((one, at) => ({
          path: one.path, branch: one.branch, is_main: false, active: false, is_folder: false,
          ownership: "zerocode-managed", external_hidden: false, last_activity_ms: at, task_id: one.task_id,
          landing: {
            state: one.state, detached: one.branch === null, ahead: one.ahead, dirty: false,
            dirty_checked_ms: Date.now() - 1000, compare_ref: "origin/main", ref_updated_ms: Date.now() - 86_400_000,
          },
        })),
      }];
      window.__ANSWER__.set_sidebar_view = () => null;
      for (const listener of window.__LISTENERS__["ledger:changed"] ?? []) listener({ payload: 9 });
      for (let tries = 0; tries < 80 && (checkoutLedger.size < 2 || paneLedger.size < 1); tries += 1) {
        await new Promise((done) => setTimeout(done, 25));
      }
      setSidebarView({ groupBy: "repo" });
      await refreshWorktrees();
      await window.__PAINTED__();
      window.__SHOWN__ = () => [...document.querySelectorAll(".wt-row[data-worktree-path]")]
        .map((node) => node.dataset.worktreePath)
        .filter((path) => path.startsWith("/r/"));
      window.__WT__ = (path) => {
        const node = document.querySelector(`.wt-row[data-worktree-path="${CSS.escape(path)}"]`);
        const fold = node?.querySelector(".wt-fold");
        return node ? {
          stage: node.querySelector(".wt-stage")?.textContent ?? "",
          blocker: node.querySelector(".wt-blocker")?.textContent ?? "",
          blockerShown: (node.querySelector(".wt-blocker") && !node.querySelector(".wt-blocker").hidden) === true,
          fold: fold && !fold.hidden ? fold.textContent : "",
          foldTip: fold?.dataset.tip ?? "",
          expanded: fold?.getAttribute("aria-expanded") ?? "",
          under: node.closest(".wt-node")?.classList.contains("is-folded-under") === true,
        } : null;
      };
      window.__PRESS_FOLD__ = async (path) => {
        const fold = document.querySelector(`.wt-row[data-worktree-path="${CSS.escape(path)}"] .wt-fold`);
        if (!fold || fold.hidden) return false;
        fold.click();
        await refreshWorktrees();
        await window.__PAINTED__();
        return true;
      };
    }, {
      rows: ROWS, own: OWN, busyLead: BUSY_LEAD, busyBeside: BUSY_BESIDE, walled: WALLED, workedCopy: WORKED_COPY, head: HEAD,
      nothing: NOTHING, wall: WALL,
    });

    /* ① — one row a task. */
    const shown = await page.evaluate(() => window.__SHOWN__());
    const task = shown.filter((path) => path.startsWith("/r/t-26584"));
    ok(
      "a task stands as one sidebar row — the checkout its head was handed in from — and the checkouts its worker cut beside it fold under that row",
      task.length === 1 && task[0] === OWN,
      JSON.stringify({ catalogRows: ROWS.length, shownRows: shown.length, taskRows: task, shown }),
    );

    /* ② — the chip, and the press that opens and folds again. */
    const before = await page.evaluate((path) => window.__WT__(path), OWN);
    const opened = await page.evaluate(async (path) => {
      const pressed = await window.__PRESS_FOLD__(path);
      return { pressed, shown: window.__SHOWN__(), row: window.__WT__(path) };
    }, OWN);
    const closed = await page.evaluate(async (path) => {
      const pressed = await window.__PRESS_FOLD__(path);
      return { pressed, shown: window.__SHOWN__(), row: window.__WT__(path) };
    }, OWN);
    const named = BESIDE.every((path) => before?.foldTip.includes(path.slice(3)));
    ok(
      "the task's row says how many checkouts are folded under it and names them, and a press shows them and folds them again",
      before !== null && before.fold === "+4 접힘" && named && before.expanded === "false" &&
        opened.pressed && BESIDE.every((path) => opened.shown.includes(path)) && opened.row?.fold === "접기" &&
        opened.row?.expanded === "true" &&
        closed.pressed && closed.shown.filter((path) => path.startsWith("/r/t-26584")).length === 1,
      JSON.stringify({ before, opened, closed }),
    );

    /* ③ — a checkout an agent runs in stays listed. */
    ok(
      "a checkout an agent runs in never folds, and the task's own row stays beside it",
      shown.includes(BUSY_BESIDE) && shown.includes(BUSY_LEAD),
      JSON.stringify({ shown }),
    );

    /* ④ — where the work stands, and what holds it back, in one word. */
    const own = await page.evaluate((path) => window.__WT__(path), OWN);
    const walled = await page.evaluate((path) => window.__WT__(path), WALLED);
    const busy = await page.evaluate((path) => window.__WT__(path), BUSY_LEAD);
    ok(
      "a row says the stage its work stands at and, in one word, what holds it back — the ledger's late review, a quota wall — and nothing where nothing does",
      own?.stage === "넘김 끝 · 검토 대기" && own?.blocker === "검증 늦음" && own?.blockerShown === true &&
        walled?.blocker === "한도 도달" && busy?.stage === "착지 대기" && busy?.blockerShown === false,
      JSON.stringify({ own, walled, busy }),
    );

    /* ⑥ — landed work in the cleanup review: it wears 「착지됨」, and its removal asks for the task's landed
     * branches to go with it (the backend deletes only what the compare ref holds, by `git branch -d`). */
    const cleanup = await page.evaluate(async () => {
      const seen = {};
      const asked = [];
      window.__CLEANUP__ = [{
        id: "/w/t-26584-landed", name: "wt/t-26584/landed", path: "/w/t-26584-landed", project: "/w/repo",
        project_name: "repo", branch: "wt/t-26584/landed", head: "abc1234", tier: "suggested", reasons: ["landed"],
        blockers: [], last_activity_ms: Date.now(), idle_days: 0,
        git: { dirty_files: 0, ahead: 0, has_upstream: true, provably_clean: true },
        force: false, fingerprint: "2|wt/t-26584/landed|abc1234|clean|0",
      }];
      window.__ANSWER__.remove_worktree = (args) => {
        asked.push(JSON.parse(JSON.stringify(args)));
        return null;
      };
      setLocale("ko", { persist: false });
      // The dialog's own scan, waited out rather than joined by a second one: a scan started while another is
      // still out hands back the `busy` it found, and every press in the dialog would stay disabled.
      openCleanup();
      for (let waited = 0; waited < 2000; waited += 20) {
        if (!wsclean.busy && document.querySelector('.wsclean-row[data-id="/w/t-26584-landed"]')) break;
        await new Promise((done) => setTimeout(done, 20));
      }
      const row = document.querySelector('.wsclean-row[data-id="/w/t-26584-landed"]');
      seen.chips = row ? [...row.querySelectorAll(".wsclean-chip")].map((chip) => chip.textContent) : null;
      if (row) {
        [...row.querySelectorAll(".wsclean-act")].at(-1).click();
        await new Promise((done) => setTimeout(done, 40));
        document.getElementById("wsclean-confirm-go").click();
        await new Promise((done) => setTimeout(done, 160));
      }
      seen.asked = asked;
      delete window.__ANSWER__.remove_worktree;
      closeCleanup();
      return seen;
    });
    ok(
      "landed work in the cleanup review says it has landed, and the one press that removes it asks for the task's landed branches to go too",
      cleanup.chips?.includes("착지됨") === true && cleanup.asked.length === 1 &&
        cleanup.asked[0].landedBranches === true && cleanup.asked[0].discardChanges === false,
      JSON.stringify(cleanup),
    );

    /* ⑦ — a task's verify, Windows and red copies fold under its row by their names, with or without a task id. */
    const copies = await page.evaluate(async ({ named, identified, worked }) => {
      const out = { shown: window.__SHOWN__() };
      out.named = window.__WT__(named);
      out.identified = window.__WT__(identified);
      out.worked = window.__WT__(worked);
      out.order = worktreeOrder.filter((path) => path.startsWith("/r/wt-n") || path.startsWith("/r/t-26600"));
      await window.__PRESS_FOLD__(named);
      out.openedNamed = { shown: window.__SHOWN__(), row: window.__WT__(named), order: worktreeOrder.filter((path) => path.startsWith("/r/wt-n1")) };
      out.copyRows = ["/r/wt-n1-verify", "/r/wt-n1-windows"].map((path) => window.__WT__(path)?.under);
      await window.__PRESS_FOLD__(named);
      out.closedAgain = { shown: window.__SHOWN__(), order: worktreeOrder.filter((path) => path.startsWith("/r/wt-n1")) };
      return out;
    }, { named: NAMED, identified: IDENTIFIED, worked: WORKED });
    const standing = (list, path) => list.filter((one) => one === path).length;
    ok(
      "a task's -verify and -windows checkouts fold under its row by their names — with no task id on the rows — and the row's chip says 「검증 사본 2」, naming them",
      standing(copies.shown, NAMED) === 1 && NAMED_COPIES.every((path) => !copies.shown.includes(path)) &&
        copies.named?.fold === "검증 사본 2" && NAMED_COPIES.every((path) => copies.named.foldTip.includes(path.slice(3))),
      JSON.stringify({ shown: copies.shown, named: copies.named }),
    );
    ok(
      "the same fold holds for rows that carry a task id — the copies fold, the chip says 「검증 사본 2」 — and a task whose other checkouts are not copies keeps the chip 「+4 접힘」",
      IDENTIFIED_COPIES.every((path) => !copies.shown.includes(path)) && copies.identified?.fold === "검증 사본 2" &&
        before?.fold === "+4 접힘",
      JSON.stringify({ identified: copies.identified, before }),
    );
    ok(
      "a folded copy is no row for the list's window or for ⌘1–9 — the order the shortcuts count leaves it out — and opened, the copies stand right under their task's row, one step in",
      NAMED_COPIES.every((path) => !copies.order.includes(path)) &&
        JSON.stringify(copies.openedNamed.order) === JSON.stringify([NAMED, ...NAMED_COPIES]) &&
        copies.openedNamed.row?.fold === "접기" && copies.copyRows.every((under) => under === true) &&
        JSON.stringify(copies.openedNamed.shown.filter((path) => path.startsWith("/r/wt-n1"))) === JSON.stringify([NAMED, ...NAMED_COPIES]) &&
        NAMED_COPIES.every((path) => !copies.closedAgain.shown.includes(path) && !copies.closedAgain.order.includes(path)),
      JSON.stringify(copies),
    );
    ok(
      "a copy an agent runs in never folds: it stands under its task's row and the row wears no fold chip",
      copies.shown.includes(WORKED) && copies.shown.includes(WORKED_COPY) && copies.worked?.fold === "",
      JSON.stringify({ shown: copies.shown, worked: copies.worked }),
    );

    /* ⑤ — the new sentences in every catalogue. */
    const langs = await page.evaluate(() => {
      const KEYS = {
        "worktree.taskFold": "+{{count}} 접힘",
        "worktree.taskFoldTip": "이 과업의 다른 체크아웃 {{count}}개({{names}})를 이 행 밑에 접었습니다 — 누르면 펼칩니다. 넘긴 머리에 담긴 것은 창이 거둡니다.",
        "worktree.copyFold": "검증 사본 {{count}}",
        "worktree.copyFoldTip": "이 과업의 검증·윈도·빨강 사본 {{count}}개({{names}})를 이 행 밑에 접었습니다 — 누르면 펼칩니다",
        "worktree.taskFoldOpen": "접기",
        "worktree.taskFoldOpenTip": "이 과업의 다른 체크아웃 {{count}}개를 다시 이 행 밑에 접습니다",
        "worktree.stall.noReview": "검증 늦음",
        "worktree.stall.noMerge": "착지 늦음",
        "worktree.stall.mergedUnrecorded": "병합 기록 없음",
        "worktree.stall.gitNot": "main에 없음",
        "worktree.reclaimedBeside": "{{task}}이(가) 넘겨져 워커 체크아웃 곁의 {{name}}을(를) 회수했습니다 — {{branch}}의 커밋은 모두 넘긴 머리({{base}})에 있고 저장되지 않은 변경은 없었습니다.",
        "board.desk.landingCheckoutsLeft": "넘길 때 같은 과업의 체크아웃을 곁에 남겼어요",
        "cleanup.reasonLanded": "착지됨",
      };
      const found = {};
      for (const code of ["en", "ja", "zh", "es"]) {
        setLocale(code, { persist: false });
        found[code] = Object.entries(KEYS).map(([key, fallback]) => ({ key, fallback: t(key, fallback) === fallback }));
      }
      setLocale("ko", { persist: false });
      return found;
    });
    const missing = Object.entries(langs).flatMap(([code, list]) => list.filter((one) => one.fallback).map((one) => `${code}:${one.key}`));
    ok(
      "every new sentence reads in English, Japanese, Chinese and Spanish, not in the Korean fallback",
      missing.length === 0,
      JSON.stringify({ missing }),
    );
  } finally {
    await page.close();
  }
}
