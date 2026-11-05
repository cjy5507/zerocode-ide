import { openWindowTestPage } from "./window-boot.mjs";

/* Idle sleep (low-spec 6, in session): on a machine with little memory a tab
 * behind the workspace in front whose conversations have all rested for the
 * policy's while becomes the stored tab a restart would have left — its record
 * kept, its shells closed by the backend (`sleep_terminal_tab`) — and says
 * 「잠듦」 on the strip until somebody looks at it, when it wakes by the stored
 * tab's own road with its last screen addressed. Every pane that has to stay
 * awake keeps its whole tab awake: a turn going, a rest too short, words typed
 * after the turn ended, the workspace in front, a zo helper's pane, a ledger
 * seat, a conversation that cannot be woken, and a tab the backend refused.
 *
 * Run in the page (`page.evaluate`), so it holds no reference outside itself. */
export async function exerciseIdleSleep() {
  const AFTER = 60_000;
  const BEHIND = "/tmp/zerocode-idle-sleep/behind";
  const FRONT = activeWorktreePath;
  const seen = {};
  const sleeps = [];
  const refuse = new Set();
  const resumed = [];
  let nextTerm = 9700;
  let nextId = 700;
  const priorAnswers = { ...window.__ANSWER__ };
  const priorRole = paneRole;
  window.__ANSWER__.sleep_terminal_tab = (args) => {
    sleeps.push(JSON.parse(JSON.stringify(args)));
    if (refuse.has(args.id)) throw "running zsh -c sleep 600";
    // The backend closes the shells and says so before it answers.
    for (const term of args.terms) {
      for (const heard of window.__LISTENERS__["term:exited"] ?? []) heard({ payload: { term } });
    }
    return null;
  };
  window.__ANSWER__.resume_session = (args) => {
    resumed.push({ agent: args.agent, id: args.session.id, restore: args.restore ?? null, worktree: args.worktree ?? null });
    return { term: (nextTerm += 1), standing: false };
  };
  const open = async ({ worktree = BEHIND, state = "done", ago = AFTER + 5_000, resumable = true } = {}) => {
    const term = await openTermTab({ placement: "tab" });
    const tab = tabOfTerm(term);
    tab.worktree = worktree;
    tab.storedId = (nextId += 1);
    paneSessions.set(term, { agent: "claude", resumable, session: { key: "session_id", id: `s-${term}` } });
    hookStates.set(term, state);
    hookStamps.set(term, Date.now() - ago);
    return { term, tab };
  };
  const awake = (one) => !one.tab.asleep && paneLeaves(one.tab.layout).includes(one.term);

  const resting = await open();
  const working = await open({ state: "working" });
  const recent = await open({ ago: 5_000 });
  const typed = await open();
  noteTypedInto(typed.term);
  const front = await open({ worktree: FRONT });
  const helper = await open();
  paneHelpers.set(helper.term, "zo-helper-1");
  const seat = await open();
  paneRole = (term) => (term === seat.term ? { kind: "coordinator", seats: [{ run: "run-1", name: "ZO" }] } : priorRole(term));
  const unresumable = await open({ resumable: false });
  const refused = await open();
  refuse.add(refused.tab.storedId);

  idleSleepAfterMs = AFTER;
  await idleSleepBeat();
  seen.asked = sleeps.map((one) => one.id);
  seen.wantAsked = [resting.tab.storedId, refused.tab.storedId];
  seen.call = sleeps[0] ?? null;
  seen.wantCall = { worktree: BEHIND, id: resting.tab.storedId, terms: [resting.term] };
  const record = resting.tab.asleep ?? null;
  seen.record = record && {
    id: record.id,
    agent: record.agents?.[0]?.agent ?? null,
    session: record.agents?.[0]?.id ?? null,
    terms: "terms" in record,
  };
  seen.wantSession = `s-${resting.term}`;
  seen.slept = resting.tab.slept === true && resting.tab.layout === null && tabs.includes(resting.tab);
  seen.stayed = Object.fromEntries(
    Object.entries({ working, recent, typed, front, helper, seat, unresumable, refused }).map(([name, one]) => [name, awake(one)]),
  );
  seen.refusedHeld = (sleepRefusedUntil.get(refused.tab.id) ?? 0) > Date.now();

  // The strip says the tab is asleep, painted where every tab's words are.
  const node = document.createElement("div");
  node.innerHTML = '<span class="tab-dot"></span><span class="tab-kind"></span><span class="tab-label"></span><span class="tab-age"></span><button class="tab-close"></button>';
  paintTabNode(node, resting.tab);
  seen.label = node.querySelector(".tab-label").textContent;
  seen.sleptWord = t("tab.slept", "잠듦");

  // A second beat asks nothing again: the one asleep is asleep, the refused waits out its while.
  await idleSleepBeat();
  seen.askedAgain = sleeps.length;

  // Looked at, it wakes by the stored tab's own road, its last screen addressed by its name.
  await wakeStoredTab(resting.tab);
  seen.resumed = resumed.find((one) => one.id === `s-${resting.term}`) ?? null;
  seen.wantRestore = { worktree: BEHIND, id: resting.tab.storedId, ordinal: 0 };
  seen.woke = !resting.tab.asleep && !resting.tab.slept && paneLeaves(resting.tab.layout).length === 1;
  paintTabNode(node, resting.tab);
  seen.labelAwake = node.querySelector(".tab-label").textContent;

  // A machine with memory enough answers no policy, and no beat runs.
  idleSleepAfterMs = null;
  const before = sleeps.length;
  await idleSleepBeat();
  seen.offAsksNothing = sleeps.length === before;

  paneRole = priorRole;
  Object.assign(window.__ANSWER__, priorAnswers);
  for (const key of Object.keys(window.__ANSWER__)) if (!(key in priorAnswers)) delete window.__ANSWER__[key];
  return seen;
}

export async function testIdleSleep(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const seen = await page.evaluate(exerciseIdleSleep);
    ok(
      "idle sleep: of nine tabs only the one at rest long enough behind the front is put to sleep, and the refused one is the only other asked",
      JSON.stringify(seen.asked) === JSON.stringify(seen.wantAsked) &&
        JSON.stringify(seen.call) === JSON.stringify(seen.wantCall),
      JSON.stringify(seen),
    );
    ok(
      "idle sleep: the slept tab stays on the strip as the stored tab a restart leaves — its conversation in the record by its name, its shells' ids spent, its shells gone from it",
      seen.slept && seen.record?.id === seen.wantCall.id && seen.record.agent === "claude" &&
        seen.record.session === seen.wantSession && seen.record.terms === false,
      JSON.stringify(seen),
    );
    ok(
      "idle sleep: a turn going, a rest under the while, words typed after the turn, the workspace in front, a zo helper's pane, a ledger seat, a conversation that cannot wake, and a refusal each keep their tab awake",
      Object.values(seen.stayed).every(Boolean) && seen.refusedHeld,
      JSON.stringify(seen.stayed),
    );
    ok(
      "idle sleep: the strip says 「잠듦」 on the slept tab, and the word is gone once it wakes",
      seen.label.endsWith(seen.sleptWord) && !seen.labelAwake.includes(seen.sleptWord),
      JSON.stringify([seen.label, seen.labelAwake, seen.sleptWord]),
    );
    ok(
      "idle sleep: a second beat asks the backend nothing again",
      seen.askedAgain === seen.wantAsked.length,
      JSON.stringify(seen),
    );
    ok(
      "idle sleep: looked at, the tab wakes its conversation by the stored road, the last screen addressed by the tab's name and leaf",
      seen.woke && seen.resumed?.agent === "claude" &&
        JSON.stringify(seen.resumed.restore) === JSON.stringify(seen.wantRestore),
      JSON.stringify(seen),
    );
    ok("idle sleep: with no policy no beat asks anything", seen.offAsksNothing, JSON.stringify(seen));
    ok("idle sleep raised no renderer errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}
