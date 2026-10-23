/* t-69978 · 관계 탭 카드 보기 — 왼쪽 목록을 편 1440 창에서도 코디 두 열 (E2 t-68475 후속, 사람 10-22).
 *
 * E2의 카드 보기는 그림 안쪽이 760px를 넘어야 코디의 열이 나란히 선다. 1440 창에서 왼쪽 목록을 펴면
 * 작업 레일(172px)과 곁의 인스펙터(300px)를 뺀 그림 안쪽이 655px라 열이 하나로 쌓였다. 여기서 재는
 * 약속은 넷이다:
 *
 *   - 그 폭에서는 작업 레일이 저절로 접히고, 두 코디의 열이 윗변을 맞춰 나란히 선다.
 *   - 툴바 맨 앞의 단추 하나(「범위 목록」)가 레일을 다시 펴고 접는다 — Tab으로 닿고, 초점이 보인다.
 *   - 사람이 펴거나 접은 것은 저절로 접는 것보다 이기고, 다시 그려도 · 창을 다시 열어도 남는다.
 *   - 이미 두 열이 서는 넉넉한 창, 720 창, 입체 보기는 지금 그대로다 — 레일도 열도 단추도.
 *   - 열이 좁아도(두 열의 문턱 바로 위, 그리고 레일을 접은 1440) 워크스페이스 칩은 잘리지 않는다.
 *
 * 장면은 E2 시안의 예시(`teamsFixture`)이고, 보드는 창의 제 자리에 선다: 왼쪽 목록은 펴고 파일 패널은
 * 접는다. 사람이 1440 창에서 보는 그 배치이고, 보드를 화면 전체로 세우는 E2 묶음과 다른 점이 그것이다.
 *
 * 넉넉한 창과 좁은 두 열의 창은 수로 적지 않고 잰 폭에서 구한다 — 레일을 편 채의 그림 안쪽이 문턱을
 * 넉넉히 넘는 창, 그리고 겨우 넘는 창. 하네스의 왼쪽 목록(280px)에서는 1560 창도 레일을 편 채 759px라
 * 이미 한 열이었다(j1, 코디 m-70325).
 *
 *   WINDOW_SUITES=agent-graph-rail node ui/tests/window.mjs
 */
import { openWindowTestPage } from "./window-boot.mjs";
import { installBoardWaits } from "./board-waits.mjs";
import { teamsFixture } from "./agent-graph-teams.mjs";

/* 사람의 선택이 남는 로컬 설정 한 키 — 입체/카드 선택(`zerocode.board-relations-view.v1`)과 같은 방식. */
const RAIL_STORE = "zerocode.board-relations-rail.v1";
/* 열이 나란히 서는 그림 안쪽의 문턱 — E2의 `@container graph (max-width: 759px)`가 말하는 수이고, E2 묶음도
 * 같은 수로 잰다(「below a 760px picture the columns stack into one」). 넉넉한 창은 레일을 편 채 이 문턱을
 * 120px 넘고, 좁은 두 열의 창은 4px 넘는다. */
const STACK = 760;
const ROOMY = STACK + 120;
const NARROW = STACK + 4;

/* 창의 배치: 왼쪽 목록은 펴고 파일 패널은 접는다 (⌘B · ⌘L의 그 손). A page function. */
function standPanels() {
  setPanelFolded("sidebar", false);
  setPanelFolded("aside", true);
}

/* 판이 다 선 뒤까지 — 보드가 그려지고, 크기 관찰자가 판의 폭을 재 그 답을 다음 프레임에 입히고,
 * 오버레이 선을 다시 잰 뒤. 보드의 기다림은 크기 관찰자보다 한 프레임 먼저 풀릴 수 있다(관찰자는
 * 그 프레임의 레이아웃 뒤에 온다) — 그래서 보드를 기다린 다음, 접기의 프레임이 비고 그림 판의 폭이 세
 * 프레임 내리 같을 때까지 보고, 보드를 한 번 더 기다린다. A page function. */
async function steady() {
  const view = document.querySelector("#board-view");
  const scroll = view.querySelector(".agent-graph-scroll");
  await window.__BOARD_SETTLED__();
  for (let last = -1, still = 0, frames = 0; frames < 120 && still < 3; frames += 1) {
    await new Promise(requestAnimationFrame);
    const folding = typeof agentGraphRailFrames !== "undefined" && agentGraphRailFrames.has(view);
    still = scroll.clientWidth === last && !folding ? still + 1 : 0;
    last = scroll.clientWidth;
  }
  await window.__BOARD_SETTLED__();
}

/* 판이 선 그대로 — 창 · 왼쪽 목록 · 보드의 폭, 그림 안쪽, 레일, 단추, 코디 열의 자리. A page function. */
function readRail(store) {
  const view = document.querySelector("#board-view");
  const scroll = view.querySelector(".agent-graph-scroll");
  const dress = getComputedStyle(scroll);
  const rail = view.querySelector(".agent-relations-rail");
  const toggle = view.querySelector(".agent-relations-rail-toggle");
  const box = (node) => node.getBoundingClientRect();
  return {
    window: window.innerWidth,
    list: Math.round(box(document.querySelector("aside.threads")).width),
    board: view.clientWidth,
    listTier: Number.parseFloat(getComputedStyle(view).getPropertyValue("--agent-graph-tier-list")),
    railToken: Number.parseFloat(getComputedStyle(view).getPropertyValue("--agent-relations-rail-width")),
    inspector: getComputedStyle(view.querySelector(".agent-inspector")).position === "static" ? "beside" : "floating",
    inner: Math.round(scroll.clientWidth - Number.parseFloat(dress.paddingLeft) - Number.parseFloat(dress.paddingRight)),
    rail: rail.checkVisibility(),
    /* 카드의 워크스페이스 칩 — 보이는 것마다 이름이 제 상자 안에 다 드는지. */
    chips: (() => {
      const names = [...view.querySelectorAll(".agent-graph-nodes .agent-graph-node-branch:not([hidden]) .agent-graph-node-branch-name")]
        .filter((one) => one.checkVisibility());
      return { shown: names.length,
        cut: names.filter((one) => one.scrollWidth > one.clientWidth + 1).map((one) => one.textContent) };
    })(),
    leads: [...view.querySelectorAll(".agent-graph-team.is-lead")]
      .map((team) => ({ top: Math.round(box(team).top), left: Math.round(box(team).left) })),
    columns: new Set([...view.querySelectorAll(".agent-graph-team")].map((team) => Math.round(box(team).left))).size,
    sideways: scroll.scrollWidth > scroll.clientWidth + 1 || view.scrollWidth > view.clientWidth + 1,
    toggle: toggle === null ? null : {
      shown: toggle.checkVisibility(),
      first: toggle.parentElement.classList.contains("agent-relations-toolbar")
        && toggle.parentElement.firstElementChild === toggle,
      expanded: toggle.getAttribute("aria-expanded"),
      controls: rail.id !== "" && toggle.getAttribute("aria-controls") === rail.id,
      name: toggle.getAttribute("aria-label") ?? "",
      tip: toggle.dataset.tip ?? "",
      focused: document.activeElement === toggle,
      ring: document.activeElement === toggle && toggle.matches(":focus-visible")
        && getComputedStyle(toggle).boxShadow !== "none",
    },
    scopeFocused: document.activeElement === view.querySelector(".agent-graph-scope"),
    /* 초점이 어디 있는가 — 태그와 클래스, 그리고 범위 선택이 보이는가(초점을 받을 수 있는가). */
    active: (() => {
      const node = document.activeElement;
      if (node === null) return null;
      const names = typeof node.className === "string" ? node.className.trim().split(/\s+/).filter(Boolean) : [];
      return [node.tagName.toLowerCase(), ...names].join(".");
    })(),
    scopeShown: view.querySelector(".agent-graph-scope")?.checkVisibility() ?? null,
    stored: localStorage.getItem(store),
  };
}

/* 단추를 누른다 — 없는 창(고치기 전)에서는 아무 일도 하지 않는다. A page function. */
function pressToggle() {
  document.querySelector("#board-view .agent-relations-rail-toggle")?.click();
}

export async function testAgentGraphRail(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  /* 장면을 처음부터 세운다 — 창을 다시 연 뒤에도 이 길로 다시 선다. */
  const stand = async (width) => {
    await installBoardWaits(page);
    await page.setViewportSize({ width, height: 900 });
    await page.evaluate(standPanels);
    await page.evaluate(teamsFixture);
    await page.evaluate(steady);
    return page.evaluate(readRail, RAIL_STORE);
  };
  const settle = async () => {
    await page.evaluate(steady);
    return page.evaluate(readRail, RAIL_STORE);
  };
  const resize = async (width) => {
    await page.setViewportSize({ width, height: 900 });
    return settle();
  };
  /* 레일을 편 채의 그림 안쪽이 `want`px가 되는 창 — 접혀 있으면 레일의 몫을 빼고 센다. 왼쪽 목록이 창의
   * 폭을 따라 바뀌어도 다시 재서 맞춘다(세 번까지). */
  const windowFor = async (want) => {
    let width = 1440;
    let seen = await resize(width);
    for (let round = 0; round < 3; round += 1) {
      const standing = seen.rail ? seen.inner : seen.inner - seen.railToken;
      if (Math.abs(standing - want) <= 2) break;
      width += Math.round(want - standing);
      seen = await resize(width);
    }
    return { width, seen };
  };
  try {
    const wide = await stand(1440);
    const [first, second] = wide.leads;
    ok("in a 1440 window with the left list open the rail folds, and the two coordinators' columns stand side by side, their tops level",
      wide.window === 1440 && wide.list > 0 && wide.inspector === "beside" && !wide.rail
        && wide.leads.length === 2 && Math.abs(first.top - second.top) <= 1 && first.left !== second.left
        && wide.columns === 2 && wide.inner >= STACK && !wide.sideways && wide.stored === null,
      JSON.stringify(wide));
    ok("the toolbar's first control is the 「범위 목록」 button: it stands while the rail is folded, says so, and names the rail it opens",
      wide.toggle?.shown === true && wide.toggle.first && wide.toggle.expanded === "false" && wide.toggle.controls
        && wide.toggle.name === "범위 목록" && wide.toggle.tip === "범위 목록",
      JSON.stringify(wide.toggle));

    /* 지금 그대로인 폭 — 사람이 아무것도 고르지 않은 판이다. */
    const { width: roomyWidth, seen: roomy } = await windowFor(ROOMY);
    ok("a roomy window, the rail open and the picture well over the two-column threshold, stands as it did: the rail beside the picture, two columns, and no new button",
      roomy.rail && roomy.inner >= ROOMY - 2 && roomy.columns >= 2 && roomy.inspector === "beside" && !roomy.sideways
        && (roomy.toggle === null || !roomy.toggle.shown),
      JSON.stringify({ roomyWidth, roomy }));

    /* 칩 — 두 열이 겨우 서는 창(레일을 편 채 문턱을 4px 넘는다)과 레일을 접은 1440. 열이 약 370px와 390px다. */
    const { width: tightWidth, seen: tight } = await windowFor(NARROW);
    ok("every workspace chip stands whole in two narrow columns: at the narrowest two-column window with the rail open, and at 1440 with it folded",
      tight.rail && tight.columns === 2 && tight.chips.shown > 0 && tight.chips.cut.length === 0
        && wide.chips.shown > 0 && wide.chips.cut.length === 0,
      JSON.stringify({ tightWidth, tight: { inner: tight.inner, columns: tight.columns, chips: tight.chips }, wide: wide.chips }));

    const narrow = await resize(720);
    ok("a 720 window stands as it did: one column, the rail as its tier keeps it, and no new button",
      narrow.columns === 1 && narrow.rail === narrow.board >= narrow.listTier && !narrow.sideways
        && (narrow.toggle === null || !narrow.toggle.shown),
      JSON.stringify(narrow));

    /* 입체는 그대로다. 이 브라우저는 WebGL2를 주지 않아 입체가 서지 않으므로(「the window harness draws the
     * 2D painter」), 입체가 판에 입히는 표지(`is-orbit`)를 손으로 입히고 레일의 규칙을 읽는다. */
    await resize(1440);
    await page.evaluate(() => {
      const view = document.querySelector("#board-view");
      view.classList.add("is-orbit");
      if (typeof syncAgentGraphRail === "function") syncAgentGraphRail(view);
    });
    const orbit = await page.evaluate(readRail, RAIL_STORE);
    await page.evaluate(() => {
      const view = document.querySelector("#board-view");
      view.classList.remove("is-orbit");
      if (typeof syncAgentGraphRail === "function") syncAgentGraphRail(view);
    });
    await settle();
    ok("the orbit view keeps the rail beside it and stands no button",
      orbit.rail && (orbit.toggle === null || !orbit.toggle.shown), JSON.stringify(orbit));

    /* 키보드. 단추는 범위 선택 바로 앞이다 — 범위 선택에서 Shift+Tab 한 번이 단추에 닿는다. 닿지 않은 판
     * (고치기 전)에서는 Enter를 누르지 않는다: 그 자리의 다른 단추가 판을 옮기면 뒤의 단언이 엉뚱한 판을
     * 읽는다. */
    await page.focus("#board-view .agent-graph-scope");
    await page.keyboard.press("Shift+Tab");
    const reached = await page.evaluate(readRail, RAIL_STORE);
    const onToggle = reached.toggle?.focused === true;
    if (onToggle) await page.keyboard.press("Enter");
    const opened = await settle();
    if (onToggle) await page.keyboard.press("Space");
    const refolded = await settle();
    ok("Tab reaches the button, Enter opens the rail with the focus ring still on the button, and Space folds it again",
      onToggle && reached.toggle.ring
        && opened.rail && opened.toggle?.expanded === "true" && opened.toggle.focused && opened.toggle.ring
        && !refolded.rail && refolded.toggle?.expanded === "false" && refolded.toggle.focused && refolded.columns === 2,
      JSON.stringify({ reached: reached.toggle, opened, refolded }));

    /* 사람의 선택은 저절로 접는 것보다 이긴다 — 편 레일은 접는 폭(1440)에서도 서고, 다시 그려도, 창을
     * 다시 열어도 남는다. */
    await page.evaluate(pressToggle);
    const kept = await settle();
    await page.evaluate(() => paintBoardView(undefined, { force: true }));
    const repainted = await settle();
    await page.reload();
    await page.waitForFunction(() => typeof BOUND !== "undefined" && BOUND.size > 0);
    const reopened = await stand(1440);
    ok("the person's choice beats the fold: opened at 1440 the rail stands, through a repaint and a reopened window",
      kept.rail && kept.stored === "open" && kept.columns === 1 && kept.toggle?.expanded === "true"
        && repainted.rail && repainted.toggle?.expanded === "true"
        && reopened.rail && reopened.stored === "open" && reopened.toggle?.shown === true && reopened.toggle.expanded === "true",
      JSON.stringify({ kept, repainted, reopened }));

    /* 접고 펴는 일은 카드를 다시 짓지 않고 모델을 다시 배치하지 않는다 — 바뀌는 것은 판의 클래스 하나다. */
    const before = await page.evaluate(() => [agentGraphNodeCreations, agentGraphLayoutRuns]);
    await page.evaluate(pressToggle);
    const folded = await settle();
    await page.evaluate(pressToggle);
    const reopenedRail = await settle();
    const after = await page.evaluate(() => [agentGraphNodeCreations, agentGraphLayoutRuns]);
    ok("folding and opening the rail rebuilds no card and runs no new layout of the model",
      !folded.rail && folded.stored === "folded" && reopenedRail.rail && reopenedRail.stored === "open"
        && before.join() === after.join(),
      JSON.stringify({ before, after, folded: folded.rail, opened: reopenedRail.rail }));

    /* 접어 둔 것도 이긴다 — 넉넉한 창에서도 접힌 채 서고, 거기서 펴면 화면이 지금 그대로로 돌아가며 단추가
     * 물러난다. 초점은 몸으로 떨어지지 않고 곁의 범위 선택으로 간다. */
    await page.evaluate(pressToggle);
    const roomyFolded = await resize(roomyWidth);
    await page.evaluate(() => document.querySelector("#board-view .agent-relations-rail-toggle")?.focus());
    const focusedThere = (await page.evaluate(readRail, RAIL_STORE)).toggle?.focused === true;
    if (focusedThere) await page.keyboard.press("Enter");
    const pressedFocus = (await page.evaluate(readRail, RAIL_STORE)).active;
    const handed = await settle();
    ok("a folded choice holds in the roomy window too, and opening it there gives the screen back with the focus on the scope select",
      roomyFolded.stored === "folded" && !roomyFolded.rail && roomyFolded.toggle?.shown === true
        && roomyFolded.toggle.expanded === "false" && roomyFolded.columns >= 2
        && focusedThere && handed.rail && handed.stored === "open" && handed.toggle?.shown === false
        && handed.scopeFocused && handed.columns >= 2,
      JSON.stringify({ roomyFolded, handed, pressedFocus }));

    ok("the folding rail raises no browser errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}
