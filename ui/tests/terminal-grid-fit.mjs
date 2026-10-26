import { openWindowTestPage } from "./window-boot.mjs";
import { installHarnessWaits } from "./harness-waits.mjs";

/* A restored background terminal gets no layout until it is revealed. The
 * backend records the requested pty size independently of termGrids, so a
 * cached request cannot serve as its own receipt. No live pty or foreground
 * window participates in this test. */
export async function testTerminalGridFit(browser, origin, ok) {
  await terminalGridFitScene(browser, origin, ok, false);
  await terminalGridFitScene(browser, origin,
    (label, ...args) => ok(`without checkVisibility: ${label}`, ...args), true);
}

async function terminalGridFitScene(browser, origin, ok, withoutCheckVisibility) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  const within = (work, label) => {
    let timer;
    return Promise.race([work, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error(`Timed out waiting for ${label}`)), 15000);
    })]).finally(() => clearTimeout(timer));
  };
  try {
    await installHarnessWaits(page);
    const seen = await within(page.evaluate(async (withoutCheckVisibility) => {
      // Safari before 17.4 has no Element.checkVisibility (macOS 14.0–14.3).
      if (withoutCheckVisibility) {
        Object.defineProperty(Element.prototype, "checkVisibility", {
          configurable: true, value: undefined,
        });
      }

      const worktree = "/tmp/terminal-grid-fit";
      const sizes = new Map();
      const resizes = [];
      let nextTerm = 9700;
      const frame = (term) => {
        const { rows, cols } = sizes.get(term);
        return {
          size: [rows, cols], full: true,
          rows: [{ index: 0, cells: Array.from({ length: cols }, () => ({ ch: "M" })) }],
          cursor: [0, 0], cursor_visible: false, scrolled_lines: 0,
          view_offset: 0, scrollback_len: 0, alt_screen: false,
          mouse_tracking: "off", mouse_sgr: false, folds: [],
        };
      };
      const spawn = (args) => {
        const term = ++nextTerm;
        sizes.set(term, { rows: args.rows, cols: args.cols });
        return term;
      };
      const room = (term) => {
        const view = termViews.get(term);
        const style = getComputedStyle(view.host);
        // Read a real glyph run, independently of the view's cached cell.
        const run = view.pre.querySelector(".term-row");
        const range = document.createRange();
        range.selectNodeContents(run);
        const pitch = range.getBoundingClientRect().width / run.textContent.length;
        return {
          cols: Math.min(400, Math.max(20, Math.floor((view.host.clientWidth
            - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight)) / pitch))),
          rows: Math.min(200, Math.max(5, Math.floor((view.host.clientHeight
            - parseFloat(style.paddingTop) - parseFloat(style.paddingBottom))
            / parseFloat(getComputedStyle(view.pre.querySelector(".term-row")).height)))),
        };
      };
      const sample = (term) => {
        const view = termViews.get(term);
        if (!view?.pre.querySelector(".term-row")?.textContent.length) return { term, matches: false, hidden: view?.host.hidden, chat: paneChatOn(term), html: view?.pre.innerHTML.slice(0,200), sent: sizes.get(term), box: view?.host.getBoundingClientRect().toJSON(), watched: watchedTerms.has(term) };
        const actual = room(term);
        const sent = sizes.get(term);
        const drawn = view.pre.querySelector(".term-row")?.textContent.length;
        return { term, actual, sent, drawn,
          matches: sent.cols === actual.cols && sent.rows === actual.rows && drawn === actual.cols };
      };
      const settle = async (terms, label) => {
        try { await window.__UNTIL__(() => terms.every((term) => sample(term).matches), label); }
        catch (error) { throw new Error(`${label}: ${JSON.stringify(terms.map(sample))}`, { cause: error }); }
        return terms.map(sample);
      };
      window.__ANSWER__.resume_session = (args) => ({ term: spawn(args), standing: false });
      window.__ANSWER__.open_term_tab = spawn;
      window.__ANSWER__.term_snapshot = ({ term }) => sizes.has(term) ? frame(term) : null;
      window.__ANSWER__.term_resize = (args) => {
        sizes.set(args.term, { rows: args.rows, cols: args.cols });
        resizes.push({ ...args });
        // The parser's next full frame after accepting a resize.
        void window.__TERM_FEED__(args.term, frame(args.term));
        return null;
      };
      try {
        const front = await openTermTab({ placement: "tab" });
        await window.__UNTIL__(() => termViews.get(front)?.pre.querySelector(".term-row"), "front frame");
        await restoreWorktreeLayouts(worktree, [{
          id: 1, focused: true, awake: true,
          root: { type: "split", direction: "vertical", first: { type: "leaf" }, second: { type: "leaf" } },
          agents: {
            0: { agent: "zo", key: "session_id", id: "session-grid-fit-a" },
            1: { agent: "zo", key: "session_id", id: "session-grid-fit-b" },
          },
        }], { behind: true });
        await storedWakesSettled(worktree);
        const tab = tabs.find((one) => one.worktree === worktree);
        const terms = paneLeaves(tab.layout);
        const hiddenResizes = resizes.filter((one) => terms.includes(one.term)).length;
        const hidden = termViews.get(terms[0]);
        // A CSS-sized box still has computed dimensions under display:none.
        // It has no laid-out room and must never be treated as a measurement.
        hidden.host.style.width = "640px";
        hidden.host.style.height = "480px";
        const hiddenGrid = hidden.gridSize();
        const originalGetComputedStyle = window.getComputedStyle;
        let probes = 0;
        window.getComputedStyle = (node, ...args) => {
          if (node.textContent === "W".repeat(20)) probes += 1;
          return originalGetComputedStyle(node, ...args);
        };
        try { hidden.measure(); }
        finally { window.getComputedStyle = originalGetComputedStyle; }
        // A leaf's own flag can be visible under a hidden ancestor.
        hidden.host.hidden = false;
        resizeTermTab(terms[0]);
        hidden.host.hidden = true;
        const hiddenResizeAfterFit = resizes.filter((one) => terms.includes(one.term)).length;
        hidden.host.style.width = "";
        hidden.host.style.height = "";
        activeWorktreePath = worktree;
        await restoreWorktreeLayouts(worktree, null, { road: "visit" });
        setActiveTab(tab.id);
        const revealed = await settle(terms, "restored tab fits its visible room");
        await splitActivePane("horizontal");
        const split = await settle(paneLeaves(tab.layout), "split panes fit their room");
        // A leaf can change size while its tab's outer box remains unchanged.
        // The pane itself must be watched, not only the whole tab container.
        const local = termViews.get(terms[0]);
        local.host.style.flex = "none";
        local.host.style.width = "70%";
        let localFits = true;
        try { await settle([terms[0]], "one pane fits after its own width changes"); }
        catch { localFits = false; }
        const localSize = sample(terms[0]);
        local.host.style.width = "";
        local.host.style.flex = "";
        // Force the normal window road before testing a zero-height box.
        window.dispatchEvent(new Event("resize"));
        await settle(paneLeaves(tab.layout), "pane width restored");
        local.host.style.padding = "0px";
        local.host.style.height = "0px";
        local.host.style.minHeight = "0px";
        local.host.style.flex = "none";
        const zeroHeight = local.gridSize();
        local.host.style.padding = "";
        local.host.style.height = "";
        local.host.style.minHeight = "";
        local.host.style.flex = "";
        return { terms, hiddenResizes, hiddenResizeAfterFit, hiddenProbes: probes, hiddenBlind: hiddenGrid.blind === true,
          zeroHeightBlind: zeroHeight.blind === true, revealed, split, localFits, localSize };
      } finally {
        window.__GRID_FIT_SETTLE__ = settle;
      }
    }, withoutCheckVisibility), "restoring and fitting terminals");
    console.log(`MEASURE terminal grid fit ${JSON.stringify(seen)}`);
    ok("a restored hidden tab sends no resize and a CSS-sized hidden box remains unmeasured",
      seen.hiddenResizes === 0 && seen.hiddenResizeAfterFit === 0 && seen.hiddenProbes === 0 && seen.hiddenBlind, JSON.stringify(seen));
    ok("revealing a restored split gives every grid the visible pane's columns and the pty's size",
      seen.revealed.length === 2 && seen.revealed.every((one) => one.matches), JSON.stringify(seen.revealed));
    ok("splitting a restored pane fits the drawn grid and the pty to each new room",
      seen.split.length === 3 && seen.split.every((one) => one.matches), JSON.stringify(seen.split));
    ok("a pane's own size change fits its grid even when its tab's outer size stays unchanged",
      seen.localFits && seen.localSize.actual.cols < seen.split[0].actual.cols, JSON.stringify(seen.localSize));
    ok("a zero-height terminal never supplies a measured grid", seen.zeroHeightBlind, JSON.stringify(seen));
    await page.setViewportSize({ width: 1040, height: 720 });
    const resized = await within(page.evaluate((terms) => window.__GRID_FIT_SETTLE__(terms, "window resize fits the restored terminals"), seen.terms), "resized terminal frames");
    console.log(`MEASURE resized terminal grid fit ${JSON.stringify(resized)}`);
    if (process.env.TERMINAL_GRID_FIT_CAPTURE) await page.screenshot({ path: process.env.TERMINAL_GRID_FIT_CAPTURE });
    ok("resizing the window fits the restored terminals again", resized.every((one) => one.matches), JSON.stringify(resized));
    ok("terminal grid fitting raised no renderer errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}
