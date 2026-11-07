/* ---- the Skills·MCP page: one common store, driven for real (2026-11-06) ----
 *
 * The person's rule — whatever any agent installs, every agent sees — is the
 * backend keeper's (`library_runtime`, its own tests on a fake home). What this
 * suite holds is the page that shows and drives it: the three tabs, rows that
 * say every state in words, a server's detail with its values only ever named,
 * the switches and the questions sending exactly the command the keeper reads,
 * the add dialog reading a pasted README block and a typed command line, the
 * deletes asking first and naming what goes, 「최근 변경」 undoing, and the
 * narrow pane putting the detail over the list. No fault may be raised. */
const AGENTS = [
  { id: "claude", label: "Claude Code", present: true, skills: "window-home", skills_state: "sees", mcp_state: "ok", mcp_file: "/Users/dev/app/.claude/.claude.json", writes: "command", login: true, favicon_domain: "" },
  { id: "codex", label: "Codex", present: true, skills: "native", skills_state: "sees", mcp_state: "ok", mcp_file: "/Users/dev/.codex/config.toml", writes: "command", login: true, favicon_domain: "" },
  { id: "gemini", label: "Gemini CLI", present: true, skills: "native", skills_state: "sees", mcp_state: "ok", mcp_file: "/Users/dev/.gemini/settings.json", writes: "command", login: false, favicon_domain: "" },
  { id: "kiro", label: "Kiro", present: true, skills: "own-only", skills_state: "own-only", mcp_state: "unreadable", mcp_file: "/Users/dev/.kiro/settings/mcp.json", writes: "file", login: false, favicon_domain: "" },
  { id: "cursor", label: "Cursor", present: false, skills: "native", skills_state: "absent", mcp_state: "absent", mcp_file: "/Users/dev/.cursor/mcp.json", writes: "file", login: false, favicon_domain: "" },
];
const agentsHave = (states) => Object.entries(states).map(([id, state]) => ({ id, state }));
const LIBRARY = {
  paused: false, running: false, merged_at: 1, store_skills: "/Users/dev/.agents/skills", store_mcp: "/Users/dev/.agents/mcp.json",
  agents: AGENTS,
  servers: [
    { name: "rea", kind: "stdio", summary: "rea mcp", secrets: ["REA_TOKEN"], origin: "codex", at_ms: 1, in_store: true, disabled: false, own: null, shared: false, off: [], pick: [],
      agents: agentsHave({ claude: "has", codex: "has", gemini: "missing", kiro: "unreadable" }) },
    { name: "context7", kind: "stdio", summary: "context7-mcp", secrets: [], origin: "zo", at_ms: 1, in_store: false, disabled: false, own: null, shared: false, off: [],
      pick: [{ agents: ["claude", "zo"], fingerprint: "fp-local", summary: "context7-mcp", remote: false }, { agents: ["codex"], fingerprint: "fp-remote", summary: "https://mcp.context7.com/mcp", remote: true }],
      agents: agentsHave({ claude: "has", codex: "differs", gemini: "missing", kiro: "unreadable" }) },
    { name: "node_repl", kind: "stdio", summary: "/Applications/ChatGPT.app/Contents/Resources/cua_node/bin/node_repl", secrets: [], origin: "codex", at_ms: 1, in_store: false, disabled: false, own: "codex", shared: false, off: [], pick: [],
      agents: agentsHave({ claude: "elsewhere", codex: "has", gemini: "elsewhere", kiro: "unreadable" }) },
    { name: "kept", kind: "stdio", summary: "kept-mcp", secrets: [], origin: "codex", at_ms: 1, in_store: false, disabled: false, own: "codex", kept_with: ["codex", "gemini"], shared: false, off: [], pick: [],
      agents: agentsHave({ claude: "elsewhere", codex: "has", gemini: "has", kiro: "unreadable" }) },
    { name: "figma", kind: "http", summary: "https://mcp.figma.com/mcp", secrets: ["Authorization"], origin: "claude", at_ms: 1, in_store: true, disabled: false, own: null, shared: false, off: ["gemini"], pick: [],
      agents: agentsHave({ claude: "has", codex: "has", gemini: "off", kiro: "unreadable" }) },
  ],
  skills: [{ name: "pdf", origin: "codex", at_ms: 1, pick: [] }, { name: "notes", origin: "store", at_ms: 1, pick: [{ agent: "claude-mac", path: "/Users/dev/.claude/skills/notes", digest: "other" }] }],
  log: [
    { id: 104, at_ms: 1, kind: "first-merge", item: "both", name: "", who: "store", count_skills: 34, count_servers: 12, count_asked: 1, detail: "", undo: false },
    { id: 103, at_ms: 2, kind: "failed", item: "mcp", name: "rea", who: "gemini", count_skills: 0, count_servers: 0, count_asked: 0, detail: "library.commandTimedOut", undo: false },
    { id: 102, at_ms: 3, kind: "deleted", item: "mcp", name: "old-server", who: "person", count_skills: 0, count_servers: 0, count_asked: 0, detail: "", undo: true },
    { id: 101, at_ms: 4, kind: "taken-in", item: "skill", name: "pdf", who: "codex", count_skills: 0, count_servers: 0, count_asked: 0, detail: "", undo: false },
  ],
};
const variant = (id, name, source_id, source_kind, source_label, directory) => ({
  id, name, description: `${name} does one thing well.`, file: `${directory}/SKILL.md`, directory, source_id, source_label, source_kind,
  providers: ["agent-skills"], version: "1.0.0", digest: "d".repeat(64), roots: [], files: 1, updated_at: 1,
});
const SKILLS = {
  families: [
    { id: "family-pdf", name: "pdf", variants: [variant("v-pdf", "pdf", "home-agents", "home", "Agent skills home", "/Users/dev/.agents/skills/pdf")], conflict: false, latest_version: "1.0.0", usage: { count_7d: 3, last_used_ms: 1, agents: ["codex"] } },
    { id: "family-notes", name: "notes", variants: [variant("v-notes", "notes", "home-agents", "home", "Agent skills home", "/Users/dev/.agents/skills/notes")], conflict: false, latest_version: "1.0.0", usage: { count_7d: 0, last_used_ms: null, agents: [] } },
    { id: "family-kiro", name: "kiro-only", variants: [variant("v-kiro", "kiro-only", "home-kiro", "home", "Kiro home", "/Users/dev/.kiro/skills/kiro-only")], conflict: false, latest_version: "1.0.0", usage: { count_7d: 0, last_used_ms: null, agents: [] } },
    { id: "family-plugin", name: "docs", variants: [variant("v-plugin", "docs", "codex-plugin-cache", "plugin", "Codex plugin cache", "/Users/dev/.codex/plugins/cache/docs")], conflict: false, latest_version: "1.0.0", usage: { count_7d: 0, last_used_ms: null, agents: [] } },
  ],
  sources: [{ id: "home-agents", path: "/Users/dev/.agents/skills", kind: "home", found: 2, exists: true }],
  agents: [["claude", "Claude"], ["codex", "Codex"]], external_agents: [["codex", "Codex"]], external_unavailable: [],
  required: [], bundled: [], policy: { terminal_rows: 24, terminal_cols: 96, install_poll_ms: 0, install_watch_ms: 0 },
  reads: 4, capped: false, evidence_scope: "hooks-only", evidence_count: 0, plans: [],
};

const STAND = `(async (library, skills) => {
  setLocale("ko", { persist: false, refresh: false });
  setSettingsOpen(false);
  window.__LIB_CALLS__ = [];
  const record = (command, answer) => (args) => { window.__LIB_CALLS__.push({ command, args: JSON.parse(JSON.stringify(args ?? {})) }); return typeof answer === "function" ? answer(args) : answer; };
  window.__ANSWER__.skills_list = () => skills;
  window.__ANSWER__.skills_rescan = () => skills;
  window.__ANSWER__.skill_detail = ({ id }) => ({ skill: skills.families.flatMap((f) => f.variants).find((v) => v.id === id), markdown: "# Body\\n\\nThe skill's own text.", evidence: [] });
  window.__ANSWER__.library_report = () => library;
  window.__ANSWER__.library_look = () => library;
  for (const command of ["library_server_switch", "library_server_agent", "library_server_choose", "library_server_share", "library_server_remove", "library_server_save", "library_paste_add", "library_skill_remove", "library_skill_import", "library_skill_choose", "library_undo", "library_pause"]) {
    window.__ANSWER__[command] = record(command, library);
  }
  window.__ANSWER__.library_paste_read = record("library_paste_read", [{ name: "github", kind: "stdio", summary: "npx -y @modelcontextprotocol/server-github", secrets: ["GITHUB_TOKEN"] }]);
  window.__ANSWER__.library_server_check = record("library_server_check", { state: "connected", detail: "Status: ✓ Connected" });
  window.__ANSWER__.library_skill_list = record("library_skill_list", ["release-notes", "code-review"]);
  window.__ANSWER__.library_skill_create = record("library_skill_create", "/Users/dev/.agents/skills/release-notes/SKILL.md");
  window.__ANSWER__.skill_install_plan = record("skill_install_plan", (args) => ({ command: "npx skills add " + args.repository + " " + args.names.map((name) => "--skill " + name).join(" ") + " --agent codex --global", rows: 24, cols: 96, status: "prepared" }));
  window.__ANSWER__.open_term_tab = record("open_term_tab", 9101);
  window.__ANSWER__.term_text = record("term_text", null);
  window.__ANSWER__.term_key = record("term_key", null);
  window.__ANSWER__.library_server_login = record("library_server_login", "claude mcp login figma");
  window.__OPENED__ = [];
  window.openFile = async (path) => { window.__OPENED__.push(path); };
  skillsReport = null; libraryReport = null;
  if (skillsPending) await skillsPending;
  document.getElementById("nav-skills").click();
  await refreshSkills();
  await window.__PAINTED__();
  return true;
})`;

const view = () => `docHost(tabs.find((tab) => tab.kind === "skills").pane, "skills")`;

export async function testLibraryPage(browser, origin, ok) {
  const { openWindowTestPage } = await import("./window-boot.mjs");
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.evaluate(`${STAND}(${JSON.stringify(LIBRARY)}, ${JSON.stringify(SKILLS)})`);

    const first = await page.evaluate(`(() => {
      const v = ${view()};
      const visible = (part) => [...v.querySelectorAll('.lib-part[data-part="' + part + '"] .lib-row')].filter((row) => !row.hidden);
      return {
        title: v.querySelector(".lib-title").textContent,
        tabs: [...v.querySelectorAll(".lib-tab")].map((tab) => tab.textContent),
        chips: v.querySelectorAll(".lib-chip").length,
        dashed: [...v.querySelectorAll(".lib-chip.is-own-only")].length,
        rows: visible("skills").map((row) => row.querySelector(".lib-row-title strong").textContent + "=" + row.querySelector(".lib-status").textContent),
        sync: v.querySelector(".lib-sync").textContent,
      };
    })()`);
    ok("the page is the common store's: 스킬·MCP over its agents and three tabs",
      first.title === "스킬·MCP" && first.tabs.length === 3 && first.chips === 4 && first.dashed === 1 && first.sync.includes("자동 공통화"),
      JSON.stringify(first));
    ok("each skill row says who sees it in words — every agent through the store, one agent's own, a plugin's",
      first.rows.includes("pdf=에이전트 3/4") && first.rows.includes("notes=고르기 필요") && first.rows.some((row) => row.startsWith("kiro-only=에이전트"))
        && first.rows.includes("docs=플러그인에서 관리"),
      JSON.stringify(first.rows));

    /* ---- MCP: rows, the question, the switch ---------------------------------------------- */
    const mcp = await page.evaluate(`(async () => {
      const v = ${view()};
      v.querySelector("#lib-tab-mcp").click();
      await window.__PAINTED__();
      const rows = [...v.querySelectorAll('.lib-part[data-part="mcp"] .lib-row')].filter((row) => !row.hidden);
      const said = rows.map((row) => row.dataset.key + "=" + row.querySelector(".lib-status").textContent);
      const attention = v.querySelector(".lib-attention").textContent;
      const before = window.__LIB_CALLS__.length;
      rows.find((row) => row.dataset.key === "rea").querySelector(".lib-switch").click();
      await new Promise((resolve) => setTimeout(resolve, 30));
      return { said, attention, switched: window.__LIB_CALLS__.slice(before) };
    })()`);
    ok("each server row says its state: every agent, some, one agent's own, a question",
      mcp.said.includes("rea=에이전트 2/3") && mcp.said.includes("context7=고르기 필요") && mcp.said.includes("node_repl=Codex 전용")
        && mcp.said.includes("figma=에이전트 2/3") && mcp.said.includes("kept=Codex 및 Gemini CLI 전용"),
      JSON.stringify(mcp.said));
    ok("what waits for the person stands above the list: the server to choose and the list that cannot be read",
      mcp.attention.includes("context7") && mcp.attention.includes("Kiro"), mcp.attention);
    ok("a row's switch turns the server off for every agent",
      mcp.switched.some((call) => call.command === "library_server_switch" && call.args.name === "rea" && call.args.on === false),
      JSON.stringify(mcp.switched));

    const detail = await page.evaluate(`(async () => {
      const v = ${view()};
      selectLibraryServer(v, skillsViews.get(v), "rea");
      await window.__PAINTED__();
      const host = v.querySelector(".skills-detail-content");
      const agents = [...host.querySelectorAll(".lib-agent-row")].map((row) => row.querySelector(".lib-agent-name").textContent + "=" + row.querySelector(".lib-status").textContent);
      const before = window.__LIB_CALLS__.length;
      host.querySelectorAll(".lib-agent-row .lib-switch")[1].click();
      await new Promise((resolve) => setTimeout(resolve, 30));
      const switched = window.__LIB_CALLS__.slice(before);
      const check = [...host.querySelectorAll(".lib-actions button")].find((button) => button.textContent.includes("연결 확인"));
      check.click();
      await new Promise((resolve) => setTimeout(resolve, 60));
      return { text: host.textContent, agents, switched, checked: v.querySelector(".lib-check")?.textContent ?? "" };
    })()`);
    ok("a server's detail names its values and never holds one", detail.text.includes("REA_TOKEN") && detail.text.includes("••••••") && detail.text.includes("rea mcp"), detail.text.slice(0, 400));
    ok("the detail says, agent by agent, whether each sees the server",
      detail.agents.includes("Claude Code=봄") && detail.agents.includes("Gemini CLI=맞추는 중") && detail.agents.includes("Kiro=설정을 못 읽음"),
      JSON.stringify(detail.agents));
    ok("an agent's switch sends that agent alone", detail.switched.some((call) => call.command === "library_server_agent" && call.args.name === "rea" && call.args.agent === "codex" && call.args.on === false), JSON.stringify(detail.switched));
    ok("「연결 확인」 says what the agent found", detail.checked.includes("연결됨"), detail.checked);

    const pick = await page.evaluate(`(async () => {
      const v = ${view()};
      selectLibraryServer(v, skillsViews.get(v), "context7");
      await window.__PAINTED__();
      const host = v.querySelector(".skills-detail-content");
      const options = [...host.querySelectorAll('.lib-pick input[type="radio"]')];
      options[1].checked = true;
      const before = window.__LIB_CALLS__.length;
      [...host.querySelectorAll("button")].find((button) => button.textContent === "이것으로 맞추기").click();
      await new Promise((resolve) => setTimeout(resolve, 30));
      selectLibraryServer(v, skillsViews.get(v), "node_repl");
      await window.__PAINTED__();
      const own = v.querySelector(".skills-detail-content").textContent;
      selectLibraryServer(v, skillsViews.get(v), "kept");
      await window.__PAINTED__();
      const kept = v.querySelector(".skills-detail-content").textContent;
      return { options: options.length, chose: window.__LIB_CALLS__.slice(before), own, kept };
    })()`);
    ok("a question is answered by choosing one version, and the choice is what is sent",
      pick.options === 2 && pick.chose.some((call) => call.command === "library_server_choose" && call.args.fingerprint === "fp-remote"), JSON.stringify(pick.chose));
    ok("a server bound to one agent says so and offers to share it", pick.own.includes("Codex 전용") && pick.own.includes("모두에게 공유"), pick.own.slice(0, 300));
    ok("a server the person kept with several agents names them in the language's list form and says the person kept it (second review t-99089 P3-F)",
      pick.kept.includes("Codex 및 Gemini CLI 전용") && pick.kept.includes("되돌려") && !pick.kept.includes("자기 기능으로"), pick.kept.slice(0, 300));

    /* ---- the add dialog: a pasted block, then a typed command line ------------------------- */
    const added = await page.evaluate(`(async () => {
      const v = ${view()};
      v.querySelector(".lib-add").click();
      const dialog = v.querySelector(".lib-dialog");
      const modal = dialog?.getAttribute("aria-modal") === "true";
      [...dialog.querySelectorAll(".lib-seg")].find((button) => button.textContent === "JSON 붙여넣기").click();
      const json = dialog.querySelector("textarea.lib-json");
      json.value = '{"mcpServers":{"github":{"command":"npx","args":["-y","@modelcontextprotocol/server-github"],"env":{"GITHUB_TOKEN":"x"}}}}';
      json.dispatchEvent(new Event("input"));
      await new Promise((resolve) => setTimeout(resolve, 400));
      const read = dialog.querySelector(".lib-result").textContent;
      const before = window.__LIB_CALLS__.length;
      [...dialog.querySelectorAll(".lib-dialog-foot button")].at(-1).click();
      await new Promise((resolve) => setTimeout(resolve, 60));
      const pasted = window.__LIB_CALLS__.slice(before);
      const closed = v.querySelector(".lib-scrim").hidden;

      v.querySelector(".lib-add").click();
      const form = v.querySelector(".lib-dialog");
      const inputs = form.querySelectorAll("input.lib-input");
      inputs[0].value = "files";
      inputs[1].value = 'npx -y "@acme/files mcp" --root \\'/Users/dev/My Files\\'';
      [...form.querySelectorAll(".lib-dialog-body .btn")].find((button) => button.textContent.includes("더하기")).click();
      const pair = form.querySelectorAll(".lib-secret-row input");
      pair[0].value = "FILES_TOKEN"; pair[1].value = "v";
      const typedBefore = window.__LIB_CALLS__.length;
      [...form.querySelectorAll(".lib-dialog-foot button")].at(-1).click();
      await new Promise((resolve) => setTimeout(resolve, 60));
      return { modal, read, pasted, closed, typed: window.__LIB_CALLS__.slice(typedBefore) };
    })()`);
    ok("the add dialog reads a pasted README block before adding it", added.modal && added.read.includes("github") && added.read.includes("GITHUB_TOKEN") && !added.read.includes('"x"'), added.read);
    ok("adding the pasted block sends it whole, and the dialog closes", added.pasted.some((call) => call.command === "library_paste_add" && call.args.text.includes("server-github")) && added.closed, JSON.stringify(added.pasted));
    const saved = added.typed.find((call) => call.command === "library_server_save");
    ok("a typed command line keeps its quoted words, and its values travel to the keeper alone",
      saved?.args.name === "files" && saved.args.server.type === "stdio" && saved.args.server.command === "npx"
        && JSON.stringify(saved.args.server.args) === JSON.stringify(["-y", "@acme/files mcp", "--root", "/Users/dev/My Files"])
        && saved.args.server.env.FILES_TOKEN === "v",
      JSON.stringify(added.typed));

    /* ---- skills: a repository typed into a terminal, a folder, a new one ------------------- */
    const skillAdds = await page.evaluate(`(async () => {
      const v = ${view()};
      v.querySelector("#lib-tab-skills").click();
      await window.__PAINTED__();
      const wait = () => new Promise((resolve) => setTimeout(resolve, 60));
      const open = (from) => {
        v.querySelector(".lib-add").click();
        const dialog = v.querySelector(".lib-dialog");
        [...dialog.querySelectorAll(".lib-seg")].find((button) => button.textContent === from).click();
        return dialog;
      };
      const submit = (dialog) => [...dialog.querySelectorAll(".lib-dialog-foot button")].at(-1);

      const repo = open("저장소 주소");
      const repoInput = [...repo.querySelectorAll("input.lib-input")].find((input) => input.placeholder.startsWith("https://"));
      repoInput.value = "https://github.com/acme/agent-skills";
      repoInput.dispatchEvent(new Event("input"));
      [...repo.querySelectorAll("button")].find((button) => button.textContent.includes("목록 읽기")).click();
      await wait();
      const listed = [...repo.querySelectorAll(".lib-check-row input")].map((input) => input.value);
      repo.querySelectorAll(".lib-check-row input")[1].click();
      const label = submit(repo).textContent;
      const note = repo.textContent.includes("Enter는 직접 누릅니다");
      let before = window.__LIB_CALLS__.length;
      submit(repo).click();
      await wait(); await wait();
      const repoCalls = window.__LIB_CALLS__.slice(before);
      // The typed command stands in a terminal tab of its own; back to the page.
      const terminalShown = tabs.some((tab) => tab.kind !== "skills" && tab.id === activeTabId);
      openSkillsView();
      await window.__PAINTED__();

      const folder = open("폴더");
      const folderInput = [...folder.querySelectorAll("input.lib-input")].find((input) => input.placeholder.includes("Downloads"));
      folderInput.value = "/Users/dev/Downloads/my-skill";
      before = window.__LIB_CALLS__.length;
      submit(folder).click();
      await wait();
      const folderCalls = window.__LIB_CALLS__.slice(before);

      const made = open("새로 만들기");
      const inputs = [...made.querySelectorAll("input.lib-input")];
      const name = inputs.find((input) => input.placeholder === "release-notes");
      const about = inputs.find((input) => input.placeholder.startsWith("언제 쓰는"));
      name.value = "release-notes"; about.value = "Write the notes for a release";
      before = window.__LIB_CALLS__.length;
      submit(made).click();
      await wait(); await wait();
      const madeCalls = window.__LIB_CALLS__.slice(before);
      return { listed, label, note, repoCalls, terminalShown, folderCalls, madeCalls, opened: window.__OPENED__.slice(), closed: v.querySelector(".lib-scrim").hidden };
    })()`);
    const plan = skillAdds.repoCalls.find((call) => call.command === "skill_install_plan");
    const typed = skillAdds.repoCalls.find((call) => call.command === "term_text");
    ok("a repository's skills are listed, the chosen ones typed into a plain terminal, and the Enter left to the person",
      JSON.stringify(skillAdds.listed) === JSON.stringify(["release-notes", "code-review"])
        && skillAdds.label === "터미널에서 설치" && skillAdds.note
        && JSON.stringify(plan?.args.names) === JSON.stringify(["release-notes"]) && plan?.args.repository === "https://github.com/acme/agent-skills"
        && skillAdds.repoCalls.some((call) => call.command === "open_term_tab" && call.args.plain === true)
        && typed?.args.text.startsWith("npx skills add https://github.com/acme/agent-skills --skill release-notes") && !/[\r\n]/.test(typed?.args.text ?? "")
        && !skillAdds.repoCalls.some((call) => call.command === "term_key") && skillAdds.terminalShown,
      JSON.stringify(skillAdds.repoCalls));
    ok("a folder is copied into the store by its path",
      skillAdds.folderCalls.some((call) => call.command === "library_skill_import" && call.args.path === "/Users/dev/Downloads/my-skill"), JSON.stringify(skillAdds.folderCalls));
    ok("a new skill is made in the store and its SKILL.md opened for writing",
      skillAdds.madeCalls.some((call) => call.command === "library_skill_create" && call.args.name === "release-notes" && call.args.description === "Write the notes for a release")
        && skillAdds.opened.includes("/Users/dev/.agents/skills/release-notes/SKILL.md") && skillAdds.closed,
      JSON.stringify(skillAdds));

    /* ---- a remote server's sign-in is typed too, and the person's alone ------------------- */
    const login = await page.evaluate(`(async () => {
      const v = ${view()};
      selectLibraryServer(v, skillsViews.get(v), "figma");
      await window.__PAINTED__();
      const button = [...v.querySelectorAll(".skills-detail-content .lib-agent-row button")].find((one) => one.textContent === "로그인");
      const before = window.__LIB_CALLS__.length;
      button?.click();
      await new Promise((resolve) => setTimeout(resolve, 120));
      const calls = window.__LIB_CALLS__.slice(before);
      // The sign-in stands in a terminal tab of its own; back to the page.
      openSkillsView();
      await window.__PAINTED__();
      return { marked: button?.dataset.personOnly === "", calls };
    })()`);
    ok("a sign-in is marked the person's, asks for the agent's own command, and types it without Enter",
      login.marked && login.calls.some((call) => call.command === "library_server_login" && call.args.name === "figma" && call.args.agent === "claude")
        && login.calls.some((call) => call.command === "term_text" && call.args.text === "claude mcp login figma")
        && !login.calls.some((call) => call.command === "term_key"),
      JSON.stringify(login));

    /* ---- deletes ask first and name what goes -------------------------------------------- */
    const deleted = await page.evaluate(`(async () => {
      const v = ${view()};
      selectLibraryServer(v, skillsViews.get(v), "figma");
      await window.__PAINTED__();
      [...v.querySelectorAll(".skills-detail-content .lib-actions button")].find((button) => button.textContent.includes("삭제")).click();
      const dialog = v.querySelector(".lib-dialog");
      const said = dialog.textContent;
      const focused = document.activeElement?.textContent;
      const marked = dialog.dataset.personOnly === "";
      const before = window.__LIB_CALLS__.length;
      [...dialog.querySelectorAll(".lib-dialog-foot button")].at(-1).click();
      await new Promise((resolve) => setTimeout(resolve, 60));
      return { said, focused, marked, calls: window.__LIB_CALLS__.slice(before) };
    })()`);
    ok("deleting a server asks first, names every list it leaves, and starts on 「취소」",
      deleted.said.includes("figma") && deleted.said.includes("mcp.json") && deleted.said.includes("Claude Code") && deleted.focused === "취소" && deleted.marked
        && deleted.calls.some((call) => call.command === "library_server_remove" && call.args.name === "figma"),
      JSON.stringify(deleted));

    const skill = await page.evaluate(`(async () => {
      const v = ${view()};
      v.querySelector("#lib-tab-skills").click();
      await window.__PAINTED__();
      await selectSkillFamily(v, "family-pdf");
      const host = v.querySelector(".skills-detail-content");
      const body = host.querySelector(".skills-markdown h1")?.textContent;
      const seen = [...host.querySelectorAll(".lib-agent-row")].map((row) => row.querySelector(".lib-agent-name").textContent + "=" + row.querySelector(".lib-status").textContent);
      [...host.querySelectorAll(".lib-actions button")].find((button) => button.textContent.includes("삭제")).click();
      const dialog = v.querySelector(".lib-dialog");
      const said = dialog.textContent;
      const before = window.__LIB_CALLS__.length;
      [...dialog.querySelectorAll(".lib-dialog-foot button")].at(-1).click();
      await new Promise((resolve) => setTimeout(resolve, 60));
      return { body, seen, said, calls: window.__LIB_CALLS__.slice(before) };
    })()`);
    ok("a store skill's detail reads its SKILL.md and says which agents see it",
      skill.body === "Body" && skill.seen.includes("Codex=봄") && skill.seen.includes("Kiro=못 봄"), JSON.stringify(skill));
    ok("deleting a skill sends it to the Trash after asking", skill.said.includes("휴지통") && skill.calls.some((call) => call.command === "library_skill_remove" && call.args.name === "pdf" && call.args.copies === false), JSON.stringify(skill.calls));

    /* ---- 최근 변경 ----------------------------------------------------------------------- */
    const log = await page.evaluate(`(async () => {
      const v = ${view()};
      v.querySelector("#lib-tab-log").click();
      await window.__PAINTED__();
      const rows = [...v.querySelectorAll('.lib-part[data-part="log"] .lib-log-row')].map((row) => row.textContent);
      const before = window.__LIB_CALLS__.length;
      [...v.querySelectorAll('.lib-part[data-part="log"] button')].find((button) => button.textContent.includes("되돌리기")).click();
      await new Promise((resolve) => setTimeout(resolve, 60));
      return { rows, undone: window.__LIB_CALLS__.slice(before) };
    })()`);
    ok("「최근 변경」 says what happened in words, a refusal included",
      log.rows.some((row) => row.includes("처음 합침") && row.includes("34")) && log.rows.some((row) => row.includes("Codex") && row.includes("pdf"))
        && log.rows.some((row) => row.includes("제때 끝나지 않았습니다")),
      JSON.stringify(log.rows));
    ok("a line that can be undone is, by its id", log.undone.some((call) => call.command === "library_undo" && call.args.id === 102), JSON.stringify(log.undone));

    /* ---- a narrow pane: the detail is its own page ----------------------------------------- */
    await page.setViewportSize({ width: 560, height: 820 });
    const narrow = await page.evaluate(`(async () => {
      const v = ${view()};
      v.querySelector("#lib-tab-mcp").click();
      await window.__PAINTED__();
      const detail = v.querySelector(".lib-detail");
      // Closing slides out first: visibility follows once the slide is done.
      await new Promise((resolve) => setTimeout(resolve, 260));
      const closed = getComputedStyle(detail).visibility;
      v.querySelector('.lib-part[data-part="mcp"] .lib-row .lib-row-open').click();
      await new Promise((resolve) => setTimeout(resolve, 260));
      const opened = getComputedStyle(detail).visibility;
      const back = detail.querySelector(".lib-back");
      const backShown = back && getComputedStyle(back).display !== "none";
      back.click();
      await new Promise((resolve) => setTimeout(resolve, 260));
      return { closed, opened, backShown, after: getComputedStyle(detail).visibility, overflow: v.scrollWidth > v.clientWidth + 1 };
    })()`);
    ok("in a narrow pane the detail opens over the list and 「목록」 brings the list back",
      narrow.closed === "hidden" && narrow.opened === "visible" && narrow.backShown && narrow.after === "hidden" && !narrow.overflow, JSON.stringify(narrow));

    ok("the Skills·MCP page raised no errors", faults.length === 0, faults.slice(0, 3).join(" | "));

    /* `CAPTURE_LIBRARY=<folder>`: the page as the person sees it, for a look
     * by eye — both themes, a wide and a narrow window, each tab. */
    const capture = process.env.CAPTURE_LIBRARY;
    if (capture) {
      for (const [width, height] of [[1440, 900], [720, 860]]) {
        await page.setViewportSize({ width, height });
        for (const theme of ["dark", "light"]) {
          for (const [tab, pick] of [["skills", "family-pdf"], ["mcp", "rea"], ["log", null]]) {
            await page.evaluate(`(async () => {
              document.documentElement.dataset.theme = ${JSON.stringify(theme)};
              openSkillsView();
              const v = ${view()};
              const state = skillsViews.get(v);
              v.querySelector("#lib-tab-${tab}").click();
              await window.__PAINTED__();
              ${JSON.stringify(pick)} && (${JSON.stringify(tab)} === "mcp" ? selectLibraryServer(v, state, ${JSON.stringify(pick)}) : await selectSkillFamily(v, ${JSON.stringify(pick)}));
              await window.__PAINTED__();
            })()`);
            await page.waitForTimeout(300);
            await page.screenshot({ path: `${capture}/library-${tab}-${width}-${theme}.png` });
          }
          for (const dialog of ["add", "delete"]) {
            await page.evaluate(`(async () => {
              const v = ${view()};
              const state = skillsViews.get(v);
              v.querySelector("#lib-tab-mcp").click();
              await window.__PAINTED__();
              if (${JSON.stringify(dialog)} === "add") v.querySelector(".lib-add").click();
              else confirmLibraryDelete(v, state, { server: libraryReport.servers.find((one) => one.name === "figma") });
            })()`);
            await page.waitForTimeout(300);
            await page.screenshot({ path: `${capture}/library-dialog-${dialog}-${width}-${theme}.png` });
            await page.evaluate(`closeLibraryDialog(${view()})`);
          }
        }
      }
    }
  } finally {
    await page.close();
  }
}

if (process.argv[1] && import.meta.url === (await import("node:url")).pathToFileURL(process.argv[1]).href) {
  const { chromium, createWindowServer } = await import("./window-boot.mjs");
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch();
  try {
    await testLibraryPage(browser, origin, (name, pass, detail) => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name} ${pass ? "" : detail}`);
      if (!pass) process.exitCode = 1;
    });
  } finally { await browser.close(); files.close(); }
}
