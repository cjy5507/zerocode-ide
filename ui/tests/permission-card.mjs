import { openWindowTestPage } from "./window-boot.mjs";

// The permission refusal card can be put down, stays down while the same row
// is still the one missing (an agent retrying a denied screenshot every few
// seconds must not raise it again, nor spawn a helper probe per retry), and
// comes back when a different permission goes missing.
export async function exercisePermissionCard() {
  const settle = (ms = 80) => new Promise((done) => setTimeout(done, ms));
  const states = [{ id: "accessibility", status: "granted" }, { id: "screenshots", status: "not-granted" }];
  let probes = 0;
  window.__ANSWER__.computer_use_permission_status = () => { probes += 1; return { platform: "darwin", helper_app_path: "/test/Helper.app", permissions: states }; };
  const deny = () => { for (const handler of window.__LISTENERS__["computer:permission-denied"] ?? []) handler({ payload: {} }); };
  const card = () => el("computer-permission-recovery");
  const seen = {};
  deny(); await settle();
  seen.shown = !card().hidden;
  // t-9719: the card's line is the palette's rule — the pixel its literal drew.
  seen.ruledByTheToken = getComputedStyle(card()).borderTopWidth === "1px"
    && getComputedStyle(document.documentElement).getPropertyValue("--rule-width").trim() === "1px";
  el("computer-permission-recovery-close").click(); await settle();
  seen.closed = card().hidden;
  const probesAtClose = probes;
  deny(); await settle(); deny(); await settle();
  seen.retriesDoNotProbe = probes === probesAtClose;
  await settle(COMPUTER_PERMISSION_RECHECK_MIN_MS + 100); deny(); await settle();
  seen.staysDownForTheSameRow = card().hidden && probes === probesAtClose + 1;
  states[1].status = "granted"; states[0].status = "not-granted";
  await settle(COMPUTER_PERMISSION_RECHECK_MIN_MS + 100); deny(); await settle();
  seen.returnsForAnotherRow = !card().hidden && el("computer-permission-recovery-text").textContent.includes("손쉬운 사용");
  states[0].status = "granted";
  el("computer-permission-recovery-check").click(); await settle();
  seen.clearsWhenGranted = card().hidden;
  delete window.__ANSWER__.computer_use_permission_status;
  return seen;
}

export async function testPermissionCard(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const seen = await page.evaluate(exercisePermissionCard);
    ok(
      "the permission card can be put down, stays down while the same row is missing without probing per retry, and returns for another row",
      seen.shown && seen.closed && seen.retriesDoNotProbe && seen.staysDownForTheSameRow && seen.returnsForAnotherRow && seen.clearsWhenGranted,
      JSON.stringify(seen),
    );
    ok("the permission card draws its line with the palette's rule, the pixel it drew before", seen.ruledByTheToken, JSON.stringify(seen));
    ok("the permission card raised no renderer errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}
