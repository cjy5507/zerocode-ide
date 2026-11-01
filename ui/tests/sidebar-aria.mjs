import { createRequire } from "node:module";
import { openWindowTestPage } from "./window-boot.mjs";

/* The sidebar list and the tab strip tell a screen reader what they hold
 * (t-88672, from the t-77627 tail review A6).
 *
 * `#worktrees` stood as a listbox with project headers, the resting line and
 * cards inside it, and `#tabstrip` as a tablist with the menu, the detached
 * pill and the new-tab button inside it: axe's `aria-required-children` fails
 * both, and a listbox/tablist that owns a button reads wrong. The fixture is
 * the person's window in small — one project at work, projects resting, agents
 * in tabs — and nobody's pane or file is read. */

const frames = (page) => page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));

export async function standSidebarAria(page) {
  await page.waitForFunction(() => projectsRead);
  await page.evaluate(async () => {
    setLocale("ko", { persist: false });
    const now = Date.now();
    const checkout = (path, branch, over = {}) => ({ path, branch, is_main: false, active: false, is_folder: false,
      ownership: "zerocode-managed", external_hidden: false, last_activity_ms: now, ...over });
    const resting = ["alpha", "beta", "gamma"];
    window.__ANSWER__.project_catalog = () => [
      { name: "work", path: "/z", worktrees: [
        checkout("/z", "main", { is_main: true, active: true }),
        checkout("/z/wt/one", "wt/t-1/one"),
        checkout("/z/wt/two", "wt/t-2/two"),
        checkout("/z/wt/three", "wt/t-3/three"),
      ] },
      ...resting.map((name) => ({ name, path: `/p/${name}`, worktrees: [checkout(`/p/${name}`, "main", { is_main: true })] })),
    ];
    window.__ANSWER__.set_sidebar_view = () => null;
    setEveryProjectClosed(false);
    startupProjectFoldsReady = true;
    for (const name of resting) closedProjects.add(`/p/${name}`);
    setSidebarView({ groupBy: "repo" });
    for (const agent of ["claude", "zo"]) {
      const term = await openTermTab({ placement: "tab" });
      tabOfTerm(term).worktree = "/z";
      paneAgents.set(term, agent);
      for (const handler of window.__LISTENERS__["hook:agent"] ?? []) handler({ payload: { term, state: "working", agent, session: `s-${term}` } });
    }
    await refreshWorktrees();
    renderTabs();
    await window.__PAINTED__();
  });
  await frames(page);
}

/* axe's verdicts for the required-children rule (and the nested-control rule
 * a row full of buttons meets) under `include`. */
export async function ownedChildrenViolations(page, include, rules = ["aria-required-children", "aria-required-parent", "nested-interactive"]) {
  const axeModule = createRequire(import.meta.url)("@axe-core/playwright");
  const AxeBuilder = axeModule.default ?? axeModule;
  const answer = await new AxeBuilder({ page }).include(include).withRules(rules).analyze();
  return answer.violations.map(({ id, nodes }) => ({
    id,
    nodes: nodes.map(({ target, failureSummary }) => ({ target, failureSummary })),
  }));
}

/* The rows' own state, read off the page: which checkout is current, which are
 * selected, and what the keyboard has done to them. */
const standing = (page) => page.evaluate(() => ({
  active: activeWorktreePath,
  selected: [...selectedWorktreePaths].sort(),
  marked: [...document.querySelectorAll('#worktrees [role="treeitem"][aria-selected=true] > .wt-row[data-worktree-path]')].map((row) => row.dataset.worktreePath).sort(),
  focus: document.activeElement?.id ?? document.activeElement?.className ?? "",
}));

const RULES = ["aria-required-children", "aria-required-parent"];

export async function testSidebarAria({ browser, origin, ok, faults }) {
  const { page } = await openWindowTestPage(browser, origin, { faults });
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await standSidebarAria(page);

    // ① The list in each of its three shapes, the resting line shut and open.
    const seen = {};
    for (const [name, change] of [
      ["by repository", () => setSidebarView({ groupBy: "repo" })],
      ["by repository, resting line open", () => { restingProjectsOpen = true; setSidebarView({ groupBy: "repo" }); }],
      ["by state", () => { restingProjectsOpen = false; setSidebarView({ groupBy: "state" }); }],
      ["not grouped", () => setSidebarView({ groupBy: "none" })],
    ]) {
      await page.evaluate(change);
      await page.evaluate(() => refreshWorktrees());
      await page.evaluate(() => window.__PAINTED__());
      seen[name] = await ownedChildrenViolations(page, "#worktrees", RULES);
      const rows = await page.evaluate(() => document.querySelectorAll("#worktrees [data-worktree-path]").length);
      ok(`the sidebar list owns only what its role allows — ${name} (axe aria-required-children)`,
        seen[name].length === 0 && rows >= 3, JSON.stringify({ rows, found: seen[name] }));
    }
    await page.evaluate(() => { restingProjectsOpen = false; setSidebarView({ groupBy: "repo" }); });
    await page.evaluate(() => refreshWorktrees());
    await page.evaluate(() => window.__PAINTED__());

    // ② The tab strip: tabs alone in the tablist, the menu, the detached pill
    //    and the new-tab button beside them — and standing where they stood.
    const strip = await ownedChildrenViolations(page, "#tabstrip", RULES);
    const shape = await page.evaluate(() => {
      const box = (node) => { const one = node.getBoundingClientRect(); return { left: Math.round(one.left), right: Math.round(one.right), top: Math.round(one.top), width: Math.round(one.width) }; };
      const list = document.querySelector('#tabstrip [role="tablist"]');
      const tabsIn = [...document.querySelectorAll("#tabstrip .tab")];
      const tail = ["tab-new"].map((name) => document.querySelector(`#tabstrip .${name}`));
      const last = tabsIn.at(-1);
      return {
        tablists: document.querySelectorAll('#tabstrip [role="tablist"], #tabstrip[role="tablist"]').length,
        tabs: tabsIn.length,
        tabsInside: list ? tabsIn.filter((tab) => list.contains(tab)).length : 0,
        buttonsInside: list ? [...list.querySelectorAll("button")].filter((one) => one.closest("[role=tab]") === null).length : -1,
        named: list?.getAttribute("aria-label") ?? null,
        tabBoxes: tabsIn.map(box),
        afterLast: tail.map((one) => (one && last ? box(one).left >= box(last).right - 1 && Math.abs(box(one).top - box(last).top) <= 8 : false)),
        tailOrder: [...document.querySelectorAll("#tabstrip > .tab-menu, #tabstrip > .tab-detached, #tabstrip > .tab-new")].map((one) => one.className.split(" ")[0]),
      };
    });
    ok("the tab strip's tablist owns only tabs (axe aria-required-children)",
      strip.length === 0 && shape.tablists === 1 && shape.tabs >= 2 && shape.tabsInside === shape.tabs && shape.buttonsInside === 0,
      JSON.stringify({ strip, shape }));
    ok("the tablist is named and the menu and new-tab button stand after the last tab on its row, in the order they had",
      shape.named === "탭" && shape.afterLast.every(Boolean) && shape.tailOrder.join() === "tab-menu,tab-detached,tab-new",
      JSON.stringify(shape));

    // ③ The keyboard is the person's and does not change: real keys. A press
    //    that activates a checkout hands the focus on to the key sink, as it
    //    always has, so the list is focused again before each key.
    // The handler steps from the checkout the window stands in; the scene
    // has no checkout to switch to, so each press is told where it stands.
    const press = async (key, from, picked = []) => {
      await page.evaluate(([path, held]) => {
        activeWorktreePath = path;
        selectedWorktreePaths.clear();
        for (const one of held) selectedWorktreePaths.add(one);
        selectionAnchorPath = held[0] ?? null;
        document.getElementById("worktrees").focus();
      }, [from, picked]);
      await page.keyboard.press(key);
      await page.evaluate(() => window.__PAINTED__());
      return standing(page);
    };
    const down = await press("ArrowDown", "/z/wt/one");
    const up = await press("ArrowUp", "/z/wt/one");
    const end = await press("End", "/z");
    const home = await press("Home", "/z/wt/two");
    ok("the arrows, Home and End step through the checkouts in the order the list shows them",
      down.selected.join() === "/z/wt/two" && up.selected.join() === "/z"
        && end.selected.join() === "/z/wt/three" && home.selected.join() === "/z"
        && [down, up, end, home].every((one) => one.marked.join() === one.selected.join()),
      JSON.stringify({ down, up, end, home }));
    await page.evaluate(() => { activeWorktreePath = "/z"; replaceWorktreeSelection("/z"); paintWorktreeSelection(); });
    await page.click('.wt-row[data-worktree-path="/z/wt/two"]', { modifiers: ["Shift"] });
    const range = await standing(page);
    ok("a Shift press selects the range from the anchor, and every selected row says so",
      range.selected.join() === ["/z", "/z/wt/one", "/z/wt/two"].join() && range.marked.join() === range.selected.join(),
      JSON.stringify(range));
    const toggled = await (async () => {
      await page.click('.wt-row[data-worktree-path="/z/wt/three"]', { modifiers: ["ControlOrMeta"] });
      return standing(page);
    })();
    ok("a primary-modifier press toggles one row into the selection and its mark follows",
      toggled.marked.join() === toggled.selected.join() && toggled.selected.includes("/z/wt/three"), JSON.stringify(toggled));
    await page.evaluate(() => document.getElementById("worktrees").focus());
    await page.keyboard.press("ContextMenu");
    const menu = await page.evaluate(() => !document.getElementById("sidebar-menu").hidden);
    ok("the context-menu key opens the checkout's menu", menu, String(menu));
    await page.keyboard.press("Escape");

    // ④ The resting line is still a button a press opens.
    const opened = await page.evaluate(async () => {
      const one = document.querySelector(".proj-resting");
      const was = { button: one?.tagName === "BUTTON", tabbable: one?.tabIndex === 0 };
      one?.click();
      await window.__PAINTED__();
      return { ...was, expanded: document.querySelector(".proj-resting")?.getAttribute("aria-expanded") };
    });
    ok("the resting line stays a tabbable button and a press opens it",
      opened.button && opened.tabbable && opened.expanded === "true", JSON.stringify(opened));
    // ⑤ Nothing in the list wears a name it may not (axe: a name on a plain box).
    const named = await ownedChildrenViolations(page, "#worktrees", ["aria-prohibited-attr", "aria-allowed-attr", "aria-valid-attr-value"]);
    ok("the sidebar list's names and states stand on elements that may wear them (axe)", named.length === 0, JSON.stringify(named));
    ok("the scene raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}
