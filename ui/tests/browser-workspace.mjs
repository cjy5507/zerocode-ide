import { openWindowTestPage, PRIMARY_EVENT } from "./window-boot.mjs";

const FRAME = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jr1sAAAAASUVORK5CYII=";

async function standWorkspace(browser, origin, { holdState = false, actorName = "codex" } = {}) {
  const opened = await openWindowTestPage(browser, origin);
  await opened.page.waitForFunction(() => {
    const actorTab = tabs.find((tab) => tab.kind === "term");
    return !!actorTab && activePaneOf(actorTab) !== null;
  });
  await opened.page.evaluate(({ frame, holdState, actorName }) => {
    window.__WORKSPACE__ = {
      opened: [], placed: [], modes: [], snapshots: [], closed: [], closeCalls: [], saved: [],
      state: [], holdState, holdMode: false, holdSnapshot: false,
      releaseState: null, releaseMode: null, releaseSnapshot: null,
    };
    let minted = 810;
    const mock = window.__WORKSPACE__;
    const actorTab = tabs.find((tab) => tab.kind === "term");
    const actorTerm = activePaneOf(actorTab) ?? actorTab.term;
    paneSessions.delete(actorTerm);
    paneAgents.set(actorTerm, actorName);
    mock.actorTerm = String(actorTerm);
    const snapshot = (state) => state && typeof state === "object"
      ? { ownerTerm: mock.actorTerm, snapshotSupported: true, ...state } : state;
    const snapshots = (states) => Array.isArray(states) ? states.map(snapshot) : states;
    window.__ANSWER__.open_browser_pane = (args) => {
      mock.opened.push(args);
      const label = args.label ?? `browser-${++minted}`;
      return mock.holdOpen ? new Promise((resolve) => { mock.releaseOpen = () => resolve(label); }) : label;
    };
    window.__ANSWER__.browser_place = (args) => {
      mock.placed.push(args);
      return mock.holdPlacement ? new Promise((resolve) => { mock.releasePlacement = resolve; }) : null;
    };
    window.__ANSWER__.browser_zoom = () => null;
    window.__ANSWER__.close_browser_pane = (args) => {
      mock.closeCalls.push(args);
      if (mock.unknownCloseAck) return {};
      if (mock.holdClose) return new Promise((resolve, reject) => {
        mock.releaseClose = () => { mock.closed.push(args.label); resolve(null); };
        mock.rejectClose = reject;
      });
      mock.closed.push(args.label);
      return null;
    };
    window.__ANSWER__.set_browser_open_tabs = ({ tabs: records }) => {
      mock.saved.push(records);
      return { browser: { ...browserPrefs, open_tabs: records } };
    };
    window.__ANSWER__.get_browser_workspaces = () => mock.holdState
      ? new Promise((resolve) => { mock.releaseState = (states) => resolve(snapshots(states)); }) : snapshots(mock.state);
    window.__ANSWER__.set_browser_workspace_mode = (args) => {
      mock.modes.push(args);
      if (mock.holdMode) return new Promise((resolve, reject) => {
        mock.releaseMode = (state) => resolve(snapshot(state));
        mock.rejectMode = reject;
      });
      const next = {
        label: args.label, mode: args.mode, revision: args.expectedRevision + 1,
        inFlight: 0, needsObservation: args.mode === "agent",
        ownerTerm: args.ownerTerm ?? mock.state[0]?.ownerTerm ?? mock.actorTerm,
        snapshotSupported: mock.state[0]?.snapshotSupported ?? true,
      };
      mock.state = [next];
      return next;
    };
    window.__ANSWER__.browser_snapshot = ({ label }) => {
      mock.snapshots.push({ label, at: performance.now() });
      return mock.holdSnapshot ? new Promise((resolve) => { mock.releaseSnapshot = resolve; }) : frame;
    };
    window.__workspaceEmit = (state) => {
      const said = snapshot(state);
      mock.state = [said];
      for (const hear of window.__LISTENERS__["browser:workspace"] ?? []) hear({ payload: said });
    };
    window.__workspaceClick = (label, action) => {
      const tab = tabs.find((one) => one.kind === "browser" && one.label === label);
      const host = tab && groupOf(tab.pane)?.browserView;
      const button = host?.querySelector(`.browser-workspace-${action}`);
      if (!button || button.hidden || button.disabled) return false;
      button.click();
      return true;
    };
  }, { frame: FRAME, holdState, actorName });
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
        text: groupOf(tabs.find((one) => one.label === label).pane)?.browserView?.querySelector(".browser-workspace-preview-note")?.textContent,
        frames: document.querySelectorAll(".browser-workspace-frame[src]").length,
      }), label);
      ok("an unknown capture result clears the image and offers inline recovery",
        badFrame.frames === 0 && /가져오지 못/.test(badFrame.text ?? ""), JSON.stringify(badFrame));
      await page.setViewportSize({ width: 760, height: 700 });
      const layout = await page.evaluate((label) => {
        const host = groupOf(tabs.find((one) => one.label === label).pane)?.browserView;
        const bar = host?.querySelector(".browser-workspace-bar");
        const box = host?.getBoundingClientRect();
        const image = host?.querySelector(".browser-workspace-frame");
        return {
          host: box?.width, bar: bar?.getBoundingClientRect().width,
          actionsFit: [...(bar?.querySelectorAll("button") ?? [])].filter((button) => !button.hidden)
            .every((button) => button.getBoundingClientRect().right <= (box?.right ?? 0) + 1),
          imageFit: image ? getComputedStyle(image).objectFit : null,
        };
      }, label);
      ok("narrow task controls stay inside their pane and preserve the image proportions",
        layout.host > 0 && layout.bar <= layout.host + 1 && layout.actionsFit && layout.imageFit === "contain", JSON.stringify(layout));
      const words = await page.evaluate(() => ["en", "ja", "zh", "es"].map((code) => {
        const keys = ["title", "agent", "watchAction", "collapseAction", "pauseAction", "humanAction", "agentAction", "refreshAction", "stateError", "ownerChoose", "ownerCurrent", "ownerError", "unsupported", "retry"];
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

  {
    const { page, faults } = await standWorkspace(browser, origin);
    try {
      const label = await openManaged(page);
      await page.evaluate((label) => {
        window.__workspaceEmit({ label, mode: "agent", revision: 60, inFlight: 0, needsObservation: false });
      }, label);
      await observe(page, label);
      await page.evaluate((label) => window.__workspaceClick(label, "pause"), label);
      await page.waitForFunction((label) => {
        const tab = tabs.find((entry) => entry.label === label);
        return tab?.workspace?.mode === "paused" && !tab.workspacePending;
      }, label);
      await page.evaluate((label) => window.__workspaceClick(label, "human"), label);
      await page.waitForFunction((label) => window.__WORKSPACE__.placed
        .filter((entry) => entry.label === label).at(-1)?.shown === true, label);
      const shown = await page.evaluate(async (label) => {
        await browserPlacing;
        const mock = window.__WORKSPACE__;
        const last = mock.placed.filter((entry) => entry.label === label).at(-1);
        mock.holdPlacement = true;
        browserSaid.delete(label);
        syncBrowserPanes();
        return last;
      }, label);
      await page.waitForFunction(() => typeof window.__WORKSPACE__.releasePlacement === "function");
      const withheld = await page.evaluate(async (label) => {
        const mock = window.__WORKSPACE__;
        const tab = tabs.find((entry) => entry.label === label);
        mock.placed = [];
        browserSaid.delete(label);
        syncBrowserPanes();
        tab.artifactSwap = {};
        mock.holdPlacement = false;
        mock.releasePlacement(null);
        await browserPlacing;
        return {
          shown: mock.placed.filter((entry) => entry.label === label && entry.shown),
          last: mock.placed.filter((entry) => entry.label === label).at(-1),
        };
      }, label);
      ok("artifact replacement control begins with a confirmed visible human workspace", shown?.shown === true, JSON.stringify(shown));
      ok("a queued workspace show rechecks an artifact replacement before placing the native view",
        withheld.shown.length === 0 && withheld.last?.shown === false, JSON.stringify(withheld));
      ok("workspace artifact replacement races raise no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }

  {
    const { page, faults } = await standWorkspace(browser, origin);
    try {
      const original = await page.evaluate(async () => {
        const source = currentTab();
        const right = splitStageBesideFocused("horizontal");
        await openTermTab({ placement: "tab" });
        const side = currentTab();
        side.pane = right;
        setActiveTab(side.id);
        const below = splitStageBesideFocused("vertical");
        await openTermTab({ placement: "tab" });
        const lower = currentTab();
        lower.pane = below;
        setActiveTab(lower.id);
        setActiveTab(source.id);
        return { source: source.id, right, side: side.id, below, lower: lower.id, tree: JSON.stringify(stageTree()) };
      });
      const label = await openManaged(page);
      await page.evaluate((label) => {
        window.__workspaceEmit({ label, mode: "agent", revision: 60, inFlight: 0, needsObservation: false });
        document.querySelector("#browser-workspace-launcher")?.click();
      }, label);
      await page.waitForTimeout(60);
      const side = await page.evaluate((label) => ({
        active: activeTabId, pane: tabs.find((one) => one.label === label)?.pane, tree: JSON.stringify(stageTree()),
      }), label);
      ok("watching reuses an existing right group without changing its split proportions",
        side.active === `browser:${label}` && side.pane === original.right && side.tree === original.tree, JSON.stringify({ original, side }));
      await page.evaluate((label) => window.__workspaceClick(label, "collapse"), label);
      await page.waitForTimeout(60);
      const restored = await page.evaluate((original) => ({
        active: activeTabId, tree: JSON.stringify(stageTree()),
        side: tabs.find((one) => one.id === original.side)?.pane,
        lower: tabs.find((one) => one.id === original.lower)?.pane,
        rightFront: activeTabIn(original.right)?.id,
      }), original);
      ok("folding preserves the right group's existing tab and layout",
        restored.active === original.source && restored.tree === original.tree
          && restored.side === original.right && restored.lower === original.below && restored.rightFront === original.side,
        JSON.stringify(restored));
      ok("workspace right-group reuse raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }

  {
    const { page, faults } = await standWorkspace(browser, origin);
    try {
      const original = await page.evaluate(async () => {
        const pane = splitStageBesideFocused("horizontal");
        await openTermTab({ placement: "tab" });
        const tab = currentTab();
        tab.pane = pane;
        setActiveTab(tab.id);
        return { id: tab.id, pane };
      });
      const label = await openManaged(page);
      await page.evaluate((label) => {
        window.__workspaceEmit({ label, mode: "agent", revision: 70, inFlight: 0, needsObservation: false });
        document.querySelector("#browser-workspace-launcher")?.click();
      }, label);
      await page.waitForTimeout(60);
      await page.evaluate(({ original, label }) => {
        dropTab(original.id);
        window.__workspaceClick(label, "collapse");
      }, { original, label });
      await page.waitForTimeout(60);
      const fallback = await page.evaluate((label) => {
        const tab = tabs.find((one) => one.label === label);
        return {
          live: !!tab && stageGroups().includes(tab.pane),
          closed: window.__WORKSPACE__.closed,
          frames: document.querySelectorAll(".browser-workspace-frame[src]").length,
        };
      }, label);
      ok("folding after the original group closes keeps the task tab in a live group",
        fallback.live && fallback.closed.length === 0 && fallback.frames === 0, JSON.stringify(fallback));
      ok("workspace missing-return-group handling raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }

  {
    const { page, faults } = await standWorkspace(browser, origin);
    try {
      const label = await openManaged(page);
      await page.evaluate((label) => {
        window.__WORKSPACE__.navigated = [];
        window.__ANSWER__.browser_navigate = (args) => { window.__WORKSPACE__.navigated.push(args); return null; };
        window.__workspacePopup = (said) => {
          for (const hear of window.__LISTENERS__["browser:popup"] ?? []) hear({ payload: said });
        };
        window.__workspaceEmit({ label, mode: "agent", revision: 80, inFlight: 0, needsObservation: false });
        window.__workspacePopup({ label, url: "https://example.com/popup-agent", revision: 80 });
      }, label);
      await page.waitForTimeout(60);
      const agent = await page.evaluate(() => ({ active: activeTabId, navigated: window.__WORKSPACE__.navigated, opened: window.__WORKSPACE__.opened.length }));
      ok("the UI delegates agent popups to the backend without navigating or opening another tab",
        agent.navigated.length === 0 && agent.opened === 1 && agent.active !== `browser:${label}`, JSON.stringify(agent));
      await page.evaluate((label) => window.__workspaceEmit({ label, mode: "human", revision: 81, inFlight: 0, needsObservation: false }), label);
      await observe(page, label);
      await page.evaluate((label) => {
        window.__workspacePopup({ label, url: "https://example.com/popup-old", revision: 80 });
        window.__workspacePopup({ label, url: "https://example.com/popup-unknown" });
      }, label);
      await page.waitForTimeout(40);
      const stale = await page.evaluate(() => window.__WORKSPACE__.navigated);
      ok("managed human popups require their current source revision", stale.length === 0, JSON.stringify(stale));
      await page.evaluate((label) => window.__workspacePopup({ label, url: "https://example.com/popup-human", revision: 81 }), label);
      await page.waitForTimeout(40);
      const human = await page.evaluate(() => window.__WORKSPACE__.navigated);
      ok("a current human popup keeps the existing browser navigation path", human.length === 1
        && human[0]?.url === "https://example.com/popup-human" && human[0]?.expectedRevision === 81, JSON.stringify(human));
      await page.evaluate((label) => {
        window.__workspaceEmit({ label, mode: "paused", revision: 82, inFlight: 0, needsObservation: false });
        window.__workspacePopup({ label, url: "https://example.com/popup-paused", revision: 81 });
      }, label);
      await page.waitForTimeout(40);
      const paused = await page.evaluate(() => window.__WORKSPACE__.navigated.length);
      ok("a popup from before pause cannot navigate the paused task browser", paused === 1, String(paused));
      await page.evaluate((label) => closeTab(`browser:${label}`), label);
      const before = await page.evaluate(() => ({ active: activeTabId, opened: window.__WORKSPACE__.opened.length }));
      await page.evaluate((label) => {
        window.__workspacePopup({ label, url: "https://example.com/popup-closed", revision: 81 });
        window.__workspacePopup({ label: "browser-999", url: "https://example.com/popup-missing" });
      }, label);
      await page.waitForTimeout(80);
      const closed = await page.evaluate(() => ({ active: activeTabId, opened: window.__WORKSPACE__.opened.length }));
      ok("a popup whose origin closed never opens or focuses an ordinary tab", closed.active === before.active
        && closed.opened === before.opened, JSON.stringify({ before, closed }));
      const person = await page.evaluate(async () => {
        const label = await openBrowserTab("https://example.com/person-popup");
        window.__workspacePopup({ label, url: "https://example.com/person-popup-next" });
        return label;
      });
      await page.waitForTimeout(40);
      const ordinary = await page.evaluate((label) => window.__WORKSPACE__.navigated.filter((one) => one.label === label), person);
      ok("ordinary user popups retain their existing navigation behavior", ordinary.length === 1
        && ordinary[0]?.url === "https://example.com/person-popup-next" && ordinary[0]?.expectedRevision === undefined, JSON.stringify(ordinary));
      ok("workspace popup handling raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }

  {
    const { page, faults } = await standWorkspace(browser, origin);
    try {
      const label = await page.evaluate(async () => {
        browserPrefs = { ...browserPrefs, restore_tabs: true, open_tabs: [{ url: "https://example.com/owner-restore", agentWorkspace: true }] };
        await restoreBrowserTabs();
        return tabs.find((tab) => tab.kind === "browser").label;
      });
      await page.evaluate((label) => window.__workspaceEmit({
        label, mode: "paused", revision: 90, inFlight: 0, needsObservation: true, ownerTerm: null,
      }), label);
      const none = await page.evaluate((label) => {
        const tab = tabs.find((one) => one.label === label);
        const host = groupOf(tab.pane)?.browserView;
        window.__workspaceClick(label, "agent");
        return {
          owner: tab.workspace?.ownerTerm,
          choice: host?.querySelector(".browser-workspace-owner-select")?.value,
          disabled: host?.querySelector(".browser-workspace-agent")?.disabled,
          modes: window.__WORKSPACE__.modes.length,
        };
      }, label);
      ok("a restored unassigned task never selects the first agent or resumes implicitly",
        none.owner === null && none.choice === "" && none.disabled === true && none.modes === 0, JSON.stringify(none));
      await page.evaluate((label) => {
        const host = groupOf(tabs.find((one) => one.label === label).pane)?.browserView;
        const select = host?.querySelector(".browser-workspace-owner-select");
        if (select) { select.value = window.__WORKSPACE__.actorTerm; select.dispatchEvent(new Event("change", { bubbles: true })); }
        window.__workspaceClick(label, "agent");
      }, label);
      await page.waitForTimeout(80);
      const assigned = await page.evaluate((label) => ({
        actor: window.__WORKSPACE__.actorTerm,
        call: window.__WORKSPACE__.modes.at(-1),
        text: groupOf(tabs.find((one) => one.label === label).pane)?.browserView?.querySelector(".browser-workspace-owner-name")?.textContent,
      }), label);
      ok("explicit assignment sends the selected decimal terminal ID and displays the confirmed agent",
        assigned.call?.ownerTerm === assigned.actor && assigned.call?.expectedRevision === 90
          && /Codex/.test(assigned.text ?? ""), JSON.stringify(assigned));
      ok("workspace restored-owner selection raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }

  {
    const { page, faults } = await standWorkspace(browser, origin);
    try {
      const label = await openManaged(page);
      await page.evaluate((label) => {
        window.__workspaceEmit({ label, mode: "paused", revision: 100, inFlight: 0, needsObservation: true, ownerTerm: null });
        setActiveTab(`browser:${label}`);
        const host = groupOf(tabs.find((one) => one.label === label).pane)?.browserView;
        const select = host?.querySelector(".browser-workspace-owner-select");
        if (select) { select.value = window.__WORKSPACE__.actorTerm; select.dispatchEvent(new Event("change", { bubbles: true })); }
        const actor = tabs.find((one) => one.kind === "term" && String(one.term) === window.__WORKSPACE__.actorTerm);
        if (actor) dropTab(actor.id);
        const tab = tabs.find((one) => one.label === label);
        paintBrowserView(tab);
        window.__workspaceClick(label, "agent");
      }, label);
      await page.waitForTimeout(60);
      const gone = await page.evaluate((label) => ({
        modes: window.__WORKSPACE__.modes.length,
        disabled: groupOf(tabs.find((one) => one.label === label).pane)?.browserView?.querySelector(".browser-workspace-agent")?.disabled,
      }), label);
      ok("a selected agent that closes cannot receive control", gone.modes === 0 && gone.disabled === true, JSON.stringify(gone));
      ok("workspace stale-owner handling raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }

  {
    const { page, faults } = await standWorkspace(browser, origin);
    try {
      const label = await openManaged(page);
      await page.evaluate((label) => window.__workspaceEmit({
        label, mode: "agent", revision: 110, inFlight: 0, needsObservation: false, snapshotSupported: false,
      }), label);
      await observe(page, label);
      await page.waitForTimeout(1100);
      const unsupported = await page.evaluate((label) => {
        const host = groupOf(tabs.find((one) => one.label === label).pane)?.browserView;
        return {
          captures: window.__WORKSPACE__.snapshots.length,
          humanDisabled: host?.querySelector(".browser-workspace-human")?.disabled,
          pauseDisabled: host?.querySelector(".browser-workspace-pause")?.disabled,
          note: host?.querySelector(".browser-workspace-preview-note")?.textContent,
        };
      }, label);
      ok("unsupported snapshots schedule no capture and retain safe control actions",
        unsupported.captures === 0 && unsupported.humanDisabled === false && unsupported.pauseDisabled === false
          && /지원하지 않/.test(unsupported.note ?? ""), JSON.stringify(unsupported));
      await page.evaluate((label) => window.__workspaceClick(label, "human"), label);
      await page.waitForTimeout(80);
      const direct = await page.evaluate((label) => ({
        last: window.__WORKSPACE__.placed.filter((one) => one.label === label).at(-1),
        captures: window.__WORKSPACE__.snapshots.length,
      }), label);
      ok("unsupported watching still permits confirmed native human control", direct.last?.shown === true && direct.captures === 0, JSON.stringify(direct));
      ok("workspace unsupported-snapshot handling raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }

  {
    const { page, faults } = await standWorkspace(browser, origin);
    try {
      const label = await openManaged(page);
      await page.evaluate((label) => {
        window.__workspaceEmit({ label, mode: "agent", revision: 120, inFlight: 0, needsObservation: false });
        window.__ANSWER__.browser_snapshot = () => { window.__WORKSPACE__.snapshots.push({ label }); throw new Error("capture_unavailable"); };
      }, label);
      await observe(page, label);
      const failed = await page.evaluate((label) => {
        const host = groupOf(tabs.find((one) => one.label === label).pane)?.browserView;
        return {
          status: host?.querySelector(".browser-workspace-status")?.textContent,
          note: host?.querySelector(".browser-workspace-preview-note")?.textContent,
          humanDisabled: host?.querySelector(".browser-workspace-human")?.disabled,
          pauseDisabled: host?.querySelector(".browser-workspace-pause")?.disabled,
          count: window.__WORKSPACE__.snapshots.length,
        };
      }, label);
      ok("capture failure leaves the confirmed control state and safety buttons available",
        /에이전트 제어/.test(failed.status ?? "") && /가져오지 못/.test(failed.note ?? "")
          && failed.humanDisabled === false && failed.pauseDisabled === false, JSON.stringify(failed));
      await page.waitForTimeout(1100);
      const bounded = await page.evaluate(() => window.__WORKSPACE__.snapshots.length);
      ok("a failed capture stops automatic retries until an explicit retry", bounded === failed.count && bounded === 1, String(bounded));
      await page.evaluate((label) => window.__workspaceClick(label, "pause"), label);
      await page.waitForTimeout(50);
      await page.evaluate((label) => window.__workspaceClick(label, "human"), label);
      await page.waitForTimeout(80);
      const safe = await page.evaluate((label) => ({
        modes: window.__WORKSPACE__.modes.map((one) => one.mode),
        last: window.__WORKSPACE__.placed.filter((one) => one.label === label).at(-1),
      }), label);
      ok("pause and human handoff remain usable after a capture error", safe.modes.join(",") === "paused,human" && safe.last?.shown === true, JSON.stringify(safe));
      ok("workspace capture-error recovery raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }

  {
    const { page, faults } = await standWorkspace(browser, origin);
    try {
      const label = await page.evaluate(() => openBrowserTab("https://example.com/new-owner", { agentWorkspace: true }));
      await page.evaluate((label) => window.__workspaceEmit({ label, mode: "paused", revision: 130, inFlight: 0, needsObservation: true, ownerTerm: null }), label);
      const suggested = await page.evaluate((label) => {
        const tab = tabs.find((one) => one.label === label);
        const host = groupOf(tab.pane)?.browserView;
        return {
          actor: window.__WORKSPACE__.actorTerm,
          choice: host?.querySelector(".browser-workspace-owner-select")?.value,
          owner: tab.workspace?.ownerTerm,
          modes: window.__WORKSPACE__.modes.length,
        };
      }, label);
      ok("a new task can suggest the active agent without assigning or starting it", suggested.choice === suggested.actor
        && suggested.owner === null && suggested.modes === 0, JSON.stringify(suggested));
      await page.evaluate((label) => window.__workspaceEmit({ label, mode: "human", revision: 131, inFlight: 0, needsObservation: false }), label);
      await observe(page, label);
      const next = await page.evaluate(async (label) => {
        const tab = tabs.find((one) => one.label === label);
        await reopenBrowserPane(tab);
        window.__workspaceEmit({ label: tab.label, mode: "paused", revision: 1, inFlight: 0, needsObservation: true, ownerTerm: null });
        return { label: tab.label, choice: tab.workspaceOwnerChoice, mode: tab.workspace?.mode };
      }, label);
      ok("recreating a task requires owner confirmation again", next.label !== label && next.choice === "" && next.mode === "paused", JSON.stringify(next));
      ok("workspace new-owner suggestions raise no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }

  {
    const { page, faults } = await standWorkspace(browser, origin);
    try {
      const label = await openManaged(page);
      await page.evaluate((label) => {
        setActiveTab(`browser:${label}`);
        for (const hear of window.__LISTENERS__["browser:workspace"] ?? []) hear({ payload: {
          label, mode: "human", revision: 140, inFlight: 0, needsObservation: false,
        } });
        window.__workspaceClick(label, "watch");
      }, label);
      const missing = await page.evaluate((label) => ({
        shown: window.__WORKSPACE__.placed.filter((one) => one.label === label && one.shown).length,
        disabled: groupOf(tabs.find((one) => one.label === label).pane)?.browserView?.querySelector(".browser-workspace-human")?.disabled,
      }), label);
      ok("missing owner and capture capabilities cannot expose a managed human page", missing.shown === 0 && missing.disabled === true, JSON.stringify(missing));
      await page.evaluate((label) => window.__workspaceEmit({ label, mode: "human", revision: 141, inFlight: 0, needsObservation: false, ownerTerm: "codex" }), label);
      await page.waitForTimeout(30);
      const invalidOwner = await page.evaluate((label) => window.__WORKSPACE__.placed.filter((one) => one.label === label && one.shown).length, label);
      ok("an agent name cannot be mistaken for a terminal owner ID", invalidOwner === 0, String(invalidOwner));
      await page.evaluate((label) => window.__workspaceEmit({ label, mode: "agent", revision: 142, inFlight: 0, needsObservation: false, snapshotSupported: "true" }), label);
      await page.waitForTimeout(100);
      const invalidCapability = await page.evaluate(() => window.__WORKSPACE__.snapshots.length);
      ok("an unknown capture capability starts no capture", invalidCapability === 0, String(invalidCapability));
      ok("workspace malformed-capability handling raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }

  {
    const { page, faults } = await standWorkspace(browser, origin);
    try {
      const initialZooms = await page.evaluate(() => window.__COUNTS__.browser_zoom ?? 0);
      const label = await openManaged(page);
      const openedZooms = await page.evaluate(() => window.__COUNTS__.browser_zoom ?? 0);
      ok("managed creation leaves initial zoom to the backend", openedZooms === initialZooms, JSON.stringify({ initialZooms, openedZooms }));
      await page.evaluate((label) => {
        window.__WORKSPACE__.mutators = [];
        for (const command of ["browser_history", "browser_reload", "browser_stop", "browser_devtools", "browser_find", "browser_find_clear", "browser_zoom", "browser_grab_arm", "browser_grab_disarm"]) {
          window.__ANSWER__[command] = (args) => {
            window.__WORKSPACE__.mutators.push({ command, ...args });
            return command === "browser_find" ? { count: 1, index: 1 } : null;
          };
        }
        window.__workspaceEmit({ label, mode: "human", revision: 150, inFlight: 0, needsObservation: false });
      }, label);
      await observe(page, label);
      await page.evaluate(async (label) => {
        const tab = tabs.find((one) => one.label === label);
        const host = groupOf(tab.pane)?.browserView;
        host?.querySelector(".browser-back")?.click();
        host?.querySelector(".browser-forward")?.click();
        tab.loading = false;
        paintBrowserView(tab);
        host?.querySelector(".browser-reload")?.click();
        tab.loading = true;
        paintBrowserView(tab);
        host?.querySelector(".browser-reload")?.click();
        host?.querySelector(".browser-devtools")?.click();
        await runBrowserFind(tab, "example", true);
        dropBrowserFindMarks(tab);
        browserZoomStep(tab, 1);
        toggleBrowserGrab(host);
        stopBrowserGrab(true);
        window.__WORKSPACE__.oldMenu = browserMenuItems(tab, {});
      }, label);
      const shortcut = await page.evaluate((primary) => {
        const before = window.__WORKSPACE__.mutators.length;
        document.dispatchEvent(new KeyboardEvent("keydown", { ...primary, key: "r", code: "KeyR", bubbles: true, cancelable: true }));
        return window.__WORKSPACE__.mutators.slice(before);
      }, PRIMARY_EVENT);
      ok("the managed human reload shortcut carries the current revision", shortcut.length === 1
        && shortcut[0]?.command === "browser_reload" && shortcut[0]?.expectedRevision === 150, JSON.stringify(shortcut));
      const calls = await page.evaluate(() => window.__WORKSPACE__.mutators);
      const expected = ["browser_history", "browser_reload", "browser_stop", "browser_devtools", "browser_find", "browser_find_clear", "browser_zoom", "browser_grab_arm", "browser_grab_disarm"];
      ok("every managed toolbar mutator carries the observed human revision", expected.every((name) => calls.some((one) => one.command === name))
        && calls.every((one) => one.expectedRevision === 150), JSON.stringify(calls));
      const away = await page.evaluate(({ label, primary }) => {
        window.__workspaceEmit({ label, mode: "agent", revision: 151, inFlight: 0, needsObservation: true });
        const before = window.__WORKSPACE__.mutators.length;
        document.dispatchEvent(new KeyboardEvent("keydown", { ...primary, key: "r", code: "KeyR", bubbles: true, cancelable: true }));
        return { added: window.__WORKSPACE__.mutators.length - before, total: window.__WORKSPACE__.mutators.length };
      }, { label, primary: PRIMARY_EVENT });
      ok("the reload shortcut sends no page mutation while watching", away.added === 0, JSON.stringify(away));
      await page.evaluate((label) => {
        window.__workspaceEmit({ label, mode: "human", revision: 152, inFlight: 0, needsObservation: false });
        for (const row of window.__WORKSPACE__.oldMenu ?? []) row.run?.();
      }, label);
      const after = await page.evaluate(() => window.__WORKSPACE__.mutators.length);
      ok("a menu from an earlier human revision cannot execute after a round trip", after === away.total, JSON.stringify({ before: away.total, after }));
      const pausedShortcut = await page.evaluate(({ label, primary }) => {
        window.__workspaceEmit({ label, mode: "paused", revision: 153, inFlight: 0, needsObservation: true });
        const before = window.__WORKSPACE__.mutators.length;
        document.dispatchEvent(new KeyboardEvent("keydown", { ...primary, key: "r", code: "KeyR", bubbles: true, cancelable: true }));
        return window.__WORKSPACE__.mutators.length - before;
      }, { label, primary: PRIMARY_EVENT });
      ok("the reload shortcut sends no mutation while paused", pausedShortcut === 0, String(pausedShortcut));
      const pendingShortcut = await page.evaluate(({ label, primary }) => {
        window.__WORKSPACE__.holdMode = true;
        window.__workspaceClick(label, "human");
        const before = window.__WORKSPACE__.mutators.length;
        document.dispatchEvent(new KeyboardEvent("keydown", { ...primary, key: "r", code: "KeyR", bubbles: true, cancelable: true }));
        return window.__WORKSPACE__.mutators.length - before;
      }, { label, primary: PRIMARY_EVENT });
      ok("the reload shortcut sends no mutation during handoff", pendingShortcut === 0, String(pendingShortcut));
      await page.waitForTimeout(30);
      await page.evaluate((label) => window.__WORKSPACE__.releaseMode?.({ label, mode: "human", revision: 154, inFlight: 0, needsObservation: false }), label);
      const person = await page.evaluate(async () => {
        const label = await openBrowserTab("https://example.com/person-mutators");
        const tab = tabs.find((one) => one.label === label);
        const host = groupOf(tab.pane)?.browserView;
        host?.querySelector(".browser-back")?.click();
        browserZoomStep(tab, 1);
        return label;
      });
      const ordinary = await page.evaluate((label) => window.__WORKSPACE__.mutators.filter((one) => one.label === label), person);
      ok("ordinary user mutators preserve their existing argument contract", ordinary.length >= 3
        && ordinary.every((one) => one.expectedRevision === undefined), JSON.stringify(ordinary));
      const missing = await page.evaluate(() => {
        const action = [...BOUND.values()].find((one) => one.id === "browser.reload");
        for (const tab of [...tabs].filter((one) => one.kind === "browser")) closeTab(tab.id);
        const before = window.__WORKSPACE__.mutators.length;
        let error = null;
        try { action?.run(); } catch (caught) { error = String(caught); }
        return { exists: !!action, error, added: window.__WORKSPACE__.mutators.length - before };
      });
      ok("a reload action whose browser closed does nothing", missing.exists && missing.error === null && missing.added === 0, JSON.stringify(missing));
      ok("workspace mutator revision handling raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }

  {
    const { page, faults } = await standWorkspace(browser, origin);
    try {
      const label = await openManaged(page);
      await page.evaluate((label) => {
        window.__workspaceEmit({ label, mode: "agent", revision: 200, inFlight: 0, needsObservation: false });
        window.__WORKSPACE__.holdClose = true;
        for (let repeat = 0; repeat < 2; repeat += 1) {
          for (const hear of window.__LISTENERS__["browser:agent-close"] ?? []) hear({ payload: { label, revision: 200 } });
        }
      }, label);
      await page.waitForTimeout(40);
      const pending = await page.evaluate((label) => ({
        exists: tabs.some((one) => one.label === label),
        stored: browserOpenTabRecords().some((one) => one.url === "https://example.com/workspace"),
        calls: window.__WORKSPACE__.closeCalls,
        closed: window.__WORKSPACE__.closed,
      }), label);
      ok("agent close waits for native acknowledgment and coalesces duplicate events", pending.exists && pending.stored
        && pending.calls.length === 1 && pending.calls[0]?.expectedRevision === 200 && pending.closed.length === 0, JSON.stringify(pending));
      await page.evaluate(() => window.__WORKSPACE__.releaseClose?.());
      await page.waitForTimeout(60);
      const done = await page.evaluate((label) => ({
        exists: tabs.some((one) => one.label === label),
        stored: browserOpenTabRecords().some((one) => one.url === "https://example.com/workspace"),
        calls: window.__WORKSPACE__.closeCalls,
      }), label);
      ok("a confirmed agent close removes the tab and record without an unversioned second close",
        !done.exists && !done.stored && done.calls.length === 1 && done.calls[0]?.expectedRevision === 200, JSON.stringify(done));
      ok("workspace confirmed-close handling raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }

  {
    const { page, faults } = await standWorkspace(browser, origin);
    try {
      const label = await openManaged(page);
      await page.evaluate((label) => window.__workspaceEmit({ label, mode: "agent", revision: 210, inFlight: 0, needsObservation: false }), label);
      await observe(page, label);
      await page.evaluate((label) => {
        window.__WORKSPACE__.holdClose = true;
        for (const hear of window.__LISTENERS__["browser:agent-close"] ?? []) hear({ payload: { label, revision: 210 } });
      }, label);
      await page.waitForTimeout(30);
      await page.evaluate(() => window.__WORKSPACE__.rejectClose?.(new Error("stale_close")));
      await page.waitForTimeout(50);
      const rejected = await page.evaluate((label) => ({
        exists: tabs.some((one) => one.label === label), closed: window.__WORKSPACE__.closed,
        note: groupOf(tabs.find((one) => one.label === label)?.pane)?.browserView?.querySelector(".browser-agent-close-note")?.textContent,
      }), label);
      ok("a rejected native close retains the tab and shows an inline refusal", rejected.exists && rejected.closed.length === 0
        && /탭을 유지/.test(rejected.note ?? ""), JSON.stringify(rejected));
      await page.evaluate((label) => {
        window.__WORKSPACE__.holdClose = false;
        window.__WORKSPACE__.unknownCloseAck = true;
        for (const hear of window.__LISTENERS__["browser:agent-close"] ?? []) hear({ payload: { label, revision: 210 } });
      }, label);
      await page.waitForTimeout(50);
      const unknown = await page.evaluate((label) => tabs.some((one) => one.label === label), label);
      ok("an unknown native close acknowledgment cannot remove a tab", unknown, String(unknown));
      await page.evaluate((label) => {
        window.__workspaceEmit({ label, mode: "human", revision: 211, inFlight: 0, needsObservation: false });
        const before = window.__WORKSPACE__.closeCalls.length;
        for (const hear of window.__LISTENERS__["browser:agent-close"] ?? []) hear({ payload: { label, revision: 210 } });
        window.__WORKSPACE__.staleCloseAdded = window.__WORKSPACE__.closeCalls.length - before;
      }, label);
      const stale = await page.evaluate((label) => ({ exists: tabs.some((one) => one.label === label), added: window.__WORKSPACE__.staleCloseAdded }), label);
      ok("a close event from before human handoff sends no close request", stale.exists && stale.added === 0, JSON.stringify(stale));
      ok("workspace close-refusal handling raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }

  {
    const { page, faults } = await standWorkspace(browser, origin);
    try {
      const label = await openManaged(page);
      const pinned = await page.evaluate((label) => {
        window.__workspaceEmit({ label, mode: "agent", revision: 220, inFlight: 0, needsObservation: false });
        const tab = tabs.find((one) => one.label === label);
        tab.pinned = true;
        const input = document.createElement("input");
        input.id = "agent-close-focus";
        document.body.appendChild(input);
        input.focus();
        for (const hear of window.__LISTENERS__["browser:agent-close"] ?? []) hear({ payload: { label, revision: 220 } });
        return { exists: tabs.includes(tab), pinned: tab.pinned, calls: window.__WORKSPACE__.closeCalls.length, focus: document.activeElement?.id };
      }, label);
      ok("a pinned task rejects agent close before native input and keeps the typing receiver", pinned.exists && pinned.pinned
        && pinned.calls === 0 && pinned.focus === "agent-close-focus", JSON.stringify(pinned));
      await page.evaluate((label) => {
        const tab = tabs.find((one) => one.label === label);
        tab.pinned = false;
        for (const hear of window.__LISTENERS__["browser:agent-close"] ?? []) hear({ payload: { label } });
        for (const hear of window.__LISTENERS__["browser:agent-close"] ?? []) hear({ payload: { label: "browser-999", revision: 220 } });
      }, label);
      const missing = await page.evaluate((label) => ({ exists: tabs.some((one) => one.label === label), calls: window.__WORKSPACE__.closeCalls.length }), label);
      ok("missing close revisions and unknown origins cannot close another tab", missing.exists && missing.calls === 0, JSON.stringify(missing));
      ok("workspace pinned-close handling raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }

  {
    const { page, faults } = await standWorkspace(browser, origin);
    try {
      const old = await page.evaluate(async () => {
        const label = await openBrowserTab("https://example.com/close-reopen");
        window.__WORKSPACE__.holdClose = true;
        const revision = Number(label.slice("browser-".length));
        for (const hear of window.__LISTENERS__["browser:agent-close"] ?? []) hear({ payload: { label, revision } });
        return label;
      });
      await page.waitForTimeout(30);
      const next = await page.evaluate(async (old) => {
        const tab = tabs.find((one) => one.label === old);
        if (!tab) return { label: null };
        const finishOld = window.__WORKSPACE__.releaseClose;
        window.__WORKSPACE__.holdClose = false;
        await reopenBrowserPane(tab);
        finishOld?.();
        return { label: tab.label };
      }, old);
      await page.waitForTimeout(60);
      const renamed = await page.evaluate((label) => ({
        exists: tabs.some((one) => one.label === label),
        closed: window.__WORKSPACE__.closed.includes(label),
      }), next.label);
      ok("a late close acknowledgment never removes a replacement label", next.label !== null && next.label !== old
        && renamed.exists && !renamed.closed, JSON.stringify({ old, next, renamed }));
      ok("workspace replacement-close handling raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }

  {
    const { page, faults } = await standWorkspace(browser, origin);
    try {
      await page.evaluate(() => {
        window.__WORKSPACE__.holdOpen = true;
        for (const hear of window.__LISTENERS__["browser:agent-open"] ?? []) hear({ payload: { label: "browser-901", url: "https://example.com/cancel-open" } });
      });
      await page.waitForFunction(() => typeof window.__WORKSPACE__.releaseOpen === "function");
      await page.evaluate(() => {
        for (const hear of window.__LISTENERS__["browser:agent-close"] ?? []) hear({ payload: { label: "browser-901", revision: 300 } });
        window.__WORKSPACE__.releaseOpen();
      });
      await page.waitForTimeout(100);
      const canceled = await page.evaluate(() => ({
        exists: tabs.some((one) => one.label === "browser-901"), calls: window.__WORKSPACE__.closeCalls,
      }));
      ok("a canceled pending open uses its captured close epoch and creates no phantom tab", !canceled.exists
        && canceled.calls.length === 1 && canceled.calls[0]?.label === "browser-901" && canceled.calls[0]?.expectedRevision === 300, JSON.stringify(canceled));
      ok("workspace canceled-open handling raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }

  {
    const { page, faults } = await standWorkspace(browser, origin, { actorName: "fixture-runner" });
    try {
      const label = await openManaged(page);
      await page.evaluate((label) => {
        window.__workspaceEmit({ label, mode: "paused", revision: 400, inFlight: 0, needsObservation: true, ownerTerm: null });
        setActiveTab(`browser:${label}`);
        const host = groupOf(tabs.find((one) => one.label === label).pane)?.browserView;
        const select = host?.querySelector(".browser-workspace-owner-select");
        if (select) { select.value = window.__WORKSPACE__.actorTerm; select.dispatchEvent(new Event("change", { bubbles: true })); }
        window.__workspaceClick(label, "agent");
      }, label);
      await page.waitForTimeout(60);
      await page.evaluate((label) => window.__workspaceClick(label, "pause"), label);
      await page.waitForTimeout(60);
      await page.evaluate((label) => window.__workspaceClick(label, "human"), label);
      await page.waitForTimeout(60);
      const generic = await page.evaluate((label) => ({
        actor: window.__WORKSPACE__.actorTerm, calls: window.__WORKSPACE__.modes,
        last: window.__WORKSPACE__.placed.filter((one) => one.label === label).at(-1),
      }), label);
      ok("an actor outside fixed provider names uses the same explicit ownership and pause contract", generic.calls.length === 3
        && generic.calls[0]?.ownerTerm === generic.actor && generic.calls[0]?.expectedRevision === 400
        && generic.calls[1]?.mode === "paused" && generic.calls[1]?.expectedRevision === 401
        && generic.calls[2]?.mode === "human" && generic.calls[2]?.expectedRevision === 402 && generic.last?.shown === true, JSON.stringify(generic));
      ok("workspace generic-actor handling raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }

  {
    const { page, faults } = await standWorkspace(browser, origin);
    try {
      const label = await openManaged(page);
      await page.evaluate((label) => {
        window.__workspaceEmit({ label, mode: "paused", revision: 500, inFlight: 0, needsObservation: true, ownerTerm: null });
        setActiveTab(`browser:${label}`);
        const host = groupOf(tabs.find((one) => one.label === label).pane)?.browserView;
        const select = host?.querySelector(".browser-workspace-owner-select");
        if (select) { select.value = window.__WORKSPACE__.actorTerm; select.dispatchEvent(new Event("change", { bubbles: true })); }
        window.__workspaceClick(label, "agent");
      }, label);
      await page.waitForFunction((label) => {
        const tab = tabs.find((one) => one.label === label);
        return tab?.workspace?.mode === "agent" && !tab.workspacePending;
      }, label);
      await page.evaluate((label) => window.__workspaceClick(label, "pause"), label);
      await page.waitForFunction((label) => {
        const tab = tabs.find((one) => one.label === label);
        return tab?.workspace?.mode === "paused" && !tab.workspacePending;
      }, label);
      await page.evaluate((label) => window.__workspaceClick(label, "human"), label);
      await page.waitForFunction((label) => {
        const tab = tabs.find((one) => one.label === label);
        const last = window.__WORKSPACE__.placed.filter((one) => one.label === label).at(-1);
        return tab?.workspace?.mode === "human" && !tab.workspacePending && last?.shown === true;
      }, label);
      const granted = await page.evaluate((label) => ({
        workspace: tabs.find((one) => one.label === label).workspace,
        last: window.__WORKSPACE__.placed.filter((one) => one.label === label).at(-1),
        modes: window.__WORKSPACE__.modes.map((request) => request.mode),
      }), label);
      await page.evaluate(({ label, revision }) => window.__workspaceEmit({
        label, mode: "human", revision, inFlight: 1, needsObservation: false,
      }), { label, revision: granted.workspace?.revision });
      await page.waitForTimeout(80);
      const own = await page.evaluate((label) => ({
        last: window.__WORKSPACE__.placed.filter((one) => one.label === label).at(-1),
        error: tabs.find((one) => one.label === label).workspaceError,
        addressReadOnly: groupOf(tabs.find((one) => one.label === label).pane)?.browserView?.querySelector(".browser-address")?.readOnly,
      }), label);
      ok("the person's own request in flight keeps the human view placed and usable",
        granted.modes.join(",") === "agent,paused,human"
          && granted.workspace?.mode === "human" && granted.last?.shown === true && own.last?.shown === true
          && own.error === null && own.addressReadOnly === false, JSON.stringify({ granted, own }));
      await page.evaluate(({ label, revision }) => window.__workspaceEmit({
        label, mode: "paused", revision: revision + 1, inFlight: 1, needsObservation: true,
      }), { label, revision: granted.workspace?.revision });
      await page.waitForTimeout(80);
      const paused = await page.evaluate((label) => window.__WORKSPACE__.placed.filter((one) => one.label === label).at(-1), label);
      ok("a paused state that claims input in flight still hides the native pane", paused?.shown === false, JSON.stringify(paused));
      ok("workspace own-request handling raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }

  {
    const { page, faults } = await standWorkspace(browser, origin);
    try {
      await page.evaluate(() => {
        const mock = window.__WORKSPACE__;
        mock.holdOpen = true;
        mock.originalWorktree = activeWorktreePath;
        mock.openPromise = openBrowserTab("https://example.com/cancel-without-epoch", {
          agentWorkspace: true, expectedWorktree: activeWorktreePath,
        });
      });
      await page.waitForFunction(() => typeof window.__WORKSPACE__.releaseOpen === "function");
      const unknown = await page.evaluate(async () => {
        const mock = window.__WORKSPACE__;
        activeWorktreePath = "/tmp/zerocode-window-test/other";
        mock.releaseOpen();
        await mock.openPromise;
        activeWorktreePath = mock.originalWorktree;
        return { calls: mock.closeCalls, exists: tabs.some((one) => one.kind === "browser") };
      });
      ok("managed open cancellation never guesses an absent creation epoch or sends user close", unknown.calls.length === 0 && !unknown.exists, JSON.stringify(unknown));
      ok("workspace unknown-open-epoch handling raises no page errors", faults.length === 0, faults.join("\n"));
    } finally { await page.close(); }
  }
}
