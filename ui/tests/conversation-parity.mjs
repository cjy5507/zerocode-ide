/* The conversation view against the Claude Code extension's own webview
 * (2.1.280, the installed CLI's): what `docs/design/agent-conversation-
 * claude-code-grammar-20260915.md` §10 lists as the remaining differences,
 * each pinned where a person would see it (t-6323). The checks read the page
 * a browser actually laid out — a font is the face the engine drew with, a
 * fold is the height the row stood at — never the source's own spelling. */
import { openWindowTestPage } from "./window-boot.mjs";

/* A wire session's page opened on `history` — the page with the most roads
 * on it (turns, a live answer, the composer's wire) — painted once. */
export async function openConversation(page, history, { status = "idle", live = [] } = {}) {
  await page.evaluate(async ({ history, status, live }) => {
    window.__CONVERSATION__ = { status, live, turns: [] };
    window.__ANSWER__.wire_start = (args) => ({ id: 41, agent: args.agent, protocol: "claude-stream", version: "2.1.280", model: null, session: null });
    window.__ANSWER__.wire_log = (args) => {
      const held = window.__CONVERSATION__;
      return {
        found: true, skipped: false, next: held.turns.length, turns: held.turns.slice(args.after ?? 0),
        status: held.status, asks: [], live: held.live, agent: "claude", protocol: "claude-stream",
        models: [], modes: [], commands: [], version: "2.1.280",
      };
    };
    window.__ANSWER__.wire_stop = () => null;
    await openWirePage("claude", "/tmp/zerocode-window-test", { history });
    await pollHelperPages();
    await window.__PAINTED__();
  }, { history, status, live });
}

/* The faces the engine drew a node's text with (CDP, the renderer's own
 * answer) — not the declared list, which names faces that may not exist. */
async function drawnFaces(cdp, selector) {
  const doc = await cdp.send("DOM.getDocument", { depth: 0 });
  const found = await cdp.send("DOM.querySelector", { nodeId: doc.root.nodeId, selector });
  if (!found.nodeId) return null;
  const { fonts } = await cdp.send("CSS.getPlatformFontsForNode", { nodeId: found.nodeId });
  return fonts.map((one) => one.familyName);
}

/* A0 — the face. The extension's panel wears VS Code's UI face
 * (`--vscode-font-family`, the platform's sans), and so must this page,
 * whatever the person chose: a chosen face that is not installed — the
 * default `Geist` is a name, never a shipped file — must fall through to the
 * platform's sans, not to the engine's default face (WebKit's Times, and for
 * Hangul AppleMyungjo: the serif the person saw, 2026-09-23). Which face is
 * the engine's default depends on the engine and the page's language, so the
 * page carries two controls beside the words: the same Latin word in a face
 * that does not exist (what the engine falls to) and in the platform's sans
 * (what the extension wears). Five ways a choice can arrive: the default,
 * none, a face this machine lacks, the same typed with quotes, and a list. */
export async function testConversationFont(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const cdp = await page.context().newCDPSession(page);
    await cdp.send("DOM.enable");
    await cdp.send("CSS.enable");
    await openConversation(page, [
      { role: "user", text: "대화 뷰가 느려요 — why is it slow?" },
      { role: "assistant", text: "원인은 paint가 폴마다 목록을 다시 세운 것입니다 — the list was rebuilt on every poll." },
      { role: "tool", text: "Read · ui/shell.js", tool: { call_id: "r1", name: "Read", input: "ui/shell.js", is_error: false } },
    ]);
    await page.evaluate(() => {
      const list = document.querySelector("#worker-view .helper-turns");
      const controls = [
        ["conversation-font-engine", "\"No Such Face 6323 Control\""],
        ["conversation-font-system", "-apple-system, BlinkMacSystemFont, sans-serif"],
      ];
      for (const [id, face] of controls) {
        const control = document.createElement("span");
        control.id = id;
        control.style.fontFamily = face;
        control.textContent = "Read";
        list.prepend(control);
      }
    });
    const engine = await drawnFaces(cdp, "#conversation-font-engine");
    const system = await drawnFaces(cdp, "#conversation-font-system");
    const cases = {};
    const choices = [
      ["default", "Geist"],
      ["unset", ""],
      ["missing", "No Such Face 6323"],
      ["quoted", "\"No Such Face 6323\""],
      ["listed", "No Such Face 6323, Also Missing 6323"],
    ];
    for (const [name, choice] of choices) {
      const declared = await page.evaluate(async (choice) => {
        appFontFamily = choice;
        applyAppFontFamily();
        await window.__PAINTED__();
        const prose = document.querySelector("#worker-view .helper-turn.is-assistant .helper-said");
        return {
          choice: document.documentElement.style.getPropertyValue("--font-ui-choice"),
          prose: getComputedStyle(prose).fontFamily,
          tool: getComputedStyle(document.querySelector("#worker-view .helper-tool-name")).fontFamily,
        };
      }, choice);
      const drawn = {
        prose: await drawnFaces(cdp, "#worker-view .helper-turn.is-assistant .helper-said p"),
        tool: await drawnFaces(cdp, "#worker-view .helper-tool-name"),
        user: await drawnFaces(cdp, "#worker-view .helper-turn.is-user .helper-said"),
      };
      // The Latin words stand in the platform's sans: the tool's name is all
      // Latin, and the prose and the person's words carry Latin beside Hangul.
      const latinInSans = drawn.tool?.join() === system?.join() &&
        [drawn.prose, drawn.user].every((faces) => faces?.includes(system?.[0]) === true);
      const fellToEngine = engine?.[0] !== system?.[0] &&
        Object.values(drawn).some((faces) => faces?.[0] === engine?.[0]);
      // The declared list ends in the platform's sans whatever came first.
      const chained = [declared.prose, declared.tool].every((list) =>
        list.includes("-apple-system") && /sans-serif\s*$/.test(list));
      cases[name] = { declared, drawn, latinInSans, fellToEngine, chained };
    }
    ok(
      "A0: the conversation draws its Latin words in the platform's sans whatever face the person chose — the default Geist (a name, not a shipped file), none, a face this machine lacks, the same typed in quotes, and a typed list all fall through to the system sans, never to the engine's default face (WebKit: Times / AppleMyungjo); the declared list keeps the system chain after the choice, a quoted name is that name, and a typed list stays a list",
      engine?.length > 0 && system?.length > 0 &&
        Object.values(cases).every((one) => one.latinInSans && !one.fellToEngine && one.chained) &&
        cases.quoted.declared.choice === cases.missing.declared.choice &&
        cases.listed.declared.choice.split(",").length === 2,
      JSON.stringify({ engine, system, cases }),
    );
    ok("A0: the font cases raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}
