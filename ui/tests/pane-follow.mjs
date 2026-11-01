import { openWindowTestPage } from "./window-boot.mjs";

// A pane follows its agent into the worktree its process moved to. The
// backend's `term:cwd` is stubbed; the catalog carries a project with a child
// checkout under `.zo/worktrees/` and a second project whose path is a prefix
// of a stranger's.
export async function exercisePaneFollow() {
  const settle = (ms = 80) => new Promise((done) => setTimeout(done, ms));
  const catalog = window.__ANSWER__.project_catalog;
  const priorLocale = locale;
  locale = "en";
  window.__ANSWER__.project_catalog = () => [
    {
      name: "follow", path: "/tmp/follow",
      worktrees: [
        { path: "/tmp/follow", branch: "main", is_main: true, active: true, is_folder: false },
        { path: "/tmp/follow/.zo/worktrees/feat", branch: "feat", is_main: false, active: false, is_folder: false },
        { path: "/tmp/follow/.zo/worktrees/detached", branch: null, is_main: false, active: false, is_folder: false },
      ],
    },
    { name: "follow-b", path: "/tmp/follow-b", worktrees: [{ path: "/tmp/follow-b", branch: "main", is_main: true, active: false, is_folder: false }] },
  ];
  await refreshWorktrees(); await settle();
  paintWorktreeAgents();
  // 「에이전트 창 없음」 is said once, on the project's head with the count of its checkouts that hold no agent window
  // (t-84799 A1) — not again on every row.
  const head = () => document.querySelector('.proj-row[data-project-path="/tmp/follow"] .proj-unseated');
  const emptyExplained = head()?.hidden === false && head()?.textContent === "No agent window 3";
  const rowsSayNothing = document.querySelectorAll(".wt-row .wt-unseated").length === 0;
  locale = "ko"; applyLocale();
  paintWorktreeAgents();
  const captionLocalized = head()?.textContent === "에이전트 창 없음 3";
  mountTermTab(9401, { agent: "zo", worktree: "/tmp/follow" }, { placement: "tab", focus: false });
  paneAgents.set(9401, "zo");
  const fire = (cwd) => { for (const handler of window.__LISTENERS__["term:cwd"] ?? []) handler({ payload: { term: 9401, cwd } }); };
  const seen = { before: tabOfTerm(9401)?.worktree };
  fire("/tmp/follow/.zo/worktrees/feat/crates/zerocode-shell"); await settle(); await settle();
  seen.followed = tabOfTerm(9401)?.worktree;
  // The agent stands in main, then follows into feat: of the three checkouts, the ones without a window are main and
  // detached — the head counts again.
  seen.emptyCaptionGone = head()?.textContent === "에이전트 창 없음 2";
  seen.emptyExplained = emptyExplained;
  seen.detachedExplained = rowsSayNothing;
  seen.captionLocalized = captionLocalized;
  fire("/tmp/follow-bee/src"); await settle(); await settle();
  seen.prefixSafe = tabOfTerm(9401)?.worktree;
  fire("/tmp/follow/ui"); await settle(); await settle();
  seen.backToRoot = tabOfTerm(9401)?.worktree;
  seen.agentKept = paneAgents.get(9401) === "zo" && !!tabOfTerm(9401);
  // A plain shell is not an agent: a person cd-ing into another checkout must
  // not have the tab jump groups under them.
  mountTermTab(9402, { worktree: "/tmp/follow" }, { placement: "tab", focus: false });
  for (const handler of window.__LISTENERS__["term:cwd"] ?? []) handler({ payload: { term: 9402, cwd: "/tmp/follow/.zo/worktrees/feat" } });
  await settle(); await settle();
  seen.plainShellStays = tabOfTerm(9402)?.worktree === "/tmp/follow";
  window.__ANSWER__.project_catalog = catalog;
  locale = priorLocale; applyLocale();
  await refreshWorktrees(); await settle();
  return seen;
}

export async function testPaneFollowsCwd(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const seen = await page.evaluate(exercisePaneFollow);
    ok(
      "a pane follows its agent into the checkout its process moved to — the deepest containing worktree, never a prefix sibling — and back; a plain shell stays put",
      seen.before === "/tmp/follow" &&
        seen.followed === "/tmp/follow/.zo/worktrees/feat" &&
        seen.prefixSafe === "/tmp/follow/.zo/worktrees/feat" &&
        seen.backToRoot === "/tmp/follow" &&
        seen.agentKept &&
        seen.plainShellStays && seen.emptyExplained && seen.emptyCaptionGone && seen.detachedExplained && seen.captionLocalized,
      JSON.stringify(seen),
    );
    ok("following a pane raised no renderer errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}
