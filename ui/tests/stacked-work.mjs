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

export async function testStackedWork(browser, origin, ok) {
  const { page } = await openWindowTestPage(browser, origin);
  try {
    const seen = await page.evaluate(async () => {
      const seen = {};
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
      await refreshWorktrees();
      absorbReleaseStatus({ status: { sha: "ca1ffcf0d1", version: "1.1.98", phase: "gate", started_at: new Date(Date.now() - 60_000).toISOString(),
        updated_at: new Date().toISOString(), phases: [] } });
      const tab = document.getElementById("activity-pile-tab");
      const panel = document.getElementById("activity-pile");
      seen.tabFirst = tab?.parentElement?.firstElementChild === tab;
      seen.count = document.getElementById("activity-pile-count")?.textContent ?? "";
      seen.emptyWhileClosed = panel ? panel.hidden && panel.childElementCount === 0 : false;
      tab?.click();
      await window.__PAINTED__();
      seen.open = panel && !panel.hidden;
      seen.selected = tab?.getAttribute("aria-selected");
      seen.release = panel?.querySelector(".pile-release")?.textContent.replace(/\s+/g, " ").trim() ?? "";
      seen.total = panel?.querySelector(".pile-total")?.textContent.replace(/\s+/g, " ").trim() ?? "";
      seen.branches = [...(panel?.querySelectorAll(".pile-branch") ?? [])].map((row) => row.textContent.replace(/\s+/g, " ").trim());
      seen.repoDoor = Boolean(panel?.querySelector(".pile-scm"));
      panel?.querySelector(".pile-scm")?.click();
      await window.__PAINTED__();
      seen.scmShown = !document.getElementById("activity-scm").hidden;
      seen.emptiedOnLeave = panel.hidden && panel.childElementCount === 0;
      return seen;
    });
    ok(
      "the right column's first tab counts what has not landed, and its panel is built only while it is up",
      seen.tabFirst && seen.count === "8" && seen.emptyWhileClosed && seen.open && seen.selected === "true" && seen.emptiedOnLeave,
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
