/* The window gate's CDP Performance ledger + timeline, scoped to one pane.
 * All instrumentation lives in the harness; the shipped renderer has no probe. */
import assert from "node:assert/strict";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { writeFile } from "node:fs/promises";
import { run } from "../../fixtures/quality-baseline/window-tools.mjs";

export function percentile(values, percent) {
  assert(values.length, "paint trace has no samples");
  return [...values].sort((a, b) => a - b)[Math.ceil(values.length * percent / 100) - 1];
}

export function paintSamples(events) {
  const marks = events.filter(e => e.name.startsWith("baseline-pane-") && e.ph === "I");
  assert(marks.length, "missing pane submission marks");
  return marks.map((mark, index) => {
    const until = marks[index + 1]?.ts ?? Infinity;
    const paints = events.filter(e => e.name === "Paint" && e.ph === "X" && e.pid === mark.pid && e.tid === mark.tid && e.ts >= mark.ts && e.ts < until);
    assert(paints.length, `no engine paint for ${mark.name}`);
    return { pid: mark.pid, paint_ms: (Math.max(...paints.map(e => e.ts + e.dur)) - mark.ts) / 1000 };
  });
}

export async function paneProbe(page, text, root, table) {
  const limits = table.Limits;
  const cdp = await page.context().newCDPSession(page);
  const events = [];
  cdp.on("Tracing.dataCollected", ({ value }) => events.push(...value));
  await cdp.send("Performance.enable");
  const layouts = async () => (await cdp.send("Performance.getMetrics")).metrics.find(e => e.name === "LayoutCount").value;
  const setup = await page.evaluate(async () => {
    const held = window.__HOLD_POLLERS__;
    window.__HOLD_POLLERS__ = true;
    const term = await openTermTab({ placement: "tab" });
    await new Promise(done => requestAnimationFrame(() => requestAnimationFrame(done)));
    window.__BASELINE_PANE__ = { term };
    return { held };
  });
  let tracing = false;
  try {
    const before = await layouts();
    await cdp.send("Tracing.start", { categories: "devtools.timeline,blink.user_timing", transferMode: "ReportEvents" });
    tracing = true;
    const queue = await page.evaluate(async ({ text, limits }) => {
      const { term } = window.__BASELINE_PANE__;
      const view = termViews.get(term);
      const grid = view.gridSize();
      let pending = 0;
      let max = 0;
      for (let sample = 0; sample < limits.pane_samples; sample++) {
        performance.mark(`baseline-pane-${sample}`);
        const told = [];
        for (let frame = 0; frame < limits.pane_burst_frames; frame++) {
          pending++;
          max = Math.max(max, pending);
          const delta = {
            rows: [{ index: 0, cells: [...`${sample} ${frame} ${text}`].slice(0, grid.cols).map(ch => ({ ch })) }],
            scrolled_lines: 0, cursor: [0, 0], title: "", alt_screen: false,
            size: [grid.rows, grid.cols], full: sample === 0 && frame === 0,
            cursor_visible: false, mouse_tracking: "off", mouse_sgr: false,
            focus_reporting: false, bell: false, view_offset: 0, scrollback_len: 0,
          };
          told.push(window.__TERM_FEED__(term, delta));
        }
        // The burst is the window's to pull on its own cadence; the sample ends
        // once its last frame has landed and been painted.
        await Promise.all(told);
        await new Promise(done => requestAnimationFrame(() => requestAnimationFrame(done)));
        pending = 0;
      }
      return max;
    }, { text, limits });
    const after = await layouts();
    const completed = new Promise(done => cdp.once("Tracing.tracingComplete", done));
    await cdp.send("Tracing.end");
    await completed;
    tracing = false;
    const samples = paintSamples(events);
    assert.equal(samples.length, limits.pane_samples);
    const pid = samples[0].pid;
    assert(samples.every(sample => sample.pid === pid));
    await writeFile(join(root, "evidence/window-paint.json"), JSON.stringify({ samples, queue_max: queue, layouts: after - before, events }));
    const { stdout } = await run("python3", [fileURLToPath(new URL("../../fixtures/quality-baseline/heap.py", import.meta.url)), String(pid), join(root, "evidence/window-heap.json")]);
    const heap = JSON.parse(stdout);
    return { stream: { queue_max: queue, paint_p95_ms: percentile(samples.map(s => s.paint_ms), limits.paint_percentile) },
      rss_peak_mb: heap.live_bytes / limits.bytes_per_mb, host_mem: heap.host_mem };
  } finally {
    if (tracing) await cdp.send("Tracing.end").catch(() => {});
    await page.evaluate(held => {
      dropTab(tabOfTerm(window.__BASELINE_PANE__.term).id);
      delete window.__BASELINE_PANE__;
      window.__HOLD_POLLERS__ = held;
      for (const mark of performance.getEntriesByType("mark")) if (mark.name.startsWith("baseline-pane-")) performance.clearMarks(mark.name);
    }, setup.held);
    await cdp.detach();
  }
}
