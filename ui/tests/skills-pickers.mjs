/* ---- the Skills·MCP page's tabs and filters show their whole label (t-58297) --
 *
 * 「모든 위치」 once lost its last letter and 「모든 에이전트」 its last two under a
 * select's arrow. The page has no select there now (2026-11-06: three tabs and a
 * segmented filter), and the rule it was measured for still stands: every
 * label the page draws in its header row is whole — measured in a real layout,
 * in every language, in both themes, at the window's default width and at the
 * narrowest width it opens at (720 px). Nothing is tied to one language's
 * words: a label's own scroll width is compared with the room it was given. */
const LOCALES = ["ko", "en", "ja", "zh", "es"];
const THEMES = ["dark", "light"];
const WIDTHS = [1280, 720];
const LABELS = [".lib-tab", ".lib-filters .lib-seg", ".lib-add", ".skills-rescan", ".lib-sync"];

const REPORT = {
  families: [{
    id: "family-0", name: "Release review",
    variants: [{
      id: "copy-0", name: "Release review", description: "Review a release.",
      file: "/home/skills/review/SKILL.md", directory: "/home/skills/review",
      source_id: "home-agents", source_label: "Agent skills home", source_kind: "home",
      providers: ["agent-skills"], version: "1.0.0", digest: "d".repeat(64), roots: [], files: 1, updated_at: 1,
    }],
    conflict: false, latest_version: "1.0.0", usage: { count_7d: 0, last_used_ms: null, agents: [] },
  }],
  sources: [{ id: "home-agents", path: "/home/.agents/skills", kind: "home", found: 1, exists: true }],
  agents: [["claude", "Claude"]], external_agents: [["claude", "Claude"]], external_unavailable: [],
  required: [], policy: { terminal_rows: 24, terminal_cols: 96 }, reads: 1, capped: false,
  evidence_scope: "hooks-only", evidence_count: 0, plans: [],
};

const MEASURE = (report, locale, theme) => `(async () => {
  setLocale(${JSON.stringify(locale)}, { persist: false, refresh: false });
  document.documentElement.dataset.theme = ${JSON.stringify(theme)};
  setSettingsOpen(false);
  window.__ANSWER__.skills_list = () => (${JSON.stringify(report)});
  skillsReport = null;
  if (skillsPending) await skillsPending;
  document.getElementById('nav-skills').click();
  await refreshSkills();
  const view = docHost(tabs.find((tab) => tab.kind === 'skills').pane, 'skills');
  await window.__PAINTED__();
  return ${JSON.stringify(LABELS)}.flatMap((selector) => [...view.querySelectorAll(selector)].map((node) => {
    const box = node.getBoundingClientRect();
    return { selector, label: node.textContent.trim(), scroll: node.scrollWidth, room: node.clientWidth, shown: box.width > 0 && box.height > 0 };
  }));
})()`;

export async function testSkillsPickers(browser, origin, ok) {
  const { openWindowTestPage } = await import("./window-boot.mjs");
  for (const locale of LOCALES) {
    for (const width of WIDTHS) {
      const { page } = await openWindowTestPage(browser, origin, { faults: [] });
      try {
        await page.setViewportSize({ width, height: 860 });
        for (const theme of THEMES) {
          const rows = await page.evaluate(MEASURE(REPORT, locale, theme));
          ok(`the Skills·MCP header draws its labels (${locale}, ${theme}, ${width}px)`, rows.length >= 8, JSON.stringify(rows));
          for (const row of rows) {
            if (!row.shown) continue;
            console.log(`MEASURE ${locale} ${theme} ${width}px ${row.selector} 「${row.label}」 ${row.scroll}/${row.room}`);
            ok(
              `${row.selector} draws 「${row.label}」 whole (${locale}, ${theme}, ${width}px)`,
              row.label !== "" && row.scroll <= row.room + 1,
              JSON.stringify(row),
            );
          }
        }
      } finally { await page.close(); }
    }
  }
}

if (process.argv[1] && import.meta.url === (await import("node:url")).pathToFileURL(process.argv[1]).href) {
  const { chromium, createWindowServer } = await import("./window-boot.mjs");
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch();
  try {
    await testSkillsPickers(browser, origin, (name, pass, detail) => {
      if (!pass) console.log(`FAIL ${name} ${detail}`);
      if (!pass) process.exitCode = 1;
    });
    if (!process.exitCode) console.log("PASS skills-pickers");
  } finally { await browser.close(); files.close(); }
}
