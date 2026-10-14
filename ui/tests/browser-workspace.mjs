import { openWindowTestPage } from "./window-boot.mjs";

const FRAME = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jr1sAAAAASUVORK5CYII=";

async function standWorkspace(browser, origin, { holdState = false } = {}) {
  const opened = await openWindowTestPage(browser, origin);
  await opened.page.evaluate(({ frame, holdState }) => {
    window.__WORKSPACE__ = {
      opened: [], placed: [], modes: [], snapshots: [], closed: [], saved: [],
      state: [], holdState, holdMode: false, holdSnapshot: false,
      releaseState: null, releaseMode: null, releaseSnapshot: null,
    };
    let minted = 810;
    const mock = window.__WORKSPACE__;
    window.__ANSWER__.open_browser_pane = (args) => {
      mock.opened.push(args);
      return args.label ?? `browser-${++minted}`;
    };
    window.__ANSWER__.browser_place = (args) => {
      mock.placed.push(args);
      return mock.holdPlacement ? new Promise((resolve) => { mock.releasePlacement = resolve; }) : null;
    };
    window.__ANSWER__.browser_zoom = () => null;
    window.__ANSWER__.close_browser_pane = ({ label }) => { mock.closed.push(label); return null; };
    window.__ANSWER__.set_browser_open_tabs = ({ tabs: records }) => {
      mock.saved.push(records);
      return { browser: { ...browserPrefs, open_tabs: records } };
    };
    window.__ANSWER__.get_browser_workspaces = () => mock.holdState
      ? new Promise((resolve) => { mock.releaseState = resolve; }) : mock.state;
    window.__ANSWER__.set_browser_workspace_mode = (args) => {
      mock.modes.push(args);
      if (mock.holdMode) return new Promise((resolve, reject) => {
        mock.releaseMode = resolve;
        mock.rejectMode = reject;
      });
      const next = {
        label: args.label, mode: args.mode, revision: args.expectedRevision + 1,
        inFlight: 0, needsObservation: args.mode === "agent",
      };
      mock.state = [next];
      return next;
    };
    window.__ANSWER__.browser_snapshot = ({ label }) => {
      mock.snapshots.push({ label, at: performance.now() });
      return mock.holdSnapshot ? new Promise((resolve) => { mock.releaseSnapshot = resolve; }) : frame;
    };
    window.__workspaceEmit = (state) => {
      mock.state = [state];
      for (const hear of window.__LISTENERS__["browser:workspace"] ?? []) hear({ payload: state });
    };
    window.__workspaceClick = (label, action) => {
      const tab = tabs.find((one) => one.kind === "browser" && one.label === label);
      const host = tab && groupOf(tab.pane)?.browserView;
      const button = host?.querySelector(`.browser-workspace-${action}`);
      if (!button || button.hidden || button.disabled) return false;
      button.click();
      return true;
    };
  }, { frame: FRAME, holdState });
  return opened;
}

async function openManaged(page, label = "browser-801") {
  await page.evaluate((label) => {
    for (const hear of window.__LISTENERS__["browser:agent-open"] ?? []) {
      hear({ payload: { label, url: "https://example.com/workspace" } });
    }
  }, label);
  await page.waitForFunction((label) => tabs.some((one) => one.label === label), label);
  return label;
}

async function observe(page, label) {
  await page.evaluate((label) => {
    setActiveTab(`browser:${label}`);
    window.__workspaceClick(label, "watch");
  }, label);
  await page.waitForTimeout(100);
}

export async function testBrowserWorkspace(browser, origin, ok) {
  {
    const { page, faults } = await standWorkspace(browser, origin, { holdState: true });
    try {
      const before = await page.evaluate(() => ({ tab: activeTabId, pane: focusedPane, tree: JSON.stringify(stageTree()) }));
      const label = await openManaged(page);
      const quiet = await page.evaluate((label) => ({
        tab: activeTabId, pane: focusedPane, tree: JSON.stringify(stageTree()),
        opened: window.__WORKSPACE__.opened,
        shown: window.__WORKSPACE__.placed.filter((one) => one.label === label && one.shown),
        captures: window.__WORKSPACE__.snapshots.length,
        launcher: !!document.querySelector("#browser-workspace-launcher"),
      }), label);
      ok("agent open opts into a workspace without moving the current tab or capturing",
        quiet.opened[0]?.agentWorkspace === true && quiet.tab === before.tab && quiet.pane === before.pane
          && quiet.tree === before.tree && quiet.shown.length === 0 && quiet.captures === 0,
        JSON.stringify(quiet));
      ok("the task browser has a visible viewing action beside the run control", quiet.launcher, JSON.stringify(quiet));
      await observe(page, label);
      const unknown = await page.evaluate((label) => ({
        shown: window.__WORKSPACE__.placed.filter((one) => one.label === label && one.shown).length,
        captures: window.__WORKSPACE__.snapshots.length,
        text: groupOf(tabs.find((one) => one.label === label).pane)?.browserView?.textContent,
      }), label);
      ok("an unconfirmed managed tab stays hidden and explains the unavailable state",
        unknown.shown === 0 && unknown.captures === 0 && /확인|상태/.test(unknown.text ?? ""), JSON.stringify(unknown));
      await page.evaluate((label) => {
        const state = { label, mode: "agent", revision: 4, inFlight: 1, needsObservation: false };
        window.__WORKSPACE__.holdState = false;
        window.__WORKSPACE__.state = [state];
        window.__WORKSPACE__.releaseState?.([state]);
        window.__workspaceEmit(state);
      }, label);
      await observe(page, label);
      const count = await page.evaluate(() => window.__WORKSPACE__.snapshots.length);
      ok("watching takes a real browser snapshot after the backend confirms ownership", count === 1, String(count));
      const input = await page.evaluate((label) => {
        const tab = tabs.find((one) => one.label === label);
        const host = groupOf(tab.pane)?.browserView;
        const before = { ...window.__COUNTS__ };
        for (const type of ["pointerdown", "click", "wheel", "keydown"]) {
          host?.querySelector(".browser-workspace-surface")?.dispatchEvent(
            type === "keydown" ? new KeyboardEvent(type, { key: "x", bubbles: true })
              : new MouseEvent(type, { bubbles: true, cancelable: true }),
          );
        }
        host?.querySelector(".browser-back")?.click();
        host?.querySelector(".browser-reload")?.click();
        submitBrowserAddress(tab, "https://example.com/blocked");
        return {
          calls: Object.keys(window.__COUNTS__).filter((key) => /browser_(history|navigate|reload|type|click|scroll|eval|key)/.test(key))
            .filter((key) => window.__COUNTS__[key] !== (before[key] ?? 0)),
          addressDisabled: host?.querySelector(".browser-address")?.readOnly,
          frames: host?.querySelectorAll(".browser-workspace-frame[src]").length,
          shown: window.__WORKSPACE__.placed.filter((one) => one.label === label && one.shown).length,
        };
      }, label);
      ok("the watch surface and address controls send no page input", input.calls.length === 0 && input.addressDisabled
        && input.frames === 1 && input.shown === 0, JSON.stringify(input));
      await page.waitForTimeout(1150);
      const cadence = await page.evaluate(() => window.__WORKSPACE__.snapshots);
      ok("visible watching starts no more than one snapshot each second", cadence.length >= 2
        && cadence.every((one, at) => at === 0 || one.at - cadence[at - 1].at >= 995), JSON.stringify(cadence));
      ok("workspace observation raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }

  {
    const { page, faults } = await standWorkspace(browser, origin);
    try {
      const label = await openManaged(page);
      await page.evaluate((label) => window.__workspaceEmit({ label, mode: "agent", revision: 10, inFlight: 1, needsObservation: false }), label);
      await observe(page, label);
      await page.evaluate((label) => {
        window.__WORKSPACE__.holdMode = true;
        window.__workspaceClick(label, "human");
      }, label);
      const pending = await page.evaluate((label) => ({
        modes: window.__WORKSPACE__.modes,
        shown: window.__WORKSPACE__.placed.filter((one) => one.label === label && one.shown).length,
        disabled: ["pause", "human", "agent"].every((name) =>
          groupOf(tabs.find((one) => one.label === label).pane)?.browserView?.querySelector(`.browser-workspace-${name}`)?.disabled),
      }), label);
      ok("human handoff uses the observed revision and hides the native pane while pending",
        pending.modes.length === 1 && pending.modes[0]?.mode === "human" && pending.modes[0]?.expectedRevision === 10
          && pending.shown === 0 && pending.disabled, JSON.stringify(pending));
      await page.evaluate((label) => {
        const state = { label, mode: "human", revision: 12, inFlight: 0, needsObservation: false };
        window.__workspaceEmit(state);
      }, label);
      await page.waitForTimeout(60);
      const beforeReply = await page.evaluate((label) => window.__WORKSPACE__.placed.filter((one) => one.label === label && one.shown).length, label);
      ok("even a human event cannot expose the native pane before its handoff response", beforeReply === 0, String(beforeReply));
      await page.evaluate((label) => window.__WORKSPACE__.releaseMode?.({
        label, mode: "human", revision: 12, inFlight: 0, needsObservation: false,
      }), label);
      await page.waitForTimeout(100);
      const human = await page.evaluate((label) => ({
        last: window.__WORKSPACE__.placed.filter((one) => one.label === label).at(-1),
        addressReadOnly: groupOf(tabs.find((one) => one.label === label).pane)?.browserView?.querySelector(".browser-address")?.readOnly,
        captures: window.__WORKSPACE__.snapshots.length,
      }), label);
      ok("confirmed human control exposes the same native pane and enables its address bar",
        human.last?.shown === true && human.addressReadOnly === false, JSON.stringify(human));
      await page.waitForTimeout(1100);
      const humanCount = await page.evaluate(() => window.__WORKSPACE__.snapshots.length);
      ok("human control does not run a snapshot timer", humanCount === human.captures, String(humanCount));
      await page.evaluate((label) => {
        window.__workspaceClick(label, "agent");
      }, label);
      await page.waitForTimeout(60);
      const returning = await page.evaluate((label) => window.__WORKSPACE__.placed.filter((one) => one.label === label).at(-1), label);
      ok("returning control hides the native pane immediately while the backend decides", returning?.shown === false, JSON.stringify(returning));
      await page.evaluate((label) => {
        window.__workspaceEmit({ label, mode: "agent", revision: 15, inFlight: 0, needsObservation: true });
        window.__WORKSPACE__.releaseMode?.({ label, mode: "human", revision: 13, inFlight: 0, needsObservation: false });
      }, label);
      await page.waitForTimeout(100);
      const stale = await page.evaluate((label) => ({
        last: window.__WORKSPACE__.placed.filter((one) => one.label === label).at(-1),
        text: groupOf(tabs.find((one) => one.label === label).pane)?.browserView?.textContent,
      }), label);
      ok("a late handoff response cannot overwrite a newer backend revision", stale.last?.shown === false
        && !/직접 조작 중/.test(stale.text ?? ""), JSON.stringify(stale));
      await page.evaluate((label) => window.__workspaceEmit({ label, mode: "human", revision: 11, inFlight: 0, needsObservation: false }), label);
      await page.waitForTimeout(60);
      const oldEvent = await page.evaluate((label) => window.__WORKSPACE__.placed.filter((one) => one.label === label).at(-1), label);
      ok("an older human event cannot reopen a native pane", oldEvent?.shown === false, JSON.stringify(oldEvent));
      ok("workspace control raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }

  {
    const { page, faults } = await standWorkspace(browser, origin);
    try {
      const original = await page.evaluate(() => activeTabId);
      const label = await openManaged(page);
      await page.evaluate((label) => {
        window.__workspaceEmit({ label, mode: "agent", revision: 20, inFlight: 0, needsObservation: false });
        window.__WORKSPACE__.holdSnapshot = true;
      }, label);
      await observe(page, label);
      await page.waitForTimeout(1150);
      const single = await page.evaluate(() => window.__WORKSPACE__.snapshots.length);
      ok("a slow capture never overlaps a second capture", single === 1, String(single));
      await page.evaluate((label) => window.__workspaceClick(label, "collapse"), label);
      await page.evaluate((frame) => window.__WORKSPACE__.releaseSnapshot?.(frame), FRAME);
      await page.waitForTimeout(1100);
      const folded = await page.evaluate((label) => ({
        count: window.__WORKSPACE__.snapshots.length,
        frames: document.querySelectorAll(".browser-workspace-frame[src]").length,
        last: window.__WORKSPACE__.placed.filter((one) => one.label === label).at(-1),
        closed: window.__WORKSPACE__.closed,
      }), label);
      ok("collapse discards a late frame, stops capturing and keeps the task browser alive",
        folded.count === 1 && folded.frames === 0 && folded.last?.shown === false && folded.closed.length === 0,
        JSON.stringify(folded));
      await page.evaluate((label) => {
        window.__WORKSPACE__.holdSnapshot = false;
        setActiveTab(`browser:${label}`);
        window.__workspaceClick(label, "watch");
      }, label);
      await page.waitForTimeout(100);
      await page.evaluate(() => {
        Object.defineProperty(document, "hidden", { configurable: true, value: true });
        document.dispatchEvent(new Event("visibilitychange"));
      });
      const hiddenCount = await page.evaluate(() => window.__WORKSPACE__.snapshots.length);
      await page.waitForTimeout(1100);
      const hidden = await page.evaluate(() => ({
        count: window.__WORKSPACE__.snapshots.length,
        frames: document.querySelectorAll(".browser-workspace-frame[src]").length,
      }));
      ok("a hidden document retains no frame and starts no capture", hidden.count === hiddenCount && hidden.frames === 0, JSON.stringify(hidden));
      await page.evaluate((original) => {
        Object.defineProperty(document, "hidden", { configurable: true, value: false });
        setActiveTab(original);
        document.dispatchEvent(new Event("visibilitychange"));
      }, original);
      const awayCount = await page.evaluate(() => window.__WORKSPACE__.snapshots.length);
      await page.waitForTimeout(1100);
      const away = await page.evaluate(() => window.__WORKSPACE__.snapshots.length);
      ok("switching away stops observation without closing the browser", away === awayCount, String(away));
      await page.evaluate((label) => {
        setActiveTab(`browser:${label}`);
        window.__WORKSPACE__.holdMode = true;
        window.__workspaceClick(label, "human");
        closeTab(`browser:${label}`);
        window.__WORKSPACE__.releaseMode?.({ label, mode: "human", revision: 21, inFlight: 0, needsObservation: false });
      }, label);
      await page.waitForTimeout(100);
      const closedCount = await page.evaluate(() => window.__WORKSPACE__.snapshots.length);
      await page.waitForTimeout(1100);
      const closed = await page.evaluate((label) => ({
        count: window.__WORKSPACE__.snapshots.length,
        exists: tabs.some((one) => one.label === label),
        frames: document.querySelectorAll(".browser-workspace-frame[src]").length,
        last: window.__WORKSPACE__.placed.filter((one) => one.label === label).at(-1),
      }), label);
      ok("a closed tab rejects late control and capture work", !closed.exists && closed.count === closedCount
        && closed.frames === 0 && closed.last?.shown !== true, JSON.stringify(closed));
      ok("workspace cleanup raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }

  {
    const { page, faults } = await standWorkspace(browser, origin, { holdState: true });
    try {
      await page.evaluate(async () => {
        browserPrefs = { ...browserPrefs, restore_tabs: true, open_tabs: [{
          url: "https://example.com/restored", agentWorkspace: true,
        }] };
        await restoreBrowserTabs();
      });
      const restored = await page.evaluate(() => ({
        opened: window.__WORKSPACE__.opened,
        shown: window.__WORKSPACE__.placed.filter((one) => one.shown),
        records: browserOpenTabRecords(),
      }));
      ok("restored workspace ownership survives persistence and stays hidden until confirmed",
        restored.opened[0]?.agentWorkspace === true && restored.opened[0]?.workspaceRestored === true && restored.shown.length === 0
          && restored.records[0]?.agentWorkspace === true, JSON.stringify(restored));
      await page.evaluate(() => {
        window.__WORKSPACE__.holdState = false;
        window.__WORKSPACE__.releaseState?.(null);
      });
      await page.waitForTimeout(60);
      const failed = await page.evaluate(() => ({
        shown: window.__WORKSPACE__.placed.filter((one) => one.shown),
        text: document.querySelector(".browser-view:not([hidden]) .browser-workspace-status")?.textContent,
      }));
      ok("an unknown state response keeps the restored view hidden with an inline error",
        failed.shown.length === 0 && !!failed.text, JSON.stringify(failed));
      const humanLabel = await page.evaluate(() => openBrowserTab("https://example.com/person"));
      await page.waitForTimeout(100);
      const person = await page.evaluate((label) => ({
        open: window.__WORKSPACE__.opened.at(-1),
        last: window.__WORKSPACE__.placed.filter((one) => one.label === label).at(-1),
        barHidden: groupOf(tabs.find((one) => one.label === label).pane)?.browserView?.querySelector(".browser-workspace-bar")?.hidden,
      }), humanLabel);
      ok("ordinary user tabs keep their native view and do not opt into agent ownership",
        person.open?.agentWorkspace !== true && person.last?.shown === true && person.barHidden === true,
        JSON.stringify(person));
      ok("workspace restoration raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }

  {
    const { page, faults } = await standWorkspace(browser, origin);
    try {
      const label = await openManaged(page);
      await page.evaluate((label) => window.__workspaceEmit({ label, mode: "agent", revision: 30, inFlight: 1, needsObservation: false }), label);
      await observe(page, label);
      await page.evaluate((label) => {
        window.__WORKSPACE__.holdMode = true;
        window.__workspaceClick(label, "pause");
        window.__workspaceClick(label, "human");
        window.__workspaceClick(label, "agent");
      }, label);
      await page.waitForTimeout(60);
      const pausing = await page.evaluate((label) => ({
        modes: window.__WORKSPACE__.modes,
        text: groupOf(tabs.find((one) => one.label === label).pane)?.browserView?.querySelector(".browser-workspace-status")?.textContent,
      }), label);
      ok("rapid control clicks submit one pause request without announcing an unconfirmed pause",
        pausing.modes.length === 1 && pausing.modes[0]?.mode === "paused"
          && pausing.modes[0]?.expectedRevision === 30 && !/작업 일시정지/.test(pausing.text ?? ""), JSON.stringify(pausing));
      await page.evaluate((label) => window.__WORKSPACE__.releaseMode?.({ label, mode: "paused", revision: 31, inFlight: 0, needsObservation: false }), label);
      await page.waitForTimeout(60);
      const paused = await page.evaluate((label) => ({
        text: groupOf(tabs.find((one) => one.label === label).pane)?.browserView?.querySelector(".browser-workspace-status")?.textContent,
        last: window.__WORKSPACE__.placed.filter((one) => one.label === label).at(-1),
      }), label);
      ok("a pause is displayed only after backend confirmation and never exposes native input",
        /일시정지/.test(paused.text ?? "") && paused.last?.shown === false, JSON.stringify(paused));
      await page.evaluate((label) => {
        window.__WORKSPACE__.holdMode = false;
        window.__workspaceClick(label, "agent");
      }, label);
      await page.waitForTimeout(60);
      const resumed = await page.evaluate((label) => ({
        mode: window.__WORKSPACE__.modes.at(-1),
        text: groupOf(tabs.find((one) => one.label === label).pane)?.browserView?.querySelector(".browser-workspace-status")?.textContent,
        last: window.__WORKSPACE__.placed.filter((one) => one.label === label).at(-1),
      }), label);
      ok("handing back uses the paused revision and explains the fresh observation requirement",
        resumed.mode?.expectedRevision === 31 && resumed.mode?.mode === "agent" && resumed.last?.shown === false
          && /새 화면 확인/.test(resumed.text ?? "") && !/작업 중/.test(resumed.text ?? ""), JSON.stringify(resumed));
      await page.evaluate((label) => {
        window.__WORKSPACE__.holdMode = true;
        window.__workspaceClick(label, "human");
      }, label);
      await page.waitForTimeout(30);
      await page.evaluate(() => window.__WORKSPACE__.rejectMode?.(new Error("handoff_timeout")));
      await page.waitForTimeout(100);
      const rejected = await page.evaluate((label) => ({
        text: groupOf(tabs.find((one) => one.label === label).pane)?.browserView?.querySelector(".browser-workspace-status")?.textContent,
        last: window.__WORKSPACE__.placed.filter((one) => one.label === label).at(-1),
        humanDisabled: groupOf(tabs.find((one) => one.label === label).pane)?.browserView?.querySelector(".browser-workspace-human")?.disabled,
      }), label);
      ok("a rejected handoff stays hidden with an inline recovery action", rejected.last?.shown === false
        && rejected.humanDisabled === true && /확인하지 못|확인할 수/.test(rejected.text ?? ""), JSON.stringify(rejected));
      await page.evaluate((label) => {
        window.__WORKSPACE__.state = [{ label, mode: "paused", revision: 34, inFlight: 0, needsObservation: false }];
        window.__workspaceClick(label, "refresh");
      }, label);
      await page.waitForTimeout(60);
      await page.evaluate((label) => {
        window.__workspaceClick(label, "human");
      }, label);
      await page.waitForTimeout(30);
      await page.evaluate((label) => window.__WORKSPACE__.releaseMode?.({ label, mode: "human", revision: 35, inFlight: 1, needsObservation: false }), label);
      await page.waitForTimeout(80);
      const invalid = await page.evaluate((label) => window.__WORKSPACE__.placed.filter((one) => one.label === label).at(-1), label);
      ok("a human response with input still in flight is refused", invalid?.shown === false, JSON.stringify(invalid));
      ok("workspace pause and error handling raise no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }

  {
    const { page, faults } = await standWorkspace(browser, origin);
    try {
      const original = await page.evaluate(() => {
        const field = document.createElement("input");
        field.id = "workspace-focus-test";
        document.body.appendChild(field);
        field.focus();
        return { id: activeTabId, tree: JSON.stringify(stageTree()) };
      });
      const label = await openManaged(page);
      await page.evaluate((label) => window.__workspaceEmit({ label, mode: "agent", revision: 40, inFlight: 0, needsObservation: false }), label);
      const quiet = await page.evaluate(() => ({
        id: activeTabId, focused: document.activeElement?.id, tree: JSON.stringify(stageTree()),
      }));
      ok("agent open and state events preserve the actual typing receiver", quiet.id === original.id
        && quiet.focused === "workspace-focus-test" && quiet.tree === original.tree, JSON.stringify(quiet));
      await page.evaluate(() => document.querySelector("#browser-workspace-launcher")?.click());
      await page.waitForTimeout(100);
      const beside = await page.evaluate((label) => ({
        active: activeTabId,
        split: stageGroups().length,
        badge: [...document.querySelectorAll(".tab[data-tab]")]
          .find((node) => node.dataset.tab === `browser:${label}`)?.querySelector(".browser-workspace-tab-badge")?.textContent,
      }), label);
      ok("the explicit viewing action opens the marked task browser beside the original pane",
        beside.active === `browser:${label}` && beside.split === 2 && !!beside.badge, JSON.stringify(beside));
      await page.evaluate((label) => window.__workspaceClick(label, "collapse"), label);
      await page.waitForTimeout(60);
      const back = await page.evaluate(() => ({
        id: activeTabId, tree: JSON.stringify(stageTree()), closed: window.__WORKSPACE__.closed,
        frames: document.querySelectorAll(".browser-workspace-frame[src]").length,
      }));
      ok("folding the side view restores the original pane and retains the native browser",
        back.id === original.id && back.tree === original.tree && back.closed.length === 0 && back.frames === 0, JSON.stringify(back));
      await observe(page, label);
      await page.evaluate((label) => {
        const newer = { label, mode: "agent", revision: 42, inFlight: 0, needsObservation: true };
        window.__workspaceEmit(newer);
        window.__workspaceEmit({ label, mode: "human", revision: 42, inFlight: 0, needsObservation: false });
      }, label);
      await page.waitForTimeout(60);
      const conflict = await page.evaluate((label) => ({
        last: window.__WORKSPACE__.placed.filter((one) => one.label === label).at(-1),
        text: groupOf(tabs.find((one) => one.label === label).pane)?.browserView?.querySelector(".browser-workspace-status")?.textContent,
      }), label);
      ok("conflicting modes at one revision fail closed instead of choosing human control",
        conflict.last?.shown === false && /확인/.test(conflict.text ?? ""), JSON.stringify(conflict));
      await page.evaluate((label) => {
        window.__WORKSPACE__.state = [{ label, mode: "agent", revision: 43, inFlight: 0, needsObservation: false }];
        window.__ANSWER__.browser_snapshot = () => null;
        window.__workspaceClick(label, "refresh");
      }, label);
      await page.waitForTimeout(1100);
      const badFrame = await page.evaluate((label) => ({
        text: groupOf(tabs.find((one) => one.label === label).pane)?.browserView?.querySelector(".browser-workspace-status")?.textContent,
        frames: document.querySelectorAll(".browser-workspace-frame[src]").length,
      }), label);
      ok("an unknown capture result clears the image and offers inline recovery",
        badFrame.frames === 0 && /가져오지 못/.test(badFrame.text ?? ""), JSON.stringify(badFrame));
      await page.setViewportSize({ width: 760, height: 700 });
      const layout = await page.evaluate((label) => {
        const host = groupOf(tabs.find((one) => one.label === label).pane)?.browserView;
        const bar = host?.querySelector(".browser-workspace-bar");
        const box = host?.getBoundingClientRect();
        return {
          host: box?.width, bar: bar?.getBoundingClientRect().width,
          actionsFit: [...(bar?.querySelectorAll("button") ?? [])].filter((button) => !button.hidden)
            .every((button) => button.getBoundingClientRect().right <= (box?.right ?? 0) + 1),
          imageFit: getComputedStyle(host.querySelector(".browser-workspace-frame")).objectFit,
        };
      }, label);
      ok("narrow task controls stay inside their pane and preserve the image proportions",
        layout.host > 0 && layout.bar <= layout.host + 1 && layout.actionsFit && layout.imageFit === "contain", JSON.stringify(layout));
      const words = await page.evaluate(() => ["en", "ja", "zh", "es"].map((code) => {
        const keys = ["title", "agent", "watchAction", "collapseAction", "pauseAction", "humanAction", "agentAction", "refreshAction", "stateError"];
        return { code, complete: keys.every((key) => typeof CATALOG[code]?.[`browser.workspace.${key}`] === "string") };
      }));
      ok("all four existing translations cover the workspace state and actions", words.every((one) => one.complete), JSON.stringify(words));
      ok("workspace viewing and recovery raise no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }

  {
    const { page, faults } = await standWorkspace(browser, origin);
    try {
      const label = await openManaged(page);
      await page.evaluate(async (label) => {
        window.__workspaceEmit({ label, mode: "agent", revision: 50, inFlight: 0, needsObservation: false });
        await browserPlacing;
        window.__WORKSPACE__.placed = [];
        window.__WORKSPACE__.holdPlacement = true;
        browserSaid.delete(label);
        syncBrowserPanes();
      }, label);
      await page.waitForTimeout(60);
      await page.evaluate((label) => {
        window.__workspaceEmit({ label, mode: "human", revision: 51, inFlight: 0, needsObservation: false });
        setActiveTab(`browser:${label}`);
        window.__workspaceClick(label, "watch");
        window.__workspaceEmit({ label, mode: "agent", revision: 52, inFlight: 0, needsObservation: true });
        window.__WORKSPACE__.holdPlacement = false;
        window.__WORKSPACE__.releasePlacement?.(null);
      }, label);
      await page.waitForTimeout(100);
      const queued = await page.evaluate((label) => ({
        shown: window.__WORKSPACE__.placed.filter((one) => one.label === label && one.shown),
        last: window.__WORKSPACE__.placed.filter((one) => one.label === label).at(-1),
      }), label);
      ok("a queued native show rechecks ownership when it actually executes",
        queued.shown.length === 0 && queued.last?.shown === false, JSON.stringify(queued));
      ok("workspace native placement races raise no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }
}
