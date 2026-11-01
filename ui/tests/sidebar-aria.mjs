import { createRequire } from "node:module";
import { openWindowTestPage } from "./window-boot.mjs";

/* The sidebar list and the tab strip tell a screen reader what they hold
 * (t-88672, from the t-77627 tail review A6; r2: the tree keys, the item that carries the active mark, the resting
 * line's own Enter).
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
      // What a screen reader's own tree check reads (WebKit drops a tree to a plain group when a group holds a
      // child with no role): the window's padding boxes say they are nothing, and an item that opens agrees with its head.
      const shape = await page.evaluate(() => ({
        bare: [...document.querySelectorAll("#worktrees .worktree-list > *")]
          .filter((one) => !["treeitem", "presentation"].includes(one.getAttribute("role") ?? "")).map((one) => one.className),
        split: [...document.querySelectorAll("#worktrees .proj[role=treeitem]")].filter((item) => {
          const head = item.querySelector(":scope > .proj-row");
          return item.hasAttribute("aria-expanded") && head && head.getAttribute("aria-expanded") !== item.getAttribute("aria-expanded");
        }).map((one) => one.dataset.projectPath ?? one.className),
      }));
      ok(`every child of a checkout list has a role it may have, and no tree item opens when its head says shut — ${name}`,
        shape.bare.length === 0 && shape.split.length === 0, JSON.stringify(shape));
    }
    await page.evaluate(() => { restingProjectsOpen = false; setSidebarView({ groupBy: "repo" }); });
    await page.evaluate(() => refreshWorktrees());
    await page.evaluate(() => window.__PAINTED__());

    // An item with nothing under it is not open or shut: it says neither.
    const hollow = await page.evaluate(() => {
      const node = document.createElement("div");
      const head = document.createElement("div");
      head.innerHTML = '<span class="proj-name">hollow</span>';
      const kids = document.createElement("div");
      kids.hidden = true;
      treeParent(node, head, kids, null);
      return { role: node.getAttribute("role"), expanded: node.getAttribute("aria-expanded"), kids: kids.getAttribute("role") };
    });
    ok("a tree item with nothing under it carries no aria-expanded", hollow.role === "treeitem" && hollow.expanded === null && hollow.kids === "group", JSON.stringify(hollow));

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

    // ④ The resting line is still a button a real key opens (its Enter is its own, not the terminal's).
    await page.evaluate(() => {
      window.__KEYS__ = [];
      window.__ANSWER__.term_key = ({ press }) => { window.__KEYS__.push(press?.key ?? "?"); return null; };
      window.__ANSWER__.key_input = ({ press }) => { window.__KEYS__.push(press?.key ?? "?"); return null; };
    });
    await page.focus(".proj-resting");
    await page.keyboard.press("Enter");
    await page.evaluate(() => window.__PAINTED__());
    const opened = await page.evaluate(() => {
      const one = document.querySelector(".proj-resting");
      return { button: one?.tagName === "BUTTON", tabbable: one?.tabIndex === 0, expanded: one?.getAttribute("aria-expanded"),
        keys: [...window.__KEYS__], item: one?.closest("[role=treeitem]") !== null };
    });
    ok("the resting line stays a tabbable button inside its tree item, and a real Enter opens it without reaching the terminal",
      opened.button && opened.tabbable && opened.item && opened.expanded === "true" && opened.keys.length === 0, JSON.stringify(opened));

    // ④b The tree's keys (t-88672 r2): Left shuts the project the current checkout stands in, Right opens it, Enter
    //    opens the checkout — and none of the three goes to the terminal. The arrows and Home/End are as they were.
    await page.evaluate(() => { restingProjectsOpen = false; setSidebarView({ groupBy: "repo" }); closedProjects.delete("/z"); });
    await page.evaluate(() => refreshWorktrees());
    await page.evaluate(() => window.__PAINTED__());
    const tree = async (key, from) => {
      await page.evaluate((path) => {
        activeWorktreePath = path;
        replaceWorktreeSelection(path);
        paintWorktreeSelection();
        paintWorktreeActive();
        window.__KEYS__.length = 0;
        document.getElementById("worktrees").focus();
      }, from);
      await page.keyboard.press(key);
      await page.evaluate(() => refreshWorktrees());
      await page.evaluate(() => window.__PAINTED__());
      return page.evaluate(() => ({
        keys: [...window.__KEYS__],
        shut: closedProjects.has("/z"),
        item: document.querySelector('.proj[data-project-path="/z"]')?.getAttribute("aria-expanded") ?? null,
        head: document.querySelector('.proj-row[data-project-path="/z"]')?.getAttribute("aria-expanded") ?? null,
        focus: document.activeElement?.id ?? "",
        selected: [...selectedWorktreePaths].sort(),
      }));
    };
    const left = await tree("ArrowLeft", "/z/wt/one");
    ok("Left shuts the project the current checkout stands in — item and head agree — and the terminal hears nothing",
      left.shut && left.item === "false" && left.head === "false" && left.keys.length === 0 && left.focus === "worktrees", JSON.stringify(left));
    const right = await tree("ArrowRight", "/z/wt/one");
    ok("Right opens it again, and the terminal hears nothing",
      !right.shut && right.item === "true" && right.head === "true" && right.keys.length === 0 && right.focus === "worktrees", JSON.stringify(right));
    const enter = await tree("Enter", "/z/wt/two");
    ok("Enter opens the current checkout (the row's own press), and the terminal hears nothing",
      enter.selected.join() === "/z/wt/two" && enter.keys.length === 0, JSON.stringify(enter));
    const pointed = await page.evaluate(() => {
      const list = document.getElementById("worktrees");
      const target = document.getElementById(list.getAttribute("aria-activedescendant") ?? "");
      return {
        id: list.getAttribute("aria-activedescendant"),
        role: target?.getAttribute("role") ?? null,
        path: target?.querySelector(":scope > .wt-row")?.dataset.worktreePath ?? null,
        active: activeWorktreePath,
        currentOnItem: [...document.querySelectorAll('#worktrees [aria-current="location"]')].map((one) => one.className),
        currentOnRow: document.querySelectorAll("#worktrees .wt-row[aria-current]").length,
      };
    });
    ok("the list points at the current checkout's tree item (aria-activedescendant), and 'current' stands on the item, not on the plain row inside it",
      pointed.role === "treeitem" && pointed.path === pointed.active && pointed.currentOnRow === 0
        && pointed.currentOnItem.length === 1 && pointed.currentOnItem[0].includes("wt-node"),
      JSON.stringify(pointed));
    await page.evaluate(() => { closedProjects.delete("/z"); activeWorktreePath = "/z"; replaceWorktreeSelection("/z"); paintWorktreeActive(); });
    await page.evaluate(() => refreshWorktrees());
    await page.evaluate(() => window.__PAINTED__());

    // ⑤ Nothing in the list wears a name it may not (axe: a name on a plain box).
    const named = await ownedChildrenViolations(page, "#worktrees", ["aria-prohibited-attr", "aria-allowed-attr", "aria-valid-attr-value"]);
    ok("the sidebar list's names and states stand on elements that may wear them (axe)", named.length === 0, JSON.stringify(named));
    ok("the scene raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}
