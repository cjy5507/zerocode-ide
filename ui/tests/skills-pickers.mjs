/* ---- the Skills screen's two filter pickers show their whole label (t-58297) --
 *
 * 「모든 위치」 lost its last letter and 「모든 에이전트」 lost its last two under
 * the dropdown arrow: the screen's own `padding` shorthand out-specified the
 * room every select reserves for the mark. Measured in a real layout, in every
 * language, in both themes, at the window's default width and at the narrowest
 * width it opens at (720 px): the selected label's right edge sits left of the
 * arrow's left edge. No pixel width is tied to one language's words — the label
 * is measured in the font the select draws it in. */
const LOCALES = ["ko", "en", "ja", "zh", "es"];
const THEMES = ["dark", "light"];
const WIDTHS = [1280, 720];
const PICKERS = [".skills-root-filter", ".skills-agent-filter"];

const REPORT = {
  families: [{
    id: "family-0", name: "Release review",
    variants: [{
      id: "copy-0", name: "Release review", description: "Review a release.",
      file: "/home/skills/review/SKILL.md", directory: "/home/skills/review",
      source_id: "home-codex", source_label: "Codex home", source_kind: "home",
      providers: ["claude"], version: "1.0.0", digest: "d".repeat(64), roots: [], files: 1, updated_at: 1,
    }],
    conflict: false, latest_version: "1.0.0", usage: { count_7d: 0, last_used_ms: null, agents: [] },
  }],
  sources: [{ id: "home-codex", path: "/home/.codex/skills", kind: "home", found: 1, exists: true }],
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
  const probe = document.createElement('div');
  probe.style.cssText = 'position:absolute;visibility:hidden;width:var(--space-1)';
  document.body.append(probe);
  const markGap = probe.getBoundingClientRect().width;
  probe.remove();
  const canvas = document.createElement('canvas').getContext('2d');
  return ${JSON.stringify(PICKERS)}.map((selector) => {
    const select = view.querySelector(selector);
    const box = select.getBoundingClientRect();
    const style = getComputedStyle(select);
    const label = select.selectedOptions[0]?.textContent ?? '';
    canvas.font = style.font;
    const labelRight = box.left + parseFloat(style.borderLeftWidth) + parseFloat(style.paddingLeft) + canvas.measureText(label).width;
    const arrowLeft = box.right - parseFloat(style.borderRightWidth) - markGap - 16;
    return { selector, label, labelRight, arrowLeft, width: box.width, shown: box.width > 0 };
  });
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
          for (const row of rows) {
            const gap = row.arrowLeft - row.labelRight;
            console.log(`MEASURE ${locale} ${theme} ${width}px ${row.selector} 「${row.label}」 gap=${gap.toFixed(1)}px`);
            ok(
              `${row.selector} draws 「${row.label}」 left of its arrow (${locale}, ${theme}, ${width}px)`,
              row.shown && row.label !== "" && gap >= 0,
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
      console.log(`${pass ? "PASS" : "FAIL"} ${name} ${pass ? "" : detail}`);
      if (!pass) process.exitCode = 1;
    });
  } finally { await browser.close(); files.close(); }
}
