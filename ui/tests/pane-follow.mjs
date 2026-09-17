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
  const childCaption = () => document.querySelector('.wt-row[data-worktree-path="/tmp/follow/.zo/worktrees/feat"] .wt-unseated');
  const emptyExplained = childCaption()?.hidden === false && childCaption()?.textContent === "No agent window";
  const detached = document.querySelector('.wt-row[data-worktree-path="/tmp/follow/.zo/worktrees/detached"] .wt-unseated');
  const detachedExplained = detached?.hidden === false && !detached.parentElement.hidden;
  locale = "ko"; applyLocale();
  const captionLocalized = childCaption()?.textContent === "에이전트 창 없음";
  mountTermTab(9401, { agent: "zo", worktree: "/tmp/follow" }, { placement: "tab", focus: false });
  paneAgents.set(9401, "zo");
  const fire = (cwd) => { for (const handler of window.__LISTENERS__["term:cwd"] ?? []) handler({ payload: { term: 9401, cwd } }); };
  const seen = { before: tabOfTerm(9401)?.worktree };
  fire("/tmp/follow/.zo/worktrees/feat/crates/zerocode-shell"); await settle(); await settle();
  seen.followed = tabOfTerm(9401)?.worktree;
  seen.emptyCaptionGone = childCaption()?.hidden === true;
  seen.emptyExplained = emptyExplained;
  seen.detachedExplained = detachedExplained;
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
