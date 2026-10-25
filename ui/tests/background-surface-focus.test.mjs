import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const status = read("../shell-status.js");
const browserCommands = read("../../crates/zerocode-shell/src/cmd/browser.rs");
const chromium = read("../../crates/zerocode-shell/src/chromium_browser.rs");
const ios = read("../../crates/zerocode-shell/src/emulator/ios.rs");
const simulator = read("../../crates/zerocode-shell/src/simulator_window.rs");

const closeSource = status.match(/^function closeTab\(.*?^\}/ms)?.[0];
assert.ok(closeSource, "the production closeTab function is present");

function closeBrowser({ active = false, receiver = "other" } = {}) {
  const body = {};
  const field = {};
  const chrome = {};
  const document = { body, activeElement: { other: field, body, chrome }[receiver] };
  const tab = { id: "browser:closing", kind: "browser", pane: 0 };
  const state = vm.createContext({
    // closeTab's default hand (shell-boot.js handNow, t-75209): the person's
    // own press closes the browser in every scene here.
    handNow: () => "person",
    tabs: [tab],
    activeTabId: active ? tab.id : "term:kept",
    document,
    termFloat: { hidden: true },
    groupOf: () => ({ browserView: { contains: (node) => node === chrome } }),
    focused: 0,
    keySink: { focus() { state.focused += 1; document.activeElement = state.keySink; } },
    dropTab(id) { state.tabs = state.tabs.filter((one) => one.id !== id); },
  });
  vm.runInContext(closeSource, state);
  state.closeTab(tab.id);
  return { state, field, document };
}

test("closing an inactive browser keeps the field receiving the person's typing", () => {
  const { state, field, document } = closeBrowser();
  assert.equal(document.activeElement, field);
  assert.equal(state.focused, 0);
  assert.equal(state.tabs.length, 0);
});

test("closing a browser covered by another field keeps that field's focus", () => {
  const { state, field, document } = closeBrowser({ active: true });
  assert.equal(document.activeElement, field);
  assert.equal(state.focused, 0);
});

for (const receiver of ["body", "chrome"]) {
  test(`closing the active browser returns its ${receiver} receiver to the terminal`, () => {
    const { state, document } = closeBrowser({ active: true, receiver });
    assert.equal(state.focused, 1);
    assert.equal(document.activeElement, state.keySink);
  });
}

test("browser placement delegates focus return instead of always focusing the main webview", () => {
  const place = browserCommands.match(/pub\(crate\) fn browser_place\([\s\S]*?\n\}/)?.[0];
  assert.ok(place, "browser_place is present");
  assert.doesNotMatch(place, /webview\.set_focus\(/);
  assert.match(place, /hide_browser_pane\(/);
  assert.match(place, /from_the_main_webview\(&webview\)\?/);
  assert.match(place, /browser_pane_of\(&app, &state, &label\)\?/);
});

test("CEF removal checks ownership of the native responder before clearing it", () => {
  const close = chromium.match(/struct CloseBrowserViewTask\s*\{[\s\S]*?\nwrap_life_span_handler!/)?.[0];
  assert.ok(close, "CloseBrowserViewTask is present");
  assert.match(close, /if\s+(?:unsafe\s*\{\s*)?[^\n]*browser_view_owns_first_responder\(view\)/);
});

test("an in-window iOS stream never hides the whole Simulator application", () => {
  const start = ios.match(/pub\(crate\) async fn start_emulator_stream\([\s\S]*?\n\}/)?.[0];
  assert.ok(start, "start_emulator_stream is present");
  assert.doesNotMatch(start, /hide_simulator\(/);
  assert.doesNotMatch(simulator, /running\.hide\(/);
});

test("Simulator preparation source leaves an already running application alone", () => {
  const prepare = ios.match(/fn prepare_simulator_services\(udid: &str\)\s*\{[\s\S]*?\n\}/)?.[0];
  assert.ok(prepare, "the macOS service preparation is present");
  assert.match(prepare, /runningApplicationsWithBundleIdentifier\(&bundle\)[\s\S]*?\.is_some\(\)\s*\{\s*return;\s*\}[\s\S]*?quiet_command\("open"\)/);
});
