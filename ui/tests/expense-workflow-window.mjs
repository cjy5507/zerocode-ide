/* Real window chrome around an isolated synthetic browser guest. */
import { resolve } from 'node:path';
import { createWindowServer, launchWindowBrowser, openWindowTestPage } from './window-boot.mjs';

export async function openExpenseWindow() {
  const { files, origin } = await createWindowServer();
  const browser = await launchWindowBrowser();
  const { page: windowPage } = await openWindowTestPage(browser, origin);
  await windowPage.waitForFunction(() => tabs.some((tab) => tab.kind === 'term'));
  let guest = await browser.newPage({ viewport: { width: 900, height: 900 } });
  let currentLabel = 'browser-801';
  const fixtureUrl = `${origin}/tests/fixtures/expense-workflow/index.html#edit`;
  await windowPage.evaluate((url) => {
    const tab = tabs.find((one) => one.kind === 'term');
    const term = activePaneOf(tab) ?? tab.term;
    paneSessions.delete(term);
    paneAgents.set(term, 'codex');
    window.__ANSWER__.open_browser_pane = ({ label }) => label;
    window.__ANSWER__.browser_place = () => null;
    window.__ANSWER__.browser_zoom = () => null;
    window.__EXPENSE_LABEL__ = 'browser-801';
    window.__ANSWER__.get_browser_workspaces = () => [{ label: window.__EXPENSE_LABEL__, mode: 'agent', revision: 1, ownerTerm: String(term), inFlight: 0, needsObservation: false, snapshotSupported: true }];
    for (const hear of window.__LISTENERS__['browser:agent-open'] ?? []) hear({ payload: { label: 'browser-801', url, term } });
  }, fixtureUrl);
  await windowPage.waitForFunction(() => tabs.some((tab) => tab.label === 'browser-801'));

  return { browser, get guest() { return guest; }, fixtureUrl,
    async reopenGuest(url, label) {
      await guest.close();
      guest = await browser.newPage({ viewport: { width: 900, height: 900 } });
      await guest.goto(url);
      currentLabel = label;
      await windowPage.evaluate(({ label, url }) => {
        window.__EXPENSE_LABEL__ = label;
        for (const hear of window.__LISTENERS__['browser:agent-open'] ?? []) hear({ payload: { label, url } });
      }, { label, url });
      await windowPage.waitForFunction((label) => tabs.some((tab) => tab.label === label), label);
    },
    async snapshot(out, name = 'window-complete') {
      const frame = `data:image/png;base64,${(await guest.screenshot()).toString('base64')}`;
      await windowPage.evaluate(({ frame, url, label }) => {
        for (const hear of window.__LISTENERS__['browser:nav'] ?? []) hear({ payload: { label, url, state: 'finished' } });
        window.__ANSWER__.browser_snapshot = () => frame;
        setActiveTab(`browser:${label}`);
        const tab = tabs.find((one) => one.label === label);
        const host = groupOf(tab.pane).browserView;
        if (!tab.workspaceWatching) host.querySelector('.browser-workspace-watch').click();
      }, { frame, url: guest.url(), label: currentLabel });
      await windowPage.waitForFunction((label) => {
        const tab = tabs.find((one) => one.label === label);
        return tab && groupOf(tab.pane).browserView.querySelector('.browser-workspace-frame')?.getAttribute('src')?.startsWith('data:image/png');
      }, currentLabel);
      await windowPage.screenshot({ path: resolve(out, `${name}.png`) });
    },
    async close() { await browser.close(); await new Promise((done) => files.close(done)); },
  };
}
