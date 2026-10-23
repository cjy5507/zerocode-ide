import { openWindowTestPage } from "./window-boot.mjs";

export async function testKeybindingText(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const results = await page.evaluate(async () => {
      zoAvailable = true;
      stageError = null;
      const results = [];
      for (const locale of ["ko", "en", "ja", "zh", "es"]) {
        await setLocale(locale);
        for (const bindings of [
          ["mod+<b>demo</b>"],
          ["mod+<img src=x onerror=alert(1)>"],
          ["mod+&lt;b&gt;demo&lt;/b&gt;"],
          ["mod+t", "mod+alt+t"],
          [],
          null,
        ]) {
          if (bindings === null) delete keybindingOverrides["terminal.newTab"];
          else keybindingOverrides["terminal.newTab"] = bindings;
          paintStagePlaceholder();
          const hint = optionalShortcutLabel("terminal.newTab");
          results.push({
            locale, bindings, hint,
            text: placeholder.querySelector("kbd")?.textContent,
            elements: placeholder.querySelector("kbd")?.children.length,
            injected: !!placeholder.querySelector("b, img, script"),
            unresolved: placeholder.textContent.includes("{{chord}}"),
          });
        }
      }
      return results;
    });
    for (const row of results) {
      ok(`empty stage treats the shortcut as text (${row.locale}, ${JSON.stringify(row.bindings)})`,
        !row.injected && !row.unresolved && row.elements === 0 &&
          (row.hint === null ? row.text === "＋" : row.text === row.hint),
        JSON.stringify(row));
    }
    ok("shortcut text scenes have no window errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}
