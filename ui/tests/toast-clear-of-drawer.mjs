/* ---- a toast never covers an open surface's button row (t-68117) ------------
 *
 * A save notice stands six seconds in the bottom-right corner — the same
 * corner a drawer or a dialog docks its buttons in. Measured in a real layout:
 * with a surface open and a toast up, no button of that surface may sit under
 * the toast's box, and the point at each button's centre must hit the button. */
const SCENES = [
  { name: "a dialog", open: "showModal(el('save-scrim')); return el('save-scrim')" },
  {
    name: "a right-docked drawer",
    open: `const d = document.createElement('aside'); d.className = 'jev-drawer'; d.id = 'scene-drawer';
      d.style.cssText = 'position:fixed;height:100vh;top:0;right:0;bottom:0;width:360px;z-index:40;display:flex;flex-direction:row;align-items:flex-end;justify-content:flex-end;gap:8px;padding:16px 16px 56px';
      for (const word of ['Cancel', 'Save']) { const b = document.createElement('button'); b.textContent = word; d.append(b); }
      document.body.append(d); return d`,
  },
  {
    // Where the window's drawers stand: deep in a leaf, beside the
    // conversations, out of the body's own children (t-84136).
    name: "a drawer docked inside the workbench",
    open: `const d = document.createElement('aside'); d.className = 'jev-drawer'; d.id = 'scene-drawer-deep';
      d.style.cssText = 'position:fixed;height:100vh;top:0;right:0;bottom:0;width:360px;z-index:40;display:flex;flex-direction:row;align-items:flex-end;justify-content:flex-end;gap:8px;padding:16px 16px 56px';
      for (const word of ['Cancel', 'Save']) { const b = document.createElement('button'); b.textContent = word; d.append(b); }
      const leaf = document.querySelector('#workbench .panes') ?? document.querySelector('#workbench');
      leaf.append(d); return d`,
  },
];

const MEASURE = (scene) => `(async () => {
  const root = (() => { ${scene.open} })();
  document.querySelectorAll('.toast').forEach((one) => one.remove());
  const note = toast('Saved', '');
  await window.__PAINTED__();
  const box = note.getBoundingClientRect();
  const buttons = [...root.querySelectorAll('button')].filter((b) => b.getClientRects().length > 0);
  const rows = buttons.map((b) => {
    const r = b.getBoundingClientRect();
    const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
    return {
      covered: !(r.right <= box.left || r.left >= box.right || r.bottom <= box.top || r.top >= box.bottom),
      reached: hit === b || b.contains(hit),
    };
  });
  return { buttons: rows.length, covered: rows.filter((r) => r.covered).length, unreachable: rows.filter((r) => !r.reached).length };
})()`;

export async function testToastClearOfDrawer(browser, origin, ok) {
  const { openWindowTestPage } = await import("./window-boot.mjs");
  for (const scene of SCENES) {
    const { page } = await openWindowTestPage(browser, origin, { faults: [] });
    try {
      await page.setViewportSize({ width: 1280, height: 800 });
      const seen = await page.evaluate(MEASURE(scene));
      console.log(`MEASURE ${scene.name}: buttons=${seen.buttons} covered=${seen.covered} unreachable=${seen.unreachable}`);
      ok(`${scene.name} has buttons to measure`, seen.buttons > 0, JSON.stringify(seen));
      ok(`a save toast covers no button of ${scene.name}`, seen.covered === 0 && seen.unreachable === 0, JSON.stringify(seen));
    } finally { await page.close(); }
  }
}

if (process.argv[1] && import.meta.url === (await import("node:url")).pathToFileURL(process.argv[1]).href) {
  const { chromium, createWindowServer } = await import("./window-boot.mjs");
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch();
  try {
    await testToastClearOfDrawer(browser, origin, (name, pass, detail) => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name} ${detail}`);
      if (!pass) process.exitCode = 1;
    });
  } finally { await browser.close(); files.close(); }
}
