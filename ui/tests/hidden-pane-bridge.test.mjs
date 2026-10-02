import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

/* What a hidden pane's page is told before its first script (t-20972): the
 * global API is gone. The script is the one the window embeds
 * (`artifact_thumbs::NO_BRIDGE_JS`), run here against a stand-in `window` that
 * wears what Tauri hands every webview, with the attributes Tauri gives it
 * (tauri 2.11, `scripts/bundle.global.js` and `manager/webview.rs`):
 * `__TAURI__` is assigned, so it can be removed; `__TAURI_INTERNALS__` and
 * `isTauri` are defined with `Object.defineProperty` and no flags, so they
 * cannot be written or removed. */
const SCRIPT = readFileSync(new URL("../hidden-pane-no-bridge.js", import.meta.url), "utf8");
const SHELL_BOOT = readFileSync(new URL("../shell-boot.js", import.meta.url), "utf8");

function paneWithBridge() {
  const pane = vm.createContext({ artifactOwn: "an artifact's own global" });
  pane.window = pane;
  Object.defineProperty(pane, "isTauri", { value: true });
  Object.defineProperty(pane, "__TAURI_INTERNALS__", {
    value: { plugins: {}, metadata: { currentWebview: { label: "artifact-thumb" } } },
  });
  pane.__TAURI__ = {
    core: { invoke: () => Promise.resolve(null), Channel: class {} },
    event: { listen: () => Promise.resolve() },
  };
  return pane;
}

test("the global API is taken away before the page's first script, and nothing else is", () => {
  const pane = paneWithBridge();
  vm.runInContext(SCRIPT, pane);
  assert.equal(pane.__TAURI__, undefined);
  assert.equal("__TAURI__" in pane, false);
  assert.equal(pane.artifactOwn, "an artifact's own global");
  // What Tauri made unremovable stays, and the script does not throw over it.
  assert.equal(pane.isTauri, true);
  assert.equal(pane.__TAURI_INTERNALS__.metadata.currentWebview.label, "artifact-thumb");
});

test("the window's own first line stops there, as it does for any page opened outside the app", () => {
  const pane = paneWithBridge();
  vm.runInContext(SCRIPT, pane);
  // The line the script exists for: `ui/shell-boot.js` takes the API apart at once.
  const first = SHELL_BOOT.split("\n").find((line) => line.includes("= window.__TAURI__.core;"));
  assert.ok(first, "shell-boot.js no longer reads the global API on one line");
  // The pane's own realm's error is known by its name, not by `instanceof`.
  assert.throws(() => vm.runInContext(first, pane), (error) => error.name === "TypeError");
});

test("without the script the same page gets its API and asks", () => {
  // The control: the line above does not throw when the API is there.
  const pane = paneWithBridge();
  const first = SHELL_BOOT.split("\n").find((line) => line.includes("= window.__TAURI__.core;"));
  assert.doesNotThrow(() => vm.runInContext(first, pane));
});

test("a page that never touched the API runs the script and notices nothing", () => {
  const pane = vm.createContext({});
  pane.window = pane;
  assert.doesNotThrow(() => vm.runInContext(SCRIPT, pane));
});

test("an API the engine will not let go of does not make the script throw", () => {
  const pane = paneWithBridge();
  Object.defineProperty(pane, "__TAURI__", { value: { kept: true }, configurable: false });
  assert.doesNotThrow(() => vm.runInContext(SCRIPT, pane));
});
