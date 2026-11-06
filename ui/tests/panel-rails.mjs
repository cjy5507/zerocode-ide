import { openWindowTestPage, recordBootPhases, standRails, waitBootSettled } from "./window-boot.mjs";
import { installHarnessWaits } from "./harness-waits.mjs";
import { ownedChildrenViolations, standSidebarAria } from "./sidebar-aria.mjs";
import { installTurns } from "./your-turn.mjs";
import { standTeamScene } from "./sidebar-team.mjs";

/* The two rails of the passed draft's item 17 (p-6893560125c34552, t-84803).
 *
 * At or under `--rail-fold-panel` the right column is a rail of its tabs with
 * the pile's count on the first, at or under `--rail-fold-sidebar` the sidebar
 * is a rail of marks, at or under `--rail-fold-away` the two leave the window's
 * width and the title bar's two toggles bring them back. What a rail hides
 * opens as a sheet over the stage — the stage keeps its box, so its terminal
 * hears nothing. A token at 0px turns its rail off and nothing else moves.
 *
 * The fixture is the window in small: invented checkouts under invented folders
 * (`standSidebarAria`), three invented questions (`installTurns`); nobody's pane
 * or file is read.
 *
 *   WINDOW_SUITES=panel-rails node ui/tests/window.mjs */

export const LOCALES = ["ko", "en", "ja", "zh", "es"];

const frames = (page) => page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
const settle = (page) => page.evaluate(() => new Promise((done) => setTimeout(done, 300)));

/* A person's fold has run: the window takes the class off when its easing has settled, however long the frames took. */
const foldEased = (page) => page.waitForFunction(() => !el("workbench").classList.contains("is-folding"));

async function width(page, px) {
  await page.setViewportSize({ width: px, height: 800 });
  await frames(page);
}

/* The checkouts wear each landing the sidebar has a chip for, one agent asks,
 * and the attention column holds three questions. */
export async function standRailFixture(page) {
  await installHarnessWaits(page);
  await page.evaluate(installTurns);
  await standSidebarAria(page);
  await page.evaluate(async () => {
    const landing = (state, over = {}) => ({ state, detached: false, ahead: 0, dirty: false,
      dirty_checked_ms: Date.now() - 1000, compare_ref: "origin/main", ref_updated_ms: Date.now() - 86_400_000, ...over });
    const told = window.__ANSWER__.project_catalog;
    const lands = { "/z/wt/one": landing("landed"), "/z/wt/two": landing("unlanded", { ahead: 13 }), "/z/wt/three": landing("unlanded", { ahead: 1, conflict: { total: 1, files: ["a.txt"] } }) };
    window.__ANSWER__.project_catalog = () => told().map((project) => ({ ...project,
      worktrees: project.worktrees.map((one) => (lands[one.path] ? { ...one, landing: lands[one.path] } : one)) }));
    await refreshWorktrees();
    const [term] = [...paneAgents.keys()];
    for (const handler of window.__LISTENERS__["hook:agent"] ?? []) {
      handler({ payload: { term, state: "needs-attention", agent: "claude", session: `s-${term}` } });
    }
    await window.__setTurns(window.__turnCards());
    await window.__PAINTED__();
  });
  await frames(page);
}

/* The window's three regions and whether anything of a rail lies outside it. */
const readLayout = () => {
  const box = (node) => {
    const rect = node.getBoundingClientRect();
    return { left: rect.left, right: rect.right, width: rect.width, height: rect.height };
  };
  const visible = (node) => {
    const rect = node.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return false;
    for (let at = node; at && at !== document.body; at = at.parentElement) {
      const style = getComputedStyle(at);
      if (style.display === "none" || style.visibility === "hidden") return false;
    }
    return true;
  };
  const threads = document.querySelector(".threads");
  const files = document.querySelector(".files");
  const outside = [];
  for (const [panel, selector] of [[threads, "button, .proj-row, .wt-row, .wt-agent"], [files, "button"]]) {
    if (!visible(panel)) continue;
    const edge = box(panel);
    for (const node of panel.querySelectorAll(selector)) {
      if (!visible(node)) continue;
      const rect = box(node);
      if (rect.left < edge.left - 1 || rect.right > edge.right + 1) outside.push(node.id || node.className);
    }
  }
  return {
    page: document.documentElement.scrollWidth,
    threads: box(threads), threadsShown: visible(threads),
    files: box(files), filesShown: visible(files),
    stage: box(document.querySelector(".stage")),
    toggleRight: box(el("toggle-aside")).right,
    outside,
    // The title bar's tab on stage: what it holds stays in it, and it stands clear of the menu beside it.
    tabSpill: (() => {
      const front = document.querySelector("#tabstrip .tab.is-front");
      const menu = document.querySelector("#tabstrip .tab-menu");
      if (!front) return [];
      const edge = box(front);
      const spilled = [...front.children].filter(visible).filter((one) => box(one).right > edge.right + 1).map((one) => one.className);
      if (menu && visible(menu) && box(menu).left < edge.right - 1) spilled.push("under the menu");
      return spilled;
    })(),
    floatDoor: visible(el("float-toggle")),
  };
};

/* A control the rail shows, with its name and its tip: what a reader hears and
 * what the eye gets on hover. */
const readRailItems = () => {
  const visible = (node) => {
    const rect = node.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return false;
    for (let at = node; at && at !== document.body; at = at.parentElement) {
      const style = getComputedStyle(at);
      if (style.display === "none" || style.visibility === "hidden") return false;
    }
    return true;
  };
  const nameOf = (node) => {
    const labelled = node.getAttribute("aria-labelledby");
    if (labelled) return labelled.split(" ").map((id) => el(id)?.textContent ?? "").join(" ").trim();
    return (node.getAttribute("aria-label") ?? node.textContent ?? "").trim();
  };
  const items = [];
  for (const [panel, selector] of [[".threads.is-railed", "button, .proj-row, .wt-row, .wt-agent"], [".files.is-railed", "button"]]) {
    for (const node of document.querySelector(panel)?.querySelectorAll(selector) ?? []) {
      if (!visible(node) || node.closest(".wt-landing")) continue;
      // A checkout's name is its tree item's, the row inside it a plain box (t-88672).
      const named = node.classList.contains("wt-row") ? node.closest(".wt-node") : node;
      items.push({ panel, what: node.id || node.className, name: nameOf(named), tip: node.closest("[data-tip]")?.dataset.tip ?? "" });
    }
  }
  return items;
};

async function testFolds(page, ok) {
  await page.evaluate(() => { setActivityItem("scm"); });
  const seen = [];
  for (const px of [1600, 1361, 1360, 1181, 1180, 1001, 1000, 761, 760, 641, 640, 560, 520, 481, 480, 391, 390]) {
    await width(page, px);
    const at = await page.evaluate(readLayout);
    seen.push({ px, ...at });
    const tag = `${px} px`;
    ok(`nothing leaves the window, and nothing of a rail leaves its rail (${tag})`,
      at.page <= px && at.stage.right <= px + 1 && at.toggleRight <= px && at.outside.length === 0, JSON.stringify(at));
    ok(`the tab on stage holds its marks and stands clear of the tab menu (${tag})`, at.tabSpill.length === 0, JSON.stringify(at.tabSpill));
    ok(`the floating trigger stands only where no rail holds its door (${tag})`, at.floatDoor === (px > 1360), JSON.stringify(at.floatDoor));
    ok(`the stage stands clear of both columns (${tag})`,
      at.stage.left >= (at.threadsShown ? at.threads.right : 0) - 1 && at.stage.right <= (at.filesShown ? at.files.left : px) + 1,
      JSON.stringify(at));
    if (px > 1360) {
      ok(`above 1360 px the right column is its full self (${tag})`, at.files.width >= 200 && at.threads.width >= 240, JSON.stringify(at));
    } else if (px > 1000) {
      ok(`at or under 1360 px the right column is a 52 px rail and the sidebar stays whole (${tag})`,
        at.files.width === 52 && at.threads.width >= 240, JSON.stringify(at));
    } else if (px > 480) {
      ok(`at or under 1000 px the sidebar is a 60 px rail beside the right column's 52 px (${tag})`,
        at.threads.width === 60 && at.files.width === 52, JSON.stringify(at));
    } else {
      ok(`at or under 480 px both leave the width and the stage has the window (${tag})`,
        !at.threadsShown && !at.filesShown && at.stage.width === px, JSON.stringify(at));
    }
  }
  return seen;
}

/* Dragging the window's edge asks the whole document's style again if anything inherited moves with it: the floating
 * trigger's spot stood on <body> and every step recalculated all 1454 elements of this fixture, 6.5 ms a step. The one
 * neighbour that reads the spot wears it. */
async function testDragStyle(page, ok) {
  const read = () => page.evaluate(() => ({
    body: document.body.style.getPropertyValue("--float-trigger-left"),
    launcher: document.querySelector(".sftp-launcher")?.style.getPropertyValue("--float-trigger-left") ?? null,
  }));
  await width(page, 1500);
  const first = await read();
  await width(page, 1400);
  const second = await read();
  ok("the floating trigger's spot moves with the window on its one neighbour, and nothing the whole document inherits moves",
    first.body === "" && second.body === "" && first.launcher !== second.launcher && second.launcher !== null, JSON.stringify({ first, second }));
}

/* A step of the window's edge moves the columns in the same frame, and only a person's fold eases (t-84803 r3). Eased,
 * a step from 1280 to 720 px left the stage 90 px wide in its first frame and gave it back over 110 ms: the Skills
 * screen's two pickers were measured in that squeeze and read their words under their arrows (the gate's l97c, l84a).
 * Read with the rails on and with them off, the way the harness stands most scenes. */
async function testEdgeSnaps(page, ok, how) {
  const read = () => page.evaluate(() => new Promise((done) => requestAnimationFrame(() => {
    const bench = el("workbench");
    done({
      easing: bench.getAnimations().filter((one) => one.transitionProperty === "grid-template-columns").length,
      stage: Math.round(el("stage").getBoundingClientRect().width),
      folding: bench.classList.contains("is-folding"),
    });
  })));
  const steps = [];
  for (const [from, to] of [[1280, 720], [720, 1440], [1440, 1100], [1100, 900]]) {
    await width(page, from);
    await settle(page);
    await page.setViewportSize({ width: to, height: 800 });
    const first = await read();
    await settle(page);
    const stood = await read();
    steps.push({ from, to, first, stood });
    ok(`a step of the window's edge from ${from} to ${to} px moves no column by easing: the stage stands at its own width in the first frame (${how})`,
      first.easing === 0 && !first.folding && Math.abs(first.stage - stood.stage) <= 1, JSON.stringify({ first, stood }));
  }
  // A person's fold still eases, and the easing is gone once it has run. Read in the press's own task: on a machine
  // that draws a frame every 100 ms two frames are longer than the fold.
  const press = () => page.evaluate(() => {
    el("toggle-sidebar").click();
    const bench = el("workbench");
    return { easing: bench.getAnimations().filter((one) => one.transitionProperty === "grid-template-columns").length,
      folding: bench.classList.contains("is-folding") };
  });
  await width(page, 1440);
  await settle(page);
  const pressed = await press();
  await foldEased(page);
  const after = await read();
  await press();
  await foldEased(page);
  ok(`a person's fold of the sidebar eases its column, and the easing leaves with it (${how})`,
    pressed.easing === 1 && pressed.folding && after.easing === 0 && !after.folding, JSON.stringify({ pressed, after }));
  // With reduced motion it arrives without travelling, and the class does not outstay the press.
  await page.emulateMedia({ reducedMotion: "reduce" });
  const reduced = await press();
  await foldEased(page);
  await press();
  await foldEased(page);
  await page.emulateMedia({ reducedMotion: "no-preference" });
  ok(`with reduced motion a person's fold of the sidebar does not ease (${how})`, reduced.easing === 0, JSON.stringify(reduced));
  return steps;
}

async function testTerminal(page, ok) {
  await page.evaluate(async () => {
    window.__GRIDS__ = [];
    window.__ANSWER__.term_resize = ({ rows, cols }) => { window.__GRIDS__.push({ rows, cols }); return null; };
    // A plain shell on stage: an agent's pane stands its conversation over the screen.
    await openTermTab({ placement: "tab" });
    await window.__PAINTED__();
  });
  const grids = () => page.evaluate(() => window.__GRIDS__.splice(0));

  await width(page, 1500);
  await settle(page);
  await grids();

  /* A pause at each width: a rail folding is one telling, the same width again none. */
  await width(page, 1360);
  await settle(page);
  const crossed = await grids();
  await width(page, 1360);
  await settle(page);
  const again = await grids();
  ok("the right column folding to its rail tells the terminal once, and the same width again tells it nothing",
    crossed.length === 1 && again.length === 0, JSON.stringify({ crossed, again }));

  /* Sheets float: the stage's box does not move and the terminal hears nothing. */
  for (const [px, side] of [[1200, "aside"], [900, "aside"], [900, "sidebar"], [390, "aside"], [390, "sidebar"]]) {
    await width(page, px);
    await settle(page);
    await grids();
    const before = await page.evaluate(() => { const r = document.querySelector(".stage").getBoundingClientRect(); return [r.left, r.width]; });
    await page.evaluate((side) => openRailSheet(side), side);
    await settle(page);
    const during = await page.evaluate(() => { const r = document.querySelector(".stage").getBoundingClientRect(); return [r.left, r.width]; });
    await page.evaluate((side) => closeRailSheet(side), side);
    await settle(page);
    const told = await grids();
    ok(`a sheet (${side}, ${px} px) leaves the stage's box as it was and tells the terminal nothing`,
      before.join() === during.join() && told.length === 0, JSON.stringify({ before, during, told }));
  }

  /* A drag, ten pixels at a time without a pause: the grid never goes back the
   * way the window is going except where a column folds — the right column's rail
   * at 1360 px, the sidebar's 240 px at 1180 px, its rail at 1000 px, the two
   * leaving at 480 px — and the rows hold. */
  await width(page, 1600);
  await settle(page);
  await grids();
  for (let px = 1600; px >= 390; px -= 10) await page.setViewportSize({ width: px, height: 800 });
  await settle(page);
  const dragged = await grids();
  const rises = dragged.filter((grid, at) => at > 0 && grid.cols > dragged[at - 1].cols).length;
  const repeats = dragged.filter((grid, at) => at > 0 && grid.cols === dragged[at - 1].cols && grid.rows === dragged[at - 1].rows).length;
  ok("a drag across every fold widens the terminal only where a column folds, never tells it one grid twice, and its rows hold",
    dragged.length > 0 && rises <= 4 && repeats === 0 && new Set(dragged.map((grid) => grid.rows)).size === 1,
    JSON.stringify({ rises, repeats, dragged }));
  await page.evaluate(() => { delete window.__ANSWER__.term_resize; });
}

async function testNamed(page, ok) {
  for (const px of [1200, 900]) {
    await width(page, px);
    for (const locale of LOCALES) {
      await page.evaluate((locale) => setLocale(locale, { persist: false }), locale);
      await frames(page);
      const items = await page.evaluate(readRailItems);
      const bare = items.filter((item) => item.name === "" || item.tip === "");
      ok(`every control of a rail has a name and a tip (${locale}, ${px} px)`,
        items.length > 4 && bare.length === 0, JSON.stringify(bare));
    }
    await page.evaluate(() => setLocale("ko", { persist: false }));
  }
  for (const theme of ["dark", "light"]) {
    await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
    await width(page, 900);
    await frames(page);
    for (const region of [".threads", ".files"]) {
      const wrong = await ownedChildrenViolations(page, region,
        ["aria-required-children", "aria-required-parent", "nested-interactive", "button-name", "aria-allowed-attr", "aria-prohibited-attr", "color-contrast"]);
      ok(`axe finds nothing in the rail ${region} (${theme})`, wrong.length === 0, JSON.stringify(wrong));
    }
    await page.evaluate(() => openRailSheet("aside"));
    await frames(page);
    const wrong = await ownedChildrenViolations(page, ".files", ["aria-required-children", "aria-required-parent", "nested-interactive", "button-name", "aria-allowed-attr", "color-contrast"]);
    ok(`axe finds nothing in the right column's sheet (${theme})`, wrong.length === 0, JSON.stringify(wrong));
    await page.evaluate(() => closeRailSheet("aside"));
  }
  await page.evaluate(() => { document.documentElement.dataset.theme = "dark"; });
}

async function testKeyboard(page, ok) {
  await width(page, 900);
  const focus = () => page.evaluate(() => document.activeElement?.id || document.activeElement?.className || "");

  /* The door to the whole sidebar: Enter opens, Escape closes and hands the focus back. */
  await page.focus("#sidebar-rail-open");
  await page.keyboard.press("Enter");
  await frames(page);
  const opened = await page.evaluate(() => ({ railed: document.querySelector(".threads").classList.contains("is-railed"),
    width: document.querySelector(".threads").getBoundingClientRect().width,
    named: document.querySelector("#worktrees .wt-title").getBoundingClientRect().width,
    search: document.querySelector(".nav-search-word").getBoundingClientRect().width,
    inside: document.querySelector(".threads").contains(document.activeElement) }));
  ok("Enter on the rail's door opens the whole sidebar over the stage, names back and the focus in it",
    !opened.railed && opened.width >= 240 && opened.named > 8 && opened.search > 8 && opened.inside, JSON.stringify(opened));
  await page.keyboard.press("Escape");
  await frames(page);
  const shut = await page.evaluate(() => document.querySelector(".threads").classList.contains("is-railed"));
  ok("Escape closes the sidebar's sheet and gives the focus back to the door", shut && await focus() === "sidebar-rail-open", await focus());

  /* The tree's keys in the rail are the tree's keys. A press that activates a checkout hands the focus on to the key
   * sink, as it does with the column whole (`sidebar-aria.mjs`), so the list is focused again before each key. */
  const press = async (key, from) => {
    await page.evaluate((from) => {
      activeWorktreePath = from;
      replaceWorktreeSelection(from);
      paintWorktreeSelection();
      el("worktrees").focus();
    }, from);
    await page.keyboard.press(key);
    await page.evaluate(() => refreshWorktrees());
    await page.evaluate(() => window.__PAINTED__());
    return page.evaluate(() => ({ selected: [...selectedWorktreePaths].join(), railed: document.querySelector(".threads").classList.contains("is-railed"),
      head: document.querySelector('.proj-row[data-project-path="/z"]')?.getAttribute("aria-expanded") }));
  };
  const down = await press("ArrowDown", "/z/wt/one");
  const up = await press("ArrowUp", "/z/wt/one");
  const end = await press("End", "/z");
  ok("the tree's Up, Down and End step through the checkouts from the rail, in the order it shows them",
    down.selected === "/z/wt/two" && up.selected === "/z" && end.selected === "/z/wt/three" && down.railed, JSON.stringify({ down, up, end }));
  const left = await press("ArrowLeft", "/z/wt/one");
  const right = await press("ArrowRight", "/z/wt/one");
  ok("the tree's Left and Right shut and open the project from the rail", left.head === "false" && right.head === "true", JSON.stringify({ left, right }));
  const entered = await press("Enter", "/z/wt/two");
  ok("the tree's Enter opens a checkout from the rail and the rail stands", entered.selected === "/z/wt/two" && entered.railed, JSON.stringify(entered));
  await page.evaluate(() => { activeWorktreePath = "/z"; replaceWorktreeSelection("/z"); paintWorktreeActive(); });
  await page.evaluate(() => window.__PAINTED__());

  /* A key on a rail's control is that control's: the terminal's sink takes none of it. */
  await page.focus("#activity-scm-tab");
  await page.keyboard.press("Enter");
  await frames(page);
  const tabbed = await page.evaluate(() => ({ showing: activityShowing(), open: el("workbench").classList.contains("is-aside-open"),
    expanded: el("activity-scm-tab").getAttribute("aria-expanded"), selected: el("activity-scm-tab").getAttribute("aria-selected"),
    panel: el("activity-scm").getBoundingClientRect().width, stage: document.querySelector(".stage").getBoundingClientRect().right,
    filesLeft: document.querySelector(".files").getBoundingClientRect().left }));
  ok("Enter on a rail tab opens its panel as a sheet over the stage, and the tab says so",
    tabbed.showing === "scm" && tabbed.open && tabbed.expanded === "true" && tabbed.selected === "true" && tabbed.panel >= 200 && tabbed.filesLeft < tabbed.stage,
    JSON.stringify(tabbed));
  ok("the rail tab keeps the focus through its Enter", await focus() === "activity-scm-tab", await focus());
  await page.keyboard.press("Enter");
  await frames(page);
  ok("Enter again on the same tab closes the sheet",
    await page.evaluate(() => !el("workbench").classList.contains("is-aside-open") && el("activity-scm-tab").getAttribute("aria-expanded") === "false"));
  await page.keyboard.press("Enter");
  await frames(page);
  await page.focus("#activity-pile-tab");
  await page.keyboard.press("Enter");
  await frames(page);
  ok("another tab of the open sheet changes the panel and leaves the sheet up",
    await page.evaluate(() => activityShowing() === "pile" && el("workbench").classList.contains("is-aside-open")));
  await page.keyboard.press("Escape");
  await frames(page);
  ok("Escape closes the right column's sheet and gives the focus back to its tab",
    await page.evaluate(() => !el("workbench").classList.contains("is-aside-open")) && await focus() === "activity-pile-tab", await focus());

  /* The eye and the keyboard walk one order, where the engine has the property for it. */
  if (await page.evaluate(() => CSS.supports("reading-flow", "flex-visual"))) {
    await page.focus("#worktrees");
    await page.keyboard.press("Tab");
    const next = await page.evaluate(() => {
      const at = document.activeElement.getBoundingClientRect();
      return { what: document.activeElement.id || document.activeElement.className, below: at.top >= el("worktrees").getBoundingClientRect().top };
    });
    ok("Tab from the list goes down the rail, the way the eye goes", next.below, JSON.stringify(next));
  }
  await page.evaluate(() => { el("activity-scm-tab").blur(); setActivityItem("scm"); });
}

async function testReadable(page, ok) {
  await width(page, 900);
  await page.evaluate(() => window.__PAINTED__());
  await frames(page);
  const read = await page.evaluate(() => {
    const row = (path) => document.querySelector(`#worktrees .wt-row[data-worktree-path="${path}"]`);
    const mark = (path) => {
      const chip = row(path).querySelector(".wt-landing");
      const rect = chip.getBoundingClientRect();
      return { tone: chip.dataset.landing ?? "", width: rect.width, glyph: getComputedStyle(chip, "::before").content,
        inRow: rect.left >= row(path).getBoundingClientRect().left - 1 && rect.right <= row(path).getBoundingClientRect().right + 1,
        described: el(row(path).closest(".wt-node").getAttribute("aria-describedby") ?? "")?.textContent ?? "" };
    };
    const node = (path) => row(path).closest(".wt-node");
    const resting = document.querySelector(".proj-resting");
    const restingAfter = getComputedStyle(resting.querySelector(".proj-resting-word"), "::after");
    const head = document.querySelector(".your-turn-group-head");
    const dot = document.querySelector("#worktrees .wt-agent.is-waiting .wt-agent-dot");
    return {
      active: { current: node("/z").getAttribute("aria-current"), ground: getComputedStyle(node("/z")).backgroundColor,
        rest: getComputedStyle(node("/z/wt/one")).backgroundColor, edge: getComputedStyle(node("/z")).boxShadow },
      landed: mark("/z/wt/one"), ahead: mark("/z/wt/two"), clash: mark("/z/wt/three"),
      resting: { width: resting.getBoundingClientRect().width, count: restingAfter.content, name: resting.getAttribute("aria-label"),
        tip: resting.dataset.tip ?? "" },
      turn: { width: head.getBoundingClientRect().width, height: head.getBoundingClientRect().height, count: getComputedStyle(head, "::after").content,
        name: head.textContent, tip: head.dataset.tip ?? "", ground: getComputedStyle(head).backgroundColor,
        rows: [...document.querySelectorAll(".your-turn-row")].filter((one) => one.getBoundingClientRect().height > 0).length },
      question: dot ? { width: dot.getBoundingClientRect().width } : null,
      pile: { count: el("activity-pile-count").textContent, shown: el("activity-pile-count").getBoundingClientRect().width,
        name: el("activity-pile-tab").getAttribute("aria-label"), tip: el("activity-pile-tab").dataset.tip,
        word: el("activity-pile-tab").querySelector(".activity-pile-words").getBoundingClientRect().width },
    };
  });
  const centres = await page.evaluate(() => {
    const rail = document.querySelector(".threads").getBoundingClientRect();
    const off = (node) => { const box = node.getBoundingClientRect(); return Math.abs(box.left + box.width / 2 - (rail.left + rail.width / 2)); };
    return [".proj-avatar", ".wt-dot", ".wt-agent .agent-ico", "#sidebar-rail-open", "#sidebar-search", "#nav-inbox"]
      .map((selector) => ({ selector, off: off(document.querySelector(`#worktrees ${selector}, .threads ${selector}`)) }));
  });
  ok("every mark of the left rail stands on its centre", centres.every((one) => one.off <= 2), JSON.stringify(centres));
  ok("the checkout the window stands in reads as selected in the rail, by a ground the others lack",
    read.active.current === "location" && read.active.ground !== read.active.rest, JSON.stringify(read.active));
  ok("a landing chip folds to a mark inside its row, a different glyph for each kind, and its word stays the item's description",
    [read.landed, read.ahead, read.clash].every((one) => one.width > 0 && one.inRow && one.glyph !== "none" && one.described.length > 0)
      && new Set([read.landed.glyph, read.ahead.glyph, read.clash.glyph]).size === 3,
    JSON.stringify([read.landed, read.ahead, read.clash]));
  ok("the resting projects fold to their count, and their line still says all of it",
    read.resting.width > 0 && read.resting.count.includes("3") && read.resting.name.includes("alpha") && read.resting.tip !== "", JSON.stringify(read.resting));
  ok("the turn group folds to a lit pill with the count, its name whole and on its tip, the rows behind it",
    read.turn.width > 0 && read.turn.height > 0 && read.turn.count.includes("3") && read.turn.name.includes("3") && read.turn.name.length > 1
      && read.turn.tip === read.turn.name && read.turn.rows === 0 && read.turn.ground !== "rgba(0, 0, 0, 0)",
    JSON.stringify(read.turn));
  ok("a question stands on its agent in the rail", read.question !== null && read.question.width > 0, JSON.stringify(read.question));
  ok("the pile's tab wears the count and says it in its name and tip, its word folded",
    read.pile.count === "14" && read.pile.shown > 0 && read.pile.word <= 1 && read.pile.name.includes("14") && read.pile.tip.includes("14"),
    JSON.stringify(read.pile));

  /* A checkout's tip in the rail is what its row said on the sidebar: its name, its state, its landing. */
  const tip = await page.evaluate(async () => {
    const row = document.querySelector('#worktrees .wt-row[data-worktree-path="/z/wt/one"]');
    row.dispatchEvent(new PointerEvent("pointerover", { bubbles: true }));
    await new Promise((done) => setTimeout(done, 700));
    const said = document.querySelector(".tooltip")?.textContent ?? "";
    row.dispatchEvent(new PointerEvent("pointerout", { bubbles: true }));
    return { said, title: row.querySelector(".wt-title").textContent, chip: row.querySelector(".wt-landing").textContent,
      name: row.closest(".wt-node").getAttribute("aria-label") };
  });
  ok("a checkout's tip in the rail says its name, its state and its landing",
    tip.said.includes(tip.name) && tip.said.includes(tip.title) && tip.chip !== "" && tip.said.includes(tip.chip), JSON.stringify(tip));
}

async function testSheets(page, ok) {
  await width(page, 900);
  /* A press on the stage closes either sheet; a checkout chosen from the sheet closes it. */
  await page.click("#activity-files-tab");
  await frames(page);
  const up = await page.evaluate(() => el("workbench").classList.contains("is-aside-open"));
  await page.mouse.click(300, 400);
  await frames(page);
  ok("a press on the stage closes the right column's sheet", up && await page.evaluate(() => !el("workbench").classList.contains("is-aside-open")));

  await page.click("#sidebar-rail-open");
  await frames(page);
  const open = await page.evaluate(() => { const rect = document.querySelector(".threads").getBoundingClientRect();
    return { left: rect.left, width: rect.width, sheet: el("workbench").classList.contains("is-sidebar-open"),
      over: Number(getComputedStyle(document.querySelector(".threads")).zIndex) >= 10 }; });
  ok("the door opens the sidebar's sheet at the window's left edge, over the stage",
    open.sheet && open.left === 0 && open.width >= 240 && open.over, JSON.stringify(open));
  await page.click('#worktrees .wt-row[data-worktree-path="/z/wt/two"]');
  await frames(page);
  ok("a checkout chosen from the sheet closes it", await page.evaluate(() => !el("workbench").classList.contains("is-sidebar-open")));
  await page.click("#sidebar-rail-open");
  await frames(page);
  await page.click("#activity-pile-tab");
  await frames(page);
  ok("one sheet stands at a time", await page.evaluate(() => el("workbench").classList.contains("is-aside-open") && !el("workbench").classList.contains("is-sidebar-open")));
  await page.evaluate(() => closeRailSheet("aside"));

  /* A sheet does not outlive the rail it opened from. */
  await page.evaluate(() => openRailSheet("aside"));
  await width(page, 1500);
  const wide = await page.evaluate(() => ({ open: el("workbench").classList.contains("is-aside-open"), position: getComputedStyle(document.querySelector(".files")).position }));
  ok("a window that grows past the rail takes the sheet back into its column", !wide.open && wide.position !== "absolute", JSON.stringify(wide));
  await width(page, 900);

  /* The person's fold still wins: ⌘B and ⌘L take a rail all the way away and bring it back. */
  await page.evaluate(() => setPanelFolded("sidebar", true));
  await page.evaluate(() => setPanelFolded("aside", true));
  await foldEased(page);
  const gone = await page.evaluate(readLayout);
  await page.evaluate(() => setPanelFolded("sidebar", false));
  await page.evaluate(() => setPanelFolded("aside", false));
  await foldEased(page);
  const back = await page.evaluate(readLayout);
  ok("a person's fold takes a rail away and gives it back",
    gone.threads.width === 0 && gone.files.width === 0 && back.threads.width === 60 && back.files.width === 52, JSON.stringify({ gone, back }));
  await page.evaluate(() => { revealActivity("scm"); });
  await frames(page);
  ok("a door that reveals a panel (⌘⇧G and its kin) opens it as the sheet",
    await page.evaluate(() => el("workbench").classList.contains("is-aside-open") && activityShowing() === "scm"));
  await page.evaluate(() => closeRailSheet("aside"));

  /* Under the last fold the title bar's two toggles are the way in. */
  await width(page, 390);
  await page.click("#toggle-sidebar");
  await frames(page);
  const phone = await page.evaluate(() => { const rect = document.querySelector(".threads").getBoundingClientRect();
    return { left: rect.left, right: rect.right, shown: getComputedStyle(document.querySelector(".threads")).visibility, pressed: el("toggle-sidebar").getAttribute("aria-pressed") }; });
  ok("at 390 px the sidebar toggle opens the whole sidebar inside the window", phone.left === 0 && phone.right <= 390 && phone.shown === "visible" && phone.pressed === "false", JSON.stringify(phone));
  await page.click("#toggle-aside");
  await frames(page);
  const phoneAside = await page.evaluate(() => { const rect = document.querySelector(".files").getBoundingClientRect();
    return { left: rect.left, right: rect.right, sidebar: el("workbench").classList.contains("is-sidebar-open"), shown: getComputedStyle(document.querySelector(".files")).visibility,
      panel: el("activity-pile").getBoundingClientRect().width + el("activity-scm").getBoundingClientRect().width + el("activity-files").getBoundingClientRect().width,
      railRight: document.querySelector(".activity-tabs").getBoundingClientRect().right }; });
  ok("at 390 px the right toggle opens the right column inside the window, its tabs and its panel both",
    phoneAside.left >= 0 && phoneAside.right <= 390 && !phoneAside.sidebar && phoneAside.shown === "visible" && phoneAside.panel >= 200 && phoneAside.railRight <= 390, JSON.stringify(phoneAside));
  await page.click("#toggle-aside");
  await frames(page);
  ok("the toggle closes what it opened", await page.evaluate(() => !el("workbench").classList.contains("is-aside-open") && getComputedStyle(document.querySelector(".files")).visibility === "hidden"));
}

/* What a command opens and what lies behind it (t-84803 r2): a sheet opened from the terminal takes the keyboard and its
 * Escape, a menu in it takes the first Escape and the sheet the next, the other sheet leaves by its own road, and a
 * native page behind the stage lies down while a sheet stands. */
async function testCommandSheet(page, ok) {
  await width(page, 1200);
  await page.evaluate(() => {
    window.__KEYS__ = [];
    const prior = window.__IPC_LOG__;
    window.__IPC_LOG__ = (command, args, outcome) => {
      if (command === "term_key" || command === "term_text") window.__KEYS__.push({ command, key: args?.press?.key ?? args?.text ?? "" });
      prior?.(command, args, outcome);
    };
  });
  const keys = () => page.evaluate(() => window.__KEYS__.splice(0));
  const sheetUp = () => page.evaluate(() => el("workbench").classList.contains("is-aside-open"));

  /* The terminal hears an Escape typed into it: the recorder records. */
  await page.evaluate(() => keySink.focus());
  await page.keyboard.press("Escape");
  await settle(page);
  const heard = await keys();
  ok("an Escape typed to the terminal with no sheet up reaches the terminal (the recorder hears keys)",
    heard.some((one) => one.key === "Escape"), JSON.stringify(heard));

  await page.evaluate(() => { keySink.focus(); revealActivity("scm"); });
  await frames(page);
  const opened = await page.evaluate(() => ({ up: el("workbench").classList.contains("is-aside-open"),
    inside: document.querySelector(".files").contains(document.activeElement), focus: document.activeElement?.id ?? "" }));
  ok("a sheet a command opens over the terminal takes the keyboard", opened.up && opened.inside, JSON.stringify(opened));
  await page.keyboard.press("q");
  await page.keyboard.press("Escape");
  await settle(page);
  const shut = await page.evaluate(() => ({ up: el("workbench").classList.contains("is-aside-open"), back: document.activeElement === keySink }));
  const leaked = await keys();
  ok("Escape closes that sheet and gives the terminal its focus back, and neither the letter nor the Escape reached the terminal",
    !shut.up && shut.back && leaked.length === 0, JSON.stringify({ shut, leaked }));

  /* A menu in the sheet: the first Escape is the menu's, the second the sheet's. */
  await page.click("#activity-scm-tab");
  await frames(page);
  await page.click("#scm-overflow");
  await frames(page);
  const menuUp = await page.evaluate(() => !sidebarMenu.hidden);
  await page.keyboard.press("Escape");
  // Past the menu's own leaving (`closing`), as a person's second press is.
  await settle(page);
  const first = { menu: await page.evaluate(() => !sidebarMenu.hidden), sheet: await sheetUp() };
  await page.keyboard.press("Escape");
  await frames(page);
  const second = await sheetUp();
  ok("in the sheet the first Escape closes the menu it opened and the next closes the sheet",
    menuUp && !first.menu && first.sheet && !second, JSON.stringify({ menuUp, first, second }));

  /* The other sheet leaves by its own road: the checks' poller hears that its column went behind the rail. */
  await width(page, 900);
  await page.evaluate(() => { setActivityItem("checks"); openRailSheet("aside"); });
  const swapped = await page.evaluate(() => {
    const real = syncChecksPoller;
    let told = 0;
    window.syncChecksPoller = () => { told += 1; real(); };
    openRailSheet("sidebar");
    window.syncChecksPoller = real;
    return { told, aside: el("workbench").classList.contains("is-aside-open"), sidebar: el("workbench").classList.contains("is-sidebar-open"),
      wanted: checksPollWanted() };
  });
  ok("opening the sidebar's sheet over the right one closes the right one by its road, and the checks stop asking",
    swapped.told >= 1 && !swapped.aside && swapped.sidebar && !swapped.wanted, JSON.stringify(swapped));
  await page.evaluate(() => { closeRailSheet("sidebar"); setActivityItem("scm"); });

  /* A native page floats over the DOM: with a sheet up it is covered, and each coming and going asks the panes again. */
  const covered = await page.evaluate(() => {
    const real = syncBrowserPanes;
    const asked = [];
    window.syncBrowserPanes = () => { asked.push(browserCovered()); real(); };
    const before = browserCovered();
    openRailSheet("aside");
    const during = browserCovered();
    const opening = asked.splice(0);
    closeRailSheet("aside");
    const after = browserCovered();
    const closing = asked.splice(0);
    openRailSheet("sidebar");
    const sidebar = browserCovered();
    closeRailSheet("sidebar");
    asked.splice(0);
    window.syncBrowserPanes = real;
    return { before, during, after, sidebar, opening, closing };
  });
  ok("a sheet up covers the native page, and its coming and its going each sync the panes once",
    covered.during && covered.sidebar && covered.after === covered.before && covered.opening.join() === "true" && covered.closing.join() === String(covered.before),
    JSON.stringify(covered));
}

/* The automation screen takes the right column's track away, rail and all: the floating trigger is the one door to
 * the floating terminal there (t-84803 r2). The task screen did too until it became a centre tab (2026-11-06) — a tab
 * keeps the right column like every other destination. */
async function testWideRouteDoor(page, ok) {
  await width(page, 1280);
  const door = () => page.evaluate(() => {
    const trigger = el("float-toggle");
    return { route: el("workbench").classList.contains("is-route-wide"), shown: !trigger.hidden && getComputedStyle(trigger).display !== "none",
      right: trigger.getBoundingClientRect().right };
  });
  const plain = await door();
  await page.evaluate(() => { el("auto-view").hidden = false; paintRouteWidth(); });
  await frames(page);
  const auto = await door();
  await page.evaluate(() => { el("auto-view").hidden = true; paintRouteWidth(); });
  await frames(page);
  ok("at 1280 px the automation screen shows the floating trigger inside the window, and the rail's own view keeps it folded",
    !plain.route && !plain.shown && auto.route && auto.shown && auto.right <= 1280, JSON.stringify({ plain, auto }));
}

/* The heads of the grouped views (상태별, PR별): inside the rail, and a tip that says them (t-84803 r2). */
async function testGroupHeads(page, ok) {
  await width(page, 900);
  for (const mode of ["state", "pr"]) {
    await page.evaluate((mode) => setSidebarView({ groupBy: mode }), mode);
    await page.evaluate(() => window.__PAINTED__());
    await frames(page);
    const heads = await page.evaluate(async () => {
      const rail = document.querySelector(".threads").getBoundingClientRect();
      const out = [];
      for (const head of document.querySelectorAll(".threads.is-railed .proj-row.is-state-group")) {
        if (head.getBoundingClientRect().height === 0) continue;
        const spill = [...head.querySelectorAll("*")].filter((one) => {
          const box = one.getBoundingClientRect();
          // No pixel of slack: the chevron that once stood here ended at 61 px of a 60 px rail.
          return box.width > 1 && getComputedStyle(one).display !== "none" && (box.left < rail.left || box.right > rail.right);
        }).map((one) => one.className.baseVal ?? one.className);
        head.dispatchEvent(new PointerEvent("pointerover", { bubbles: true }));
        await new Promise((done) => setTimeout(done, 700));
        const said = document.querySelector(".tooltip:not([hidden])")?.textContent ?? "";
        head.dispatchEvent(new PointerEvent("pointerout", { bubbles: true }));
        out.push({ name: head.querySelector(".proj-name").textContent, count: head.querySelector(".state-group-count").textContent, said, spill });
      }
      return out;
    });
    ok(`the heads of the ${mode} grouping stand inside the rail and their tip says the name and the count`,
      heads.length > 0 && heads.every((head) => head.spill.length === 0 && head.said.includes(head.name) && head.said.includes(head.count)),
      JSON.stringify(heads));
  }
  await page.evaluate(() => setSidebarView({ groupBy: "repo" }));
  await page.evaluate(() => window.__PAINTED__());
}

/* Turning a fold off is one token. Nothing else moves: the older rules come back. */
async function testOffSwitch(page, ok) {
  const tokens = await page.evaluate(() => {
    const read = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
    return { panel: read("--rail-fold-panel"), sidebar: read("--rail-fold-sidebar"), away: read("--rail-fold-away") };
  });
  ok("the rails read their widths from named tokens — 1360, 1000 and 480 px", tokens.panel === "1360px" && tokens.sidebar === "1000px" && tokens.away === "480px", JSON.stringify(tokens));
  const turn = (name, value) => page.evaluate(([name, value]) => {
    if (value === null) document.documentElement.style.removeProperty(name);
    else document.documentElement.style.setProperty(name, value);
    paintWorkbenchRails();
  }, [name, value]);
  await width(page, 900);
  await turn("--rail-fold-sidebar", "0px");
  await frames(page);
  const off = await page.evaluate(readLayout);
  const titled = await page.evaluate(() => document.querySelector("#worktrees .wt-title").getBoundingClientRect().width);
  ok("--rail-fold-sidebar at 0px gives a 900 px window the 240 px sidebar back, and the right rail stays",
    off.threads.width === 240 && off.files.width === 52 && titled > 8, JSON.stringify(off));
  await width(page, 600);
  const stacked = await page.evaluate(readLayout);
  ok("with the sidebar's rail off a 600 px window stacks its panels as it did",
    stacked.stage.width === 600 && stacked.threads.width === 600 && stacked.files.width === 600, JSON.stringify(stacked));
  await turn("--rail-fold-sidebar", null);
  await width(page, 900);
  await turn("--rail-fold-panel", "0px");
  await frames(page);
  const panelOff = await page.evaluate(readLayout);
  ok("--rail-fold-panel at 0px gives a 900 px window its right column back, and the sidebar's rail stays",
    panelOff.files.width >= 200 && panelOff.threads.width === 60, JSON.stringify(panelOff));
  await turn("--rail-fold-panel", null);
  await frames(page);
  const on = await page.evaluate(readLayout);
  ok("the tokens back on fold them again", on.threads.width === 60 && on.files.width === 52, JSON.stringify(on));
}

/* The sidebar standing as a team: the same rail, a coordinator's face and each worker's dot. */
async function testTeam(browser, origin, ok, faults) {
  const { page } = await openWindowTestPage(browser, origin, { faults });
  try {
    await page.setViewportSize({ width: 1440, height: 800 });
    await standTeamScene(page, { layout: "team" });
    await standRails(page);
    await width(page, 900);
    await page.evaluate(() => window.__PAINTED__());
    const at = await page.evaluate(readLayout);
    const items = await page.evaluate(readRailItems);
    const rows = items.filter((item) => item.what.includes("team-row"));
    ok("the team list folds to the same rail: its rows inside it, each with a name and a tip",
      at.threads.width === 60 && at.outside.length === 0 && rows.length > 1 && rows.every((row) => row.name !== "" && row.tip !== ""),
      JSON.stringify({ rows, outside: at.outside }));
    const marks = await page.evaluate(() => [...document.querySelectorAll(".threads.is-railed .team-row")]
      .filter((row) => row.getBoundingClientRect().height > 0)
      .map((row) => ({ what: row.className, mark: [...row.querySelectorAll(".team-face, .team-dot")].some((one) => one.getBoundingClientRect().width > 0) })));
    ok("each row of the team rail shows its face or its state", marks.length > 1 && marks.every((row) => row.mark), JSON.stringify(marks));
  } finally {
    await page.close();
  }
}

/* The window booting as a person's narrow window boots: the rails on from the first script, not stood after the boot
 * (t-84803 r2). At 1280 px the first paint of the rails once threw out of the source-control script before half of it
 * had run, and the window stood with no sidebar row and no tab — every other scene here boots with the rails off. */
export async function testRailsAtBoot({ browser, origin, ok }) {
  for (const [px, rail] of [[1280, "is-aside-rail"], [1000, "is-sidebar-rail"]]) {
    const faults = [];
    const { page } = await openWindowTestPage(browser, origin, { faults, rails: true, before: recordBootPhases,
      context: { viewport: { width: px, height: 800 } } });
    try {
      const settled = await waitBootSettled(page).then(() => true, (error) => String(error).slice(0, 200));
      const stood = await page.evaluate((rail) => ({
        rail: el("workbench").classList.contains(rail),
        rows: document.querySelectorAll("#worktrees .proj-row, #worktrees .wt-row").length,
        tabs: document.querySelectorAll("#tabstrip .tab").length,
      }), rail);
      ok(`a ${px} px window boots with its rails on: the rail stands, the sidebar has rows, the title bar has tabs, no page error`,
        settled === true && stood.rail && stood.rows > 0 && stood.tabs > 0 && faults.length === 0,
        JSON.stringify({ settled, ...stood, faults: faults.map((fault) => fault.split("\n")[0]) }));
    } finally {
      await page.close();
    }
  }
}

export async function testPanelRails({ browser, origin, ok, faults }) {
  const { page } = await openWindowTestPage(browser, origin, { faults });
  try {
    await page.setViewportSize({ width: 1440, height: 800 });
    await standRailFixture(page);
    await standRails(page);
    await testFolds(page, ok);
    await testDragStyle(page, ok);
    await testEdgeSnaps(page, ok, "rails on");
    await testTerminal(page, ok);
    await testNamed(page, ok);
    await testKeyboard(page, ok);
    await testReadable(page, ok);
    await testSheets(page, ok);
    await testCommandSheet(page, ok);
    await testWideRouteDoor(page, ok);
    await testGroupHeads(page, ok);
    await testOffSwitch(page, ok);
  } finally {
    await page.close();
  }
  await testTeam(browser, origin, ok, faults);
  {
    const { page: plain } = await openWindowTestPage(browser, origin, { faults });
    try {
      await testEdgeSnaps(plain, ok, "rails off");
    } finally {
      await plain.close();
    }
  }
  ok("the rails raise no browser errors", faults.length === 0, faults.join("\n"));
}
