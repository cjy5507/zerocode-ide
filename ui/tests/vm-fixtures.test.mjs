import test from "node:test";

// window.mjs loads these assertion-only VM fixtures before selecting a
// browser suite. Check their bootstrap dependencies before any Rust build or
// browser launch, using the same production source that each fixture reads.
for (const fixture of ["scm-notices.mjs", "mail-waiting.mjs", "prompt-parked.mjs", "summon-lineup-notice.mjs"]) {
  test(`VM fixture ${fixture} initializes with its production dependencies`, async () => {
    await import(new URL(fixture, import.meta.url));
  });
}
