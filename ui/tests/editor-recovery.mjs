import { mkdir } from "node:fs/promises";
import { openWindowTestPage } from "./window-boot.mjs";

export async function testEditorRecovery(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(() => {
      editingPrefs.editor_auto_save = false;
      activeWorktreePath = "/recovery-a";
      focusedPane = stageGroups()[0];
      window.__RECOVERY_SAVED__ = {};
      window.__RECOVERY_WRITES__ = [];
      window.__RECOVERY_READS__ = [];
      window.__RECOVERY_IN_FLIGHT__ = 0;
      window.__RECOVERY_MAX_IN_FLIGHT__ = 0;
      window.__ANSWER__.file_version = ({ path }) => {
        if (path.startsWith("/recovery-missing/")) throw new Error("file is missing");
        return "v1";
      };
      window.__ANSWER__.read_text_file = (args) => {
        window.__RECOVERY_READS__.push(args);
        if (args.path.startsWith("/recovery-missing/")) {
          if (!args.allowMissing) throw new Error("file is missing");
          return { text: "", version: "", missing: true };
        }
        return { text: "disk original", version: "v1" };
      };
      window.__ANSWER__.save_stage_layouts = async (args) => {
        const record = JSON.parse(JSON.stringify(args));
        window.__RECOVERY_IN_FLIGHT__ += 1;
        window.__RECOVERY_MAX_IN_FLIGHT__ = Math.max(
          window.__RECOVERY_MAX_IN_FLIGHT__, window.__RECOVERY_IN_FLIGHT__,
        );
        try {
          if (window.__RECOVERY_BLOCK_NEXT__) {
            window.__RECOVERY_BLOCK_NEXT__ = false;
            await new Promise((done) => { window.__RECOVERY_RELEASE__ = done; });
          }
          if (window.__RECOVERY_FAIL_NEXT__) {
            window.__RECOVERY_FAIL_NEXT__ = false;
            throw new Error("recovery disk full");
          }
          window.__RECOVERY_SAVED__[args.worktree] = record.layout;
          window.__RECOVERY_WRITES__.push({ ...record, typing: !!window.__RECOVERY_TYPING__ });
        } finally {
          window.__RECOVERY_IN_FLIGHT__ -= 1;
        }
      };
      window.__RECOVERY_TYPE__ = (path, text) => {
        const tab = tabs.find((one) => one.id === `file:${path}`);
        const editor = editorShowing(tab);
        editor.dispatch({ changes: { from: 0, to: editor.state.doc.length, insert: text } });
      };
      window.__RECOVERY_DRAFT__ = (worktree, path) =>
        window.__RECOVERY_SAVED__[worktree]?.groups.flatMap((group) => group.tabs)
          .find((tab) => tab.path === path)?.draft;
    });

    for (const preview of [false, true]) {
      const path = `/recovery-a/${preview ? "preview" : "plain"}.txt`;
      await page.evaluate(async ({ path, preview }) => {
        await openFile(path, { preview });
        window.__RECOVERY_TYPE__(path, "first edit");
      }, { path, preview });
      await page.waitForFunction((path) => window.__RECOVERY_DRAFT__("/recovery-a", path) === "first edit", path);
      await page.evaluate((path) => window.__RECOVERY_TYPE__(path, "latest unsaved edit"), path);
      await page.waitForFunction((path) => window.__RECOVERY_DRAFT__("/recovery-a", path) === "latest unsaved edit", path);
      const seen = await page.evaluate((path) => ({
        draft: window.__RECOVERY_DRAFT__("/recovery-a", path),
        preview: tabs.find((tab) => tab.path === path).preview,
        sourceWrites: window.__COUNTS__.write_text_file ?? 0,
      }), path);
      ok(`${preview ? "preview" : "ordinary"} editor checkpoints later typing without enabling source auto-save`,
        seen.draft === "latest unsaved edit" && !seen.preview && seen.sourceWrites === 0, JSON.stringify(seen));
    }

    const continuous = await page.evaluate(async () => {
      window.__RECOVERY_TYPING__ = true;
      const start = window.__RECOVERY_WRITES__.length;
      for (let at = 0; at < 12; at += 1) {
        window.__RECOVERY_TYPE__("/recovery-a/preview.txt", `continuous ${at}`);
        await new Promise((done) => setTimeout(done, STAGE_DRAFT_SAVE_MS / 6));
      }
      window.__RECOVERY_TYPING__ = false;
      return window.__RECOVERY_WRITES__.slice(start).filter((write) => write.typing).length;
    });
    await page.waitForFunction(() => window.__RECOVERY_DRAFT__("/recovery-a", "/recovery-a/preview.txt") === "continuous 11");
    ok("continuous typing reaches recovery storage before the typist pauses", continuous > 0 && continuous < 12, String(continuous));

    await page.waitForFunction(() => !savingStageLayouts && pendingStageLayouts.size === 0);
    const queued = await page.evaluate(async () => {
      window.__RECOVERY_BLOCK_NEXT__ = true;
      window.__RECOVERY_TYPE__("/recovery-a/preview.txt", "held write");
      persistStageLayouts();
      window.__RECOVERY_TYPE__("/recovery-a/preview.txt", "superseded draft");
      window.__RECOVERY_TYPE__("/recovery-a/preview.txt", "newest draft");
      activeWorktreePath = "/recovery-b";
      focusedPane = stageGroups()[0];
      await openFile("/recovery-b/other.txt");
      window.__RECOVERY_TYPE__("/recovery-b/other.txt", "other worktree");
      await new Promise((done) => setTimeout(done, STAGE_DRAFT_SAVE_MS * 2));
      const seen = { inFlight: window.__RECOVERY_IN_FLIGHT__, queued: [...pendingStageLayouts.keys()] };
      window.__RECOVERY_RELEASE__();
      return seen;
    });
    await page.waitForFunction(() =>
      window.__RECOVERY_DRAFT__("/recovery-a", "/recovery-a/preview.txt") === "newest draft" &&
      window.__RECOVERY_DRAFT__("/recovery-b", "/recovery-b/other.txt") === "other worktree",
    );
    const maxInFlight = await page.evaluate(() => window.__RECOVERY_MAX_IN_FLIGHT__);
    ok("slow recovery writes are serialized, coalesced, and retain the worktree captured before navigation",
      queued.inFlight === 1 && maxInFlight === 1 && queued.queued.includes("/recovery-a") && queued.queued.includes("/recovery-b"),
      JSON.stringify({ ...queued, maxInFlight }));

    const backgroundSave = await page.evaluate(async () => {
      window.__ANSWER__.write_text_file = () => "v2";
      const tab = tabs.find((one) => one.path === "/recovery-a/preview.txt");
      return saveFile(tab);
    });
    await page.waitForFunction(() => window.__RECOVERY_DRAFT__("/recovery-a", "/recovery-a/preview.txt") === undefined);
    const backgroundStayed = await page.evaluate(() =>
      activeWorktreePath === "/recovery-b" && window.__RECOVERY_DRAFT__("/recovery-b", "/recovery-b/other.txt") === "other worktree",
    );
    ok("a background file save clears only its own worktree recovery draft", backgroundSave && backgroundStayed);

    const saved = await page.evaluate(async () => {
      const tab = tabs.find((one) => one.path === "/recovery-b/other.txt");
      window.__ANSWER__.write_text_file = () => "v2";
      const success = await saveFile(tab);
      return { success, dirty: isDirty(tab) };
    });
    await page.waitForFunction(() => window.__RECOVERY_DRAFT__("/recovery-b", "/recovery-b/other.txt") === undefined);
    const clean = await page.evaluate(() => window.__RECOVERY_DRAFT__("/recovery-b", "/recovery-b/other.txt") === undefined);
    ok("a successful source save clears its obsolete recovery draft", saved.success && !saved.dirty && clean, JSON.stringify(saved));

    const failed = await page.evaluate(async () => {
      const notices = [];
      const previous = console.error;
      console.error = (error) => notices.push(String(error));
      window.__RECOVERY_FAIL_NEXT__ = true;
      persistStageLayouts();
      await new Promise((done) => setTimeout(done, 0));
      console.error = previous;
      window.__RECOVERY_TYPE__("/recovery-b/other.txt", "after storage recovers");
      return notices;
    });
    await page.waitForFunction(() => window.__RECOVERY_DRAFT__("/recovery-b", "/recovery-b/other.txt") === "after storage recovers");
    ok("a failed recovery write is visible and does not wedge later checkpoints", failed.some((word) => word.includes("recovery disk full")), JSON.stringify(failed));

    const restored = await page.evaluate(async () => {
      const worktree = "/recovery-missing";
      activeWorktreePath = worktree;
      window.__ANSWER__.stage_layouts = () => ({
        root: { type: "leaf" }, focused: 0,
        groups: [{ active: 0, tabs: [
          { kind: "file", path: `${worktree}/gone.md`, draft: "only surviving copy" },
          { kind: "file", path: `${worktree}/empty.txt`, draft: "" },
        ] }],
      });
      await restoreStageLayout(worktree);
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const held = tabs.filter((tab) => tab.worktree === worktree);
      const visible = editorShowing(held.find((tab) => tab.path.endsWith("gone.md")))?.state.doc.toString();
      persistStageLayouts();
      return { tabs: held.map((tab) => ({ path: tab.path, gone: tab.gone, draft: tab.draft })), visible };
    });
    await page.waitForFunction(() => !savingStageLayouts && pendingStageLayouts.size === 0);
    const carried = await page.evaluate(() => ({
      empty: window.__RECOVERY_DRAFT__("/recovery-missing", "/recovery-missing/empty.txt"),
      missingReads: window.__RECOVERY_READS__.filter((args) => args.path.startsWith("/recovery-missing/")),
    }));
    ok("missing files reopen with their unsaved draft, including an intentionally empty draft",
      restored.tabs.length === 2 && restored.tabs.every((tab) => tab.gone) &&
      restored.visible === "only surviving copy" && carried.empty === "" && carried.missingReads.every((args) => args.allowMissing === true),
      JSON.stringify({ restored, carried }));
    await mkdir("output/playwright", { recursive: true });
    await page.screenshot({ path: "output/playwright/editor-recovery.png" });

    const rescued = await page.evaluate(async () => {
      const writes = [];
      window.__ANSWER__.write_text_file = (args) => { writes.push(args); return "restored-v1"; };
      const tab = tabs.find((one) => one.path === "/recovery-missing/gone.md");
      const success = await saveFile(tab);
      return { success, gone: tab.gone, dirty: isDirty(tab), writes };
    });
    ok("saving a restored missing file uses the guarded recreation path with the recovered bytes",
      rescued.success && !rescued.gone && !rescued.dirty && rescued.writes.length === 1 &&
      rescued.writes[0].createIfMissing === true && rescued.writes[0].text === "only surviving copy", JSON.stringify(rescued));
    ok("editor recovery raised no renderer errors", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}
