import { openWindowTestPage } from "./window-boot.mjs";

/* 「쌓인 일」 — the right column's first tab (IDE design pass 2, B: item 12 of the
 * passed draft p-6893560125c34552): what is piling up in the project on screen,
 * before the source-control list.
 *
 *  - 빌드·게이트: this project's builds as the build card reads them
 *    (`buildsHere`), and the gate's line.
 *  - 미반영: the project's checkouts git says are not in main yet, each with
 *    its commit count and how far behind main it runs, and the sum of them
 *    on the tab itself.
 *  - 저장소: the branch, its upstream and the changed files, and the door to
 *    source control.
 *
 * Nothing is built while the tab is not up: its panel is empty until it is
 * opened and emptied when another tab takes the column.
 *
 *   WINDOW_SUITES=stacked-work node ui/tests/window.mjs */

/* Two unlanded checkouts (six commits, and two that run twenty behind main),
 * a landed one, and a build of the project on its test step — any build's
 * record, answered with a revision as the backend does. A page function. */
export async function standPile() {
  const root = projects[0].path;
  const base = projects[0];
  const landing = (ahead, extra = {}) => ({ state: "unlanded", ahead, dirty: false, compare_ref: "origin/main", ...extra });
  window.__ANSWER__.project_catalog = () => [{
    ...base,
    worktrees: [
      ...base.worktrees,
      { path: `${root}-stack`, branch: "zo/v1199-stack", is_main: false, active: false, is_folder: false,
        ownership: "zerocode-managed", external_hidden: false, landing: landing(6) },
      { path: `${root}-weekly`, branch: "zo/v1199-weekly-review", is_main: false, active: false, is_folder: false,
        ownership: "zerocode-managed", external_hidden: false, landing: landing(2, { far_behind: true, behind: 20 }) },
      { path: `${root}-done`, branch: "zo/landed", is_main: false, active: false, is_folder: false,
        ownership: "zerocode-managed", external_hidden: false, landing: { state: "landed", ahead: 0, dirty: false, compare_ref: "origin/main" } },
    ],
  }];
  // The repository's last commits, as `git_history` answers them (the source-control list's rows).
  const commit = (id, subject, references = []) => ({
    item: { id, parent_ids: [], subject, author: "dev", display_id: id.slice(0, 7), timestamp: 1754700000, references },
    kind: "node", width: 22, height: 24, paths: [], circles: [{ cx: 11, cy: 12, r: 3.5, fill: "git-graph-ref" }],
  });
  window.__ANSWER__.git_history = (args) => {
    window.__PILE_HISTORY_ASKED__ = (window.__PILE_HISTORY_ASKED__ ?? 0) + 1;
    window.__PILE_HISTORY_LIMIT__ = args?.limit;
    return { rows: [
      commit("aaaaaaa1", "release: v1.1.98", [{ id: "refs/heads/main", name: "main", revision: "aaaaaaa1", category: "branches", color: "git-graph-ref" }]),
      commit("bbbbbbb2", "green (l80a g3b3): every test that moved"),
      commit("ccccccc3", "fix: t-77569 — the naming writes its answer"),
    ], has_more: true, current: null, remote: null };
  };
  await refreshWorktrees();
  window.__PILE_BUILDS__ = [{ id: "app", name: "앱 빌드", label: "v1.1.98", sha: "ca1ffcf0d1", state: "running",
    started_ms: Date.now() - 60_000, running_since_ms: Date.now() - 60_000, updated_ms: Date.now(),
    steps: [{ name: "test", words: { ko: "단위 시험" }, state: "running", expected: { secs: 0, samples: 0 } }] }];
  window.__ANSWER__.board_builds = ({ known = null } = {}) => {
    const revision = window.__PILE_BUILDS__[0].state === "failed" ? 2 : 1;
    return known === revision ? { revision, unchanged: true, builds: [] } : { revision, unchanged: false, builds: window.__PILE_BUILDS__ };
  };
  // The schedule the gate line reads, as the backend answers it for a project whose workspace folder holds a build
  // line with no gate job in it — held for the checkout on screen, and said again when asked again (B9).
  window.__PILE_SCHEDULE__ = { revision: 1, unchanged: false, read_ms: Date.now(), line: { found: true, runner_lanes: [1] },
    jobs: [], builds: [] };
  window.__ANSWER__.schedule_timeline = ({ known = null } = {}) => (known === window.__PILE_SCHEDULE__.revision
    ? { revision: known, unchanged: true, read_ms: Date.now() } : window.__PILE_SCHEDULE__);
  holdBoardScheduleAnswer(window.__PILE_SCHEDULE__, activeWorktreePath);
  boardScheduleAnsweredAt = Date.now();
  await askBoardBuilds();
}

export async function testStackedWork(browser, origin, ok) {
  const { page } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(standPile);
    const seen = await page.evaluate(async () => {
      const seen = {};
      const tab = document.getElementById("activity-pile-tab");
      const panel = document.getElementById("activity-pile");
      // First after 「대화」, the conversation's own panel that took the front on 2026-11-08 (D6).
      seen.tabFirst = tab?.previousElementSibling?.id === "activity-chat-tab" && tab.parentElement.firstElementChild.id === "activity-chat-tab";
      seen.count = document.getElementById("activity-pile-count")?.textContent ?? "";
      // The draft's word tab: 「쌓인 일 8」, not an icon with a badge.
      seen.tabWords = tab?.textContent.replace(/\s+/g, " ").trim() ?? "";
      seen.emptyWhileClosed = panel ? panel.hidden && panel.childElementCount === 0 : false;
      tab?.click();
      await window.__PAINTED__();
      seen.open = panel && !panel.hidden;
      seen.selected = tab?.getAttribute("aria-selected");
      seen.build = [...(panel?.querySelector(".pile-build")?.children ?? [])].map((part) => part.textContent).join(" | ");
      seen.total = panel?.querySelector(".pile-total")?.textContent.replace(/\s+/g, " ").trim() ?? "";
      seen.branches = [...(panel?.querySelectorAll(".pile-branch") ?? [])].map((row) => row.textContent.replace(/\s+/g, " ").trim());
      seen.repoDoor = Boolean(panel?.querySelector(".pile-scm"));
      // The draft's half that was missing: the time the facts were read, the builds group's
      // count and its gate line (a dash where the window holds no gate job), a bar per branch
      // as long as its commits, the checkouts no agent window holds and the door to clean them
      // up, the repository's automatic checks and its last commits.
      seen.when = panel?.querySelector(".pile-when")?.textContent ?? "";
      seen.buildCount = panel?.querySelector(".pile-build")?.closest(".pile-section")?.querySelector(".pile-section-count")?.textContent ?? "";
      seen.gate = panel?.querySelector(".pile-gate")?.textContent.replace(/\s+/g, " ").trim() ?? "";
      seen.bars = [...(panel?.querySelectorAll(".pile-bar > i") ?? [])].map((bar) => Math.round(bar.getBoundingClientRect().width));
      seen.note = panel?.querySelector(".pile-note")?.textContent.replace(/\s+/g, " ").trim() ?? "";
      const cleanups = [];
      const realCleanup = openCleanup;
      openCleanup = () => cleanups.push(true);
      panel?.querySelector(".pile-cleanup")?.click();
      openCleanup = realCleanup;
      seen.cleanup = cleanups.length;
      seen.checks = panel?.querySelector(".pile-checks")?.textContent.replace(/\s+/g, " ").trim() ?? "";
      // The history's answer, named — not a fixed wait (t-84799 B8).
      const arrived = async () => { for (let frame = 0; frame < 600 && panel?.querySelectorAll(".pile-commits .history-row").length === 0; frame += 1) await new Promise((done) => requestAnimationFrame(done)); };
      await arrived();
      seen.commitsHead = panel?.querySelector(".pile-commits")?.closest(".pile-section")?.querySelector(".pile-section-head")?.textContent ?? "";
      seen.commits = [...(panel?.querySelectorAll(".pile-commits .history-row") ?? [])].map((row) => row.querySelector(".history-subject")?.textContent ?? "");
      seen.more = Boolean(panel?.querySelector(".pile-more"));
      seen.historyAsked = window.__PILE_HISTORY_ASKED__ ?? 0;
      panel?.querySelector(".pile-scm")?.click();
      await window.__PAINTED__();
      seen.scmShown = !document.getElementById("activity-scm").hidden;
      seen.emptiedOnLeave = panel.hidden && panel.childElementCount === 0;
      return seen;
    });
    ok(
      "the right column's first tab after 「대화」 counts what has not landed, and its panel is built only while it is up",
      seen.tabFirst && seen.count === "8" && seen.tabWords === "쌓인 일 8" && seen.emptyWhileClosed && seen.open && seen.selected === "true" && seen.emptiedOnLeave,
      JSON.stringify(seen),
    );
    ok(
      "쌓인 일 says when it was read, counts the builds group with its gate line, draws a bar per branch, offers to clean up the checkouts no agent holds, and says the checks and the last commits",
      /\d{1,2}:\d{2}/.test(seen.when) && seen.buildCount === "1" && seen.gate.includes("—") &&
        seen.bars.length === 2 && seen.bars[0] > seen.bars[1] * 2 && seen.bars[1] > 0 &&
        seen.note.includes("에이전트 창 없음") && seen.note.includes("2") && seen.cleanup === 1 &&
        seen.checks.includes("자동 확인") && seen.checks.includes("—") &&
        seen.commitsHead.includes("3+") && seen.commits.length === 3 && seen.commits[0] === "release: v1.1.98" &&
        seen.more && seen.historyAsked === 1,
      JSON.stringify(seen),
    );
    ok(
      "쌓인 일 says the project's build in the record's words, the unlanded commits and branches with how far behind main, and opens source control",
      seen.build === "앱 빌드 v1.1.98 | 도는 중 · 단위 시험" && seen.total.includes("8") &&
        seen.branches.length === 2 && seen.branches[0].includes("zo/v1199-stack") && seen.branches[0].includes("6") &&
        seen.branches[1].includes("zo/v1199-weekly-review") && seen.branches[1].includes("20") &&
        seen.repoDoor && seen.scmShown,
      JSON.stringify(seen),
    );

    /* A failed build is the tab's to say while nothing else is up: its name and tip say it, in the halt tone. */
    const halted = await page.evaluate(async () => {
      window.__PILE_BUILDS__ = window.__PILE_BUILDS__.map((one) => ({ ...one, state: "failed", running_since_ms: undefined,
        steps: one.steps.map((step) => ({ ...step, state: "failed", secs: 30 })) }));
      await askBoardBuilds();
      const count = document.getElementById("activity-pile-count");
      return { tip: document.getElementById("activity-pile-tab").dataset.tip, tone: count.dataset.tone, hidden: count.hidden };
    });
    ok(
      "a failed build of the project turns the tab's words and tone to it",
      halted.tip === "쌓인 일 8 — 빌드 실패" && halted.tone === "halt" && halted.hidden === false,
      JSON.stringify(halted),
    );

    /* t-84799 B9: the gate line reads the schedule's last answer — the gate the board's own line names — and keeps its dash
     * only where the schedule has nothing to say. */
    const gate = await page.evaluate(async () => {
      const seen = {};
      const now = Date.now();
      const minute = 60_000;
      const step = (kind, state, start, end) => ({ name: `0-${Math.round((now - 20 * minute) / 1000)}-run1-l61r-${kind}`, who: "run1-l61r", state,
        class: "first", lane: 1, cap_secs: 600, queued_ms: now - 20 * minute, start_ms: start, end_ms: end,
        secs: start && end ? Math.round((end - start) / 1000) : null, rc: state === "failed" ? 1 : state === "passed" ? 0 : null,
        kind, gate: "l61", part: "r", what: null, expected: { secs: 300, samples: 5 } });
      const stood = window.__PILE_SCHEDULE__;
      const hold = (answer) => {
        window.__PILE_SCHEDULE__ = answer;
        holdBoardScheduleAnswer(answer, activeWorktreePath);
      };
      const line = { found: true, runner_lanes: [1], paused: false };
      hold({ revision: 7, unchanged: false, read_ms: now, line, builds: [],
        jobs: [step("g1-build", "passed", now - 18 * minute, now - 12 * minute), step("g2-tests", "failed", now - 12 * minute, now - 6 * minute)] });
      // The answer's own arrival time is what keeps the line honest (r2 P2-2): this one came just now.
      boardScheduleAnsweredAt = now;
      const tab = document.getElementById("activity-pile-tab");
      tab.click();
      await window.__PAINTED__();
      paintStackedWork();
      const gateText = () => document.querySelector("#activity-pile .pile-gate")?.textContent.replace(/\s+/g, " ").trim() ?? null;
      seen.gate = gateText();
      hold({ revision: 8, unchanged: false, read_ms: now, line, builds: [], jobs: [] });
      paintStackedWork();
      seen.dash = gateText();
      // A project whose workspace folder holds no build line: no gate line stands, and the section is its builds alone.
      hold({ revision: 9, unchanged: false, read_ms: now, line: { found: false, runner_lanes: [] }, builds: [], jobs: [] });
      paintStackedWork();
      seen.none = gateText();
      seen.head = document.querySelector("#activity-pile .pile-build")?.closest(".pile-section")
        ?.querySelector(".pile-section-head")?.firstChild?.textContent ?? "";
      hold(stood);
      return seen;
    });
    ok(
      "the gate line of 쌓인 일 names the gate the schedule's last answer holds and what became of it — a dash only without one — and on a project with no build line no gate line stands, the section its builds alone",
      gate.gate?.includes("l61") && gate.gate.includes("실패") && !gate.gate.includes("—") && gate.dash?.includes("—")
        && gate.none === null && gate.head === "빌드",
      JSON.stringify(gate),
    );

    /* r2 P2-2: the line must not tell an old answer as the present. With the panel up the schedule is asked again (at its
     * own pace, not on every paint); until the new answer comes, an answer held for minutes says a dash — not a gate
     * "running" that ended hours ago — and when the answer arrives the panel is drawn again without anyone painting it. */
    const fresh = await page.evaluate(async () => {
      const seen = {};
      const now = Date.now();
      const minute = 60_000;
      const step = (kind, state, start, end, queued) => ({ name: `0-${Math.round(queued / 1000)}-run1-l61r-${kind}`, who: "run1-l61r", state,
        class: "first", lane: 1, cap_secs: 600, queued_ms: queued, start_ms: start, end_ms: end,
        secs: start && end ? Math.round((end - start) / 1000) : null, rc: state === "failed" ? 1 : state === "passed" ? 0 : null,
        kind, gate: "l61", part: "r", what: null, expected: { secs: 300, samples: 5 } });
      const line = { found: true, runner_lanes: [1], paused: false };
      // The held answer says the gate is running (it started minutes ago by the clock), but it was received five hours ago.
      holdBoardScheduleAnswer({ revision: 11, unchanged: false, read_ms: now - 300 * minute, line, builds: [],
        jobs: [step("g1-build", "running", now - 3 * minute, null, now - 4 * minute)] }, activeWorktreePath);
      boardScheduleAnsweredAt = now - 300 * minute;
      boardScheduleAskedAt = 0;
      const asked = [];
      let release = null;
      window.__ANSWER__.schedule_timeline = (args) => { asked.push(args); return new Promise((done) => { release = done; }); };
      const gateText = () => document.querySelector("#activity-pile .pile-gate")?.textContent.replace(/\s+/g, " ").trim() ?? "";
      const frames = async (count) => { for (let i = 0; i < count; i += 1) await new Promise((done) => requestAnimationFrame(done)); };
      // Leave the tab and come back: the panel is drawn from nothing, with the old answer in hand.
      document.getElementById("activity-files-tab")?.click();
      await window.__PAINTED__();
      document.getElementById("activity-pile-tab").click();
      await window.__PAINTED__();
      seen.askedOnOpen = asked.length;
      seen.staleText = gateText();
      for (let i = 0; i < 4; i += 1) paintStackedWork();
      seen.askedAfterRepaints = asked.length;
      // The answer comes: a gate that failed since. No paint is called from here.
      release({ revision: 12, unchanged: false, read_ms: Date.now(), line, builds: [],
        jobs: [step("g1-build", "failed", now - 3 * minute, now - minute, now - 4 * minute)] });
      await frames(6);
      seen.freshText = gateText();
      seen.answeredNow = Math.abs(Date.now() - boardScheduleAnsweredAt) < 5_000;
      // An answer that stays silent (no change) is still an answer received: the line keeps its words.
      boardScheduleAskedAt = 0;
      window.__ANSWER__.schedule_timeline = (args) => { asked.push(args); return { revision: 12, unchanged: true, read_ms: Date.now() }; };
      refreshBoardSchedule();
      await frames(6);
      seen.afterUnchanged = gateText();
      delete window.__ANSWER__.schedule_timeline;
      boardScheduleAnswer = null;
      return seen;
    });
    ok(
      "쌓인 일 asks the schedule again while it is up (once per beat, not per paint), says a dash for an answer held for hours, and draws again when the new answer arrives",
      fresh.askedOnOpen === 1 && fresh.askedAfterRepaints === 1 && fresh.staleText.includes("—") && !fresh.staleText.includes("도는 중") &&
        fresh.freshText.includes("l61") && fresh.freshText.includes("실패") && !fresh.freshText.includes("—") && fresh.answeredNow &&
        fresh.afterUnchanged.includes("l61") && fresh.afterUnchanged.includes("실패"),
      JSON.stringify(fresh),
    );
  } finally { await page.close(); }
}
