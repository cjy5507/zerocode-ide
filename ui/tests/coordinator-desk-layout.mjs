/* 데스크의 격자 (t-7448 → t-67607) — 사람이 통과시킨 시안 B의 두 단과 넓은 한 단.
 *
 * 잰 높이로 세 배치 가운데 하나를 고르던 두 열(`DESK_LAYOUTS`, t-7448)은 시안 B의 고정
 * 격자로 대신했다: 「지금」 탭에서 당신 차례는 왼쪽 넓은 열(2fr), 빌드 줄은 오른쪽(1fr)에
 * 한 행으로 나란히 서고 두 카드의 높이가 같으며, 통합 묶음과 지금 도는 일은 그 아래 판의
 * 폭 전체를 쓴다. 한 장만 서면 그 장이 행을 다 쓰고, 블록 사이는 디자인 간격(`--space-5`)
 * 하나다 — 숨은 블록은 자리도 간격도 남기지 않는다. 목록 티어(보드 699px 이하)에서는 DOM
 * 차례로 한 열이다. 「워커」와 「기록」 탭도 한 열이다. 쓰는 중인 답은 티어 넘기와 조용한
 * 폴을 지나도 노드·글자·초점이 그대로이고, 조용한 폴은 아무것도 쓰지 않는다. 재는 것은
 * 실제 박스다. 픽스처는 `coordinator-desk.mjs`의 것이다.
 *
 *   node ui/tests/coordinator-desk-layout.mjs
 *   node ui/tests/window.mjs --suite coordinator-desk-layout */
import { mkdir } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";
import { installBoardWaits } from "./board-waits.mjs";
import { coordinatorDeskFixture, deskMutations } from "./coordinator-desk.mjs";

const SHOTS = "output/playwright/coordinator-desk-layout";
const REPLY = '#board-view [data-letter="run-desk/m-901"]';

const near = (a, b, tolerance = 1) => Math.abs(a - b) <= tolerance;

async function settle(page) {
  await page.evaluate(async () => {
    await window.__BOARD_SETTLED__();
    for (let beat = 0; beat < 30 && (deskLedgerAsking || deskPaintFrame !== null); beat += 1) {
      await new Promise((done) => requestAnimationFrame(done));
    }
    await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
  });
}

/* The desk's visible boxes in one evaluation, in DOM order. */
async function readDesk(page) {
  return page.evaluate(() => {
    const view = document.querySelector("#board-view");
    const desk = view.querySelector(".task-board-desk");
    const tenth = (value) => Math.round(value * 10) / 10;
    const rect = (node) => {
      const box = node.getBoundingClientRect();
      return { top: tenth(box.top), bottom: tenth(box.bottom), left: tenth(box.left), right: tenth(box.right),
        width: tenth(box.width), height: tenth(box.height) };
    };
    const shown = [...desk.querySelectorAll(":scope > .board-desk-block")]
      .filter((node) => node.checkVisibility())
      .map((node) => ({ id: node.dataset.deskBlock, ...rect(node) }));
    const surface = view.querySelector(".task-board-surface");
    return {
      gap: parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--space-5")),
      board: view.clientWidth,
      desk: { ...rect(desk), scrollWidth: desk.scrollWidth, clientWidth: desk.clientWidth },
      surface: { clientWidth: surface.clientWidth, scrollWidth: surface.scrollWidth },
      shown,
      by: Object.fromEntries(shown.map((block) => [block.id, block])),
    };
  });
}

/* Every shown block in one column: each the desk's width, each one gap under the last. */
function oneColumnFaults(facts) {
  const faults = [];
  facts.shown.forEach((block, at) => {
    if (!near(block.left, facts.desk.left) || !near(block.width, facts.desk.width)) {
      faults.push(`${block.id} is not the column's width: ${block.left}+${block.width} against ${facts.desk.left}+${facts.desk.width}`);
    }
    const above = facts.shown[at - 1];
    if (above && !near(block.top, above.bottom + facts.gap)) {
      faults.push(`${block.id} is not one gap under ${above.id}: ${block.top} against ${above.bottom}+${facts.gap}`);
    }
  });
  if (facts.surface.scrollWidth > facts.surface.clientWidth + 1) faults.push("the surface scrolls sideways");
  return faults;
}

async function openDesk(browser, origin, fixture, { width = 1440, height = 900 } = {}) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  await page.setViewportSize({ width, height });
  await installBoardWaits(page);
  await page.evaluate(coordinatorDeskFixture, { now: Date.now(), ...fixture });
  await page.evaluate(async () => {
    await window.__BOARD_SETTLED__();
    const view = document.querySelector("#board-view");
    Object.assign(view.style, { position: "fixed", inset: "0", width: "100vw", height: "100vh", zIndex: "100" });
    await askReleaseStatus();
    await paintBoardView();
    paintCoordinatorDesk(view);
  });
  await settle(page);
  return { page, faults };
}

export async function testCoordinatorDeskLayout(browser, origin, ok) {
  await mkdir(SHOTS, { recursive: true });
  const { page, faults } = await openDesk(browser, origin, {});
  try {
    /* ---- 넓은 판: 두 단과 넓은 한 단 ---- */
    const wide = await readDesk(page);
    const { asks, release, running } = wide.by;
    ok("on a wide board your turn takes the wide left column and the build line the right, side by side, one height",
      Boolean(asks && release) && asks.left < release.left && near(asks.top, release.top) && near(asks.height, release.height)
        && near(release.left, asks.right + wide.gap) && near(release.right, wide.desk.right)
        && near(asks.width, (wide.desk.width - wide.gap) * 2 / 3, 2), JSON.stringify(wide.by));
    ok("running now takes the board's whole width, one gap under the two cards",
      Boolean(running) && near(running.left, wide.desk.left) && near(running.width, wide.desk.width)
        && near(running.top, Math.max(asks.bottom, release.bottom) + wide.gap), JSON.stringify(wide.by));
    ok("the Now tab shows its own blocks and none of the others'",
      wide.shown.every((block) => ["asks", "release", "batches", "running"].includes(block.id)), JSON.stringify(wide.shown.map((one) => one.id)));
    await page.screenshot({ path: `${SHOTS}/now-1440.png` });

    /* ---- 한 장만 서면 그 장이 행을 다 쓴다 ---- */
    await page.evaluate(async () => {
      window.__HELD_RELEASE__ = window.__RELEASE__;
      window.__RELEASE__ = null;
      await askReleaseStatus();
      paintCoordinatorDesk(document.querySelector("#board-view"));
    });
    await settle(page);
    const lone = await readDesk(page);
    ok("with no lane, your turn takes the whole row", !lone.by.release && near(lone.by.asks.width, lone.desk.width),
      JSON.stringify(lone.by));
    await page.evaluate(async () => {
      window.__RELEASE__ = window.__HELD_RELEASE__;
      await askReleaseStatus();
      paintCoordinatorDesk(document.querySelector("#board-view"));
    });
    await settle(page);

    /* ---- 조용한 폴은 아무것도 쓰지 않는다 ---- */
    const quiet = await deskMutations(page, () => paintCoordinatorDesk(document.querySelector("#board-view")));
    ok("a quiet poll writes nothing on the desk", quiet === 0, `mutations ${quiet}`);

    /* ---- 쓰는 중인 답은 티어를 넘어도 그대로 ---- */
    await page.click(`${REPLY} .board-desk-letter-act`);
    await page.fill(`${REPLY} .board-desk-reply-field`, "기준은 main으로 하세요.");
    await page.evaluate((reply) => { window.__REPLY_NODE__ = document.querySelector(`${reply} .board-desk-reply-field`); }, REPLY);

    /* ---- 목록 티어: DOM 차례로 한 열 ---- */
    for (const width of [699, 560]) {
      await page.setViewportSize({ width, height: 900 });
      await settle(page);
      const narrow = await readDesk(page);
      const holes = oneColumnFaults(narrow);
      ok(`at ${width}px the Now tab is one column in DOM order, one gap apart, nothing sideways`,
        holes.length === 0 && narrow.shown.map((one) => one.id).join() === wide.shown.map((one) => one.id).join(),
        JSON.stringify({ holes, shown: narrow.shown }));
      await page.screenshot({ path: `${SHOTS}/now-${width}.png` });
    }
    await page.setViewportSize({ width: 1440, height: 900 });
    await settle(page);
    const kept = await page.evaluate((reply) => {
      const field = document.querySelector(`${reply} .board-desk-reply-field`);
      return { same: field === window.__REPLY_NODE__, value: field?.value, focused: document.activeElement === field };
    }, REPLY);
    ok("a reply being written keeps its node, its words and its focus across the tier and back",
      kept.same && kept.value === "기준은 main으로 하세요." && kept.focused, JSON.stringify(kept));
    const writing = await deskMutations(page, () => paintCoordinatorDesk(document.querySelector("#board-view")));
    ok("a quiet poll writes nothing while a reply is being written", writing === 0, `mutations ${writing}`);

    /* ---- 워커·기록 탭은 한 열 ---- */
    for (const id of ["workers", "history"]) {
      await page.evaluate((id) => chooseBoardTab(document.querySelector("#board-view"), id, false), id);
      await settle(page);
      const tab = await readDesk(page);
      const holes = oneColumnFaults(tab);
      ok(`the ${id} tab is one column, one gap apart`, tab.shown.length > 0 && holes.length === 0,
        JSON.stringify({ holes, shown: tab.shown }));
      await page.screenshot({ path: `${SHOTS}/${id}-1440.png` });
    }
    await page.evaluate(() => chooseBoardTab(document.querySelector("#board-view"), "now", false));
    ok("the desk's grid raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failures = 0;
  try {
    await testCoordinatorDeskLayout(browser, origin, (name, pass, detail = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass && detail ? `\n${detail}` : ""}`);
      if (!pass) failures++;
    });
  } finally {
    await browser.close();
    files.close();
  }
  if (failures) process.exitCode = 1;
}
