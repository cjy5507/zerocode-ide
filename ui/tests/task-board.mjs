import { mkdir } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";

// A self-contained fixture also used for screenshots of the real window UI.
export function taskBoardFixture() {
  const now = Date.now();
  const card = (pane, heading, state, extra = {}) => ({
    pane, heading, state, agent: "codex", project: "/repos/zerocode",
    worktree: "main", task: "", ask: "", said: "", you: "", parent: "",
    ledger: "", at: now - 30_000, changed_at: now - 180_000, unseen: true, ...extra,
  });
  window.__COLUMNS__ = [
    { bucket: "attention", cards: [card("term:101", "로그인 개선", "needs-attention", {
      agent: "claude", ask: "세션 유지 방식을 선택해 주세요.",
      ask_prompt: { questions: [{ question: "세션 유지 방식을 선택해 주세요.", multi_select: false,
        options: [{ label: "기존 방식 유지", description: "기존 설정을 사용합니다." },
          { label: "자동 연장", description: "활동 중에는 세션을 유지합니다." }] }] },
    })] },
    { bucket: "working", cards: [
      card("term:102", "SQL 응답 속도 개선", "working", {
        project: "/repos/acme", you: "느린 쿼리의 원인을 분석하고 응답 시간을 줄여줘.",
        said: "병목 쿼리 2개를 확인했고 인덱스를 수정하고 있어요.",
      }),
      card("sub:102:bench", "수정 전후 성능 비교", "working", {
        project: "/repos/acme", parent: "term:102", agent: "claude", at: 0,
      }),
      card("term:103", "에이전트 보드 개선", "working", {
        said: "작업별 배치를 적용했고 작은 창에서도 읽기 쉬운지 확인하고 있어요.",
      }),
    ] },
    { bucket: "done", cards: [card("term:104", "관리자 화면 구현", "done", {
      project: "/repos/lotto", said: "관리자 화면 구현 결과를 확인해 주세요.",
    })] },
    { bucket: "idle", cards: [card("term:105", "이전 작업", "idle")] },
  ];
  window.__PANES__ = [];
  window.__LEDGER__ = [];
  paneActivities.set("term:102", [{ at: now - 30_000, activity: { verb: "edit", target: "db/queries.sql", phase: "started" } }]);
  paneActivities.set("term:103", [{ at: now - 15_000, activity: { verb: "bash", target: "npm run test:window", phase: "started" } }]);
  paneModels.set(102, "gpt-6-astra");
  agentBoardMode = "tasks";
  taskBoardFilter = "all";
  agentGraphSelectedKey = null;
  boardQuery = "";
  boardBroken = false;
  openBoard();
}

export async function testTaskBoard(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(taskBoardFixture);
    await page.waitForSelector(".task-board-row");
    const read = await page.evaluate(() => {
      const view = document.querySelector("#board-view");
      const tasks = taskBoardModels.get(view);
      const sql = tasks.groups.find((group) => group.title === "SQL 응답 속도 개선");
      return { count: tasks.groups.length, members: sql.members.length,
        head: view.querySelector(".board-title").textContent,
        graphHidden: view.querySelector(".agent-graph-surface").hidden,
        sections: [...view.querySelectorAll("[data-task-section]")].map((node) => node.dataset.taskSection),
        historyClosed: !view.querySelector('details[data-task-section="idle"]').open,
        current: sql.activity,
        noInventedCompletion: !view.querySelector('[data-task-section="done"]').textContent.includes("검증 완료"),
        noPreview: !view.querySelector(".board-card-screen"),
      };
    });
    ok("task view is the default, groups helpers, and separates attention, work, endings and history",
      read.count === 5 && read.members === 2 && read.head === "작업 상황판" && read.graphHidden &&
      read.sections.join() === "attention,working,done,idle" && read.historyClosed &&
      read.current.includes("queries.sql") && read.noInventedCompletion && read.noPreview, JSON.stringify(read));

    const grouping = await page.evaluate(() => {
      const base = { pane: "term:1", heading: "터미널 1", you: "요청에서 작업 이름을 읽는다", task: "",
        project: "/repos/one", worktree: "main", state: "working", at: 0, parent: "" };
      const cards = [base, { ...base, pane: "term:2" },
        { ...base, pane: "term:3", task: "same dispatch" },
        { ...base, pane: "term:4", task: "same dispatch" },
        { ...base, pane: "term:5", parent: "term:1", task: "a separate dispatch" },
        { ...base, pane: "term:6", parent: "term:7" }, { ...base, pane: "term:7", parent: "term:6" },
        { ...base, pane: "term:0", parent: "term:6" },
        { ...base, pane: "term:8", task: "same dispatch" },
        { ...base, pane: "term:9", task: "same dispatch", project: "/repos/two" },
        { ...base, pane: "term:10", parent: "term:missing" }];
      const places = new Map(cards.map((card) => [card.pane, { checkout: card.project,
        taskId: ["term:3", "term:4", "term:8", "term:9"].includes(card.pane) ? "t-same" : "",
        run: card.pane === "term:8" ? "run-2" : card.pane === "term:9" ? "run-3" : "run-1" }]));
      const model = agentGraphModel([{ bucket: "working", cards }], places, new Map(), Date.now(), null, []);
      const groups = taskBoardModel(model).groups;
      // A finished parent never hides its working child.
      const child = { ...base, pane: "sub:1:live", parent: "term:1" };
      const live = taskBoardModel(agentGraphModel([
        { bucket: "done", cards: [{ ...base, state: "done" }] },
        { bucket: "working", cards: [child] },
      ], places, new Map(), Date.now(), null, [])).groups[0];
      return { count: groups.length, title: taskBoardTitle(base),
        dispatch: groups.filter((group) => group.title === "same dispatch")
          .map((group) => group.members.length).sort().join(),
        live: live.bucket, liveMembers: live.members.length };
    });
    ok("grouping requires real lineage or a scoped dispatch; handles cycles and live children of ended parents",
      grouping.count === 8 && grouping.dispatch === "1,1,2" && grouping.live === "working" &&
      grouping.liveMembers === 2 && grouping.title === "요청에서 작업 이름을 읽는다", JSON.stringify(grouping));

    await page.click('[data-task-filter="attention"]');
    ok("attention filter shows the actionable task", await page.locator(".task-board-row").count() === 1);
    await page.click(".task-board-action");
    await page.waitForSelector(".board-ask-field");
    await page.fill(".board-ask-field", "작성 중인 답변");
    const steady = await page.evaluate(async () => {
      const view = document.querySelector("#board-view");
      const row = view.querySelector(".task-board-row");
      const input = view.querySelector(".board-ask-field");
      const working = window.__COLUMNS__.find((column) => column.bucket === "working").cards[0];
      working.said = "다른 에이전트의 활동이 갱신됐어요.";
      window.__COLUMNS__[0].cards[0].at -= 120_000;
      await paintBoardView();
      return { same: row === view.querySelector(".task-board-row"),
        inputSame: input === view.querySelector(".board-ask-field"),
        value: view.querySelector(".board-ask-field").value,
        focused: document.activeElement === input };
    });
    ok("unrelated live updates preserve the task row and an in-progress answer with focus",
      steady.same && steady.inputSame && steady.value === "작성 중인 답변" && steady.focused, JSON.stringify(steady));
    await page.click(".agent-inspector-close");
    await page.click('[data-task-filter="all"]');
    await page.fill(".board-query", "성능 비교");
    await page.waitForFunction(() => document.querySelectorAll(".task-board-row").length === 1);
    ok("searching a helper keeps its complete task context", await page.locator(".task-board-title").textContent() === "SQL 응답 속도 개선");
    await page.fill(".board-query", "");
    await page.waitForFunction(() => document.querySelectorAll(".task-board-row").length === 5);
    await page.fill(".board-query", "이전 작업");
    await page.waitForFunction(() => document.querySelector('details[data-task-section="idle"]')?.open);
    await page.fill(".board-query", "");
    await page.waitForFunction(() => document.querySelectorAll(".task-board-row").length === 5);
    ok("search expands history without overwriting the person's collapsed preference",
      await page.locator('details[data-task-section="idle"]').evaluate((node) => !node.open));

    // Screenshots tell the example's story, without the mutations used above.
    await page.evaluate(taskBoardFixture);
    await page.waitForFunction(() => document.querySelector(".task-board-message-text") !== null &&
      document.querySelector(".task-board-surface").textContent.includes("병목 쿼리 2개"));
    await mkdir("output/playwright/task-board", { recursive: true });
    for (const width of [1440, 900, 560, 360]) {
      await page.setViewportSize({ width, height: 1000 });
      await page.evaluate(() => {
        const view = document.querySelector("#board-view");
        view.style.position = "fixed"; view.style.inset = "0"; view.style.width = "100vw";
        view.style.zIndex = "100"; view.style.height = "100vh";
      });
      await page.screenshot({ path: `output/playwright/task-board/dark-${width}.png` });
      const size = await page.evaluate(() => {
        const surface = document.querySelector(".task-board-surface");
        const heading = document.querySelector(".agent-graph-head");
        return { width: surface.clientWidth, scroll: surface.scrollWidth, head: heading.clientWidth, headScroll: heading.scrollWidth };
      });
      ok(`task view has no horizontal overflow at ${width}px`, size.scroll <= size.width + 1 && size.headScroll <= size.head + 1, JSON.stringify(size));
    }
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.evaluate(() => applyTheme("light"));
    await page.screenshot({ path: "output/playwright/task-board/light-1440.png" });
    await page.click('[data-board-mode="graph"]');
    ok("the relations view remains available", await page.locator(".agent-graph-surface").isVisible() && await page.locator(".agent-graph-node.is-agent").count() > 0);
    await page.click('[data-board-mode="tasks"]');
    ok("returning to tasks preserves the task list", await page.locator(".task-board-row").count() === 5);
    ok("task board raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failures = 0;
  try {
    await testTaskBoard(browser, origin, (name, pass, detail = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass && detail ? `\n${detail}` : ""}`);
      if (!pass) failures++;
    });
  } finally {
    await browser.close();
    files.close();
  }
  if (failures) process.exitCode = 1;
}
