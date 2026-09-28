import { openWindowTestPage } from "./window-boot.mjs";

/* The agents a zo session starts, in the sidebar (t-11827, t-11753).
 *
 * Reported 2026-09-28 from the person's own window: under a zo session row
 * the agents it had started stood in the main workspace's card — some at
 * work, some finished — and "에이전트가 다 보이지 않고, 눌러도 열리지 않는다":
 * a row's tooltip stood over the row below it, pressing a helper's row
 * brought forward the zo pane the person was already looking at, and the
 * arrow keys on a row moved the whole sidebar to another workspace. And
 * "agent가 done 상태여도 창을 걷어 내지 않고 화면에 계속 표시됨": a helper
 * that ran in a pane of its own kept its row, finished, between the working
 * ones.
 *
 * Every fixture here is built from the window's own events — `hook:agent`,
 * `hook:subagent`, `term:split` — the roads the backend speaks on, and the
 * page dies with the suite. No conversation of anybody's is read. */

export async function testSidebarAgents({ browser, origin, ok, faults }) {
  const { page } = await openWindowTestPage(browser, origin, { faults });
  try {
    await page.waitForFunction(() => projectsRead);
    await page.evaluate(async () => { setEveryProjectClosed(false); await refreshWorktrees(); });

    /* A zo session and what it started: two helpers at work inside zo, two
     * finished there, one at work in a pane of its own and one finished in
     * a pane of its own — the six of the report. Finished work gathers
     * under the one 「완료 N개」 line below the working rows, whichever road
     * it ran on, and opening that line shows each finished agent above it; a
     * finished pane the person is looking at stays out of the fold. The
     * same rule for a Claude Code lead and its finished team pane. */
    const folds = await page.evaluate(async () => {
      const seen = {};
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      const term = await openTermTab({ placement: "tab" });
      const owner = tabOfTerm(term);
      const path = owner.worktree;
      const card = () => document.querySelector(`.wt-agents[data-worktree-path="${CSS.escape(path)}"]`);
      const drawn = () => [...(card()?.querySelectorAll(".wt-agent") ?? [])];
      paneAgents.set(term, "zo");
      tell("hook:agent", { term, state: "working", agent: "zo", session: "zo-lead", model: "claude-opus-5-5" });
      tell("hook:subagent", { term, rows: [
        { id: "in-1", name: "artifact-audit", state: "running", tool_calls: 76 },
        { id: "in-2", name: "explore·sidebar", state: "running", tool_calls: 4 },
        { id: "in-3", name: "deep-research·artifact-trends", state: "done", tool_calls: 47 },
        { id: "in-4", name: "plan·cleanup", state: "done", tool_calls: 5 },
        { id: "pane-w", name: "code-reviewer·live", state: "running", tool_calls: 9 },
        { id: "pane-d", name: "code-reviewer·Gemini design review", state: "done", tool_calls: 37 },
      ] });
      const working = term + 7100;
      const finished = term + 7101;
      tell("term:split", { parent: term, term: working, direction: "vertical", agent: "zo", helper: "pane-w" });
      tell("hook:agent", { term: working, state: "working", agent: "zo", session: "zo-w" });
      tell("term:split", { parent: term, term: finished, direction: "vertical", agent: "zo", helper: "pane-d" });
      tell("hook:agent", { term: finished, state: "done", agent: "zo", session: "zo-d" });
      // The person is looking at the lead, not at either helper pane.
      await focusAgentPane(path, owner.id, term, "zo");
      await window.__PAINTED__();
      const said = (node) => node.dataset.history
        ? `history:${node.dataset.history}`
        : node.dataset.sub ?? `term:${node.dataset.term === String(term) ? "lead" : node.dataset.term}`;
      seen.folded = drawn().map(said);
      seen.historyAfterWork = (() => {
        const rows = drawn();
        const history = rows.findIndex((node) => node.dataset.history);
        return history > 0 && rows.slice(history + 1).every((node) => !node.classList.contains("is-working"));
      })();
      drawn().find((node) => node.dataset.history)?.click();
      await window.__PAINTED__();
      seen.opened = drawn().map(said);
      drawn().find((node) => node.dataset.history)?.click();
      await window.__PAINTED__();
      // The finished pane on stage stands where it can be seen.
      await focusAgentPane(path, owner.id, finished, "zo");
      await window.__PAINTED__();
      seen.stagedStands = drawn().some((node) => node.dataset.term === String(finished) && !node.dataset.history);
      await focusAgentPane(path, owner.id, term, "zo");
      await window.__PAINTED__();

      // Claude Code: a lead and a finished team pane — the same rule.
      const lead = await openTermTab({ placement: "tab" });
      const leadTab = tabOfTerm(lead);
      tell("hook:agent", { term: lead, state: "working", agent: "claude", session: "cc-lead" });
      const mate = lead + 7200;
      tell("term:split", { parent: lead, term: mate, direction: "vertical", agent: "claude" });
      tell("hook:agent", { term: mate, state: "done", agent: "claude", session: "cc-mate" });
      await focusAgentPane(leadTab.worktree, leadTab.id, lead, "claude");
      await window.__PAINTED__();
      const ccRows = worktreeAgentRows(leadTab.worktree).filter((row) => row.term === lead || row.term === mate);
      seen.claudeFolded = ccRows.map((row) => row.history ? `history:${row.history}` : row.term === lead ? "lead" : "mate");

      for (const done of [mate, lead, finished, working]) tell("term:exited", { term: done });
      tell("hook:subagent", { term, rows: [] });
      window.__PANES__ = [];
      for (const tab of [...tabs]) dropTab(tab.id);
      for (const at of [...termViews.keys()]) dropTermView(at);
      return seen;
    });
    ok(
      "finished agents a zo session started fold under one 「완료 N개」 line below the working ones, whether they ran inside zo or in a pane of their own; opening it shows each; a finished pane on stage stays out; a Claude Code team pane folds the same way",
      JSON.stringify(folds.folded) === JSON.stringify(["term:lead", "in-1", "in-2", "pane-w", "history:3"]) &&
        folds.historyAfterWork &&
        JSON.stringify(folds.opened) ===
          JSON.stringify(["term:lead", "in-1", "in-2", "pane-w", "in-3", "in-4", "pane-d", "history:3"]) &&
        folds.stagedStands &&
        JSON.stringify(folds.claudeFolded) === JSON.stringify(["lead", "history:1"]),
      JSON.stringify(folds),
    );

    /* Pressing a helper's row shows what that helper is doing. A helper
     * that runs inside its lead has no pane of its own; its row used to take
     * the person to the lead's pane — the pane already in front of them, so
     * the press seemed to do nothing. The row now opens the helper's own
     * conversation, the page its small reading hand opens. A helper whose
     * vendor left no conversation to read still goes to the lead's pane, and
     * the window says in one line why that is all there is to show. */
    const presses = await page.evaluate(async () => {
      const seen = {};
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      const term = await openTermTab({ placement: "tab" });
      const owner = tabOfTerm(term);
      const path = owner.worktree;
      const drawn = () => [...(document.querySelector(`.wt-agents[data-worktree-path="${CSS.escape(path)}"]`)
        ?.querySelectorAll(".wt-agent") ?? [])];
      paneAgents.set(term, "zo");
      tell("hook:agent", { term, state: "working", agent: "zo", session: "zo-press" });
      tell("hook:subagent", { term, rows: [
        { id: "read-me", name: "artifact-audit", state: "running", tool_calls: 3 },
        { id: "no-page", name: "quiet-helper", state: "running", tool_calls: 1 },
      ] });
      await focusAgentPane(path, owner.id, term, "zo");
      await window.__PAINTED__();
      window.__ANSWER__.subagent_log = (args) => args.id === "read-me"
        ? { found: true, next: 1, turns: [{ role: "assistant", text: "reading the manifests" }] }
        : { found: false };
      const toasts = () => [...document.querySelectorAll(".toasts .toast")].map((node) => node.textContent);
      const before = toasts().length;
      drawn().find((node) => node.dataset.sub === "read-me")?.click();
      await new Promise((done) => setTimeout(done, 120));
      seen.pageOpened = activeTabId === `helper:${term}:read-me`;
      drawn().find((node) => node.dataset.sub === "no-page")?.click();
      await new Promise((done) => setTimeout(done, 120));
      seen.leadInFront = activeTabId === owner.id && owner.activePane === term;
      seen.said = toasts().slice(before);
      seen.wantSaid = t("session.subagentNoTranscript", "이 에이전트는 따로 볼 기록을 남기지 않아 부모 에이전트의 화면을 엽니다");
      delete window.__ANSWER__.subagent_log;
      tell("hook:subagent", { term, rows: [] });
      tell("term:exited", { term });
      window.__PANES__ = [];
      for (const tab of [...tabs]) dropTab(tab.id);
      for (const at of [...termViews.keys()]) dropTermView(at);
      return seen;
    });
    ok(
      "pressing a helper's row opens that helper's own conversation; one with nothing to read brings its lead forward and says why in one line",
      presses.pageOpened && presses.leadInFront &&
        presses.said.length === 1 && presses.said[0].includes(presses.wantSaid),
      JSON.stringify(presses),
    );

    /* The keyboard reaches the same rows. Up and down move between the
     * agent rows of one card — they used to fall through to the list below
     * and move the whole window to another workspace — Home and End go to
     * the ends, Enter presses the row. Each row names itself to a screen
     * reader in one sentence: who, and in what state. */
    const keys = await page.evaluate(async () => {
      const seen = {};
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      const term = await openTermTab({ placement: "tab" });
      const owner = tabOfTerm(term);
      const path = owner.worktree;
      paneAgents.set(term, "zo");
      tell("hook:agent", { term, state: "working", agent: "zo", session: "zo-keys" });
      tell("hook:subagent", { term, rows: [
        { id: "k-1", name: "first-helper", state: "running", tool_calls: 2 },
        { id: "k-2", name: "second-helper", state: "running", tool_calls: 0 },
        { id: "k-3", name: "third-helper", state: "done", tool_calls: 8 },
      ] });
      await focusAgentPane(path, owner.id, term, "zo");
      await window.__PAINTED__();
      window.__KEYS_PATH__ = path;
      window.__KEYS_TERM__ = term;
      const rows = [...document.querySelector(`.wt-agents[data-worktree-path="${CSS.escape(path)}"]`).querySelectorAll(".wt-agent")];
      seen.names = rows.map((node) => node.getAttribute("aria-label") ?? "");
      seen.wantWorking = bucketWord("working");
      rows[0].focus();
      return seen;
    });
    const where = () => page.evaluate(() => {
      const rows = [...document.querySelector(`.wt-agents[data-worktree-path="${CSS.escape(window.__KEYS_PATH__)}"]`).querySelectorAll(".wt-agent")];
      return { at: rows.indexOf(document.activeElement), of: rows.length, workspace: activeWorktreePath };
    });
    const start = await where();
    await page.keyboard.press("ArrowDown");
    const down = await where();
    await page.keyboard.press("End");
    const end = await where();
    await page.keyboard.press("ArrowUp");
    const up = await where();
    await page.keyboard.press("Home");
    const home = await where();
    await page.keyboard.press("ArrowDown");
    await page.evaluate(() => {
      window.__ANSWER__.subagent_log = () => ({ found: true, next: 1, turns: [{ role: "assistant", text: "first" }] });
    });
    await page.keyboard.press("Enter");
    await page.waitForTimeout(120);
    const entered = await page.evaluate(() => {
      const opened = activeTabId === `helper:${window.__KEYS_TERM__}:k-1`;
      delete window.__ANSWER__.subagent_log;
      const term = window.__KEYS_TERM__;
      for (const handler of window.__LISTENERS__["term:exited"] ?? []) handler({ payload: { term } });
      window.__PANES__ = [];
      for (const tab of [...tabs]) dropTab(tab.id);
      for (const at of [...termViews.keys()]) dropTermView(at);
      return opened;
    });
    const moves = { start, down, end, up, home, entered, ...keys };
    ok(
      "the arrow keys move between one card's agent rows without leaving the workspace, Home and End reach the ends, Enter opens the row, and each row names who it is and its state",
      start.at === 0 && down.at === 1 && end.at === end.of - 1 && up.at === end.of - 2 && home.at === 0 &&
        [down, end, up, home].every((one) => one.workspace === start.workspace) &&
        entered &&
        keys.names.length >= 3 && keys.names.every(Boolean) &&
        keys.names[1].includes("first-helper") && keys.names[1].includes(keys.wantWorking),
      JSON.stringify(moves),
    );

    /* A row's tooltip — the full words a narrow row cuts — stands beside the
     * row, off the list, rather than below it over the next agent. */
    const tip = await page.evaluate(async () => {
      const seen = {};
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      const term = await openTermTab({ placement: "tab" });
      const owner = tabOfTerm(term);
      const path = owner.worktree;
      paneAgents.set(term, "zo");
      tell("hook:agent", { term, state: "working", agent: "zo", session: "zo-tip" });
      tell("hook:subagent", { term, rows: [
        { id: "t-1", name: "code-reviewer·Gemini design review of the knowledge graph plates", state: "running", tool_calls: 37 },
        { id: "t-2", name: "artifact-audit", state: "running", tool_calls: 76 },
        { id: "t-3", name: "deep-research·artifact-trends", state: "running", tool_calls: 47 },
      ] });
      await focusAgentPane(path, owner.id, term, "zo");
      await window.__PAINTED__();
      const rows = [...document.querySelector(`.wt-agents[data-worktree-path="${CSS.escape(path)}"]`).querySelectorAll(".wt-agent")];
      const target = rows.find((node) => node.dataset.sub === "t-1");
      target.focus();
      const pill = document.querySelector(".tooltip");
      seen.shown = pill ? !pill.hidden && pill.textContent.includes("code-reviewer") : false;
      const box = pill.getBoundingClientRect();
      const overlaps = (other) => {
        const at = other.getBoundingClientRect();
        return box.left < at.right && at.left < box.right && box.top < at.bottom && at.top < box.bottom;
      };
      seen.covers = rows.filter((node) => node !== target && overlaps(node)).map((node) => node.dataset.sub ?? node.dataset.term);
      seen.inWindow = box.left >= 0 && box.right <= window.innerWidth && box.top >= 0 && box.bottom <= window.innerHeight;
      target.blur();
      tell("hook:subagent", { term, rows: [] });
      tell("term:exited", { term });
      window.__PANES__ = [];
      for (const tab of [...tabs]) dropTab(tab.id);
      for (const at of [...termViews.keys()]) dropTermView(at);
      return seen;
    });
    ok(
      "an agent row's tooltip stands beside the row and covers no other agent row",
      tip.shown && tip.covers.length === 0 && tip.inWindow,
      JSON.stringify(tip),
    );

    /* Thirty agents under one zo session — fifteen at work, fifteen
     * finished and opened out — are all reachable: every row the model
     * lists is drawn, and each, scrolled to, is the thing under the
     * pointer at its own centre. */
    const reach = await page.evaluate(async () => {
      const seen = {};
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      const term = await openTermTab({ placement: "tab" });
      const owner = tabOfTerm(term);
      const path = owner.worktree;
      paneAgents.set(term, "zo");
      tell("hook:agent", { term, state: "working", agent: "zo", session: "zo-thirty" });
      tell("hook:subagent", { term, rows: Array.from({ length: 30 }, (_, at) => ({
        id: `many-${at}`, name: `helper-${at}`, state: at % 2 ? "done" : "running", tool_calls: at,
      })) });
      agentHistoryShown.add(term);
      await focusAgentPane(path, owner.id, term, "zo");
      await window.__PAINTED__();
      const host = document.querySelector(`.wt-agents[data-worktree-path="${CSS.escape(path)}"]`);
      const rows = [...host.querySelectorAll(".wt-agent")];
      seen.model = worktreeAgentRows(path).length;
      seen.drawn = rows.length;
      const hidden = [];
      for (const node of rows) {
        node.scrollIntoView({ block: "center" });
        const box = node.getBoundingClientRect();
        const hit = document.elementFromPoint(box.left + Math.min(box.width / 2, 40), box.top + box.height / 2);
        if (!hit || !node.contains(hit)) hidden.push(node.dataset.sub ?? node.dataset.history ?? node.dataset.term);
      }
      seen.hidden = hidden;
      agentHistoryShown.delete(term);
      tell("hook:subagent", { term, rows: [] });
      tell("term:exited", { term });
      window.__PANES__ = [];
      for (const tab of [...tabs]) dropTab(tab.id);
      for (const at of [...termViews.keys()]) dropTermView(at);
      return seen;
    });
    ok(
      "thirty agents under one zo session are all drawn and each can be reached and pressed",
      reach.drawn === reach.model && reach.model === 32 && reach.hidden.length === 0,
      JSON.stringify(reach),
    );
  } finally {
    await page.close();
  }
}
