import { openWindowTestPage } from "./window-boot.mjs";

export async function testSidebarReclaimReason(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.waitForFunction(() => projectsRead);
    await page.evaluate(async () => {
      setLocale("ko", { persist: false });
      setEveryProjectClosed(false);
      window.__RECLAIM_HOLD__ = null;
      window.__ANSWER__.project_catalog = () => [{ name: "r", path: "/r", worktrees: [
        { path: "/r", branch: "main", is_main: true, active: true },
        { path: "/r/kept", branch: "wt/kept", is_main: false, active: false,
          ownership: "zerocode-managed", reclaim: window.__RECLAIM_HOLD__,
          landing: { state: "landed", ahead: 0, dirty: false, ignored: true, compare_ref: "origin/main" } },
      ] }];
      setSidebarView({ groupBy: "repo" });
      await refreshWorktrees();
      await window.__PAINTED__();
    });
    for (const [kind, word, detail] of [
      ["open_conversation", "열린 터미널 있음", "1 terminal still stands in this checkout"],
      ["locked", "워크트리 잠김", "git reports that the worktree is locked"],
      ["ignored_file", "보존할 파일 있음", ".notes-local would be removed with the checkout"],
    ]) {
      const seen = await page.evaluate(async ({ kind, detail }) => {
        window.__RECLAIM_HOLD__ = { kind, detail };
        await refreshWorktrees();
        await window.__PAINTED__();
        const chip = document.querySelector('.wt-row[data-worktree-path="/r/kept"] .wt-landing');
        return { word: chip?.textContent, tip: chip?.dataset.tip, cleanable: chip?.dataset.cleanable };
      }, { kind, detail });
      ok(`the sidebar names the reclaimer's ${kind} reason instead of blaming build output`,
        seen.word?.includes(word) && !seen.word?.includes("무시된 파일 남음")
          && seen.tip?.includes(detail) && !seen.cleanable, JSON.stringify(seen));
    }
    // t-73910: a checkout kept for the worker's own unlanded commits says how many and the base it waits for.
    const waiting = await page.evaluate(async () => {
      window.__RECLAIM_HOLD__ = {
        kind: "unlanded",
        detail: "2 commit(s) on wt/kept are not in main yet, 1 of them on no other branch",
        waits: { own: 1, base: "main" },
      };
      await refreshWorktrees();
      await window.__PAINTED__();
      const chip = document.querySelector('.wt-row[data-worktree-path="/r/kept"] .wt-landing');
      return {
        word: chip?.textContent, tip: chip?.dataset.tip, cleanable: chip?.dataset.cleanable,
        expected: t("worktree.reclaimUnlanded", "워커 커밋 {{own}}개 {{base}} 대기", { own: 1, base: "main" }),
        line: t("worktree.reclaimWaits", "워커가 여기서 만든 커밋 {{own}}개가 아직 {{base}}에 없습니다 — 들어가면 자동으로 정리합니다", { own: 1, base: "main" }),
        missing: ["en", "ja", "zh", "es"].filter((code) =>
          !CATALOG[code]?.["worktree.reclaimUnlanded"] || !CATALOG[code]?.["worktree.reclaimWaits"]),
      };
    });
    ok("a checkout kept for the worker's own unlanded commits says how many and the base it waits for, on its chip and in its tip",
      waiting.word?.includes(waiting.expected) && !waiting.word?.includes("정리 보류") && waiting.tip?.includes(waiting.line)
        && !waiting.cleanable && waiting.missing.length === 0, JSON.stringify(waiting));
    const cleared = await page.evaluate(async () => {
      window.__RECLAIM_HOLD__ = null;
      await refreshWorktrees();
      await window.__PAINTED__();
      const chip = document.querySelector('.wt-row[data-worktree-path="/r/kept"] .wt-landing');
      return { word: chip?.textContent, cleanable: chip?.dataset.cleanable };
    });
    ok("clearing a reclaim hold removes its stale reason and restores the ordinary cleanup offer",
      cleared.word?.includes("무시된 파일 남음") && cleared.cleanable === "1", JSON.stringify(cleared));
    ok("reclaim reasons raise no renderer errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}
