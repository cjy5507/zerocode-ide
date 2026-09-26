/* ---- 관계의 입체 보기 (t-9444 → t-10118) ------------------------------------
 *
 * 보드의 `관계` 그림을 한 번 더 그리는 방법. 카드와 연결선 대신 워크스페이스·에이전트·
 * 하위 에이전트가 3차원 공간의 점으로 서고, 원근 카메라가 그 공간을 비춘다 — 메인
 * 워크스페이스가 원점에, 다른 워크스페이스는 그 둘레의 구면에, 에이전트는 제
 * 워크스페이스 둘레의 작은 구면에, 하위 에이전트는 부모 곁에 선다. 우편과 과업 의존은
 * 두 점 사이의 공간 곡선이고, 우편의 점이 그 곡선을 탄다. 카드 그림은 그대로 남고,
 * 툴바의 토글 하나가 둘 사이를 오간다(로컬 설정 한 키).
 *
 * **읽는 것은 관계 그림의 모델 하나다.** `paintAgentGraph`가 그린 그 모델(범위와
 * 검색이 이미 먹은 것)을 받아 점과 선으로 다시 읽을 뿐, 새 백엔드 호출도 원장 읽기도
 * 없다. 상태의 낱말(`agentGraphStateWord`)·이름(`agentGraphIdentity`)·선택
 * (`selectAgentGraphEntity`)·배율(`agentGraphZoom`)·관계 목록(`agentGraphRelations`)은
 * 관계 그림의 것을 그대로 쓴다 — 한 화면이 같은 에이전트를 두 가지로 말하지 않게.
 *
 * **상태는 모델이 움직일 때만 적용한다.** 이 보기가 쓰는 사실(상태·부모·검색·우편의
 * 신선함·의존)의 서명이 바뀐 판에서만 점들의 목표가 새로 서고(`agentOrbitApplied`),
 * 프레임은 그 목표로 **보간만** 한다 — 같은 판을 다시 그리는 훅의 박자와 프레임은
 * 적용을 한 번도 부르지 않는다.
 *
 * **자리는 한 번 앉으면 지킨다.** 한 중심 둘레의 칸은 피보나치 구면의 격자이고, 새 점은
 * 빈 칸 중 이미 앉은 점들과 가장 먼 칸에 앉는다(첫 점은 키의 해시가 고른 칸). 누가 오고
 * 가도 남의 칸은 그대로라, 에이전트 하나가 와도 다른 점은 움직이지 않는다.
 *
 * **카메라.** 원점을 보는 카메라가 세 수(yaw·pitch·거리)로 선다. 끌면 돌고 놓으면
 * 미끄러져 서며, 가만두면 아주 천천히 돈다 — 사람이 한 번 돌려 놓은 판은 그 자리에
 * 선다(두 번 누르면 맞춤으로 돌아가 다시 돈다). 거리는 관계 그림의 배율이 정한다 — 휠·
 * 단추·「전체 보기」가 한 길이다. 카메라는 저장하지 않는다: 여는 판은 늘 표의 자리다.
 *
 * **깊이.** 먼 것이 먼저 그려지고(화가의 순서), 원근이 크기를 줄이고, 안개가 알파를
 * 줄이며, 바닥의 격자가 공간을 세우고, 이름은 가까운 몇만 선다. 흐림 필터는 쓰지 않는다 —
 * 캔버스 2d에서 그리기마다 값이 든다.
 *
 * **왜 캔버스 2d인가.** 점은 수십, 선은 수백이다. 원근 투영·깊이 정렬·알파 안개는 이
 * 크기에서 2d로 충분하고(60 에이전트 판의 실측은 하네스 표), WebGL은 컨텍스트 유실의
 * 폴백과 WKWebView의 위험과 둘째 페인터의 유지비를 데려온다. 카메라가 멈춘 프레임은
 * 간선을 다시 투영하지 않고 지난 투영의 꺾은선을 그대로 긋는다.
 *
 * **보이지 않거나 멈추면 박자가 없다.** 그림이 서는지는 실시간 지도와 같은 한 손으로
 * 묻는다(`agentGraphPictureOn`·`agentGraphViewStands`, shell-board-live.js) — 숨은
 * 문서·작업 목록·숨은 판·카드 보기에서 rAF는 0이다. 손이나 초점이 라벨에 든 판, 사람이
 * 돌려 놓은 뒤 흐를 것이 없는 판도 0이다: 다음 박자는 움직일 것이 있을 때만 건다.
 * 움직임을 줄이라는 판은 정지 화면이고, 상태가 바뀌거나 사람이 끌 때만 한 장을 다시
 * 그린다. 초점이 없는 창은 초당 30장까지만 그린다.
 *
 * 수는 아래 표 `ORBIT` 하나에 있고, 색은 CSS 토큰(`--agent-orbit-*`, shell.css
 * 「입체」 절)이 든다 — 역할 토큰에서 오므로 라이트·다크가 함께 뒤집힌다. */

/* 이 보기의 수, 한 표. 시각 수치와 데이터 양이 여기 함께 서는 것은 이 그림이 캔버스라
 * CSS가 셈을 대신할 수 없기 때문이다 — 캔버스가 읽는 수가 두 곳에 적히면 한 곳만
 * 움직이는 날이 온다. 공간의 길이는 세계 단위이고, 맞춤 배율 1에서 원점 깊이의 CSS
 * 픽셀 하나다. 화면의 길이(테·틈·굵기)는 CSS 픽셀이다 — 캔버스는 기기 픽셀 비율을 변환
 * 하나(`setTransform`)로 곱한다. */
const ORBIT = Object.freeze({
  /* 고른 보기가 남는 로컬 설정 한 키와 그 값 둘. 값 `orbit`은 사람이 이미 저장한 선택이라
   * 보기의 이름이 바뀌어도 그대로 읽는다. */
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
  /* 멈췄다 온 박자 하나가 흘리는 시간의 상한 — 돌아온 판이 카메라를 한 번에 몇 바퀴
   * 돌리지 않게. */
  stepCapMs: 100,
  /* 목표로 다가가는 시간 상수(지수 접근). 자리나 크기가 바뀐 점은 `settleMs`에 걸쳐
   * 미끄러지고, 처음 선 점은 제 자리·제 크기로 서서 `appearMs`에 걸쳐 나타난다. 남은
   * 거리가 `rest`(세계 단위)·남은 알파가 `fade` 안에 들면 선다 — 끝없이 조금씩 흐르며
   * 박자를 붙잡지 않게. 손이 라벨 위에 오르거나 초점이 들면 카메라와 우편은 `holdMs`에
   * 걸쳐 곧게 줄어 멈추고, 떠나면 같은 시간에 걸쳐 다시 움직인다. */
  settleMs: 700,
  appearMs: 240,
  rest: 0.05,
  fade: 0.02,
  holdMs: 260,
  /* 상태 → 잉크. 확인 필요와 실패는 제 색의 테를 한 겹 더 두른다. 낱말은 관계 그림의
   * 한 손(`agentGraphStateWord`)이 짓는다. */
  states: Object.freeze({
    working: Object.freeze({ ink: "working", ringed: false }),
    "needs-attention": Object.freeze({ ink: "attention", ringed: true }),
    idle: Object.freeze({ ink: "idle", ringed: false }),
    paused: Object.freeze({ ink: "idle", ringed: false }),
    done: Object.freeze({ ink: "done", ringed: false }),
    failed: Object.freeze({ ink: "failed", ringed: true }),
  }),
  /* 공간의 반지름. 프로젝트가 둘 이상이면 프로젝트의 중심이 `project` 구면에, 한
   * 프로젝트 안에서 메인이 아닌 워크스페이스가 `workspace` 구면에, 에이전트가 제
   * 워크스페이스 둘레 `agent` 구면에, 하위 에이전트가 부모 둘레 `child` 거리에 선다. 한
   * 구면의 칸이 다 차면 다음 겹이 반지름의 `shell`배만큼 바깥에 선다. */
  space: Object.freeze({ project: 460, workspace: 190, agent: 52, child: 20, shell: 0.45 }),
  /* 한 구면의 칸 수(피보나치 격자). */
  slots: Object.freeze({ project: 16, workspace: 32, agent: 24, child: 12 }),
  golden: Math.PI * (3 - Math.sqrt(5)),
  /* 점의 반지름(세계 단위). 워크스페이스는 에이전트 수를 따라 자라고(상한), 바닥까지 곧은
   * 기둥(`drop` 알파)과 바닥에 눌린 그림자(반지름의 `shadow`배, `shadowAlpha`)를 든다 — 공간
   * 속 높이를 말하는 단서다. 에이전트는 하위 수를 따라 자란다(상한). `hit`은 라벨의 누를
   * 자리가 점을 넘어 더 덮는 폭(px), `ringGap`은 확인 필요·실패의 테가 점에서 떨어진
   * 거리(px). */
  hub: Object.freeze({ radius: 8, perAgent: 0.7, agentCap: 10, drop: 0.3, shadow: 1.6, shadowAlpha: 0.22 }),
  agent: Object.freeze({ radius: 5.2, perChild: 1, childCap: 4, hit: 5, ringGap: 3, ringWidth: 1.4 }),
  child: Object.freeze({ radius: 3.4 }),
  /* 카메라. `distance`는 장면 반지름의 몇 배 거리에서 보는가(작을수록 원근이 세다).
   * `yaw`·`pitch`는 여는 자리(라디안)이고 pitch는 `pitchMin`..`pitchMax` 안에 선다 —
   * 카메라가 바닥 밑으로 가지 않는다. `turn`은 끈 1 px의 회전(라디안), `spin`은 가만둔
   * 판이 도는 빠르기(초당 도, `degree`로 라디안이 된다). 놓은 뒤의 미끄러짐은 끌던 마지막
   * `sampleMs`의 빠르기에서 시작해(상한 `glideMax`, 라디안/ms) `glideMs`의 시간 상수로
   * 줄고, `rest`(라디안/ms) 밑이면 선다. `near`는 이보다 가까운 것(장면 반지름 대비)을
   * 그리지 않는 앞 평면이다. 여는 판과 맞춤은 표의 yaw에서 한 바퀴를 `views`칸으로 나눠
   * 워크스페이스들이 화면에서 가장 떨어져 서는 방향을 고른다(`agentOrbitAim`). */
  camera: Object.freeze({ distance: 3.4, yaw: -0.55, pitch: 0.38, pitchMin: -0.25, pitchMax: 1.3,
    turn: 0.006, spin: 5, sampleMs: 80, glideMs: 320, glideMax: 0.003, rest: 0.00005, near: 0.2,
    views: 12 }),
  degree: Math.PI / 180,
  /* 안개: 장면의 가장 먼 쪽은 이 알파까지 옅어진다. 선은 알파를 `steps`단으로 끊어 한
   * 단을 한 경로로 긋는다 — 선마다 `stroke`를 부르지 않게. */
  fog: Object.freeze({ far: 0.3, steps: 4 }),
  /* 바닥: 장면 아래 반지름의 `drop`배에 놓인 격자, 한 변이 반지름의 `reach`배의 두 배,
   * 줄 `lines`칸, 한 줄은 안개가 먹도록 `pieces`조각. */
  grid: Object.freeze({ drop: 0.95, reach: 1.3, lines: 8, pieces: 4, width: 1, alpha: 0.26 }),
  /* 간선. 곡선(우편·의존)은 `segments`조각으로 투영하고, 조절점을 원점에서 바깥으로 두
   * 끝 거리의 `bend`배, 보낸 쪽 → 받는 쪽의 한쪽으로 `twist`배 띄운다 — 오가는 두 곡선이
   * 서로 다른 쪽으로 휜다. 구조선(메인 → 워크스페이스 → 에이전트)과 계보는 곧다. 알파는
   * 종류마다, 고른 점에 닿은 선은 `lit`. 점선은 카드 그림의 계보·의존 선과 같은 마디다. */
  edge: Object.freeze({ segments: 14, bend: 0.2, twist: 0.1, width: 1, lit: 0.62,
    structure: 0.16, spawned: 0.4, dependency: 0.72,
    dashes: Object.freeze({ spawned: Object.freeze([4, 3]), dependency: Object.freeze([7, 3]) }) }),
  /* 우편. 방금 오간(`freshMs` 안) 링크나 미확인이 있는 링크에 점이 흐르고, 점 하나가
   * 곡선을 건너는 데 `travelMs`가 든다. 미확인이 있으면 밝다. `cap`은 한 판에 흐르는
   * 점의 상한 — 넘치면 오래된 링크의 점부터 접는다. 곡선은 셋으로 옅어진다: 고른 점에
   * 닿은 것(`litAlpha`), 방금 오간 것(`linkAlpha`), 오래된 것과 검색에 흐려진 것
   * (`quietAlpha`). 점의 꼬리는 곡선 길이와 무관하게 `tailPx` 픽셀이다. */
  mail: Object.freeze({ freshMs: 600_000, travelMs: 2_600, perLink: 1, cap: 32,
    litAlpha: 0.5, linkAlpha: 0.2, quietAlpha: 0.06, readAlpha: 0.75, unreadAlpha: 1,
    radius: 1.8, unreadRadius: 2.5, glow: 4.6, tailPx: 9, tailAlpha: 0.45, tailWidth: 1.2,
    stillAt: 0.6 }),
  /* 턴 끝의 맥동 한 번: `ms` 동안 `reach`만큼 번지며 옅어진다. */
  pulse: Object.freeze({ ms: 1_400, reach: 24, width: 1.6 }),
  /* 고른 점의 테와 손이 올라간 점의 테. */
  select: Object.freeze({ gap: 4, width: 1.6, hoverAlpha: 0.55 }),
  /* 검색에 맞지 않는 점의 불투명도. */
  dim: 0.22,
  /* 점 한 장: 잉크를 칠한 원에 빛(`shine`, 반지름의 `light`배만큼 왼쪽 위로 비킨 자리에서
   * `shineReach`배까지)과 그늘(`shade`, 반지름의 `shadeFrom`배부터 가장자리까지)을 얹은
   * 구 — 원근과 함께 깊이를 말하는 단서 하나. `sprite`는 그 한 장의 크기(CSS px). */
  ball: Object.freeze({ sprite: 64, light: 0.36, shine: 0.6, shineReach: 0.9, shade: 0.55, shadeFrom: 0.5 }),
  /* 우편 점의 빛무리 한 장: 속(core)의 몫과 바깥 빛의 세기, 판의 색 구성표에 따른 세기 —
   * 흰 바탕의 빛무리는 얼룩이 된다. */
  glow: Object.freeze({ sprite: 64, core: 0.34, outer: 0.5, dark: 0.85, light: 0.28 }),
  /* 장면의 칸: 가장자리 여백, 라벨이 오른쪽으로 쓰는 폭, 맞춤 배율의 위·아래. 아래보다
   * 작은 판은 카메라를 당겨 본다. */
  fit: Object.freeze({ pad: 24, labelRoom: 108, max: 1.25, min: 0.3 }),
  /* 라벨: 자리를 다시 쓰는 문턱(px)과 반올림의 눈금, 이름이 서는 가까운 점의 수 —
   * 워크스페이스의 이름은 늘 서고, 손이 오르거나 고른 점은 CSS가 세운다. */
  label: Object.freeze({ epsilon: 0.2, round: 10, near: 10 }),
  /* 캔버스가 읽는 잉크 — `--agent-orbit-<이름>`. 워크스페이스의 빛깔은 관계 그림이
   * 매긴 레인(`laneClass`)이고, 레인이 없는 워크스페이스는 첫 레인을 입는다. */
  hubInk: "lane-1",
  inks: Object.freeze(["working", "attention", "idle", "done", "failed", "grid", "link", "dependency",
    "mail", "unread", "select", "shine", "shade", "lane-1", "lane-2", "lane-3", "lane-4", "lane-5"]),
});

const ORBIT_TURN = Math.PI * 2;
/* 가만둔 판이 도는 빠르기, 라디안/ms. */
const ORBIT_SPIN = (ORBIT.camera.spin * ORBIT.degree) / ORBIT.second;
/* 반지름 1의 구를 반지름의 `distance`배 거리에서 보면 화면에서 d/√(d²−1)배로 선다 — 맞춤은
 * 그 몫만큼 작게 앉는다. */
const ORBIT_SILHOUETTE = ORBIT.camera.distance / Math.sqrt(ORBIT.camera.distance ** 2 - 1);
const ORBIT_SOLID = Object.freeze([]);
/* 프레임마다 새 배열을 짓지 않으려고 돌려 쓰는 한 점과 한 투영. */
const ORBIT_POINT = [0, 0, 0];
const ORBIT_SEEN = { x: 0, y: 0, depth: 0, scale: 0, fog: 1, shown: true };

/* ---- 고른 보기 ------------------------------------------------------------- */

/* 움직임을 줄이라는 판인가 — 한 번 묻고 바뀔 때 듣는다. */
const agentOrbitMotion = typeof window.matchMedia === "function" ? window.matchMedia(ORBIT.reduced) : null;

function agentOrbitReduced() {
  return agentOrbitMotion?.matches === true;
}

/* 사람이 고른 보기. `null`은 아직 저장소를 읽지 않았다는 뜻이다 — 보드를 처음 그릴
 * 때 한 번 읽는다. */
let agentOrbitHeld = null;

/* 고른 적이 없는 사람의 보기: 입체. 움직임을 줄이라는 판에서는 카드 — 도는 그림이
 * 기본으로 서면 그 사람이 처음 보는 것이 그 사람이 끄라고 한 움직임이다. */
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
 * 세울지는 `dressAgentOrbitMode`가 정한다. 입체로 돌아오는 판은, 사람이 배율을 쥔
 * 적이 없으면 배율을 1로 둔다: 카드 그림의 맞춤이 내린 배율은 카드의 것이다. */
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

/* 이 판에 입체가 서는가, 그리고 그 옷. 고른 보기가 입체이고, 관계 그림이며, 그릴
 * 에이전트가 있을 때만 — 빈 판은 카드 그림의 빈 문장과 창고 띠가 답한다. 판의 클래스
 * `is-orbit`과 묻는 손 `agentOrbitShowing`은 실시간 지도·맞춤·초점이 함께 읽는 경계다. */
function dressAgentOrbitMode(view, model, taskMode) {
  const orbit = !taskMode && agentOrbitChoice() === ORBIT.orbit && (model?.agents.length ?? 0) > 0;
  view.classList.toggle("is-orbit", orbit);
  writeHidden(view.querySelector(".agent-graph-scroll"), orbit);
  writeHidden(view.querySelector(".agent-orbit"), !orbit);
  const hint = view.querySelector(".agent-graph-gesture-hint");
  if (hint) {
    writeAttribute(hint, "data-i18n", orbit ? "board.orbit.hint" : "board.graph.gestureHint");
    writeTextContent(hint, orbit
      ? t("board.orbit.hint", "점을 누르면 상세가 열립니다")
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
/* 입체를 그린 적이 있는 판들. 박자는 이 몇 장만 묻는다 — 문서 전체를 프레임마다
 * 훑지 않게. 떠난 판은 박자가 걷는다. */
const agentOrbitViews = new Set();
/* 박자 하나: 걸려 있는 rAF와 마지막으로 그린 때. */
let agentOrbitFrame = null;
let agentOrbitDrawnAt = null;
/* 시험과 보고가 읽는 수. 늘기만 한다. `agentOrbitProjections`는 장면(점·간선·바닥)을
 * 다시 투영한 횟수 — 카메라와 점이 멈춘 프레임은 세지 않는다. */
let agentOrbitApplied = 0;
let agentOrbitFrames = 0;
let agentOrbitTicks = 0;
let agentOrbitPulses = 0;
let agentOrbitInkReads = 0;
let agentOrbitProjections = 0;

/* 여는 판의 카메라: 표의 자리에서, 저절로 돈다. */
function agentOrbitCameraAtRest() {
  return { yaw: ORBIT.camera.yaw, pitch: ORBIT.camera.pitch, spin: true, turning: 0, tilting: 0, aimed: false };
}

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
    bodies: new Map(), order: [], edges: [], links: [], dots: [], slots: new Map(),
    reach: 0, fit: 1, zoom: 1, scale: 1, named: 0,
    camera: agentOrbitCameraAtRest(),
    hold: 1, holdTarget: 1,
    lens: null, strokes: null, strokesFor: null, floor: [],
    inks: null, glowAlpha: 1, sprites: new Map(),
    settling: false, pulsing: false, flowing: false,
    shownAtApply: false, dirty: true,
  };
  agentOrbitStates.set(view, state);
  agentOrbitViews.add(view);
  wireAgentOrbitStage(state);
  watchGraphResize(stage, () => agentOrbitResized(view));
  agentOrbitWatchTheme();
  return state;
}

/* 카드로 돌아간 판: 라벨과 점을 내려놓고 카메라를 표의 자리로. 다음에 서는 판은 처음
 * 서는 판이다 — 같은 입력이면 같은 자리에. */
function agentOrbitRelease(view) {
  const state = agentOrbitStates.get(view);
  if (!state || state.bodies.size === 0) return;
  state.host.replaceChildren();
  state.bodies.clear();
  state.order = [];
  state.edges = [];
  state.links = [];
  state.dots = [];
  state.slots = new Map();
  state.lens = null;
  state.strokes = null;
  state.signature = null;
  state.selected = null;
  state.hovered = null;
  state.focused = null;
  state.camera = agentOrbitCameraAtRest();
  state.hold = 1;
  state.holdTarget = 1;
}

/* 이 판의 입체가 지금 사람에게 보이는가 — 실시간 지도와 같은 손으로 묻고
 * (`agentGraphPictureOn`·`agentGraphViewStands`), 입체가 선 판인지와 무대가 자리를
 * 가졌는지를 더한다. 크기를 읽지 않는다: 크기는 관찰자가 이미 적어 두었다. */
function agentOrbitShown(state) {
  return agentGraphPictureOn() && agentGraphViewStands(state.view) && agentOrbitShowing(state.view)
    && state.width > 0 && state.height > 0;
}

/* ---- 관계 그림에서 오는 손 --------------------------------------------------- */

/* `paintAgentGraph`가 모델과 선택과 머리를 적은 뒤, 카드 그림 앞에서 부르는 문 — 입체가
 * 선 판은 그 뒤의 카드 그림을 건너뛴다 (t-9532). 서명이 움직인 판에서만 상태를 적용하고,
 * 선택만 바뀐 판은 라벨 둘과 다음 그림만 고친다. */
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

/* 배율이 움직였다(`applyAgentGraphZoom`). 카메라의 거리는 맞춤 거리 ÷ 사람의 배율이다. */
function agentOrbitZoomed(view) {
  const state = agentOrbitStates.get(view);
  /* 판마다 지나는 길이다(`paintAgentGraph`가 배율을 되살린다) — 배율이 그대로면
   * 아무것도 하지 않는다: 움직임을 줄인 판에서 정지 화면을 한 장 더 그리는 것은
   * 상태가 바뀌지 않은 그림이다. */
  if (!state || !agentOrbitShowing(view) || state.scale === state.fit * agentGraphZoom) return;
  agentOrbitLayout(state);
  agentOrbitWake();
}

/* 「전체 보기」와 두 번 누르기: 배율 1, 카메라는 여는 판의 자리(워크스페이스들이 가장
 * 떨어져 서는 방향, 표의 기울기)로 돌아가 다시 돈다. */
function agentOrbitFit(view) {
  const state = agentOrbitStates.get(view);
  setAgentGraphZoom(view, 1);
  if (!state) return;
  agentOrbitLayout(state);
  Object.assign(state.camera, { yaw: agentOrbitAim(state), pitch: ORBIT.camera.pitch, spin: true,
    turning: 0, tilting: 0, aimed: true });
  state.dirty = true;
  agentOrbitWake();
}

/* 고른 점의 라벨에 초점을 — 카드 그림의 `focusAgentGraphSelection`이 입체에서 하는 일.
 * 스크롤할 것이 없다: 입체는 판에 맞춰 선다. */
function agentOrbitFocus(view, key) {
  const state = agentOrbitStates.get(view);
  state?.bodies.get(key)?.label?.focus({ preventScroll: true });
}

/* 무대의 크기가 움직였다 — 숨었다 드러난 판도 여기로 온다(크기 0 → 제 크기).
 *
 * 실시간 지도의 떠나는 문도 이 관찰자가 지난다. 지도는 판이 숨거나 닫히는 것을 카드
 * 판의 관찰자(`watchAgentGraphSize`)가 크기 0을 들고 오는 것으로 알았는데, 입체가
 * 선 판에서 카드 판은 이미 접혀(크기 0) 있어 그 관찰자가 다시 오지 않는다 — 그러면
 * 닫힌 판에 지도의 시계와 맥박이 남고 돌아온 판이 기준선을 치르지 않는다. 보이는
 * 판이 입체일 때 그 소식을 드는 것은 이 관찰자다. */
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
  agentOrbitLayout(state);
  agentOrbitWake();
}

/* ---- 서명과 적용 ------------------------------------------------------------- */

function agentOrbitFresh(edge, now) {
  return edge.unread > 0 || now - (Number(edge.at) || 0) <= ORBIT.mail.freshMs;
}

/* 과업 의존: 관계 그림이 인스펙터에 싣는 그 목록(`agentGraphRelations`)에서 두 끝이 다
 * 에이전트인 것, 쌍마다 하나 — 선언과 작업 과업의 짝을 여기서 다시 셈하지 않는다. */
function agentOrbitDependencies(model) {
  const seen = new Set();
  return agentGraphRelations(model).filter((relation) => {
    if (relation.type !== "dependency" || !relation.from || !relation.to) return false;
    const pair = `${relation.from}\u001f${relation.to}`;
    if (seen.has(pair)) return false;
    seen.add(pair);
    return true;
  });
}

/* 이 보기가 쓰는 사실의 서명. 여기 없는 것(카드의 낱말·시계·도구 경로)은 이 보기를
 * 움직이지 않으므로, 그것만 바뀐 판은 적용을 부르지 않는다. */
function agentOrbitSignature(model, now) {
  const parts = [locale, model.searchActive ? "search" : ""];
  for (const entry of model.agents) {
    parts.push([
      entry.key, entry.state, entry.workspace?.key ?? "", entry.workspace?.label ?? "",
      entry.workspace?.laneClass ?? "", entry.card.parent ?? "", entry.searchMatch ? "match" : "",
      agentGraphIdentity(entry.card), entry.card.ledger ?? "",
    ].join("\u001f"));
  }
  for (const edge of model.overlayData?.mail ?? []) {
    parts.push([edge.key, edge.unread > 0 ? "unread" : "", agentOrbitFresh(edge, now) ? "fresh" : ""]
      .join("\u001f"));
  }
  for (const relation of agentOrbitDependencies(model)) parts.push(`${relation.from}>${relation.to}`);
  return parts.join("\u001e");
}

/* 새 점 하나. `world`는 지금 서 있는 자리, `target`은 가려는 자리, `seen`은 지난 투영. */
function agentOrbitBody(key) {
  return { key, world: [0, 0, 0], target: [0, 0, 0], size: 0, alpha: 0, label: null, pulseAt: null,
    isNew: true, placed: null, rank: null, named: null,
    seen: { x: 0, y: 0, depth: 0, scale: 0, fog: 1, shown: false } };
}

/* 모델 한 판을 점들의 목표로. 부르는 쪽은 서명이 움직였을 때만 부른다. */
function agentOrbitApply(state, model, now, shown) {
  agentOrbitApplied += 1;
  const pulsing = shown && state.shownAtApply;
  const started = performance.now();
  const entries = model.agents.filter((entry) => entry.workspace);
  const byPane = new Map(entries.map((entry) => [entry.card.pane, entry]));

  /* 누가 누구 곁에 서는가: 같은 워크스페이스의 부모가 있으면 그 부모 곁에, 아니면 제
   * 워크스페이스 둘레에. 고리처럼 이어진 계보는 끊어서 에이전트로 세운다. */
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

  /* 워크스페이스: 에이전트가 있는 워크스페이스마다 점 하나. */
  const bodies = new Map();
  for (const entry of entries) {
    const workspace = entry.workspace;
    let hub = bodies.get(workspace.key);
    if (!hub) {
      hub = state.bodies.get(workspace.key) ?? agentOrbitBody(workspace.key);
      Object.assign(hub, { kind: "workspace", workspace, entry: null, state: null, ringed: false,
        project: workspace.project?.key ?? "", main: workspace.path === workspace.project?.path,
        agents: 0, ink: workspace.laneClass || ORBIT.hubInk, name: workspace.label, dim: false, matched: false });
      bodies.set(workspace.key, hub);
    }
    hub.agents += 1;
    hub.matched ||= entry.searchMatch !== false;
  }
  for (const hub of bodies.values()) {
    hub.targetSize = ORBIT.hub.radius + ORBIT.hub.perAgent * Math.min(hub.agents, ORBIT.hub.agentCap);
    hub.dim = model.searchActive && !hub.matched;
  }

  /* 에이전트와 하위 에이전트. 이미 있던 점은 제 자리를 지키고 옷만 바꾼다. 부모가 먼저
   * 선다 — 하위의 자리는 부모의 자리에서 잰다. */
  const ordered = [...entries]
    .sort((left, right) => depthOf(left.key) - depthOf(right.key) || left.key.localeCompare(right.key));
  for (const entry of ordered) {
    const hostKey = hostOf.get(entry.key);
    const child = hostKey !== null && hostKey !== undefined;
    const known = ORBIT.states[entry.state] ?? ORBIT.states.idle;
    const body = state.bodies.get(entry.key) ?? agentOrbitBody(entry.key);
    const before = body.state;
    Object.assign(body, {
      kind: child ? "child" : "agent",
      entry, state: entry.state, ink: known.ink, ringed: known.ringed,
      hostKey: child ? hostKey : entry.workspace.key,
      name: agentGraphIdentity(entry.card),
      dim: model.searchActive && entry.searchMatch === false,
      targetSize: child ? ORBIT.child.radius
        : ORBIT.agent.radius + ORBIT.agent.perChild * Math.min((children.get(entry.key) ?? []).length,
          ORBIT.agent.childCap),
    });
    if (pulsing && before === "working" && entry.state !== "working") {
      body.pulseAt = started;
      agentOrbitPulses += 1;
    }
    bodies.set(entry.key, body);
  }
  agentOrbitPlace(state, bodies, ordered.map((entry) => bodies.get(entry.key)));

  /* 선: 구조(메인 → 워크스페이스 → 에이전트)와 계보(부모 → 하위)는 곧고, 의존은 곡선이다.
   * 다른 워크스페이스에서 시작된 하위도 계보의 선을 든다 — 그 하위는 제 워크스페이스
   * 둘레에 서므로 선이 둘을 잇는다. */
  const edges = [];
  const line = (kind, from, to, curve) => {
    if (from && to && from !== to) edges.push({ key: `${kind}:${from.key}>${to.key}`, kind, from, to, curve });
  };
  for (const body of bodies.values()) {
    if (body.kind === "workspace") line("structure", body.middle, body);
    else if (body.kind === "agent") line("structure", bodies.get(body.hostKey), body);
    else line("spawned", bodies.get(body.hostKey), body);
  }
  for (const entry of entries) {
    const parent = byPane.get(entry.card.parent);
    if (parent && hostOf.get(entry.key) === null && parent !== entry) {
      line("spawned", bodies.get(parent.key), bodies.get(entry.key));
    }
  }
  for (const relation of agentOrbitDependencies(model)) {
    line("dependency", bodies.get(relation.from), bodies.get(relation.to), []);
  }

  /* 우편: 두 끝이 다 그림에 있는 링크. 점은 제 자리(`t`)를 지킨다. */
  const heldLinks = new Map(state.links.map((link) => [link.key, link]));
  const links = [];
  for (const edge of model.overlayData?.mail ?? []) {
    const from = bodies.get(edge.from);
    const to = bodies.get(edge.to);
    if (!from || !to || from === to) continue;
    const held = heldLinks.get(edge.key);
    const fresh = agentOrbitFresh(edge, now);
    links.push({ key: edge.key, kind: "mail", from, to, curve: [], unread: edge.unread > 0, fresh,
      at: Number(edge.at) || 0, particles: fresh ? held?.particles?.length ? held.particles
        : Array.from({ length: ORBIT.mail.perLink }, (_, index) => ({
          t: ((knowledgeHash(edge.key) / ORBIT_TURN) + index / ORBIT.mail.perLink) % 1,
          seen: { ...ORBIT_SEEN }, tail: { ...ORBIT_SEEN },
        })) : [] });
  }
  /* 점의 상한: 넘치면 오래된 링크의 점부터 접는다 — 링크 자체는 그대로 선다. */
  let flowing = 0;
  for (const link of [...links].sort((left, right) => right.at - left.at)) {
    if (flowing + link.particles.length > ORBIT.mail.cap) link.particles = [];
    flowing += link.particles.length;
  }

  state.bodies = bodies;
  state.edges = edges;
  state.links = links;
  state.flowing = flowing > 0;
  state.lens = null;
  state.strokes = null;
  agentOrbitLabels(state);
  agentOrbitLayout(state);
  if (!state.camera.aimed) {
    state.camera.yaw = agentOrbitAim(state);
    state.camera.aimed = true;
  }

  /* 캔버스가 말하는 요약 — 그림을 볼 수 없는 사람에게 같은 수를. */
  const working = entries.filter((entry) => entry.state === "working").length;
  const attention = entries.filter((entry) => entry.state === "needs-attention").length;
  writeAttribute(state.canvas, "aria-label", t("board.orbit.summary",
    "에이전트 {{count}}개 · 작업 중 {{working}} · 확인 필요 {{attention}}",
    { count: model.agents.length, working, attention }));
  state.dirty = true;
}

/* ---- 자리 -------------------------------------------------------------------- */

/* 피보나치 구면의 한 칸: `count`칸 중 `index`번째 칸의 단위 방향. 칸들은 구면을 거의
 * 고르게 덮는다 — 위아래(y)를 고르게 끊고, 둘레는 황금각씩 돈다. */
function agentOrbitLattice(index, count) {
  const y = 1 - (2 * index + 1) / count;
  const ring = Math.sqrt(1 - y * y);
  const angle = index * ORBIT.golden;
  return [Math.cos(angle) * ring, y, Math.sin(angle) * ring];
}

/* 새 점이 앉을 칸. 칸 번호는 겹을 넘어 이어진다(`count`마다 한 겹 바깥). 비어 있는 칸이
 * 남은 가장 안쪽 겹에서, 그 겹에 이미 앉은 점들과 가장 먼 칸 — 처음 선 판의 점들이
 * 한 곳에 몰리지 않는다. 겹에 앉은 점이 없으면 키의 해시가 고른 칸이고, 거리가 같으면
 * 해시에서 가까운 칸이다. */
function agentOrbitSlot(taken, count, seed) {
  const home = knowledgeHash(seed) % count;
  for (let base = 0; ; base += count) {
    const mine = [...taken].filter((slot) => slot >= base && slot < base + count)
      .map((slot) => agentOrbitLattice(slot - base, count));
    if (mine.length >= count) continue;
    if (mine.length === 0) return base + home;
    let best = base + home;
    let widest = -1;
    for (let step = 0; step < count; step += 1) {
      const index = (home + step) % count;
      if (taken.has(base + index)) continue;
      const [x, y, z] = agentOrbitLattice(index, count);
      let nearest = Infinity;
      for (const there of mine) nearest = Math.min(nearest, Math.hypot(x - there[0], y - there[1], z - there[2]));
      if (nearest > widest) {
        widest = nearest;
        best = base + index;
      }
    }
    return best;
  }
}

/* 한 중심 둘레의 칸들: 이미 앉은 키는 제 칸을 지키고, 떠난 키의 칸은 비며, 새 키는
 * 정렬된 차례로 빈 칸을 받는다. 판에 남은 중심만 다음 판으로 간다. */
function agentOrbitSeat(held, next, center, keys, count) {
  const before = held.get(center) ?? new Map();
  const seats = new Map();
  for (const key of keys) {
    if (before.has(key)) seats.set(key, before.get(key));
  }
  const taken = new Set(seats.values());
  for (const key of [...keys].sort()) {
    if (seats.has(key)) continue;
    const slot = agentOrbitSlot(taken, count, `${center}\u001f${key}`);
    seats.set(key, slot);
    taken.add(slot);
  }
  next.set(center, seats);
  return seats;
}

/* 칸 하나의 자리: 중심에서 칸의 방향으로, 겹마다 반지름의 `shell`배 더 바깥. */
function agentOrbitSpot(center, slot, count, radius) {
  const [x, y, z] = agentOrbitLattice(slot % count, count);
  const reach = radius * (1 + Math.floor(slot / count) * ORBIT.space.shell);
  return [center[0] + x * reach, center[1] + y * reach, center[2] + z * reach];
}

/* 칸들 중 가장 바깥 겹의 반지름. */
function agentOrbitShellReach(seats, count, radius) {
  const outer = Math.max(0, ...[...seats.values()].map((slot) => Math.floor(slot / count)));
  return radius * (1 + outer * ORBIT.space.shell);
}

/* 점들을 공간에 앉힌다. 프로젝트가 하나면 그 중심이 원점이고, 둘 이상이면 프로젝트의
 * 중심들이 원점 둘레의 구면에 선다. 한 프로젝트의 가운데는 메인 워크스페이스(경로가
 * 프로젝트의 경로인 것) — 메인이 그림에 없으면 키가 앞선 워크스페이스다. 장면의 반지름
 * (`reach`)은 쓰인 구면의 반지름을 더한 것이라 에이전트 하나가 와도 겹이 늘지 않는 한
 * 그대로다 — 맞춤도 카메라도 그대로다. 처음 선 점은 제 자리에서 나타난다. */
function agentOrbitPlace(state, bodies, agents) {
  const next = new Map();
  const hubs = [...bodies.values()].filter((body) => body.kind === "workspace")
    .sort((left, right) => left.key.localeCompare(right.key));
  const projects = [...new Set(hubs.map((hub) => hub.project))].sort();
  const spread = projects.length > 1;
  const projectSeats = agentOrbitSeat(state.slots, next, "", projects, ORBIT.slots.project);
  let reach = spread ? agentOrbitShellReach(projectSeats, ORBIT.slots.project, ORBIT.space.project) : 0;
  let around = 0;
  for (const project of projects) {
    const center = spread
      ? agentOrbitSpot([0, 0, 0], projectSeats.get(project), ORBIT.slots.project, ORBIT.space.project)
      : [0, 0, 0];
    const mine = hubs.filter((hub) => hub.project === project);
    const middle = mine.find((hub) => hub.main) ?? mine[0];
    const others = mine.filter((hub) => hub !== middle);
    const seats = agentOrbitSeat(state.slots, next, project, others.map((hub) => hub.key), ORBIT.slots.workspace);
    middle.target = center;
    middle.middle = null;
    for (const hub of others) {
      hub.target = agentOrbitSpot(center, seats.get(hub.key), ORBIT.slots.workspace, ORBIT.space.workspace);
      hub.middle = middle;
    }
    if (others.length > 0) {
      around = Math.max(around, agentOrbitShellReach(seats, ORBIT.slots.workspace, ORBIT.space.workspace));
    }
  }
  reach += around;
  /* 에이전트는 제 워크스페이스 둘레에, 하위는 부모 둘레에 — 부모가 먼저 앉는다. */
  const circles = new Map();
  for (const body of agents) {
    const key = body.hostKey;
    circles.set(key, [...(circles.get(key) ?? []), body]);
  }
  let agentReach = 0;
  let childReach = 0;
  for (const body of agents) {
    const host = bodies.get(body.hostKey);
    const kind = body.kind === "child" ? "child" : "agent";
    const seats = next.get(host.key) ?? agentOrbitSeat(state.slots, next, host.key,
      circles.get(host.key).map((one) => one.key), ORBIT.slots[kind]);
    body.target = agentOrbitSpot(host.target, seats.get(body.key), ORBIT.slots[kind], ORBIT.space[kind]);
    const shell = agentOrbitShellReach(seats, ORBIT.slots[kind], ORBIT.space[kind]);
    if (kind === "child") childReach = Math.max(childReach, shell);
    else agentReach = Math.max(agentReach, shell);
  }
  state.slots = next;
  state.reach = reach + agentReach + childReach;
  for (const body of bodies.values()) {
    if (!body.isNew) continue;
    body.world = [...body.target];
    body.size = body.targetSize;
    body.isNew = false;
  }
}

/* 맞춤: 장면의 구가 판(라벨의 폭과 여백을 뺀 자리)에 드는 배율, 위·아래로 묶어. 카메라의
 * 거리는 이 배율과 사람의 배율에서 나온다(`agentOrbitLens`). 누를 자리의 반지름도 여기서
 * 적는다 — 프레임은 적지 않는다. */
function agentOrbitLayout(state) {
  const room = Math.min((state.width - ORBIT.fit.labelRoom) / 2, state.height / 2) - ORBIT.fit.pad;
  const fit = state.reach > 0 ? room / (state.reach * ORBIT_SILHOUETTE) : ORBIT.fit.max;
  state.fit = Math.max(ORBIT.fit.min, Math.min(ORBIT.fit.max, fit || ORBIT.fit.max));
  state.zoom = agentGraphZoom;
  state.scale = state.fit * agentGraphZoom;
  for (const body of state.bodies.values()) {
    if (body.label) {
      writeStyleProperty(body.label, "--orbit-body",
        `${Math.round((body.targetSize * state.scale + ORBIT.agent.hit) * ORBIT.label.round) / ORBIT.label.round}px`);
    }
  }
  state.dirty = true;
}

/* ---- 라벨 -------------------------------------------------------------------- */

/* 라벨 하나: 점을 덮는 누를 자리 + 이름, 상태 칩은 `::after`가 `data-chip`을 읽는다.
 * 요소가 둘뿐인 것은 이 그림의 DOM 예산 때문이다(카드 그림 대비 +50 이하). 모든 점이
 * 라벨을 가진다 — 이름이 접힌 먼 점도 누르고, 손을 올리고, 탭으로 온다. */
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
  for (const body of state.bodies.values()) {
    const held = existing.get(body.key);
    const label = held ?? agentOrbitLabelNode();
    if (body.kind === "workspace") {
      const count = agentGraphCountWord("agent", body.agents);
      writeClassName(label, ["agent-orbit-label", "is-workspace", body.ink, body.dim ? "is-search-dimmed" : ""]
        .filter(Boolean).join(" "));
      writeAttribute(label, "data-chip", count);
      writeAttribute(label, "aria-label", `${body.name}, ${count}`);
      writeAttribute(label, "data-tip", `${t("board.graph.workspace", "워크스페이스")} · ${body.name} · ${count}`);
    } else {
      const word = agentGraphStateWord(body.state, body.entry.card.ledger ?? "");
      writeClassName(label, ["agent-orbit-label", `is-${body.kind}`, `is-${body.state}`,
        body.dim ? "is-search-dimmed" : ""].filter(Boolean).join(" "));
      writeAttribute(label, "data-chip", word);
      writeAttribute(label, "aria-label", `${body.name}, ${word}`);
      writeAttribute(label, "data-tip", [body.name, word, body.entry.workspace?.label].filter(Boolean).join(" · "));
    }
    writeAttribute(label, "data-orbit-key", body.key);
    writeTextContent(label.firstElementChild, body.name);
    /* 새 라벨은 자리·깊이·이름을 처음 적는다; 이어 쓰는 라벨은 제 값을 그대로 든다. */
    if (body.label !== label || !held) {
      body.placed = null;
      body.rank = null;
      body.named = null;
    }
    body.label = label;
    wanted.push(label);
  }
  reconcileElementOrder(state.host, wanted);
  state.selected = null;
}

/* 고른 점: 라벨의 `aria-pressed`와 다음 그림의 테. 바뀐 판에서만 쓴다. */
function agentOrbitDressSelection(state) {
  if (state.selected === agentGraphSelectedKey) return;
  for (const key of [state.selected, agentGraphSelectedKey]) {
    const label = state.bodies.get(key)?.label;
    if (label) writeAttribute(label, "aria-pressed", String(key === agentGraphSelectedKey));
  }
  for (const label of state.host.children) {
    if (!label.hasAttribute("aria-pressed")) writeAttribute(label, "aria-pressed", "false");
  }
  state.selected = agentGraphSelectedKey;
  state.dirty = true;
}

/* 무대의 손들, 한 번. 라벨을 누르면 관계 그림의 선택 길로, 손이 오르거나 초점이 들면
 * 카메라와 우편이 멈춘다(움직이는 과녁은 누르기 어렵다). 빈 곳을 끌면 카메라가 돌고
 * (가로가 yaw, 세로가 pitch), 놓으면 끌던 빠르기로 미끄러져 선다. 빈 곳을 두 번 누르면
 * 맞춤, ⌘/Ctrl 휠은 관계 그림의 배율(카메라의 거리)을 움직인다. 끌기와 누르기를 가르는
 * 문턱은 관계 그림들이 함께 쓰는 끌기의 것이다(`wireGraphDrag`). */
function wireAgentOrbitStage(state) {
  const { view, stage, host } = state;
  const keyOf = (node) => (node instanceof Element ? node.closest("[data-orbit-key]") : null);
  /* 손이 오른 점이 바뀌면 그 테를 한 장 다시 그린다 — 멈춘 판에서도. */
  const hold = (hovered) => {
    if (hovered !== undefined && hovered !== state.hovered) {
      state.hovered = hovered;
      state.dirty = true;
    }
    state.holdTarget = state.hovered || state.focused ? 0 : 1;
    agentOrbitWake();
  };
  host.onclick = (event) => {
    const label = keyOf(event.target);
    if (label) selectAgentGraphEntity(view, label.dataset.orbitKey);
  };
  host.onpointerover = (event) => hold(keyOf(event.target)?.dataset.orbitKey ?? null);
  host.onpointerout = (event) => {
    if (keyOf(event.relatedTarget)) return;
    hold(null);
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
  let grab = null;
  wireGraphDrag(stage, {
    /* 스페이스를 쥔 손은 라벨 위에서도 카메라를 돌린다 — 카드 그림과 같은 몸짓. */
    grabbed: () => agentGraphSpaceHeld,
    onGrab: () => {
      grab = { yaw: state.camera.yaw, pitch: state.camera.pitch, marks: [] };
    },
    onDrag: (moveX, moveY) => {
      if (!grab) return;
      agentGraphPauseFollow(view);
      if (!stage.classList.contains("is-panning")) stage.classList.add("is-panning");
      const camera = state.camera;
      Object.assign(camera, { spin: false, turning: 0, tilting: 0,
        yaw: grab.yaw + moveX * ORBIT.camera.turn, pitch: agentOrbitPitch(grab.pitch + moveY * ORBIT.camera.turn) });
      const now = performance.now();
      grab.marks.push([now, camera.yaw, camera.pitch]);
      while (grab.marks.length > 1 && now - grab.marks[0][0] > ORBIT.camera.sampleMs) grab.marks.shift();
      state.dirty = true;
      agentOrbitWake();
    },
    onEnd: (turned) => {
      stage.classList.remove("is-panning");
      const marks = grab?.marks ?? [];
      grab = null;
      if (!turned || marks.length < 2 || agentOrbitReduced()) return;
      const [from, to] = [marks[0], marks[marks.length - 1]];
      const span = to[0] - from[0];
      if (span <= 0) return;
      const cap = ORBIT.camera.glideMax;
      state.camera.turning = Math.max(-cap, Math.min(cap, (to[1] - from[1]) / span));
      state.camera.tilting = Math.max(-cap, Math.min(cap, (to[2] - from[2]) / span));
      agentOrbitWake();
    },
  });
  stage.ondblclick = (event) => {
    if (keyOf(event.target)) return;
    agentOrbitFit(view);
  };
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

/* 우편 점의 빛무리 한 장, 잉크마다. 두 겹의 방사 그라디언트(넓고 옅은 것, 좁고 진한 것)를
 * 그린 뒤 그 알파만 남기고 잉크로 칠한다 — 색을 문자열로 쪼개지 않는다. */
function agentOrbitSprite(state, name) {
  const held = state.sprites.get(`glow:${name}`);
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
  state.sprites.set(`glow:${name}`, sprite);
  return sprite;
}

/* 점 한 장, 잉크마다: 잉크를 칠한 원 위에(`source-atop` — 원 밖은 칠하지 않는다) 왼쪽
 * 위에서 비친 빛과 가장자리의 그늘. 두 그라디언트는 잉크에서 끝나거나 잉크에서 시작하므로
 * 투명과 섞이는 테두리가 없다. */
function agentOrbitBall(state, name) {
  const held = state.sprites.get(`ball:${name}`);
  if (held) return held;
  const size = Math.max(1, Math.round(ORBIT.ball.sprite * state.dpr));
  const sprite = document.createElement("canvas");
  sprite.width = size;
  sprite.height = size;
  const paint = sprite.getContext("2d");
  const middle = size / 2;
  const { inks } = state;
  paint.fillStyle = inks[name];
  paint.beginPath();
  paint.arc(middle, middle, middle, 0, ORBIT_TURN);
  paint.fill();
  paint.globalCompositeOperation = "source-atop";
  const lit = middle - middle * ORBIT.ball.light;
  const shine = paint.createRadialGradient(lit, lit, 0, lit, lit, middle * ORBIT.ball.shineReach);
  shine.addColorStop(0, inks.shine);
  shine.addColorStop(1, inks[name]);
  paint.globalAlpha = ORBIT.ball.shine;
  paint.fillStyle = shine;
  paint.fillRect(0, 0, size, size);
  const shade = paint.createRadialGradient(middle, middle, middle * ORBIT.ball.shadeFrom, middle, middle, middle);
  shade.addColorStop(0, inks[name]);
  shade.addColorStop(1, inks.shade);
  paint.globalAlpha = ORBIT.ball.shade;
  paint.fillStyle = shade;
  paint.fillRect(0, 0, size, size);
  state.sprites.set(`ball:${name}`, sprite);
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

/* 이 판에 다음 그림이 필요한가: 새로 그릴 것(옷·크기·선택)이 있거나, 점이 아직 미끄러지고
 * 맥동이 번지거나, 손이 떠나며 다시 움직이는 중이거나, 카메라가 미끄러지거나 — 손이 없는
 * 판에서 카메라가 저절로 돌거나 우편이 흐를 때. 어느 것도 아니면 박자는 서고 rAF는 0이다. */
function agentOrbitMoving(state) {
  const camera = state.camera;
  return state.dirty || state.settling || state.pulsing || state.hold !== state.holdTarget
    || camera.turning !== 0 || camera.tilting !== 0 || (state.hold > 0 && (camera.spin || state.flowing));
}

/* 보이는 판마다: 움직일 판이면 박자를 걸고, 움직임을 줄인 판이면 바뀐 것이 있을
 * 때만 정지 화면 한 장. 어느 쪽도 보이지 않는 판에는 아무것도 하지 않는다. */
function agentOrbitWake() {
  const reduced = agentOrbitReduced();
  let moving = false;
  for (const view of agentOrbitViews) {
    const state = agentOrbitStates.get(view);
    if (!state || !agentOrbitShown(state)) continue;
    if (!reduced) {
      moving ||= agentOrbitMoving(state);
      continue;
    }
    if (state.dirty) agentOrbitDraw(state, 0, performance.now(), { still: true });
  }
  if (moving && agentOrbitFrame === null) agentOrbitFrame = requestAnimationFrame(agentOrbitTick);
}

/* 박자 하나. 보이는 판이 없거나 움직일 판이 없으면 다음 rAF를 걸지 않는다 — 숨은 판과
 * 멈춘 판의 rAF는 0이다. 초점이 없는 창은 박자를 건너뛰어 초당 30장까지만 그린다. */
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
  const rate = document.hasFocus() ? ORBIT.fps.focused : ORBIT.fps.blurred;
  const gap = ORBIT.second / rate - ORBIT.fps.slackMs;
  if (shown.length > 0 && (agentOrbitDrawnAt === null || stamp - agentOrbitDrawnAt >= gap)) {
    const step = agentOrbitDrawnAt === null ? 0 : Math.min(ORBIT.stepCapMs, stamp - agentOrbitDrawnAt);
    agentOrbitDrawnAt = stamp;
    for (const state of shown) agentOrbitDraw(state, step, stamp);
  }
  if (!shown.some(agentOrbitMoving)) {
    agentOrbitDrawnAt = null;
    return;
  }
  agentOrbitFrame = requestAnimationFrame(agentOrbitTick);
}

/* ---- 카메라와 투영 ----------------------------------------------------------- */

function agentOrbitPitch(pitch) {
  return Math.max(ORBIT.camera.pitchMin, Math.min(ORBIT.camera.pitchMax, pitch));
}

/* 카메라를 한 걸음: 가만둔 판은 돌고, 놓은 판은 미끄러진다. 손이 오른 판(`sim` 0)에서는
 * 둘 다 서지만, 미끄러짐의 빠르기는 흐른 시간으로 줄어 손이 떠나도 되살아나지 않는다. */
function agentOrbitTurn(state, step, sim) {
  const camera = state.camera;
  if (camera.spin) camera.yaw += sim * ORBIT_SPIN;
  if (camera.turning === 0 && camera.tilting === 0) return;
  camera.yaw += camera.turning * sim;
  const pitch = agentOrbitPitch(camera.pitch + camera.tilting * sim);
  if (pitch !== camera.pitch + camera.tilting * sim) camera.tilting = 0;
  camera.pitch = pitch;
  const decay = Math.exp(-step / ORBIT.camera.glideMs);
  camera.turning *= decay;
  camera.tilting *= decay;
  if (Math.abs(camera.turning) < ORBIT.camera.rest) camera.turning = 0;
  if (Math.abs(camera.tilting) < ORBIT.camera.rest) camera.tilting = 0;
}

/* 이 프레임의 렌즈: 화면의 가운데(라벨의 폭만큼 왼쪽), 카메라의 거리(장면 반지름 ×
 * `distance` ÷ 사람의 배율), 초점 거리(원점 깊이의 배율이 `scale`이 되는 값), 앞 평면,
 * 안개가 재는 깊이의 앞과 폭, 그리고 카메라의 두 각의 사인·코사인. */
function agentOrbitLens(state, { yaw, pitch } = state.camera) {
  const distance = (state.reach * ORBIT.camera.distance) / state.zoom;
  return {
    yaw, pitch,
    cx: (state.width - ORBIT.fit.labelRoom) / 2, cy: state.height / 2,
    distance, focal: state.scale * distance, near: state.reach * ORBIT.camera.near,
    front: distance - state.reach, span: state.reach * 2,
    cosYaw: Math.cos(yaw), sinYaw: Math.sin(yaw), cosPitch: Math.cos(pitch), sinPitch: Math.sin(pitch),
  };
}

/* 여는 방향: 표의 yaw에서 한 바퀴를 `views`칸으로 나눈 방향들 중, 워크스페이스 둘이 화면에서
 * 가장 가까이 선 거리가 가장 먼 방향 — 두 워크스페이스가 시선 위에 겹친 판으로 열리지
 * 않게. 같은 자리면 같은 방향이고, 워크스페이스가 둘 미만이면 표의 yaw다. */
function agentOrbitAim(state) {
  const hubs = [...state.bodies.values()].filter((body) => body.kind === "workspace");
  let best = ORBIT.camera.yaw;
  if (hubs.length < 2 || state.reach <= 0) return best;
  let widest = -1;
  const seen = hubs.map(() => ({ ...ORBIT_SEEN }));
  for (let view = 0; view < ORBIT.camera.views; view += 1) {
    const yaw = ORBIT.camera.yaw + (view * ORBIT_TURN) / ORBIT.camera.views;
    const lens = agentOrbitLens(state, { yaw, pitch: ORBIT.camera.pitch });
    hubs.forEach((hub, at) => agentOrbitProject(lens, hub.target[0], hub.target[1], hub.target[2], seen[at]));
    let nearest = Infinity;
    for (let one = 0; one < seen.length; one += 1) {
      for (let two = one + 1; two < seen.length; two += 1) {
        nearest = Math.min(nearest, Math.hypot(seen[one].x - seen[two].x, seen[one].y - seen[two].y));
      }
    }
    if (nearest > widest) {
      widest = nearest;
      best = yaw;
    }
  }
  return best;
}

function agentOrbitSameLens(left, right) {
  return left !== null && left.yaw === right.yaw && left.pitch === right.pitch && left.distance === right.distance
    && left.focal === right.focal && left.cx === right.cx && left.cy === right.cy;
}

/* 세계의 한 점을 이 프레임의 카메라로. 세로축(y) 둘레로 yaw, 가로축(x) 둘레로 pitch만큼
 * 돌린 뒤 원근: 깊이는 카메라에서 잰 거리이고, 그 깊이의 배율은 초점 거리 ÷ 깊이다 — 가까운
 * 점이 크게 선다. 안개는 장면의 앞에서 뒤까지 알파를 `fog.far`까지 줄인다. 앞 평면보다
 * 가까우면 `shown`이 거짓이다. `into`에 적어 돌려준다(프레임마다 새 객체를 짓지 않게). */
function agentOrbitProject(lens, x, y, z, into) {
  const turnedX = x * lens.cosYaw - z * lens.sinYaw;
  const turnedZ = x * lens.sinYaw + z * lens.cosYaw;
  const tiltedY = y * lens.cosPitch + turnedZ * lens.sinPitch;
  const tiltedZ = turnedZ * lens.cosPitch - y * lens.sinPitch;
  const depth = lens.distance - tiltedZ;
  const shown = depth > lens.near;
  const scale = shown ? lens.focal / depth : 0;
  into.x = lens.cx + turnedX * scale;
  into.y = lens.cy + tiltedY * scale;
  into.depth = depth;
  into.scale = scale;
  into.fog = agentOrbitFog(lens, depth);
  into.shown = shown;
  return into;
}

/* 한 깊이의 안개: 장면의 앞(1)에서 뒤(`fog.far`)까지 곧게. */
function agentOrbitFog(lens, depth) {
  return 1 - (1 - ORBIT.fog.far) * Math.min(1, Math.max(0, (depth - lens.front) / lens.span));
}

/* 곡선의 조절점: 두 끝의 가운데에서 원점 바깥쪽으로(가운데가 원점이면 위로) 두 끝 거리의
 * `bend`배, 그리고 두 끝을 잇는 방향과 바깥 방향 둘 다에 수직인 쪽으로 `twist`배 — 보낸
 * 쪽과 받는 쪽이 바뀌면 그 쪽이 뒤집혀 오가는 두 곡선이 겹치지 않는다. */
function agentOrbitBend(edge) {
  const from = edge.from.world;
  const to = edge.to.world;
  const middle = [0, 1, 2].map((axis) => (from[axis] + to[axis]) / 2);
  const chord = [0, 1, 2].map((axis) => to[axis] - from[axis]);
  const span = Math.hypot(...chord);
  const away = Math.hypot(...middle);
  const out = away > ORBIT.rest ? middle.map((axis) => axis / away) : [0, -1, 0];
  const side = [chord[1] * out[2] - chord[2] * out[1], chord[2] * out[0] - chord[0] * out[2],
    chord[0] * out[1] - chord[1] * out[0]];
  const sideLength = Math.hypot(...side) || 1;
  edge.curve[0] = from;
  edge.curve[1] = [0, 1, 2].map((axis) => middle[axis] + out[axis] * span * ORBIT.edge.bend
    + (side[axis] / sideLength) * span * ORBIT.edge.twist);
  edge.curve[2] = to;
}

/* 이차 곡선 위의 한 점(`at` 0 → 1), `into`에. */
function agentOrbitAlong(curve, at, into) {
  const rest = 1 - at;
  for (let axis = 0; axis < into.length; axis += 1) {
    into[axis] = rest * rest * curve[0][axis] + 2 * rest * at * curve[1][axis] + at * at * curve[2][axis];
  }
  return into;
}

/* 선 하나를 화면의 꺾은선으로: 곧은 선은 두 끝, 곡선은 `segments`조각. 안개는 조각들의
 * 평균 깊이의 것, 길이는 화면에서 잰 길이(우편 점의 꼬리가 쓴다). */
function agentOrbitTrace(edge, lens) {
  const pieces = edge.curve ? ORBIT.edge.segments : 1;
  if (edge.curve) agentOrbitBend(edge);
  edge.points ??= new Float64Array((pieces + 1) * 2);
  let depth = 0;
  let length = 0;
  let shown = true;
  for (let piece = 0; piece <= pieces; piece += 1) {
    const at = piece / pieces;
    if (edge.curve) agentOrbitAlong(edge.curve, at, ORBIT_POINT);
    else {
      for (let axis = 0; axis < ORBIT_POINT.length; axis += 1) {
        ORBIT_POINT[axis] = edge.from.world[axis] + (edge.to.world[axis] - edge.from.world[axis]) * at;
      }
    }
    const seen = agentOrbitProject(lens, ORBIT_POINT[0], ORBIT_POINT[1], ORBIT_POINT[2], ORBIT_SEEN);
    shown &&= seen.shown;
    depth += seen.depth;
    if (piece > 0) {
      length += Math.hypot(seen.x - edge.points[piece * 2 - 2], seen.y - edge.points[piece * 2 - 1]);
    }
    edge.points[piece * 2] = seen.x;
    edge.points[piece * 2 + 1] = seen.y;
  }
  edge.shown = shown;
  edge.length = Math.max(1, length);
  edge.fog = agentOrbitFog(lens, depth / (pieces + 1));
}

/* 알파를 안개의 단으로 — 같은 단의 선은 한 경로다. */
function agentOrbitFogStep(fog) {
  return Math.round(fog * ORBIT.fog.steps) / ORBIT.fog.steps;
}

/* 묶음에 꺾은선 하나를 얹는다. 묶음은 잉크·알파·점선·굵기가 같은 선들이고, 그리는 손이
 * 한 경로로 긋는다(`agentOrbitDraw`) — 선마다 `stroke`를 부르지 않게. */
function agentOrbitStroke(groups, ink, alpha, dash, width, points) {
  const key = `${ink}\u001f${alpha}\u001f${dash}\u001f${width}`;
  let group = groups.get(key);
  if (!group) {
    group = { ink, alpha, width, dash: ORBIT.edge.dashes[dash] ?? ORBIT_SOLID, lines: [] };
    groups.set(key, group);
  }
  group.lines.push(points);
}

/* 바닥의 격자를 이 렌즈로: 줄마다 `pieces`조각, 조각마다 제 깊이의 안개 단. 판의 선
 * 묶음 몇 개(`state.floor`)가 된다 — 카메라가 멈춘 프레임은 그대로 긋는다. */
function agentOrbitFloor(state, lens) {
  const groups = new Map();
  const floor = state.reach * ORBIT.grid.drop;
  const half = state.reach * ORBIT.grid.reach;
  const { lines, pieces } = ORBIT.grid;
  for (let row = 0; row <= lines; row += 1) {
    const across = -half + (half * 2 * row) / lines;
    for (let way = 0; way < 2; way += 1) {
      for (let part = 0; part < pieces; part += 1) {
        const piece = new Float64Array(2 * 2);
        let fog = 0;
        let shown = true;
        for (let end = 0; end < 2; end += 1) {
          const along = -half + (half * 2 * (part + end)) / pieces;
          const seen = way === 0 ? agentOrbitProject(lens, across, floor, along, ORBIT_SEEN)
            : agentOrbitProject(lens, along, floor, across, ORBIT_SEEN);
          shown &&= seen.shown;
          fog += seen.fog / 2;
          piece[end * 2] = seen.x;
          piece[end * 2 + 1] = seen.y;
        }
        if (shown) agentOrbitStroke(groups, "grid", ORBIT.grid.alpha * agentOrbitFogStep(fog), "", ORBIT.grid.width, piece);
      }
    }
  }
  /* 워크스페이스의 기둥: 점에서 바닥까지 곧게, 바닥의 선과 같은 잉크로. */
  for (const hub of state.bodies.values()) {
    if (hub.kind !== "workspace") continue;
    const top = agentOrbitProject(lens, hub.world[0], hub.world[1], hub.world[2], ORBIT_SEEN);
    const [x, y, shown] = [top.x, top.y, top.shown];
    const foot = agentOrbitProject(lens, hub.world[0], floor, hub.world[2], hub.foot ??= { ...ORBIT_SEEN });
    if (shown && foot.shown) {
      agentOrbitStroke(groups, "grid", ORBIT.hub.drop * agentOrbitFogStep(foot.fog), "", ORBIT.grid.width,
        new Float64Array([x, y, foot.x, foot.y]));
    }
  }
  state.floor = [...groups.values()];
}

/* 장면을 다시 투영한다 — 카메라나 점이 움직인 프레임에서만. 점은 제 투영을 들고, 깊은
 * 것부터 선다(`order`, 화가의 순서); 선은 꺾은선을, 바닥은 경로를 다시 짓는다. */
function agentOrbitProjectScene(state, lens) {
  agentOrbitProjections += 1;
  for (const body of state.bodies.values()) {
    agentOrbitProject(lens, body.world[0], body.world[1], body.world[2], body.seen);
  }
  state.order = [...state.bodies.values()].sort((left, right) => right.seen.depth - left.seen.depth);
  for (const edge of state.edges) agentOrbitTrace(edge, lens);
  for (const link of state.links) agentOrbitTrace(link, lens);
  agentOrbitFloor(state, lens);
  state.lens = lens;
  state.strokes = null;
}

/* 선들의 묶음: 종류·고른 점·검색·신선함·안개 단이 같은 선은 한 경로다. 투영이나 선택이
 * 바뀐 프레임에서만 짓는다. */
function agentOrbitStrokes(state) {
  const groups = new Map();
  const selected = state.selected;
  for (const edge of state.edges) {
    if (!edge.shown) continue;
    const lit = edge.from.key === selected || edge.to.key === selected;
    const dim = edge.from.dim || edge.to.dim ? ORBIT.dim : 1;
    const alpha = lit ? ORBIT.edge.lit : ORBIT.edge[edge.kind] * dim;
    agentOrbitStroke(groups, edge.kind === "dependency" ? "dependency" : "link",
      alpha * agentOrbitFogStep(edge.fog), edge.kind, ORBIT.edge.width, edge.points);
  }
  for (const link of state.links) {
    if (!link.shown) continue;
    const lit = link.from.key === selected || link.to.key === selected;
    const [ink, alpha] = lit ? ["mail", ORBIT.mail.litAlpha]
      : link.fresh && !link.from.dim && !link.to.dim ? ["link", ORBIT.mail.linkAlpha] : ["link", ORBIT.mail.quietAlpha];
    agentOrbitStroke(groups, ink, alpha * agentOrbitFogStep(link.fog), "", ORBIT.edge.width, link.points);
  }
  state.strokes = [...groups.values()];
  state.strokesFor = selected;
}

/* 라벨을 투영에 맞춘다 — 다시 투영한 프레임에서만, 값이 움직인 것만. 쓰는 것은 라벨의
 * style 셋뿐이다: 자리(`translate`), 깊이의 차례(`--orbit-depth`, 가까운 것이 위),
 * 이름이 서는가(`--orbit-name`). 이름은 워크스페이스와 가까운 `near`개의 점에 선다. */
function agentOrbitPlaceLabels(state) {
  let near = 0;
  let named = 0;
  for (let rank = state.order.length - 1; rank >= 0; rank -= 1) {
    const body = state.order[rank];
    const label = body.label;
    if (!label) continue;
    const seen = body.seen;
    const x = Math.round(seen.x * ORBIT.label.round) / ORBIT.label.round;
    const y = Math.round(seen.y * ORBIT.label.round) / ORBIT.label.round;
    const placed = body.placed;
    if (!placed || Math.abs(placed[0] - x) >= ORBIT.label.epsilon || Math.abs(placed[1] - y) >= ORBIT.label.epsilon) {
      body.placed = [x, y];
      label.style.translate = `${x}px ${y}px`;
    }
    if (body.rank !== rank) {
      body.rank = rank;
      label.style.setProperty("--orbit-depth", String(rank));
    }
    const shows = body.kind === "workspace" || near < ORBIT.label.near;
    if (body.kind !== "workspace" && shows) near += 1;
    if (shows) named += 1;
    const name = shows ? "1" : "0";
    if (body.named !== name) {
      body.named = name;
      label.style.setProperty("--orbit-name", name);
    }
  }
  state.named = named;
}

/* 점들을 목표로 한 걸음. 자리가 움직였는지 돌려준다 — 움직인 프레임만 다시 투영한다. */
function agentOrbitSettle(state, settle, appear) {
  let moved = false;
  let settling = false;
  for (const body of state.bodies.values()) {
    for (let axis = 0; axis < body.world.length; axis += 1) {
      const gap = body.target[axis] - body.world[axis];
      if (gap === 0) continue;
      moved = true;
      body.world[axis] = Math.abs(gap) <= ORBIT.rest ? body.target[axis] : body.world[axis] + gap * settle;
      settling ||= body.world[axis] !== body.target[axis];
    }
    const grow = body.targetSize - body.size;
    body.size = Math.abs(grow) <= ORBIT.rest ? body.targetSize : body.size + grow * settle;
    body.alpha = 1 - body.alpha <= ORBIT.fade ? 1 : body.alpha + (1 - body.alpha) * appear;
    settling ||= body.size !== body.targetSize || body.alpha !== 1;
  }
  state.settling = settling;
  return moved;
}

/* ---- 그림 -------------------------------------------------------------------- */

/* 한 장. `step`은 지난 그림 뒤로 흐른 밀리초(정지 화면은 0)이고, 카메라와 우편은 손이
 * 오른 동안 멈춘다(`hold`). 목표로의 미끄러짐은 멈추지 않는다. 차례: 바닥, 선, 그리고
 * 점과 우편의 점을 깊은 것부터 — 가까운 것이 먼 것을 덮는다. */
function agentOrbitDraw(state, step, stamp, { still = false } = {}) {
  const inks = agentOrbitInks(state);
  if (!inks || state.width <= 0 || state.height <= 0 || state.reach <= 0) return;
  const { ctx } = state;
  const settle = still ? 1 : 1 - Math.exp(-step / ORBIT.settleMs);
  const appear = still ? 1 : 1 - Math.exp(-step / ORBIT.appearMs);
  /* 멈춤은 곧게 줄어 `holdMs`에 정확히 선다 — 지수로 다가가면 끝내 조금씩 흐른다. */
  const ramp = still ? 1 : step / ORBIT.holdMs;
  state.hold = state.holdTarget < state.hold
    ? Math.max(state.holdTarget, state.hold - ramp) : Math.min(state.holdTarget, state.hold + ramp);
  const sim = still ? 0 : step * state.hold;
  if (!agentOrbitReduced()) agentOrbitTurn(state, step, sim);
  const moved = agentOrbitSettle(state, settle, appear);
  const lens = agentOrbitLens(state);
  const projected = moved || !agentOrbitSameLens(state.lens, lens);
  if (projected) agentOrbitProjectScene(state, lens);
  if (!state.strokes || state.strokesFor !== state.selected) agentOrbitStrokes(state);

  ctx.setTransform(state.dpr, 0, 0, state.dpr, 0, 0);
  ctx.clearRect(0, 0, state.width, state.height);
  ctx.lineCap = "round";
  agentOrbitShadows(state, inks);
  for (const group of [...state.floor, ...state.strokes]) {
    ctx.strokeStyle = inks[group.ink];
    ctx.globalAlpha = group.alpha;
    ctx.lineWidth = group.width;
    ctx.setLineDash(group.dash);
    ctx.beginPath();
    for (const points of group.lines) {
      ctx.moveTo(points[0], points[1]);
      for (let at = 2; at < points.length; at += 2) ctx.lineTo(points[at], points[at + 1]);
    }
    ctx.stroke();
  }
  ctx.setLineDash(ORBIT_SOLID);

  /* 우편의 점: 곡선 위의 자리(`t`)를 옮기고 투영한다 — 점은 늘 움직이므로 프레임마다. */
  const dots = state.dots;
  dots.length = 0;
  const lensNow = state.lens;
  for (const link of state.links) {
    if (!link.shown) continue;
    for (const particle of link.particles) {
      if (still) particle.t = ORBIT.mail.stillAt;
      else particle.t = (particle.t + sim / ORBIT.mail.travelMs) % 1;
      agentOrbitAlong(link.curve, particle.t, ORBIT_POINT);
      agentOrbitProject(lensNow, ORBIT_POINT[0], ORBIT_POINT[1], ORBIT_POINT[2], particle.seen);
      agentOrbitAlong(link.curve, Math.max(0, particle.t - ORBIT.mail.tailPx / link.length), ORBIT_POINT);
      agentOrbitProject(lensNow, ORBIT_POINT[0], ORBIT_POINT[1], ORBIT_POINT[2], particle.tail);
      particle.link = link;
      dots.push(particle);
    }
  }
  dots.sort((left, right) => right.seen.depth - left.seen.depth);
  let dot = 0;
  let pulsing = false;
  for (const body of state.order) {
    while (dot < dots.length && dots[dot].seen.depth >= body.seen.depth) {
      agentOrbitPaintDot(state, dots[dot], inks);
      dot += 1;
    }
    agentOrbitPaintBody(state, body, inks, stamp, still);
    pulsing ||= body.pulseAt !== null;
  }
  for (; dot < dots.length; dot += 1) agentOrbitPaintDot(state, dots[dot], inks);
  ctx.globalAlpha = 1;
  state.pulsing = pulsing;

  if (projected) agentOrbitPlaceLabels(state);
  state.dirty = false;
  agentOrbitFrames += 1;
}

/* 워크스페이스의 그림자: 기둥의 발치에 바닥으로 눌린 타원 — 가로는 점의 크기, 세로는 바닥이
 * 카메라 쪽으로 기운 만큼(pitch의 사인). 제 빛깔, 안개의 알파. */
function agentOrbitShadows(state, inks) {
  const { ctx } = state;
  const tilt = Math.abs(Math.sin(state.camera.pitch));
  for (const hub of state.bodies.values()) {
    const foot = hub.foot;
    if (hub.kind !== "workspace" || !foot?.shown) continue;
    const wide = hub.size * ORBIT.hub.shadow * foot.scale;
    ctx.globalAlpha = ORBIT.hub.shadowAlpha * foot.fog * hub.alpha * (hub.dim ? ORBIT.dim : 1);
    ctx.fillStyle = inks[hub.ink];
    ctx.beginPath();
    ctx.ellipse(foot.x, foot.y, wide, wide * tilt, 0, 0, ORBIT_TURN);
    ctx.fill();
  }
}

/* 점 하나: 구 한 장(원근의 크기, 안개의 알파), 확인 필요·실패는 제 색의 테, 고른 것과 손이
 * 오른 것의 테, 턴 끝의 맥동. */
function agentOrbitPaintBody(state, body, inks, stamp, still) {
  const seen = body.seen;
  if (!seen.shown) return;
  const { ctx } = state;
  const radius = body.size * seen.scale;
  const alpha = body.alpha * (body.dim ? ORBIT.dim : 1) * seen.fog;
  ctx.globalAlpha = alpha;
  ctx.drawImage(agentOrbitBall(state, body.ink), seen.x - radius, seen.y - radius, radius * 2, radius * 2);
  let edge = radius;
  if (body.ringed) {
    edge = radius + ORBIT.agent.ringGap;
    agentOrbitRing(ctx, inks[body.ink], seen.x, seen.y, edge, alpha, ORBIT.agent.ringWidth);
  }
  if (body.key === state.selected) agentOrbitRing(ctx, inks.select, seen.x, seen.y, edge + ORBIT.select.gap, 1);
  else if (body.key === state.hovered) {
    agentOrbitRing(ctx, inks.select, seen.x, seen.y, edge + ORBIT.select.gap, ORBIT.select.hoverAlpha);
  }
  if (body.pulseAt === null) return;
  const progress = still ? 1 : (stamp - body.pulseAt) / ORBIT.pulse.ms;
  if (progress >= 1 || progress < 0) {
    body.pulseAt = null;
    return;
  }
  const spread = 1 - (1 - progress) ** 2;
  agentOrbitRing(ctx, inks.working, seen.x, seen.y, edge + ORBIT.pulse.reach * spread, (1 - progress) * alpha,
    ORBIT.pulse.width);
}

/* 우편의 점 하나: 빛무리, 꼬리, 점 — 보낸 쪽 → 받는 쪽, 미확인이 있으면 밝고 크다. 크기는
 * 원근을 따르고(원점 깊이에서 표의 크기), 알파는 안개를 따른다. */
function agentOrbitPaintDot(state, particle, inks) {
  const { ctx } = state;
  const link = particle.link;
  const seen = particle.seen;
  if (!seen.shown) return;
  const ink = link.unread ? "unread" : "mail";
  const near = seen.scale / state.scale;
  const alpha = (link.unread ? ORBIT.mail.unreadAlpha : ORBIT.mail.readAlpha)
    * (link.from.dim || link.to.dim ? ORBIT.dim : 1) * seen.fog;
  const radius = (link.unread ? ORBIT.mail.unreadRadius : ORBIT.mail.radius) * near;
  const glow = radius * ORBIT.mail.glow;
  ctx.globalAlpha = alpha * state.glowAlpha;
  ctx.drawImage(agentOrbitSprite(state, ink), seen.x - glow, seen.y - glow, glow * 2, glow * 2);
  ctx.globalAlpha = alpha * ORBIT.mail.tailAlpha;
  ctx.strokeStyle = inks[ink];
  ctx.lineWidth = ORBIT.mail.tailWidth;
  ctx.beginPath();
  ctx.moveTo(particle.tail.x, particle.tail.y);
  ctx.lineTo(seen.x, seen.y);
  ctx.stroke();
  ctx.globalAlpha = alpha;
  ctx.fillStyle = inks[ink];
  ctx.beginPath();
  ctx.arc(seen.x, seen.y, radius, 0, ORBIT_TURN);
  ctx.fill();
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
  const bodies = state ? [...state.bodies.values()] : [];
  const edges = state?.edges ?? [];
  const links = state?.links ?? [];
  const count = (list, kind) => list.filter((one) => one.kind === kind).length;
  return {
    applied: agentOrbitApplied,
    frames: agentOrbitFrames,
    ticks: agentOrbitTicks,
    pulses: agentOrbitPulses,
    inkReads: agentOrbitInkReads,
    projections: agentOrbitProjections,
    running: agentOrbitFrame !== null,
    reduced: agentOrbitReduced(),
    workspaces: count(bodies, "workspace"),
    agents: count(bodies, "agent"),
    children: count(bodies, "child"),
    bodies: bodies.length,
    edges: { structure: count(edges, "structure"), spawned: count(edges, "spawned"),
      dependency: count(edges, "dependency") },
    links: links.length,
    particles: links.reduce((total, link) => total + link.particles.length, 0),
    bright: links.filter((link) => link.unread && link.particles.length > 0).length,
    scale: state?.scale ?? null,
    zoom: agentGraphZoom,
    paused: state ? state.holdTarget === 0 : null,
    labels: state?.host.childElementCount ?? 0,
    named: state?.named ?? 0,
    camera: state ? { yaw: state.camera.yaw, pitch: state.camera.pitch, spin: state.camera.spin } : null,
    order: state?.order.map((body) => body.key) ?? [],
  };
}
