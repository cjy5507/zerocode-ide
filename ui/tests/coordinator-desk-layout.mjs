/* 데스크의 두 열 (t-7448) — 짧은 「답할 우편」이 화면 절반을 비우지 않는다.
 *
 * 격자의 자동 배치는 한 행의 높이를 그 행에서 가장 긴 블록에 맞춘다: 우편 세 통
 * 옆에 워커 여덟 행이 서면 우편 아래 화면 절반이 비고, 과업 흐름과 릴리즈 레인은
 * 다음 행이라 접힌 아래로 밀렸다(09-24 사용자 스크린샷). 고침은 잰 높이로 고르는
 * 세 배치(`DESK_LAYOUTS`: 둘 다 왼쪽·나눠 넣기·둘 다 오른쪽)이고, 여기서 재는 것은
 * 실제 박스다 — 이름 붙인 영역이나 속성이 아니라: 잰 높이가 부르는 배치가 서 있고,
 * 격자가 낸 높이가 그 배치의 셈과 같으며(걸침은 두 열이 독립, 나눠 넣기는 행마다
 * 긴 쪽), 열 안의 블록은 앞 블록의 **내용의 발** 바로 아래에 디자인 간격(`--space-4`)
 * 으로 이어 붙고, 블록의 박스는 제 내용만큼이며, 어느 블록이 숨어도 빈 행·앞뒤
 * 틈이 남지 않고, 목록 티어(보드 699px 이하)에서는 DOM 차례로 한 열이 된다. 답
 * 초안·포커스·캐럿·노드는 펼치기·워커 늘기·조용한 폴·티어 넘기·접기를 지나도
 * 그대로다. 픽스처와 수는 `coordinator-desk.mjs`의 것이고, 그 파일은 t-7388이
 * 만지므로 이 스위트는 따로 선다.
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
const ALL = ["machine", "mail", "workers", "pipeline", "release"];

const tenth = (value) => Math.round(value * 10) / 10;
const near = (a, b, tolerance = 1) => Math.abs(a - b) <= tolerance;
const overlaps = (a, b) => a.left < b.right - 0.5 && b.left < a.right - 0.5 && a.top < b.bottom - 0.5 && b.top < a.bottom - 0.5;

/* The three placements and the height the grid makes of each — the oracle
 * the measured heights are held against (`DESK_LAYOUTS` in shell-board.js). A
 * hidden block is height 0 and takes neither room nor a gap. */
const stackOf = (heights, gap) => heights.filter((height) => height > 0)
  .reduce((sum, height, at) => sum + height + (at > 0 ? gap : 0), 0);
const LAYOUTS = [
  { id: "stack-left", columns: { pipeline: "left", release: "left" },
    height: (h, gap) => Math.max(stackOf([h.mail, h.pipeline, h.release], gap), h.workers) },
  { id: "split", columns: { pipeline: "left", release: "right" },
    height: (h, gap) => stackOf([Math.max(h.mail, h.workers), Math.max(h.pipeline, h.release)], gap) },
  { id: "stack-right", columns: { pipeline: "right", release: "right" },
    height: (h, gap) => Math.max(h.mail, stackOf([h.workers, h.pipeline, h.release], gap)) },
];
function modelOf(facts) {
  const heights = Object.fromEntries(["mail", "workers", "pipeline", "release"]
    .map((id) => [id, facts.blocks[id].hidden ? 0 : facts.blocks[id].height]));
  const candidates = LAYOUTS.map((layout) => ({ id: layout.id, columns: layout.columns, height: tenth(layout.height(heights, facts.gap)) }));
  let chosen = candidates[0];
  for (const candidate of candidates) if (candidate.height < chosen.height - 0.5) chosen = candidate;
  return { heights, candidates, chosen };
}

/* The desk's boxes in one evaluation: each block's border box, its column
 * attribute, whether it is hidden, and where its content ends (its last
 * child's margin box plus the block's own padding and border — a box
 * stretched to a track is longer than that); the desk, the scroll surface,
 * the filters and the first task row after it; the leaves a clipped layout
 * would cut. Reading forces one layout, after everything the scenario
 * scheduled has settled. */
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
    const foot = (node) => {
      const last = node.lastElementChild;
      if (!last) return rect(node).bottom;
      const own = getComputedStyle(node);
      const end = getComputedStyle(last);
      return tenth(last.getBoundingClientRect().bottom + parseFloat(end.marginBottom)
        + parseFloat(own.paddingBottom) + parseFloat(own.borderBottomWidth));
    };
    const blocks = {};
    for (const node of desk.querySelectorAll(":scope > .board-desk-block")) {
      blocks[node.dataset.deskBlock] = { hidden: node.hidden, column: node.dataset.deskColumn ?? null, ...rect(node),
        foot: node.hidden ? 0 : foot(node), scrollWidth: node.scrollWidth, clientWidth: node.clientWidth };
    }
    const leaves = (selector) => [...desk.querySelectorAll(selector)]
      .filter((node) => !node.hidden && node.getClientRects().length > 0).map(rect);
    const head = (id) => {
      const node = desk.querySelector(`:scope > [data-desk-block="${id}"] .board-desk-head`);
      return node && !node.parentElement.hidden ? rect(node) : null;
    };
    const surface = view.querySelector(".task-board-surface");
    const filters = view.querySelector(".task-board-filters");
    const first = view.querySelector(".task-board-row");
    return {
      gap: parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--space-4")),
      boardWidth: view.clientWidth,
      deskHidden: desk.hidden,
      desk: { ...rect(desk), scrollWidth: desk.scrollWidth, clientWidth: desk.clientWidth },
      surface: { ...rect(surface), clientHeight: surface.clientHeight, clientWidth: surface.clientWidth,
        scrollWidth: surface.scrollWidth, scrollTop: surface.scrollTop },
      blocks,
      order: [...desk.children].map((node) => node.dataset.deskBlock),
      heads: { pipeline: head("pipeline"), release: head("release") },
      filters: filters && !filters.hidden ? rect(filters) : null,
      firstRow: first ? rect(first) : null,
      mailRows: desk.querySelectorAll(".board-desk-letter").length,
      workerRows: desk.querySelectorAll(".board-desk-worker").length,
      leaves: {
        mail: leaves('[data-desk-block="mail"] .board-desk-letter-act, [data-desk-block="mail"] .board-desk-letters-more, [data-desk-block="mail"] .board-desk-reply-field'),
        workers: leaves('[data-desk-block="workers"] .board-desk-worker-facts'),
        pipeline: leaves('[data-desk-block="pipeline"] .board-desk-stage, [data-desk-block="pipeline"] .board-desk-task'),
        release: leaves('[data-desk-block="release"] .board-desk-release-phase, [data-desk-block="release"] .board-desk-release-reason, [data-desk-block="release"] .board-desk-release-skips'),
      },
    };
  });
}

const shownOf = (facts) => facts.order.filter((id) => facts.blocks[id] && !facts.blocks[id].hidden)
  .map((id) => ({ id, ...facts.blocks[id] }));

/* What every state at every tier must satisfy. `lead` is how far under the
 * desk's top the first block stood with every block shown: hiding one moves
 * nothing above the rest, and nothing trails below the last. */
function commonFaults(facts, lead) {
  const faults = [];
  const { gap, desk, blocks } = facts;
  const shown = shownOf(facts);
  if (shown.length === 0) {
    if (!facts.deskHidden) faults.push("every block hidden, yet the desk stands");
    return faults;
  }
  if (facts.deskHidden) faults.push("blocks shown, yet the desk is hidden");
  for (const [id, block] of Object.entries(blocks)) {
    if (block.hidden && (block.width !== 0 || block.height !== 0)) faults.push(`${id} hidden yet ${block.width}×${block.height}`);
    if (!block.hidden && !near(block.bottom, block.foot)) faults.push(`${id}'s box runs ${tenth(block.bottom - block.foot)}px past its content`);
    if (!block.hidden && block.scrollWidth > block.clientWidth + 1) faults.push(`${id} overflows sideways`);
  }
  if (!blocks.machine.hidden) {
    if (!near(blocks.machine.width, desk.width)) faults.push(`machine ${blocks.machine.width} wide, the desk ${desk.width}`);
    const below = shown.filter((block) => block.id !== "machine");
    if (below.length > 0 && !near(Math.min(...below.map((block) => block.top)) - blocks.machine.foot, gap)) {
      faults.push(`${tenth(Math.min(...below.map((block) => block.top)) - blocks.machine.foot)}px under the machine strip, not ${gap}`);
    }
  }
  const top = Math.min(...shown.map((block) => block.top));
  const bottom = Math.max(...shown.map((block) => block.bottom));
  if (lead !== null && !near(top - desk.top, lead)) faults.push(`first block ${tenth(top - desk.top)}px under the desk's top, was ${lead}`);
  if (!near(desk.bottom, bottom)) faults.push(`desk runs ${tenth(desk.bottom - bottom)}px past its last block`);
  if (facts.filters && facts.filters.top < bottom - 1) faults.push("the filters stand above the desk's foot");
  if (facts.firstRow && facts.firstRow.top < bottom - 1) faults.push("the first task row stands above the desk's foot");
  if (desk.scrollWidth > desk.clientWidth + 1) faults.push("the desk overflows sideways");
  if (facts.surface.scrollWidth > facts.surface.clientWidth + 1) faults.push("the surface overflows sideways");
  for (const [id, boxes] of Object.entries(facts.leaves)) {
    if (blocks[id].hidden) continue;
    for (const leaf of boxes) {
      if (leaf.right > blocks[id].right + 1 || leaf.bottom > blocks[id].bottom + 1 || leaf.left < blocks[id].left - 1 || leaf.top < blocks[id].top - 1) {
        faults.push(`a leaf of ${id} stands outside its block (${JSON.stringify(leaf)})`);
        break;
      }
    }
  }
  for (let a = 0; a < shown.length; a += 1) {
    for (let b = a + 1; b < shown.length; b += 1) if (overlaps(shown[a], shown[b])) faults.push(`${shown[a].id} overlaps ${shown[b].id}`);
  }
  return faults;
}

/* The wide tier: the placement standing is the one the measured heights call
 * for, the boxes are laid as that placement says (a stacked column keeps the
 * design gap under each foot, a split row starts under the taller of the row
 * above), and the grid's height is that placement's own arithmetic — no
 * empty row anywhere, however the blocks are dealt. */
function wideFaults(facts, lead = null) {
  const faults = commonFaults(facts, lead);
  const { gap, desk, blocks } = facts;
  const shown = shownOf(facts);
  if (shown.length === 0) return faults;
  const { chosen } = modelOf(facts);
  for (const id of ["pipeline", "release"]) {
    if (blocks[id].column !== chosen.columns[id]) faults.push(`${id} is placed ${blocks[id].column}, the heights call for ${chosen.columns[id]} (${chosen.id})`);
  }
  const columnOf = (id) => (id === "mail" ? "left" : id === "workers" ? "right" : blocks[id].column);
  const column = (side) => ALL.filter((id) => id !== "machine" && !blocks[id].hidden && columnOf(id) === side).map((id) => ({ id, ...blocks[id] }));
  const left = column("left");
  const right = column("right");
  for (const block of left) if (!near(block.left, desk.left) || block.right >= desk.left + desk.width / 2) faults.push(`${block.id} is not in the left column`);
  for (const block of right) if (block.left <= desk.left + desk.width / 2 || !near(block.right, desk.right)) faults.push(`${block.id} is not in the right column`);
  const below = shown.filter((block) => block.id !== "machine");
  if (below.length === 0) return faults;
  const top = Math.min(...below.map((block) => block.top));
  const stacked = (blocksOf, name) => {
    if (blocksOf.length > 0 && !near(blocksOf[0].top, top)) faults.push(`${blocksOf[0].id} starts at ${blocksOf[0].top}, the columns at ${top}`);
    for (let at = 1; at < blocksOf.length; at += 1) {
      const above = blocksOf[at - 1];
      const under = blocksOf[at];
      if (!near(under.top - above.foot, gap)) faults.push(`${name}: ${above.id}→${under.id} ${tenth(under.top - above.foot)}px, not ${gap}`);
    }
  };
  if (chosen.id === "split") {
    const first = below.filter((block) => block.id === "mail" || block.id === "workers");
    const second = below.filter((block) => block.id === "pipeline" || block.id === "release");
    for (const block of first) if (!near(block.top, top)) faults.push(`${block.id} starts at ${block.top}, the first row at ${top}`);
    const secondTop = first.length > 0 ? Math.max(...first.map((block) => block.foot)) + gap : top;
    for (const block of second) if (!near(block.top, secondTop)) faults.push(`${block.id} starts at ${block.top}, the second row at ${secondTop}`);
  } else {
    stacked(left, "left");
    stacked(right, "right");
  }
  if (!near(desk.bottom - top, chosen.height)) faults.push(`the columns stand ${tenth(desk.bottom - top)}px tall, ${chosen.id} makes ${chosen.height}`);
  return faults;
}

/* The list tier: one column in DOM order, each block the design gap under the
 * foot of the one before. */
function narrowFaults(facts, lead = null) {
  const faults = commonFaults(facts, lead);
  const { gap, desk } = facts;
  const shown = shownOf(facts);
  for (const block of shown) {
    if (!near(block.left, desk.left) || !near(block.width, desk.width)) faults.push(`${block.id} is not the one column`);
  }
  for (let at = 1; at < shown.length; at += 1) {
    const above = shown[at - 1];
    const under = shown[at];
    if (!near(under.top - above.foot, gap)) faults.push(`${above.id}→${under.id} ${tenth(under.top - above.foot)}px, not ${gap}`);
  }
  return faults;
}

/* The hole under the shorter column, and the same for every placement the
 * heights allowed — the standing one must be the floor. */
function holesOf(facts) {
  const { heights, candidates, chosen } = modelOf(facts);
  const hole = (candidate) => {
    const h = heights;
    const columns = candidate.id === "split"
      ? [stackOf([h.mail, h.pipeline], facts.gap), stackOf([h.workers, h.release], facts.gap)]
      : candidate.id === "stack-left" ? [stackOf([h.mail, h.pipeline, h.release], facts.gap), h.workers]
        : [h.mail, stackOf([h.workers, h.pipeline, h.release], facts.gap)];
    return tenth(candidate.height - Math.min(...columns));
  };
  return { chosen: chosen.id, hole: hole(chosen), candidates: candidates.map((one) => `${one.id} ${one.height} (hole ${hole(one)})`) };
}

const settle = (page) => page.evaluate(async () => {
  for (let beat = 0; beat < 50 && (deskLedgerAsking || deskPaintFrame !== null); beat += 1) {
    await new Promise((done) => requestAnimationFrame(done));
  }
  await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
});

/* A resize alone: the desk's own watch on the scroll surface re-deals the
 * placement after the layout, without a poll. */
async function resize(page, width, height) {
  await page.setViewportSize({ width, height });
  await settle(page);
}

/* A page whose board stands as the whole window (the desk suite's glance check
 * does the same), every block answered, the paint settled. */
async function openDesk(browser, origin, fixture, { width = 1280, height = 800 } = {}) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  await page.setViewportSize({ width, height });
  await installBoardWaits(page);
  await page.evaluate(coordinatorDeskFixture, { now: Date.now(), ...fixture });
  await page.waitForSelector(".task-board-row");
  await page.evaluate(async () => {
    await window.__BOARD_SETTLED__();
    const view = document.querySelector("#board-view");
    Object.assign(view.style, { position: "fixed", inset: "0", width: "100vw", height: "100vh", zIndex: "100" });
    askMachineLoad();
    await new Promise((done) => setTimeout(done, 0));
    await askReleaseStatus();
    await paintBoardView();
    paintCoordinatorDesk(view);
    window.__HELD_ANSWERS__ = { machine: window.__MACHINE__, mail: window.__DESK__.mail, stages: window.__DESK__.stages,
      ledger: window.__LEDGER__, release: window.__RELEASE__ };
  });
  await settle(page);
  return { page, faults };
}

/* One state of the desk through the product's own answers: the strip through
 * `machine_load`, the mail and the stages through `board_desk`, the roster
 * through `ledger_agents`, the lane through `release_status`. */
async function showOnly(page, off) {
  await page.evaluate(async ({ off }) => {
    const hidden = new Set(off);
    const held = window.__HELD_ANSWERS__;
    window.__MACHINE__ = hidden.has("machine") ? null : held.machine;
    window.__DESK__ = { ...window.__DESK__, revision: window.__DESK__.revision + 1,
      mail: hidden.has("mail") ? [] : held.mail,
      stages: hidden.has("pipeline") ? held.stages.map((one) => ({ ...one, count: 0 })) : held.stages };
    window.__LEDGER__ = hidden.has("workers") ? [] : held.ledger;
    window.__RELEASE__ = hidden.has("release") ? null : held.release;
    askMachineLoad();
    await askReleaseStatus();
    refreshDeskLedger();
    await new Promise((done) => setTimeout(done, 0));
    paintCoordinatorDesk(document.querySelector("#board-view"));
  }, { off });
  await settle(page);
}

const STATES = [
  { name: "the strip hidden", off: ["machine"] },
  { name: "no mail, the workers standing (after t-7388)", off: ["mail"] },
  { name: "no workers", off: ["workers"] },
  { name: "no flow", off: ["pipeline"] },
  { name: "no lane", off: ["release"] },
  ...ALL.map((only) => ({ name: `only the ${only}`, off: ALL.filter((id) => id !== only) })),
  { name: "everything hidden", off: ALL },
  { name: "everything back", off: [] },
];

const summarize = (facts) => Object.fromEntries(shownOf(facts).map((block) => [block.id,
  `${block.top}–${block.bottom} (foot ${block.foot}, h ${block.height}${block.column ? `, ${block.column}` : ""}) × ${block.left}–${block.right}`]));

export async function testCoordinatorDeskLayout(browser, origin, ok) {
  await mkdir(SHOTS, { recursive: true });

  /* ---- 짧은 우편 열: 우편 3 · 워커 8 ---------------------------------------- */
  {
    const { page, faults } = await openDesk(browser, origin, { mail: 3, workers: 8 }, { width: 1920, height: 1080 });
    try {
      for (const [width, height] of [[1920, 1080], [1280, 800]]) {
        await resize(page, width, height);
        const facts = await readDesk(page);
        const { mail, workers, pipeline, release } = facts.blocks;
        const shown = shownOf(facts).map((block) => block.id).join();
        const holes = holesOf(facts);
        ok(`a_short_mail_column_does_not_push_the_pipeline_and_the_lane_below_the_fold (${width}×${height}: the flow and the lane follow the mail's foot in the left column, by their boxes)`,
          shown === "machine,mail,workers,pipeline,release" && facts.mailRows === 3 && facts.workerRows === 8 &&
          holes.chosen === "stack-left" &&
          near(pipeline.top - mail.foot, facts.gap) && near(release.top - pipeline.foot, facts.gap) &&
          near(pipeline.left, mail.left) && near(release.left, mail.left) &&
          pipeline.top < workers.bottom && release.top < workers.bottom &&
          near(workers.top, mail.top) && workers.left >= mail.right,
          JSON.stringify({ gap: facts.gap, shown, mailRows: facts.mailRows, workerRows: facts.workerRows, holes, ...summarize(facts) }));
        ok(`a_short_mail_column_does_not_push_the_pipeline_and_the_lane_below_the_fold (${width}×${height}: the flow's head is on the surface's first screen)`,
          facts.surface.scrollTop === 0 && facts.heads.pipeline !== null && facts.heads.pipeline.bottom <= facts.surface.bottom,
          JSON.stringify({ head: facts.heads.pipeline, surface: facts.surface }));
        const wide = wideFaults(facts);
        ok(`a_short_mail_column_does_not_push_the_pipeline_and_the_lane_below_the_fold (${width}×${height}: the placement is the lowest of the three, every box is its content, nothing overlaps or is cut, the list stands right under the desk)`,
          wide.length === 0 && holes.hole === Math.min(...holes.candidates.map((one) => Number(/hole ([\d.]+)/.exec(one)[1]))),
          JSON.stringify({ faults: wide, holes }));
        await page.screenshot({ path: `${SHOTS}/short-mail-${width}x${height}.png` });
      }
      const facts1080 = await (async () => { await resize(page, 1920, 1080); return readDesk(page); })();
      ok("a_short_mail_column_does_not_push_the_pipeline_and_the_lane_below_the_fold (1920×1080: the lane's head is on the first screen too)",
        facts1080.heads.release !== null && facts1080.heads.release.bottom <= facts1080.surface.bottom,
        JSON.stringify({ head: facts1080.heads.release, surface: facts1080.surface }));

      /* ---- 조용한 폴과 티어 넘기는 아무것도 쓰지 않는다 -------------------- */
      await resize(page, 1280, 800);
      const quiet = await deskMutations(page, async () => {
        await paintBoardView();
        paintCoordinatorDesk(document.querySelector("#board-view"));
      });
      ok("a quiet poll writes nothing on the desk: the placement it measures is the one standing", quiet === 0, `mutations ${quiet}`);
      await page.evaluate(() => {
        const surface = document.querySelector("#board-view .task-board-surface");
        const records = [];
        const watch = new MutationObserver((batch) => records.push(...batch));
        watch.observe(surface, { subtree: true, childList: true, attributes: true, characterData: true });
        window.__TIER_STOP__ = () => { records.push(...watch.takeRecords()); watch.disconnect(); return records.length; };
      });
      await resize(page, 700, 900);
      const at700 = await readDesk(page);
      await resize(page, 699, 900);
      const at699 = await readDesk(page);
      await resize(page, 1280, 800);
      const crossed = await page.evaluate(() => window.__TIER_STOP__());
      ok("the_list_tier_is_the_board's_own_699px (at 700 the workers stand beside the mail; at 699 one column in DOM order)",
        at700.boardWidth === 700 && at699.boardWidth === 699 &&
        wideFaults(at700).length === 0 && at700.blocks.workers.left >= at700.blocks.mail.right &&
        narrowFaults(at699).length === 0 &&
        shownOf(at699).map((block) => block.id).join() === "machine,mail,workers,pipeline,release" &&
        shownOf(at699).every((block, at, all) => at === 0 || block.top > all[at - 1].bottom),
        JSON.stringify({ at700: { width: at700.boardWidth, faults: wideFaults(at700), ...summarize(at700) },
          at699: { width: at699.boardWidth, faults: narrowFaults(at699), ...summarize(at699) } }));
      ok("crossing the tier and back writes nothing on the surface: the one column keeps the wide tier's placement, and the same heights re-deal the same columns",
        crossed === 0, `mutations ${crossed}`);
      await resize(page, 699, 900);
      await page.screenshot({ path: `${SHOTS}/short-mail-699x900.png` });
      await resize(page, 1280, 800);

      /* ---- 숨은 블록: 앞·중간·전부·재등장, 두 티어에서 ------------------------ */
      for (const [tier, width, height, faultsOf] of [["wide", 1280, 800, wideFaults], ["list", 699, 900, narrowFaults]]) {
        await resize(page, width, height);
        await showOnly(page, []);
        const whole = await readDesk(page);
        const lead = tenth(Math.min(...shownOf(whole).map((block) => block.top)) - whole.desk.top);
        const wholeFaults = faultsOf(whole, lead);
        ok(`a_hidden_block_leaves_no_empty_row (${tier} tier: every block shown is the reference)`,
          shownOf(whole).length === 5 && wholeFaults.length === 0, JSON.stringify({ lead, faults: wholeFaults, ...summarize(whole) }));
        for (const state of STATES) {
          await showOnly(page, state.off);
          const facts = await readDesk(page);
          const shown = shownOf(facts).map((block) => block.id);
          const expected = ALL.filter((id) => !state.off.includes(id));
          const found = faultsOf(facts, lead);
          const back = state.off.length === 0
            ? ALL.filter((id) => !near(facts.blocks[id].top, whole.blocks[id].top) || !near(facts.blocks[id].bottom, whole.blocks[id].bottom)
              || !near(facts.blocks[id].left, whole.blocks[id].left) || !near(facts.blocks[id].right, whole.blocks[id].right))
            : [];
          ok(`a_hidden_block_leaves_no_empty_row (${tier} tier, ${state.name})`,
            shown.join() === expected.join() && found.length === 0 && back.length === 0,
            JSON.stringify({ shown, expected, faults: found, moved: back, deskHidden: facts.deskHidden,
              placement: tier === "wide" ? holesOf(facts).chosen : "one column", ...summarize(facts) }));
        }
      }
      await resize(page, 1280, 800);
      await showOnly(page, ["mail"]);
      await page.screenshot({ path: `${SHOTS}/no-mail-1280x800.png` });
      await showOnly(page, ["workers"]);
      await page.screenshot({ path: `${SHOTS}/no-workers-1280x800.png` });
      await showOnly(page, []);
      ok("the short mail column raises no browser errors", faults.length === 0, faults.join("\n"));
    } finally {
      await page.close();
    }
  }

  /* ---- 기본 픽스처: 우편 20(5행 보임) · 워커 5 — t-6588의 첫 화면 약속 그대로 ---- */
  {
    const { page, faults } = await openDesk(browser, origin, {}, { width: 1998, height: 1069 });
    try {
      const facts = await readDesk(page);
      const holes = holesOf(facts);
      const wide = wideFaults(facts);
      ok("the_default_desk_splits_the_flow_and_the_lane (mail 5 rows beside workers 5: the flow under the mail, the lane under the workers, the hole under the shorter column no deeper than one gap)",
        facts.mailRows === 5 && facts.workerRows === 5 && holes.chosen === "split" && holes.hole <= facts.gap && wide.length === 0,
        JSON.stringify({ holes, faults: wide, ...summarize(facts) }));
      ok("the_default_desk_splits_the_flow_and_the_lane (on a 1998×1069 board the first task row is on the first screen, as t-6588 promised)",
        facts.firstRow !== null && facts.firstRow.top < facts.surface.bottom && facts.firstRow.top >= facts.desk.bottom,
        JSON.stringify({ firstRow: facts.firstRow, desk: facts.desk, surface: facts.surface }));
      await page.screenshot({ path: `${SHOTS}/default-1998x1069.png` });
      ok("the default desk raises no browser errors", faults.length === 0, faults.join("\n"));
    } finally {
      await page.close();
    }
  }

  /* ---- 긴 우편 열: 우편 20 · 워커 2, 답을 쓰는 채로 ---------------------------- */
  {
    const { page, faults } = await openDesk(browser, origin, { mail: 20, workers: 2 });
    try {
      const folded = await readDesk(page);
      await page.click(`${REPLY} .board-desk-letter-act`);
      await page.fill(`${REPLY} .board-desk-reply-field`, "그대로 main에 올리세요.");
      await page.evaluate((selector) => {
        const field = document.querySelector(`${selector} .board-desk-reply-field`);
        field.focus();
        field.setSelectionRange(3, 3);
        const desk = document.querySelector("#board-view .task-board-desk");
        window.__HELD_NODES__ = { field, letter: document.querySelector(selector),
          blocks: Object.fromEntries([...desk.children].map((node) => [node.dataset.deskBlock, node])) };
      }, REPLY);
      await page.evaluate(() => document.querySelector('#board-view [data-desk-block="mail"] .board-desk-letters-more').click());
      await settle(page);
      const unfolded = await readDesk(page);
      const { mail, workers, pipeline, release } = unfolded.blocks;
      const unfoldedFaults = wideFaults(unfolded);
      const unfoldedHoles = holesOf(unfolded);
      ok("a_long_mail_column_keeps_the_workers_beside_it (twenty letters unfolded: the workers stand at the top of the right column, the flow and the lane stack under them, the mail's foot ends the desk)",
        folded.mailRows === 5 && unfolded.mailRows === 20 && mail.height > folded.blocks.mail.height &&
        unfoldedHoles.chosen === "stack-right" &&
        near(workers.top, mail.top) && workers.left >= mail.right && workers.bottom < mail.foot &&
        near(pipeline.top - workers.foot, unfolded.gap) && near(release.top - pipeline.foot, unfolded.gap) &&
        release.bottom < mail.foot && near(unfolded.desk.bottom, mail.bottom) &&
        unfoldedFaults.length === 0,
        JSON.stringify({ foldedRows: folded.mailRows, rows: unfolded.mailRows, holes: unfoldedHoles, faults: unfoldedFaults, ...summarize(unfolded) }));
      await page.screenshot({ path: `${SHOTS}/long-mail-unfolded-1280x800.png`, fullPage: false });
      await page.evaluate(async () => {
        const rows = window.__LEDGER__;
        for (let n = rows.length + 1; n <= 8; n += 1) {
          rows.push({ ...rows[(n - 1) % 2], worker: `w-${n}`, task_id: `t-${n}`, task: `데스크 과업 ${n}`, term: null, pane: `%${n}`, dispatch_id: `dp-${n}` });
        }
        window.__DESK__ = { ...window.__DESK__, revision: window.__DESK__.revision + 1 };
        refreshDeskLedger();
        await new Promise((done) => setTimeout(done, 0));
      });
      await settle(page);
      const grown = await readDesk(page);
      const grownFaults = wideFaults(grown);
      ok("a_long_mail_column_keeps_the_workers_beside_it (the roster grew from two to eight: the right column re-stacks under it with the same gaps, the mail column did not move)",
        grown.workerRows === 8 && grown.blocks.workers.height > unfolded.blocks.workers.height &&
        holesOf(grown).chosen === "stack-right" &&
        near(grown.blocks.mail.top, unfolded.blocks.mail.top) && near(grown.blocks.mail.foot, unfolded.blocks.mail.foot) &&
        near(grown.blocks.pipeline.top - grown.blocks.workers.foot, grown.gap) && grownFaults.length === 0,
        JSON.stringify({ rows: grown.workerRows, holes: holesOf(grown), faults: grownFaults, ...summarize(grown) }));
      const quiet = await deskMutations(page, async () => {
        await paintBoardView();
        paintCoordinatorDesk(document.querySelector("#board-view"));
      });
      ok("a quiet poll writes nothing while a reply is being written in the unfolded column", quiet === 0, `mutations ${quiet}`);
      await resize(page, 699, 900);
      const narrow = await readDesk(page);
      const narrowFound = narrowFaults(narrow);
      ok("a_long_mail_column_keeps_the_workers_beside_it (the list tier stacks the twenty letters, the workers and the rest in DOM order)",
        narrow.mailRows === 20 && narrowFound.length === 0 &&
        shownOf(narrow).map((block) => block.id).join() === "machine,mail,workers,pipeline,release",
        JSON.stringify({ faults: narrowFound, ...summarize(narrow) }));
      await resize(page, 1280, 800);
      await page.evaluate(() => document.querySelector('#board-view [data-desk-block="mail"] .board-desk-letters-more').click());
      await settle(page);
      const refolded = await readDesk(page);
      const kept = await page.evaluate((selector) => {
        const field = document.querySelector(`${selector} .board-desk-reply-field`);
        const desk = document.querySelector("#board-view .task-board-desk");
        return { sameField: field === window.__HELD_NODES__.field, value: field?.value, focused: document.activeElement === field,
          caret: [field?.selectionStart, field?.selectionEnd],
          sameLetter: document.querySelector(selector) === window.__HELD_NODES__.letter,
          sameBlocks: [...desk.children].every((node) => window.__HELD_NODES__.blocks[node.dataset.deskBlock] === node),
          order: [...desk.children].map((node) => node.dataset.deskBlock).join() };
      }, REPLY);
      ok("a reply being written keeps its node, its words, its focus and its caret through the unfold, the roster's growth, a quiet poll, the tier crossing and the fold",
        kept.sameField && kept.value === "그대로 main에 올리세요." && kept.focused && kept.caret.join() === "3,3" &&
        kept.sameLetter && kept.sameBlocks && kept.order === "machine,mail,workers,pipeline,release" && refolded.mailRows === 5,
        JSON.stringify({ kept, rows: refolded.mailRows }));
      const refoldedFaults = wideFaults(refolded);
      ok("a_long_mail_column_keeps_the_workers_beside_it (folded back to five with the reply open: the placement the heights now call for stands, re-dealt on the same nodes)",
        refoldedFaults.length === 0 && near(refolded.blocks.workers.top, refolded.blocks.mail.top),
        JSON.stringify({ holes: holesOf(refolded), faults: refoldedFaults, ...summarize(refolded) }));
      await page.screenshot({ path: `${SHOTS}/long-mail-folded-1280x800.png` });
      ok("the long mail column raises no browser errors", faults.length === 0, faults.join("\n"));
    } finally {
      await page.close();
    }
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
