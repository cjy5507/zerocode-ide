import assert from "node:assert/strict";

export async function testVaultSubagents(page) {
  const seen = await page.evaluate(async () => {
    vaultChildrenOpen.clear();
    vaultDockOpen.clear();
    locale = "ko";
    const parent = {
      id: "parent", agent: "claude", session_id: "parent", title: "볼트 세션과 서브에이전트",
      cwd: activeWorktreePath, branch: "vault-rows", model: null,
      file_path: "/tmp/parent.jsonl", modified_at: new Date().toISOString(),
      message_count: 1, preview: [{ role: "user", text: "세션 한도와 자식 행을 확인합니다" }],
      resume: "claude --resume parent", children: [], depth: 0,
    };
    parent.children = [{ ...parent, id: "child", session_id: "child", title: "자식 미리보기",
      preview: [{ role: "assistant", text: "기록된 출처를 확인했습니다" }], children: [],
      depth: 1, parent_id: parent.id, resume: null,
      origin: { pane: "term-12", worker: "w-34", tool_call_id: "call-56" } }];
    window.__VAULT__ = { groups: [{ key: "fixture", label: "Vault 검증", sessions: [parent] }],
      agents: [{ slug: "claude", sessions: 1 }], issues: [], shown: 50, total: 250,
      matched: 250, truncated: true, limits: { choices: [50, 100, 200], absolute_max: 1000, default: 200, children_per_parent: 50 } };
    window.__VAULT_LIMIT_ROWS__ = [parent, ...Array.from({ length: 249 }, (_, n) => ({
      ...parent, id: `parent-${n}`, session_id: `parent-${n}`, title: `세션 ${n + 2}`, children: [],
    }))];
    vaultSessionLimit = 50;
    document.getElementById("activity-vault-tab").click();
    await refreshVaultDock();
    await new Promise(done => setTimeout(done, 50));
    const toggle = document.querySelector(".vault-subagents-toggle");
    const keys = Object.keys(CATALOG.en).filter(key => key.startsWith("vaultDock.subagents") || key.startsWith("vaultDock.limit"));
    const catalogs = ["en", "ja", "zh", "es"].every(lang => keys.every(key => CATALOG[lang][key]));
    const seen = { beforeNotice: document.getElementById("vdock-sub").textContent, catalogs, toggle: toggle?.textContent, collapsed: toggle?.getAttribute("aria-expanded") };
    toggle?.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    const children = document.querySelector(".vault-subagents-list");
    seen.expanded = children && !children.hidden;
    seen.badges = [...(children?.querySelectorAll(".vault-origin") ?? [])].map(n => n.textContent);
    const child = children?.querySelector(".vdock-card");
    child?.click();
    const detail = document.querySelector(".vault-subagents-list .vdock-detail");
    seen.preview = detail?.textContent;
    seen.childButtons = [...(children?.querySelectorAll("button") ?? [])].map(n => n.textContent + n.getAttribute("aria-label"));
    // The stage uses its existing row renderer for the very same child model.
    const stage = vaultRow(parent);
    stage.querySelector(".vault-subagents-toggle")?.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    seen.stageChildren = stage.querySelectorAll(".vault-subagents-list .vault-row").length;
    seen.stageResume = [...stage.querySelectorAll(".vault-subagents-list button")].some(n => n.textContent === "다시 열기");
    openVaultDockView(300, 120, document.getElementById("vdock-view"));
    const item = [...document.querySelectorAll(".sidebar-menu-item")].find(n => n.textContent.includes("200개"));
    seen.limitOption = Boolean(item);
    item?.click();
    await new Promise(done => setTimeout(done, 100));
    seen.askedLimit = window.__ASKED_VAULT__?.query?.limit;
    seen.force = window.__ASKED_VAULT__?.force;
    seen.notice = document.getElementById("vdock-sub").textContent;
    seen.more = document.querySelector(".vault-limit-more")?.textContent;
    closeSidebarMenu();

    // Verify stage vault layout geometry and issue folding
    window.__VAULT__ = {
      groups: [{ key: "fixture", label: "Vault 검증", sessions: [parent] }],
      agents: [{ slug: "codex", sessions: 1 }],
      issues: [
        { agent: "codex", reason: "child-parent-missing", count: 1 },
        { agent: "codex", reason: "child-parent-missing", count: 1 },
        { agent: "codex", reason: "child-limit", count: 1 },
      ],
      shown: 1,
      total: 1,
      matched: 1,
      truncated: true,
      limits: { choices: [50, 100, 200], absolute_max: 1000, default: 200, children_per_parent: 50 },
    };
    openTab({ id: "vault", kind: "vault" });
    await refreshVault();
    const vaultViewEl = document.getElementById("vault-view");
    const headRect = vaultViewEl.querySelector(".vault-head").getBoundingClientRect();
    const noteRect = vaultViewEl.querySelector(".vault-note").getBoundingClientRect();
    const listRect = vaultViewEl.querySelector(".vault-list").getBoundingClientRect();
    const vaultViewStyle = getComputedStyle(vaultViewEl);
    const headStyle = getComputedStyle(vaultViewEl.querySelector(".vault-head"));
    const noteStyle = getComputedStyle(vaultViewEl.querySelector(".vault-note"));
    seen.debug = {
      vaultDisplay: vaultViewStyle.display,
      vaultFlexDirection: vaultViewStyle.flexDirection,
      headGridRow: headStyle.gridRow,
      noteGridRow: noteStyle.gridRow,
      vaultClass: vaultViewEl.className,
      headRect: { top: headRect.top, bottom: headRect.bottom, height: headRect.height },
      noteRect: { top: noteRect.top, bottom: noteRect.bottom, height: noteRect.height, hidden: vaultViewEl.querySelector(".vault-note").hidden },
      listRect: { top: listRect.top, bottom: listRect.bottom, height: listRect.height },
    };
    seen.stageGeometry = {
      headTop: headRect.top,
      noteTop: noteRect.top,
      listTop: listRect.top,
      noteText: vaultViewEl.querySelector(".vault-note").textContent,
    };
    closeTab(tabs.find(t => t.kind === "vault"));

    return seen;
  });
  assert.equal(seen.catalogs, true);
  assert.equal(seen.toggle, "서브에이전트 1");
  assert.equal(seen.collapsed, "false");
  assert.equal(seen.expanded, true);
  assert.deepEqual(seen.badges, ["판 term-12", "워커 w-34", "도구 call-56"]);
  assert.ok(seen.preview.includes("기록된 출처를 확인했습니다"));
  assert.ok(!seen.childButtons.some(n => n.includes("다시 열기")));
  assert.equal(seen.stageChildren, 1);
  assert.equal(seen.stageResume, false);
  assert.equal(seen.limitOption, true);
  assert.equal(seen.askedLimit, 200);
  assert.equal(seen.force, false);
  assert.equal(seen.beforeNotice, "250개 중 50개 표시");
  assert.equal(seen.notice, "250개 중 200개 표시");
  assert.equal(seen.more, "더 보기");
  assert.ok(seen.debug.headRect.bottom <= seen.debug.noteRect.top, `head bottom (${seen.debug.headRect.bottom}) must not overlap note top (${seen.debug.noteRect.top})`);
  assert.ok(seen.debug.noteRect.bottom <= seen.debug.listRect.top, `note bottom (${seen.debug.noteRect.bottom}) must not overlap list top (${seen.debug.listRect.top})`);
  assert.ok(seen.stageGeometry.noteText.includes("Codex: 부모 세션을 찾을 수 없습니다 (2)"), "child-parent-missing issues should be folded with count 2");
  assert.ok(seen.stageGeometry.noteText.includes("Codex: 부모당 서브에이전트 한도에 도달했습니다 (1)"), "child-limit issue should have count 1");
  assert.ok(seen.stageGeometry.noteText.includes("최근 세션만 보여줍니다"), "truncated notice should be included");
  await page.waitForTimeout(250);
  await page.emulateMedia({ reducedMotion: "reduce" });
  for (const theme of ["dark", "light"]) {
    await page.evaluate(theme => { document.documentElement.dataset.theme = theme; }, theme);
    const contrast = await page.evaluate(() => {
      const rgb = value => {
        const canvas = document.createElement("canvas");
        canvas.width = canvas.height = 1;
        const ctx = canvas.getContext("2d");
        ctx.fillStyle = value;
        ctx.fillRect(0, 0, 1, 1);
        return [...ctx.getImageData(0, 0, 1, 1).data].slice(0, 3);
      };
      const luminance = value => rgb(value).map(n => n / 255).map(n => n <= 0.04045 ? n / 12.92 : ((n + 0.055) / 1.055) ** 2.4)
        .reduce((sum, n, i) => sum + n * [0.2126, 0.7152, 0.0722][i], 0);
      return [".vault-origin", ".vault-subagents-toggle"].map(selector => {
        const style = getComputedStyle(document.querySelector(selector));
        const a = luminance(style.color), b = luminance(style.backgroundColor);
        return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
      });
    });
    assert.ok(contrast.every(ratio => ratio >= 4.5), `${theme} contrast: ${contrast}`);
    console.log(`VAULT_CONTRAST ${theme} ${JSON.stringify(contrast)}`);
    await page.screenshot({ path: `/tmp/t-2787-children-${theme}.png` });
  }
  await page.evaluate(() => openVaultDockView(300, 120, document.getElementById("vdock-view")));
  await page.screenshot({ path: "/tmp/t-2787-limit-menu.png" });
  await page.evaluate(() => {
    closeSidebarMenu();
    document.documentElement.dataset.theme = "dark";
    window.__VAULT__ = undefined;
    window.__VAULT_LIMIT_ROWS__ = undefined;
    vaultDockAnswer = null;
    vaultDockOpen.clear();
    vaultChildrenOpen.clear();
  });
  await page.emulateMedia({ reducedMotion: "no-preference" });
  console.log("PASS vault subagent rows, origin, keyboard, preview-only children and query limit");
}
