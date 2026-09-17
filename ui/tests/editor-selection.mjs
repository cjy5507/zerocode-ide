import { mkdir } from "node:fs/promises";
import { openWindowTestPage } from "./window-boot.mjs";

/* The markdown editor's selection is the terminal's own highlight.
 *
 * Selecting prose in a `.md` file must look like selecting it in Codex CLI
 * running in the terminal: the SAME solid pair the terminal paints, not a
 * washed-out accent tint (live report 2026-09-09 — the lavender 30% mix left
 * selected text unreadable in dark mode). CodeMirror paints a selection in two
 * places at once: its own `.cm-selectionBackground` layer while focused, and
 * the browser's `::selection` over the glyphs. The layer is a real element, so
 * its colour is read as a computed style; the `::selection` rule is read from
 * the CSSOM, because a pseudo-element has no computed style to ask.
 *
 * Legibility is asserted as a NUMBER, not a look: the pair the editor wears
 * must clear WCAG AA for body text (4.5:1) in each theme. The solid fill on
 * its own fails that for nearly every syntax colour (function 2.06, comment
 * 1.44 on #5a7898), so the foreground the `::selection` rule carries is
 * load-bearing — this is the pin that says so. */

const AA_BODY_TEXT = 4.5;

// Runs INSIDE the page (`page.evaluate`), so nothing from this module's scope
// is visible here — the AA threshold rides in as the argument.
export async function exerciseEditorSelection(aaBodyText) {
  const paint = (value) => {
    const probe = document.createElement("span");
    probe.style.color = value;
    probe.style.backgroundColor = value;
    document.body.appendChild(probe);
    const seen = getComputedStyle(probe);
    const answer = { color: seen.color, backgroundColor: seen.backgroundColor };
    probe.remove();
    return answer;
  };
  const token = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  const settle = () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
  // WCAG relative luminance from a computed `rgb(...)` / `rgba(...)` string.
  const luminance = (rgb) => {
    const [r, g, b] = rgb.match(/[\d.]+/g).slice(0, 3).map((part) => Number(part) / 255);
    const lin = (c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
    return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
  };
  const contrast = (a, b) => {
    const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
    return (hi + 0.05) / (lo + 0.05);
  };
  // The `::selection` rule CodeMirror injected for this editor, read from the
  // CSSOM. Its `color` is the authored value, so the pin is the token's name.
  const selectionRule = () => {
    for (const sheet of document.styleSheets) {
      let rules;
      try { rules = sheet.cssRules; } catch { continue; }
      for (const rule of rules) {
        if (rule.selectorText?.includes(".cm-content ::selection")) return rule;
      }
    }
    return null;
  };

  const heldRead = window.__ANSWER__.read_text_file;
  const heldVersion = window.__ANSWER__.file_version;
  window.__ANSWER__.file_version = () => "v0";
  window.__ANSWER__.read_text_file = () => ({
    text: "# 제목\n\napp 에서 추가되어야 하는 것\neasy care 한국사람 차단\n\n학생 비자 = 학생은 가입 불가\n",
    version: "v0",
  });
  await openFile("/w/repo/guide.md");
  const tab = tabs.find((one) => one.id === "file:/w/repo/guide.md");
  tab.raw = true;
  paintFileView(tab);
  const editor = editorShowing(tab);
  const view = [...document.querySelectorAll(".file-view")].find((one) => one.dataset.tab === tab.id);
  const host = view.querySelector(".file-edit");
  editor.focus();
  editor.dispatch({ selection: { anchor: 0, head: editor.state.doc.length } });
  await settle();

  const readTheme = (theme) => {
    const wantBg = paint(token("--term-selection-bg")).backgroundColor;
    const wantFg = paint(token("--term-selection-fg")).color;
    const layer = host.querySelector(".cm-selectionBackground");
    const layerBg = layer ? getComputedStyle(layer).backgroundColor : null;
    const rule = selectionRule();
    const ruleColor = rule?.style.color ?? null;
    const ruleBg = rule?.style.backgroundColor ?? null;
    return {
      theme,
      wantBg,
      wantFg,
      layerBg,
      layerWearsTerminalBg: layerBg === wantBg,
      ruleColor,
      ruleWearsTerminalFg: typeof ruleColor === "string" && ruleColor.includes("--term-selection-fg"),
      ruleBgWearsEditorToken: typeof ruleBg === "string" && ruleBg.includes("--editor-selection"),
      contrast: Number(contrast(wantFg, wantBg).toFixed(2)),
      legible: contrast(wantFg, wantBg) >= aaBodyText,
      editorRect: host.getBoundingClientRect().toJSON(),
    };
  };

  // The theme is switched the way the window switches it — `setTheme` runs
  // `applyTheme`, which stamps `data-theme` AND re-applies the terminal
  // palette (its colour tokens are written inline on the root, so a bare
  // attribute flip would leave them standing). The terminal keeps its dark
  // palette in a light window unless `use_separate_light_theme` is on, so the
  // token need not CHANGE across the flip; what must hold in every state is
  // that the editor wears whatever the terminal resolves to.
  const root = document.documentElement;
  const startedTheme = theme;
  const current = readTheme(root.dataset.theme ?? "dark");
  setTheme(startedTheme === "light" ? "dark" : "light");
  await settle();
  const flipped = readTheme(root.dataset.theme ?? "dark");
  setTheme(startedTheme);
  await settle();

  window.__ANSWER__.read_text_file = heldRead;
  window.__ANSWER__.file_version = heldVersion;
  return { current, flipped, tokensDiffer: current.wantBg !== flipped.wantBg };
}

export async function testEditorSelection(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const seen = await page.evaluate(exerciseEditorSelection, AA_BODY_TEXT);
    const both = [seen.current, seen.flipped];
    ok(
      "the markdown editor's selection layer wears whatever the terminal's selection colour resolves to, in both themes",
      both.every((one) => one.layerWearsTerminalBg) && seen.current.theme !== seen.flipped.theme,
      JSON.stringify({ current: seen.current, flipped: seen.flipped, tokensDiffer: seen.tokensDiffer }),
    );
    ok(
      "the editor's ::selection rule carries the terminal's selection foreground over the editor token",
      both.every((one) => one.ruleWearsTerminalFg && one.ruleBgWearsEditorToken),
      JSON.stringify(both.map((one) => ({ theme: one.theme, ruleColor: one.ruleColor }))),
    );
    ok(
      `the selection pair clears WCAG AA body text (${AA_BODY_TEXT}:1) in both themes`,
      both.every((one) => one.legible),
      JSON.stringify(both.map((one) => ({ theme: one.theme, contrast: one.contrast, fg: one.wantFg, bg: one.wantBg }))),
    );
    // Evidence, on request: the painted editor with everything selected.
    if (process.env.EDITOR_SELECTION_SHOTS) {
      const dir = process.env.EDITOR_SELECTION_SHOTS;
      await mkdir(dir, { recursive: true });
      const rect = seen.current.editorRect;
      await page.screenshot({
        path: `${dir}/editor-selection-${seen.current.theme}.png`,
        clip: { x: rect.x, y: rect.y, width: Math.min(rect.width, 900), height: Math.min(rect.height, 320) },
      });
    }
    ok("the editor selection probe raised no renderer errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}
