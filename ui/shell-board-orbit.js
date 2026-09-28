/* ---- 관계의 입체 보기 (t-9444 → t-10118 → 흐름 보드, 2026-09-27) ------------
 *
 * 보드의 `관계` 그림을 한 번 더 그리는 방법. 카드와 연결선 대신 워크스페이스는 떠 있는
 * 판이 되고, 에이전트는 그 판 위에 앉은 로봇 하나가 되며, 지시는 빛의 레일을 따라 왼쪽에서
 * 오른쪽으로 흐른다 — 사람이 승인한 시안(docs/design/board-3d-preview/index.html의
 * 「새 시안」)을 실제 보드의 데이터 위에 세운 것이다. 카드 그림은 그대로 남고, 툴바의 토글
 * 하나가 둘 사이를 오간다(로컬 설정 한 키).
 *
 * **왼쪽에서 오른쪽으로 읽는다.** 메인 워크스페이스는 왼쪽의 큰 판(「본진」)이고, 조율자와
 * 부모 없는 에이전트가 거기 앉는다. 다른 워크스페이스는 오른쪽의 섬이고, 섬의 열은 계보의
 * 깊이다 — 다른 워크스페이스에서 지시를 받은 섬은 지시한 쪽의 오른쪽에 선다. 섬은 앞뒤로
 * 한 줄씩 쌓인다(섬마다 제 줄). 그래서 섬이 옆으로 자라도 다른 섬과 겹치지 않는다. 같은
 * 워크스페이스의 하위 에이전트는 부모 곁의 빈자리에 앉는다.
 *
 * **자리는 한 번 앉으면 지킨다.** 판의 자리는 번호로 키마다 붙고(`agentOrbitSeatAll`),
 * 새로 온 키는 빈자리나 끝자리를 받는다 — 떠난 키의 자리는 비고, 남의 자리는 그대로다.
 * 판은 처음 설 때 다음 손님의 빈자리를 하나 남겨 두고, 자리의 좌표는 판의 크기에 기대지
 * 않는다: 본진은 왼쪽으로, 섬은 오른쪽으로 자라며 앉은 로봇은 움직이지 않는다. 판의 줄도
 * 한 번 받으면 지킨다. 같은 입력이면 같은 자리다.
 *
 * **빛의 색은 상태, 몸의 색은 모델.** 눈·고리·레일·판의 테는 상태의 잉크만 입고(판의 테는
 * 그 판에서 가장 급한 상태 — 확인 필요·실패 > 작업 중 > 완료 > 대기), 로봇의 몸은 모델의
 * 계열(Claude는 흙·살구, Codex는 흑연·강철, 그 밖은 무채)에서 에이전트 키의 해시가 고른 한
 * 톤을 입는다 — 에이전트마다 손으로 적은 색은 없다. 작업 중인 로봇은 타자를 치고, 대기는
 * 고개를 떨구며, 확인 필요는 손을 흔들며 뛰고 호박색 빛기둥이 서고, 완료는 두 팔을 들며
 * 막 끝난 판에서 한 번 뛴다. 실패는 고개를 떨군 채 제 빛을 든다. 본진의 조율자는 1.5배로
 * 단 위에 서고 천천히 도는 두 고리와 헤드셋을 든다.
 *
 * **흐르는 것은 지금 오가는 것뿐이다.** 부모 → 자식은 빛의 관(레일)이고, 그 위의 빛
 * 알갱이는 작업 중인 자식에게 흐르며, 확인 필요인 자식의 알갱이는 끝 근처에서 멈춰 맥박친다.
 * 우편은 방금 오간(`mail.freshMs`) 링크만 보낸 쪽 → 받는 쪽으로 호를 그리며 흐르고, 과업
 * 의존은 가늘고 옅은 관이다. 구조(에이전트 → 워크스페이스)는 선이 아니라 앉은 자리가 말한다.
 *
 * **읽는 것은 관계 그림의 모델 하나다.** `paintAgentGraph`가 그린 그 모델(범위와
 * 검색이 이미 먹은 것)을 받아 다시 읽을 뿐, 새 백엔드 호출도 원장 읽기도 없다. 상태의
 * 낱말(`agentGraphStateWord`)·이름(`agentGraphIdentity`)·선택(`selectAgentGraphEntity`)·
 * 배율(`agentGraphZoom`)·관계 목록(`agentGraphRelations`)은 관계 그림의 것을 그대로
 * 쓴다 — 한 화면이 같은 에이전트를 두 가지로 말하지 않게.
 *
 * **상태는 모델이 움직일 때만 적용한다.** 이 보기가 쓰는 사실(상태·부모·검색·우편의
 * 신선함·의존)의 서명이 바뀐 판에서만 적용하고(`agentOrbitApplied`), 자리나 판의 크기가
 * 움직인 판에서만 GPU의 모양(판·레일·인스턴스)을 다시 짓는다 — 상태만 바뀐 판은 색만
 * 고친다. 같은 판을 다시 그리는 훅의 박자와 프레임은 적용을 한 번도 부르지 않는다.
 *
 * **카메라.** 직교 카메라가 표의 yaw·pitch로 비스듬히 내려다본다 — 먼 것이 작아지지
 * 않는다. 끌면 돌고(yaw는 표의 폭 안에서, pitch는 표의 위아래 안에서) 놓으면 미끄러져
 * 서며, 가만두면 아주 천천히 흔들린다 — 사람이 한 번 돌려 놓은 판은 그 자리에 선다(두 번
 * 누르면 맞춤으로 돌아가 다시 흔들린다). 맞춤은 그림 전체가 판에 드는 배율이고, 사람의
 * 배율이 그것을 곱한다 — 휠·단추·「전체 보기」가 한 길(`agentGraphZoom`)이다. 카메라는
 * 저장하지 않는다: 여는 판은 늘 표의 자리다.
 *
 * **이름표.** 이름은 DOM 버튼(`.agent-orbit-label`)이 든다 — 선택·초점·보조 기술이 카드
 * 그림과 같은 길로 간다. 섬의 에이전트는 섬 오른쪽, 조율자는 머리 위, 본진의 나머지는 본진
 * 앞에 쌓이고, 겹칠 자리밖에 없는 이름표는 로봇 머리의 작은 칩으로 접힌다(손이나 초점이
 * 오면 펼쳐진다). 말풍선(확인 필요·완료·실패·대기)은 이름표의 `::before`라 요소가 늘지
 * 않는다. 로봇이나 판을 눌러도(광선 투사) 같은 선택 길이다. 고른 에이전트의 발밑에는 고리가
 * 서고, 그 계보 밖의 줄은 옅어진다.
 *
 * **왜 WebGL인가 (2026-09-27, 사람의 결정).** 이 보기는 캔버스 2d였고, 그때 WebGL을
 * 물리친 까닭은 셋이었다 — 컨텍스트 유실의 폴백, WKWebView의 위험, 둘째 페인터의 유지비.
 * 사람이 시안을 보고 「이대로」 three.js를 제품에 들이기로 정했고, 셋은 이렇게 답한다.
 * 유실: `webglcontextlost`에서 이 판은 카드 보기로 서고 토글이 까닭을 말한다(실시간 지도의
 * 손잡이와 같은 모양); `webglcontextrestored`면 다시 입체로 선다. WebGL2가 없는 창도 같은
 * 길로 카드에 선다. WKWebView: 하네스가 `--engine webkit`으로 같은 시험과 무게를 잰다.
 * 유지비: 손으로 쓴 GL은 없다 — 이 파일 하나가 three.js 한 벌(`ui/vendor/three.js`)로
 * 그리고, 판을 내려놓을 때 GPU 자원을 모두 돌려준다(토글을 오가도 `renderer.info.memory`가
 * 자라지 않는다). 그리기 호출은 로봇 부품마다 인스턴스 하나, 판·레일은 합친 모양 하나라
 * 에이전트 수로 늘지 않는다. 빛번짐(bloom)은 시안의 것 그대로다 — 문턱 → 다섯 단의 분리
 * 가우스(HalfFloat) → 톤 매핑과 색 공간의 합성. 문턱의 NaN·Inf 가드와 빛기둥 셰이더의
 * `pow()` 안 `clamp()`는 지운다면 NaN 한 픽셀이 검은 네모로 번진다. 부동소수 렌더
 * 타깃이 없는 창은 빛번짐 없이 그린다.
 *
 * **보이지 않거나 멈추면 박자가 없다.** 그림이 서는지는 실시간 지도와 같은 한 손으로
 * 묻는다(`agentGraphPictureOn`·`agentGraphViewStands`, shell-board-live.js) — 숨은
 * 문서·작업 목록·숨은 판·카드 보기에서 rAF는 0이다. 손이나 초점이 라벨에 든 판도 0이고,
 * 사람이 돌려 놓은 판에 흐르는 것이 없어도 0이다. **흐른다**는 것은 작업 중·확인 필요인
 * 에이전트(타자·손짓·레일의 알갱이)가 있거나 방금 오간 우편이 흐르는 것이다 — 대기·완료·
 * 실패만 남은 판은 흐르지 않는다. 움직임을 줄이라는 판은 정지 화면이고, 상태가 바뀌거나
 * 사람이 끌 때만 한 장을 다시 그린다. 초점이 없는 창은 초당 30장까지만 그린다.
 *
 * 수는 아래 표 `ORBIT` 하나에 있고(셰이더의 글도 그 안에 산다), 색은 CSS 토큰
 * (`--agent-orbit-*`, shell.css 「입체」 절)이 든다 — 역할 토큰에서 오므로 라이트·다크가
 * 함께 뒤집힌다. 빛의 세기는 판의 색 구성표(라이트·다크)마다 표의 한 줄이다. */

/* 이 보기의 수, 한 표. 셰이더의 글과 모양의 치수가 여기 함께 서는 것은 이 그림이 WebGL이라
 * CSS가 셈을 대신할 수 없기 때문이다 — GPU가 읽는 수가 두 곳에 적히면 한 곳만 움직이는 날이
 * 온다. 공간의 길이는 세계 단위(시안의 단위 그대로)이고, 화면의 길이는 CSS 픽셀이다. */
const ORBIT = Object.freeze({
  /* 고른 보기가 남는 로컬 설정 한 키와 그 값 둘. 값 `orbit`은 사람이 이미 저장한 선택이라
   * 보기의 이름이 바뀌어도 그대로 읽는다. */
  store: "zerocode.board-relations-view.v1",
  orbit: "orbit",
  cards: "cards",
  /* 움직임을 줄이라는 판을 묻는 미디어 질의 — CSS의 같은 질의가 전이와 말풍선을 끈다. */
  reduced: "(prefers-reduced-motion: reduce)",
  second: 1000,
  /* 그림의 박자. 초점이 있는 창 60, 없는 창 30 — rAF가 이보다 잦게 와도(120 Hz 화면) 그림은
   * 이 수를 넘지 않는다. `slackMs`는 반 박자 일찍 온 rAF를 건너뛰지 않게 하는 여유다. */
  fps: Object.freeze({ focused: 60, blurred: 30, slackMs: 2 }),
  /* 멈췄다 온 박자 하나가 흘리는 시간의 상한 — 돌아온 판이 한 번에 몇 초를 건너뛰지 않게. */
  stepCapMs: 100,
  /* 손이 라벨 위에 오르거나 초점이 들면 카메라와 움직임은 `holdMs`에 걸쳐 곧게 줄어 멈추고,
   * 떠나면 같은 시간에 걸쳐 다시 움직인다. */
  holdMs: 260,
  /* 상태 → 잉크, 급함(판의 테가 고르는 차례), 자세, 흐르는가. 낱말은 관계 그림의 한 손
   * (`agentGraphStateWord`)이 짓는다. */
  states: Object.freeze({
    "needs-attention": Object.freeze({ ink: "attention", rank: 4, pose: "attention", flows: true, bubble: "attention" }),
    failed: Object.freeze({ ink: "failed", rank: 3, pose: "failed", flows: false, bubble: "failed" }),
    working: Object.freeze({ ink: "working", rank: 2, pose: "working", flows: true, bubble: "" }),
    done: Object.freeze({ ink: "done", rank: 1, pose: "done", flows: false, bubble: "done" }),
    paused: Object.freeze({ ink: "idle", rank: 0, pose: "idle", flows: false, bubble: "idle" }),
    idle: Object.freeze({ ink: "idle", rank: 0, pose: "idle", flows: false, bubble: "idle" }),
  }),
  /* 본진: 메인 워크스페이스의 판. 자리 사이 `seat`(조율자의 단이 이웃과 닿지 않는 폭),
   * 처음 설 때의 줄 `rows`(`minRows`..`maxRows`)와 열(오른쪽 끝에서 왼쪽으로 자란다),
   * 가장자리 여백, 판의 두께·모서리·빗면, 빛나는 테의 여유·모서리·굵기. */
  deck: Object.freeze({ seat: 104, cols: 2, minRows: 2, maxRows: 4, margin: 40, height: 24, radius: 28,
    bevel: 4, rim: 8, rimRadius: 32, tube: 2 }),
  /* 섬: 다른 워크스페이스의 판. 자리 사이, 처음의 열 수 하한, 한 줄의 목표 자리 수와 줄의
   * 상한, 여백, 오른쪽 끝의 화면 자리(`bay`), 두께·모서리·빗면·테. */
  /* 섬이 처음 선 뒤 오른쪽으로 자랄 수 있는 열의 수(`growth`) — 다음 열은 그만큼 비워 두고 선다. */
  island: Object.freeze({ seat: 68, minCols: 3, perRow: 4, maxRows: 3, marginX: 26, marginZ: 28, bay: 62,
    height: 18, radius: 22, bevel: 3, rim: 6, rimRadius: 25, tube: 1.7, growth: 3 }),
  /* 판마다 처음 설 때 남겨 두는 빈자리 — 다음 손님이 판을 키우지 않게. */
  spare: 1,
  /* 흐름의 틀: 본진의 오른쪽 끝(x = 0)에서 첫 열의 섬까지의 틈, 앞 열의 가장 오른쪽 끝에서 다음
   * 열까지의 틈, 줄 사이(z), 섬의 줄 사이에 보이게 남기는 틈(`clear` — 섬의 줄 사이 자체는
   * `agentOrbitRowGap`이 여는 카메라에서 잰다). */
  flow: Object.freeze({ gap: 210, column: 160, row: 32, clear: 12 }),
  /* 판의 모양을 짓는 조각 수: 모서리의 곡선, 빗면, 테의 둘레와 관의 둘레. */
  shape: Object.freeze({ curve: 10, bevel: 3, outline: 160, rimRadial: 6, top: 0.5 }),
  /* 로봇 한 벌(시안의 치수, 세계 단위). */
  bot: Object.freeze({
    body: Object.freeze({ radius: 10, length: 9, cap: 4, radial: 12, y: 17.5 }),
    neck: Object.freeze({ top: 5, bottom: 6, height: 6, radial: 12, y: 34.5 }),
    head: Object.freeze({ y: 37, w: 28, d: 22, h: 19, radius: 8, bevel: 3, skull: 19 }),
    visor: Object.freeze({ w: 24, h: 11, radius: 5, y: 9.5, z: 14.35 }),
    eye: Object.freeze({ radius: 1.7, length: 3.2, cap: 3, radial: 8, x: 5.2, y: 10, z: 14.9 }),
    stem: Object.freeze({ radius: 0.9, height: 8, radial: 8, y: 23 }),
    tip: Object.freeze({ radius: 2.8, width: 12, height: 10, y: 28.5, ring: 2.4, tube: 0.9, ringY: 29.6,
      tubular: 8, around: 24 }),
    ear: Object.freeze({ radius: 5.2, height: 4, radial: 16, x: 17.8, y: 9 }),
    arm: Object.freeze({ radius: 2.4, length: 8, cap: 4, radial: 10, x: 10.8, y: 26, z: 1, hang: -6.5 }),
    mitt: Object.freeze({ radius: 3.6, width: 12, height: 10, hang: -14 }),
    chest: Object.freeze({ radius: 3, segments: 16, y: 20, z: 10.05 }),
    keys: Object.freeze({ w: 28, d: 10, y: 16.5, z: 15 }),
    pad: Object.freeze({ top: 25, bottom: 27, height: 3, radial: 32, y: 1.5, ring: 26, tube: 0.9, ringY: 3.1,
      tubular: 6, around: 48 }),
    pulse: Object.freeze({ inner: 24, outer: 26.5, segments: 48, y: 3.4 }),
    pick: Object.freeze({ radius: 34, height: 96, radial: 12 }),
    /* 머리 위 이름표·말풍선이 재는 높이의 기준(로봇의 키). */
    tall: 72,
    scale: Object.freeze({ lead: 1.5, deck: 1.3, agent: 1.25, child: 1.15 }),
    dais: Object.freeze({ top: 46, bottom: 50, height: 8, radial: 48, y: 4, lift: 8, ring: 47, tube: 1.3,
      ringY: 8.3, tubular: 8, around: 96 }),
    halo: Object.freeze({ radius: 64, tube: 0.7, y: 50, outer: 76, outerTube: 0.45, outerY: 58, tubular: 6,
      around: 120 }),
    mic: Object.freeze({ path: Object.freeze([Object.freeze([19.5, 6, 2]), Object.freeze([18.5, 2, 9]),
      Object.freeze([10, 1, 15.6])]), tube: 0.75, segments: 16, radial: 6, ball: 1.9, ballWidth: 12,
    ballHeight: 10 }),
    beam: Object.freeze({ radius: 10, core: 1.6, height: 170, radial: 24, coreRadial: 8, y: 3 }),
    select: Object.freeze({ inner: 35, outer: 37, segments: 72, y: 0.6 }),
  }),
  /* 로봇의 바탕 재질(거칠기·금속성) — 몸의 세 톤과 무채의 셋. */
  material: Object.freeze({
    main: Object.freeze({ roughness: 0.36, metalness: 0.12 }),
    light: Object.freeze({ roughness: 0.34, metalness: 0.05 }),
    accent: Object.freeze({ roughness: 0.35, metalness: 0.25 }),
    metal: Object.freeze({ roughness: 0.38, metalness: 0.45 }),
    visor: Object.freeze({ roughness: 0.12, metalness: 0.7 }),
    pad: Object.freeze({ roughness: 0.4, metalness: 0.5 }),
    plate: Object.freeze({ roughness: 0.5, metalness: 0.2 }),
    sheath: 0.16,
  }),
  /* 자세(라디안, 초). 시안의 수 그대로 — 타자(`working`), 고개 떨굼(`idle`), 손짓과 뜀
   * (`attention`), 두 팔(`done`), 떨군 채 제 빛(`failed`). `face`는 로봇이 보는 방향. */
  pose: Object.freeze({
    face: Object.freeze({ lead: 1.05, island: 0.95, deck: 0.34, speed: 6 }),
    blink: Object.freeze({ every: 3.8, for: 0.13, shut: 0.15 }),
    working: Object.freeze({ arm: -1.15, spread: 0.1, tap: 0.2, tapSpeed: 15, head: 0.1, nod: 0.05,
      nodSpeed: 2.2, eyes: 1 }),
    idle: Object.freeze({ head: 0.28, nod: 0.04, nodSpeed: 1.1, tilt: 0.08, arm: 0.08, spread: 0.12, eyes: 0.2,
      body: 0.12 }),
    attention: Object.freeze({ head: -0.14, sway: 0.08, swaySpeed: 5, arm: -0.15, spread: 0.18, wave: -2.85,
      waveTilt: 0.3, waveAmp: 0.28, waveSpeed: 9, eyes: 1.15, hop: 7, hopSpeed: 4.2 }),
    done: Object.freeze({ head: -0.1, arm: -2.55, spread: 0.5, eyes: 0.4, jump: 16, jumpFor: 1.2 }),
    failed: Object.freeze({ head: 0.34, tilt: -0.06, arm: 0.1, spread: 0.08, eyes: 0.5, body: 0.16 }),
    /* 발밑 고리의 맥박: 상태마다 초당 몇 번, 얼마나 번지고 얼마나 진한가. */
    pulse: Object.freeze({ working: 0.42, attention: 1.25, done: 0.25, grow: 1.4, growAttention: 1.9,
      alpha: 0.55, alphaAttention: 0.9 }),
    /* 조율자의 두 고리가 도는 빠르기와 흔들림. */
    halo: Object.freeze({ tilt: 0.2, wobble: 0.04, wobbleSpeed: 0.5, spin: 0.35, tiltOuter: 0.16,
      wobbleOuter: 0.05, wobbleOuterSpeed: 0.4, spinOuter: 0.22 }),
    /* 빛기둥의 맥박과 고른 고리의 숨. */
    beam: Object.freeze({ base: 0.75, amp: 0.25, speed: 5 }),
    select: Object.freeze({ base: 0.55, amp: 0.35, speed: 2.4 }),
  }),
  /* 레일: 판 위의 높이, 조율자에서 나오는 거리, 로봇 앞에서 멈추는 거리, 부채꼴의 두 점
   * (x의 몫, z의 몫), 섬 둘레의 길, 관과 덮개의 굵기, 한 판 안의 짧은 레일이 솟는 높이, 의존의
   * 호(몸에서 솟는 높이, 가운데의 들림과 거리의 몫), 관의 조각 수, 알갱이가 쓰는 표본 수. */
  rail: Object.freeze({ y: 12, exit: 44, stop: 29, fan: Object.freeze([0.3, 0.72]),
    fanZ: Object.freeze([0.32, 0.86]), lane: 8, entry: 2, core: 2.4, sheath: 9, radial: 8,
    sheathRadial: 12, dependency: 1, hop: 18, rise: 30, lift: 40, reach: 0.18, segments: 96, samples: 200 }),
  /* 빛 알갱이(혜성). 머리 하나와 꼬리 `tail`개, 꼬리 사이, 크기의 줄어듦, 알파의 줄어듦,
   * 머리가 섞는 흰빛, 레일의 빠르기(초당 몫)와 레일마다의 수, 확인 필요의 멈춘 자리. */
  /* `points`는 처음 잡는 알갱이의 자리 수 — 레일과 우편이 더 많으면 지을 때 늘린다(`agentOrbitCometRoom`). */
  comet: Object.freeze({ points: 2048, tail: 11, step: 0.0105, fade: 13, shrink: 0.72, drop: 12, curve: 1.6,
    tailAlpha: 0.85, headMix: 0.55, tailK: 0.8, size: 19, stallSize: 17, speed: 0.15, perRail: 3,
    stallAt: 0.66, stallStep: 0.11, stallPulse: 6, stallLow: 0.55, stallAmp: 0.45, phase: 0.11 }),
  /* 우편. 방금 오간(`freshMs` 안) 링크에 알갱이 하나가 보낸 쪽 → 받는 쪽으로 흐르고, 한 번
   * 건너는 데 `travelMs`가 든다. 미확인이면 크고 밝다. `cap`은 한 판에 흐르는 우편의 상한 —
   * 넘치면 오래된 링크의 것부터 접는다. 호는 몸에서 `rise`만큼 솟은 두 끝을 잇고, 가운데가
   * `arc` + 거리의 `bow`배만큼 들린다. 정지 화면의 알갱이는 `stillAt`에 선다. */
  mail: Object.freeze({ freshMs: 600_000, travelMs: 2_600, cap: 32, size: 15, unreadSize: 22, rise: 30, arc: 40,
    bow: 0.25, stillAt: 0.6 }),
  /* 빛의 세기(HDR 곱) 중 판의 색 구성표와 무관한 몫 — 시안의 `paint()`가 쓰던 곱들. */
  glow: Object.freeze({ tip: 1.2, ringIdle: 0.7, ringSelected: 1.5, chest: 0.85, keys: 0.75, pulse: 0.8,
    beam: 0.5, core: 1.2, halo: 0.75, haloOuter: 0.45, edgeIdle: 0.6, edgeSelected: 1.35, gridIdle: 0.4,
    poolIdle: 0.2, deckPool: 0.9, railIdle: 0.35, dependency: 0.35, screen: 0.7, dim: 0.3, search: 0.22 }),
  /* 판의 색 구성표마다의 빛: 그림자의 진함, 반구광·주광·역광, 테·격자·빛·잠든 눈·화면·레일·
   * 알갱이·웅덩이의 곱, 바닥 격자의 알파, 빛번짐(세기·문턱·반경), 고른 고리. */
  themes: Object.freeze({
    dark: Object.freeze({ shadow: 0.6, hemi: 0.55, key: 1.5, rim: 0.8, edge: 1.9, grid: 0.22, glow: 2.6,
      eyeIdle: 0.35, screen: 1.25, rail: 2.2, comet: 3.4, pool: 0.16, floor: 0.05, strength: 1.05,
      threshold: 0.78, radius: 0.55, select: 1.25, sheath: 0.16, sheathInk: 2.2, additive: true, ambient: 0.07 }),
    light: Object.freeze({ shadow: 0.2, hemi: 1.15, key: 1.9, rim: 0.35, edge: 1.05, grid: 0.12, glow: 1.25,
      eyeIdle: 0.5, screen: 0.9, rail: 1.7, comet: 1.9, pool: 0.05, floor: 0.08, strength: 0.4,
      threshold: 1.25, radius: 0.35, select: 0.9, sheath: 0.62, sheathInk: 0.9, additive: false, ambient: 0 }),
  }),
  /* 빛: 주광의 자리(과녁에서), 역광의 자리, 그림자 지도의 크기·치우침·면의 치우침, 그림자
   * 카메라가 장면 반지름의 몇 배를 덮는가와 앞뒤. */
  light: Object.freeze({ key: Object.freeze([-420, 900, 700]), rim: Object.freeze([600, 320, -800]),
    map: 2048, bias: -0.0008, normalBias: 0.8, reach: 1.25, near: 100, far: 2600 }),
  /* 바닥: 판 아래의 높이, 넓이, 그림자를 받는 판, 격자가 옅어지는 반경(장면 반지름의 몫),
   * 빛 웅덩이의 수와 반경, 본진의 웅덩이가 오른쪽으로 비키는 거리. */
  ground: Object.freeze({ y: -86, size: 6000, catcher: 3200, lift: 0.5, fadeFrom: 0.45, fadeTo: 1.9, pools: 10,
    deckPool: 250, islandPool: 190, deckShift: 40, ambient: 480 }),
  /* 떠다니는 먼지: 수, 씨앗(파크-밀러 난수), 높이, 장면을 넘어 퍼지는 여유. */
  dust: Object.freeze({ count: 170, seed: 7, multiplier: 16807, modulus: 2147483647, height: 330, floor: -70,
    reach: 180 }),
  /* 섬 위의 화면(섬마다 하나): 크기, 높이, 판 오른쪽 끝에서 들어온 자리, 돌아선 각(표 yaw의
   * 몫), 화면의 모양(코드·완료·확인). */
  screen: Object.freeze({ w: 50, h: 32, y: 44, turn: 0.8, modes: Object.freeze({ code: 0, done: 1, attention: 2 }) }),
  /* 카메라. 여는 자리(yaw·pitch), 끌어서 도는 yaw의 폭(여는 자리에서 ±), pitch의 위아래,
   * 끈 1 px의 회전, 가만둔 판의 흔들림(진폭과 초당 빠르기), 놓은 뒤의 미끄러짐(끌던 마지막
   * `sampleMs`의 빠르기에서 시작, 상한 `glideMax` 라디안/ms, 시간 상수 `glideMs`, `rest`
   * 밑이면 선다), 과녁에서의 거리와 앞뒤 평면. */
  camera: Object.freeze({ yaw: 0.34, pitch: 0.6, range: 0.75, pitchMin: 0.28, pitchMax: 1.2, turn: 0.005,
    sway: 0.04, swaySpeed: 0.16, sampleMs: 80, glideMs: 320, glideMax: 0.003, rest: 0.00005, distance: 2400,
    near: 1, far: 6000 }),
  /* 맞춤: 판의 가장자리 여백(왼쪽, 오른쪽 끝, 흐름 한 줄의 위, 범례의 아래), 좁은 판의 문턱과 그
   * 여백, 맞춤 배율의 위·아래. 섬의 이름표가 서는 오른쪽 폭은 잰 이름표가 더한다. */
  fit: Object.freeze({ pad: Object.freeze({ left: 24, right: 12, top: 112, bottom: 66 }), compact: 520,
    padCompact: Object.freeze({ left: 10, right: 8, top: 96, bottom: 64 }), max: 4, min: 0.05 }),
  /* 그리기: 기기 픽셀의 상한, 노출, 장면 타깃의 다중 표본, 빛번짐의 문턱 무릎과 다섯 단의 커널,
   * 합성이 부르는 다섯 단의 이름. */
  render: Object.freeze({ pixelCap: 2, exposure: 1.05, samples: 4, knee: 0.45,
    kernels: Object.freeze([3, 5, 7, 9, 11]), taps: Object.freeze(["b0", "b1", "b2", "b3", "b4"]) }),
  /* 이름표: 자리를 다시 쓰는 문턱(px)과 반올림의 눈금, 서로의 틈, 판 가장자리의 여백, 섬 오른쪽
   * 끝에서 떨어지는 거리(세계), 조율자 머리 위 높이(로봇 키의 배), 본진 앞 이름표가 매달리는
   * 높이, 판 이름이 서는 자리, 한 자리에서 비켜 보는 차례, 말풍선의 높이와 대기의 비킴. */
  label: Object.freeze({ epsilon: 0.2, round: 10, gap: 6, pad: 4, edge: 8, island: 10, lead: 1.7,
    deckDrop: -26, plate: -12, plateInset: 12,
    tries: Object.freeze({ agent: Object.freeze([0, 1, -1, 2, -2, 3, -3]), deck: Object.freeze([0, 1, 2, -1]),
      lead: Object.freeze([0, -1, 1, -2, 2]), plate: Object.freeze([0, 1, -1]), bubble: Object.freeze([0, -1]) }),
    /* 말풍선: 상태마다 머리 위 높이(로봇 키의 배), 대기의 비킴, 글줄의 높이(글자 크기의 배). */
    bubble: Object.freeze({ attention: 1.28, done: 1.1, failed: 1, idle: 0.8, shift: 22, line: 1.3 }),
    /* 이름표가 먼저 자리를 고르는 차례: 조율자와 판의 이름표가 먼저, 그다음 급한 상태. */
    leadRank: 10 }),
  /* 판의 화면 기둥의 여덟 모서리(x 끝, z 끝, 높이) — 0은 왼쪽·뒤·윗면, 1은 오른쪽·앞·로봇의 키.
   * 앞의 셋이 기둥의 두 모서리 방향(x·z)을 준다. */
  prism: Object.freeze([Object.freeze([0, 0, 0]), Object.freeze([1, 0, 0]), Object.freeze([0, 1, 0]),
    Object.freeze([1, 1, 0]), Object.freeze([0, 0, 1]), Object.freeze([1, 0, 1]), Object.freeze([0, 1, 1]),
    Object.freeze([1, 1, 1])]),
  /* 말풍선의 글: 상태 낱말(`{{word}}`)에 붙는 표식. 대기는 낱말 없이 조는 표식 하나. */
  bubbles: Object.freeze({ attention: "{{word}} !", done: "✓ {{word}}", failed: "✕ {{word}}", idle: "z z" }),
  /* 모델의 계열: 카드의 `agent`가 이 이름이면 그 계열, 아니면 무채. 톤은 셋(머리·몸·귀와
   * 손)이고, 계열마다 두 끝(`a`·`b`) 사이에서 에이전트 키의 해시가 자리를 고른다. */
  families: Object.freeze({ claude: "claude", codex: "codex", other: "other" }),
  /* 해시(FNV-1a, 32비트)를 0..1로 펴는 몫. */
  hashSpan: 4294967296,
  /* 한국어 조사를 고르는 한글 음절의 범위와 종성의 수 — 받침이 있으면 앞의 것. */
  hangul: Object.freeze({ first: 0xac00, last: 0xd7a3, finals: 28 }),
  /* 흐름 한 줄이 이름을 끼워 넣는 표지 — 번역된 문장을 이 자리에서 셋으로 나눈다. */
  mark: "\u0000",
  /* 계산된 색의 한 채널(0..255)과 세 축(x·y·z, r·g·b). */
  channel: 255,
  axes: 3,
  quarter: Math.PI / 2,
  turn: Math.PI * 2,
  /* 캔버스가 읽는 잉크 — `--agent-orbit-<이름>`. */
  inks: Object.freeze(["working", "attention", "idle", "done", "failed", "lead", "mail", "unread", "dependency",
    "spark", "select", "ground", "grid", "top", "side", "metal", "visor", "pad", "sky", "earth", "sun", "rim",
    "dust", "claude-main-a", "claude-main-b", "claude-light-a", "claude-light-b", "claude-accent-a",
    "claude-accent-b", "codex-main-a", "codex-main-b", "codex-light-a", "codex-light-b", "codex-accent-a",
    "codex-accent-b", "other-main-a", "other-main-b", "other-light-a", "other-light-b", "other-accent-a",
    "other-accent-b"]),
  /* 셰이더의 글. 셰이더가 쓰는 수는 여기에만 산다. 사용자 셰이더는 끝에 톤 매핑과 색 공간을
   * 든다 — 빛번짐의 장면 타깃(선형)으로 그릴 때는 빈 줄이고, 빛번짐 없이 화면에 바로 그릴
   * 때(부동소수 타깃이 없는 창)만 일한다. */
  shader: Object.freeze({
    quad: "varying vec2 vUv;void main(){vUv=uv;gl_Position=vec4(position.xy,0.,1.);}",
    threshold: "uniform sampler2D tDiffuse;uniform float threshold,knee;varying vec2 vUv;void main(){vec3 c=texture2D(tDiffuse,vUv).rgb;if(any(isnan(c))||any(isinf(c)))c=vec3(0.);float l=dot(c,vec3(.2126,.7152,.0722));gl_FragColor=vec4(c*smoothstep(threshold,threshold+knee,l),1.);}",
    blur: "uniform sampler2D tDiffuse;uniform vec2 texel,dir;uniform float sigma;varying vec2 vUv;void main(){vec3 s=texture2D(tDiffuse,vUv).rgb;float ws=1.;for(int i=1;i<=KERNEL;i++){float w=exp(-.5*float(i*i)/(sigma*sigma));vec2 o=dir*texel*float(i);s+=(texture2D(tDiffuse,vUv+o).rgb+texture2D(tDiffuse,vUv-o).rgb)*w;ws+=2.*w;}gl_FragColor=vec4(s/ws,1.);}",
    composite: "uniform sampler2D tBase,b0,b1,b2,b3,b4;uniform float strength,radius;varying vec2 vUv;float f(float x){return mix(x,1.2-x,radius);}void main(){vec3 b=f(1.)*texture2D(b0,vUv).rgb+f(.8)*texture2D(b1,vUv).rgb+f(.6)*texture2D(b2,vUv).rgb+f(.4)*texture2D(b3,vUv).rgb+f(.2)*texture2D(b4,vUv).rgb;gl_FragColor=vec4(texture2D(tBase,vUv).rgb+strength*b,1.);\n#include <tonemapping_fragment>\n#include <colorspace_fragment>\n}",
    instanced: "varying vec2 vUv;varying vec3 vTint;void main(){vUv=uv;vTint=vec3(1.);\n#ifdef USE_INSTANCING_COLOR\nvTint=instanceColor;\n#endif\nvec4 p=vec4(position,1.);\n#ifdef USE_INSTANCING\np=instanceMatrix*p;\n#endif\ngl_Position=projectionMatrix*modelViewMatrix*p;}",
    beam: "uniform float time,alpha;varying vec2 vUv;varying vec3 vTint;void main(){float fall=pow(clamp(1.-vUv.y,0.,1.),1.7);float band=.72+.28*sin(vUv.y*46.-time*7.);gl_FragColor=vec4(vTint*band,fall*alpha);\n#include <tonemapping_fragment>\n#include <colorspace_fragment>\n}",
    keys: "varying vec2 vUv;varying vec3 vTint;void main(){vec2 c=fract(vUv*vec2(10.,3.));vec2 k=step(vec2(.12,.16),c)*step(c,vec2(.88,.84));gl_FragColor=vec4(vTint*(.18+.82*k.x*k.y),1.);\n#include <tonemapping_fragment>\n#include <colorspace_fragment>\n}",
    screenVertex: "attribute vec2 look;varying vec2 vUv;varying vec3 vTint;varying vec2 vLook;void main(){vUv=uv;vLook=look;vTint=vec3(1.);\n#ifdef USE_INSTANCING_COLOR\nvTint=instanceColor;\n#endif\nvec4 p=vec4(position,1.);\n#ifdef USE_INSTANCING\np=instanceMatrix*p;\n#endif\ngl_Position=projectionMatrix*modelViewMatrix*p;}",
    screen: "uniform float time;varying vec2 vUv;varying vec3 vTint;varying vec2 vLook;float hash(vec2 p){return fract(sin(dot(p,vec2(127.1,311.7)))*43758.5453);}float seg(vec2 p,vec2 a,vec2 b){vec2 pa=p-a,ba=b-a;float h=clamp(dot(pa,ba)/dot(ba,ba),0.,1.);return length(pa-ba*h);}void main(){vec2 p=vUv;float edge=min(min(p.x,1.-p.x),min(p.y,1.-p.y));float frame=1.-smoothstep(.012,.03,edge);float ink=0.;if(vLook.x<.5){float y=p.y*14.+time*.9+vLook.y*14.;float row=floor(y);float fy=fract(y);float x0=.08+floor(hash(vec2(row,1.))*3.)*.07;float x1=min(x0+.2+hash(vec2(row,2.))*.62,.92);float bar=step(.3,fy)*step(fy,.7)*step(x0,p.x)*step(p.x,x1);float word=step(.3,fract((p.x-x0)*(3.+hash(vec2(row,3.))*4.)));ink=bar*mix(.45,1.,word);}else{vec2 q=vec2(p.x,p.y*.64);float d=vLook.x<1.5?min(seg(q,vec2(.31,.33),vec2(.45,.2)),seg(q,vec2(.45,.2),vec2(.7,.46))):min(seg(q,vec2(.5,.52),vec2(.5,.27)),length(q-vec2(.5,.15))-.015);ink=1.-smoothstep(.035,.06,d);}gl_FragColor=vec4(vTint*max(ink,frame*.8),1.);\n#include <tonemapping_fragment>\n#include <colorspace_fragment>\n}",
    gridVertex: "attribute vec2 local;attribute vec3 shape;varying vec2 vLocal;varying vec3 vShape;varying vec3 vTint;void main(){vLocal=local;vShape=shape;vTint=color;gl_Position=projectionMatrix*modelViewMatrix*vec4(position,1.);}",
    grid: "varying vec2 vLocal;varying vec3 vShape;varying vec3 vTint;float lines(vec2 p,float s){vec2 q=p/s;vec2 g=abs(fract(q-.5)-.5)/fwidth(q);return 1.-min(min(g.x,g.y),1.);}void main(){vec2 d=abs(vLocal)-vShape.xy+vShape.z;float sd=length(max(d,0.))+min(max(d.x,d.y),0.)-vShape.z;float inside=1.-smoothstep(-2.,0.,sd);gl_FragColor=vec4(vTint*lines(vLocal,32.)*inside,1.);\n#include <tonemapping_fragment>\n#include <colorspace_fragment>\n}",
    groundVertex: "varying vec3 vW;void main(){vec4 w=modelMatrix*vec4(position,1.);vW=w.xyz;gl_Position=projectionMatrix*viewMatrix*w;}",
    ground: "uniform vec3 base,grid;uniform float gridA;uniform vec2 centre,fade;uniform vec4 pools[POOLS];uniform vec3 poolCol[POOLS];varying vec3 vW;float lines(vec2 p,float s){vec2 q=p/s;vec2 g=abs(fract(q-.5)-.5)/fwidth(q);return 1.-min(min(g.x,g.y),1.);}void main(){vec2 p=vW.xz;float f=1.-smoothstep(fade.x,fade.y,length(p-centre));vec3 c=base+grid*(lines(p,40.)*gridA+lines(p,200.)*gridA*1.7)*f;for(int i=0;i<POOLS;i++){vec2 d=(p-pools[i].xy)/pools[i].z;c+=poolCol[i]*exp(-dot(d,d));}gl_FragColor=vec4(c,1.);\n#include <tonemapping_fragment>\n#include <colorspace_fragment>\n}",
    cometVertex: "attribute float size;attribute float alpha;varying vec3 vC;varying float vA;uniform float scale;void main(){vC=color;vA=alpha;gl_Position=projectionMatrix*modelViewMatrix*vec4(position,1.);gl_PointSize=size*scale;}",
    comet: "varying vec3 vC;varying float vA;void main(){float d=length(gl_PointCoord-.5)*2.;float a=1.-smoothstep(0.,1.,d);a*=a;gl_FragColor=vec4(vC,a*vA);\n#include <tonemapping_fragment>\n#include <colorspace_fragment>\n}",
    dustVertex: "attribute float seed;uniform float scale,time;varying float vA;void main(){vec3 p=position;p.y=FLOOR+mod(p.y+time*(3.+seed*7.),HEIGHT);p.x+=sin(time*.3+seed*40.)*8.;vA=(.35+.35*sin(time*1.4+seed*30.))*smoothstep(FLOOR,-20.,p.y)*(1.-smoothstep(200.,260.,p.y));gl_Position=projectionMatrix*modelViewMatrix*vec4(p,1.);gl_PointSize=(2.+seed*3.)*scale;}",
    dust: "uniform vec3 color;varying float vA;void main(){float d=length(gl_PointCoord-.5)*2.;gl_FragColor=vec4(color,(1.-smoothstep(0.,1.,d))*vA);\n#include <tonemapping_fragment>\n#include <colorspace_fragment>\n}",
  }),
});

/* ---- 고른 보기 ------------------------------------------------------------- */

/* 움직임을 줄이라는 판인가 — 한 번 묻고 바뀔 때 듣는다. */
const agentOrbitMotion = typeof window.matchMedia === "function" ? window.matchMedia(ORBIT.reduced) : null;

function agentOrbitReduced() {
  return agentOrbitMotion?.matches === true;
}

/* 사람이 고른 보기. `null`은 아직 저장소를 읽지 않았다는 뜻이다 — 보드를 처음 그릴
 * 때 한 번 읽는다. */
let agentOrbitHeld = null;

/* 입체를 그릴 수 없는 까닭 — `null`(그릴 수 있다), `unsupported`(WebGL2나 three.js가 없는
 * 창), `lost`(그래픽 장치가 컨텍스트를 놓았다). 까닭이 서 있는 동안 판은 카드로 서고, 토글이
 * 그 까닭을 말한다. 고른 보기(저장된 선택)는 그대로다 — 돌아오면 다시 입체로 선다. */
let agentOrbitBroken = null;
let agentOrbitProbed = false;

/* 고른 적이 없는 사람의 보기: 입체. 움직임을 줄이라는 판에서는 카드 — 움직이는 그림이
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

/* 이 창이 입체를 그릴 수 있는가. 처음 한 번 버리는 캔버스로 WebGL2를 물어보고(그 컨텍스트는
 * 곧바로 놓는다), 답을 든다. 유실은 판의 캔버스가 알려 온다(`agentOrbitLost`). */
function agentOrbitCapable() {
  if (!agentOrbitProbed) {
    agentOrbitProbed = true;
    let context = null;
    try {
      context = typeof window.THREE === "object" ? document.createElement("canvas").getContext("webgl2") : null;
    } catch {
      context = null;
    }
    if (context) context.getExtension("WEBGL_lose_context")?.loseContext();
    else agentOrbitBroken = "unsupported";
  }
  return agentOrbitBroken === null;
}

/* 토글의 손. 고른 것을 한 키에 적고 관계 그림을 다시 그린다 — 그림이 둘 중 무엇을
 * 세울지는 `dressAgentOrbitMode`가 정한다. 입체로 돌아오는 판은, 사람이 배율을 쥔
 * 적이 없으면 배율을 1로 둔다: 카드 그림의 맞춤이 내린 배율은 카드의 것이다. */
function setAgentOrbitChoice(view, choice) {
  if (choice !== ORBIT.orbit && choice !== ORBIT.cards) return;
  if (choice === ORBIT.orbit && !agentOrbitCapable()) return;
  if (agentOrbitChoice() === choice) return;
  agentOrbitHeld = choice;
  try {
    localStorage.setItem(ORBIT.store, choice);
  } catch {
    // 저장하지 못해도 이 창은 고른 보기로 선다 — 다음 시작만 기본으로 돌아간다.
  }
  if (choice === ORBIT.orbit && !agentGraphZoomTaken) setAgentGraphZoom(view, 1);
  agentOrbitRepaint(view);
}

function agentOrbitRepaint(view) {
  const model = agentGraphFullModel(view);
  if (model) paintAgentGraph(view, model);
  else wireAgentOrbitToggle(view);
}

/* 입체를 그릴 수 없는 까닭의 말. */
function agentOrbitBrokenWords() {
  return agentOrbitBroken === "lost"
    ? t("board.orbit.glLost", "그래픽 장치가 입체 그림을 놓아 카드로 보여 줍니다 — 돌아오면 다시 섭니다")
    : t("board.orbit.noGl", "이 창에서는 입체를 그릴 수 없어 카드로 보여 줍니다");
}

/* 툴바의 두 단추. 판마다 다시 맨다 — 복제된 판은 손을 들고 오지 않는다. 입체를 그릴 수
 * 없는 판에서 입체 단추는 누를 수 없는 채로(`aria-disabled` — 초점과 손은 받아 팁이 까닭을
 * 말한다) 서고, 눌린 것은 실제로 선 카드다. */
function wireAgentOrbitToggle(view) {
  const broken = !agentOrbitCapable();
  const shown = broken ? ORBIT.cards : agentOrbitChoice();
  for (const button of view.querySelectorAll("[data-relations-view]")) {
    const orbit = button.dataset.relationsView === ORBIT.orbit;
    writeAttribute(button, "aria-pressed", String(button.dataset.relationsView === shown));
    if (orbit) {
      writeAttribute(button, "aria-disabled", String(broken));
      if (broken) {
        const words = agentOrbitBrokenWords();
        writeAttribute(button, "data-tip", words);
        writeAttribute(button, "aria-label", words);
      } else {
        button.removeAttribute("data-tip");
        button.removeAttribute("aria-label");
      }
    }
    button.onclick = () => setAgentOrbitChoice(view, button.dataset.relationsView);
  }
}

/* 이 판에 입체가 서는가, 그리고 그 옷. 고른 보기가 입체이고, 관계 그림이며, 그릴
 * 에이전트가 있고, 이 창이 입체를 그릴 수 있을 때만 — 빈 판은 카드 그림의 빈 문장과 창고
 * 띠가 답한다. 판의 클래스 `is-orbit`과 묻는 손 `agentOrbitShowing`은 실시간 지도·맞춤·
 * 초점이 함께 읽는 경계다. */
function dressAgentOrbitMode(view, model, taskMode) {
  const orbit = !taskMode && agentOrbitChoice() === ORBIT.orbit && (model?.agents.length ?? 0) > 0
    && agentOrbitCapable();
  view.classList.toggle("is-orbit", orbit);
  writeHidden(view.querySelector(".agent-graph-scroll"), orbit);
  writeHidden(view.querySelector(".agent-orbit"), !orbit);
  const hint = view.querySelector(".agent-graph-gesture-hint");
  if (hint) {
    writeAttribute(hint, "data-i18n", orbit ? "board.orbit.hint" : "board.graph.gestureHint");
    writeTextContent(hint, orbit
      ? t("board.orbit.hint", "캐릭터나 이름표를 누르면 상세가 열립니다")
      : t("board.graph.gestureHint", "카드와 연결선을 눌러보세요"));
  }
  if (!taskMode) wireAgentOrbitToggle(view);
  /* 카드로 돌아간 판은 라벨과 GPU의 모양을 내려놓는다 — 보이지 않는 버튼 수십이 카드
   * 그림의 요소 수와 탭 순서에 얹히지 않게. 다시 서는 판이 한 번 짓는다. */
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
/* 시험과 보고가 읽는 수. 늘기만 한다. `agentOrbitProjections`는 이름표를 다시 앉힌(모든
 * 자리를 화면으로 다시 투영한) 횟수 — 카메라가 멈춘 프레임은 세지 않는다. `agentOrbitBuilds`는
 * GPU의 모양(판·레일·인스턴스)을 다시 지은 횟수. */
let agentOrbitApplied = 0;
let agentOrbitFrames = 0;
let agentOrbitTicks = 0;
let agentOrbitPulses = 0;
let agentOrbitInkReads = 0;
let agentOrbitProjections = 0;
let agentOrbitBuilds = 0;

/* 여는 판의 카메라: 표의 자리에서, 저절로 흔들린다. */
function agentOrbitCameraAtRest() {
  return { yaw: ORBIT.camera.yaw, pitch: ORBIT.camera.pitch, spin: true, turning: 0, tilting: 0, sway: 0 };
}

function agentOrbitState(view) {
  const held = agentOrbitStates.get(view);
  if (held) return held;
  const stage = view.querySelector(".agent-orbit");
  const canvas = stage?.querySelector(".agent-orbit-canvas");
  const host = stage?.querySelector(".agent-orbit-labels");
  if (!stage || !canvas || !host || !agentOrbitCapable()) return null;
  /* 복제된 판(`docHost`)은 첫 판의 라벨을 죽은 마크업으로 들고 온다. */
  host.replaceChildren();
  const state = {
    view, stage, canvas, host,
    glance: stage.querySelector(".agent-orbit-glance"),
    legend: stage.querySelector(".agent-orbit-key"),
    width: 0, height: 0, dpr: 1,
    signature: null, shape: null, selected: null, hovered: null, focused: null,
    bodies: new Map(), agents: [], plates: [], rails: [], links: [],
    seats: new Map(), rows: new Map(), columns: new Map(), lefts: new Map(),
    fit: 1, zoom: 1, scale: 1, box: null, bounds: null, named: 0, tags: [],
    camera: agentOrbitCameraAtRest(),
    hold: 1, holdTarget: 1, clock: 0,
    inks: null, theme: ORBIT.themes.dark, chip: 0, reserved: [], measured: false,
    lens: null, placed: false,
    gl: null, scene: null, warm: null,
    flowing: false, pulsing: false,
    shownAtApply: false, dirty: true,
  };
  state.gl = agentOrbitRenderer(state);
  if (!state.gl) {
    agentOrbitBroken = "unsupported";
    return null;
  }
  agentOrbitStates.set(view, state);
  agentOrbitViews.add(view);
  wireAgentOrbitStage(state);
  watchGraphResize(stage, () => agentOrbitResized(view));
  agentOrbitWatchTheme();
  return state;
}

/* 판의 렌더러 하나 — 판이 사는 동안 하나다(토글을 오갈 때마다 컨텍스트를 새로 짓지 않는다).
 * 빛번짐의 타깃도 여기 산다. 부동소수 렌더 타깃이 없으면 빛번짐 없이 화면에 바로 그린다. */
function agentOrbitRenderer(state) {
  const T = window.THREE;
  let renderer = null;
  try {
    renderer = new T.WebGLRenderer({ canvas: state.canvas, antialias: false, alpha: false });
  } catch {
    return null;
  }
  if (!renderer.capabilities.isWebGL2) {
    renderer.dispose();
    return null;
  }
  renderer.outputColorSpace = T.SRGBColorSpace;
  renderer.toneMapping = T.ACESFilmicToneMapping;
  renderer.toneMappingExposure = ORBIT.render.exposure;
  renderer.shadowMap.enabled = true;
  renderer.shadowMap.type = T.PCFSoftShadowMap;
  renderer.info.autoReset = false;
  const floats = renderer.extensions.has("EXT_color_buffer_float")
    || renderer.extensions.has("EXT_color_buffer_half_float");
  const gl = { T, renderer, bloom: floats ? agentOrbitBloom(T) : null, calls: 0, sceneCalls: 0, triangles: 0,
    lost: () => agentOrbitLost(state), restored: () => agentOrbitRestored(state) };
  state.canvas.addEventListener("webglcontextlost", gl.lost);
  state.canvas.addEventListener("webglcontextrestored", gl.restored);
  return gl;
}

/* 빛번짐 한 벌: 장면 타깃(다중 표본), 문턱, 다섯 단의 가로·세로 흐림, 합성 — 시안의 것 그대로. */
function agentOrbitBloom(T) {
  const target = (options = {}) => new T.WebGLRenderTarget(1, 1, { type: T.HalfFloatType, depthBuffer: false,
    ...options });
  const quad = new T.Mesh(new T.PlaneGeometry(2, 2));
  quad.frustumCulled = false;
  const shots = new T.Scene();
  shots.add(quad);
  const flat = { vertexShader: ORBIT.shader.quad, depthTest: false, depthWrite: false };
  return {
    quad, shots, lens: new T.OrthographicCamera(-1, 1, 1, -1, 0, 1),
    scene: target({ depthBuffer: true, samples: ORBIT.render.samples }),
    high: target(),
    across: ORBIT.render.kernels.map(() => target()),
    down: ORBIT.render.kernels.map(() => target()),
    threshold: new T.ShaderMaterial({ ...flat, fragmentShader: ORBIT.shader.threshold,
      uniforms: { tDiffuse: { value: null }, threshold: { value: 1 }, knee: { value: ORBIT.render.knee } } }),
    blur: ORBIT.render.kernels.map((kernel) => new T.ShaderMaterial({ ...flat, defines: { KERNEL: kernel },
      fragmentShader: ORBIT.shader.blur,
      uniforms: { tDiffuse: { value: null }, texel: { value: new T.Vector2() }, dir: { value: new T.Vector2() },
        sigma: { value: kernel } } })),
    composite: new T.ShaderMaterial({ ...flat, fragmentShader: ORBIT.shader.composite,
      uniforms: { tBase: { value: null }, strength: { value: 1 }, radius: { value: 1 },
        ...Object.fromEntries(ORBIT.render.taps.map((tap) => [tap, { value: null }])) } }),
  };
}

/* 카드로 돌아간 판: 라벨과 GPU의 모양을 내려놓고 카메라를 표의 자리로. 다음에 서는 판은
 * 처음 서는 판이다 — 같은 입력이면 같은 자리에. 렌더러와 빛번짐의 타깃은 판이 사는 동안
 * 남는다(다시 서는 판이 컨텍스트를 새로 짓지 않게). */
function agentOrbitRelease(view) {
  const state = agentOrbitStates.get(view);
  if (!state) return;
  agentOrbitDropScene(state, { warm: true });
  if (state.bodies.size === 0 && state.host.childElementCount === 0) return;
  state.host.replaceChildren();
  state.bodies.clear();
  state.agents = [];
  state.plates = [];
  state.rails = [];
  state.links = [];
  state.seats = new Map();
  state.rows = new Map();
  state.columns = new Map();
  state.lefts = new Map();
  state.signature = null;
  state.shape = null;
  state.selected = null;
  state.hovered = null;
  state.focused = null;
  state.lens = null;
  state.tags = [];
  state.camera = agentOrbitCameraAtRest();
  state.hold = 1;
  state.holdTarget = 1;
}

/* GPU의 모양을 모두 돌려준다: 지은 기하·재질·인스턴스·그림자 지도. 유실된 컨텍스트에서도
 * 부를 수 있다(three.js가 빈 손으로 지난다). */
/* 장면을 내려놓는다. 카드로 돌아간 판(`warm`)은 재질만 쥐어 둔다: three는 쓰는 재질이 없는 셰이더를
 * 지우므로, 그대로 두면 다시 서는 판이 셰이더를 전부 새로 엮고 그 첫 장에서 창이 멈춘다(소프트웨어
 * GL에서 2.6초 남짓). 쥔 재질은 새 판이 한 장을 그려 같은 셰이더를 제 것으로 잡은 뒤 놓는다
 * (`agentOrbitCool`). 잃은 컨텍스트와 닫힌 판은 모두 돌려준다. */
function agentOrbitDropScene(state, { warm = false } = {}) {
  if (!warm) agentOrbitCool(state);
  const scene = state.scene;
  if (!scene) return;
  for (const one of [...scene.owned, ...scene.built]) {
    if (warm && one.isMaterial) (state.warm ??= []).push(one);
    else one.dispose();
  }
  scene.scene.clear();
  state.gl?.renderer.renderLists.dispose();
  state.scene = null;
  state.shape = null;
}

/* 쥐어 둔 재질을 놓는다 — 새 판이 그 셰이더를 잡은 뒤, 또는 판이 컨텍스트를 잃거나 떠날 때. */
function agentOrbitCool(state) {
  if (!state.warm) return;
  for (const material of state.warm) material.dispose();
  state.warm = null;
}

/* 컨텍스트를 잃었다: 이 창의 입체는 카드로 서고 토글이 까닭을 말한다. 모양은 버린다 —
 * 돌아온 컨텍스트에서 새로 짓는다. */
function agentOrbitLost(state) {
  /* 닫힌 판(문서를 떠난 둘째 칸의 복제)의 캔버스가 늦게 알리는 유실은 이 창의 일이 아니다. */
  if (agentOrbitStates.get(state.view) !== state) return;
  agentOrbitBroken = "lost";
  agentOrbitDropScene(state);
  state.signature = null;
  for (const view of agentOrbitViews) agentOrbitRepaint(view);
}

function agentOrbitRestored(state) {
  if (agentOrbitStates.get(state.view) !== state || agentOrbitBroken !== "lost") return;
  agentOrbitBroken = null;
  state.signature = null;
  state.dirty = true;
  for (const view of agentOrbitViews) agentOrbitRepaint(view);
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
 * 선택만 바뀐 판은 라벨 둘과 빛의 곱만 고친다. 렌더러를 짓지 못한 판은 카드로 다시 선다. */
function paintAgentOrbit(view, model) {
  if (!agentOrbitShowing(view)) return;
  const state = agentOrbitState(view);
  if (!state) {
    if (agentOrbitBroken) Promise.resolve().then(() => agentOrbitRepaint(view));
    return;
  }
  const now = Date.now();
  const shown = agentOrbitShown(state);
  const signature = agentOrbitSignature(model, now);
  if (signature !== state.signature || !state.scene) {
    agentOrbitApply(state, model, now, shown);
    state.signature = signature;
  }
  /* 이 판이 본 것이 기준선이다: 숨은 동안 적용된 전이는 돌아온 뒤 뛰지 않고, 보이는 동안의
   * 다음 전이는 뛴다. */
  state.shownAtApply = shown;
  agentOrbitDressSelection(state);
  agentOrbitWake();
}

/* 배율이 움직였다(`applyAgentGraphZoom`). 화면의 배율은 맞춤 × 사람의 배율이다. */
function agentOrbitZoomed(view) {
  const state = agentOrbitStates.get(view);
  /* 판마다 지나는 길이다(`paintAgentGraph`가 배율을 되살린다) — 배율이 그대로면 아무것도
   * 하지 않는다: 움직임을 줄인 판에서 정지 화면을 한 장 더 그리는 것은 상태가 바뀌지 않은
   * 그림이다. */
  if (!state || !agentOrbitShowing(view) || state.scale === state.fit * agentGraphZoom) return;
  state.zoom = agentGraphZoom;
  state.scale = state.fit * agentGraphZoom;
  state.dirty = true;
  agentOrbitWake();
}

/* 「전체 보기」와 두 번 누르기: 배율 1, 카메라는 여는 판의 자리로 돌아가 다시 흔들린다. */
function agentOrbitFit(view) {
  const state = agentOrbitStates.get(view);
  setAgentGraphZoom(view, 1);
  if (!state) return;
  state.zoom = agentGraphZoom;
  state.scale = state.fit * agentGraphZoom;
  Object.assign(state.camera, { yaw: ORBIT.camera.yaw, pitch: ORBIT.camera.pitch, spin: true, turning: 0,
    tilting: 0 });
  state.dirty = true;
  agentOrbitWake();
}

/* 고른 에이전트의 라벨에 초점을 — 카드 그림의 `focusAgentGraphSelection`이 입체에서 하는 일.
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
  const dpr = Math.min(window.devicePixelRatio || 1, ORBIT.render.pixelCap);
  if (width === state.width && height === state.height && dpr === state.dpr) return;
  state.width = width;
  state.height = height;
  state.dpr = dpr;
  agentOrbitSize(state);
  agentOrbitFitCamera(state);
  state.measured = false;
  state.dirty = true;
  agentOrbitWake();
}

/* 캔버스와 빛번짐 타깃의 크기(기기 픽셀). 빛번짐의 단은 반씩 줄어든다. */
function agentOrbitSize(state) {
  const { renderer, bloom } = state.gl;
  if (state.width <= 0 || state.height <= 0) return;
  renderer.setPixelRatio(state.dpr);
  renderer.setSize(state.width, state.height, false);
  if (!bloom) return;
  const wide = Math.round(state.width * state.dpr);
  const tall = Math.round(state.height * state.dpr);
  bloom.scene.setSize(wide, tall);
  bloom.high.setSize(Math.max(1, wide >> 1), Math.max(1, tall >> 1));
  let fold = 1;
  for (let at = 0; at < bloom.across.length; at += 1) {
    fold *= 2;
    const across = Math.max(1, Math.round(wide / fold));
    const down = Math.max(1, Math.round(tall / fold));
    bloom.across[at].setSize(across, down);
    bloom.down[at].setSize(across, down);
  }
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
      entry.workspace?.path ?? "", entry.card.parent ?? "", entry.searchMatch ? "match" : "",
      agentGraphIdentity(entry.card), entry.card.ledger ?? "", entry.card.agent ?? "",
    ].join("\u001f"));
  }
  for (const edge of model.overlayData?.mail ?? []) {
    parts.push([edge.key, edge.unread > 0 ? "unread" : "", agentOrbitFresh(edge, now) ? "fresh" : ""]
      .join("\u001f"));
  }
  for (const relation of agentOrbitDependencies(model)) parts.push(`${relation.from}>${relation.to}`);
  return parts.join("\u001e");
}

/* FNV-1a를 0..1로. 톤·위상·먼지가 같은 입력에 같은 답을 받는다. */
function agentOrbitUnit(key) {
  return knowledgeHash(key) / ORBIT.hashSpan;
}

/* 모델 한 판을 판·자리·레일·우편으로. 부르는 쪽은 서명이 움직였을 때만 부른다. */
function agentOrbitApply(state, model, now, shown) {
  agentOrbitApplied += 1;
  const pulsing = shown && state.shownAtApply;
  const started = performance.now();
  const entries = model.agents.filter((entry) => entry.workspace);
  const byPane = new Map(entries.map((entry) => [entry.card.pane, entry]));

  /* 부모: 그림에 있는 부모만. 고리처럼 이어진 계보는 끊는다. */
  const parentOf = new Map();
  for (const entry of entries) {
    const parent = byPane.get(entry.card.parent);
    parentOf.set(entry.key, parent && parent !== entry ? parent : null);
  }
  for (const entry of entries) {
    const seen = new Set([entry.key]);
    for (let at = parentOf.get(entry.key); at; at = parentOf.get(at.key)) {
      if (seen.has(at.key)) {
        parentOf.set(entry.key, null);
        break;
      }
      seen.add(at.key);
    }
  }

  /* 판: 에이전트가 있는 워크스페이스마다 하나. 메인 워크스페이스(경로가 프로젝트의 경로인
   * 것)가 본진이다. */
  const bodies = new Map();
  for (const entry of entries) {
    const workspace = entry.workspace;
    let plate = bodies.get(workspace.key);
    if (!plate) {
      plate = state.bodies.get(workspace.key) ?? { key: workspace.key, label: null, seen: agentOrbitSeen() };
      Object.assign(plate, { kind: "workspace", workspace, name: workspace.label,
        deck: workspace.path === workspace.project?.path, members: [], matched: false, dim: false });
      bodies.set(workspace.key, plate);
    }
    plate.matched ||= entry.searchMatch !== false;
  }
  for (const entry of entries) {
    const known = ORBIT.states[entry.state] ?? ORBIT.states.idle;
    const parent = parentOf.get(entry.key);
    const plate = bodies.get(entry.workspace.key);
    const body = state.bodies.get(entry.key) ?? { key: entry.key, label: null, burstAt: null,
      seen: agentOrbitSeen(), face: null };
    const before = body.state;
    const family = ORBIT.families[entry.card.agent] ?? ORBIT.families.other;
    Object.assign(body, {
      kind: parent && parent.workspace?.key === entry.workspace.key ? "child" : "agent",
      entry, state: entry.state, known, family, plate, parentKey: parent?.key ?? null, children: [],
      unit: agentOrbitUnit(entry.key),
      name: agentGraphIdentity(entry.card),
      searchDim: model.searchActive && entry.searchMatch === false,
    });
    if (pulsing && before === "working" && entry.state !== "working") {
      body.burstAt = started;
      agentOrbitPulses += 1;
    }
    plate.members.push(body);
    bodies.set(entry.key, body);
  }
  for (const body of bodies.values()) {
    if (body.kind !== "workspace" && body.parentKey) bodies.get(body.parentKey).children.push(body);
  }
  for (const plate of bodies.values()) {
    if (plate.kind === "workspace") plate.dim = model.searchActive && !plate.matched;
  }
  for (const body of bodies.values()) {
    if (body.kind === "workspace") continue;
    body.lead = body.plate.deck && body.children.length > 0;
    body.scale = body.lead ? ORBIT.bot.scale.lead : body.plate.deck ? ORBIT.bot.scale.deck
      : body.kind === "child" ? ORBIT.bot.scale.child : ORBIT.bot.scale.agent;
    body.lift = body.lead ? ORBIT.bot.dais.lift : 0;
  }

  state.bodies = bodies;
  agentOrbitSeatAll(state);
  state.agents = [...bodies.values()].filter((body) => body.kind !== "workspace");
  state.plates = [...bodies.values()].filter((body) => body.kind === "workspace");

  /* 레일: 부모 → 자식 하나씩, 과업 의존 하나씩. 우편: 두 끝이 다 그림에 있는 링크. */
  const rails = [];
  for (const body of state.agents) {
    if (body.parentKey) rails.push({ key: `rail:${body.parentKey}>${body.key}`, kind: "spawned",
      from: bodies.get(body.parentKey), to: body });
  }
  for (const relation of agentOrbitDependencies(model)) {
    const from = bodies.get(relation.from);
    const to = bodies.get(relation.to);
    if (from && to && from !== to) rails.push({ key: `dependency:${from.key}>${to.key}`, kind: "dependency", from, to });
  }
  const heldLinks = new Map(state.links.map((link) => [link.key, link]));
  const links = [];
  for (const edge of model.overlayData?.mail ?? []) {
    const from = bodies.get(edge.from);
    const to = bodies.get(edge.to);
    if (!from || !to || from === to || from.kind === "workspace" || to.kind === "workspace") continue;
    const fresh = agentOrbitFresh(edge, now);
    links.push({ key: edge.key, from, to, unread: edge.unread > 0, fresh, at: Number(edge.at) || 0,
      phase: heldLinks.get(edge.key)?.phase ?? agentOrbitUnit(edge.key), flowing: fresh, curve: null });
  }
  /* 흐르는 우편의 상한: 넘치면 오래된 링크의 것부터 접는다 — 링크 자체는 그대로 선다. */
  let flowing = 0;
  for (const link of [...links].sort((left, right) => right.at - left.at)) {
    if (link.flowing && flowing >= ORBIT.mail.cap) link.flowing = false;
    if (link.flowing) flowing += 1;
  }
  const heldRails = new Map(state.rails.map((rail) => [rail.key, rail]));
  state.rails = rails;
  state.links = links;
  state.flowing = flowing > 0 || state.agents.some((body) => body.known.flows);

  agentOrbitLabels(state);
  const shape = agentOrbitShapeSignature(state);
  if (!state.scene || shape !== state.shape) {
    agentOrbitBuild(state);
    state.shape = shape;
  } else {
    /* 모양이 그대로인 판: 레일은 같은 키로 같은 길이다 — 지은 관의 자리와 표본을 옮겨 든다. */
    for (const rail of rails) {
      const built = heldRails.get(rail.key);
      Object.assign(rail, { coreRange: built.coreRange, sheathRange: built.sheathRange, samples: built.samples });
    }
    for (const link of links) link.curve = agentOrbitArc(link);
  }
  agentOrbitGlance(state);

  /* 그림이 말하는 요약 — 그림을 볼 수 없는 사람에게 같은 수를. */
  const working = entries.filter((entry) => entry.state === "working").length;
  const attention = entries.filter((entry) => entry.state === "needs-attention").length;
  writeAttribute(state.canvas, "aria-label", t("board.orbit.summary",
    "에이전트 {{count}}개 · 작업 중 {{working}} · 확인 필요 {{attention}}",
    { count: model.agents.length, working, attention }));
  if (state.scene) state.scene.painted = false;
  state.placed = false;
  state.dirty = true;
}

function agentOrbitSeen() {
  return { x: 0, y: 0, depth: 0 };
}

/* ---- 자리 -------------------------------------------------------------------- */

/* 판의 자리들. 이미 앉은 키는 제 자리 번호를 지키고, 떠난 키의 자리는 비며, 새 키는
 * 빈자리(하위 에이전트는 부모에게 가장 가까운 빈자리)나 끝자리를 받는다. 판의 줄(z)과
 * 열(x)도 처음 받은 것을 지킨다 — 새 판은 제 무더기의 앞쪽 끝에 붙는다. */
function agentOrbitSeatAll(state) {
  const plates = [...state.bodies.values()].filter((body) => body.kind === "workspace");
  const keys = new Set(plates.map((plate) => plate.key));
  for (const key of [...state.rows.keys()]) {
    if (!keys.has(key)) {
      state.rows.delete(key);
      state.seats.delete(key);
      state.columns.delete(key);
      state.lefts.delete(key);
    }
  }

  /* 섬의 열: 계보의 깊이. 다른 판의 부모에게서 지시를 받은 섬은 그 판의 열 + 1이다(본진은 0).
   * 한 번 받은 열은 지킨다. */
  const depthOf = (plate, visiting = new Set()) => {
    if (plate.deck) return 0;
    const held = state.columns.get(plate.key);
    if (held !== undefined) return held;
    if (visiting.has(plate.key)) return 1;
    visiting.add(plate.key);
    let deepest = 1;
    for (const member of plate.members) {
      const parent = member.parentKey ? state.bodies.get(member.parentKey) : null;
      if (parent && parent.plate !== plate) deepest = Math.max(deepest, depthOf(parent.plate, visiting) + 1);
    }
    visiting.delete(plate.key);
    return deepest;
  };
  for (const plate of plates) {
    if (!plate.deck && !state.columns.has(plate.key)) state.columns.set(plate.key, depthOf(plate));
  }

  /* 새 판의 줄: 본진은 본진끼리, 섬은 섬끼리 한 무더기로 쌓인다. 처음 선 무더기는 가운데에
   * 맞춰 서고, 뒤에 온 판은 앞쪽 끝에 붙는다. 섬의 차례는 계보를 따라 — 부모 섬 뒤에 그
   * 자식 섬. */
  const fresh = plates.filter((plate) => !state.rows.has(plate.key));
  const byKey = (left, right) => left.key.localeCompare(right.key);
  const islandOrder = [];
  const islands = fresh.filter((plate) => !plate.deck);
  const childIslands = (plate) => islands.filter((one) => one !== plate && one.members.some((member) => {
    const parent = member.parentKey ? state.bodies.get(member.parentKey) : null;
    return parent?.plate === plate;
  })).sort(byKey);
  const walk = (plate) => {
    if (islandOrder.includes(plate)) return;
    islandOrder.push(plate);
    for (const child of childIslands(plate)) walk(child);
  };
  for (const plate of [...islands].sort((left, right) =>
    (state.columns.get(left.key) - state.columns.get(right.key)) || byKey(left, right))) {
    if (state.columns.get(plate.key) === 1) walk(plate);
  }
  for (const plate of [...islands].sort(byKey)) walk(plate);
  const decks = fresh.filter((plate) => plate.deck).sort(byKey);
  for (const [group, isDeck] of [[decks, true], [islandOrder, false]]) {
    if (group.length === 0) continue;
    const built = group.map((plate) => ({ plate, size: agentOrbitPlateStart(plate, isDeck) }));
    const stack = [...state.rows.entries()].filter(([key]) => state.bodies.get(key)?.deck === isDeck);
    const gap = isDeck ? ORBIT.flow.row : agentOrbitRowGap();
    let back = stack.length > 0
      ? Math.max(...stack.map(([, row]) => row.back + row.depth)) + gap
      : -(built.reduce((total, one) => total + one.size.depth, 0) + gap * (built.length - 1)) / 2;
    for (const { plate, size } of built) {
      state.rows.set(plate.key, { back, depth: size.depth, rows: size.rows, cols: size.cols, start: size.cols });
      back += size.depth + gap;
    }
  }

  /* 섬의 왼쪽 끝: 첫 열은 본진에서 틈만큼, 그다음 열은 앞 열 섬들의 가장 오른쪽 끝에서 틈만큼 — 지시를
   * 받은 섬은 지시한 섬의 오른쪽 끝보다 오른쪽에 선다. 앞 열의 끝은 섬마다 자랄 몫(`island.growth`열)을
   * 넣어 잰다: 그 몫 안에서 자라는 섬은 다음 열에 닿지 않고 아무도 움직이지 않는다. 몫을 넘어 자란 섬이
   * 있으면 그 뒤의 열이 그만큼 오른쪽으로 비킨다 — 겹치지 않는 것이 먼저다. 한 번 받은 끝은 지키고,
   * 비킬 때만 커진다. */
  for (const plate of plates) {
    if (plate.deck) agentOrbitSeatPlate(state, plate);
  }
  const standing = plates.filter((plate) => !plate.deck);
  const deepest = Math.max(0, ...standing.map((plate) => state.columns.get(plate.key)));
  let edge = null;
  for (let column = 1; column <= deepest; column += 1) {
    const left = edge === null ? ORBIT.flow.gap : edge + ORBIT.flow.column;
    for (const plate of standing.filter((one) => state.columns.get(one.key) === column)) {
      state.lefts.set(plate.key, Math.max(state.lefts.get(plate.key) ?? left, left));
      agentOrbitSeatPlate(state, plate);
      const row = state.rows.get(plate.key);
      const end = state.lefts.get(plate.key)
        + agentOrbitIslandWidth(Math.max(row.cols, (row.start ?? row.cols) + ORBIT.island.growth));
      edge = edge === null ? end : Math.max(edge, end);
    }
  }
}

/* 섬의 폭: 여백 둘, 자리들, 오른쪽 끝의 화면 자리. */
function agentOrbitIslandWidth(cols) {
  return ORBIT.island.marginX * 2 + cols * ORBIT.island.seat + ORBIT.island.bay;
}

/* 섬의 줄 사이: 여는 카메라에서 앞 섬의 뒷줄 로봇(가장 큰 키)이 머리를 들어도 뒤 섬의 앞 옆면과
 * 두 테를 넘지 않고, 그 사이에 `clear`만큼 틈이 보이게. 화면에서 판의 x 모서리에 수직인 거리로
 * 잰다 — 세계의 z 한 단위와 높이 한 단위가 그 방향으로 얼마씩 가는가(`perZ`·`perY`). */
function agentOrbitRowGap() {
  const { yaw, pitch } = ORBIT.camera;
  const edge = Math.hypot(Math.cos(yaw), Math.sin(pitch) * Math.sin(yaw));
  const perZ = Math.sin(pitch) / edge;
  const perY = (Math.cos(pitch) * Math.cos(yaw)) / edge;
  const I = ORBIT.island;
  const tall = ORBIT.bot.tall * Math.max(ORBIT.bot.scale.agent, ORBIT.bot.scale.child);
  const reach = ((tall + I.height) * perY) / perZ - (I.marginZ + I.seat / 2);
  return Math.max(ORBIT.flow.row, reach + I.rim * 2 + ORBIT.flow.clear);
}

/* 새 판의 줄과 열: 처음 앉는 손님 수에 빈자리 하나를 더해 잰다. */
function agentOrbitPlateStart(plate, isDeck) {
  const wanted = plate.members.length + ORBIT.spare;
  if (isDeck) {
    const rows = Math.min(ORBIT.deck.maxRows, Math.max(ORBIT.deck.minRows, Math.ceil(wanted / ORBIT.deck.cols)));
    return { rows, cols: Math.max(ORBIT.deck.cols, Math.ceil(wanted / rows)),
      depth: ORBIT.deck.margin * 2 + rows * ORBIT.deck.seat };
  }
  const rows = Math.min(ORBIT.island.maxRows, Math.max(1, Math.ceil(wanted / ORBIT.island.perRow)));
  return { rows, cols: Math.max(ORBIT.island.minCols, Math.ceil(wanted / rows)),
    depth: ORBIT.island.marginZ * 2 + rows * ORBIT.island.seat };
}

/* 한 판의 손님들을 앉히고 판의 테두리를 잰다. 본진의 자리는 오른쪽 끝(x = 0)에서 왼쪽으로,
 * 섬의 자리는 제 열의 왼쪽 끝에서 오른쪽으로 번호를 받는다 — 판이 자라도 앉은 자리는 그대로다. */
function agentOrbitSeatPlate(state, plate) {
  const row = state.rows.get(plate.key);
  const held = state.seats.get(plate.key) ?? new Map();
  const seats = new Map();
  for (const member of plate.members) {
    if (held.has(member.key)) seats.set(member.key, held.get(member.key));
  }
  const taken = new Set(seats.values());
  const deck = plate.deck;
  const table = deck ? ORBIT.deck : ORBIT.island;
  const seat = deck ? ORBIT.deck.seat : ORBIT.island.seat;
  const cellOf = (index) => ({ col: Math.floor(index / row.rows), row: index % row.rows });
  /* 처음 앉는 판은 계보의 차례로(조율자 먼저, 부모 뒤에 제 자식), 나중에 온 손님은 키의 차례로. */
  const members = [...plate.members].sort((left, right) => left.key.localeCompare(right.key));
  const ordered = [];
  const place = (member) => {
    if (ordered.includes(member)) return;
    ordered.push(member);
    for (const child of members.filter((one) => one.parentKey === member.key)) place(child);
  };
  const leads = members.filter((member) => member.lead);
  for (const member of [...leads, ...members.filter((one) => !one.parentKey || one.kind !== "child")]) place(member);
  for (const member of members) place(member);
  for (const member of ordered) {
    if (seats.has(member.key)) continue;
    const parentSeat = member.kind === "child" ? seats.get(member.parentKey) : undefined;
    /* 빈자리는 판의 자리 수와 이미 앉은 가장 먼 자리의 다음, 그리고 그 너머 한 열 안에서 찾는다 — 한꺼번에
     * 여럿이 와도 자리가 모자라지 않는다. */
    let reach = row.rows * row.cols;
    for (const index of taken) reach = Math.max(reach, index + 1);
    const limit = reach + row.rows;
    let chosen = -1;
    let nearest = Infinity;
    for (let index = 0; index < limit; index += 1) {
      if (taken.has(index)) continue;
      if (parentSeat === undefined) {
        chosen = index;
        break;
      }
      const [here, there] = [cellOf(index), cellOf(parentSeat)];
      const distance = Math.hypot(here.col - there.col, here.row - there.row);
      if (distance < nearest) {
        nearest = distance;
        chosen = index;
      }
    }
    seats.set(member.key, chosen);
    taken.add(chosen);
  }
  state.seats.set(plate.key, seats);
  const cols = Math.max(row.cols, ...[...seats.values()].map((index) => cellOf(index).col + 1));
  row.cols = cols;
  const width = deck ? ORBIT.deck.margin * 2 + cols * seat : agentOrbitIslandWidth(cols);
  const left = deck ? -width : state.lefts.get(plate.key);
  Object.assign(plate, { left, right: left + width, back: row.back, front: row.back + row.depth,
    height: table.height, radius: table.radius, bevel: table.bevel, rim: table.rim, rimRadius: table.rimRadius,
    tube: table.tube });
  for (const member of plate.members) {
    const cell = cellOf(seats.get(member.key));
    const x = deck ? -(ORBIT.deck.margin + cell.col * seat + seat / 2)
      : left + ORBIT.island.marginX + cell.col * seat + seat / 2;
    const z = row.back + (deck ? ORBIT.deck.margin : ORBIT.island.marginZ) + cell.row * seat + seat / 2;
    member.world = [x, member.lift, z];
    member.cell = cell;
    member.leftmost = !deck && cell.col === 0;
  }
}

/* 모양의 서명: 판의 테두리, 앉은 자리, 레일의 두 끝, 조율자 — 이것이 그대로면 GPU의 모양도
 * 그대로다(상태만 바뀐 판은 색만 고친다). */
function agentOrbitShapeSignature(state) {
  const parts = [];
  for (const plate of state.plates) parts.push([plate.key, plate.left, plate.right, plate.back, plate.front].join(","));
  for (const body of state.agents) {
    parts.push([body.key, ...body.world, body.scale, body.lead ? "lead" : "", body.family].join(","));
  }
  for (const rail of state.rails) parts.push(rail.key);
  return parts.join("\u001e");
}

/* ---- GPU의 모양 --------------------------------------------------------------- */

/* 한 판의 장면: 한 번 짓는 것(바닥·빛·먼지·알갱이·로봇 부품의 기하와 재질)과 자리가 움직일
 * 때마다 다시 짓는 것(판·레일·인스턴스)으로 나뉜다. `owned`는 장면이 사는 동안의 것이고,
 * `built`는 다시 지을 때 먼저 돌려주는 것이다. */
function agentOrbitScene(state) {
  if (state.scene) return state.scene;
  const T = state.gl.T;
  const owned = [];
  const keep = (one) => {
    owned.push(one);
    return one;
  };
  const scene = new T.Scene();
  const camera = new T.OrthographicCamera(-1, 1, 1, -1, ORBIT.camera.near, ORBIT.camera.far);
  const B = ORBIT.bot;
  const standard = (look) => keep(new T.MeshStandardMaterial({ ...look }));
  const materials = {
    main: standard(ORBIT.material.main),
    light: standard(ORBIT.material.light),
    accent: standard(ORBIT.material.accent),
    metal: standard(ORBIT.material.metal),
    visor: standard(ORBIT.material.visor),
    pad: standard(ORBIT.material.pad),
    plate: keep(new T.MeshStandardMaterial({ ...ORBIT.material.plate, vertexColors: true })),
    glow: keep(new T.MeshBasicMaterial()),
    add: keep(new T.MeshBasicMaterial({ transparent: true, blending: T.AdditiveBlending, depthWrite: false })),
    rim: keep(new T.MeshBasicMaterial({ vertexColors: true })),
    rail: keep(new T.MeshBasicMaterial({ vertexColors: true })),
    sheath: keep(new T.MeshBasicMaterial({ vertexColors: true, transparent: true, opacity: ORBIT.material.sheath,
      blending: T.AdditiveBlending, depthWrite: false })),
    pick: keep(new T.MeshBasicMaterial({ visible: false })),
    /* 고른 고리는 더하지 않고 덮는다 — 흰 바닥에서 더한 빛은 보이지 않는다. */
    select: keep(new T.MeshBasicMaterial({ transparent: true, depthWrite: false })),
    beam: keep(new T.ShaderMaterial({ vertexShader: ORBIT.shader.instanced, fragmentShader: ORBIT.shader.beam,
      uniforms: { time: { value: 0 }, alpha: { value: 1 } }, transparent: true, blending: T.AdditiveBlending,
      depthWrite: false, side: T.DoubleSide })),
    keys: keep(new T.ShaderMaterial({ vertexShader: ORBIT.shader.instanced, fragmentShader: ORBIT.shader.keys,
      transparent: true, blending: T.AdditiveBlending, depthWrite: false, side: T.DoubleSide })),
    screen: keep(new T.ShaderMaterial({ vertexShader: ORBIT.shader.screenVertex, fragmentShader: ORBIT.shader.screen,
      uniforms: { time: { value: 0 } }, transparent: true, blending: T.AdditiveBlending, depthWrite: false,
      side: T.DoubleSide })),
    grid: keep(new T.ShaderMaterial({ vertexShader: ORBIT.shader.gridVertex, fragmentShader: ORBIT.shader.grid,
      vertexColors: true, transparent: true, blending: T.AdditiveBlending, depthWrite: false })),
  };
  const mic = new T.CatmullRomCurve3(B.mic.path.map((point) => new T.Vector3(point[0], point[1], point[2])));
  const geometry = {
    body: new T.CapsuleGeometry(B.body.radius, B.body.length, B.body.cap, B.body.radial),
    neck: new T.CylinderGeometry(B.neck.top, B.neck.bottom, B.neck.height, B.neck.radial),
    skull: agentOrbitSlab(T, B.head.w, B.head.d, B.head.h, B.head.radius, B.head.bevel),
    visor: new T.ShapeGeometry(agentOrbitRounded(T, B.visor.w, B.visor.h, B.visor.radius)),
    eye: new T.CapsuleGeometry(B.eye.radius, B.eye.length, B.eye.cap, B.eye.radial),
    stem: new T.CylinderGeometry(B.stem.radius, B.stem.radius, B.stem.height, B.stem.radial),
    tip: new T.SphereGeometry(B.tip.radius, B.tip.width, B.tip.height),
    tipRing: new T.TorusGeometry(B.tip.ring, B.tip.tube, B.tip.tubular, B.tip.around),
    ear: new T.CylinderGeometry(B.ear.radius, B.ear.radius, B.ear.height, B.ear.radial).rotateZ(ORBIT.quarter),
    arm: new T.CapsuleGeometry(B.arm.radius, B.arm.length, B.arm.cap, B.arm.radial),
    mitt: new T.SphereGeometry(B.mitt.radius, B.mitt.width, B.mitt.height),
    chest: new T.CircleGeometry(B.chest.radius, B.chest.segments),
    keys: new T.PlaneGeometry(B.keys.w, B.keys.d).rotateX(-ORBIT.quarter),
    pad: new T.CylinderGeometry(B.pad.top, B.pad.bottom, B.pad.height, B.pad.radial),
    padRing: new T.TorusGeometry(B.pad.ring, B.pad.tube, B.pad.tubular, B.pad.around).rotateX(ORBIT.quarter),
    pulse: new T.RingGeometry(B.pulse.inner, B.pulse.outer, B.pulse.segments).rotateX(-ORBIT.quarter),
    pick: new T.CylinderGeometry(B.pick.radius, B.pick.radius, B.pick.height, B.pick.radial),
    beam: new T.CylinderGeometry(B.beam.radius, B.beam.radius, B.beam.height, B.beam.radial, 1, true)
      .translate(0, B.beam.height / 2, 0),
    core: new T.CylinderGeometry(B.beam.core, B.beam.core, B.beam.height, B.beam.coreRadial, 1, true)
      .translate(0, B.beam.height / 2, 0),
    dais: new T.CylinderGeometry(B.dais.top, B.dais.bottom, B.dais.height, B.dais.radial),
    daisRing: new T.TorusGeometry(B.dais.ring, B.dais.tube, B.dais.tubular, B.dais.around).rotateX(ORBIT.quarter),
    halo: new T.TorusGeometry(B.halo.radius, B.halo.tube, B.halo.tubular, B.halo.around),
    haloOuter: new T.TorusGeometry(B.halo.outer, B.halo.outerTube, B.halo.tubular, B.halo.around),
    boom: new T.TubeGeometry(mic, B.mic.segments, B.mic.tube, B.mic.radial, false),
    mic: new T.SphereGeometry(B.mic.ball, B.mic.ballWidth, B.mic.ballHeight),
    select: new T.RingGeometry(B.select.inner, B.select.outer, B.select.segments).rotateX(-ORBIT.quarter),
    plate: new T.PlaneGeometry(1, 1).rotateX(-ORBIT.quarter),
  };
  for (const one of Object.values(geometry)) keep(one);

  /* 바닥: 옅어지는 격자와 판 밑의 빛 웅덩이(셰이더), 그림자를 받는 판. */
  const pools = Array.from({ length: ORBIT.ground.pools }, () => new T.Vector4(0, 0, 1, 0));
  const poolInks = Array.from({ length: ORBIT.ground.pools }, () => new T.Color(0, 0, 0));
  const ground = new T.Mesh(keep(new T.PlaneGeometry(ORBIT.ground.size, ORBIT.ground.size).rotateX(-ORBIT.quarter)),
    keep(new T.ShaderMaterial({ vertexShader: ORBIT.shader.groundVertex, fragmentShader: ORBIT.shader.ground,
      defines: { POOLS: ORBIT.ground.pools },
      uniforms: { base: { value: new T.Color() }, grid: { value: new T.Color() }, gridA: { value: 0 },
        centre: { value: new T.Vector2() }, fade: { value: new T.Vector2(1, 2) }, pools: { value: pools },
        poolCol: { value: poolInks } } })));
  ground.position.y = ORBIT.ground.y;
  const shadow = keep(new T.ShadowMaterial({ opacity: 1 }));
  const catcher = new T.Mesh(keep(new T.PlaneGeometry(ORBIT.ground.catcher, ORBIT.ground.catcher)
    .rotateX(-ORBIT.quarter)), shadow);
  catcher.position.y = ORBIT.ground.y + ORBIT.ground.lift;
  catcher.receiveShadow = true;

  const hemi = keep(new T.HemisphereLight());
  const key = keep(new T.DirectionalLight());
  const rim = keep(new T.DirectionalLight());
  key.castShadow = true;
  key.shadow.mapSize.set(ORBIT.light.map, ORBIT.light.map);
  key.shadow.bias = ORBIT.light.bias;
  key.shadow.normalBias = ORBIT.light.normalBias;

  /* 떠다니는 먼지와 빛 알갱이 — 점 두 벌. */
  const dustGeometry = keep(new T.BufferGeometry());
  const dustAt = new Float32Array(ORBIT.dust.count * ORBIT.axes);
  dustGeometry.setAttribute("position", new T.BufferAttribute(dustAt, ORBIT.axes));
  const dustSeed = new Float32Array(ORBIT.dust.count);
  const random = agentOrbitRandom(ORBIT.dust.seed);
  for (let at = 0; at < ORBIT.dust.count; at += 1) dustSeed[at] = random();
  dustGeometry.setAttribute("seed", new T.BufferAttribute(dustSeed, 1));
  const dustMaterial = keep(new T.ShaderMaterial({ vertexShader: ORBIT.shader.dustVertex, fragmentShader: ORBIT.shader.dust,
    defines: { FLOOR: ORBIT.dust.floor.toFixed(1), HEIGHT: ORBIT.dust.height.toFixed(1) },
    uniforms: { scale: { value: 1 }, time: { value: 0 }, color: { value: new T.Color() } }, transparent: true,
    blending: T.AdditiveBlending, depthWrite: false }));
  const dust = new T.Points(dustGeometry, dustMaterial);
  dust.frustumCulled = false;

  const points = ORBIT.comet.points;
  const cometGeometry = keep(new T.BufferGeometry());
  const cometAt = new T.BufferAttribute(new Float32Array(points * ORBIT.axes), ORBIT.axes).setUsage(T.DynamicDrawUsage);
  const cometInk = new T.BufferAttribute(new Float32Array(points * ORBIT.axes), ORBIT.axes).setUsage(T.DynamicDrawUsage);
  const cometSize = new T.BufferAttribute(new Float32Array(points), 1).setUsage(T.DynamicDrawUsage);
  const cometAlpha = new T.BufferAttribute(new Float32Array(points), 1).setUsage(T.DynamicDrawUsage);
  cometGeometry.setAttribute("position", cometAt);
  cometGeometry.setAttribute("color", cometInk);
  cometGeometry.setAttribute("size", cometSize);
  cometGeometry.setAttribute("alpha", cometAlpha);
  cometGeometry.setDrawRange(0, 0);
  const comets = new T.Points(cometGeometry, keep(new T.ShaderMaterial({ vertexShader: ORBIT.shader.cometVertex,
    fragmentShader: ORBIT.shader.comet, uniforms: { scale: { value: 1 } }, vertexColors: true, transparent: true,
    blending: T.AdditiveBlending, depthWrite: false })));
  comets.frustumCulled = false;

  const select = new T.Mesh(geometry.select, materials.select);
  select.visible = false;
  scene.add(ground, catcher, hemi, key, key.target, rim, dust, comets, select);

  /* 로봇 한 벌의 고정된 마디: 머리·몸에 붙는 부품의 자리. */
  const M = (x, y, z) => new T.Matrix4().makeTranslation(x, y, z);
  const joints = {
    neck: M(0, B.neck.y, 0), chest: M(0, B.chest.y, B.chest.z), keys: M(0, B.keys.y, B.keys.z),
    pad: M(0, B.pad.y, 0), padRing: M(0, B.pad.ringY, 0), skull: M(0, B.head.skull, 0),
    visor: M(0, B.visor.y, B.visor.z), stem: M(0, B.stem.y, 0), tip: M(0, B.tip.y, 0),
    tipRing: M(0, B.tip.ringY, 0), earLeft: M(-B.ear.x, B.ear.y, 0), earRight: M(B.ear.x, B.ear.y, 0),
    arm: M(0, B.arm.hang, 0), mitt: M(0, B.mitt.hang, 0), mic: M(B.mic.path[B.mic.path.length - 1][0],
      B.mic.path[B.mic.path.length - 1][1], B.mic.path[B.mic.path.length - 1][2]),
  };
  const scratch = {
    root: new T.Matrix4(), head: new T.Matrix4(), pivot: new T.Matrix4(), local: new T.Matrix4(),
    out: new T.Matrix4(), none: new T.Matrix4().makeScale(0, 0, 0), at: new T.Vector3(), size: new T.Vector3(),
    turn: new T.Quaternion(), euler: new T.Euler(), ink: new T.Color(), tailInk: new T.Color(), headInk: new T.Color(),
    point: new T.Vector3(), depth: new T.Vector3(), ray: new T.Raycaster(), pointer: new T.Vector2(),
  };
  state.scene = { T, scene, camera, owned, built: [], materials, geometry, ground, shadow, hemi, key, rim, dust,
    dustAt, comets, cometAt, cometInk, cometSize, cometAlpha, select, joints, scratch, meshes: null, painted: false };
  return state.scene;
}

/* 둥근 네모 한 장(x·y 평면, 가운데가 원점). */
function agentOrbitRounded(T, w, d, r) {
  const shape = new T.Shape();
  const x = -w / 2;
  const y = -d / 2;
  shape.moveTo(x + r, y);
  shape.lineTo(x + w - r, y);
  shape.quadraticCurveTo(x + w, y, x + w, y + r);
  shape.lineTo(x + w, y + d - r);
  shape.quadraticCurveTo(x + w, y + d, x + w - r, y + d);
  shape.lineTo(x + r, y + d);
  shape.quadraticCurveTo(x, y + d, x, y + d - r);
  shape.lineTo(x, y + r);
  shape.quadraticCurveTo(x, y, x + r, y);
  return shape;
}

/* 빗면을 친 둥근 판 — 윗면이 y = 0, 두께만큼 아래로. */
function agentOrbitSlab(T, w, d, h, r, bevel) {
  const slab = new T.ExtrudeGeometry(agentOrbitRounded(T, w, d, r), { depth: h - bevel * 2, bevelEnabled: true,
    bevelThickness: bevel, bevelSize: bevel, bevelSegments: ORBIT.shape.bevel, curveSegments: ORBIT.shape.curve });
  slab.rotateX(-ORBIT.quarter);
  slab.translate(0, -(h - bevel), 0);
  return slab;
}

/* 판의 테가 도는 길: 둥근 네모의 둘레를 고르게 딴 닫힌 곡선(높이 `y`). */
function agentOrbitOutline(T, w, d, r, y) {
  const points = agentOrbitRounded(T, w, d, r).getSpacedPoints(ORBIT.shape.outline).slice(0, -1)
    .map((point) => new T.Vector3(point.x, y, -point.y));
  return new T.CatmullRomCurve3(points, true, "centripetal");
}

/* 자리가 움직인 판: 판·레일·로봇의 인스턴스를 다시 짓는다. 먼저 지은 것은 돌려준다. */
function agentOrbitBuild(state) {
  agentOrbitBuilds += 1;
  const scene = agentOrbitScene(state);
  const T = scene.T;
  for (const one of scene.built) {
    scene.scene.remove(one.mesh ?? one);
    one.dispose();
  }
  scene.built = [];
  /* 지은 것 하나: 인스턴스는 제 버퍼를(기하는 장면이 든 부품을 함께 쓴다), 합친 모양은 제 기하를
   * 돌려준다. `own`은 그 밖에 이 메시만 쓰는 기하다. */
  const add = (mesh, own = null) => {
    scene.scene.add(mesh);
    scene.built.push({ mesh, dispose: () => {
      if (mesh.isInstancedMesh) mesh.dispose();
      else mesh.geometry.dispose();
      own?.dispose();
    } });
    return mesh;
  };

  /* 판: 빗면의 판(윗면과 옆면을 정점 색으로 가른다), 빛나는 테, 윗면의 격자 — 판마다 하나씩
   * 지어 셋으로 합친다(판이 몇이든 그리기 셋). */
  const slabs = [];
  const rims = [];
  const grids = [];
  for (const plate of state.plates) {
    const w = plate.right - plate.left;
    const d = plate.front - plate.back;
    const x = (plate.left + plate.right) / 2;
    const z = (plate.back + plate.front) / 2;
    const slab = agentOrbitSlab(T, w, d, plate.height, plate.radius, plate.bevel).translate(x, 0, z);
    const rim = new T.TubeGeometry(agentOrbitOutline(T, w + plate.rim, d + plate.rim, plate.rimRadius, -plate.bevel / 2),
      ORBIT.shape.outline, plate.tube, ORBIT.shape.rimRadial, true).translate(x, 0, z);
    const grid = new T.PlaneGeometry(w, d).rotateX(-ORBIT.quarter).translate(x, ORBIT.ground.lift, z);
    const corners = grid.attributes.position.count;
    const local = new Float32Array(corners * 2);
    const shape = new Float32Array(corners * ORBIT.axes);
    for (let at = 0; at < corners; at += 1) {
      local[at * 2] = grid.attributes.position.getX(at) - x;
      local[at * 2 + 1] = grid.attributes.position.getZ(at) - z;
      shape.set([w / 2, d / 2, plate.radius], at * ORBIT.axes);
    }
    grid.setAttribute("local", new T.BufferAttribute(local, 2));
    grid.setAttribute("shape", new T.BufferAttribute(shape, ORBIT.axes));
    grid.setAttribute("color", new T.BufferAttribute(new Float32Array(corners * ORBIT.axes), ORBIT.axes));
    plate.slab = { start: slabs.reduce((total, one) => total + one.attributes.position.count, 0),
      count: slab.attributes.position.count };
    plate.rimRange = { start: rims.reduce((total, one) => total + one.attributes.position.count, 0),
      count: rim.attributes.position.count };
    plate.gridRange = { start: grids.reduce((total, one) => total + one.attributes.position.count, 0),
      count: corners };
    slabs.push(slab);
    rims.push(rim);
    grids.push(grid);
  }
  const merge = (list, material, { shadow = false } = {}) => {
    const merged = T.mergeGeometries(list, false);
    for (const one of list) one.dispose();
    if (!merged.attributes.color) {
      merged.setAttribute("color", new T.BufferAttribute(new Float32Array(merged.attributes.position.count * ORBIT.axes),
        ORBIT.axes));
    }
    const mesh = new T.Mesh(merged, material);
    mesh.castShadow = shadow;
    mesh.receiveShadow = shadow;
    return add(mesh);
  };
  const plates = state.plates.length > 0 ? {
    slab: merge(slabs, scene.materials.plate, { shadow: true }),
    rim: merge(rims, scene.materials.rim),
    grid: merge(grids, scene.materials.grid),
  } : null;
  if (plates) plates.grid.renderOrder = 1;

  /* 레일: 관(부모 → 자식은 굵은 관과 덮개, 의존은 가는 관만) — 합친 관 하나, 합친 덮개 하나.
   * 알갱이가 쓰는 표본은 레일마다 든다. */
  const cores = [];
  const sheaths = [];
  for (const rail of state.rails) {
    const lineage = rail.kind === "spawned";
    const curve = new T.CatmullRomCurve3(agentOrbitRoute(state, rail).map((point) =>
      new T.Vector3(point[0], point[1], point[2])), false, "centripetal");
    const core = new T.TubeGeometry(curve, ORBIT.rail.segments, lineage ? ORBIT.rail.core : ORBIT.rail.dependency,
      ORBIT.rail.radial, false);
    rail.coreRange = { start: cores.reduce((total, one) => total + one.attributes.position.count, 0),
      count: core.attributes.position.count, radial: ORBIT.rail.radial };
    cores.push(core);
    if (lineage) {
      const sheath = new T.TubeGeometry(curve, ORBIT.rail.segments, ORBIT.rail.sheath, ORBIT.rail.sheathRadial, false);
      rail.sheathRange = { start: sheaths.reduce((total, one) => total + one.attributes.position.count, 0),
        count: sheath.attributes.position.count, radial: ORBIT.rail.sheathRadial };
      sheaths.push(sheath);
    } else rail.sheathRange = null;
    const spaced = curve.getSpacedPoints(ORBIT.rail.samples);
    rail.samples = new Float32Array(spaced.length * ORBIT.axes);
    spaced.forEach((point, at) => rail.samples.set([point.x, point.y, point.z], at * ORBIT.axes));
  }
  const rails = {
    core: cores.length > 0 ? merge(cores, scene.materials.rail) : null,
    sheath: sheaths.length > 0 ? merge(sheaths, scene.materials.sheath) : null,
  };
  if (rails.sheath) rails.sheath.renderOrder = 1;
  for (const link of state.links) link.curve = agentOrbitArc(link);

  /* 로봇: 부품마다 인스턴스 하나. 부품이 둘인 것(눈·귀·팔·손)은 에이전트마다 두 칸이다. */
  const count = state.agents.length;
  const leads = state.agents.filter((body) => body.lead);
  const islands = state.plates.filter((plate) => !plate.deck);
  const G = scene.geometry;
  const materials = scene.materials;
  const instanced = (geometry, material, many, { shadow = false, colored = false } = {}) => {
    const mesh = new T.InstancedMesh(geometry, material, Math.max(1, many));
    mesh.count = many;
    mesh.visible = many > 0;
    mesh.frustumCulled = false;
    mesh.castShadow = shadow;
    mesh.instanceMatrix.setUsage(T.DynamicDrawUsage);
    if (colored) {
      for (let at = 0; at < Math.max(1, many); at += 1) mesh.setColorAt(at, scene.scratch.ink.setRGB(0, 0, 0));
      mesh.instanceColor.setUsage(T.DynamicDrawUsage);
    }
    return add(mesh);
  };
  const screenGeometry = new T.PlaneGeometry(ORBIT.screen.w, ORBIT.screen.h);
  screenGeometry.setAttribute("look", new T.InstancedBufferAttribute(new Float32Array(Math.max(1, islands.length) * 2), 2));
  const meshes = {
    body: instanced(G.body, materials.light, count, { shadow: true, colored: true }),
    skull: instanced(G.skull, materials.main, count, { shadow: true, colored: true }),
    arm: instanced(G.arm, materials.main, count * 2, { shadow: true, colored: true }),
    ear: instanced(G.ear, materials.accent, count * 2, { shadow: true, colored: true }),
    mitt: instanced(G.mitt, materials.accent, count * 2, { shadow: true, colored: true }),
    neck: instanced(G.neck, materials.metal, count),
    stem: instanced(G.stem, materials.metal, count),
    visor: instanced(G.visor, materials.visor, count),
    pad: instanced(G.pad, materials.pad, count),
    eye: instanced(G.eye, materials.glow, count * 2, { colored: true }),
    tip: instanced(G.tip, materials.glow, count, { colored: true }),
    tipRing: instanced(G.tipRing, materials.glow, count, { colored: true }),
    chest: instanced(G.chest, materials.glow, count, { colored: true }),
    padRing: instanced(G.padRing, materials.glow, count, { colored: true }),
    pulse: instanced(G.pulse, materials.add, count, { colored: true }),
    keys: instanced(G.keys, materials.keys, count, { colored: true }),
    beam: instanced(G.beam, materials.beam, count, { colored: true }),
    core: instanced(G.core, materials.beam, count, { colored: true }),
    pick: instanced(G.pick, materials.pick, count),
    dais: instanced(G.dais, materials.pad, leads.length, { shadow: true }),
    daisRing: instanced(G.daisRing, materials.glow, leads.length, { colored: true }),
    halo: instanced(G.halo, materials.glow, leads.length, { colored: true }),
    haloOuter: instanced(G.haloOuter, materials.glow, leads.length, { colored: true }),
    boom: instanced(G.boom, materials.metal, leads.length),
    mic: instanced(G.mic, materials.glow, leads.length, { colored: true }),
    screen: add(new T.InstancedMesh(screenGeometry, materials.screen, Math.max(1, islands.length)), screenGeometry),
    plates: instanced(G.plate, materials.pick, state.plates.length),
  };
  meshes.screen.count = islands.length;
  meshes.screen.visible = islands.length > 0;
  meshes.screen.frustumCulled = false;
  for (let at = 0; at < Math.max(1, islands.length); at += 1) meshes.screen.setColorAt(at, scene.scratch.ink.setRGB(0, 0, 0));
  meshes.pad.receiveShadow = true;
  meshes.dais.receiveShadow = true;
  for (const mesh of [meshes.beam, meshes.core, meshes.pulse, meshes.keys, meshes.screen]) mesh.renderOrder = 1;
  scene.meshes = meshes;
  /* 자세가 프레임마다 쓰는 것 — 판·단·화면·고르기의 자리는 지을 때 한 번 쓰고 처음 그릴 때 오른다. */
  scene.posed = Object.values(meshes).filter((mesh) => ![meshes.pick, meshes.plates, meshes.screen, meshes.dais,
    meshes.daisRing].includes(mesh));
  scene.plates = plates;
  scene.rails = rails;
  scene.leads = leads;
  scene.islands = islands;
  state.agents.forEach((body, index) => {
    body.index = index;
  });
  leads.forEach((body, index) => {
    body.leadIndex = index;
  });

  /* 고정된 것은 여기서 한 번: 누를 자리(로봇과 판), 본진의 단, 섬의 화면. */
  const { scratch } = scene;
  for (const body of state.agents) {
    const [x, y, z] = body.world;
    scratch.out.compose(scratch.at.set(x, y + (ORBIT.bot.pick.height * body.scale) / 2, z), scratch.turn.identity(),
      scratch.size.set(body.scale, body.scale, body.scale));
    meshes.pick.setMatrixAt(body.index, scratch.out);
  }
  state.plates.forEach((plate, index) => {
    scratch.out.compose(scratch.at.set((plate.left + plate.right) / 2, ORBIT.ground.lift, (plate.back + plate.front) / 2),
      scratch.turn.identity(), scratch.size.set(plate.right - plate.left, 1, plate.front - plate.back));
    meshes.plates.setMatrixAt(index, scratch.out);
    plate.index = index;
  });
  for (const body of leads) {
    const [x, , z] = body.world;
    meshes.dais.setMatrixAt(body.leadIndex, scratch.out.makeTranslation(x, ORBIT.bot.dais.y, z));
    meshes.daisRing.setMatrixAt(body.leadIndex, scratch.out.makeTranslation(x, ORBIT.bot.dais.ringY, z));
  }
  islands.forEach((plate, index) => {
    plate.screenIndex = index;
    scratch.out.compose(scratch.at.set(plate.right - ORBIT.island.bay / 2, ORBIT.screen.y, (plate.back + plate.front) / 2),
      scratch.turn.setFromEuler(scratch.euler.set(0, ORBIT.camera.yaw * ORBIT.screen.turn, 0)),
      scratch.size.set(1, 1, 1));
    meshes.screen.setMatrixAt(index, scratch.out);
  });

  agentOrbitCometRoom(state);
  agentOrbitBounds(state);
  agentOrbitFitCamera(state);
  state.measured = false;
  scene.painted = false;
  agentOrbitPose(state, 0, performance.now(), true);
}

/* 한 레일의 길(세계의 점들). 한 판 안의 부모와 자식은 짧게 솟는 호로 잇는다. 판을 건너는
 * 레일은 부모의 판을 떠나(본진의 조율자는 오른쪽으로 곧장, 섬의 부모는 앞쪽 가장자리를 돌아),
 * 부채꼴의 두 점을 지나, 자식의 섬에 든다 — 줄의 맨 왼쪽 자리는 곧장, 아니면 섬 뒤쪽의 길로.
 * 의존은 두 몸에서 솟아 가운데가 들린 호다. */
function agentOrbitRoute(state, rail) {
  const R = ORBIT.rail;
  const { from, to } = rail;
  const [ax, ay, az] = from.world;
  const [bx, by, bz] = to.world;
  if (rail.kind === "dependency") {
    const reach = Math.hypot(bx - ax, bz - az);
    return [[ax, ay + R.rise * from.scale, az],
      [(ax + bx) / 2, Math.max(ay, by) + R.lift + reach * R.reach, (az + bz) / 2],
      [bx, by + R.rise * to.scale, bz]];
  }
  if (from.plate === to.plate) {
    const reach = Math.hypot(bx - ax, bz - az) || 1;
    const [ux, uz] = [(bx - ax) / reach, (bz - az) / reach];
    return [[ax + ux * R.stop * from.scale, R.y, az + uz * R.stop * from.scale],
      [(ax + bx) / 2, R.y + R.hop, (az + bz) / 2],
      [bx - ux * R.stop * to.scale, R.y, bz - uz * R.stop * to.scale]];
  }
  const A = from.plate;
  const B = to.plate;
  const leave = A.deck ? [[ax + R.exit * from.scale, R.y, az]]
    : [[ax, R.y, az + R.stop * from.scale], [ax, R.y, A.front + R.lane], [A.right + R.lane, R.y, A.front + R.lane]];
  let arrive;
  if (B.deck) arrive = [[B.right + R.lane, R.y, bz], [bx + R.stop * to.scale, R.y, bz]];
  else if (to.leftmost) arrive = [[B.left + R.entry, R.y, bz], [bx - R.stop * to.scale, R.y, bz]];
  else {
    const lane = B.back + ORBIT.island.marginZ / 2;
    arrive = [[B.left + R.entry, R.y, lane], [bx, R.y, lane], [bx, R.y, bz - R.stop * to.scale]];
  }
  const [sx, , sz] = leave[leave.length - 1];
  const [ex, , ez] = arrive[0];
  const fan = R.fan.map((part, at) => [sx + (ex - sx) * part, R.y, sz + (ez - sz) * R.fanZ[at]]);
  return [...leave, ...fan, ...arrive];
}

/* 빛 알갱이의 자리: 레일마다 `perRail`개와 흐르는 우편의 상한, 알갱이마다 머리 하나와 꼬리 — 모자라면
 * 늘린다(줄이지 않는다). 60 판의 작업 중 자식 60이 우편을 밀어내지 않게. */
function agentOrbitCometRoom(state) {
  const scene = state.scene;
  const C = ORBIT.comet;
  let rails = 0;
  for (const rail of state.rails) {
    if (rail.kind === "spawned") rails += 1;
  }
  const wanted = (rails * C.perRail + ORBIT.mail.cap) * (C.tail + 1);
  if (wanted <= scene.cometAt.count) return;
  const T = scene.T;
  const geometry = scene.comets.geometry;
  geometry.dispose();
  const grow = (attribute) => new T.BufferAttribute(new Float32Array(wanted * attribute.itemSize), attribute.itemSize)
    .setUsage(T.DynamicDrawUsage);
  Object.assign(scene, { cometAt: grow(scene.cometAt), cometInk: grow(scene.cometInk), cometSize: grow(scene.cometSize),
    cometAlpha: grow(scene.cometAlpha) });
  geometry.setAttribute("position", scene.cometAt);
  geometry.setAttribute("color", scene.cometInk);
  geometry.setAttribute("size", scene.cometSize);
  geometry.setAttribute("alpha", scene.cometAlpha);
  geometry.setDrawRange(0, 0);
}

/* 우편의 호: 보낸 쪽의 가슴에서 받는 쪽의 가슴까지, 가운데가 들린 이차 곡선의 세 점. */
function agentOrbitArc(link) {
  const [ax, ay, az] = link.from.world;
  const [bx, by, bz] = link.to.world;
  const reach = Math.hypot(bx - ax, bz - az);
  const a = [ax, ay + ORBIT.mail.rise * link.from.scale, az];
  const b = [bx, by + ORBIT.mail.rise * link.to.scale, bz];
  return [a, [(ax + bx) / 2, Math.max(a[1], b[1]) + ORBIT.mail.arc + reach * ORBIT.mail.bow, (az + bz) / 2], b];
}

/* 이차 곡선 위의 한 점(`at` 0 → 1), `into`에. */
function agentOrbitAlong(curve, at, into) {
  const rest = 1 - at;
  for (let axis = 0; axis < into.length; axis += 1) {
    into[axis] = rest * rest * curve[0][axis] + 2 * rest * at * curve[1][axis] + at * at * curve[2][axis];
  }
  return into;
}

/* 그림의 테두리: 판의 네 모서리(윗면과 밑면)와 로봇의 머리 — 맞춤이 재는 점들. 카메라의 과녁,
 * 바닥 격자의 가운데, 그림자 카메라, 먼지의 자리가 이것을 따른다. */
function agentOrbitBounds(state) {
  const scene = state.scene;
  const T = scene.T;
  const points = [];
  for (const plate of state.plates) {
    for (const x of [plate.left - plate.rim, plate.right + plate.rim]) {
      for (const z of [plate.back - plate.rim, plate.front + plate.rim]) {
        points.push(new T.Vector3(x, 0, z), new T.Vector3(x, -plate.height, z));
      }
    }
  }
  for (const body of state.agents) {
    points.push(new T.Vector3(body.world[0], body.world[1] + ORBIT.bot.tall * body.scale, body.world[2]));
  }
  const box = new T.Box3().setFromPoints(points);
  const centre = box.getCenter(new T.Vector3());
  const radius = Math.max(1, Math.hypot(box.max.x - box.min.x, box.max.z - box.min.z) / 2);
  state.bounds = { points, target: new T.Vector3(centre.x, 0, centre.z), radius, box };

  const { ground, key, rim, dustAt } = scene;
  const uniforms = ground.material.uniforms;
  uniforms.centre.value.set(centre.x, centre.z);
  uniforms.fade.value.set(radius * ORBIT.ground.fadeFrom, radius * ORBIT.ground.fadeTo);
  const [kx, ky, kz] = ORBIT.light.key;
  const [rx, ry, rz] = ORBIT.light.rim;
  key.position.set(centre.x + kx, ky, centre.z + kz);
  key.target.position.set(centre.x, ORBIT.ground.y, centre.z);
  key.target.updateMatrixWorld();
  rim.position.set(centre.x + rx, ry, centre.z + rz);
  const reach = radius * ORBIT.light.reach;
  Object.assign(key.shadow.camera, { left: -reach, right: reach, top: reach, bottom: -reach,
    near: ORBIT.light.near, far: ORBIT.light.far });
  key.shadow.camera.updateProjectionMatrix();

  const random = agentOrbitRandom(ORBIT.dust.seed);
  const spread = radius + ORBIT.dust.reach;
  for (let at = 0; at < dustAt.length; at += 1) {
    const axis = at % scene.dust.geometry.attributes.position.itemSize;
    dustAt[at] = axis === 1 ? random() * ORBIT.dust.height
      : (axis === 0 ? centre.x : centre.z) + (random() * 2 - 1) * spread;
  }
  scene.dust.geometry.attributes.position.needsUpdate = true;
}

/* 파크-밀러 난수 — 같은 씨앗은 같은 줄을 낸다(먼지의 씨앗과 자리). */
function agentOrbitRandom(seed) {
  let value = seed;
  return () => {
    value = (value * ORBIT.dust.multiplier) % ORBIT.dust.modulus;
    return value / ORBIT.dust.modulus;
  };
}

/* 맞춤: 표의 자리(yaw·pitch)에서 본 그림이 판(여백을 뺀 자리)에 드는 배율과 그 네모의 가운데. */
function agentOrbitFitCamera(state) {
  if (!state.scene || !state.bounds || state.width <= 0 || state.height <= 0) return;
  const { camera, scratch } = state.scene;
  agentOrbitPlace(state, ORBIT.camera.yaw, ORBIT.camera.pitch);
  let [x0, x1, y0, y1] = [Infinity, -Infinity, Infinity, -Infinity];
  for (const point of state.bounds.points) {
    scratch.point.copy(point).applyMatrix4(camera.matrixWorldInverse);
    x0 = Math.min(x0, scratch.point.x);
    x1 = Math.max(x1, scratch.point.x);
    y0 = Math.min(y0, scratch.point.y);
    y1 = Math.max(y1, scratch.point.y);
  }
  const pad = state.width < ORBIT.fit.compact ? ORBIT.fit.padCompact : ORBIT.fit.pad;
  /* 위와 아래는 잰 흐름 한 줄과 범례가 덮는 띠 — 재기 전에는 표의 여백. */
  const high = state.padTop ?? pad.top;
  const low = state.padBottom ?? pad.bottom;
  const room = [state.width - pad.left - pad.right - (state.tagRoom ?? 0), state.height - high - low];
  const fit = Math.max(ORBIT.fit.min, Math.min(ORBIT.fit.max, room[0] / Math.max(1, x1 - x0),
    room[1] / Math.max(1, y1 - y0)));
  const spareX = room[0] - fit * (x1 - x0);
  const spareY = room[1] - fit * (y1 - y0);
  const left = x0 - (pad.left + spareX / 2) / fit;
  const top = y1 + (high + spareY / 2) / fit;
  state.box = { x: left + state.width / fit / 2, y: top - state.height / fit / 2 };
  state.fit = fit;
  state.zoom = agentGraphZoom;
  state.scale = fit * agentGraphZoom;
}

/* 카메라를 과녁 둘레의 한 자리에: 과녁에서 `distance`만큼, yaw는 세로축 둘레, pitch는 위로. */
function agentOrbitPlace(state, yaw, pitch) {
  const { camera } = state.scene;
  const target = state.bounds.target;
  const far = ORBIT.camera.distance;
  camera.position.set(target.x + far * Math.sin(yaw) * Math.cos(pitch), target.y + far * Math.sin(pitch),
    target.z + far * Math.cos(yaw) * Math.cos(pitch));
  camera.up.set(0, 1, 0);
  camera.lookAt(target);
  camera.updateMatrixWorld();
}

/* ---- 라벨 -------------------------------------------------------------------- */

/* 한 상태의 말풍선 글 — 이 보기의 이름표와 관계 그림의 카드가 함께 읽는 한 곳. 작업 중은 말풍선이
 * 없으므로 빈 글이다. 낱말은 관계 그림의 한 손(`agentGraphStateWord`)이 짓는다. */
function agentOrbitBubbleKind(state) {
  return (ORBIT.states[state] ?? ORBIT.states.idle).bubble;
}

function agentOrbitBubbleWord(state, ledger = "") {
  const bubble = agentOrbitBubbleKind(state);
  return bubble ? ORBIT.bubbles[bubble].replace("{{word}}", agentGraphStateWord(state, ledger)) : "";
}

/* 라벨 하나: 버튼과 이름표(`span`) — 둘째 줄(모델 · 상태)은 이름표의 `::after`, 말풍선은 버튼의
 * `::before`가 든다. 요소가 둘뿐인 것은 이 그림의 DOM 예산 때문이다(카드 그림 대비 +50 이하). */
function agentOrbitLabelNode() {
  const label = document.createElement("button");
  label.type = "button";
  label.className = "agent-orbit-label";
  label.append(Object.assign(document.createElement("span"), { className: "agent-orbit-name" }));
  return label;
}

/* 적용한 판의 라벨: 판과 에이전트마다 하나, 글과 옷을 쓰고 차례를 맞춘다. 크기는 다음 자리를
 * 잡을 때 한 번 잰다(`agentOrbitMeasure`). */
function agentOrbitLabels(state) {
  const existing = new Map([...state.host.children].map((node) => [node.dataset.orbitKey, node]));
  const wanted = [];
  for (const body of state.bodies.values()) {
    const label = existing.get(body.key) ?? agentOrbitLabelNode();
    if (body.kind === "workspace") {
      const count = agentGraphCountWord("agent", body.members.length);
      const name = body.deck ? `${body.name} · ${t("board.orbit.home", "본진")}` : body.name;
      const branch = body.workspace.branch ?? "";
      writeClassName(label, ["agent-orbit-label", "is-workspace", body.deck ? "is-deck" : "",
        body.dim ? "is-search-dimmed" : ""].filter(Boolean).join(" "));
      writeAttribute(label, "aria-label", [name, count, branch].filter(Boolean).join(", "));
      writeAttribute(label, "data-tip", [t("board.graph.workspace", "워크스페이스"), name, count, branch]
        .filter(Boolean).join(" · "));
      writeAttribute(label, "data-chip", [count, branch].filter(Boolean).join(" · "));
      label.removeAttribute("data-bubble");
      writeStyleProperty(label, "--orbit-ink", `var(--agent-orbit-${agentOrbitPlateInk(body)})`);
      writeAttribute(label.firstElementChild, "data-who", "");
      writeTextContent(label.firstElementChild, name);
    } else {
      /* 두 줄: 이름과 모델, 그리고 상태 · 가지(없으면 워크스페이스). 말풍선은 이름표의 `::before`. */
      const word = agentGraphStateWord(body.state, body.entry.card.ledger ?? "");
      const who = agentName(body.entry.card.agent) || "";
      const where = body.entry.workspace.branch || body.plate.name;
      const bubble = agentOrbitBubbleWord(body.state, body.entry.card.ledger ?? "");
      writeClassName(label, ["agent-orbit-label", `is-${body.kind}`, body.lead ? "is-lead" : "",
        `is-${body.state}`, body.searchDim ? "is-search-dimmed" : ""].filter(Boolean).join(" "));
      writeAttribute(label, "data-chip", [word, where].filter(Boolean).join(" · "));
      writeAttribute(label, "data-state", word);
      writeAttribute(label, "aria-label", [body.name, word, who, body.plate.name].filter(Boolean).join(", "));
      writeAttribute(label, "data-tip", [body.name, word, who, body.plate.name].filter(Boolean).join(" · "));
      if (bubble) writeAttribute(label, "data-bubble", bubble);
      else label.removeAttribute("data-bubble");
      writeStyleProperty(label, "--orbit-ink", `var(--agent-orbit-${body.lead ? "lead" : body.known.ink})`);
      writeAttribute(label.firstElementChild, "data-who", who);
      writeTextContent(label.firstElementChild, body.name);
    }

    writeAttribute(label, "data-orbit-key", body.key);
    if (body.label !== label) {
      body.named = null;
      body.placed = null;
      body.bubbleAt = null;
    }
    body.label = label;
    body.box = null;
    wanted.push(label);
  }
  reconcileElementOrder(state.host, wanted);
  state.selected = null;
  state.measured = false;
  state.labelOrder = null;
}

/* 라벨의 크기와 판의 막힌 자리(흐름 한 줄과 범례)를 잰다 — 적용이나 크기가 바뀐 뒤 처음
 * 자리를 잡는 프레임에서 한 번. 접힌 라벨은 재는 동안만 펼친다. */
function agentOrbitMeasure(state) {
  for (const body of state.bodies.values()) {
    if (body.label && body.named !== "1") {
      body.named = "1";
      writeStyleProperty(body.label, "--orbit-name", "1");
    }
  }
  for (const body of state.bodies.values()) {
    body.box = body.label ? { w: body.label.offsetWidth, h: body.label.offsetHeight } : null;
  }
  state.chip = Number.parseFloat(getComputedStyle(state.stage).getPropertyValue("--agent-orbit-chip")) || 0;
  state.reserved = [state.glance, state.legend].filter((node) => node && node.offsetWidth > 0)
    .map((node) => ({ x: node.offsetLeft, y: node.offsetTop, w: node.offsetWidth, h: node.offsetHeight }));
  state.measured = state.width > 0 && [...state.bodies.values()].every((body) => (body.box?.w ?? 0) > 0);

  /* 말풍선의 크기: 글은 `::before`라 요소가 없으니, 그 글꼴로 캔버스가 글의 폭을 잰다. */
  const L = ORBIT.label;
  agentOrbitPen ??= document.createElement("canvas").getContext("2d");
  const px = (value) => Number.parseFloat(value) || 0;
  for (const body of state.bodies.values()) {
    const words = body.label?.dataset.bubble;
    body.bubbleBox = null;
    if (!words || !agentOrbitPen) continue;
    const style = getComputedStyle(body.label, "::before");
    agentOrbitPen.font = `${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
    body.bubbleBox = {
      w: agentOrbitPen.measureText(words).width + px(style.letterSpacing) * words.length + px(style.paddingLeft)
        + px(style.paddingRight),
      h: px(style.fontSize) * L.bubble.line + px(style.paddingTop) + px(style.paddingBottom),
    };
  }

  /* 섬의 이름표가 서는 오른쪽 폭(가장 넓은 이름표) — 바뀌면 맞춤이 그만큼 비켜 선다. */
  const tagged = [...state.bodies.values()].filter((body) => body.kind !== "workspace" && !body.lead
    && !body.plate.deck && body.box);
  const tagRoom = tagged.length > 0 ? Math.max(...tagged.map((body) => body.box.w)) + L.gap + L.island : 0;
  /* 흐름 한 줄의 아래 끝과 범례의 위 끝 — 맞춤은 그 사이의 띠에 선다. */
  const [glance, legend] = [state.glance, state.legend].map((node) => (node && node.offsetWidth > 0 ? node : null));
  const padTop = glance ? glance.offsetTop + glance.offsetHeight + L.gap : undefined;
  /* 판의 이름은 판 앞 모서리 밑에 매달린다 — 가장 아래 판의 이름이 범례에 걸리지 않게 그 키만큼 더 비운다. */
  let caption = 0;
  for (const body of state.bodies.values()) {
    if (body.kind === "workspace" && body.box) caption = Math.max(caption, body.box.h + L.gap);
  }
  const padBottom = legend ? state.height - legend.offsetTop + L.gap + caption : undefined;
  if (tagRoom !== state.tagRoom || padTop !== state.padTop || padBottom !== state.padBottom) {
    Object.assign(state, { tagRoom, padTop, padBottom });
    if (state.scene && state.bounds) {
      agentOrbitFitCamera(state);
      const lens = agentOrbitLens(state);
      agentOrbitCameraTo(state, lens);
      state.lens = lens;
    }
  }
}

/* 고른 에이전트: 라벨의 `aria-pressed`와 계보의 옅어짐, 발밑의 고리. 바뀐 판에서만 쓴다. */
function agentOrbitDressSelection(state) {
  if (state.selected === agentGraphSelectedKey) return;
  state.selected = agentGraphSelectedKey;
  const picked = state.bodies.get(agentGraphSelectedKey);
  /* 계보 밖을 옅게: 고른 에이전트가 부모를 가질 때만 — 그 에이전트, 그 조상과 자손이 제 빛이다. */
  const lane = new Set();
  if (picked && picked.kind !== "workspace" && picked.parentKey) {
    for (let at = picked; at; at = at.parentKey ? state.bodies.get(at.parentKey) : null) lane.add(at.key);
    const down = (body) => {
      lane.add(body.key);
      for (const child of body.children) down(child);
    };
    down(picked);
  }
  for (const body of state.bodies.values()) {
    body.laneDim = lane.size > 0 && body.kind !== "workspace" && !lane.has(body.key);
    if (!body.label) continue;
    writeAttribute(body.label, "aria-pressed", String(body.key === agentGraphSelectedKey));
    body.label.classList.toggle("is-lane-dimmed", body.laneDim);
  }
  for (const plate of state.plates) plate.laneDim = lane.size > 0 && plate.members.every((member) => member.laneDim);
  if (state.scene) state.scene.painted = false;
  state.dirty = true;
}

/* 흐름 한 줄: 모델에서 읽은 지금의 이야기 — 확인을 기다리는 에이전트, 실패, 부모에게 돌아가는
 * 완료, 조율자가 보낸 지시, 그도 없으면 판의 수. 번역된 문장에 이름의 표지를 넣고 그 자리에서
 * 셋(앞 · 이름 · 뒤)으로 나눈다 — 어느 언어의 어순이든 이름만 제 색을 입는다. */
function agentOrbitGlance(state) {
  const line = state.glance?.querySelector(".agent-orbit-glance-line");
  const sub = state.glance?.querySelector(".agent-orbit-glance-sub");
  if (!line || !sub) return;
  const agents = state.agents;
  const count = (wanted) => agents.filter((body) => body.state === wanted).length;
  const vars = { working: count("working"), attention: count("needs-attention"), done: count("done"),
    idle: count("idle") + count("paused"), count: agents.length, workspaces: state.plates.length };
  const byName = (left, right) => left.name.localeCompare(right.name);
  const first = (wanted) => agents.filter((body) => body.state === wanted).sort(byName)[0] ?? null;
  const subject = (body) => agentOrbitParticle(body.name, t("board.orbit.particle", "이|가"));
  const mark = ORBIT.mark;
  const lead = [...agents].filter((body) => body.children.length > 0)
    .sort((left, right) => right.children.length - left.children.length || byName(left, right))[0] ?? null;
  const waiting = first("needs-attention");
  const failed = first("failed");
  const done = agents.filter((body) => body.state === "done" && body.parentKey).sort(byName)[0] ?? null;
  let words = "";
  let ink = "working";
  let named = "";
  if (waiting) {
    ink = "attention";
    words = t("board.orbit.glanceAttention", "{{name}}{{subject}} 확인을 기다립니다.",
      { name: mark, subject: subject(waiting) });
    named = waiting.name;
  } else if (failed) {
    ink = "failed";
    words = t("board.orbit.glanceFailed", "{{name}}{{subject}} 실패했습니다.", { name: mark, subject: subject(failed) });
    named = failed.name;
  } else if (done) {
    ink = "done";
    words = t("board.orbit.glanceDone", "{{name}} 완료 — 결과가 {{parent}}에게 돌아갑니다.",
      { name: mark, parent: state.bodies.get(done.parentKey).name });
    named = done.name;
  } else if (lead) {
    ink = "lead";
    const busy = lead.children.filter((child) => child.state === "working").length;
    words = busy === lead.children.length
      ? t("board.orbit.glanceAll", "{{name}}{{subject}} 작업 {{count}}개에 지시를 보냈고, 모두 작업 중입니다.",
        { name: mark, subject: subject(lead), count: lead.children.length })
      : t("board.orbit.glanceSome", "{{name}}{{subject}} 작업 {{count}}개에 지시를 보냈고, {{working}}개가 작업 중입니다.",
        { name: mark, subject: subject(lead), count: lead.children.length, working: busy });
    named = lead.name;
  } else {
    words = t("board.orbit.glancePlain", "에이전트 {{count}}개가 워크스페이스 {{workspaces}}곳에 앉아 있습니다.", vars);
  }
  const [before, after = ""] = words.split(mark);
  const [head, name, tail] = line.children;
  writeTextContent(head, before);
  writeTextContent(name, words.includes(mark) ? named : "");
  writeTextContent(tail, after);
  writeStyleProperty(line, "--orbit-glance-ink", `var(--agent-orbit-${ink})`);
  writeTextContent(sub, t("board.orbit.glanceRead",
    "왼쪽 → 오른쪽으로 읽습니다 · 빛의 색 = 상태 · 움직이는 빛 = 오가는 지시와 우편 · 대기 {{idle}}", vars));
}

/* 한국어의 주격 조사: 카탈로그가 든 짝(`받침 있을 때|없을 때`)에서 이름의 마지막 음절이
 * 고른다. 한글이 아닌 끝이면 둘을 함께(「이(가)」), 짝이 빈 언어는 빈 글이다. */
function agentOrbitParticle(word, pair) {
  const [withFinal = "", without = ""] = String(pair).split("|");
  if (!withFinal && !without) return "";
  const code = String(word).charCodeAt(String(word).length - 1);
  if (!(code >= ORBIT.hangul.first && code <= ORBIT.hangul.last)) return `${withFinal}(${without})`;
  return (code - ORBIT.hangul.first) % ORBIT.hangul.finals ? withFinal : without;
}

/* 무대의 손들, 한 번. 라벨을 누르면 관계 그림의 선택 길로, 손이 오르거나 초점이 들면
 * 카메라와 움직임이 멈춘다(움직이는 과녁은 누르기 어렵다). 빈 곳을 끌면 카메라가 돌고
 * (가로가 yaw, 세로가 pitch), 놓으면 끌던 빠르기로 미끄러져 선다. 끌지 않고 놓은 자리에
 * 로봇이나 판이 있으면(광선 투사) 그것을 고른다. 빈 곳을 두 번 누르면 맞춤, ⌘/Ctrl 휠은
 * 관계 그림의 배율을 움직인다. 끌기와 누르기를 가르는 문턱은 관계 그림들이 함께 쓰는
 * 끌기의 것이다(`wireGraphDrag`). */
function wireAgentOrbitStage(state) {
  const { view, stage, host } = state;
  const keyOf = (node) => (node instanceof Element ? node.closest("[data-orbit-key]") : null);
  const hold = (hovered) => {
    if (hovered !== undefined) state.hovered = hovered;
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
  stage.onpointermove = (event) => {
    if (event.buttons !== 0 || keyOf(event.target)) return;
    stage.classList.toggle("is-pointing", agentOrbitPick(state, event.clientX, event.clientY) !== null);
  };
  let grab = null;
  wireGraphDrag(stage, {
    /* 스페이스를 쥔 손은 라벨 위에서도 카메라를 돌린다 — 카드 그림과 같은 몸짓. */
    grabbed: () => agentGraphSpaceHeld,
    onGrab: (event) => {
      grab = { yaw: agentOrbitYaw(state), pitch: state.camera.pitch, marks: [], x: event.clientX, y: event.clientY };
    },
    onDrag: (moveX, moveY, moved) => {
      if (!grab) return;
      agentGraphPauseFollow(view);
      if (!stage.classList.contains("is-panning")) stage.classList.add("is-panning");
      const camera = state.camera;
      Object.assign(camera, { spin: false, turning: 0, tilting: 0,
        yaw: agentOrbitClampYaw(grab.yaw + moveX * ORBIT.camera.turn),
        pitch: agentOrbitPitch(grab.pitch + moveY * ORBIT.camera.turn) });
      /* 손이 움직인 때(이벤트의 시각) — 바쁜 판에서 늦게 처리된 움직임도 제 빠르기를 잰다. */
      const now = moved?.timeStamp ?? performance.now();
      grab.marks.push([now, camera.yaw, camera.pitch]);
      /* 빠르기는 끌던 마지막 `sampleMs`에서 — 바쁜 판에서 움직임이 드물게 오면 마지막 둘에서. */
      while (grab.marks.length > 2 && now - grab.marks[0][0] > ORBIT.camera.sampleMs) grab.marks.shift();
      state.dirty = true;
      agentOrbitWake();
    },
    onEnd: (turned) => {
      stage.classList.remove("is-panning");
      const held = grab;
      grab = null;
      if (!held) return;
      if (!turned) {
        const key = agentOrbitPick(state, held.x, held.y);
        if (key) selectAgentGraphEntity(view, key);
        return;
      }
      const marks = held.marks;
      if (marks.length < 2 || agentOrbitReduced()) return;
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

/* 화면의 한 자리 밑의 로봇이나 판(광선 투사) — 그 키, 없으면 `null`. 로봇이 판보다 앞선다. */
function agentOrbitPick(state, clientX, clientY) {
  const scene = state.scene;
  if (!scene?.meshes || !state.lens) return null;
  const box = state.stage.getBoundingClientRect();
  const { ray, pointer } = scene.scratch;
  pointer.set(((clientX - box.left) / state.width) * 2 - 1, -((clientY - box.top) / state.height) * 2 + 1);
  ray.setFromCamera(pointer, scene.camera);
  const bot = ray.intersectObject(scene.meshes.pick, false)[0];
  if (bot) return state.agents[bot.instanceId]?.key ?? null;
  const plate = ray.intersectObject(scene.meshes.plates, false)[0];
  return plate ? state.plates[plate.instanceId]?.key ?? null : null;
}

/* ---- 잉크 -------------------------------------------------------------------- */

/* 그림이 쓰는 색: 무대 위의 `--agent-orbit-*` 토큰을 계산된 색으로. 캔버스 자신이 탐침이다 —
 * `color`에 토큰을 걸고 계산된 값을 읽어 선형 색으로 옮긴다. 테마가 바뀌면 한 곳
 * (`agentOrbitWatchTheme`)이 이것을 비우고, 다음 그림이 한 번 다시 읽는다. 빛의 세기는 판의
 * 색 구성표(라이트·다크)마다 표의 한 줄이다. */
function agentOrbitInks(state) {
  if (state.inks) return state.inks;
  const T = state.gl.T;
  const probe = state.canvas;
  const held = probe.style.color;
  const inks = {};
  for (const name of ORBIT.inks) {
    probe.style.color = `var(--agent-orbit-${name})`;
    inks[name] = agentOrbitColor(T, getComputedStyle(probe).color);
  }
  probe.style.color = held;
  if (Object.values(inks).some((ink) => ink === null)) return null;
  const scheme = getComputedStyle(document.documentElement).colorScheme;
  state.theme = String(scheme).includes("light") ? ORBIT.themes.light : ORBIT.themes.dark;
  state.inks = inks;
  if (state.scene) state.scene.painted = false;
  agentOrbitInkReads += 1;
  return inks;
}

/* 계산된 색 한 줄(`rgb(…)`나 `color(srgb …)`)을 선형 색 하나로. */
function agentOrbitColor(T, text) {
  const values = String(text).match(/[\d.]+/g)?.map(Number) ?? [];
  if (values.length < ORBIT.axes) return null;
  const unit = String(text).startsWith("color(") ? 1 : ORBIT.channel;
  return new T.Color().setRGB(values[0] / unit, values[1] / unit, values[2] / unit, T.SRGBColorSpace);
}

let agentOrbitThemeWatch = null;
/* 말풍선의 폭을 재는 캔버스 펜 하나. */
let agentOrbitPen = null;

/* 테마를 보는 눈 하나 — 창 전체에서. `data-theme` 한 속성만 본다. */
function agentOrbitWatchTheme() {
  if (agentOrbitThemeWatch !== null || typeof MutationObserver !== "function") return;
  agentOrbitThemeWatch = new MutationObserver(() => {
    for (const view of agentOrbitViews) {
      const state = agentOrbitStates.get(view);
      if (!state) continue;
      state.inks = null;
      state.measured = false;
      state.dirty = true;
    }
    agentOrbitWake();
  });
  agentOrbitThemeWatch.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
}

/* 빛의 곱: 잉크 × 세기(HDR)를 `into`에. */
function agentOrbitGlow(into, ink, strength) {
  return into.copy(ink).multiplyScalar(strength);
}

/* 색을 입힌다 — 잉크가 바뀌었거나(테마), 상태·선택·모양이 바뀐 판에서만. 프레임마다 도는 것은
 * 맥박과 빛기둥의 세기뿐이다(`agentOrbitPose`). */
function agentOrbitPaint(state) {
  const scene = state.scene;
  const inks = state.inks;
  const K = state.theme;
  const T = scene.T;
  const G = ORBIT.glow;
  const { materials, meshes, scratch } = scene;
  const ink = scratch.ink;
  const glow = (name, strength) => agentOrbitGlow(ink, inks[name], strength);
  scene.scene.background = inks.ground;
  const uniforms = scene.ground.material.uniforms;
  uniforms.base.value.copy(inks.ground);
  uniforms.grid.value.copy(inks.grid);
  uniforms.gridA.value = K.floor;
  scene.shadow.opacity = K.shadow;
  /* 레일의 덮개: 어두운 판에서는 더하는 빛, 밝은 판에서는 덮는 색 — 흰 바닥에 더한 빛은 보이지 않는다. */
  materials.sheath.opacity = K.sheath;
  materials.sheath.blending = K.additive ? T.AdditiveBlending : T.NormalBlending;
  scene.hemi.color.copy(inks.sky);
  scene.hemi.groundColor.copy(inks.earth);
  scene.hemi.intensity = K.hemi;
  scene.key.color.copy(inks.sun);
  scene.key.intensity = K.key;
  scene.rim.color.copy(inks.rim);
  scene.rim.intensity = K.rim;
  materials.metal.color.copy(inks.metal);
  materials.visor.color.copy(inks.visor);
  materials.pad.color.copy(inks.pad);
  scene.dust.material.uniforms.color.value.copy(inks.dust);
  const bloom = state.gl.bloom;
  if (bloom) {
    bloom.composite.uniforms.strength.value = K.strength;
    bloom.composite.uniforms.radius.value = K.radius;
    bloom.threshold.uniforms.threshold.value = K.threshold;
  }

  /* 로봇: 몸은 모델의 계열에서 해시가 고른 톤, 빛은 상태. */
  for (const body of state.agents) {
    const dim = agentOrbitDim(body);
    const idle = body.known.ink === "idle";
    const state_ = body.known.ink;
    for (const [tone, mesh, many] of [["main", meshes.skull, 1], ["light", meshes.body, 1], ["main", meshes.arm, 2],
      ["accent", meshes.ear, 2], ["accent", meshes.mitt, 2]]) {
      ink.lerpColors(inks[`${body.family}-${tone}-a`], inks[`${body.family}-${tone}-b`], body.unit);
      for (let side = 0; side < many; side += 1) mesh.setColorAt(body.index * many + side, ink);
    }
    const eyes = glow(state_, (idle ? K.eyeIdle : K.glow) * dim);
    meshes.eye.setColorAt(body.index * 2, eyes);
    meshes.eye.setColorAt(body.index * 2 + 1, eyes);
    const tip = glow(body.lead ? "lead" : state_, (idle ? K.eyeIdle : K.glow * G.tip) * dim);
    meshes.tip.setColorAt(body.index, tip);
    meshes.tipRing.setColorAt(body.index, tip);
    const selected = body.key === state.selected;
    meshes.padRing.setColorAt(body.index, glow(state_, (idle ? G.ringIdle : K.edge * (selected ? G.ringSelected : 1)) * dim));
    meshes.chest.setColorAt(body.index, glow(state_, (idle ? K.eyeIdle : K.glow * G.chest) * dim));
    meshes.keys.setColorAt(body.index, glow(state_, K.screen * G.keys * dim));
    meshes.beam.setColorAt(body.index, glow(state_, K.glow * G.beam));
    meshes.core.setColorAt(body.index, glow(state_, K.glow * G.core));
    body.pulseInk = (body.pulseInk ?? new T.Color()).copy(glow(state_, K.glow * G.pulse * dim));
    if (body.lead) {
      meshes.daisRing.setColorAt(body.leadIndex, glow("lead", K.glow));
      meshes.halo.setColorAt(body.leadIndex, glow("lead", K.glow * G.halo));
      meshes.haloOuter.setColorAt(body.leadIndex, glow("lead", K.glow * G.haloOuter));
      meshes.mic.setColorAt(body.leadIndex, glow("lead", K.glow));
    }
  }
  for (const mesh of Object.values(meshes)) {
    if (mesh.instanceColor) mesh.instanceColor.needsUpdate = true;
  }

  /* 판: 윗면과 옆면(정점 색), 테와 격자는 그 판에서 가장 급한 상태 — 조율자가 앉은 본진은
   * 확인 필요·실패가 없으면 조율자의 빛. 웅덩이는 본진 하나와 급한 섬들. */
  const plates = scene.plates;
  const pools = uniforms.pools.value;
  const poolInks = uniforms.poolCol.value;
  let pool = 0;
  const urgent = [...state.plates].sort((left, right) => agentOrbitUrgency(right).rank - agentOrbitUrgency(left).rank);
  for (const plate of state.plates) {
    const top = plates.slab.geometry.attributes.color;
    const normal = plates.slab.geometry.attributes.normal;
    for (let at = plate.slab.start; at < plate.slab.start + plate.slab.count; at += 1) {
      const face = normal.getY(at) > ORBIT.shape.top ? inks.top : inks.side;
      top.setXYZ(at, face.r, face.g, face.b);
    }
    const known = agentOrbitUrgency(plate);
    const name = agentOrbitPlateInk(plate);
    const idle = name === "idle";
    const dim = (plate.dim ? G.search : 1) * (plate.laneDim ? G.dim : 1);
    const picked = plate.members.some((member) => member.key === state.selected) || plate.key === state.selected;
    glow(name, (idle ? G.edgeIdle : K.edge) * dim * (picked ? G.edgeSelected : 1));
    for (let at = plate.rimRange.start; at < plate.rimRange.start + plate.rimRange.count; at += 1) {
      plates.rim.geometry.attributes.color.setXYZ(at, ink.r, ink.g, ink.b);
    }
    glow(name, K.grid * (idle ? G.gridIdle : 1) * dim);
    for (let at = plate.gridRange.start; at < plate.gridRange.start + plate.gridRange.count; at += 1) {
      plates.grid.geometry.attributes.color.setXYZ(at, ink.r, ink.g, ink.b);
    }
    if (!plate.deck) {
      const mode = known.ink === "attention" || known.ink === "failed" ? ORBIT.screen.modes.attention
        : known.ink === "done" ? ORBIT.screen.modes.done : ORBIT.screen.modes.code;
      const look = meshes.screen.geometry.attributes.look;
      look.setXY(plate.screenIndex, mode, knowledgeHash(plate.key) / ORBIT.hashSpan);
      look.needsUpdate = true;
      plate.screenInk = (plate.screenInk ?? new T.Color()).copy(idle ? ink.setRGB(0, 0, 0)
        : glow(known.ink, K.screen * G.screen * dim));
      meshes.screen.setColorAt(plate.screenIndex, plate.screenInk);
    }
  }
  /* 바탕빛: 판 전체에 옅게 번지는 두 웅덩이 — 본진 둘레의 조율자 보라, 섬들 둘레의 작업 하늘색. */
  for (const [group, name] of [[state.plates.filter((plate) => plate.deck), "lead"],
    [state.plates.filter((plate) => !plate.deck), "working"]]) {
    if (group.length === 0 || pool >= pools.length) continue;
    const [x0, x1] = [Math.min(...group.map((plate) => plate.left)), Math.max(...group.map((plate) => plate.right))];
    const [z0, z1] = [Math.min(...group.map((plate) => plate.back)), Math.max(...group.map((plate) => plate.front))];
    pools[pool].set((x0 + x1) / 2, (z0 + z1) / 2, Math.max(ORBIT.ground.ambient, Math.hypot(x1 - x0, z1 - z0) / 2), 0);
    agentOrbitGlow(poolInks[pool], inks[name], K.ambient);
    pool += 1;
  }
  for (const plate of urgent) {
    if (pool >= pools.length) break;
    const name = agentOrbitPlateInk(plate);
    const x = (plate.left + plate.right) / 2 + (plate.deck ? ORBIT.ground.deckShift : 0);
    pools[pool].set(x, (plate.back + plate.front) / 2, plate.deck ? ORBIT.ground.deckPool : ORBIT.ground.islandPool, 0);
    agentOrbitGlow(poolInks[pool], inks[name], K.pool * (name === "idle" ? G.poolIdle : plate.deck ? G.deckPool : 1)
      * (plate.dim ? G.search : 1) * (plate.laneDim ? G.dim : 1));
    pool += 1;
  }
  for (; pool < pools.length; pool += 1) poolInks[pool].setRGB(0, 0, 0);
  if (plates) {
    for (const part of [plates.slab, plates.rim, plates.grid]) part.geometry.attributes.color.needsUpdate = true;
  }
  if (meshes.screen.instanceColor) meshes.screen.instanceColor.needsUpdate = true;

  /* 레일: 부모 쪽은 조율자의 빛, 자식 쪽은 자식의 상태 — 부드러운 계단으로 번진다. 의존은 옅다. */
  const from = new T.Color();
  const to = new T.Color();
  /* 덮개는 제 세기(`sheathInk`)로 — 밝은 판에서는 관의 밝은 심 밑에 짙은 색의 관이 선다. */
  const deep = K.sheathInk / K.rail;
  const [sheathFrom, sheathTo] = [new T.Color(), new T.Color()];
  for (const rail of state.rails) {
    const dim = Math.min(agentOrbitDim(rail.from), agentOrbitDim(rail.to));
    if (rail.kind === "dependency") {
      agentOrbitGlow(from, inks.dependency, K.rail * G.dependency * dim);
      to.copy(from);
    } else {
      agentOrbitGlow(from, inks.lead, K.rail * dim);
      agentOrbitGlow(to, inks[rail.to.known.ink], K.rail * (rail.to.known.ink === "idle" ? G.railIdle : 1) * dim);
    }
    agentOrbitTint(T, scene.rails.core, rail.coreRange, from, to);
    if (rail.sheathRange) {
      agentOrbitTint(T, scene.rails.sheath, rail.sheathRange, sheathFrom.copy(from).multiplyScalar(deep),
        sheathTo.copy(to).multiplyScalar(deep));
    }
  }
  for (const part of [scene.rails.core, scene.rails.sheath]) {
    if (part) part.geometry.attributes.color.needsUpdate = true;
  }
  materials.select.color.copy(glow("select", K.select));
  scene.painted = true;
}

/* 관 하나를 두 끝의 색으로: 조각마다 부드러운 계단(smoothstep)으로 섞는다. */
function agentOrbitTint(T, mesh, range, from, to) {
  const color = mesh.geometry.attributes.color;
  const ring = range.radial + 1;
  const pieces = range.count / ring - 1;
  for (let piece = 0; piece <= pieces; piece += 1) {
    const at = piece / pieces;
    const blend = T.MathUtils.smoothstep(at, 0, 1);
    const r = from.r + (to.r - from.r) * blend;
    const g = from.g + (to.g - from.g) * blend;
    const b = from.b + (to.b - from.b) * blend;
    for (let side = 0; side < ring; side += 1) color.setXYZ(range.start + piece * ring + side, r, g, b);
  }
}

/* 판의 테가 입는 잉크: 가장 급한 상태 — 조율자가 앉은 본진은 확인 필요·실패가 없으면 조율자의 빛. */
function agentOrbitPlateInk(plate) {
  const known = agentOrbitUrgency(plate);
  return plate.deck && plate.members.some((member) => member.lead) && known.rank < ORBIT.states.failed.rank
    ? "lead" : known.ink;
}

/* 판에서 가장 급한 상태. */
function agentOrbitUrgency(plate) {
  let best = ORBIT.states.idle;
  for (const member of plate.members) if (member.known.rank > best.rank) best = member.known;
  return best;
}

/* 한 몸의 빛이 옅어지는 몫: 검색에 맞지 않거나 고른 계보 밖. */
function agentOrbitDim(body) {
  return (body.searchDim ? ORBIT.glow.search : 1) * (body.laneDim ? ORBIT.glow.dim : 1);
}

/* ---- 박자 -------------------------------------------------------------------- */

/* 이 판에 다음 그림이 필요한가: 새로 그릴 것(옷·크기·선택)이 있거나, 막 끝난 로봇이 뛰는
 * 중이거나, 손이 떠나며 다시 움직이는 중이거나, 카메라가 미끄러지거나 — 손이 없는 판에서
 * 카메라가 흔들리거나 무언가 흐를 때. 어느 것도 아니면 박자는 서고 rAF는 0이다. */
function agentOrbitMoving(state) {
  const camera = state.camera;
  return state.dirty || state.pulsing || state.hold !== state.holdTarget
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
 * 멈춘 판의 rAF는 0이다. 초점이 없는 창은 박자를 건너뛰어 초당 30장까지만 그린다. 문서를
 * 떠난 판은 여기서 걷는다 — 컨텍스트까지 돌려준다. */
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
      if (state) agentOrbitClose(state);
      continue;
    }
    if (agentOrbitShown(state)) shown.push(state);
  }
  const rate = document.hasFocus() ? ORBIT.fps.focused : ORBIT.fps.blurred;
  const gap = ORBIT.second / rate - ORBIT.fps.slackMs;
  if (shown.length > 0 && (agentOrbitDrawnAt === null || stamp - agentOrbitDrawnAt >= gap)) {
    const step = agentOrbitDrawnAt === null ? 0 : Math.min(ORBIT.stepCapMs, stamp - agentOrbitDrawnAt);
    agentOrbitDrawnAt = stamp;
    for (const state of shown) {
      if (agentOrbitMoving(state)) agentOrbitDraw(state, step, stamp);
    }
  }
  if (!shown.some(agentOrbitMoving)) {
    agentOrbitDrawnAt = null;
    return;
  }
  agentOrbitFrame = requestAnimationFrame(agentOrbitTick);
}

/* 닫힌 판: 모양을 돌려주고 렌더러와 컨텍스트를 놓는다. */
function agentOrbitClose(state) {
  agentOrbitDropScene(state);
  agentOrbitStates.delete(state.view);
  const { renderer, bloom } = state.gl;
  if (bloom) {
    for (const one of [bloom.scene, bloom.high, ...bloom.across, ...bloom.down, bloom.threshold, ...bloom.blur,
      bloom.composite, bloom.quad.geometry]) one.dispose();
  }
  /* 손을 먼저 뗀다 — 놓아 준 컨텍스트의 유실 소식이 살아 있는 판을 카드로 넘기지 않게. */
  state.canvas.removeEventListener("webglcontextlost", state.gl.lost);
  state.canvas.removeEventListener("webglcontextrestored", state.gl.restored);
  renderer.dispose();
  renderer.forceContextLoss();
}

/* ---- 카메라와 투영 ----------------------------------------------------------- */

function agentOrbitPitch(pitch) {
  return Math.max(ORBIT.camera.pitchMin, Math.min(ORBIT.camera.pitchMax, pitch));
}

function agentOrbitClampYaw(yaw) {
  return Math.max(ORBIT.camera.yaw - ORBIT.camera.range, Math.min(ORBIT.camera.yaw + ORBIT.camera.range, yaw));
}

/* 지금 보는 yaw: 사람이 세운 yaw에, 가만둔 판의 흔들림을 더한 것. */
function agentOrbitYaw(state) {
  const camera = state.camera;
  return camera.yaw + (camera.spin && !agentOrbitReduced() ? ORBIT.camera.sway * Math.sin(camera.sway) : 0);
}

/* 카메라를 한 걸음: 가만둔 판은 흔들리고, 놓은 판은 미끄러진다. 손이 오른 판(`sim` 0)에서는
 * 둘 다 서지만, 미끄러짐의 빠르기는 흐른 시간으로 줄어 손이 떠나도 되살아나지 않는다. */
function agentOrbitTurn(state, step, sim) {
  const camera = state.camera;
  if (camera.spin) camera.sway += (sim / ORBIT.second) * ORBIT.camera.swaySpeed * ORBIT.turn;
  if (camera.turning === 0 && camera.tilting === 0) return;
  const yaw = agentOrbitClampYaw(camera.yaw + camera.turning * sim);
  if (yaw !== camera.yaw + camera.turning * sim) camera.turning = 0;
  camera.yaw = yaw;
  const pitch = agentOrbitPitch(camera.pitch + camera.tilting * sim);
  if (pitch !== camera.pitch + camera.tilting * sim) camera.tilting = 0;
  camera.pitch = pitch;
  const decay = Math.exp(-step / ORBIT.camera.glideMs);
  camera.turning *= decay;
  camera.tilting *= decay;
  if (Math.abs(camera.turning) < ORBIT.camera.rest) camera.turning = 0;
  if (Math.abs(camera.tilting) < ORBIT.camera.rest) camera.tilting = 0;
}

/* 이 프레임의 렌즈: yaw·pitch, 과녁, 화면의 배율(맞춤 × 사람의 배율, CSS px / 세계 단위), 그리고
 * 과녁이 서는 화면의 자리. 직교라 깊이는 크기를 바꾸지 않는다: 화면의 자리 = (cx, cy) + 배율 ×
 * (카메라의 가로·세로 축 위의 거리). */
function agentOrbitLens(state) {
  const scale = state.fit * state.zoom;
  const box = state.box ?? { x: 0, y: 0 };
  const target = state.bounds?.target;
  return {
    yaw: agentOrbitYaw(state), pitch: state.camera.pitch, scale,
    target: target ? [target.x, target.y, target.z] : [0, 0, 0],
    cx: state.width / 2 - scale * box.x, cy: state.height / 2 + scale * box.y,
  };
}

function agentOrbitSameLens(left, right) {
  return left !== null && left.yaw === right.yaw && left.pitch === right.pitch && left.scale === right.scale
    && left.cx === right.cx && left.cy === right.cy && left.target.every((value, axis) => value === right.target[axis]);
}

/* 카메라를 이 렌즈에 세운다(직교의 네 벽은 맞춤의 네모와 사람의 배율에서). */
function agentOrbitCameraTo(state, lens) {
  const { camera } = state.scene;
  agentOrbitPlace(state, lens.yaw, lens.pitch);
  const box = state.box ?? { x: 0, y: 0 };
  const halfWide = state.width / lens.scale / 2;
  const halfTall = state.height / lens.scale / 2;
  camera.left = box.x - halfWide;
  camera.right = box.x + halfWide;
  camera.top = box.y + halfTall;
  camera.bottom = box.y - halfTall;
  camera.updateProjectionMatrix();
}

/* 세계의 한 점을 지금의 카메라로 화면(CSS px)에 — 라벨과 시험이 함께 쓴다. */
function agentOrbitScreen(state, x, y, z, into = agentOrbitSeen()) {
  const { camera, scratch } = state.scene;
  scratch.point.set(x, y, z);
  const depth = -scratch.depth.copy(scratch.point).applyMatrix4(camera.matrixWorldInverse).z;
  scratch.point.project(camera);
  into.x = ((scratch.point.x + 1) / 2) * state.width;
  into.y = ((1 - scratch.point.y) / 2) * state.height;
  into.depth = depth;
  return into;
}

/* ---- 자세 -------------------------------------------------------------------- */

/* 로봇마다 한 벌의 자세를 인스턴스의 행렬에 쓴다. `still`이면 시간이 서 있는 한 장(움직임을
 * 줄인 판과 막 지은 판) — 뜀과 맥박이 없다. `step`은 흐른 밀리초(얼굴을 돌리는 데 쓴다). */
function agentOrbitPose(state, step, stamp, still) {
  const scene = state.scene;
  if (!scene?.meshes) return;
  const { meshes, joints, scratch } = scene;
  const P = ORBIT.pose;
  const B = ORBIT.bot;
  const time = state.clock;
  const yaw = agentOrbitYaw(state);
  const { root, head, pivot, local, out, none, at, size, turn, euler } = scratch;
  const set = (mesh, index, parent, joint) => mesh.setMatrixAt(index, out.multiplyMatrices(parent, joint));
  const pose = (matrix, x, y, z, rx, ry, rz, sx = 1, sy = 1, sz = 1) =>
    matrix.compose(at.set(x, y, z), turn.setFromEuler(euler.set(rx, ry, rz)), size.set(sx, sy, sz));
  let pulsing = false;
  for (const body of state.agents) {
    const index = body.index;
    const kind = body.known.pose;
    const phase = body.unit * ORBIT.turn;
    const dim = agentOrbitDim(body);
    const [x, y, z] = body.world;
    const s = body.scale;
    const rest = body.lead ? P.face.lead : body.plate.deck ? P.face.deck : P.face.island;
    const facing = kind === "attention" || kind === "done" ? yaw : rest;
    body.face = still || body.face === null ? facing
      : body.face + (facing - body.face) * Math.min(1, (step / ORBIT.second) * P.face.speed);
    const blink = !still && (time + body.unit * P.blink.every) % P.blink.every < P.blink.for;
    let lift = y;
    let headX = 0;
    let headZ = 0;
    let tilt = 0;
    let eyes = 1;
    /* 두 팔의 굽힘(x)과 벌림(z) — 배열 없이 네 수로. */
    let leftX = 0;
    let leftZ = 0;
    let rightX = 0;
    let rightZ = 0;
    if (kind === "working") {
      const swing = time * P.working.tapSpeed + phase;
      leftX = P.working.arm + (still ? 0 : Math.max(0, Math.sin(swing)) * P.working.tap);
      leftZ = -P.working.spread;
      rightX = P.working.arm + (still ? 0 : Math.max(0, Math.sin(swing + Math.PI)) * P.working.tap);
      rightZ = P.working.spread;
      headX = P.working.head + (still ? 0 : Math.sin(time * P.working.nodSpeed + phase) * P.working.nod);
      eyes = blink ? P.blink.shut : P.working.eyes;
    } else if (kind === "idle") {
      headX = P.idle.head + (still ? 0 : Math.sin(time * P.idle.nodSpeed + phase) * P.idle.nod);
      headZ = P.idle.tilt;
      leftX = P.idle.arm;
      leftZ = -P.idle.spread;
      rightX = P.idle.arm;
      rightZ = P.idle.spread;
      eyes = P.idle.eyes;
      tilt = P.idle.body;
    } else if (kind === "attention") {
      headX = P.attention.head;
      headZ = still ? 0 : Math.sin(time * P.attention.swaySpeed) * P.attention.sway;
      leftX = P.attention.arm;
      leftZ = -P.attention.spread;
      rightX = P.attention.wave;
      rightZ = P.attention.waveTilt + (still ? 0 : Math.sin(time * P.attention.waveSpeed) * P.attention.waveAmp);
      eyes = blink ? P.blink.shut : P.attention.eyes;
      if (!still) lift += Math.abs(Math.sin(time * P.attention.hopSpeed)) * P.attention.hop;
    } else if (kind === "done") {
      headX = P.done.head;
      leftX = P.done.arm;
      leftZ = -P.done.spread;
      rightX = P.done.arm;
      rightZ = P.done.spread;
      eyes = P.done.eyes;
    } else {
      headX = P.failed.head;
      headZ = P.failed.tilt;
      leftX = P.failed.arm;
      leftZ = -P.failed.spread;
      rightX = P.failed.arm;
      rightZ = P.failed.spread;
      eyes = P.failed.eyes;
      tilt = P.failed.body;
    }
    if (body.burstAt !== null) {
      const age = (stamp - body.burstAt) / ORBIT.second;
      if (still || age >= P.done.jumpFor || age < 0) body.burstAt = null;
      else {
        pulsing = true;
        if (kind === "done") lift += Math.sin((age / P.done.jumpFor) * Math.PI) * P.done.jump;
      }
    }
    pose(root, x, lift, z, 0, body.face, 0, s, s, s);
    meshes.body.setMatrixAt(index, out.multiplyMatrices(root, pose(local, 0, B.body.y, 0, tilt, 0, 0)));
    set(meshes.neck, index, root, joints.neck);
    set(meshes.chest, index, root, joints.chest);
    set(meshes.pad, index, root, joints.pad);
    set(meshes.padRing, index, root, joints.padRing);
    if (kind === "working") set(meshes.keys, index, root, joints.keys);
    else meshes.keys.setMatrixAt(index, none);
    head.multiplyMatrices(root, pose(local, 0, B.head.y, 0, headX, 0, headZ));
    set(meshes.skull, index, head, joints.skull);
    set(meshes.visor, index, head, joints.visor);
    set(meshes.stem, index, head, joints.stem);
    const codex = body.family === ORBIT.families.codex;
    if (codex) {
      set(meshes.tipRing, index, head, joints.tipRing);
      meshes.tip.setMatrixAt(index, none);
    } else {
      set(meshes.tip, index, head, joints.tip);
      meshes.tipRing.setMatrixAt(index, none);
    }
    set(meshes.ear, index * 2, head, joints.earLeft);
    set(meshes.ear, index * 2 + 1, head, joints.earRight);
    for (let side = 0; side < 2; side += 1) {
      const sign = side === 0 ? -1 : 1;
      meshes.eye.setMatrixAt(index * 2 + side,
        out.multiplyMatrices(head, pose(local, sign * B.eye.x, B.eye.y, B.eye.z, 0, 0, 0, 1, eyes, 1)));
      pivot.multiplyMatrices(root, pose(local, sign * B.arm.x, B.arm.y, B.arm.z, side === 0 ? leftX : rightX, 0,
        side === 0 ? leftZ : rightZ));
      set(meshes.arm, index * 2 + side, pivot, joints.arm);
      set(meshes.mitt, index * 2 + side, pivot, joints.mitt);
    }
    if (body.lead) {
      set(meshes.boom, body.leadIndex, head, local.identity());
      set(meshes.mic, body.leadIndex, head, joints.mic);
      const H = P.halo;
      const [hx, , hz] = body.world;
      meshes.halo.setMatrixAt(body.leadIndex, pose(out, hx, B.halo.y, hz,
        ORBIT.quarter - H.tilt + Math.sin(time * H.wobbleSpeed) * H.wobble, 0, time * H.spin));
      meshes.haloOuter.setMatrixAt(body.leadIndex, pose(out, hx, B.halo.outerY, hz, ORBIT.quarter + H.tiltOuter,
        Math.sin(time * H.wobbleOuterSpeed) * H.wobbleOuter, -time * H.spinOuter));
    }
    /* 발밑 고리의 맥박: 작업 중·확인 필요·완료만, 알파는 색에 싣는다(더하기 섞기). */
    const speed = P.pulse[kind] ?? 0;
    if (speed > 0 && !still) {
      const grown = (time * speed + body.unit) % 1;
      const wide = 1 + grown * (kind === "attention" ? P.pulse.growAttention : P.pulse.grow);
      meshes.pulse.setMatrixAt(index, out.multiplyMatrices(root, pose(local, 0, B.pulse.y, 0, 0, 0, 0, wide, 1, wide)));
      const alpha = (1 - grown) * (kind === "attention" ? P.pulse.alphaAttention : P.pulse.alpha) * dim;
      meshes.pulse.setColorAt(index, scratch.ink.copy(body.pulseInk ?? scratch.ink.setRGB(0, 0, 0)).multiplyScalar(alpha));
    } else meshes.pulse.setMatrixAt(index, none);
    if (kind === "attention") {
      meshes.beam.setMatrixAt(index, pose(out, x, y + B.beam.y, z, 0, 0, 0));
      meshes.core.setMatrixAt(index, out);
    } else {
      meshes.beam.setMatrixAt(index, none);
      meshes.core.setMatrixAt(index, none);
    }
  }
  for (const mesh of scene.posed) mesh.instanceMatrix.needsUpdate = true;
  if (meshes.pulse.instanceColor) meshes.pulse.instanceColor.needsUpdate = true;
  const beam = P.beam.base + (still ? 0 : P.beam.amp * Math.sin(time * P.beam.speed));
  scene.materials.beam.uniforms.time.value = time;
  scene.materials.beam.uniforms.alpha.value = beam;
  scene.materials.screen.uniforms.time.value = still ? 0 : time;
  scene.dust.material.uniforms.time.value = time;

  /* 고른 에이전트의 발밑 고리. */
  const picked = state.bodies.get(state.selected);
  scene.select.visible = Boolean(picked && picked.kind !== "workspace");
  if (scene.select.visible) {
    scene.select.position.set(picked.world[0], picked.world[1] + B.select.y, picked.world[2]);
    scene.select.scale.setScalar(picked.scale);
    scene.materials.select.opacity = P.select.base + (still ? 0 : P.select.amp * Math.sin(time * P.select.speed));
  }
  state.pulsing = pulsing;
}

/* 빛 알갱이를 채운다: 작업 중인 자식의 레일 위로 흐르는 셋, 확인 필요인 자식의 레일 끝에서
 * 멈춰 맥박치는 셋, 방금 오간 우편마다 호 위의 하나. 머리는 밝고 꼬리는 옅어진다. */
function agentOrbitComets(state, still) {
  const scene = state.scene;
  const inks = state.inks;
  const K = state.theme;
  const C = ORBIT.comet;
  const { cometAt, cometInk, cometSize, cometAlpha, scratch } = scene;
  const room = cometAt.count;
  const positions = cometAt.array;
  const colors = cometInk.array;
  const time = state.clock;
  let count = 0;
  const point = (scene.cometPoint ??= [0, 0, 0]);
  const put = (color, size, alpha) => {
    if (count >= room) return;
    positions.set(point, count * point.length);
    colors[count * point.length] = color.r;
    colors[count * point.length + 1] = color.g;
    colors[count * point.length + 2] = color.b;
    cometSize.array[count] = size;
    cometAlpha.array[count] = alpha;
    count += 1;
  };
  const sampleRail = (rail, at) => {
    const last = rail.samples.length / point.length - 1;
    const spot = Math.min(1, Math.max(0, at)) * last;
    const index = Math.min(last - 1, Math.floor(spot));
    const blend = spot - index;
    for (let axis = 0; axis < point.length; axis += 1) {
      const a = rail.samples[index * point.length + axis];
      const b = rail.samples[(index + 1) * point.length + axis];
      point[axis] = a + (b - a) * blend;
    }
  };
  /* 레일이면 표본 사이를 곧게, 우편이면 호를 — 알갱이 하나가 설 자리를 `point`에. */
  const sampleAt = (source, at) => {
    if (source.samples) sampleRail(source, at);
    else agentOrbitAlong(source.curve, Math.min(1, Math.max(0, at)), point);
  };
  const comet = (source, at, way, ink, size, alpha) => {
    agentOrbitGlow(scratch.tailInk, ink, K.comet * C.tailK);
    scratch.headInk.copy(scratch.tailInk).lerp(agentOrbitGlow(scratch.ink, inks.spark, K.comet), C.headMix);
    sampleAt(source, at);
    put(scratch.headInk, size, alpha);
    for (let back = 1; back <= C.tail; back += 1) {
      const there = at - way * back * C.step;
      if (there < 0 || there > 1) break;
      sampleAt(source, there);
      put(scratch.tailInk, size * (1 - back / C.fade) * C.shrink, ((1 - back / C.drop) ** C.curve) * C.tailAlpha * alpha);
    }
  };
  for (const rail of state.rails) {
    if (rail.kind !== "spawned" || !rail.samples) continue;
    const child = rail.to;
    const dim = Math.min(agentOrbitDim(rail.from), agentOrbitDim(child));
    if (child.known.pose === "working") {
      for (let one = 0; one < C.perRail; one += 1) {
        const at = still ? (one / C.perRail + child.unit) % 1 : (time * C.speed + one / C.perRail + child.unit * C.phase) % 1;
        comet(rail, at, 1, inks.working, C.size, dim);
      }
    } else if (child.known.pose === "attention") {
      for (let one = 0; one < C.perRail; one += 1) {
        const pulse = still ? 1 : C.stallLow + C.stallAmp * Math.sin(time * C.stallPulse + one);
        comet(rail, C.stallAt + one * C.stallStep, 1, inks.attention, C.stallSize, pulse * dim);
      }
    }
  }
  for (const link of state.links) {
    if (!link.flowing || !link.curve) continue;
    const dim = Math.min(agentOrbitDim(link.from), agentOrbitDim(link.to));
    const at = still ? ORBIT.mail.stillAt : ((time * ORBIT.second) / ORBIT.mail.travelMs + link.phase) % 1;
    link.spot = at;
    comet(link, at, 1, link.unread ? inks.unread : inks.mail, link.unread ? ORBIT.mail.unreadSize : ORBIT.mail.size, dim);
    link.head ??= [0, 0, 0];
    agentOrbitAlong(link.curve, at, link.head);
  }
  scene.comets.geometry.setDrawRange(0, count);
  for (const attribute of [cometAt, cometInk, cometSize, cometAlpha]) attribute.needsUpdate = true;
  state.comets = count;
}

/* ---- 이름표의 자리 ------------------------------------------------------------ */

/* 이름표를 투영에 맞춘다 — 카메라나 판이 움직인 프레임에서만, 값이 움직인 것만. 쓰는 것은
 * 라벨의 style뿐이다: 자리(`translate`), 이름이 서는가(`--orbit-name`), 말풍선의 자리
 * (`--orbit-bubble-on`·`-x`·`-y`). 먼저 고른 것, 조율자와 판의 이름, 급한 상태의 차례로 자리를
 * 고르고, 비킬 자리가 없으면 머리 위 칩으로 접힌다. 프레임마다 새 배열·객체를 짓지 않는다 — 선 자리의
 * 네모, 이름표 목록, 판의 그림자는 판에 매인 것을 다시 쓰고, 차례는 판이나 고른 것이 바뀔 때만 센다. */
function agentOrbitPlaceLabels(state) {
  agentOrbitProjections += 1;
  if (!state.measured) agentOrbitMeasure(state);
  if (!state.labelOrder || state.labelOrderFor !== state.selected) agentOrbitLabelOrder(state);
  const L = ORBIT.label;
  const width = state.width;
  const height = state.height;
  const round = (value) => Math.round(value * L.round) / L.round;
  const spot = (state.probe ??= agentOrbitSeen());
  const rects = (state.rects ??= []);
  let used = 0;
  const take = (x, y, w, h) => {
    const rect = (rects[used] ??= { x: 0, y: 0, w: 0, h: 0 });
    rect.x = x;
    rect.y = y;
    rect.w = w;
    rect.h = h;
    used += 1;
  };
  for (const rect of state.reserved) take(rect.x, rect.y, rect.w, rect.h);
  const free = (x, y, w, h) => {
    for (let at = 0; at < used; at += 1) {
      const rect = rects[at];
      if (x < rect.x + rect.w + L.pad && x + w + L.pad > rect.x && y < rect.y + rect.h + L.pad
        && y + h + L.pad > rect.y) return false;
    }
    return true;
  };
  for (const body of state.bodies.values()) {
    if (body.kind === "workspace") {
      agentOrbitScreen(state, (body.left + body.right) / 2, 0, (body.back + body.front) / 2, body.seen);
    } else agentOrbitScreen(state, body.world[0], body.world[1], body.world[2], body.seen);
  }
  /* 판마다 화면의 그림자: 테까지의 윗면과 그 위로 선 로봇의 키 — 에이전트의 이름표는 이 위에 서지 않는다
   * (판의 이름은 판 앞 모서리가 제자리이고, 말풍선은 로봇 머리 위가 제자리라 비키지 않는다). 조율자의
   * 이름표만은 제 판 위, 제 머리 위에 선다(`own`) — 시안의 「조율자」 자리다. */
  for (const plate of state.plates) agentOrbitShade(state, plate, spot);
  const clear = (x, y, w, h, own) => {
    if (!free(x, y, w, h)) return false;
    for (const plate of state.plates) {
      if (plate !== own && agentOrbitShaded(plate.shade, x, y, w, h)) return false;
    }
    return true;
  };
  const tags = (state.tags ??= []);
  let named = 0;
  for (const body of state.labelOrder) {
    const label = body.label;
    const box = body.box;
    if (!label || !box) continue;
    const w = box.w;
    const h = box.h;
    const step = h + L.gap;
    const caption = body.kind === "workspace";
    let anchorX = 0;
    let anchorY = 0;
    let lean = 0;
    let tries = L.tries.agent;
    if (caption) {
      agentOrbitScreen(state, body.left + L.plateInset, L.plate, body.front, spot);
      anchorX = spot.x;
      anchorY = spot.y + L.gap;
      tries = L.tries.plate;
    } else if (body.lead) {
      agentOrbitScreen(state, body.world[0], body.world[1] + ORBIT.bot.tall * L.lead * body.scale, body.world[2], spot);
      anchorX = spot.x - w / 2;
      anchorY = spot.y - h;
      tries = L.tries.lead;
    } else if (body.plate.deck) {
      agentOrbitScreen(state, body.world[0], L.deckDrop, body.plate.front, spot);
      anchorX = spot.x - w / 2;
      anchorY = spot.y + L.gap + (state.deckRow.get(body.key) ?? 0) * step;
      tries = L.tries.deck;
    } else {
      /* 섬의 오른쪽 끝은 화면에서 비스듬하다(뒤로 갈수록 오른쪽 위로) — 이름표의 위 모서리가 끝을
       * 넘지 않게, 높이 설수록 오른쪽으로 민다. */
      agentOrbitScreen(state, body.plate.right + L.island, 0, body.world[2] - 1, spot);
      const backX = spot.x;
      const backY = spot.y;
      agentOrbitScreen(state, body.plate.right + L.island, 0, body.world[2], spot);
      lean = spot.y > backY ? (backX - spot.x) / (spot.y - backY) : 0;
      anchorX = spot.x + L.gap + (lean * h) / 2;
      anchorY = spot.y - h / 2;
    }
    let x = 0;
    let y = null;
    for (const shift of tries) {
      const there = Math.max(L.edge, Math.min(height - h - L.edge, anchorY + shift * step));
      const across = Math.max(L.edge, Math.min(width - w - L.edge, anchorX + lean * (anchorY - there)));
      if (caption ? free(across, there, w, h) : clear(across, there, w, h, body.lead ? body.plate : null)) {
        x = across;
        y = there;
        break;
      }
    }
    let placedX = 0;
    let placedY = 0;
    let placedW = w;
    let placedH = h;
    if (y !== null) {
      placedX = round(x);
      placedY = round(y);
      const tag = (tags[named] ??= { key: "", x: 0, y: 0, w: 0, h: 0 });
      tag.key = body.key;
      tag.x = placedX;
      tag.y = placedY;
      tag.w = w;
      tag.h = h;
      named += 1;
      agentOrbitName(body, "1");
    } else {
      /* 비킬 자리가 없는 이름표는 칩으로 접힌다 — 로봇은 머리 위, 판은 이름이 설 자리에. */
      const chip = state.chip;
      if (caption) {
        spot.x = anchorX + chip / 2;
        spot.y = anchorY + chip / 2;
      } else agentOrbitScreen(state, body.world[0], body.world[1] + ORBIT.bot.tall * body.scale, body.world[2], spot);
      placedX = round(spot.x - chip / 2);
      placedY = round(spot.y - chip / 2);
      placedW = chip;
      placedH = chip;
      agentOrbitName(body, "0");
    }
    take(placedX, placedY, placedW, placedH);
    const held = body.placed;
    const moved = !held || Math.abs(held.x - placedX) >= L.epsilon || Math.abs(held.y - placedY) >= L.epsilon;
    const placed = (body.placed ??= { x: 0, y: 0, w: 0, h: 0 });
    placed.w = placedW;
    placed.h = placedH;
    if (moved) {
      placed.x = placedX;
      placed.y = placedY;
      label.style.translate = `${placedX}px ${placedY}px`;
    }
  }
  tags.length = named;

  /* 말풍선도 이름표처럼 비킨다: 로봇 머리 위에서, 막히면 한 칸 위로, 거기도 막히면 접힌다
   * (급한 상태가 먼저 자리를 고른다). */
  for (const body of state.labelOrder) {
    if (body.kind === "workspace" || !body.label || !body.placed) continue;
    const bubble = body.known.bubble;
    const box = body.bubbleBox;
    if (!bubble || !box) {
      agentOrbitBubble(body, false, 0, 0);
      continue;
    }
    const w = box.w;
    const h = box.h;
    const lift = ORBIT.bot.tall * (L.bubble[bubble] ?? 1) * body.scale;
    agentOrbitScreen(state, body.world[0], body.world[1] + lift, body.world[2], spot);
    const middle = spot.x + (bubble === "idle" ? L.bubble.shift : 0);
    let top = null;
    for (const shift of L.tries.bubble) {
      const there = spot.y - h + shift * (h + L.gap);
      if (free(middle - w / 2, there, w, h)) {
        top = there;
        break;
      }
    }
    if (top === null) {
      agentOrbitBubble(body, false, 0, 0);
      continue;
    }
    take(middle - w / 2, top, w, h);
    agentOrbitBubble(body, true, round(middle - body.placed.x), round(top + h - body.placed.y));
  }
  state.named = named;
}

/* 이름표가 자리를 고르는 차례(고른 것, 조율자와 판의 이름, 급한 상태)와 본진의 줄 — 판이 바뀌거나
 * (`agentOrbitLabels`가 지운다) 고른 것이 바뀔 때만 다시 센다. */
function agentOrbitLabelOrder(state) {
  const L = ORBIT.label;
  const rank = (body) => (body.key === state.selected ? L.leadRank * 2 : 0)
    + (body.kind === "workspace" || body.lead ? L.leadRank : 0) + (body.known?.rank ?? 0);
  state.labelOrder = [...state.bodies.values()].sort((left, right) => rank(right) - rank(left)
    || (left.cell && right.cell ? left.plate.key.localeCompare(right.plate.key) || left.cell.col - right.cell.col
      || left.cell.row - right.cell.row : left.key.localeCompare(right.key)));
  state.labelOrderFor = state.selected;
  state.deckRow = new Map();
  for (const plate of state.plates) {
    if (!plate.deck) continue;
    plate.members.filter((member) => !member.lead).sort((left, right) => left.world[0] - right.world[0]
      || left.world[2] - right.world[2]).forEach((member, row) => state.deckRow.set(member.key, row));
  }
}

/* 말풍선의 자리를 라벨에 — 바뀐 값만 쓴다. */
function agentOrbitBubble(body, on, x, y) {
  const held = (body.bubbleAt ??= { on: null, x: null, y: null });
  if (held.on !== on) {
    held.on = on;
    writeStyleProperty(body.label, "--orbit-bubble-on", on ? "1" : "0");
  }
  if (!on) return;
  if (held.x !== x) {
    held.x = x;
    writeStyleProperty(body.label, "--orbit-bubble-x", `${x}px`);
  }
  if (held.y !== y) {
    held.y = y;
    writeStyleProperty(body.label, "--orbit-bubble-y", `${y}px`);
  }
}

/* 판의 화면 그림자: 테까지의 윗면에서 로봇의 키까지 선 기둥(`ORBIT.prism`의 여덟 모서리)을 비춘 볼록
 * 다각형. 직교 투영에서 그 변은 기둥의 세 모서리 방향을 따르고 높이는 화면의 세로다 — 그래서 네모 틀
 * (`x0`…`y1`)과 x·z 모서리의 두 법선(`ax·ay`, `bx·by`) 위의 그림자 끝(`a0·a1`, `b0·b1`)이 분리축의 전부다.
 * 판에 매인 객체 하나를 다시 쓴다. */
function agentOrbitShade(state, plate, spot) {
  const shade = (plate.shade ??= { x0: 0, y0: 0, x1: 0, y1: 0, ax: 0, ay: 0, a0: 0, a1: 0, bx: 0, by: 0, b0: 0, b1: 0 });
  let tallest = 0;
  for (const member of plate.members) tallest = Math.max(tallest, member.scale);
  const lift = ORBIT.bot.tall * tallest;
  const P = ORBIT.prism;
  agentOrbitCorner(state, plate, lift, P[0], spot);
  const ox = spot.x;
  const oy = spot.y;
  agentOrbitCorner(state, plate, lift, P[1], spot);
  shade.ax = oy - spot.y;
  shade.ay = spot.x - ox;
  agentOrbitCorner(state, plate, lift, P[2], spot);
  shade.bx = oy - spot.y;
  shade.by = spot.x - ox;
  shade.x0 = Infinity;
  shade.y0 = Infinity;
  shade.a0 = Infinity;
  shade.b0 = Infinity;
  shade.x1 = -Infinity;
  shade.y1 = -Infinity;
  shade.a1 = -Infinity;
  shade.b1 = -Infinity;
  for (const corner of P) {
    agentOrbitCorner(state, plate, lift, corner, spot);
    const a = spot.x * shade.ax + spot.y * shade.ay;
    const b = spot.x * shade.bx + spot.y * shade.by;
    shade.x0 = Math.min(shade.x0, spot.x);
    shade.x1 = Math.max(shade.x1, spot.x);
    shade.y0 = Math.min(shade.y0, spot.y);
    shade.y1 = Math.max(shade.y1, spot.y);
    shade.a0 = Math.min(shade.a0, a);
    shade.a1 = Math.max(shade.a1, a);
    shade.b0 = Math.min(shade.b0, b);
    shade.b1 = Math.max(shade.b1, b);
  }
}

/* 기둥의 한 모서리(`corner`: x 끝, z 끝, 높이 — 0이면 왼쪽·뒤·윗면)의 화면 자리를 `spot`에. */
function agentOrbitCorner(state, plate, lift, corner, spot) {
  return agentOrbitScreen(state, corner[0] ? plate.right + plate.rim : plate.left - plate.rim, corner[2] ? lift : 0,
    corner[1] ? plate.front + plate.rim : plate.back - plate.rim, spot);
}

/* 네모(x·y·w·h)가 판의 그림자와 겹치는가: 네모 틀로 먼저 거르고, 기둥의 두 법선 위에서 가른다.
 * 닿기만 한 것은 겹치지 않는다. */
function agentOrbitShaded(shade, x, y, w, h) {
  if (x >= shade.x1 || x + w <= shade.x0 || y >= shade.y1 || y + h <= shade.y0) return false;
  return agentOrbitCast(shade.ax, shade.ay, shade.a0, shade.a1, x, y, w, h)
    && agentOrbitCast(shade.bx, shade.by, shade.b0, shade.b1, x, y, w, h);
}

/* 법선(nx·ny) 위에서 네모의 그림자가 [low, high]와 겹치는가. 길이 없는 법선은 가르지 못한다. */
function agentOrbitCast(nx, ny, low, high, x, y, w, h) {
  if (nx === 0 && ny === 0) return true;
  const base = x * nx + y * ny;
  const min = base + Math.min(0, w * nx) + Math.min(0, h * ny);
  const max = base + Math.max(0, w * nx) + Math.max(0, h * ny);
  return max > low && min < high;
}

function agentOrbitName(body, value) {
  if (body.named === value) return;
  body.named = value;
  writeStyleProperty(body.label, "--orbit-name", value);
}

/* ---- 그림 -------------------------------------------------------------------- */

/* 한 장. `step`은 지난 그림 뒤로 흐른 밀리초(정지 화면은 0)이고, 카메라와 움직임은 손이
 * 오른 동안 멈춘다(`hold`). 차례: 잉크 → 카메라 → 자세와 알갱이 → 이름표(카메라가 움직인
 * 판만) → 장면(빛번짐이 있으면 장면 타깃 → 문턱 → 다섯 단 → 합성). */
function agentOrbitDraw(state, step, stamp, { still = false } = {}) {
  /* 컨텍스트를 막 잃은 판은 그리지 않는다 — 유실의 소식(`webglcontextlost`)은 한 박자 늦게 온다. */
  if (state.gl.renderer.getContext().isContextLost()) return;
  const inks = agentOrbitInks(state);
  if (!inks || !state.scene || state.width <= 0 || state.height <= 0 || !state.bounds) return;
  const scene = state.scene;
  const { renderer, bloom } = state.gl;
  /* 멈춤은 곧게 줄어 `holdMs`에 정확히 선다 — 지수로 다가가면 끝내 조금씩 흐른다. */
  const ramp = still ? 1 : step / ORBIT.holdMs;
  state.hold = state.holdTarget < state.hold
    ? Math.max(state.holdTarget, state.hold - ramp) : Math.min(state.holdTarget, state.hold + ramp);
  const sim = still ? 0 : step * state.hold;
  if (!agentOrbitReduced()) agentOrbitTurn(state, step, sim);
  state.clock += sim / ORBIT.second;
  if (!scene.painted) agentOrbitPaint(state);
  agentOrbitPose(state, sim, stamp, still);
  agentOrbitComets(state, still);

  const lens = agentOrbitLens(state);
  const moved = !agentOrbitSameLens(state.lens, lens) || !state.placed;
  agentOrbitCameraTo(state, lens);
  if (moved) {
    state.lens = lens;
    agentOrbitPlaceLabels(state);
    state.placed = true;
  }
  const pixels = state.dpr * lens.scale;
  scene.comets.material.uniforms.scale.value = pixels;
  scene.dust.material.uniforms.scale.value = state.dpr;

  renderer.info.reset();
  if (!bloom) {
    renderer.setRenderTarget(null);
    renderer.render(scene.scene, scene.camera);
    state.gl.sceneCalls = renderer.info.render.calls;
  } else {
    renderer.setRenderTarget(bloom.scene);
    renderer.render(scene.scene, scene.camera);
    state.gl.sceneCalls = renderer.info.render.calls;
    const pass = (material, target) => {
      bloom.quad.material = material;
      renderer.setRenderTarget(target);
      renderer.render(bloom.shots, bloom.lens);
    };
    bloom.threshold.uniforms.tDiffuse.value = bloom.scene.texture;
    pass(bloom.threshold, bloom.high);
    let input = bloom.high;
    bloom.blur.forEach((material, at) => {
      const across = bloom.across[at];
      const down = bloom.down[at];
      material.uniforms.tDiffuse.value = input.texture;
      material.uniforms.texel.value.set(1 / input.width, 1 / input.height);
      material.uniforms.dir.value.set(1, 0);
      pass(material, across);
      material.uniforms.tDiffuse.value = across.texture;
      material.uniforms.texel.value.set(1 / across.width, 1 / across.height);
      material.uniforms.dir.value.set(0, 1);
      pass(material, down);
      input = down;
    });
    const uniforms = bloom.composite.uniforms;
    uniforms.tBase.value = bloom.scene.texture;
    ORBIT.render.taps.forEach((tap, at) => {
      uniforms[tap].value = bloom.down[at].texture;
    });
    pass(bloom.composite, null);
  }
  state.gl.calls = renderer.info.render.calls;
  state.gl.triangles = renderer.info.render.triangles;
  state.dirty = false;
  agentOrbitCool(state);
  agentOrbitFrames += 1;
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
  const rails = state?.rails ?? [];
  const links = state?.links ?? [];
  const count = (list, kind) => list.filter((one) => one.kind === kind).length;
  const memory = state?.gl?.renderer.info.memory;
  return {
    applied: agentOrbitApplied,
    frames: agentOrbitFrames,
    ticks: agentOrbitTicks,
    pulses: agentOrbitPulses,
    inkReads: agentOrbitInkReads,
    projections: agentOrbitProjections,
    builds: agentOrbitBuilds,
    running: agentOrbitFrame !== null,
    reduced: agentOrbitReduced(),
    broken: agentOrbitBroken,
    flowing: state?.flowing ?? false,
    workspaces: count(bodies, "workspace"),
    agents: count(bodies, "agent"),
    children: count(bodies, "child"),
    leads: bodies.filter((body) => body.lead).length,
    bodies: bodies.length,
    edges: { spawned: count(rails, "spawned"), dependency: count(rails, "dependency") },
    links: links.length,
    particles: links.filter((link) => link.flowing).length,
    bright: links.filter((link) => link.unread && link.flowing).length,
    comets: state?.comets ?? 0,
    scale: state?.scale ?? null,
    zoom: agentGraphZoom,
    paused: state ? state.holdTarget === 0 : null,
    labels: state?.host.childElementCount ?? 0,
    named: state?.named ?? 0,
    tags: state?.tags ?? [],
    camera: state ? { yaw: agentOrbitYaw(state), pitch: state.camera.pitch, spin: state.camera.spin } : null,
    order: [...bodies].sort((left, right) => right.seen.depth - left.seen.depth).map((body) => body.key),
    bloom: Boolean(state?.gl?.bloom),
    gpu: state?.gl ? { calls: state.gl.calls, sceneCalls: state.gl.sceneCalls, triangles: state.gl.triangles,
      geometries: memory.geometries,
      textures: memory.textures, programs: state.gl.renderer.info.programs?.length ?? 0 } : null,
  };
}
