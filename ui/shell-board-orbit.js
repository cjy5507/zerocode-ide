/* ---- 관계의 행성계 (t-9444) ------------------------------------------------
 *
 * 보드의 `관계` 그림을 한 번 더 그리는 방법. 카드와 연결선 대신 워크스페이스가
 * 항성이고, 에이전트가 그 둘레를 도는 행성이며, 하위 에이전트는 제 부모 행성을
 * 도는 위성이다. 우편은 두 행성 사이의 링크를 따라 흐르는 입자다. 카드 그림은
 * 그대로 남고, 툴바의 토글 하나가 둘 사이를 오간다(로컬 설정 한 키).
 *
 * **읽는 것은 관계 그림의 모델 하나다.** `paintAgentGraph`가 그린 그 모델(범위와
 * 검색이 이미 먹은 것)을 받아 항성·행성·위성·링크로 다시 읽을 뿐, 새 백엔드
 * 호출도 원장 읽기도 없다. 상태의 낱말(`agentGraphStateWord`)·이름
 * (`agentGraphIdentity`)·선택(`selectAgentGraphEntity`)·배율(`agentGraphZoom`)은
 * 관계 그림의 것을 그대로 쓴다 — 한 화면이 같은 에이전트를 두 가지로 말하지 않게.
 *
 * **상태는 모델이 움직일 때만 적용한다.** 행성계가 쓰는 사실(상태·부모·검색·
 * 활동량의 단·우편의 신선함)의 서명이 바뀐 판에서만 몸들의 목표가 새로 서고
 * (`agentOrbitApplied`), 프레임은 그 목표로 **보간만** 한다 — 같은 판을 다시
 * 그리는 훅의 박자와 60 fps의 프레임은 적용을 한 번도 부르지 않는다.
 *
 * **보이지 않으면 박자가 없다.** 그림이 서는지는 실시간 지도와 같은 한 손으로
 * 묻는다(`agentGraphPictureOn`·`agentGraphViewStands`, shell-board-live.js) —
 * 숨은 문서·작업 목록·숨은 판·카드 보기에서 행성계의 rAF는 0이다. 움직임을
 * 줄이라는 판은 정지 화면이고, 상태가 바뀔 때만 한 장을 다시 그린다. 초점이 없는
 * 창은 초당 30장까지만 그린다.
 *
 * 수는 아래 표 `ORBIT` 하나에 있고, 색은 CSS 토큰(`--agent-orbit-*`, shell.css
 * 「행성계」 절)이 든다 — 역할 토큰에서 섞으므로 라이트·다크가 함께 뒤집힌다. */

/* 행성계의 수, 한 표. 시각 수치와 데이터 양이 여기 함께 서는 것은 이 그림이
 * 캔버스라 CSS가 셈을 대신할 수 없기 때문이다 — 캔버스가 읽는 수가 두 곳에
 * 적히면 한 곳만 움직이는 날이 온다. 길이는 배율 1의 CSS 픽셀이다. */
const ORBIT = Object.freeze({
  /* 고른 보기가 남는 로컬 설정 한 키와 그 값 둘. */
  store: "zerocode.board-relations-view.v1",
  orbit: "orbit",
  cards: "cards",
  /* 움직임을 줄이라는 판을 묻는 미디어 질의 — CSS의 같은 질의가 전이를 끈다. */
  reduced: "(prefers-reduced-motion: reduce)",
  second: 1000,
  /* 그림의 박자. 초점이 있는 창 60, 없는 창 30 — rAF가 이보다 잦게 와도(120 Hz
   * 화면) 그림은 이 수를 넘지 않는다. `slackMs`는 반 박자 일찍 온 rAF를
   * 건너뛰지 않게 하는 여유다. */
  fps: Object.freeze({ focused: 60, blurred: 30, slackMs: 2 }),
  /* 멈췄다 온 박자 하나가 흘리는 시간의 상한 — 돌아온 판이 궤도를 한 번에 몇
   * 바퀴 돌리지 않게. */
  stepCapMs: 100,
  /* 목표로 다가가는 시간 상수(지수 접근). 궤도가 바뀐 행성은 이만큼에 걸쳐
   * 미끄러진다. 손이 라벨 위에 오르거나 초점이 들면 공전은 `holdMs`에 걸쳐 곧게
   * 줄어 멈추고, 떠나면 같은 시간에 걸쳐 다시 돈다. */
  settleMs: 700,
  holdMs: 260,
  /* 상태 → 궤도의 단(안쪽 0부터)과 잉크. 확인 필요와 실패는 제 색의 고리를 한 겹
   * 더 두른다(실패는 먼 궤도의 붉은 고리). 낱말은 관계 그림의 한 손
   * (`agentGraphStateWord`)이 짓는다. */
  states: Object.freeze({
    working: Object.freeze({ ring: 0, ink: "working", ringed: false }),
    "needs-attention": Object.freeze({ ring: 1, ink: "attention", ringed: true }),
    idle: Object.freeze({ ring: 2, ink: "idle", ringed: false }),
    paused: Object.freeze({ ring: 2, ink: "idle", ringed: false }),
    done: Object.freeze({ ring: 3, ink: "done", ringed: false }),
    failed: Object.freeze({ ring: 3, ink: "failed", ringed: true }),
  }),
  rings: 4,
  /* 궤도의 반지름: 첫 단 `first`, 단마다 `gap`. 안쪽 궤도(작업 중)는 제 계의
   * 빛깔로 조금 더 밝다 — 일하는 자리가 판의 중심이다. 먼 궤도(완료)는 점선이고
   * 그 점선이 `drift`(px/ms)로 천천히 돈다 — 조용한 판도 살아 있는 판으로 읽힌다. */
  ring: Object.freeze({ first: 46, gap: 30, width: 1, alpha: 0.2, inner: 0.42,
    dash: Object.freeze([3, 5]), drift: 0.004 }),
  /* 항성: 반지름은 에이전트 수를 따라 자라고(상한), 빛무리와 제 빛깔의 테를 두른다. */
  star: Object.freeze({ radius: 8, perAgent: 1.3, agentCap: 10, glow: 4.4, rim: 3, rimWidth: 1.2 }),
  /* 행성: 반지름은 하위 수를 따라 자란다(상한). `hit`은 라벨의 누를 자리가 몸을
   * 넘어 더 덮는 폭, `ringGap`은 확인 필요·실패의 고리가 몸에서 떨어진 거리. */
  planet: Object.freeze({ radius: 4.2, perChild: 1.4, childCap: 4, glow: 4.2, hit: 5,
    ringGap: 3.5, ringWidth: 1.4 }),
  /* 위성: 부모를 도는 반지름은 `orbit`에서 하나마다 `step`씩 벌어진다. */
  moon: Object.freeze({ radius: 2.8, orbit: 13, step: 6, glow: 3.6, alpha: 0.34 }),
  /* 공전: 최근 창(`windowMs`) 안의 도구 호출이 `full`개면 가장 빠르고(`busyMs`에 한
   * 바퀴), 없으면 가장 느리다(`restMs`에 한 바퀴). 활동량은 `steps`단으로 끊어
   * 서명에 든다 — 도구 호출 하나하나가 적용을 부르지 않게. */
  period: Object.freeze({ restMs: 240_000, busyMs: 18_000 }),
  activity: Object.freeze({ windowMs: 120_000, full: 12, steps: 6 }),
  /* 꼬리: 최근 `spanMs`의 궤적, `segments`조각으로 옅어진다. 한 바퀴의 `maxTurn`을
   * 넘지 않는다. */
  trail: Object.freeze({ spanMs: 10_000, segments: 6, alpha: 0.5, maxTurn: 0.85, width: 1.5 }),
  /* 우편. 방금 오간(`freshMs` 안) 링크나 미확인이 있는 링크에 입자가 흐르고,
   * 입자 하나가 링크를 건너는 데 `travelMs`가 든다. 미확인이 있으면 밝다.
   * `cap`은 한 판에 흐르는 입자의 상한 — 넘치면 오래된 링크의 입자부터 접는다. */
  /* 링크는 셋으로 옅어진다: 고른 몸에 닿은 것(`litAlpha`), 방금 오간 것(`linkAlpha`),
   * 오래된 것과 검색에 흐려진 것(`quietAlpha`). 입자의 꼬리는 링크 길이와 무관하게
   * `tailPx` 픽셀이다 — 긴 링크의 입자가 선이 되지 않게. */
  mail: Object.freeze({ freshMs: 600_000, travelMs: 2_600, perLink: 1, cap: 32, bend: 0.14,
    width: 1, linkAlpha: 0.16, litAlpha: 0.42, quietAlpha: 0.05, readAlpha: 0.7, unreadAlpha: 1,
    radius: 1.7, unreadRadius: 2.4, glow: 4.6, tailPx: 9, tailAlpha: 0.45, tailWidth: 1.2,
    stillAt: 0.6 }),
  /* 턴 끝의 맥동 한 번: `ms` 동안 `reach`만큼 번지며 옅어진다. */
  pulse: Object.freeze({ ms: 1_400, reach: 24, width: 1.6 }),
  /* 고른 몸의 고리와 손이 올라간 몸의 고리. */
  select: Object.freeze({ gap: 4, width: 1.6, hoverAlpha: 0.55 }),
  /* 검색에 맞지 않는 몸의 불투명도, 그리고 새로 선 몸이 나타나는 시작 불투명도. */
  dim: 0.22,
  /* 빛무리 한 장의 크기(CSS px)와 속(core)의 몫, 바깥 빛의 세기. 빛의 세기는 판의
   * 색 구성표(`color-scheme`)를 따른다 — 흰 바탕의 빛무리는 얼룩이 된다. */
  glow: Object.freeze({ sprite: 64, core: 0.34, outer: 0.5, dark: 0.85, light: 0.28 }),
  /* 계(系) 하나의 칸: 궤도 바깥의 여백, 라벨이 오른쪽으로 쓰는 폭, 맞춤 배율의
   * 위·아래. 아래보다 작은 판은 끌어서 본다. */
  system: Object.freeze({ pad: 24, labelRoom: 108, fitMax: 1.25, fitMin: 0.3 }),
  /* 라벨의 자리를 다시 쓰는 문턱(px)과 반올림의 눈금. 배율이 `lod` 밑이면 이름은
   * 확인 필요·실패·고른 것·손이 오른 것만 선다 — 누를 자리는 그대로 남는다. 도는 몸이
   * `crowd`개를 넘는 판에서는 대기·완료의 이름도 손이 올라야 선다(60개 판 실측:
   * 이름이 계마다 겹쳐 확인 필요의 칩을 가렸다). */
  label: Object.freeze({ epsilon: 0.2, round: 10, lod: 0.5, crowd: 24 }),
  /* 캔버스가 읽는 잉크 — `--agent-orbit-<이름>`. 워크스페이스의 빛깔은 관계 그림이
   * 매긴 레인(`laneClass`)이고, 레인이 없는 워크스페이스는 첫 레인을 입는다. */
  starInk: "lane-1",
  inks: Object.freeze(["working", "attention", "idle", "done", "failed", "ring", "link", "mail",
    "unread", "select", "core", "lane-1", "lane-2", "lane-3", "lane-4", "lane-5"]),
});

const ORBIT_TURN = Math.PI * 2;
/* 먼 궤도 점선의 한 마디 — 점선이 도는 자리를 이 길이 안에 둔다. */
const ORBIT_DASH = ORBIT.ring.dash.reduce((total, one) => total + one, 0);

/* ---- 고른 보기 ------------------------------------------------------------- */

/* 움직임을 줄이라는 판인가 — 한 번 묻고 바뀔 때 듣는다. */
const agentOrbitMotion = typeof window.matchMedia === "function" ? window.matchMedia(ORBIT.reduced) : null;

function agentOrbitReduced() {
  return agentOrbitMotion?.matches === true;
}

/* 사람이 고른 보기. `null`은 아직 저장소를 읽지 않았다는 뜻이다 — 보드를 처음 그릴
 * 때 한 번 읽는다. */
let agentOrbitHeld = null;

/* 고른 적이 없는 사람의 보기: 행성계. 움직임을 줄이라는 판에서는 카드 — 움직이는
 * 그림이 기본으로 서면 그 사람이 처음 보는 것이 그 사람이 끄라고 한 움직임이다. */
function agentOrbitDefaultChoice() {
  return agentOrbitReduced() ? ORBIT.cards : ORBIT.orbit;
}

function agentOrbitChoice() {
  if (agentOrbitHeld !== null) return agentOrbitHeld;
  let kept = null;
  try {
    kept = localStorage.getItem(ORBIT.store);
  } catch {
    // 저장소를 못 읽는 판은 고른 적 없는 판이다.
  }
  agentOrbitHeld = kept === ORBIT.orbit || kept === ORBIT.cards ? kept : agentOrbitDefaultChoice();
  return agentOrbitHeld;
}

/* 토글의 손. 고른 것을 한 키에 적고 관계 그림을 다시 그린다 — 그림이 둘 중 무엇을
 * 세울지는 `dressAgentOrbitMode`가 정한다. 행성계로 돌아오는 판은, 사람이 배율을
 * 쥔 적이 없으면 배율을 1로 둔다: 카드 그림의 맞춤이 내린 배율은 카드의 것이다. */
function setAgentOrbitChoice(view, choice) {
  if (choice !== ORBIT.orbit && choice !== ORBIT.cards) return;
  if (agentOrbitChoice() === choice) return;
  agentOrbitHeld = choice;
  try {
    localStorage.setItem(ORBIT.store, choice);
  } catch {
    // 저장하지 못해도 이 창은 고른 보기로 선다 — 다음 시작만 기본으로 돌아간다.
  }
  if (choice === ORBIT.orbit && !agentGraphZoomTaken) setAgentGraphZoom(view, 1);
  const state = agentOrbitStates.get(view);
  if (state) state.pan = { x: 0, y: 0 };
  const model = agentGraphFullModel(view);
  if (model) paintAgentGraph(view, model);
  else wireAgentOrbitToggle(view);
}

/* 툴바의 두 단추. 판마다 다시 맨다 — 복제된 판은 손을 들고 오지 않는다. */
function wireAgentOrbitToggle(view) {
  for (const button of view.querySelectorAll("[data-relations-view]")) {
    writeAttribute(button, "aria-pressed", String(button.dataset.relationsView === agentOrbitChoice()));
    button.onclick = () => setAgentOrbitChoice(view, button.dataset.relationsView);
  }
}

/* 이 판에 행성계가 서는가, 그리고 그 옷. 고른 보기가 행성계이고, 관계 그림이며,
 * 그릴 에이전트가 있을 때만 — 빈 판은 카드 그림의 빈 문장과 창고 띠가 답한다. */
function dressAgentOrbitMode(view, model, taskMode) {
  const orbit = !taskMode && agentOrbitChoice() === ORBIT.orbit && (model?.agents.length ?? 0) > 0;
  view.classList.toggle("is-orbit", orbit);
  writeHidden(view.querySelector(".agent-graph-scroll"), orbit);
  writeHidden(view.querySelector(".agent-orbit"), !orbit);
  const hint = view.querySelector(".agent-graph-gesture-hint");
  if (hint) {
    writeAttribute(hint, "data-i18n", orbit ? "board.orbit.hint" : "board.graph.gestureHint");
    writeTextContent(hint, orbit
      ? t("board.orbit.hint", "행성이나 항성을 누르면 상세가 열립니다")
      : t("board.graph.gestureHint", "카드와 연결선을 눌러보세요"));
  }
  if (!taskMode) wireAgentOrbitToggle(view);
  /* 카드로 돌아간 판은 라벨을 내려놓는다 — 보이지 않는 버튼 열여섯이 카드 그림의
   * 요소 수와 탭 순서에 얹히지 않게. 다시 서는 판이 한 번 짓는다. */
  if (!orbit && !taskMode) agentOrbitRelease(view);
  return orbit;
}

function agentOrbitShowing(view) {
  return view?.classList.contains("is-orbit") === true;
}

/* ---- 판마다의 상태 ----------------------------------------------------------- */

const agentOrbitStates = new WeakMap();
/* 행성계를 그린 적이 있는 판들. 박자는 이 몇 장만 묻는다 — 문서 전체를 프레임마다
 * 훑지 않게. 떠난 판은 박자가 걷는다. */
const agentOrbitViews = new Set();
/* 박자 하나: 걸려 있는 rAF와 마지막으로 그린 때. */
let agentOrbitFrame = null;
let agentOrbitDrawnAt = null;
/* 시험과 보고가 읽는 수. 늘기만 한다. */
let agentOrbitApplied = 0;
let agentOrbitFrames = 0;
let agentOrbitTicks = 0;
let agentOrbitPulses = 0;
let agentOrbitInkReads = 0;

function agentOrbitState(view) {
  const held = agentOrbitStates.get(view);
  if (held) return held;
  const stage = view.querySelector(".agent-orbit");
  const canvas = stage?.querySelector(".agent-orbit-canvas");
  const host = stage?.querySelector(".agent-orbit-labels");
  const ctx = canvas?.getContext("2d");
  if (!stage || !canvas || !host || !ctx) return null;
  /* 복제된 판(`docHost`)은 첫 판의 라벨을 죽은 마크업으로 들고 온다. */
  host.replaceChildren();
  const state = {
    view, stage, canvas, host, ctx,
    width: 0, height: 0, dpr: 1,
    signature: null, selected: null, hovered: null, focused: null,
    stars: new Map(), bodies: new Map(), orbiters: [], links: [],
    fit: 1, scale: 1, pan: { x: 0, y: 0 },
    hold: 1, holdTarget: 1, drift: 0,
    inks: null, glowAlpha: 1, sprites: new Map(),
    shownAtApply: false, dirty: true,
  };
  agentOrbitStates.set(view, state);
  agentOrbitViews.add(view);
  wireAgentOrbitStage(state);
  watchGraphResize(stage, () => agentOrbitResized(view));
  agentOrbitWatchTheme();
  return state;
}

/* 카드로 돌아간 판: 라벨과 몸을 내려놓는다. 다음에 서는 판은 처음 서는 판이다. */
function agentOrbitRelease(view) {
  const state = agentOrbitStates.get(view);
  if (!state || state.bodies.size === 0) return;
  state.host.replaceChildren();
  state.stars.clear();
  state.bodies.clear();
  state.orbiters = [];
  state.links = [];
  state.signature = null;
  state.selected = null;
  state.hovered = null;
  state.focused = null;
  state.hold = 1;
  state.holdTarget = 1;
}

/* 이 판의 행성계가 지금 사람에게 보이는가 — 실시간 지도와 같은 손으로 묻고
 * (`agentGraphPictureOn`·`agentGraphViewStands`), 행성계가 선 판인지와 무대가
 * 자리를 가졌는지를 더한다. 크기를 읽지 않는다: 크기는 관찰자가 이미 적어 두었다. */
function agentOrbitShown(state) {
  return agentGraphPictureOn() && agentGraphViewStands(state.view) && agentOrbitShowing(state.view)
    && state.width > 0 && state.height > 0;
}

/* ---- 관계 그림에서 오는 손 --------------------------------------------------- */

/* `paintAgentGraph`가 관계 그림을 다 그린 뒤 부르는 문. 서명이 움직인 판에서만
 * 상태를 적용하고, 선택만 바뀐 판은 라벨 둘과 다음 그림만 고친다. */
function paintAgentOrbit(view, model) {
  if (!agentOrbitShowing(view)) return;
  const state = agentOrbitState(view);
  if (!state) return;
  const now = Date.now();
  const shown = agentOrbitShown(state);
  const signature = agentOrbitSignature(model, now);
  if (signature !== state.signature) {
    agentOrbitApply(state, model, now, shown);
    state.signature = signature;
  }
  /* 이 판이 본 것이 기준선이다: 숨은 동안 적용된 전이는 돌아온 뒤 맥동하지 않고,
   * 보이는 동안의 다음 전이는 맥동한다. */
  state.shownAtApply = shown;
  agentOrbitDressSelection(state);
  agentOrbitWake();
}

/* 배율이 움직였다(`applyAgentGraphZoom`). 행성계의 배율은 맞춤 × 사람의 배율이다. */
function agentOrbitZoomed(view) {
  const state = agentOrbitStates.get(view);
  /* 판마다 지나는 길이다(`paintAgentGraph`가 배율을 되살린다) — 배율이 그대로면
   * 아무것도 하지 않는다: 움직임을 줄인 판에서 정지 화면을 한 장 더 그리는 것은
   * 상태가 바뀌지 않은 그림이다. */
  if (!state || !agentOrbitShowing(view) || state.scale === state.fit * agentGraphZoom) return;
  agentOrbitLayout(state, { snap: true });
  agentOrbitWake();
}

/* 「전체 보기」: 배율 1, 끌어 옮긴 것은 제자리로. */
function agentOrbitFit(view) {
  const state = agentOrbitStates.get(view);
  if (state) state.pan = { x: 0, y: 0 };
  setAgentGraphZoom(view, 1);
  if (state) {
    agentOrbitLayout(state, { snap: true });
    agentOrbitWake();
  }
}

/* 고른 몸의 라벨에 초점을 — 카드 그림의 `focusAgentGraphSelection`이 행성계에서
 * 하는 일. 스크롤할 것이 없다: 행성계는 판에 맞춰 선다. */
function agentOrbitFocus(view, key) {
  const state = agentOrbitStates.get(view);
  state?.bodies.get(key)?.label?.focus({ preventScroll: true });
}

/* 무대의 크기가 움직였다 — 숨었다 드러난 판도 여기로 온다(크기 0 → 제 크기).
 *
 * 실시간 지도의 떠나는 문도 이 관찰자가 지난다. 지도는 판이 숨거나 닫히는 것을 카드
 * 판의 관찰자(`watchAgentGraphSize`)가 크기 0을 들고 오는 것으로 알았는데, 행성계가
 * 선 판에서 카드 판은 이미 접혀(크기 0) 있어 그 관찰자가 다시 오지 않는다 — 그러면
 * 닫힌 판에 지도의 시계와 맥박이 남고 돌아온 판이 기준선을 치르지 않는다. 보이는
 * 판이 행성계일 때 그 소식을 드는 것은 이 관찰자다. */
function agentOrbitResized(view) {
  agentGraphLiveSettle();
  const state = agentOrbitStates.get(view);
  if (!state) return;
  const width = state.stage.clientWidth;
  const height = state.stage.clientHeight;
  const dpr = window.devicePixelRatio || 1;
  if (width === state.width && height === state.height && dpr === state.dpr) return;
  state.width = width;
  state.height = height;
  state.dpr = dpr;
  state.canvas.width = Math.round(width * dpr);
  state.canvas.height = Math.round(height * dpr);
  state.sprites.clear();
  agentOrbitLayout(state, { snap: true });
  agentOrbitWake();
}

/* ---- 서명과 적용 ------------------------------------------------------------- */

/* 한 행성의 최근 활동량, `steps`단으로. 창의 활동 고리(`paneActivities`, 백엔드
 * 링의 사본)에서 최근 창 안의 호출을 센다. */
function agentOrbitActivity(pane, now) {
  let count = 0;
  for (const stamped of paneActivities.get(pane) ?? []) {
    if (now - (Number(stamped?.at) || 0) <= ORBIT.activity.windowMs) count += 1;
  }
  const level = Math.min(1, count / ORBIT.activity.full);
  return Math.round(level * ORBIT.activity.steps) / ORBIT.activity.steps;
}

function agentOrbitFresh(edge, now) {
  return edge.unread > 0 || now - (Number(edge.at) || 0) <= ORBIT.mail.freshMs;
}

/* 행성계가 쓰는 사실의 서명. 여기 없는 것(카드의 낱말·시계·도구 경로)은 행성계를
 * 움직이지 않으므로, 그것만 바뀐 판은 적용을 부르지 않는다. */
function agentOrbitSignature(model, now) {
  const parts = [locale, model.searchActive ? "search" : ""];
  for (const entry of model.agents) {
    parts.push([
      entry.key, entry.state, entry.workspace?.key ?? "", entry.workspace?.label ?? "",
      entry.workspace?.laneClass ?? "", entry.card.parent ?? "", entry.searchMatch ? "match" : "",
      agentOrbitActivity(entry.card.pane, now), agentGraphIdentity(entry.card), entry.card.ledger ?? "",
    ].join("\u001f"));
  }
  for (const edge of model.overlayData?.mail ?? []) {
    parts.push([edge.key, edge.unread > 0 ? "unread" : "", agentOrbitFresh(edge, now) ? "fresh" : ""]
      .join("\u001f"));
  }
  return parts.join("\u001e");
}

/* 새 몸이 설 각. 같은 중심·같은 궤도의 몸들 사이에서 가장 넓은 빈틈의 한가운데 —
 * 처음 선 판의 몸들이 한 점에 겹쳐 서지 않고, 다시 그려도 같은 자리다. */
function agentOrbitOpening(taken, seed) {
  if (taken.length === 0) return (knowledgeHash(seed) / ORBIT_TURN) % ORBIT_TURN;
  const sorted = [...taken].map((phase) => ((phase % ORBIT_TURN) + ORBIT_TURN) % ORBIT_TURN)
    .sort((left, right) => left - right);
  let best = 0;
  let at = sorted[0];
  sorted.forEach((phase, index) => {
    const next = index + 1 < sorted.length ? sorted[index + 1] : sorted[0] + ORBIT_TURN;
    if (next - phase > best) {
      best = next - phase;
      at = phase + (next - phase) / 2;
    }
  });
  return at % ORBIT_TURN;
}

function agentOrbitSpeed(level) {
  const rest = ORBIT_TURN / ORBIT.period.restMs;
  const busy = ORBIT_TURN / ORBIT.period.busyMs;
  return rest + (busy - rest) * level;
}

/* 모델 한 판을 몸들의 목표로. 부르는 쪽은 서명이 움직였을 때만 부른다. */
function agentOrbitApply(state, model, now, shown) {
  agentOrbitApplied += 1;
  const pulsing = shown && state.shownAtApply;
  const started = performance.now();
  const entries = model.agents;
  const byPane = new Map(entries.map((entry) => [entry.card.pane, entry]));

  /* 항성: 에이전트가 있는 워크스페이스마다 하나. */
  const stars = new Map();
  for (const entry of entries) {
    const workspace = entry.workspace;
    if (!workspace) continue;
    let star = stars.get(workspace.key);
    if (!star) {
      star = state.stars.get(workspace.key) ?? { key: workspace.key, kind: "star", x: 0, y: 0, tx: 0, ty: 0,
        size: 0, alpha: 0, label: null, isNew: true };
      Object.assign(star, { workspace, agents: 0, ink: workspace.laneClass || ORBIT.starInk,
        name: workspace.label, dim: false, matched: false });
      stars.set(workspace.key, star);
    }
    star.agents += 1;
    star.matched ||= entry.searchMatch !== false;
  }
  for (const star of stars.values()) {
    star.targetSize = ORBIT.star.radius + ORBIT.star.perAgent * Math.min(star.agents, ORBIT.star.agentCap);
    star.dim = model.searchActive && !star.matched;
  }

  /* 누가 누구를 도는가: 같은 워크스페이스의 부모가 있으면 그 부모를, 아니면 항성을.
   * 고리처럼 이어진 계보는 끊어서 행성으로 세운다. */
  const hostOf = new Map();
  for (const entry of entries) {
    const parent = byPane.get(entry.card.parent);
    hostOf.set(entry.key, parent && parent !== entry && parent.workspace?.key === entry.workspace?.key
      ? parent.key : null);
  }
  const depthOf = (key) => {
    const seen = new Set();
    let depth = 0;
    let at = hostOf.get(key);
    while (at) {
      if (seen.has(at)) return -1;
      seen.add(at);
      depth += 1;
      at = hostOf.get(at);
    }
    return depth;
  };
  for (const entry of entries) {
    if (depthOf(entry.key) < 0) hostOf.set(entry.key, null);
  }
  const children = new Map();
  for (const entry of entries) {
    const host = hostOf.get(entry.key);
    if (host) children.set(host, [...(children.get(host) ?? []), entry.key]);
  }

  /* 행성과 위성. 이미 있던 몸은 제 각과 자리를 지키고 목표만 바꾼다. */
  const bodies = new Map();
  const ordered = [...entries].filter((entry) => entry.workspace)
    .sort((left, right) => depthOf(left.key) - depthOf(right.key) || left.key.localeCompare(right.key));
  for (const entry of ordered) {
    const held = state.bodies.get(entry.key);
    const hostKey = hostOf.get(entry.key);
    const moon = hostKey !== null && hostKey !== undefined;
    const known = ORBIT.states[entry.state] ?? ORBIT.states.idle;
    const body = held ?? { key: entry.key, phase: null, orbit: 0, size: 0, alpha: 0, x: 0, y: 0,
      label: null, pulseAt: null, isNew: true };
    const before = body.state;
    const level = agentOrbitActivity(entry.card.pane, now);
    const siblings = moon ? children.get(hostKey) ?? [] : [];
    Object.assign(body, {
      kind: moon ? "moon" : "planet",
      entry, state: entry.state, ink: known.ink, ringed: known.ringed, ring: known.ring,
      hostKey: moon ? hostKey : entry.workspace.key,
      name: agentGraphIdentity(entry.card),
      speed: agentOrbitSpeed(level), level,
      dim: model.searchActive && entry.searchMatch === false,
      targetOrbit: moon ? ORBIT.moon.orbit + ORBIT.moon.step * Math.max(0, siblings.indexOf(entry.key))
        : ORBIT.ring.first + ORBIT.ring.gap * known.ring,
      targetSize: moon ? ORBIT.moon.radius
        : ORBIT.planet.radius + ORBIT.planet.perChild * Math.min((children.get(entry.key) ?? []).length,
          ORBIT.planet.childCap),
      moons: (children.get(entry.key) ?? []).length,
    });
    if (pulsing && before === "working" && entry.state !== "working") {
      body.pulseAt = started;
      agentOrbitPulses += 1;
    }
    bodies.set(entry.key, body);
  }
  /* 새 몸의 각: 같은 중심·같은 궤도의 빈틈에. */
  for (const body of bodies.values()) {
    if (body.phase !== null) continue;
    const taken = [...bodies.values()].filter((one) => one !== body && one.phase !== null
      && one.hostKey === body.hostKey && one.targetOrbit === body.targetOrbit).map((one) => one.phase);
    body.phase = agentOrbitOpening(taken, `${body.hostKey}\u001f${body.targetOrbit}\u001f${body.key}`);
  }
  for (const body of bodies.values()) {
    body.host = body.kind === "moon" ? bodies.get(body.hostKey) : stars.get(body.hostKey);
    if (body.isNew) body.orbit = body.targetOrbit;
  }

  /* 링크: 두 끝이 다 그림에 있는 우편. 입자는 제 자리(`t`)를 지킨다. */
  const heldLinks = new Map(state.links.map((link) => [link.key, link]));
  const links = [];
  for (const edge of model.overlayData?.mail ?? []) {
    const from = bodies.get(edge.from);
    const to = bodies.get(edge.to);
    if (!from || !to || from === to) continue;
    const held = heldLinks.get(edge.key);
    const fresh = agentOrbitFresh(edge, now);
    links.push({ key: edge.key, from, to, unread: edge.unread > 0, fresh, at: Number(edge.at) || 0,
      particles: fresh ? held?.particles?.length ? held.particles
        : Array.from({ length: ORBIT.mail.perLink }, (_, index) => ({
          t: ((knowledgeHash(edge.key) / ORBIT_TURN) + index / ORBIT.mail.perLink) % 1,
        })) : [] });
  }
  /* 입자의 상한: 넘치면 오래된 링크의 입자부터 접는다 — 링크 자체는 그대로 선다. */
  let flowing = 0;
  for (const link of [...links].sort((left, right) => right.at - left.at)) {
    if (flowing + link.particles.length > ORBIT.mail.cap) link.particles = [];
    flowing += link.particles.length;
  }

  state.stars = stars;
  state.bodies = bodies;
  state.orbiters = [...bodies.values()];
  state.links = links;
  state.stage.classList.toggle("is-crowded", bodies.size > ORBIT.label.crowd);
  agentOrbitLabels(state);
  agentOrbitLayout(state, { snap: false });
  for (const body of [...stars.values(), ...bodies.values()]) body.isNew = false;

  /* 캔버스가 말하는 요약 — 그림을 볼 수 없는 사람에게 같은 수를. */
  const working = entries.filter((entry) => entry.state === "working").length;
  const attention = entries.filter((entry) => entry.state === "needs-attention").length;
  writeAttribute(state.canvas, "aria-label", t("board.orbit.summary",
    "에이전트 {{count}}개 · 작업 중 {{working}} · 확인 필요 {{attention}}",
    { count: entries.length, working, attention }));
  state.dirty = true;
}

/* ---- 자리 -------------------------------------------------------------------- */

/* 계(系)들을 판에 앉힌다: 에이전트가 많은 계부터, 판의 가로세로에 가장 크게 드는
 * 열 수로. 칸의 크기는 모든 계가 같다(궤도 넷이 같은 문법이므로) — 사람이 배운
 * 「안쪽이 작업 중」은 어느 계에서나 같은 자리다. `snap`이면 목표로 바로 간다
 * (크기·배율·끌기), 아니면 프레임이 미끄러뜨린다(계가 늘거나 줄 때). */
function agentOrbitLayout(state, { snap }) {
  const stars = [...state.stars.values()].sort((left, right) =>
    right.agents - left.agents || left.name.localeCompare(right.name));
  const reach = ORBIT.ring.first + ORBIT.ring.gap * (ORBIT.rings - 1) + ORBIT.system.pad;
  const tall = reach * 2;
  const wide = tall + ORBIT.system.labelRoom;
  let columns = 1;
  let fit = 0;
  for (let count = 1; count <= Math.max(1, stars.length); count += 1) {
    const rows = Math.ceil(stars.length / count);
    const room = Math.min(state.width / (count * wide), state.height / (rows * tall));
    if (room > fit) {
      fit = room;
      columns = count;
    }
  }
  state.fit = Math.max(ORBIT.system.fitMin, Math.min(ORBIT.system.fitMax, fit || 1));
  state.scale = state.fit * agentGraphZoom;
  state.stage.classList.toggle("is-lod", state.scale < ORBIT.label.lod);
  const rows = Math.max(1, Math.ceil(stars.length / columns));
  const cellWide = wide * state.scale;
  const cellTall = tall * state.scale;
  const top = (state.height - rows * cellTall) / 2 + state.pan.y;
  stars.forEach((star, index) => {
    const row = Math.floor(index / columns);
    const inRow = Math.min(columns, stars.length - row * columns);
    const left = (state.width - inRow * cellWide) / 2 + state.pan.x;
    const column = index - row * columns;
    star.tx = left + column * cellWide + (cellWide - ORBIT.system.labelRoom * state.scale) / 2;
    star.ty = top + row * cellTall + cellTall / 2;
    if (snap || star.isNew) {
      star.x = star.tx;
      star.y = star.ty;
    }
  });
  for (const body of [...state.stars.values(), ...state.bodies.values()]) {
    if (body.label) {
      writeStyleProperty(body.label, "--orbit-body",
        `${Math.round((body.targetSize * state.scale + ORBIT.planet.hit) * ORBIT.label.round) / ORBIT.label.round}px`);
    }
  }
  state.dirty = true;
}

/* ---- 라벨 -------------------------------------------------------------------- */

/* 라벨 하나: 몸을 덮는 누를 자리 + 이름, 상태 칩은 `::after`가 `data-chip`을 읽는다.
 * 요소가 둘뿐인 것은 이 그림의 DOM 예산 때문이다(카드 그림 대비 +50 이하). */
function agentOrbitLabelNode() {
  const label = document.createElement("button");
  label.type = "button";
  label.className = "agent-orbit-label";
  label.append(Object.assign(document.createElement("span"), { className: "agent-orbit-name" }));
  return label;
}

function agentOrbitLabels(state) {
  const existing = new Map([...state.host.children].map((node) => [node.dataset.orbitKey, node]));
  const wanted = [];
  for (const star of state.stars.values()) {
    const label = existing.get(star.key) ?? agentOrbitLabelNode();
    const count = agentGraphCountWord("agent", star.agents);
    writeClassName(label, ["agent-orbit-label", "is-star", star.ink, star.dim ? "is-search-dimmed" : ""]
      .filter(Boolean).join(" "));
    writeAttribute(label, "data-orbit-key", star.key);
    writeAttribute(label, "data-chip", count);
    writeAttribute(label, "aria-label", `${star.name}, ${count}`);
    writeAttribute(label, "data-tip", `${t("board.graph.workspace", "워크스페이스")} · ${star.name} · ${count}`);
    writeTextContent(label.firstElementChild, star.name);
    star.label = label;
    wanted.push(label);
  }
  for (const body of state.bodies.values()) {
    const label = existing.get(body.key) ?? agentOrbitLabelNode();
    const word = agentGraphStateWord(body.state, body.entry.card.ledger ?? "");
    /* 쪽(`is-west`)은 프레임이 정한다 — 다시 쓰는 옷이 그것을 떨어뜨리면 서쪽의
     * 이름이 한 프레임 동안 동쪽으로 튄다. */
    writeClassName(label, ["agent-orbit-label", `is-${body.kind}`, `is-${body.state}`,
      body.dim ? "is-search-dimmed" : "", body.west === true ? "is-west" : ""].filter(Boolean).join(" "));
    writeAttribute(label, "data-orbit-key", body.key);
    writeAttribute(label, "data-chip", word);
    writeAttribute(label, "aria-label", `${body.name}, ${word}`);
    writeAttribute(label, "data-tip", [body.name, word, body.entry.workspace?.label].filter(Boolean).join(" · "));
    writeTextContent(label.firstElementChild, body.name);
    body.label = label;
    body.placed = null;
    body.west = label.classList.contains("is-west");
    wanted.push(label);
  }
  reconcileElementOrder(state.host, wanted);
  state.selected = null;
}

/* 고른 몸: 라벨의 `aria-pressed`와 다음 그림의 고리. 바뀐 판에서만 쓴다. */
function agentOrbitDressSelection(state) {
  if (state.selected === agentGraphSelectedKey) return;
  for (const key of [state.selected, agentGraphSelectedKey]) {
    const label = state.stars.get(key)?.label ?? state.bodies.get(key)?.label;
    if (label) writeAttribute(label, "aria-pressed", String(key === agentGraphSelectedKey));
  }
  for (const label of state.host.children) {
    if (!label.hasAttribute("aria-pressed")) writeAttribute(label, "aria-pressed", "false");
  }
  state.selected = agentGraphSelectedKey;
  state.dirty = true;
}

/* 무대의 손들, 한 번. 라벨을 누르면 관계 그림의 선택 길로, 손이 오르거나 초점이
 * 들면 공전이 멈춘다(움직이는 과녁은 누르기 어렵다). 빈 곳을 끌면 판이 움직이고,
 * ⌘/Ctrl 휠은 관계 그림의 배율을 움직인다. */
function wireAgentOrbitStage(state) {
  const { view, stage, host } = state;
  const keyOf = (node) => (node instanceof Element ? node.closest("[data-orbit-key]") : null);
  const hold = () => {
    state.holdTarget = state.hovered || state.focused ? 0 : 1;
    agentOrbitWake();
  };
  host.onclick = (event) => {
    const label = keyOf(event.target);
    if (label) selectAgentGraphEntity(view, label.dataset.orbitKey);
  };
  host.onpointerover = (event) => {
    const label = keyOf(event.target);
    state.hovered = label?.dataset.orbitKey ?? null;
    hold();
  };
  host.onpointerout = (event) => {
    if (keyOf(event.relatedTarget)) return;
    state.hovered = null;
    hold();
  };
  /* `focusin`은 속성 손(`onfocusin`)이 없는 엔진이 있다 — 귀로 건다. 이 함수는 판마다
   * 한 번만 돈다(`agentOrbitState`). */
  host.addEventListener("focusin", (event) => {
    state.focused = keyOf(event.target)?.dataset.orbitKey ?? null;
    hold();
  });
  host.addEventListener("focusout", (event) => {
    if (keyOf(event.relatedTarget)) return;
    state.focused = null;
    hold();
  });
  let from = { x: 0, y: 0 };
  wireGraphDrag(stage, {
    /* 스페이스를 쥔 손은 라벨 위에서도 판을 민다 — 카드 그림과 같은 몸짓. */
    grabbed: () => agentGraphSpaceHeld,
    onGrab: () => {
      from = { ...state.pan };
    },
    onDrag: (moveX, moveY) => {
      agentGraphPauseFollow(view);
      stage.classList.add("is-panning");
      state.pan = { x: from.x + moveX, y: from.y + moveY };
      agentOrbitLayout(state, { snap: true });
      agentOrbitWake();
    },
    onEnd: () => stage.classList.remove("is-panning"),
  });
  stage.onwheel = (event) => {
    if (!event.ctrlKey && !event.metaKey) return;
    event.preventDefault();
    agentGraphPauseFollow(view);
    takeAgentGraphZoom(view, agentGraphZoom * graphWheelZoomFactor(event.deltaY));
  };
}

/* ---- 잉크 -------------------------------------------------------------------- */

/* 캔버스가 쓰는 색: 무대 위의 `--agent-orbit-*` 토큰을 계산된 색으로. 캔버스 자신이
 * 탐침이다 — `color`에 토큰을 걸고 계산된 값을 읽는다. 테마가 바뀌면 한 곳
 * (`agentOrbitWatchTheme`)이 이것을 비우고, 다음 그림이 한 번 다시 읽는다. */
function agentOrbitInks(state) {
  if (state.inks) return state.inks;
  const probe = state.canvas;
  const held = probe.style.color;
  const inks = {};
  for (const name of ORBIT.inks) {
    probe.style.color = `var(--agent-orbit-${name})`;
    inks[name] = getComputedStyle(probe).color;
  }
  probe.style.color = held;
  if (Object.values(inks).some((ink) => !ink)) return null;
  const scheme = getComputedStyle(document.documentElement).colorScheme;
  state.glowAlpha = String(scheme).includes("light") ? ORBIT.glow.light : ORBIT.glow.dark;
  state.inks = inks;
  state.sprites.clear();
  agentOrbitInkReads += 1;
  return inks;
}

/* 빛무리 한 장, 잉크마다. 두 겹의 방사 그라디언트(넓고 옅은 것, 좁고 진한 것)를
 * 그린 뒤 그 알파만 남기고 잉크로 칠한다 — 색을 문자열로 쪼개지 않는다. */
function agentOrbitSprite(state, name) {
  const held = state.sprites.get(name);
  if (held) return held;
  const size = Math.max(1, Math.round(ORBIT.glow.sprite * state.dpr));
  const sprite = document.createElement("canvas");
  sprite.width = size;
  sprite.height = size;
  const paint = sprite.getContext("2d");
  const middle = size / 2;
  const ink = state.inks[name];
  for (const [reach, strength] of [[middle, ORBIT.glow.outer], [middle * ORBIT.glow.core, 1]]) {
    const gradient = paint.createRadialGradient(middle, middle, 0, middle, middle, reach);
    gradient.addColorStop(0, ink);
    gradient.addColorStop(1, "transparent");
    paint.globalAlpha = strength;
    paint.fillStyle = gradient;
    paint.fillRect(0, 0, size, size);
  }
  paint.globalAlpha = 1;
  paint.globalCompositeOperation = "source-in";
  paint.fillStyle = ink;
  paint.fillRect(0, 0, size, size);
  state.sprites.set(name, sprite);
  return sprite;
}

let agentOrbitThemeWatch = null;

/* 테마를 보는 눈 하나 — 창 전체에서. `data-theme` 한 속성만 본다. */
function agentOrbitWatchTheme() {
  if (agentOrbitThemeWatch !== null || typeof MutationObserver !== "function") return;
  agentOrbitThemeWatch = new MutationObserver(() => {
    for (const view of agentOrbitViews) {
      const state = agentOrbitStates.get(view);
      if (!state) continue;
      state.inks = null;
      state.dirty = true;
    }
    agentOrbitWake();
  });
  agentOrbitThemeWatch.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
}

/* ---- 박자 -------------------------------------------------------------------- */

/* 보이는 판마다: 움직일 판이면 박자를 걸고, 움직임을 줄인 판이면 바뀐 것이 있을
 * 때만 정지 화면 한 장. 어느 쪽도 보이지 않는 판에는 아무것도 하지 않는다. */
function agentOrbitWake() {
  const reduced = agentOrbitReduced();
  let moving = false;
  for (const view of agentOrbitViews) {
    const state = agentOrbitStates.get(view);
    if (!state || !agentOrbitShown(state)) continue;
    if (!reduced) {
      moving = true;
      continue;
    }
    if (state.dirty) agentOrbitDraw(state, 0, performance.now(), { still: true });
  }
  if (moving && agentOrbitFrame === null) agentOrbitFrame = requestAnimationFrame(agentOrbitTick);
}

/* 박자 하나. 보이는 판이 없으면 다음 rAF를 걸지 않는다 — 숨은 판의 rAF는 0이다.
 * 초점이 없는 창은 박자를 건너뛰어 초당 30장까지만 그린다. */
function agentOrbitTick(stamp) {
  agentOrbitFrame = null;
  agentOrbitTicks += 1;
  if (agentOrbitReduced()) {
    agentOrbitDrawnAt = null;
    return;
  }
  const shown = [];
  for (const view of agentOrbitViews) {
    const state = agentOrbitStates.get(view);
    if (!view.isConnected || !state) {
      agentOrbitViews.delete(view);
      continue;
    }
    if (agentOrbitShown(state)) shown.push(state);
  }
  if (shown.length === 0) {
    agentOrbitDrawnAt = null;
    return;
  }
  const rate = document.hasFocus() ? ORBIT.fps.focused : ORBIT.fps.blurred;
  const gap = ORBIT.second / rate - ORBIT.fps.slackMs;
  if (agentOrbitDrawnAt === null || stamp - agentOrbitDrawnAt >= gap) {
    const step = agentOrbitDrawnAt === null ? 0 : Math.min(ORBIT.stepCapMs, stamp - agentOrbitDrawnAt);
    agentOrbitDrawnAt = stamp;
    for (const state of shown) agentOrbitDraw(state, step, stamp);
  }
  agentOrbitFrame = requestAnimationFrame(agentOrbitTick);
}

/* ---- 그림 -------------------------------------------------------------------- */

/* 두 끝과 굽은 정도로 링크의 조절점을 — 보낸 쪽에서 받는 쪽으로 오른쪽으로 굽으므로
 * 오가는 두 링크는 서로 다른 쪽으로 휜다. */
function agentOrbitBend(link) {
  const { from, to } = link;
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  link.cx = (from.x + to.x) / 2 - dy * ORBIT.mail.bend;
  link.cy = (from.y + to.y) / 2 + dx * ORBIT.mail.bend;
  link.span = Math.max(1, Math.hypot(dx, dy));
}

function agentOrbitAlong(link, at) {
  const rest = 1 - at;
  return [
    rest * rest * link.from.x + 2 * rest * at * link.cx + at * at * link.to.x,
    rest * rest * link.from.y + 2 * rest * at * link.cy + at * at * link.to.y,
  ];
}

/* 한 장. `step`은 지난 그림 뒤로 흐른 밀리초(정지 화면은 0)이고, 공전과 입자는
 * 손이 오른 동안 멈춘다(`hold`). 목표로의 미끄러짐은 멈추지 않는다. */
function agentOrbitDraw(state, step, stamp, { still = false } = {}) {
  const inks = agentOrbitInks(state);
  if (!inks || state.width <= 0 || state.height <= 0) return;
  const { ctx, scale } = state;
  const settle = still ? 1 : 1 - Math.exp(-step / ORBIT.settleMs);
  /* 멈춤은 곧게 줄어 `holdMs`에 정확히 선다 — 지수로 다가가면 끝내 조금씩 흐른다. */
  const ramp = still ? 1 : step / ORBIT.holdMs;
  state.hold = state.holdTarget < state.hold
    ? Math.max(state.holdTarget, state.hold - ramp) : Math.min(state.holdTarget, state.hold + ramp);
  const sim = still ? 0 : step * state.hold;
  for (const star of state.stars.values()) {
    star.x += (star.tx - star.x) * settle;
    star.y += (star.ty - star.y) * settle;
    star.size += (star.targetSize - star.size) * settle;
    star.alpha += (1 - star.alpha) * settle;
  }
  for (const body of state.orbiters) {
    body.phase = (body.phase + sim * body.speed) % ORBIT_TURN;
    body.orbit += (body.targetOrbit - body.orbit) * settle;
    body.size += (body.targetSize - body.size) * settle;
    body.alpha += (1 - body.alpha) * settle;
    const host = body.host;
    if (!host) continue;
    body.x = host.x + Math.cos(body.phase) * body.orbit * scale;
    body.y = host.y + Math.sin(body.phase) * body.orbit * scale;
  }

  ctx.setTransform(state.dpr, 0, 0, state.dpr, 0, 0);
  ctx.clearRect(0, 0, state.width, state.height);
  ctx.lineCap = "round";

  /* 궤도: 안쪽(작업 중)은 제 계의 빛깔로 조금 더 밝게, 먼 궤도(완료)는 천천히 도는
   * 점선으로. */
  ctx.lineWidth = ORBIT.ring.width;
  const inner = ORBIT.ring.first * scale;
  ctx.globalAlpha = ORBIT.ring.inner;
  for (const star of state.stars.values()) {
    ctx.strokeStyle = inks[star.ink];
    ctx.beginPath();
    ctx.arc(star.x, star.y, inner, 0, ORBIT_TURN);
    ctx.stroke();
  }
  ctx.strokeStyle = inks.ring;
  ctx.globalAlpha = ORBIT.ring.alpha;
  state.drift = (state.drift + sim * ORBIT.ring.drift) % ORBIT_DASH;
  for (let ring = 1; ring < ORBIT.rings; ring += 1) {
    const far = ring === ORBIT.rings - 1;
    ctx.setLineDash(far ? ORBIT.ring.dash : []);
    ctx.lineDashOffset = far ? -state.drift : 0;
    ctx.beginPath();
    for (const star of state.stars.values()) {
      const radius = (ORBIT.ring.first + ORBIT.ring.gap * ring) * scale;
      ctx.moveTo(star.x + radius, star.y);
      ctx.arc(star.x, star.y, radius, 0, ORBIT_TURN);
    }
    ctx.stroke();
  }
  ctx.setLineDash([]);
  ctx.lineDashOffset = 0;
  ctx.globalAlpha = ORBIT.moon.alpha;
  ctx.beginPath();
  for (const body of state.orbiters) {
    if (body.moons === 0) continue;
    for (let moon = 0; moon < body.moons; moon += 1) {
      const radius = (ORBIT.moon.orbit + ORBIT.moon.step * moon) * scale;
      ctx.moveTo(body.x + radius, body.y);
      ctx.arc(body.x, body.y, radius, 0, ORBIT_TURN);
    }
  }
  ctx.stroke();

  /* 링크: 고른 몸에 닿은 것은 밝게, 방금 오간 것은 보이게, 오래된 것과 검색에
   * 흐려진 몸에 닿은 것은 거의 보이지 않게 — 링크 마흔 개가 실타래가 되지 않는다. */
  const selected = state.selected;
  const groups = [[], [], []];
  for (const link of state.links) {
    agentOrbitBend(link);
    const lit = link.from.key === selected || link.to.key === selected;
    groups[lit ? 0 : link.fresh && !link.from.dim && !link.to.dim ? 1 : 2].push(link);
  }
  ctx.lineWidth = ORBIT.mail.width;
  [[inks.mail, ORBIT.mail.litAlpha], [inks.link, ORBIT.mail.linkAlpha], [inks.link, ORBIT.mail.quietAlpha]]
    .forEach(([ink, alpha], index) => {
      if (groups[index].length === 0) return;
      ctx.strokeStyle = ink;
      ctx.globalAlpha = alpha;
      ctx.beginPath();
      for (const link of groups[index]) {
        ctx.moveTo(link.from.x, link.from.y);
        ctx.quadraticCurveTo(link.cx, link.cy, link.to.x, link.to.y);
      }
      ctx.stroke();
    });

  /* 꼬리: 최근 10초의 궤적, 행성 뒤로 옅어진다. */
  ctx.lineWidth = ORBIT.trail.width;
  const segments = ORBIT.trail.segments;
  for (const body of state.orbiters) {
    const host = body.host;
    if (!host) continue;
    const span = Math.min(ORBIT.trail.maxTurn * ORBIT_TURN, body.speed * ORBIT.trail.spanMs);
    const radius = body.orbit * scale;
    ctx.strokeStyle = inks[body.ink];
    const fade = body.alpha * (body.dim ? ORBIT.dim : 1) * ORBIT.trail.alpha;
    for (let piece = 0; piece < segments; piece += 1) {
      ctx.globalAlpha = fade * (1 - piece / segments);
      ctx.beginPath();
      ctx.arc(host.x, host.y, radius, body.phase - span * (piece + 1) / segments, body.phase - span * piece / segments);
      ctx.stroke();
    }
  }

  /* 우편의 입자: 보낸 쪽 → 받는 쪽, 미확인이 있으면 밝고 크다. */
  for (const link of state.links) {
    if (link.particles.length === 0) continue;
    const ink = link.unread ? "unread" : "mail";
    const sprite = agentOrbitSprite(state, ink);
    const alpha = (link.unread ? ORBIT.mail.unreadAlpha : ORBIT.mail.readAlpha)
      * (link.from.dim || link.to.dim ? ORBIT.dim : 1);
    const radius = link.unread ? ORBIT.mail.unreadRadius : ORBIT.mail.radius;
    for (const particle of link.particles) {
      if (still) particle.t = ORBIT.mail.stillAt;
      else particle.t = (particle.t + sim / ORBIT.mail.travelMs) % 1;
      const [x, y] = agentOrbitAlong(link, particle.t);
      const [tailX, tailY] = agentOrbitAlong(link, Math.max(0, particle.t - ORBIT.mail.tailPx / link.span));
      const glow = radius * ORBIT.mail.glow;
      ctx.globalAlpha = alpha * state.glowAlpha;
      ctx.drawImage(sprite, x - glow, y - glow, glow * 2, glow * 2);
      ctx.globalAlpha = alpha * ORBIT.mail.tailAlpha;
      ctx.strokeStyle = inks[ink];
      ctx.lineWidth = ORBIT.mail.tailWidth;
      ctx.beginPath();
      ctx.moveTo(tailX, tailY);
      ctx.lineTo(x, y);
      ctx.stroke();
      ctx.globalAlpha = alpha;
      ctx.fillStyle = inks[ink];
      ctx.beginPath();
      ctx.arc(x, y, radius, 0, ORBIT_TURN);
      ctx.fill();
    }
  }

  /* 항성: 제 빛깔의 빛무리, 흰 속, 제 빛깔의 테. */
  for (const star of state.stars.values()) {
    const radius = star.size * scale;
    const alpha = star.alpha * (star.dim ? ORBIT.dim : 1);
    const glow = radius * ORBIT.star.glow;
    ctx.globalAlpha = alpha * state.glowAlpha;
    ctx.drawImage(agentOrbitSprite(state, star.ink), star.x - glow, star.y - glow, glow * 2, glow * 2);
    ctx.globalAlpha = alpha;
    ctx.fillStyle = inks.core;
    ctx.beginPath();
    ctx.arc(star.x, star.y, radius, 0, ORBIT_TURN);
    ctx.fill();
    ctx.strokeStyle = inks[star.ink];
    ctx.lineWidth = ORBIT.star.rimWidth;
    ctx.beginPath();
    ctx.arc(star.x, star.y, radius + ORBIT.star.rim, 0, ORBIT_TURN);
    ctx.stroke();
    if (star.key === selected) agentOrbitRing(ctx, inks.select, star.x, star.y, radius + ORBIT.star.rim + ORBIT.select.gap, 1);
    else if (star.key === state.hovered) {
      agentOrbitRing(ctx, inks.select, star.x, star.y, radius + ORBIT.star.rim + ORBIT.select.gap, ORBIT.select.hoverAlpha);
    }
  }

  /* 행성과 위성: 빛무리, 몸, 확인 필요·실패의 고리, 고른 것의 고리, 맥동. */
  for (const body of state.orbiters) {
    const radius = body.size * scale;
    const alpha = body.alpha * (body.dim ? ORBIT.dim : 1);
    const glow = radius * (body.kind === "moon" ? ORBIT.moon.glow : ORBIT.planet.glow);
    ctx.globalAlpha = alpha * state.glowAlpha;
    ctx.drawImage(agentOrbitSprite(state, body.ink), body.x - glow, body.y - glow, glow * 2, glow * 2);
    ctx.globalAlpha = alpha;
    ctx.fillStyle = inks[body.ink];
    ctx.beginPath();
    ctx.arc(body.x, body.y, radius, 0, ORBIT_TURN);
    ctx.fill();
    if (body.ringed) {
      agentOrbitRing(ctx, inks[body.ink], body.x, body.y, radius + ORBIT.planet.ringGap, alpha, ORBIT.planet.ringWidth);
    }
    if (body.key === selected) {
      agentOrbitRing(ctx, inks.select, body.x, body.y, radius + ORBIT.planet.ringGap + ORBIT.select.gap, 1);
    } else if (body.key === state.hovered) {
      agentOrbitRing(ctx, inks.select, body.x, body.y, radius + ORBIT.planet.ringGap + ORBIT.select.gap,
        ORBIT.select.hoverAlpha);
    }
    if (body.pulseAt !== null) {
      const progress = still ? 1 : (stamp - body.pulseAt) / ORBIT.pulse.ms;
      if (progress >= 1 || progress < 0) {
        body.pulseAt = null;
      } else {
        const spread = 1 - (1 - progress) ** 2;
        ctx.lineWidth = ORBIT.pulse.width;
        ctx.strokeStyle = inks.working;
        ctx.globalAlpha = (1 - progress) * alpha;
        ctx.beginPath();
        ctx.arc(body.x, body.y, radius + ORBIT.pulse.reach * spread, 0, ORBIT_TURN);
        ctx.stroke();
      }
    }
  }
  ctx.globalAlpha = 1;

  /* 라벨: 몸을 따라 움직인다 — `translate` 하나만 쓰고, 움직임이 문턱보다 작으면
   * 쓰지 않는다. 배치를 읽지 않는다. 궤도의 서쪽 반에 선 몸은 이름을 바깥(왼쪽)에
   * 단다 — 이름이 제 항성과 안쪽 궤도를 가로지르지 않게. 쪽이 바뀌는 것은 반 바퀴에
   * 한 번이라 클래스도 그때만 쓴다. */
  for (const body of [...state.stars.values(), ...state.orbiters]) {
    const label = body.label;
    if (!label) continue;
    if (body.kind !== "star") {
      const west = Math.cos(body.phase) < 0;
      if (body.west !== west) {
        body.west = west;
        label.classList.toggle("is-west", west);
      }
    }
    const x = Math.round(body.x * ORBIT.label.round) / ORBIT.label.round;
    const y = Math.round(body.y * ORBIT.label.round) / ORBIT.label.round;
    const placed = body.placed;
    if (placed && Math.abs(placed[0] - x) < ORBIT.label.epsilon && Math.abs(placed[1] - y) < ORBIT.label.epsilon) {
      continue;
    }
    body.placed = [x, y];
    label.style.translate = `${x}px ${y}px`;
  }
  state.dirty = false;
  agentOrbitFrames += 1;
}

function agentOrbitRing(ctx, ink, x, y, radius, alpha, width = ORBIT.select.width) {
  ctx.strokeStyle = ink;
  ctx.lineWidth = width;
  ctx.globalAlpha = alpha;
  ctx.beginPath();
  ctx.arc(x, y, radius, 0, ORBIT_TURN);
  ctx.stroke();
}

/* ---- 문서의 귀 --------------------------------------------------------------- */

/* 문서가 돌아오면 박자를 다시 건다(숨는 문서는 박자가 스스로 멈춘다). 움직임을
 * 줄이라는 설정이 바뀌면 — 켜진 판은 정지 화면으로, 꺼진 판은 다시 움직임으로. */
document.addEventListener("visibilitychange", () => {
  if (!document.hidden) agentOrbitWake();
});
agentOrbitMotion?.addEventListener?.("change", () => {
  for (const view of agentOrbitViews) {
    const state = agentOrbitStates.get(view);
    if (state) state.dirty = true;
  }
  agentOrbitWake();
});

/* ---- 시험과 보고가 읽는 것 ----------------------------------------------------- */

function agentOrbitHandles() {
  const view = [...agentOrbitViews].find((one) => one.isConnected && agentOrbitStates.has(one));
  const state = view ? agentOrbitStates.get(view) : null;
  const orbiters = state?.orbiters ?? [];
  const links = state?.links ?? [];
  return {
    applied: agentOrbitApplied,
    frames: agentOrbitFrames,
    ticks: agentOrbitTicks,
    pulses: agentOrbitPulses,
    inkReads: agentOrbitInkReads,
    running: agentOrbitFrame !== null,
    reduced: agentOrbitReduced(),
    stars: state?.stars.size ?? 0,
    planets: orbiters.filter((body) => body.kind === "planet").length,
    moons: orbiters.filter((body) => body.kind === "moon").length,
    bodies: (state?.stars.size ?? 0) + orbiters.length,
    links: links.length,
    particles: links.reduce((total, link) => total + link.particles.length, 0),
    bright: links.filter((link) => link.unread && link.particles.length > 0).length,
    scale: state?.scale ?? null,
    zoom: agentGraphZoom,
    paused: state ? state.holdTarget === 0 : null,
    labels: state?.host.childElementCount ?? 0,
  };
}
