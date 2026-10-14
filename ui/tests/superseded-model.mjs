import { openWindowTestPage } from "./window-boot.mjs";

/* A pane on a release the catalog superseded says so (t-46882).
 *
 * The person, 10-11 16:5x: 「페이블 5.1이 있는데 5를 왜」 — a coordinator's
 * pane came back after a restart on `claude-fable-5` while the catalog held
 * `claude-fable-5-1`, and nothing on screen said the pane ran an old release.
 * Now the pane's sidebar row and — while it is the terminal in front — the
 * status bar wear one word beside the model, with the release standing for
 * it in the tip, in every catalog. The verdict is the backend's
 * (`superseded_models` once at boot, then every lineup read,
 * `summon-lineup:superseded`), and drawing a row asks it nothing; a pane on a
 * current release wears its model and nothing more. */
export async function testSupersededModel({ browser, origin, ok, faults }) {
  const { page } = await openWindowTestPage(browser, origin, { faults });
  try {
    await page.waitForFunction(() => projectsRead);
    await page.evaluate(async () => { setEveryProjectClosed(false); await refreshWorktrees(); });
    const seen = await page.evaluate(async () => {
      const seen = {};
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      const settle = async () => {
        await new Promise((done) => setTimeout(done, 30));
        await window.__PAINTED__();
      };
      const term = await openTermTab({ placement: "tab" });
      const owner = tabOfTerm(term);
      const card = () => document.querySelector(`.wt-agents[data-worktree-path="${CSS.escape(owner.worktree)}"]`);
      const row = () => [...(card()?.querySelectorAll(".wt-agent") ?? [])].find((one) => !one.dataset.sub) ?? null;
      const worn = () => row()?.querySelector(".wt-agent-model") ?? null;
      const bar = () => document.getElementById("sb-model");

      // A current release first: its model, and nothing more.
      tell("hook:agent", { term, state: "done", agent: "claude", session: "s-46882", model: "claude-opus-5-5" });
      await settle();
      setActiveTab(owner.id);
      seen.currentRow = worn()?.textContent ?? null;
      seen.currentBar = bar().textContent;

      // The catalog's word arrives with a read; then the pane reports the
      // superseded release. Drawing its row asks the backend nothing.
      tell("summon-lineup:superseded", { claude: { "claude-fable-5": "claude-fable-5-1" } });
      tell("hook:agent", { term, state: "done", agent: "claude", session: "s-46882", model: "claude-fable-5" });
      await window.__PAINTED__();
      window.__COUNTS__ = {};
      paintWorktreeAgents();
      paintStatusModel();
      seen.askedToDraw = Object.keys(window.__COUNTS__);
      await settle();
      seen.row = worn()?.textContent ?? null;
      seen.rowTip = worn()?.dataset.tip ?? null;
      seen.bar = bar().textContent;
      seen.barTip = bar().dataset.tip ?? null;

      // Every catalog words it, the model ids as the vendor said them.
      const wore = locale;
      seen.byLocale = {};
      for (const code of ["en", "ja", "zh", "es"]) {
        setLocale(code, { refresh: false, persist: false });
        paintWorktreeAgents();
        paintStatusModel();
        seen.byLocale[code] = { row: worn()?.textContent ?? "", tip: worn()?.dataset.tip ?? "", bar: bar().textContent };
      }
      setLocale(wore, { refresh: false, persist: false });
      paintWorktreeAgents();
      paintStatusModel();

      // The pane moves to the release that stands: the word goes from both.
      tell("hook:agent", { term, state: "done", agent: "claude", session: "s-46882", model: "claude-fable-5-1" });
      await settle();
      seen.movedRow = worn()?.textContent ?? null;
      seen.movedBar = bar().textContent;

      // A later read says the catalog moved on again, and the event alone
      // brings the word back.
      tell("summon-lineup:superseded", { claude: { "claude-fable-5-1": "claude-fable-5-2" } });
      await settle();
      seen.readRow = worn()?.textContent ?? null;
      seen.readTip = worn()?.dataset.tip ?? null;

      tell("summon-lineup:superseded", {});
      tell("term:exited", { term });
      window.__PANES__ = [];
      for (const tab of [...tabs]) dropTab(tab.id);
      for (const at of [...termViews.keys()]) dropTermView(at);
      return seen;
    });
    ok(
      "a pane on a current release wears its model and nothing more, on its row and in the status bar",
      seen.currentRow === "claude-opus-5-5" && seen.currentBar === "",
      JSON.stringify(seen),
    );
    ok(
      "a pane on a superseded release wears one word beside its model — its sidebar row and the status bar — and the release standing for it in the tip (t-46882)",
      seen.row === "claude-fable-5 · 대체됨" && seen.bar === "claude-fable-5 · 대체됨" &&
        seen.rowTip?.includes("claude-fable-5-1") && seen.barTip?.includes("claude-fable-5-1") &&
        seen.askedToDraw.length === 0,
      JSON.stringify(seen),
    );
    const worded = Object.entries(seen.byLocale);
    ok(
      "every catalog words it, the ids as the vendor said them: en, ja, zh and es each their own word and tip",
      worded.length === 4 &&
        worded.every(([, said]) => said.row.startsWith("claude-fable-5 · ") && !said.row.endsWith("대체됨") &&
          said.row === said.bar && said.tip.includes("claude-fable-5-1") && !said.tip.includes("대체")) &&
        new Set(worded.map(([, said]) => said.row)).size === 4,
      JSON.stringify(seen.byLocale),
    );
    ok(
      "a pane moved to the release that stands wears that model alone; a later read alone brings the word back",
      seen.movedRow === "claude-fable-5-1" && seen.movedBar === "" &&
        seen.readRow === "claude-fable-5-1 · 대체됨" && seen.readTip?.includes("claude-fable-5-2"),
      JSON.stringify(seen),
    );
  } finally {
    await page.close();
  }
}
