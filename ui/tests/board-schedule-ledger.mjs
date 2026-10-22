/* 일정 탭의 워커 과업, 사람의 원장 사본 위에서 (t-71840) — 「지금」 선까지 늘어난 끝난 막대의 수와 행 수를
 * 고치기 전의 일정과 이 판의 일정으로 잰다.
 *
 * 과업 행은 러스트 측정(`orchestration::desk::tests::the_schedule_over_the_ledger_that_already_happened`)이
 * 원장의 읽기 전용 사본에서 이 빌드의 데스크로 뽑은 것(id·단계·시각만 — 제목·본문·경로는 없다)이고,
 * `landings`는 그때 이 저장소의 git이 말한 착지·발행이다. 고치기 전의 일정은 `--before`에 준 그 판의
 * `shell-board-schedule.js`를 창이 싣는 대신 받는다(같은 창, 같은 답 — 일정 파일 하나만 다르다). 그 판의
 * 확대마다(고치기 전은 하루·한 주, 뒤는 지금 앞뒤까지) 일정 모델(`boardScheduleModel`)이 세운 워커 과업의
 * 행을 센다:
 *
 *   rows       묶음에 선 행
 *   ended      그 가운데 끝난 것(배포·병합·착지할 것 없음·실패)
 *   toNow      끝난 것 가운데 막대 끝이 「지금」인 것 — 0이어야 한다
 *   more       「+N 끝남」으로 접힌 끝난 것
 *
 *   node ui/tests/board-schedule-ledger.mjs --replay replay.json --before old/shell-board-schedule.js [--json out.json] */
import { readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";

const option = (name) => {
  const at = process.argv.indexOf(`--${name}`);
  return at >= 0 ? process.argv[at + 1] : null;
};

/* The window's schedule over the replay, counted per zoom. `source` stands in for the window's own
 * schedule file when given — the schedule as it was before the change. */
export async function countScheduleOverReplay(browser, origin, replay, source = null) {
  const { page } = await openWindowTestPage(browser, origin, {
    before: source === null ? null : (page) => page.route(/\/shell-board-schedule\.js(\?.*)?$/, (route) =>
      route.fulfill({ status: 200, contentType: "text/javascript; charset=utf-8", body: source })),
  });
  try {
    return await page.evaluate((replay) => {
      const answer = { revision: 1, unchanged: false, read_ms: replay.now_ms, line: {}, jobs: [], lanes: [],
        landings: replay.landings };
      const counted = {};
      // Every zoom the schedule of that tree has: day and week before the change, 「지금 앞뒤」 too after it.
      for (const zoom of Object.keys(BOARD_SCHEDULE.zooms)) {
        const choice = { zoom, folded: new Set(), opened: new Set(), log: null };
        const model = boardScheduleModel(answer, replay.tasks, replay.now_ms, choice, 900);
        const workers = model.groups.find((group) => group.id === "workers");
        const rows = workers.rows.filter((row) => row.depth === 0);
        const ended = rows.filter((row) => row.state === "done" || row.state === "failed");
        counted[zoom] = {
          rows: rows.length,
          ended: ended.length,
          toNow: ended.filter((row) => row.end >= model.now).length,
          more: Number(workers.more) || 0,
        };
      }
      return counted;
    }, replay);
  } finally {
    await page.close();
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const replayPath = option("replay");
  const beforePath = option("before");
  if (!replayPath || !beforePath) {
    console.error("usage: node ui/tests/board-schedule-ledger.mjs --replay replay.json --before old.js [--json out.json]");
    process.exit(2);
  }
  const replay = JSON.parse(await readFile(replayPath, "utf8"));
  const before = await readFile(beforePath, "utf8");
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  try {
    const counted = {
      now_ms: replay.now_ms,
      tasks: replay.tasks.length,
      landings: Object.keys(replay.landings ?? {}).length,
      git: replay.git ?? null,
      before: await countScheduleOverReplay(browser, origin, replay, before),
      after: await countScheduleOverReplay(browser, origin, replay),
    };
    console.log(JSON.stringify(counted));
    if (option("json")) await writeFile(option("json"), `${JSON.stringify(counted, null, 2)}\n`);
  } finally {
    await browser.close();
    files.close();
  }
}
