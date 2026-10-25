import { readFile } from "node:fs/promises";
import { openWindowTestPage } from "./window-boot.mjs";

// A toast still up after this long is standing, not passing through.
const TOAST_WAIT_MS = 10000;
const POLL_MS = 100;
// The poll's own slack on top of the toast's lifetime before a leave is late.
const LEAVE_ALLOWANCE_MS = 1000;
const conflict = { id: "pr:mergeable", root: "/wt/topic", checkout: "topic", number: 12, kind: "mergeable", opened_at: 1, resolved_at: null };

// The PR condition toasts and their status bar mark, in a window that was laid
// out for real (t-45801). Every number is printed as a MEASURE line before the
// checks that use it, so a failing run still says what it measured.
export async function testScmNoticeWindow(browser, origin, ok) {
  const source = await readFile(new URL("../shell-status.js", import.meta.url), "utf8");
  const toastMs = Number(source.match(/const TOAST_MS = (\d+);/)?.[1]);
  const faults = [];
  const pages = [];
  const openPage = async () => {
    const { page } = await openWindowTestPage(browser, origin, { faults });
    pages.push(page);
    return page;
  };
  const deliver = (page, rows) => page.evaluate((payload) => {
    activeWorktreePath = "/wt/topic";
    for (const listener of window.__LISTENERS__["scm:notices"] ?? []) listener({ payload });
  }, rows);
  const toastCount = (page) => page.evaluate(() =>
    [...document.querySelectorAll(".toast")].filter((note) => note.textContent.includes("PR #")).length);
  const markState = (page) => page.evaluate(() => {
    const button = document.getElementById("sb-scm-notices");
    return {
      exists: button !== null,
      hidden: button?.hidden ?? true,
      tip: button?.dataset.tip ?? "",
      label: document.getElementById("sb-scm-notices-label")?.textContent ?? "",
    };
  });
  const ledger = (page) => page.evaluate(() => window.__SCM_NOTICE_MARKS__ ?? []);
  const leftAlone = async (page) => {
    const started = Date.now();
    while (Date.now() - started < TOAST_WAIT_MS) {
      if (await toastCount(page) === 0) return Date.now() - started;
      await page.waitForTimeout(POLL_MS);
    }
    return null;
  };

  try {
    const first = await openPage();
    await deliver(first, [conflict]);
    const said = await toastCount(first);
    const left = await leftAlone(first);
    console.log(`MEASURE toast_lifetime=${left === null ? `still up after ${TOAST_WAIT_MS} ms` : `${left} ms (TOAST_MS ${toastMs})`}`);
    ok("a condition is said as one toast", said === 1, `said=${said}`);
    ok("the toast leaves on its own within TOAST_MS", left !== null && left <= toastMs + LEAVE_ALLOWANCE_MS, `left=${left}`);
    const marks = await ledger(first);
    ok("showing the toast records the mark in the ledger",
      marks.some((mark) => mark.cmd === "shown" && mark.id === conflict.id && mark.openedAt === conflict.opened_at), JSON.stringify(marks));
    const standing = await markState(first);
    ok("the standing condition is a status bar mark naming its PR and checkout",
      standing.exists && !standing.hidden && standing.tip.includes("PR #12") && standing.tip.includes("topic"), JSON.stringify(standing));
    ok("the mark's words never carry the checkout's full path", standing.exists && !standing.tip.includes("/wt/"), standing.tip);
    if (standing.exists && !standing.hidden) await first.click("#sb-scm-notices");
    const opened = await first.evaluate(() => document.querySelector(".files")?.dataset.activity ?? "");
    ok("one click on the mark opens the Checks panel", opened === "checks", `activity=${opened}`);

    await first.evaluate(() => {
      window.__CHOSEN_SCM__ = [];
      selectWorktree = (root) => window.__CHOSEN_SCM__.push(root);
    });
    const ci = { ...conflict, id: "pr:ci", root: "/wt/other", checkout: "other", number: 4, kind: "ci" };
    await deliver(first, [conflict, ci]);
    await first.click("#sb-scm-notices");
    const choices = first.locator(".sidebar-menu-item");
    const choiceCount = await choices.count();
    console.log(`MEASURE selectable_conditions=${choiceCount} conditions`);
    ok("the mark offers each standing PR condition", choiceCount === 2, `choices=${choiceCount}`);
    await first.getByRole("menuitem", { name: "PR #4 CI 실패 · other", exact: true }).click();
    const chosen = await first.evaluate(() => window.__CHOSEN_SCM__);
    ok("the second condition opens its own checkout", chosen.length === 1 && chosen[0] === ci.root, JSON.stringify(chosen));

    const second = await openPage();
    await deliver(second, [{ ...conflict, shown_at: 7 }]);
    const again = await toastCount(second);
    console.log(`MEASURE restart_toasts=${again} (the first window said ${said})`);
    ok("a restarted window does not say the condition again", again === 0, `again=${again}`);
    ok("a restarted window still shows the standing mark", !(await markState(second)).hidden);

    const third = await openPage();
    await deliver(third, [conflict]);
    const closable = await third.locator(".toast .toast-action").count();
    ok("the toast offers a close button", closable === 1, `closable=${closable}`);
    if (closable === 1) await third.click(".toast .toast-action");
    ok("closing the toast writes the close to the ledger",
      (await ledger(third)).some((mark) => mark.cmd === "dismissed" && mark.openedAt === conflict.opened_at), JSON.stringify(await ledger(third)));
    ok("closing the toast keeps the condition on the status bar", !(await markState(third)).hidden);

    const fourth = await openPage();
    await deliver(fourth, [{ ...conflict, dismissed_at: 8 }]);
    ok("a condition closed in the ledger stays silent after a restart", await toastCount(fourth) === 0);
    ok("SCM notices raise no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    for (const page of pages) await page.close();
  }
}
