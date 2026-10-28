import { openWindowTestPage } from "./window-boot.mjs";

/* 「쌓인 일」 — the right column's first tab (IDE design pass 2, B: item 12 of the
 * passed draft p-6893560125c34552): what is piling up in the project on screen,
 * before the source-control list.
 *
 *  - 발행·게이트: the release lane the status bar already reads (`releaseLane`).
 *  - 미반영: the project's checkouts git says are not in main yet, each with
 *    its commit count and how far behind main it runs, and the sum of them
 *    on the tab itself.
 *  - 저장소: the branch, its upstream and the changed files, and the door to
 *    source control.
 *
 * Nothing is built while the tab is not up: its panel is empty until it is
 * opened and emptied when another tab takes the column.
 *
 *   WINDOW_SUITES=stacked-work node ui/tests/window.mjs */

/* Two unlanded checkouts (six commits, and two that run twenty behind main),
 * a landed one, and a release lane at its gate. A page function. */
export async function standPile() {
  const root = projects[0].path;
  const base = projects[0];
  const landing = (ahead, extra = {}) => ({ state: "unlanded", ahead, dirty: false, compare_ref: "origin/main", ...extra });
  window.__ANSWER__.project_catalog = () => [{
    ...base,
    worktrees: [
      ...base.worktrees,
      { path: `${root}-stack`, branch: "zo/v1199-stack", is_main: false, active: false, is_folder: false,
        ownership: "zerocode-managed", external_hidden: false, landing: landing(6) },
      { path: `${root}-weekly`, branch: "zo/v1199-weekly-review", is_main: false, active: false, is_folder: false,
        ownership: "zerocode-managed", external_hidden: false, landing: landing(2, { far_behind: true, behind: 20 }) },
      { path: `${root}-done`, branch: "zo/landed", is_main: false, active: false, is_folder: false,
        ownership: "zerocode-managed", external_hidden: false, landing: { state: "landed", ahead: 0, dirty: false, compare_ref: "origin/main" } },
    ],
  }];
  // The repository's last commits, as `git_history` answers them (the source-control list's rows).
  const commit = (id, subject, references = []) => ({
    item: { id, parent_ids: [], subject, author: "dev", display_id: id.slice(0, 7), timestamp: 1754700000, references },
    kind: "node", width: 22, height: 24, paths: [], circles: [{ cx: 11, cy: 12, r: 3.5, fill: "git-graph-ref" }],
  });
  window.__ANSWER__.git_history = (args) => {
    window.__PILE_HISTORY_ASKED__ = (window.__PILE_HISTORY_ASKED__ ?? 0) + 1;
    window.__PILE_HISTORY_LIMIT__ = args?.limit;
    return { rows: [
      commit("aaaaaaa1", "release: v1.1.98", [{ id: "refs/heads/main", name: "main", revision: "aaaaaaa1", category: "branches", color: "git-graph-ref" }]),
      commit("bbbbbbb2", "green (l80a g3b3): every test that moved"),
      commit("ccccccc3", "fix: t-77569 — the naming writes its answer"),
    ], has_more: true, current: null, remote: null };
  };
  await refreshWorktrees();
  absorbReleaseStatus({ status: { sha: "ca1ffcf0d1", version: "1.1.98", phase: "gate", started_at: new Date(Date.now() - 60_000).toISOString(),
    updated_at: new Date().toISOString(), phases: [] } });
}

export async function testStackedWork(browser, origin, ok) {
  const { page } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(standPile);
    const seen = await page.evaluate(async () => {
      const seen = {};
      const tab = document.getElementById("activity-pile-tab");
      const panel = document.getElementById("activity-pile");
      seen.tabFirst = tab?.parentElement?.firstElementChild === tab;
      seen.count = document.getElementById("activity-pile-count")?.textContent ?? "";
      // The draft's word tab: 「쌓인 일 8」, not an icon with a badge.
      seen.tabWords = tab?.textContent.replace(/\s+/g, " ").trim() ?? "";
      seen.emptyWhileClosed = panel ? panel.hidden && panel.childElementCount === 0 : false;
      tab?.click();
      await window.__PAINTED__();
      seen.open = panel && !panel.hidden;
      seen.selected = tab?.getAttribute("aria-selected");
      seen.release = panel?.querySelector(".pile-release")?.textContent.replace(/\s+/g, " ").trim() ?? "";
      seen.total = panel?.querySelector(".pile-total")?.textContent.replace(/\s+/g, " ").trim() ?? "";
      seen.branches = [...(panel?.querySelectorAll(".pile-branch") ?? [])].map((row) => row.textContent.replace(/\s+/g, " ").trim());
      seen.repoDoor = Boolean(panel?.querySelector(".pile-scm"));
      // The draft's half that was missing: the time the facts were read, the release group's
      // count and its gate line (a dash where the window holds no gate job), a bar per branch
      // as long as its commits, the checkouts no agent window holds and the door to clean them
      // up, the repository's automatic checks and its last commits.
      seen.when = panel?.querySelector(".pile-when")?.textContent ?? "";
      seen.releaseCount = panel?.querySelector(".pile-release")?.closest(".pile-section")?.querySelector(".pile-section-count")?.textContent ?? "";
      seen.gate = panel?.querySelector(".pile-gate")?.textContent.replace(/\s+/g, " ").trim() ?? "";
      seen.bars = [...(panel?.querySelectorAll(".pile-bar > i") ?? [])].map((bar) => Math.round(bar.getBoundingClientRect().width));
      seen.note = panel?.querySelector(".pile-note")?.textContent.replace(/\s+/g, " ").trim() ?? "";
      const cleanups = [];
      const realCleanup = openCleanup;
      openCleanup = () => cleanups.push(true);
      panel?.querySelector(".pile-cleanup")?.click();
      openCleanup = realCleanup;
      seen.cleanup = cleanups.length;
      seen.checks = panel?.querySelector(".pile-checks")?.textContent.replace(/\s+/g, " ").trim() ?? "";
      await new Promise((done) => setTimeout(done, 60));
      seen.commitsHead = panel?.querySelector(".pile-commits")?.closest(".pile-section")?.querySelector(".pile-section-head")?.textContent ?? "";
      seen.commits = [...(panel?.querySelectorAll(".pile-commits .history-row") ?? [])].map((row) => row.querySelector(".history-subject")?.textContent ?? "");
      seen.more = Boolean(panel?.querySelector(".pile-more"));
      seen.historyAsked = window.__PILE_HISTORY_ASKED__ ?? 0;
      panel?.querySelector(".pile-scm")?.click();
      await window.__PAINTED__();
      seen.scmShown = !document.getElementById("activity-scm").hidden;
      seen.emptiedOnLeave = panel.hidden && panel.childElementCount === 0;
      return seen;
    });
    ok(
      "the right column's first tab counts what has not landed, and its panel is built only while it is up",
      seen.tabFirst && seen.count === "8" && seen.tabWords === "쌓인 일 8" && seen.emptyWhileClosed && seen.open && seen.selected === "true" && seen.emptiedOnLeave,
      JSON.stringify(seen),
    );
    ok(
      "쌓인 일 says when it was read, counts the release group with its gate line, draws a bar per branch, offers to clean up the checkouts no agent holds, and says the checks and the last commits",
      /\d{1,2}:\d{2}/.test(seen.when) && seen.releaseCount === "1" && seen.gate.includes("—") &&
        seen.bars.length === 2 && seen.bars[0] > seen.bars[1] * 2 && seen.bars[1] > 0 &&
        seen.note.includes("에이전트 창 없음") && seen.note.includes("2") && seen.cleanup === 1 &&
        seen.checks.includes("자동 확인") && seen.checks.includes("—") &&
        seen.commitsHead.includes("3+") && seen.commits.length === 3 && seen.commits[0] === "release: v1.1.98" &&
        seen.more && seen.historyAsked === 1,
      JSON.stringify(seen),
    );
    ok(
      "쌓인 일 says the release lane, the unlanded commits and branches with how far behind main, and opens source control",
      seen.release.includes("v1.1.98") && seen.total.includes("8") &&
        seen.branches.length === 2 && seen.branches[0].includes("zo/v1199-stack") && seen.branches[0].includes("6") &&
        seen.branches[1].includes("zo/v1199-weekly-review") && seen.branches[1].includes("20") &&
        seen.repoDoor && seen.scmShown,
      JSON.stringify(seen),
    );
  } finally { await page.close(); }
}
