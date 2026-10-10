import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { openWindowTestPage } from "./window-boot.mjs";

/* The sidebar says what each piece of work is doing (t-44016).
 *
 * Reported 2026-10-10 15:1x~15:2x from the person's own window: 「어떤 작업을
 * 하고 있는지 보이지가 않네」. The agent rows said a title and a clock and no
 * more; a worktree row said its branch and 「에이전트 창 없음」 and nothing
 * about the task, the agent that worked there or where the work stood; an
 * empty worktree had no words at all; and a Codex tab was named by the
 * fragment of a tool tag it printed (`<send_user_message_question_re…`).
 *
 * What stands here, built from the window's own roads (`hook:agent`,
 * `hook:activity`, `term:title`, `ledger_agents`, `project_catalog`), with no
 * conversation of anybody's read:
 *   ① a live agent row says what it is doing now, or what it waits for;
 *   ② a worktree row says its task (id and title), the agent that worked
 *      there with its model and end time, and the stage the ledger puts the
 *      work at; 「에이전트 창 없음」 is never its only word;
 *   ③ an empty worktree shows a summary card: task, state, last commit and
 *      the next step, and only facts the ledger or git hold;
 *   ④ a tab is named by the task, the first words or the folder — never by
 *      a tag or XML fragment a program printed as its title;
 *   ⑤ every new sentence reads in all five catalogues (ko, en, ja, zh, es).
 *
 * `ok` details carry the numbers the report quotes (rows that say what they
 * do, before and after; paint time). */

const NOW = Date.now();
const HOUR = 3_600_000;
const REF = "origin/main";
const HEAD = "9f3c2a1b7d4e5f60718293a4b5c6d7e8f9a0b1c2";
const TASK = "사이드바가 무슨 일을 하는지 보이게";
const NOTHING = { verified: false, merged: false, deployed: false, written: false };
const base = (state, over = {}) => ({
  state, detached: false, ahead: 0, dirty: false, dirty_checked_ms: NOW - 1000, compare_ref: REF,
  ref_updated_ms: NOW - 2 * 86_400_000, ...over,
});
const WALL = {
  wall: "quota_walled", observed_at_ms: NOW - 60_000, resets_at_ms: NOW + 2 * HOUR,
  reset_waitable: true, stands_until_ms: NOW + 2 * HOUR + 60_000,
};

/* The live rows: one pane each, worded by what the window was told. `want` is
 * the sentence the row must say for it to count as saying what it does. */
const PANES = [
  { term: 9201, path: "/r/act-run", hook: "working", activity: { verb: "bash", target: "npm test" },
    ledger: { task: "작업 실행 과업", task_id: "t-9201" }, want: "npm test" },
  { term: 9202, path: "/r/act-ask", hook: "done",
    ledger: { task: "질문 과업", task_id: "t-9202", asking: true }, want: "답을 기다리는 중" },
  { term: 9203, path: "/r/act-review", hook: "done",
    ledger: { task: "검토 과업", task_id: "t-9203", reported: true, review_since_ms: NOW - HOUR }, want: "검토를 기다리는 중" },
  { term: 9204, path: "/r/act-quota", hook: "done",
    ledger: { task: "한도 과업", task_id: "t-9204", wall: WALL }, want: "사용량 한도" },
];

/* Tabs whose program printed a tag fragment as its title (④). */
const TABS = [
  { term: 9301, path: "/r/tab-task", title: "<send_user_message_question_re…", agent: "codex",
    ledger: { task: "컴퓨터 유즈 성능 분석", task_id: "t-9301" }, want: "컴퓨터 유즈 성능 분석" },
  { term: 9302, path: "/r/tab-prompt", title: "<send_user_message>", agent: "codex", prompt: "로그인 버그 고치기",
    want: "로그인 버그 고치기" },
  { term: 9303, path: "/r/tab-folder", title: "<send_user_message>", agent: "codex", want: "tab-folder" },
  { term: 9304, path: "/r/tab-word", title: "cargo build", want: "cargo build" },
];

/* The worktree that finished its work and waits to land (②, ③). */
const FINISHED = "/r/finished-wt";

const SCENE_PATHS = [...PANES.map((one) => one.path), ...TABS.map((one) => one.path), FINISHED, "/r/empty-wt"];

export async function testSidebarActivity({ browser, origin, ok, faults }) {
  const { page } = await openWindowTestPage(browser, origin, { faults });
  const capture = process.env.SIDEBAR_ACTIVITY_CAPTURE ?? null;
  if (capture) await mkdir(capture, { recursive: true });
  try {
    await page.setViewportSize({ width: 1280, height: 1600 });
    await page.waitForFunction(() => projectsRead);
    await page.evaluate(() => { setEveryProjectClosed(false); });
    await page.evaluate(async (scene) => {
      setLocale("ko", { persist: false });
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      const ledger = [];
      const row = (over) => ({
        run: "run-1", worker: `w-${over.tag}`, agent: "claude", state: "working", ledger: "active",
        hearing: "pending", hearing_at: 1, checkout: over.checkout, task: "", task_id: "",
        reported: false, failed: false, settled: false, session: null, at: 1, review: { ...scene.nothing },
        review_since_ms: null, closed: null, asking: false, wall: null, model: null, effort: null, handed_in: null,
        kept: null, ...over,
      });
      for (const one of scene.panes) {
        tabs.push({ kind: "term", worktree: one.path, id: `tab-${one.term}`, layout: { type: "leaf", term: one.term } });
        paneAgents.set(one.term, "claude");
        tell("hook:agent", { term: one.term, state: one.hook, agent: "claude", session: `s-${one.term}`, model: "claude-haiku-5-5" });
        if (one.activity) {
          // A batch row is `{ seq, activity }` — the shape the backend's beat sends (conversation-parity).
          tell("hook:activity", { pane: `term:${one.term}`, activities: [{ seq: one.term, activity: { phase: "started", ...one.activity } }] });
        }
        if (one.ledger) ledger.push(row({ tag: one.term, term: one.term, checkout: one.path, ...one.ledger, review: { ...scene.nothing, ...(one.ledger.review ?? {}) } }));
      }
      for (const one of scene.tabs) {
        tabs.push({ kind: "term", worktree: one.path, id: `tab-${one.term}`, layout: { type: "leaf", term: one.term } });
        paneAgents.set(one.term, one.agent);
        tell("term:title", { term: one.term, title: one.title });
        if (one.prompt) panePrompts.set(one.term, one.prompt);
        if (one.ledger) ledger.push(row({ tag: one.term, term: one.term, checkout: one.path, ...one.ledger }));
      }
      // The worktree whose work is handed in and verified, not yet merged: the ledger keeps it as a
      // released worker's finished work in the checkout (`settled`).
      ledger.push(row({
        tag: "finished", checkout: scene.finished, settled: true, reported: true, task: scene.task, task_id: "t-44016",
        agent: "claude", model: "claude-haiku-5-5", at: Date.now() - 2 * 3_600_000, handed_in: scene.head,
        review: { ...scene.nothing, verified: true, written: true, author: "coordinator" },
      }));
      window.__LEDGER__ = ledger;
      window.__ANSWER__.project_catalog = () => [{
        name: "r", path: "/r",
        external: { visibility: "show", legacy: false, authoritative: true, shown: 0, hidden: [], prompt: false, inbox: [] },
        worktrees: scene.paths.map((path, at) => ({
          path, branch: path.slice(3), is_main: false, active: false, is_folder: false,
          ownership: "zerocode-managed", external_hidden: false, last_activity_ms: at,
          landing: scene.landings[path],
        })),
      }];
      window.__ANSWER__.set_sidebar_view = () => null;
      for (const listener of window.__LISTENERS__["ledger:changed"] ?? []) listener({ payload: 9 });
      const seated = scene.panes.filter((one) => one.ledger).length + scene.tabs.filter((one) => one.ledger).length;
      for (let tries = 0; tries < 80 && paneLedger.size < seated; tries += 1) {
        await new Promise((done) => setTimeout(done, 25));
      }
      setSidebarView({ groupBy: "repo" });
      await refreshWorktrees();
      await window.__PAINTED__();
      // What a person can read of one agent row: its tooltip's words, its status and its clock.
      window.__ROW__ = (term) => {
        const node = document.querySelector(`.wt-agent[data-term="${term}"]`);
        return node ? { text: node.textContent, aria: node.getAttribute("aria-label") ?? "" } : null;
      };
      window.__WT__ = (path) => {
        const node = document.querySelector(`.wt-row[data-worktree-path="${CSS.escape(path)}"]`);
        return node ? {
          title: node.querySelector(".wt-title")?.textContent ?? "",
          branch: node.querySelector(".wt-branch")?.textContent ?? "",
          stage: node.querySelector(".wt-stage")?.textContent ?? "",
          taskId: node.querySelector(".wt-task-id")?.textContent ?? "",
          unseatedShown: (node.querySelector(".wt-unseated") && !node.querySelector(".wt-unseated").hidden) === true,
        } : null;
      };
      // The strip's own words for a terminal tab (`tabLabel`), read without painting a strip per tab.
      window.__TAB__ = (term) => tabLabel(tabs.find((one) => one.id === `tab-${term}`) ?? { kind: "gone" });
    }, {
      panes: PANES, tabs: TABS, finished: FINISHED, paths: SCENE_PATHS, task: TASK, head: HEAD, nothing: NOTHING,
      // Built here, where `base` lives: the page cannot call a helper of this file.
      landings: Object.fromEntries(SCENE_PATHS.map((path) => [path, path === FINISHED
        ? base("unlanded", { ahead: 3 })
        : base("landed")])),
    });

    /* ① — the live rows. Counted the same way before and after: a row counts when its own
     * words (tooltip, status line or the 「지금」 line) carry what the row is waiting for or doing. */
    const rows = await page.evaluate((panes) => panes.map((one) => ({
      term: one.term, want: one.want, got: window.__ROW__(one.term),
    })), PANES);
    const saying = rows.filter((one) => one.got !== null && one.got.aria.includes(one.want)).length;
    ok(
      "a live agent row says what it does now or what it waits for — a tool step, an answer it asked for, a review it waits on, a quota wall — beside its title and clock",
      saying === PANES.length,
      JSON.stringify({ saying, of: PANES.length, rows: rows.map((one) => ({ term: one.term, want: one.want, aria: one.got?.aria ?? null })) }),
    );

    /* ② — the worktree rows. */
    const wt = await page.evaluate((path) => window.__WT__(path), FINISHED);
    const empty = await page.evaluate((path) => window.__WT__(path), "/r/empty-wt");
    const wtNumbers = await page.evaluate((paths) => paths.map((path) => window.__WT__(path)), SCENE_PATHS);
    ok(
      "a worktree row says its task id and title, the stage its work stands at in the ledger's words, and the agent that worked there with its model and end time — and 「에이전트 창 없음」 is not said beside that stage",
      wt !== null && wt.taskId.includes("t-44016") && wt.title.includes(TASK) && wt.stage === "착지 대기" &&
        wt.branch.includes("Claude") && wt.branch.includes("haiku-5-5") && wt.branch.includes("끝") &&
        wt.unseatedShown === false && empty !== null,
      JSON.stringify({ finished: wt, empty, rows: wtNumbers }),
    );

    /* ③ — the empty worktree's summary card. Activating it is what a person does to open it. */
    const card = await page.evaluate(async (path) => {
      // The checkout is the active one the way activation leaves it: its path, its selection and the
      // empty stage repainted. A path git does not know cannot go through a real activation here.
      activeWorktreePath = path;
      paintWorktreeSelection();
      paintStagePlaceholder();
      await window.__PAINTED__();
      const node = document.querySelector(".stage-card");
      return node && !node.hidden ? {
        text: node.textContent,
        task: node.querySelector(".stage-card-task")?.textContent ?? "",
        state: node.querySelector(".stage-card-state")?.textContent ?? "",
        commit: node.querySelector(".stage-card-commit")?.textContent ?? "",
        next: node.querySelector(".stage-card-next")?.textContent ?? "",
      } : null;
    }, FINISHED);
    ok(
      "an empty worktree opens on a summary card — its task, its state, the last commit the ledger holds and the next step — and only facts the ledger or git hold are said",
      card !== null && card.task.includes(TASK) && card.state === "착지 대기" && card.commit.includes(HEAD.slice(0, 9)) &&
        card.next.length > 0,
      JSON.stringify(card),
    );

    /* ④ — the tab names, from the task, the first words or the folder. */
    const tabs = await page.evaluate((list) => list.map((one) => ({ term: one.term, want: one.want, got: window.__TAB__(one.term) })), TABS);
    const agentRowOfTag = await page.evaluate(() => window.__ROW__(9301)?.aria ?? "");
    ok(
      "a tab is named by its task, its first words or its folder — never by a tag or XML fragment a program printed as its title, and the agent row beside it does not wear that fragment either",
      tabs.every((one) => one.got === one.want) && !agentRowOfTag.includes("<send_user_message"),
      JSON.stringify({ tabs, agentRowOfTag }),
    );

    /* ④b — a tab that was painted before its ledger row arrived: the strip must repaint on the task's
     * arrival alone, with no hook, no term:title and no manual render in between (run-11955 m-44624).
     * The tab is a real one of the checkout in front, opened the way a person opens a terminal, so the
     * strip paints it (`paneTabs` keeps only the active checkout's tabs) and the DOM is read, not tabLabel. */
    const lateTab = await page.evaluate(async () => {
      const term = await openTermTab({ placement: "tab" });
      const tab = tabOfTerm(term);
      const folder = tab?.worktree ? tab.worktree.split("/").filter(Boolean).pop() : null;
      paneAgents.set(term, "codex");
      for (const handler of window.__LISTENERS__["term:title"] ?? []) handler({ payload: { term, title: "<send_user_message>" } });
      renderTabs();
      await window.__PAINTED__();
      const node = () => document.querySelector(`.tab[data-tab="${tab?.id}"] .tab-label`);
      const before = node()?.textContent ?? null;
      // The ledger row arrives after the strip was painted: the task is the only news.
      window.__LEDGER__.push({
        run: "run-1", worker: `w-${term}`, agent: "codex", state: "working", ledger: "active", hearing: "pending",
        hearing_at: 1, checkout: tab?.worktree ?? "", task: "늦게 온 과업", task_id: "t-9305", reported: false, failed: false,
        settled: false, session: null, at: 1, review: { verified: false, merged: false, deployed: false, written: false, author: null },
        review_since_ms: null, closed: null, asking: false, wall: null, model: null, effort: null, handed_in: null, term, kept: null,
      });
      for (const listener of window.__LISTENERS__["ledger:changed"] ?? []) listener({ payload: 10 });
      for (let tries = 0; tries < 80 && !paneLedger.has(term); tries += 1) await new Promise((done) => setTimeout(done, 25));
      await window.__PAINTED__();
      await new Promise((done) => setTimeout(done, 200));
      await window.__PAINTED__();
      return { term, folder, before, after: node()?.textContent ?? null, seated: paneLedger.has(term) };
    });
    ok(
      "a tab painted before its ledger row arrived takes the task's name when the row comes — the strip repaints on the task alone, with no hook, title or manual render in between",
      lateTab.seated && lateTab.before !== null && lateTab.before === lateTab.folder && lateTab.after === "늦게 온 과업",
      JSON.stringify(lateTab),
    );

    /* ⑤ — the new sentences in every catalogue. */
    const langs = await page.evaluate(async () => {
      const KEYS = {
        "agent.now.waitAnswer": "답을 기다리는 중",
        "agent.now.waitReview": "검토를 기다리는 중",
        "agent.now.waitQuota": "사용량 한도",
        "worktree.stage.landWait": "착지 대기",
        "stage.card.next.land": "병합을 기다리는 중",
      };
      const found = {};
      for (const code of ["en", "ja", "zh", "es"]) {
        setLocale(code, { persist: false });
        found[code] = Object.entries(KEYS).map(([key, fallback]) => ({ key, text: t(key, fallback) === fallback }));
      }
      setLocale("ko", { persist: false });
      return found;
    });
    const missing = Object.entries(langs).flatMap(([code, list]) => list.filter((one) => one.text).map((one) => `${code}:${one.key}`));
    ok(
      "every new sentence reads in English, Japanese, Chinese and Spanish, not in the Korean fallback",
      missing.length === 0,
      JSON.stringify({ missing }),
    );

    /* The numbers the report quotes (t-44016), printed when `SIDEBAR_ACTIVITY_MEASURE=1`: how many of the scene's
     * rows and tabs say what they do or wait for, whether the empty worktree shows its card, what one full
     * repaint of the sidebar costs (median and 95th percentile, ms, from `performance.now()`), and how many
     * backend calls the repaints make (none is the target: a paint reads what the window holds). */
    if (process.env.SIDEBAR_ACTIVITY_MEASURE) {
      const numbers = await page.evaluate((scene) => {
        const total = () => Object.values(window.__COUNTS__).reduce((sum, count) => sum + count, 0);
        const runs = 60;
        const timed = (fn) => {
          const ms = [];
          for (let i = 0; i < runs; i += 1) {
            const at = performance.now();
            fn();
            ms.push(performance.now() - at);
          }
          ms.sort((a, b) => a - b);
          return { median: Number(ms[runs >> 1].toFixed(3)), p95: Number(ms[Math.floor(runs * 0.95)].toFixed(3)) };
        };
        // A full repaint: the shape guard is cleared so every host rebuilds its rows, as a real change would.
        const repaint = () => {
          for (const host of document.querySelectorAll(".wt-agents[data-worktree-path]")) {
            delete host.dataset.said;
            delete host.dataset.restartSource;
          }
          paintWorktreeAgents();
        };
        const liveSaying = scene.panes.filter((one) => {
          const node = document.querySelector(`.wt-agent[data-term="${one.term}"]`);
          return node?.getAttribute("aria-label")?.includes(one.want);
        }).length;
        const tabsSaying = scene.tabs.filter((one) => {
          const tab = tabs.find((held) => held.id === `tab-${one.term}`);
          return tab && tabLabel(tab) === one.want;
        }).length;
        const rowsSaying = scene.paths.filter((path) => {
          const row = document.querySelector(`.wt-row[data-worktree-path="${CSS.escape(path)}"]`);
          return [row?.querySelector(".wt-task-id"), row?.querySelector(".wt-stage")]
            .some((node) => node && !node.hidden && node.textContent !== "");
        }).length;
        // The empty worktree's card, as the checks above opened it: the checkout active again, its stage repainted.
        activeWorktreePath = scene.finished;
        paintStagePlaceholder();
        const card = document.querySelector(".stage-card");
        const before = total();
        const paint = timed(repaint);
        const idleInvokes = total() - before;
        const cardPaint = typeof paintStageCard === "function" ? timed(() => paintStageCard()) : null;
        return {
          liveSaying, liveOf: scene.panes.length, tabsSaying, tabsOf: scene.tabs.length,
          rowsSaying, rowsOf: scene.paths.length, cardShown: Boolean(card && !card.hidden),
          paintMs: paint, cardPaintMs: cardPaint, invokesDuringRepaints: idleInvokes, repaints: runs,
        };
      }, { panes: PANES, tabs: TABS, paths: SCENE_PATHS, finished: FINISHED });
      console.log("MEASURE " + JSON.stringify(numbers));
    }

    if (capture) {
      await page.locator("#sidebar").screenshot({ path: resolve(capture, "sidebar-activity.png"), animations: "disabled" }).catch(() => {});
    }
  } finally {
    await page.close().catch(() => {});
  }
}
