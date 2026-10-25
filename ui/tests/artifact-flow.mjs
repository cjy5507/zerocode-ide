/* The Artifacts tab's flow (t-68458) — the second draft the person passed (artifact p-4958d4720ba9e103
 * v3), drawn by the window. The tab opens on the flow of the project in view: three groups — now,
 * before the next release, released — one line a task saying what was done with its before → after
 * number, its six steps and its state, its artifacts as chips. A line unfolds into what · why · cost and
 * its steps in time; a chip stands its artifact in the reader beside it, whose head says which step of
 * which attempt it is.
 *
 * The draft's own measure, by the draft's own rules (reports/t-68338/src/measure.js), on the 1440×900
 * first screen at the tab's real width: sentence lines ≥ 6, figures ≤ 4, groups 3, type sizes ≤ 4, and
 * one press — opening the tab — to read what t-501 did and where it stands. Then the unfolded steps, the
 * reader's head, a release unfolded, the records with no task, a search, an empty project, a folder in no
 * repository the window lists, a repaint that writes nothing, thirty tasks and twelve releases within 400
 * elements and 3 ms a paint with the CPU slowed four times, the screen rules (12 px text, 28 px presses),
 * axe and 4.5:1 in both themes, the four catalogs.
 *
 * The scene is the draft's (reports/t-68338/src/data.js), laid over the synthetic catalog and tasks of
 * `artifact-cards.mjs` and `artifact-tasks.mjs`. Every read of the page is defensive: on a tree that has
 * not built the flow the checks fail at their assertions, one line each, and the suite runs to its end.
 *
 *   node ui/tests/artifact-flow.mjs
 *   WINDOW_SUITES=artifact-flow node ui/tests/window.mjs */
import { createRequire } from "node:module";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { createWindowServer, launchWindowBrowser, openWindowTestPage } from "./window-boot.mjs";
import { installHarnessWaits } from "./harness-waits.mjs";
import { REAL_WINDOW, standCatalog } from "./artifact-cards.mjs";
import { standTasks } from "./artifact-tasks.mjs";
import { axeViolations, contrastTable } from "./settings-quality.mjs";
import { median } from "./coordinator-desk-perf.mjs";

/* The draft's lines for the first screen (t-68338 report, 「처음 연 화면」: 6 · 4 · 3 · 3). */
const SENTENCES_MIN = 6;
const FIGURES_MAX = 4;
const GROUPS = 3;
const TYPE_SIZES_MAX = 4;
const CONTRAST_MIN = 4.5;
/* The screen rules every view of the tab keeps (artifact-cards.mjs, artifact-tasks.mjs). */
const TEXT_MIN_PX = 12;
const PRESS_MIN_PX = 28;
/* The narrow window the draft was drawn at too: 720 wide, the right panel folded. */
const NARROW_WINDOW = Object.freeze({ width: 720, height: 900 });
/* Thirty tasks and twelve releases: the weight the briefing names, and its two budgets. */
const HEAVY = Object.freeze({ tasks: 30, releases: 12 });
const ELEMENTS_MAX = 400;
const PAINT_MS_MAX = 3;
const PAINT_ROUNDS = 15;
/* The low-spec profile's cpu4x level (tools/low-spec/profile.mjs). A measuring run on the efficiency
 * cores (`taskpolicy -b`, the t-56141 way) sets `ARTIFACT_FLOW_CPU_THROTTLE=1` to read those cores alone. */
const CPU_THROTTLE = Number(process.env.ARTIFACT_FLOW_CPU_THROTTLE ?? 4);
/* How long a step waits for the window: a frame or two of layout and the fake runtime's answer. */
const SETTLED_MS = 300;

/* The flow the runtime answers for the project in view — the draft's scene, in the catalog's own task
 * ids, so a chip opens the rows `standCatalog` and `standTasks` stand, and its records are the catalog's
 * rows no task is linked to. `heavy` lays thirty tasks over twelve releases instead; `empty` is a project
 * with nothing in it, and `outside` a folder in no repository the window lists, which the runtime asked
 * git nothing for (t-69070 P2). A page function: no closure. */
export function standFlow(page, { heavy = null, empty = false, outside = false, stale = false } = {}) {
  return page.evaluate(({ heavy, empty, outside, stale }) => {
    const now = Date.now();
    const minute = 60 * 1000;
    const ago = (minutes) => now - minutes * minute;
    const parts = (held = {}) => ({ reports: 0, pages: 0, files: 0, pictures: 0, logs: 0, other: 0, missing: 0, ...held });
    const chips = ({ parts: held = {}, ...rest } = {}) => ({
      report: null, page: null, pairs: 0, singles: 0, sessions: 0, attempts: 0, ...rest, parts: parts(held),
    });
    const sha = (n) => n.toString(16).padStart(8, "0").repeat(5);
    const attempt = (n, worker, [agent, model], [from, to], succeeded, said = null) => ({
      dispatch: `dp-${n}`, worker, agent, model, started_ms: ago(from), ended_ms: to === null ? null : ago(to),
      succeeded, source: succeeded ? sha(n) : null, said,
    });
    const line = (task, said, stage, more = {}) => ({
      task, run: "run-9", title: said, said, said_from: "summary", gain: null, stage, ledger_stage: stage, three: null,
      summary: null, opened_ms: ago(600), last_ms: ago(60), attempts: [], hand_in: null, review: null, land: null, ship: null,
      chips: chips(), ...more,
    });
    const CLAUDE = ["claude", "claude-sonnet-5-5"];
    const shipped = (task, title, said, version, minutes, more = {}) => line(task, said, "deployed", {
      title, opened_ms: ago(minutes + 300), last_ms: ago(minutes + 60),
      attempts: [attempt(Number(task.slice(2)), `w-${task.slice(2)}`, CLAUDE, [minutes + 240, minutes + 120], true)],
      land: { at_ms: ago(minutes + 30), commit: sha(Number(task.slice(2)) + 1), line: null, told: "git" },
      ship: { version, at_ms: ago(minutes), told: "git" },
      chips: chips({ report: `r-${task.slice(2)}`, parts: { reports: 1 }, attempts: 1 }), ...more,
    });
    const group = (tasks, records = []) => ({ tasks, more: 0, records });
    const release = (version, minutes, tasks) => ({ version, commit: sha(minutes), at_ms: ago(minutes), more: 0, records: [], tasks });
    let flow;
    if (empty) {
      flow = { root: activeProjectPath, base: "main", now: group([]), next: group([]), releases: [], older: null, ledger: true,
        sources: { said: {}, land: {}, ship: {} } };
    } else if (heavy) {
      // Thirty tasks: three now, three before the next release, the rest two to a release.
      const working = ["dispatched", "reported", "verified"].map((stage, n) => line(`t-80${n}`, `지금 하는 일을 한 문장으로 말한다 ${n}`, stage));
      const landed = [3, 4, 5].map((n) => line(`t-80${n}`, `main에 넣은 일을 한 문장으로 말한다 ${n}`, "merged", {
        land: { at_ms: ago(n * 60), commit: sha(n), line: null, told: "git" },
      }));
      const releases = [...Array(heavy.releases).keys()].map((n) => {
        const version = `v3.${heavy.releases - n}.0`;
        const minutes = (12 + n * 24) * 60;
        return release(version, minutes, [0, 1].map((k) => shipped(`t-${810 + n * 2 + k}`, "발행된 일의 이름", "발행된 일을 한 문장으로 말한다", version, minutes)));
      });
      flow = { root: activeProjectPath, base: "main", now: group(working), next: group(landed), releases, older: null, ledger: true,
        sources: { said: { summary: heavy.tasks }, land: { git: heavy.tasks - 3 }, ship: { git: heavy.tasks - 6 } } };
    } else {
      const working = [
        line("t-506", "닫기 시간 상한을 설정 화면에서 고르게 바꾸는 중이다", "dispatched", {
          last_ms: ago(40), attempts: [attempt(61, "w-31", ["claude", "claude-opus-5-5"], [40, null], null)],
        }),
        line("t-504", "작업 묶음 화면의 시안을 페이지로 만들었다 — 사람의 승인을 기다린다", "reported", {
          title: "작업 묶음 시안", last_ms: ago(170),
          attempts: [attempt(58, "w-15", ["zo", "claude-opus-5-5"], [210, 170], true, "시안 페이지 하나를 넘겼다")],
          hand_in: { attempt: "dp-58", source: "p-demo", at_ms: ago(170) },
          chips: chips({ page: "p-demo", parts: { pages: 1 } }),
        }),
        line("t-700", "검색 결과에 찾은 줄과 그 앞뒤 한 줄이 보이게 했다", "verified", {
          title: "검색 결과에 본문 줄 보이기", gain: "검색 42 → 38 ms", last_ms: ago(250),
          attempts: [attempt(41, "w-21", ["codex", "gpt-6-astra"], [380, 250], true)],
          hand_in: { attempt: "dp-41", source: sha(41), at_ms: ago(250) }, review: { at_ms: ago(220), source: sha(41) },
          chips: chips({ report: "r-700", parts: { reports: 1, logs: 1 }, pairs: 1 }),
        }),
      ];
      const landed = [
        line("t-501", "큐를 닫을 때 남은 일이 기록 없이 사라지던 것을 고쳤다", "merged", {
          title: "큐 닫기 재시도 — 닫을 때 남은 일을 비운다", gain: "사라진 일 7 → 0건", opened_ms: ago(470), last_ms: ago(70),
          attempts: [
            attempt(11, "w-11", CLAUDE, [455, 410], false, "닫기가 큐를 기다리지 않는 원인을 못 찾고 멈췄다"),
            attempt(12, "w-12", CLAUDE, [405, 320], true, "닫기가 남은 일을 먼저 비우고, 넘치면 기록하고 닫게 고쳤다"),
          ],
          hand_in: { attempt: "dp-12", source: sha(12), at_ms: ago(320) }, review: { at_ms: ago(270), source: sha(12) },
          land: { at_ms: ago(70), commit: sha(13), line: null, told: "git" },
          three: {
            what: "닫기 요청이 오면 남은 일을 처리하고, 나머지는 기록에 남긴 뒤 닫는다.",
            why: "닫을 때 큐에 남은 일이 기록 없이 사라졌다.",
            cost: "시간 안에 못 끝낸 일은 다음 실행으로 미룬다.",
          },
          chips: chips({ report: "r-final", page: "p-linked", parts: { reports: 2, pages: 1, files: 6, pictures: 3, logs: 2, other: 1 }, pairs: 1, attempts: 2 }),
        }),
        line("t-45392", "일곱 자리 과업 번호도 번호 칸 안에 맞게 그렸다", "merged", {
          title: "일곱 자리 번호 줄", last_ms: ago(240), land: { at_ms: ago(210), commit: sha(14), line: null, told: "ledger" },
          attempts: [attempt(44, "w-24", CLAUDE, [360, 240], true)],
          chips: chips({ report: "r-45392", parts: { reports: 1, pictures: 1 }, singles: 1 }),
        }),
        line("t-701", "터미널에 찍힌 파일 경로를 누르면 그 파일이 편집기에서 열리게 했다", "merged", {
          title: "터미널 링크를 눌러 파일 열기", last_ms: ago(360), land: { at_ms: ago(330), commit: sha(15), line: null, told: "git" },
          attempts: [attempt(31, "w-17", CLAUDE, [480, 360], true)],
          chips: chips({ report: "r-701", parts: { reports: 1, logs: 1 } }),
        }),
      ];
      const releases = [
        release("v2.4.0", 650, [
          shipped("t-702", "빌드 줄 대기 시간 줄이기", "빌드 줄의 대기 시간을 줄였다", "v2.4.0", 650, { gain: "평균 14 → 6분" }),
          shipped("t-703", "설정 화면의 글자 크기 맞추기", "설정 화면의 글자 크기를 본문과 같게 맞췄다", "v2.4.0", 650, { gain: "12 → 13 px" }),
          shipped("t-704", "내보내기 메뉴의 파일 이름", "내보내기가 파일 이름을 문서 제목으로 짓게 했다", "v2.4.0", 650),
          shipped("t-705", "주석을 만든 판으로 돌려주기", "주석을 단 판으로 답이 돌아가게 했다", "v2.4.0", 650),
        ]),
        release("v2.3.2", 1750, [
          shipped("t-706", "목차가 긴 보고서에서만 서게", "목차가 두 화면을 넘는 보고서에서만 서게 했다", "v2.3.2", 1750),
          shipped("t-707", "전후 비교 손잡이를 경계 위로", "전후 비교의 손잡이를 경계선 위로 옮겼다", "v2.3.2", 1750),
          shipped("t-708", "사라진 파일 줄을 바닥으로", "사라진 파일 줄을 목록 바닥으로 내렸다", "v2.3.2", 1750),
        ]),
        release("v2.3.1", 4410, [
          shipped("t-503", "옛 빌드가 적은 보고서", "옛 빌드가 쓴 보고서도 어느 과업 것인지 찾아 보이게 했다", "v2.3.1", 4410),
          shipped("t-709", "작업 칩으로 세 탭 거르기", "과업 칩을 누르면 세 탭이 함께 그 과업으로 걸러지게 했다", "v2.3.1", 4410),
          shipped("t-710", "빈 탭이 무엇이 없는지 말하기", "빈 탭이 무엇이 없는지 한 줄로 말하게 했다", "v2.3.1", 4410),
        ]),
        release("v2.3.0", 8880, [
          shipped("t-711", "보고서 제목을 첫 머리말에서", "보고서 제목을 첫 머리말에서 가져오게 했다", "v2.3.0", 8880),
          shipped("t-712", "새로 만들기 메뉴 정리", "새로 만들기 메뉴를 네 갈래로 줄였다", "v2.3.0", 8880),
          shipped("t-713", "썸네일 줄의 상한", "썸네일을 한 번에 두 장까지만 만들게 했다", "v2.3.0", 8880, { gain: "탭 열기 1.9 → 0.6 s" }),
          shipped("t-714", "세부 정보 접기", "세부 정보를 머리띠 아래로 접었다", "v2.3.0", 8880),
          shipped("t-715", "카드 풀이 노드를 다시 쓰게", "카드를 다시 그릴 때 노드를 새로 만들지 않게 했다", "v2.3.0", 8880),
        ]),
      ];
      // The records are the catalog's own rows no task is linked to — the reader opens them as it would.
      const catalog = new Map(window.__ANSWER__.artifacts_list({ filter: {} }).rows.map((row) => [row.id, row]));
      const records = ["p-loose", "d-loose", "r-brief", "e-steps"].map((id) => catalog.get(id)).filter(Boolean);
      flow = {
        root: activeProjectPath, base: "main", now: group(working), next: group(landed, records), releases,
        older: { releases: 5, newest: "v2.2.4", oldest: "v2.2.0", tasks: 44, until_ms: ago(20160) },
        ledger: true, sources: { said: { summary: 22 }, land: { git: 20, ledger: 1 }, ship: { git: 15 } },
      };
    }
    // Whether the runtime asked git: only for a folder in a repository the window lists (t-69070 P2).
    flow.git = outside ? "outside" : stale ? "stale" : "kept";
    if (outside) flow.base = null;
    const seen = (window.__FLOW__ = { asks: [], flow });
    window.__ANSWER__.artifact_flow = (args) => {
      seen.asks.push(JSON.parse(JSON.stringify(args?.ask ?? null)));
      return JSON.parse(JSON.stringify(seen.flow));
    };
    return { lines: [flow.now, flow.next, ...flow.releases].reduce((sum, one) => sum + one.tasks.length, 0), releases: flow.releases.length };
  }, { heavy, empty, outside, stale });
}

/* Open the tab fresh, as a person does from the sidebar: nothing picked, nothing unfolded. */
export async function openFlow(page) {
  await page.evaluate(async (settled) => {
    dropTab("artifacts");
    Object.assign(artifactFilter, { tab: ARTIFACT_FLOW_TAB, query: "", origin: null, project: null, agent: null, period: "all", showMissing: false });
    artifactTabPicked = false;
    closeArtifactFlowReading();
    for (const held of [artifactFlowOpen, artifactFlowReleasesOpen, artifactFlowRecordsOpen]) held.clear();
    artifactSelectedId = null;
    el("nav-artifacts").click();
    await window.__PAINTED__();
    await window.__UNTIL__(() => !artifactAsking && artifactFlowAnswer !== null, "the flow's answer");
    await new Promise((done) => setTimeout(done, settled));
  }, SETTLED_MS);
}

/* The draft's measure of a first screen (reports/t-68338/src/measure.js `measureNow`), on the tab: what
 * stands above the fold, inside every scroll pane it sits in. A figure is a number an element says in
 * its own text — a count, a time, a size — and not one inside a task id, a version, a hash or a file
 * name; a person's sentence (a line's words, its three lines, a step's words, a release's digest, the
 * reader's context) is read, not counted. A sentence line is an element whose words are ten characters
 * or more of Korean ending in 「~다 · ~요」, counted once where they nest. A page function. */
function flowFirstScreen() {
  const root = artifactsView();
  const fold = window.innerHeight;
  const groups = ".artifact-flow-group-head";
  const content = [".artifact-flow-said", ".artifact-flow-three", ".artifact-flow-step-said", ".artifact-flow-release-digest",
    ".artifact-flow-context-said", ".artifact-detail-title", ".artifact-bundle-page-title", ".artifact-preview-md",
    ".artifact-preview-text", ".artifact-digest-rows", ".artifact-outline-now"].join(", ");
  const NUM = /(?<![\w\-.,%/#:])\d[\d,]*(?:[:.]\d+)*(?![\w%-]|\.[A-Za-z])/g;
  const DATETIME = /\d{1,2}\.\s?\d{1,2}\.\s?\d{1,2}:\d{2}/g;
  const SAID = /[가-힣]/;
  const SAID_END = /(다|요)(?=[\s.,。!—–:;)\-]|$)/;
  const ownText = (el) => [...el.childNodes].filter((one) => one.nodeType === 3).map((one) => one.textContent).join("").replace(/\s+/g, " ").trim();
  const clipped = (el) => {
    const box = el.getBoundingClientRect();
    for (let up = el.parentElement; up; up = up.parentElement) {
      const style = getComputedStyle(up);
      if (/(auto|scroll|hidden|clip)/.test(style.overflowX + style.overflowY) || up === root) {
        const edge = up.getBoundingClientRect();
        if (box.bottom <= edge.top + 1 || box.top >= edge.bottom - 1 || box.right <= edge.left + 1 || box.left >= edge.right - 1) return true;
      }
      if (up === root) break;
    }
    return false;
  };
  const shown = (el) => {
    if (!el.getClientRects().length) return false;
    if (!el.checkVisibility({ visibilityProperty: true, contentVisibilityAuto: true, opacityProperty: false })) return false;
    if (el.closest("details:not([open])") && !el.closest("summary")) return false;
    const box = el.getBoundingClientRect();
    if (box.width <= 1 || box.height <= 1 || box.top >= fold) return false;
    if (getComputedStyle(el).visibility === "hidden") return false;
    return !clipped(el);
  };
  const all = [...(root?.querySelectorAll("*") ?? [])].filter((el) => !el.parentElement?.closest("svg") && shown(el));
  const sizes = new Set();
  const counted = [];
  const saidAt = [];
  let figures = 0;
  for (const el of all) {
    const own = ownText(el);
    if (!own || el.closest("svg")) continue;
    sizes.add(parseFloat(getComputedStyle(el).fontSize));
    const whole = el.textContent.replace(/\s+/g, " ").trim();
    if (whole.length >= 10 && SAID.test(whole) && SAID_END.test(whole)) saidAt.push(el);
    if (el.closest(content)) continue;
    const found = own.replace(DATETIME, " 0 ").match(NUM) ?? [];
    figures += found.length;
    if (found.length > 0) counted.push(own.slice(0, 40));
  }
  const said = saidAt.filter((el) => {
    for (let up = el.parentElement; up && up !== root; up = up.parentElement) if (saidAt.includes(up)) return false;
    return true;
  });
  return {
    figures, counted, said: said.length, saidLines: said.map((el) => el.textContent.trim().slice(0, 40)),
    groups: all.filter((el) => el.matches(groups)).length,
    sizes: [...sizes].sort((a, b) => b - a),
    titled: [...(root?.querySelectorAll(".artifacts-flow [title]") ?? [])].length,
    tipless: [...(root?.querySelectorAll(".artifacts-flow button.artifact-flow-more") ?? [])].filter((one) => !one.dataset.tip).length,
  };
}

/* The screen rules over the whole tab, as the task tab's suite reads them (artifact-tasks.mjs): text
 * outside pictures at 12 px or more, and everything a person presses at 28 px or more. A page function. */
export function flowScreenRules() {
  const view = artifactsView();
  const shown = (node) => Boolean(node) && !node.closest("[hidden]") && node.getClientRects().length > 0;
  const name = (node) => String(node.className?.baseVal ?? node.className ?? node.tagName).split(" ").slice(0, 2).join(".");
  const picture = (node) => Boolean(node.closest('[class*="agent-ico"], svg'));
  const small = new Map();
  for (const node of view?.querySelectorAll("*") ?? []) {
    if (!shown(node) || picture(node)) continue;
    const own = [...node.childNodes].some((child) => child.nodeType === 3 && child.textContent.trim() !== "");
    if (own && parseFloat(getComputedStyle(node).fontSize) < 12) small.set(name(node), parseFloat(getComputedStyle(node).fontSize));
  }
  const short = new Map();
  for (const node of view?.querySelectorAll('button, select, input, summary, a[href], [role="tab"], [role="option"]') ?? []) {
    if (!shown(node) || node.closest(".md-body")) continue;
    const height = Math.round(node.getBoundingClientRect().height);
    if (height < 28) short.set(name(node), height);
  }
  return { small: [...small].map(([key, px]) => `${key}:${px}`), short: [...short].map(([key, px]) => `${key}:${px}`) };
}

export async function testArtifactFlow(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  const axeModule = createRequire(import.meta.url)("@axe-core/playwright");
  const AxeBuilder = axeModule.default ?? axeModule;
  const settle = () => page.evaluate((ms) => new Promise((done) => setTimeout(done, ms)), SETTLED_MS);
  try {
    await page.setViewportSize(REAL_WINDOW);
    await installHarnessWaits(page);
    await standCatalog(page);
    await standTasks(page);
    await standFlow(page);
    await openFlow(page);

    /* ---- the tab opens on the flow of the project in view ------------------------------------------ */
    const opened = await page.evaluate(() => {
      const view = artifactsView();
      const shown = (node) => Boolean(node) && !node.closest("[hidden]") && node.getClientRects().length > 0;
      const here = projects.find((one) => one.path === activeProjectPath) ?? null;
      return {
        tab: artifactFilter.tab,
        selected: view?.querySelector('[data-artifact-tab][aria-selected="true"]')?.dataset.artifactTab ?? null,
        flow: shown(view?.querySelector(".artifacts-flow")),
        grid: shown(view?.querySelector(".artifacts-grid")),
        tasks: shown(view?.querySelector(".artifacts-tasks")),
        drawer: shown(view?.querySelector(".artifacts-drawer")),
        asked: window.__FLOW__.asks.at(-1) ?? null,
        here: here?.path ?? null,
        project: view?.querySelector(".artifacts-project-name")?.textContent ?? "",
        projectLabel: here ? here.name ?? here.path.split("/").at(-1) : "",
        hereWord: shown(view?.querySelector(".artifacts-here")) ? view.querySelector(".artifacts-here").textContent : "",
        counts: [...(view?.querySelectorAll(".artifacts-tab-count") ?? [])].filter(shown).length,
        gallery: artifactFilter.project,
        outside: shown(view?.querySelector(".artifact-flow-outside")),
      };
    });
    ok("the tab opens on the flow — the flow's tab chosen, the cards and the task list away, no reader yet",
      opened.tab === "flow" && opened.selected === "flow" && opened.flow && !opened.grid && !opened.tasks && !opened.drawer,
      JSON.stringify(opened));
    ok("the flow is asked for the project the sidebar is looking at, and the head names it as the one in view",
      opened.here !== null && opened.asked?.root === opened.here && Array.isArray(opened.asked?.roots) && opened.asked.roots.includes(opened.here)
        && opened.project === opened.projectLabel && opened.hereWord === "지금 보는 프로젝트",
      JSON.stringify(opened));
    ok("the full list keeps its own filters: the gallery's project stays 「전체」 while the flow stands on the one in view",
      opened.gallery === null, JSON.stringify(opened));
    ok("no tab carries a count while the flow stands — the first screen's numbers are the groups'", opened.counts === 0, JSON.stringify(opened));
    ok("a project in a listed repository says nothing of git not being asked", !opened.outside, JSON.stringify(opened));

    /* ---- the first screen: the draft's lines, and t-501 read in one press -------------------------- */
    const first = await page.evaluate(flowFirstScreen);
    ok(`the first screen says what was done in at least ${SENTENCES_MIN} sentences (${first.said})`,
      first.said >= SENTENCES_MIN, JSON.stringify(first));
    ok(`the first screen carries at most ${FIGURES_MAX} figures (${first.figures})`, first.figures <= FIGURES_MAX, JSON.stringify(first));
    ok(`the first screen stands in ${GROUPS} groups — now, before the next release, released`, first.groups === GROUPS, JSON.stringify(first));
    ok(`the first screen uses at most ${TYPE_SIZES_MAX} type sizes (${first.sizes.join(" · ")} px)`, first.sizes.length <= TYPE_SIZES_MAX,
      JSON.stringify(first));
    ok("the flow's buttons speak through data-tip, never a native title", first.titled === 0 && first.tipless === 0, JSON.stringify(first));
    const answered = await page.evaluate(() => {
      const view = artifactsView();
      const row = view?.querySelector('.artifact-flow-item[data-task="t-501"]') ?? null;
      const box = row?.getBoundingClientRect();
      return {
        inView: Boolean(box) && box.top >= 0 && box.bottom <= window.innerHeight,
        said: row?.querySelector(".artifact-flow-said")?.textContent ?? "",
        state: row?.querySelector(".artifact-flow-state-word")?.textContent ?? "",
        group: row?.closest(".artifact-flow-group")?.dataset.group ?? null,
        track: [...(row?.querySelectorAll(".artifact-flow-track rect") ?? [])].map((one) => one.getAttribute("class")),
        chips: [...(row?.querySelectorAll(".artifact-flow-chip") ?? [])].map((one) => one.dataset.flowChip),
        who: row?.querySelector(".artifact-flow-who")?.textContent ?? "",
        groups: [...(view?.querySelectorAll(".artifact-flow-group") ?? [])].map((one) => one.dataset.group),
      };
    });
    ok("one press — opening the tab — reads what t-501 did and where it stands: its sentence, its number, 「반영됨」 before the next release",
      answered.inView && answered.said.includes("사라지던 것을 고쳤다") && answered.said.includes("7 → 0건") && answered.state === "반영됨"
        && answered.group === "next" && answered.groups.join(",") === "now,next,shipped",
      JSON.stringify(answered));
    ok("a line's six steps say the work landed and waits for a release, its chips name what it left, and who carried it how",
      answered.track.join(",") === "is-done,is-done,is-done,is-done,is-done,is-now"
        && answered.chips.join(",") === "report,evidence,pair,page" && answered.who.includes("두 번째 시도에"),
      JSON.stringify(answered));

    /* ---- a line unfolds into what · why · cost and its steps in time ------------------------------- */
    const unfolded = await page.evaluate(async (settled) => {
      const view = artifactsView();
      view?.querySelector('.artifact-flow-item[data-task="t-501"] .artifact-flow-more')?.click();
      await new Promise((done) => setTimeout(done, settled));
      const row = view?.querySelector('.artifact-flow-item[data-task="t-501"]') ?? null;
      const steps = [...(row?.querySelectorAll(".artifact-flow-step") ?? [])];
      return {
        expanded: row?.getAttribute("aria-expanded") ?? null,
        three: [...(row?.querySelectorAll(".artifact-flow-three dt") ?? [])].map((one) => one.textContent),
        steps: steps.map((one) => one.dataset.step),
        tones: steps.map((one) => one.dataset.tone),
        names: steps.map((one) => one.querySelector(".artifact-flow-step-name")?.textContent ?? ""),
        attemptRows: [...(steps[2]?.querySelectorAll("[data-flow-row]") ?? [])].map((one) => one.dataset.flowRow),
        ship: steps.at(-1)?.querySelector(".artifact-flow-step-said")?.textContent ?? "",
        landTip: steps.at(-2)?.querySelector(".artifact-flow-step-when")?.dataset.tip ?? "",
        bundles: window.__TASKS__.bundles.filter((one) => one === "t-501").length,
      };
    }, SETTLED_MS);
    ok("the unfolded line says what, why and cost from its report's three lines",
      unfolded.expanded === "true" && unfolded.three.join(",") === "무엇,왜,대가", JSON.stringify(unfolded));
    ok("its steps stand in time: task, attempt 1 stopped, attempt 2, hand-in, review, landing, the release still to come",
      unfolded.steps.join(",") === "task,try,try,handin,review,land,ship"
        && unfolded.tones.join(",") === "done,stop,done,done,done,done,now"
        && unfolded.names[1] === "시도 1" && unfolded.names[2] === "시도 2" && unfolded.ship === "다음 발행을 기다립니다",
      JSON.stringify(unfolded));
    ok("the second attempt's step carries the rows its worker left, from the task's bundle asked once, and the landing says git told its time",
      unfolded.attemptRows.includes("r-final") && unfolded.attemptRows.includes("b-before") && !unfolded.attemptRows.includes("r-first")
        && unfolded.bundles === 1 && unfolded.landTip.includes("git"),
      JSON.stringify(unfolded));

    /* ---- the keyboard's place: one shape, the board's own (the review t-69070 P3 ⑧) ------------------ */
    const focused = await page.evaluate(() => {
      const view = artifactsView();
      const aim = {
        list: ".artifact-flow-list", item: ".artifact-flow-item", chip: ".artifact-flow-chip",
        more: ".artifact-flow-more", release: ".artifact-flow-release-head", fold: ".artifact-flow-pile-fold",
      };
      const seen = {};
      for (const [name, selector] of Object.entries(aim)) {
        const node = view?.querySelector(selector);
        if (!node) continue;
        node.focus({ focusVisible: true });
        const style = getComputedStyle(node);
        seen[name] = { outline: `${style.outlineStyle} ${style.outlineWidth}`, ring: style.boxShadow.replace(/inset /g, "") !== "none" && /rgb\(\s*\d+/.test(style.boxShadow) };
        node.blur();
      }
      return seen;
    });
    ok("every place the keyboard stands in the flow shows the board's focus outline — 2px solid, no ring shadow beside it",
      Object.keys(focused).length >= 4 && Object.values(focused).every((one) => one.outline === "solid 2px" && !one.ring),
      JSON.stringify(focused));

    /* ---- a chip stands its artifact in the reader, whose head says which step it is -------------- */
    const chip = (name) => `.artifact-flow-item[data-task="t-501"] > .artifact-flow-meta [data-flow-chip="${name}"]`;
    const reading = await page.evaluate(async ({ settled, report }) => {
      const view = artifactsView();
      view?.querySelector(report)?.click();
      await new Promise((done) => setTimeout(done, settled));
      const drawer = view?.querySelector(".artifacts-drawer") ?? null;
      const shown = (node) => Boolean(node) && !node.closest("[hidden]") && node.getClientRects().length > 0;
      return {
        reading: view?.classList.contains("is-flow-reading") ?? false,
        drawer: shown(drawer),
        id: drawer?.querySelector(".artifacts-detail")?.dataset.artifactId ?? null,
        where: drawer?.querySelector(".artifact-flow-where")?.textContent ?? "",
        badge: drawer?.querySelector(".artifact-flow-badge-word")?.textContent ?? "",
        context: drawer?.querySelector(".artifact-flow-context-said")?.textContent ?? "",
        landing: shown(drawer?.querySelector(".artifact-detail-landing")),
        // The bar's own task chip, time and attempt picker step back: the line and its steps say them.
        bar: [".artifact-detail-task", ".artifact-detail-when", ".artifact-detail-attempts"]
          .filter((part) => shown(drawer?.querySelector(`.artifact-detail-bar ${part}`))),
        pressed: view?.querySelector(report)?.getAttribute("aria-pressed") ?? null,
      };
    }, { settled: SETTLED_MS, report: chip("report") });
    ok("the report chip stands the final report in the reader beside the flow, its head 「보고서 · 시도 2의 최종 보고」 beside the state 「반영됨」",
      reading.reading && reading.drawer && reading.id === "r-final" && reading.where === "보고서 · 시도 2의 최종 보고"
        && reading.badge === "반영됨" && reading.context.includes("사라지던 것을 고쳤다") && !reading.landing && reading.bar.length === 0
        && reading.pressed === "true",
      JSON.stringify(reading));
    const marks = await page.evaluate(() => {
      const drawer = artifactsView()?.querySelector(".artifacts-drawer");
      const mark = (node) => ({ shape: node?._shape ?? null, signal: node?.dataset.signal ?? null, drawn: Boolean(node?.querySelector("svg")) });
      return { badge: mark(drawer?.querySelector(".artifact-flow-badge .artifact-flow-node")),
        context: mark(drawer?.querySelector(".artifact-flow-context .artifact-flow-node")) };
    });
    ok("the reader's context line leads with its task's own step shape and signal, the badge's — not the empty circle it is built with",
      marks.badge.drawn && marks.badge.shape !== null && marks.badge.shape !== "wait"
        && marks.context.shape === marks.badge.shape && marks.context.signal === marks.badge.signal && marks.context.drawn,
      JSON.stringify(marks));
    const compared = await page.evaluate(async ({ settled, pair }) => {
      const view = artifactsView();
      view?.querySelector(pair)?.click();
      await new Promise((done) => setTimeout(done, settled));
      const compare = view?.querySelector(".artifact-compare") ?? null;
      return { open: Boolean(compare) && !compare.hidden, pair: compare?.dataset.pair ?? "" };
    }, { settled: SETTLED_MS, pair: chip("pair") });
    ok("the before-and-after chip opens the comparison of the task's pair", compared.open && compared.pair === "b-before:b-after",
      JSON.stringify(compared));
    const closed = await page.evaluate(async ({ settled, pair }) => {
      const view = artifactsView();
      view?.querySelector(pair)?.click();
      await new Promise((done) => setTimeout(done, settled));
      const compare = view?.querySelector(".artifact-compare") ?? null;
      return { reading: view?.classList.contains("is-flow-reading") ?? true, selected: artifactSelectedId, compare: Boolean(compare) && !compare.hidden };
    }, { settled: SETTLED_MS, pair: chip("pair") });
    ok("pressing the pressed chip again closes the reader and the comparison with it",
      !closed.reading && closed.selected === null && !closed.compare, JSON.stringify(closed));

    /* ---- a release folds its tasks, and the records with no task stand in their time --------------- */
    const release = await page.evaluate(async (settled) => {
      const view = artifactsView();
      const box = view?.querySelector('.artifact-flow-release[data-release="v2.4.0"]') ?? null;
      const folded = {
        expanded: box?.getAttribute("aria-expanded") ?? null,
        rows: box?.querySelectorAll(".artifact-flow-item[data-task]").length ?? -1,
        digest: box?.querySelector(".artifact-flow-release-digest")?.textContent ?? "",
        count: box?.querySelector(".artifact-flow-release-count")?.textContent ?? "",
      };
      box?.querySelector("[data-flow-release]")?.click();
      await new Promise((done) => setTimeout(done, settled));
      const open = view?.querySelector('.artifact-flow-release[data-release="v2.4.0"]') ?? null;
      return {
        folded,
        expanded: open?.getAttribute("aria-expanded") ?? null,
        rows: [...(open?.querySelectorAll(".artifact-flow-item[data-task]") ?? [])].map((one) => one.dataset.task),
        state: open?.querySelector(".artifact-flow-state-word")?.textContent ?? "",
        older: view?.querySelector(".artifact-flow-older")?.textContent ?? "",
      };
    }, SETTLED_MS);
    ok("a release stands folded — its version, when, the names of its tasks and how many — and unfolds into their lines",
      release.folded.expanded === "false" && release.folded.rows === 0 && release.folded.digest.includes("빌드 줄 대기 시간 줄이기")
        && release.folded.count === "과업 4" && release.expanded === "true" && release.rows.join(",") === "t-702,t-703,t-704,t-705"
        && release.state === "발행됨 v2.4.0" && release.older.includes("v2.2.0 ~ v2.2.4") && release.older.includes("과업 44"),
      JSON.stringify(release));
    const records = await page.evaluate(async (settled) => {
      const view = artifactsView();
      const head = view?.querySelector('.artifact-flow-group[data-group="next"] .artifact-flow-item.is-records') ?? null;
      const folded = { said: head?.querySelector(".artifact-flow-said")?.textContent ?? "", chips: head?.querySelectorAll("[data-flow-record]").length ?? 0 };
      head?.click();
      await new Promise((done) => setTimeout(done, settled));
      const rows = [...(view?.querySelectorAll('.artifact-flow-group[data-group="next"] .artifact-flow-item.is-record') ?? [])];
      rows[0]?.querySelector("[data-flow-record]")?.click();
      await new Promise((done) => setTimeout(done, settled));
      return {
        folded, rows: rows.map((one) => one.dataset.record),
        where: view?.querySelector(".artifact-flow-where")?.textContent ?? "",
        id: view?.querySelector(".artifacts-detail")?.dataset.artifactId ?? null,
      };
    }, SETTLED_MS);
    ok("the records no task is linked to stand as one line before the next release, unfold one a record, and read as a 「기록」",
      records.folded.said.includes("누구의 작업도 아닌 페이지") && records.folded.chips === 4
        && records.rows.join(",") === "p-loose,d-loose,r-brief,e-steps" && records.id === "p-loose" && records.where.endsWith("기록"),
      JSON.stringify(records));

    /* ---- the screen rules and axe, with a line and a release unfolded and the reader open ----------- */
    await page.evaluate((report) => artifactsView()?.querySelector(report)?.click(), chip("report"));
    await settle();
    const rules = await page.evaluate(flowScreenRules);
    ok(`every line of text in the flow and its reader is ${TEXT_MIN_PX}px or more`, rules.small.length === 0, JSON.stringify(rules.small));
    ok(`everything a person presses there is at least ${PRESS_MIN_PX}px high`, rules.short.length === 0, JSON.stringify(rules.short));
    for (const theme of ["dark", "light"]) {
      await page.evaluate((next) => document.documentElement.setAttribute("data-theme", next), theme);
      await settle();
      const violations = await axeViolations(page, AxeBuilder, ".artifacts-view").catch((error) => [String(error)]);
      ok(`the flow and its reader pass axe in ${theme}`, violations.length === 0, JSON.stringify(violations));
      const rows = await contrastTable(page, AxeBuilder, ".artifacts-view .artifacts-flow").catch((error) => [{ error: String(error) }]);
      const low = rows.filter((row) => row.error || (row.verdict !== "hidden" && (row.ratio === null || row.ratio < CONTRAST_MIN)));
      ok(`every text of the flow keeps ${CONTRAST_MIN}:1 in ${theme}`, rows.length > 0 && low.length === 0, JSON.stringify(low.slice(0, 8)));
    }
    await page.evaluate(() => document.documentElement.setAttribute("data-theme", "dark"));
    await page.evaluate((report) => artifactsView()?.querySelector(report)?.click(), chip("report"));
    await settle();

    /* ---- the narrow window: the flow stays inside its column ------------------------------------- */
    await page.setViewportSize(NARROW_WINDOW);
    const narrow = await page.evaluate(async (settled) => {
      setPanelFolded("aside", true);
      await new Promise((done) => setTimeout(done, settled));
      const view = artifactsView();
      const list = view?.querySelector(".artifact-flow-list") ?? null;
      const edge = list?.getBoundingClientRect().right ?? 0;
      const out = [...(list?.querySelectorAll(".artifact-flow-item, .artifact-flow-release-head") ?? [])]
        .filter((one) => one.getClientRects().length > 0 && one.getBoundingClientRect().right > edge + 1)
        .map((one) => one.dataset.task ?? one.dataset.flowRelease ?? one.className);
      const said = list?.querySelector('.artifact-flow-item[data-task="t-501"] .artifact-flow-said') ?? null;
      const seen = { overflow: list ? list.scrollWidth - list.clientWidth : -1, out, saidWidth: said ? Math.round(said.getBoundingClientRect().width) : 0 };
      setPanelFolded("aside", false);
      return seen;
    }, SETTLED_MS);
    await page.setViewportSize(REAL_WINDOW);
    await settle();
    ok("at 720 wide the flow scrolls only down: no line and no release runs past its column",
      narrow.overflow <= 1 && narrow.out.length === 0 && narrow.saidWidth > 0, JSON.stringify(narrow));

    /* ---- a search narrows the lines; a repaint of the same answer writes nothing -------------------- */
    const searched = await page.evaluate(async () => {
      const view = artifactsView();
      const query = view?.querySelector(".artifacts-query") ?? null;
      const type = async (text) => {
        query.value = text;
        query.dispatchEvent(new Event("input", { bubbles: true }));
        await new Promise((done) => setTimeout(done, artifactTuning(view).searchDebounce + 120));
        return [...(view?.querySelectorAll(".artifacts-flow .artifact-flow-item[data-task]") ?? [])]
          .filter((one) => one.getClientRects().length > 0).map((one) => one.dataset.task);
      };
      const placeholder = query?.placeholder ?? "";
      const found = await type("검색 결과");
      const none = await type("없는 말 하나");
      const empty = view?.querySelector(".artifact-flow-empty-word")?.textContent ?? "";
      const steps = view?.querySelector(".artifact-flow-empty-steps")?.hidden ?? false;
      await type("");
      return { placeholder, found, none, empty, steps };
    });
    ok("the search asks the lines' words: one match, then nothing — said as nothing matching this filter",
      searched.placeholder === "한 일 · 제목 · 과업 번호 검색" && searched.found.join(",") === "t-700" && searched.none.length === 0
        && searched.empty === "이 거르개에 맞는 한 일이 없습니다" && searched.steps,
      JSON.stringify(searched));
    const quiet = await page.evaluate(async (settled) => {
      const view = artifactsView();
      const box = view?.querySelector(".artifacts-flow") ?? null;
      let writes = 0;
      const watch = new MutationObserver((found) => { writes += found.length; });
      watch.observe(box, { subtree: true, childList: true, attributes: true, characterData: true });
      const asks = window.__FLOW__.asks.length;
      const built = artifactFlowRowCreations;
      await refreshArtifactFlow(view);
      paintArtifactFlow(view);
      await new Promise((done) => setTimeout(done, settled));
      watch.disconnect();
      return { writes, asked: window.__FLOW__.asks.length - asks, built: artifactFlowRowCreations - built };
    }, SETTLED_MS);
    ok("the same answer asked again and drawn again writes nothing and builds no line",
      quiet.asked === 1 && quiet.writes === 0 && quiet.built === 0, JSON.stringify(quiet));

    /* ---- the four catalogs ------------------------------------------------------------------------------ */
    const words = await page.evaluate(async (settled) => {
      const view = artifactsView();
      const read = () => [...(view?.querySelectorAll('.artifact-flow-group-name, .artifact-flow-state-word, .artifact-flow-step-name, .artifact-flow-release-count, [data-artifact-tab="flow"] span') ?? [])]
        .map((one) => one.textContent).join(" | ");
      const out = {};
      for (const code of ["en", "ja", "zh", "es"]) {
        locale = code;
        applyLocale();
        paintArtifactsView();
        await new Promise((done) => setTimeout(done, settled));
        out[code] = read();
      }
      locale = "ko";
      applyLocale();
      paintArtifactsView();
      await new Promise((done) => setTimeout(done, settled));
      return out;
    }, SETTLED_MS);
    ok("the flow speaks each of the four catalogs — no Korean left in its heads, states, steps and counts",
      Object.values(words).every((said) => said.length > 0 && !/[가-힣]/.test(said)), JSON.stringify(words));

    /* ---- an empty project says what is missing and what comes next, and holds the tab --------------- */
    await standFlow(page, { empty: true });
    const empty = await page.evaluate(async (settled) => {
      const view = artifactsView();
      await refreshArtifacts();
      await new Promise((done) => setTimeout(done, settled));
      const shown = (node) => Boolean(node) && !node.closest("[hidden]") && node.getClientRects().length > 0;
      return {
        tab: artifactFilter.tab,
        empty: shown(view?.querySelector(".artifact-flow-empty")),
        list: shown(view?.querySelector(".artifact-flow-list")),
        word: view?.querySelector(".artifact-flow-empty-word")?.textContent ?? "",
        steps: view?.querySelectorAll(".artifact-flow-empty-steps li").length ?? 0,
        all: shown(view?.querySelector(".artifact-flow-empty-all")),
      };
    }, SETTLED_MS);
    ok("a project with no work keeps the flow and says so in its own words, with the three steps that fill it and the way to the full list",
      empty.tab === "flow" && empty.empty && !empty.list && empty.word.endsWith("아직 한 일이 없습니다") && empty.steps === 3 && empty.all,
      JSON.stringify(empty));
    const all = await page.evaluate(async (settled) => {
      const view = artifactsView();
      view?.querySelector(".artifact-flow-empty-all")?.click();
      await new Promise((done) => setTimeout(done, settled));
      const shown = (node) => Boolean(node) && !node.closest("[hidden]") && node.getClientRects().length > 0;
      const seen = { tab: artifactFilter.tab, grid: shown(view?.querySelector(".artifacts-grid")), flow: shown(view?.querySelector(".artifacts-flow")) };
      // Back to the flow for the weight checks, as a person picking its tab.
      view?.querySelector('[data-artifact-tab="flow"]')?.click();
      await new Promise((done) => setTimeout(done, settled));
      return seen;
    }, SETTLED_MS);
    ok("「전체 목록 보기」 opens the full list on a tab that has rows", all.tab === "pages" && all.grid && !all.flow, JSON.stringify(all));

    /* ---- a folder in no listed repository says git was not asked there (t-69070 P2) ------------------ */
    await standFlow(page, { empty: true, outside: true });
    const outside = await page.evaluate(async (settled) => {
      const view = artifactsView();
      await refreshArtifactFlow(view);
      await new Promise((done) => setTimeout(done, settled));
      const shown = (node) => Boolean(node) && !node.closest("[hidden]") && node.getClientRects().length > 0;
      const line = view?.querySelector(".artifact-flow-outside");
      return {
        tab: artifactFilter.tab,
        shown: shown(line),
        word: line?.textContent ?? "",
        size: line ? parseFloat(getComputedStyle(line).fontSize) : 0,
        empty: shown(view?.querySelector(".artifact-flow-empty")),
      };
    }, SETTLED_MS);
    ok("a folder in no repository the window lists says so in one line — git was not asked there — above what the flow holds",
      outside.tab === "flow" && outside.shown && outside.word === "등록된 저장소 밖 폴더 — git 기록 없음" && outside.size >= TEXT_MIN_PX
        && outside.empty,
      JSON.stringify(outside));
    for (const theme of ["dark", "light"]) {
      await page.evaluate((next) => document.documentElement.setAttribute("data-theme", next), theme);
      await settle();
      const rows = await contrastTable(page, AxeBuilder, ".artifacts-view .artifact-flow-outside").catch((error) => [{ error: String(error) }]);
      const low = rows.filter((row) => row.error || (row.verdict !== "hidden" && (row.ratio === null || row.ratio < CONTRAST_MIN)));
      ok(`that line keeps ${CONTRAST_MIN}:1 in ${theme}`, rows.length > 0 && low.length === 0, JSON.stringify(rows.slice(0, 4)));
    }
    await page.evaluate(() => document.documentElement.setAttribute("data-theme", "dark"));

    /* ---- git did not answer: the flow stays, and says it is being read again (t-69107 ③) ------------- */
    await standFlow(page, { stale: true });
    const stale = await page.evaluate(async () => {
      const view = artifactsView();
      await refreshArtifactFlow(view);
      const shown = (node) => Boolean(node) && !node.closest("[hidden]") && node.getClientRects().length > 0;
      await window.__UNTIL__(() => shown(view?.querySelector(".artifact-flow-stale")), "the stale git notice");
      const line = view?.querySelector(".artifact-flow-stale");
      return {
        shown: shown(line), word: line?.textContent ?? "",
        size: line ? parseFloat(getComputedStyle(line).fontSize) : 0,
        lines: view?.querySelectorAll(".artifact-flow-item").length ?? 0,
        quietOutside: !shown(view?.querySelector(".artifact-flow-outside")),
      };
    });
    ok("a flow read over git's last answer keeps its lines and says 「다시 읽는 중」 in one line",
      stale.shown && stale.word === "다시 읽는 중" && stale.size >= TEXT_MIN_PX && stale.lines > 0 && stale.quietOutside,
      JSON.stringify(stale));
    await standFlow(page);
    const staleGone = await page.evaluate(async () => {
      const view = artifactsView();
      await refreshArtifactFlow(view);
      const hidden = () => Boolean(view?.querySelector(".artifact-flow-stale")?.closest("[hidden]") ?? view?.querySelector(".artifact-flow-stale")?.hidden);
      await window.__UNTIL__(hidden, "the stale git notice to clear");
      return hidden();
    });
    ok("and the line is gone once git answers", staleGone, "the line still stands");

    /* ---- thirty tasks over twelve releases: 400 elements, 3 ms a paint with the CPU slowed ----------- */
    const weight = await standFlow(page, { heavy: HEAVY });
    await page.evaluate(async (settled) => {
      await refreshArtifactFlow(artifactsView());
      await new Promise((done) => setTimeout(done, settled));
    }, SETTLED_MS);
    const cdp = await page.context().newCDPSession(page);
    await cdp.send("Emulation.setCPUThrottlingRate", { rate: CPU_THROTTLE });
    const fresh = [];
    const again = [];
    for (let round = 0; round < PAINT_ROUNDS; round += 1) {
      const [scratch, repaint] = await page.evaluate(() => {
        const view = artifactsView();
        // From scratch, as the list is first drawn: no kept node, no section, no head.
        view._artifactFlowRows = null;
        view._artifactFlowKept = null;
        const list = view.querySelector(".artifact-flow-list");
        list._sections = null;
        list._heads = null;
        list.replaceChildren();
        const from = performance.now();
        paintArtifactFlow(view);
        const built = performance.now() - from;
        const twice = performance.now();
        paintArtifactFlow(view);
        return [built, performance.now() - twice];
      });
      fresh.push(scratch);
      again.push(repaint);
    }
    await cdp.send("Emulation.setCPUThrottlingRate", { rate: 1 });
    const heavy = await page.evaluate(() => ({
      tab: artifactFilter.tab,
      elements: artifactsView()?.querySelector(".artifacts-flow")?.querySelectorAll("*").length ?? -1,
      lines: artifactFlowLines().length,
      releases: artifactFlowAnswer?.releases?.length ?? 0,
    }));
    ok(`${HEAVY.tasks} tasks and ${HEAVY.releases} releases stand within ${ELEMENTS_MAX} elements (${heavy.elements})`,
      heavy.tab === "flow" && heavy.lines === weight.lines && heavy.lines === HEAVY.tasks && heavy.releases === HEAVY.releases
        && heavy.elements > 0 && heavy.elements <= ELEMENTS_MAX,
      JSON.stringify(heavy));
    ok(`the flow is drawn from scratch in at most ${PAINT_MS_MAX} ms with the CPU slowed ${CPU_THROTTLE}x (p50 ${median(fresh).toFixed(2)} ms)`,
      median(fresh) <= PAINT_MS_MAX, JSON.stringify(fresh.map((one) => one.toFixed(2))));
    ok(`and drawn again over itself in at most ${PAINT_MS_MAX} ms (p50 ${median(again).toFixed(2)} ms)`,
      median(again) <= PAINT_MS_MAX, JSON.stringify(again.map((one) => one.toFixed(2))));

    ok("the flow raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await launchWindowBrowser();
  let failures = 0;
  try {
    await testArtifactFlow(browser, origin, (name, pass, detail = "") => {
      console.log(`${pass ? "PASS" : "FAIL"}  ${name}${!pass && detail ? `  — ${detail}` : ""}`);
      if (!pass) failures += 1;
    });
  } finally {
    await browser.close();
    files.close();
  }
  if (failures > 0) process.exitCode = 1;
}
