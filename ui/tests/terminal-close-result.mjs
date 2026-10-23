import { openWindowTestPage } from "./window-boot.mjs";

const frame = (page) => page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));

async function scene(browser, origin, split) {
  const opened = await openWindowTestPage(browser, origin);
  await opened.page.evaluate(async (split) => {
    confirmCloseRunning = false;
    confirmClosePinnedTab = false;
    const first = await openTermTab({ placement: "tab", door: "terminal" });
    const tab = tabOfTerm(first);
    if (split) await splitActivePane("row");
    window.__CLOSE_SCENE__ = { tab: tab.id, first, going: activePaneOf(tab), calls: [] };
    window.__CLOSE_VIEW__ = viewOfTerm(window.__CLOSE_SCENE__.going);
  }, split);
  return opened;
}

const inspect = (page) => page.evaluate(() => {
  const scene = window.__CLOSE_SCENE__;
  const tab = tabs.find((one) => one.id === scene.tab);
  return {
    exists: !!tab,
    leaves: tab ? paneLeaves(tab.layout) : [],
    view: !!viewOfTerm(scene.going),
    sameView: viewOfTerm(scene.going) === window.__CLOSE_VIEW__,
    firstView: !!viewOfTerm(scene.first),
    calls: [...scene.calls],
    going: scene.going,
    first: scene.first,
    notice: document.querySelector(".toast[data-kind=halt]")?.textContent ?? "",
  };
});

export async function testTerminalCloseResult(browser, origin, ok) {
  for (const road of ["tab", "pane"]) {
    for (const kind of ["journal_unavailable", "journal_failed", "signal_failed", "unconfirmed", "replaced", "ipc"]) {
      const { page, faults } = await scene(browser, origin, road === "pane");
      try {
        await page.evaluate(({ road, kind }) => {
          window.__ANSWER__.close_term = ({ term }) => {
            window.__CLOSE_SCENE__.calls.push(term);
            if (kind === "ipc") throw new Error("synthetic IPC refusal");
            throw { kind };
          };
          const held = window.__CLOSE_SCENE__;
          if (road === "tab") closeTab(held.tab);
          else void closePaneLeaf(tabs.find((one) => one.id === held.tab), held.going);
        }, { road, kind });
        await frame(page);
        const failed = await inspect(page);
        ok(`failed ${road} close keeps its live screen (${kind})`,
          failed.exists && failed.leaves.includes(failed.going) && failed.sameView,
          JSON.stringify(failed));
        ok(`failed ${road} close says why and can be retried (${kind})`,
          failed.notice.length > 0 && !failed.notice.includes("[object Object]"), JSON.stringify(failed));
        if (failed.exists && failed.leaves.includes(failed.going)) {
          await page.evaluate((road) => {
            window.__ANSWER__.close_term = ({ term }) => (window.__CLOSE_SCENE__.calls.push(term), null);
            const held = window.__CLOSE_SCENE__;
            if (road === "tab") closeTab(held.tab);
            else void closePaneLeaf(tabs.find((one) => one.id === held.tab), held.going);
          }, road);
          await frame(page);
          const retried = await inspect(page);
          ok(`confirmed retry removes only the closed ${road}`,
            !retried.view && !retried.leaves.includes(retried.going) &&
              (road === "tab" ? !retried.exists : retried.firstView), JSON.stringify(retried));
        }
        ok(`close scene has no unhandled script error (${road}, ${kind})`, faults.length === 0, faults.join("\n"));
      } finally { await page.close(); }
    }
  }

  const { page, faults } = await scene(browser, origin, true);
  try {
    await page.evaluate(() => {
      window.__ANSWER__.close_term = ({ term }) => {
        window.__CLOSE_SCENE__.calls.push(term);
        if (term === window.__CLOSE_SCENE__.going) throw { kind: "signal_failed" };
        return null;
      };
      closeTab(window.__CLOSE_SCENE__.tab);
    });
    await frame(page);
    const partial = await inspect(page);
    ok("a partly closed tab keeps only its failed pane and its screen",
      partial.exists && partial.sameView && !partial.firstView &&
        partial.leaves.length === 1 && partial.leaves[0] === partial.going,
      JSON.stringify(partial));
    ok("partial close has no unhandled script error", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }

  const pending = await scene(browser, origin, false);
  try {
    await pending.page.evaluate(() => {
      window.__ANSWER__.close_term = ({ term }) => {
        window.__CLOSE_SCENE__.calls.push(term);
        return new Promise((resolve) => { window.__CLOSE_REPLY__ = resolve; });
      };
      closeTab(window.__CLOSE_SCENE__.tab);
      closeTab(window.__CLOSE_SCENE__.tab);
    });
    await frame(pending.page);
    const before = await inspect(pending.page);
    ok("an unanswered close keeps the screen and coalesces repeat clicks",
      before.exists && before.sameView && before.calls.length === 1, JSON.stringify(before));
    await pending.page.evaluate(() => window.__CLOSE_REPLY__(null));
    await frame(pending.page);
    const after = await inspect(pending.page);
    ok("the screen leaves when its close receipt arrives", !after.exists && !after.view, JSON.stringify(after));
  } finally { await pending.page.close(); }
}
