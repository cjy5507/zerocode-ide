/* ---- 지식 그래프의 우주 보기 (t-12443, 승인된 시안 v4 「실제 우주」) ----------------------
 *
 * 사람이 고른 그림이다(09-28 18:3x, 「지금 화면 좋아」): 주제는 은하, 작은 묶음은 성단, 홀로 선
 * 쪽은 떠돌이 별, 쪽 하나는 이름 있는 별이다. 별의 밝기는 연결 수이고 빛깔은 최근 고침이며, 주제
 * 사이의 굵은 연결은 흐르는 필라멘트다. 시안(docs/design/knowledge-graph-3d-v4/, 디자이너 w-12381)의
 * 셰이더와 수를 그대로 옮긴다 — 지어낸 수는 없다. 시안과 다른 것은 자료다: 시안은 지어낸 볼트를
 * 그렸고, 이 파일은 창의 모델(`knowledgeModel`)과 평면 지도의 배치(`knowledgeLayout`)에서 우주를
 * 짓는다. 은하의 자리는 평면 지도의 원반 자리 그대로이고(x·z) 높이만 더한다 — 외운 지도가 흔들리지
 * 않는다(6차 설계 §4-a).
 *
 * 어디서 서는가. 전체 지도에서, WebGL2가 서는 판에서, 사람이 3D를 고른 동안(고른 적이 없으면 표의
 * 기본 줄, `KNOWLEDGE_DIMENSIONS`). 주변 탐색·관계 편집·점 끌기는 평면의 일이다(설계 §4-f). 평면
 * 지도는 이 판 아래에서 그대로 살고(두 페인터 중 하나), 우주가 서 있는 동안만 가려진다.
 *
 * 그리는 일은 있을 때만 한다. 비행·미끄러짐·움직임(아주 느린 회전과 필라멘트의 흐름) 중에만 프레임을
 * 잇고, 쉬면 한 장도 그리지 않는다. 판이 가려지면 멈춘다. 떠날 때는 GPU의 것(기하·텍스처·프로그램·
 * 렌더 타깃·문맥)을 전부 놓는다. */

/* 전체 지도의 두 얼굴. 표의 순서가 머리의 순서이고, `first`가 사람이 고른 적 없는 판의 기본이다 —
 * 기본값은 이 한 줄에만 산다. 3D는 WebGL2가 서는 판에서만 설 수 있다(`knowledgeDimensionNow`). */
const KNOWLEDGE_DIMENSIONS = Object.freeze([
  Object.freeze({ id: "2d", key: "knowledge.dimension2d", word: "평면 지도로 보기", short: "2D" }),
  Object.freeze({ id: "3d", key: "knowledge.dimension3d", word: "우주로 보기", short: "3D", first: true }),
]);
const KNOWLEDGE_DIMENSION_FIRST = KNOWLEDGE_DIMENSIONS.find((row) => row.first === true)?.id
  ?? KNOWLEDGE_DIMENSIONS[0].id;

/* 시안의 수 — 값은 tokens.css의 `--knowledge-3d-*` 한 곳에 있고 판마다 한 번 읽는다
 * (`knowledgeUniverseTuning`). 이름은 시안의 자리를 따른다(PORTING.md §3, index.html의 `computeHome`·
 * `flyTo`·`frame`·손). 셰이더 안의 모양 상수(빛의 식)는 셰이더의 글자 그대로다. */
const KNOWLEDGE_UNIVERSE_TOKENS = Object.freeze({
  /* 시야각(도)과 앞뒤 자름 — 뒤 자름은 우주가 넓어져도 배경 별이 잘리지 않게 닿는 거리의 배수까지. */
  fov: "--knowledge-3d-fov",
  near: "--knowledge-3d-near",
  far: "--knowledge-3d-far",
  farReach: "--knowledge-3d-far-reach",
  pixelCap: "--knowledge-3d-dpr-cap",
  /* 처음 자리(`computeHome`): 비스듬히 내려다보는 두 각, 닿는 거리의 여백과 최소값, 판의 비의 하한,
   * 세로의 몫, 물러섬, 목표점의 들림. */
  homeYaw: "--knowledge-3d-home-yaw",
  homePitch: "--knowledge-3d-home-pitch",
  homeFit: "--knowledge-3d-home-fit",
  homePad: "--knowledge-3d-home-pad",
  homeLeast: "--knowledge-3d-home-least",
  homeAspect: "--knowledge-3d-home-aspect",
  homeTall: "--knowledge-3d-home-tall",
  homeLift: "--knowledge-3d-home-lift",
  /* 평면 지도의 자세 — 거의 수직으로 내려다보고, 별은 이 높이의 판 위에 선다. */
  flatPitch: "--knowledge-3d-flat-pitch",
  flatHeight: "--knowledge-3d-flat-height",
  pitchLow: "--knowledge-3d-pitch-low",
  pitchHigh: "--knowledge-3d-pitch-high",
  /* 다가갈 수 있는 가장 가까운 거리, 물러설 수 있는 처음 거리의 배수, 바퀴·단추의 걸음. */
  zoomMin: "--knowledge-3d-zoom-min",
  zoomMax: "--knowledge-3d-zoom-max",
  zoomWheel: "--knowledge-3d-zoom-wheel",
  zoomStep: "--knowledge-3d-zoom-step",
  zoomStepMs: "--knowledge-3d-zoom-step-ms",
  /* 손: 끌기의 문턱(px, 가로+세로), 궤도·옮기기의 px당 몫, 놓은 뒤 미끄러짐(줄어듦·멈춤 문턱·속도 상한
   * 둘·세로의 몫·놓기 직전의 창), 화살표 한 번의 궤도. */
  dragSlop: "--knowledge-3d-drag-slop",
  dragYaw: "--knowledge-3d-drag-yaw",
  dragPitch: "--knowledge-3d-drag-pitch",
  dragPan: "--knowledge-3d-drag-pan",
  glideDecay: "--knowledge-3d-glide-decay",
  glideStop: "--knowledge-3d-glide-stop",
  glideYaw: "--knowledge-3d-glide-yaw",
  glidePitch: "--knowledge-3d-glide-pitch",
  glidePitchShare: "--knowledge-3d-glide-pitch-share",
  glideWindow: "--knowledge-3d-glide-window",
  keyTurn: "--knowledge-3d-key-turn",
  /* 쉬는 동안 돌기: 쉰 시간, 도는 빠르기(rad/s), 올라서는 빠르기. 한 프레임의 시간 상한과 첫 프레임. */
  idleDelay: "--knowledge-3d-idle-delay",
  idleSpin: "--knowledge-3d-idle-spin",
  idleRamp: "--knowledge-3d-idle-ramp",
  frameMost: "--knowledge-3d-frame-most",
  frameFirst: "--knowledge-3d-frame-first",
  /* 문을 연 판·처음 자리로 돌아간 판·움직임을 켠 판이 이미 쉰 것으로 치는 시간. */
  openRest: "--knowledge-3d-open-rest",
  homeRest: "--knowledge-3d-home-rest",
  wakeRest: "--knowledge-3d-wake-rest",
  /* 비행(ms): 떠오름·내려앉음·보통·처음 자리. */
  /* 날아가기(시안 `select`·`focusTopic`·`showFlow`): 쪽은 거리 max(`fly-page-dist`, R × `fly-page-reach`)에
   * 은하를 거의 정면으로(`face-page`), 은하는 R × `fly-galaxy-reach` + `fly-galaxy-pad`(`face-galaxy`), 흐름은 두
   * 은하 사이 거리 × `fly-flow-reach` + `fly-flow-pad`에 pitch `fly-flow-pitch`. 정면의 pitch는 `face-pitch-*` 안. */
  flyPage: "--knowledge-3d-fly-page",
  flyPageDist: "--knowledge-3d-fly-page-dist",
  flyPageReach: "--knowledge-3d-fly-page-reach",
  facePage: "--knowledge-3d-face-page",
  faceGalaxy: "--knowledge-3d-face-galaxy",
  facePitchLow: "--knowledge-3d-face-pitch-low",
  facePitchHigh: "--knowledge-3d-face-pitch-high",
  flyGalaxyReach: "--knowledge-3d-fly-galaxy-reach",
  flyGalaxyPad: "--knowledge-3d-fly-galaxy-pad",
  flyFlow: "--knowledge-3d-fly-flow",
  flyFlowReach: "--knowledge-3d-fly-flow-reach",
  flyFlowPad: "--knowledge-3d-fly-flow-pad",
  flyFlowPitch: "--knowledge-3d-fly-flow-pitch",
  rise: "--knowledge-3d-rise",
  fold: "--knowledge-3d-fold",
  fly: "--knowledge-3d-fly",
  homeMs: "--knowledge-3d-home-ms",
  noticeMs: "--knowledge-3d-notice-ms",
  /* 평면 자리에서 세계로: 은하 반지름 = √쪽 × `world-root` × (쪽 / `world-pages`)^`world-spread`. */
  worldRoot: "--knowledge-3d-world-root",
  worldPages: "--knowledge-3d-world-pages",
  worldSpread: "--knowledge-3d-world-spread",
  /* 높이의 반폭 — 닿는 거리에 대한 몫(은하 ±0.17, 성단 ±0.21, 떠돌이 ±0.25). */
  liftGalaxy: "--knowledge-3d-lift-galaxy",
  liftCluster: "--knowledge-3d-lift-cluster",
  liftOrphan: "--knowledge-3d-lift-orphan",
  /* 이름 있는 별: 밝기 = ((1 + 연결) / (1 + 가운데값))^지수, 빛깔 = 오늘 1 → `age-span`일 0(로그, 무릎
   * `age-scale`일). */
  starLumExp: "--knowledge-3d-star-lum-exp",
  /* 연결 비의 천장 — 허브의 비가 시안(850쪽 최대 13.5)보다 크게 나는 볼트에서 가까이 가기 전부터 빛살이 크기
   * 천장에 닿지 않게(디자이너 m-12546의 2). */
  starRatioCap: "--knowledge-3d-star-ratio-cap",
  starAgeScale: "--knowledge-3d-star-age-scale",
  starAgeSpan: "--knowledge-3d-star-age-span",
  /* 은하 짓기(시안 `universe`, 디자이너 답 m-12467의 1): 이름 있는 군집 중 순위 앞에서부터 은하가 되는 수의
   * 상한과 그 문턱(쪽 수 ≥ max(`galaxy-least`, `galaxy-share` × 쪽 / `world-pages`)), 성운을 두르는 성단의 수. */
  galaxyMax: "--knowledge-3d-galaxy-max",
  galaxyLeast: "--knowledge-3d-galaxy-least",
  galaxyShare: "--knowledge-3d-galaxy-share",
  nebulaMax: "--knowledge-3d-nebula-max",
  /* 반지름: 은하 = 평면 원반 × `galaxy-radius`, 성단 = 원반 × (`cluster-sphere` / `world-root`) × `cluster-grow`
   * + `cluster-pad`. */
  galaxyRadius: "--knowledge-3d-galaxy-radius",
  clusterSphere: "--knowledge-3d-cluster-sphere",
  clusterGrow: "--knowledge-3d-cluster-grow",
  clusterPad: "--knowledge-3d-cluster-pad",
  /* 기울기: 처음 시점 쪽으로 기울인 축(몫 `normal-lean`)에서 `tilt-least` + 씨앗 × `tilt-span` rad. */
  normalLean: "--knowledge-3d-normal-lean",
  tiltLeast: "--knowledge-3d-tilt-least",
  tiltSpan: "--knowledge-3d-tilt-span",
  /* 형태: 팔 수(넓은 판은 `arms-every`의 나머지 셋째), 감김(기본·막대·나선의 바닥과 폭), 막대, 팽대부(기본·막대·
   * 나선의 바닥과 폭·타원), 먼지(기본·불규칙). */
  arms: "--knowledge-3d-arms",
  armsWide: "--knowledge-3d-arms-wide",
  armsEvery: "--knowledge-3d-arms-every",
  winding: "--knowledge-3d-winding",
  windingBar: "--knowledge-3d-winding-bar",
  windingLeast: "--knowledge-3d-winding-least",
  windingSpan: "--knowledge-3d-winding-span",
  bar: "--knowledge-3d-bar",
  bulge: "--knowledge-3d-bulge",
  bulgeBar: "--knowledge-3d-bulge-bar",
  bulgeLeast: "--knowledge-3d-bulge-least",
  bulgeSpan: "--knowledge-3d-bulge-span",
  bulgeElliptical: "--knowledge-3d-bulge-elliptical",
  dust: "--knowledge-3d-dust",
  dustIrregular: "--knowledge-3d-dust-irregular",
  /* 형태 규칙의 수: 막대 나선 순위의 상한, 불규칙(끝에서 몇, 은하가 몇 이상일 때), 타원(순위 몇부터, 몇마다,
   * 활동이 몇 밑일 때), 활동의 창(날). */
  barredRanks: "--knowledge-3d-barred-ranks",
  irregularLast: "--knowledge-3d-irregular-last",
  irregularLeast: "--knowledge-3d-irregular-least",
  ellipticalFrom: "--knowledge-3d-elliptical-from",
  ellipticalEvery: "--knowledge-3d-elliptical-every",
  ellipticalActivity: "--knowledge-3d-elliptical-activity",
  activityDays: "--knowledge-3d-activity-days",
  /* 몸의 이름 없는 별 수: 은하 = `decor-least` + `decor-grow` × √(쪽 / 가장 큰 군집의 쪽), 성단, 배경. */
  decorLeast: "--knowledge-3d-decor-least",
  decorGrow: "--knowledge-3d-decor-grow",
  decorCluster: "--knowledge-3d-decor-cluster",
  decorSky: "--knowledge-3d-decor-sky",
  /* 원반·핵·성운의 사각형 반지름(R의 배수, 시안 `bb`): 원반, 핵, 타원의 핵, 성단의 성운 셋. */
  disk: "--knowledge-3d-disk",
  core: "--knowledge-3d-core",
  coreElliptical: "--knowledge-3d-core-elliptical",
  nebulaOuter: "--knowledge-3d-nebula-outer",
  nebulaMid: "--knowledge-3d-nebula-mid",
  nebulaInner: "--knowledge-3d-nebula-inner",
  /* 은하 빛깔 = `galaxy-tint-base` + (1 − base) × 선형(색 칸의 어두운 테마 값). */
  galaxyTintBase: "--knowledge-3d-galaxy-tint-base",
  /* 필라멘트(시안 `buildScene`): 두 은하 사이 선이 몇 개부터인가, 몇 쌍까지인가(셰이더 배열의 크기까지), 입자 수
   * = `filament-least-points` + `filament-grow-points` × √(n / 가장 굵은 n). */
  filamentLeast: "--knowledge-3d-filament-least",
  filamentMost: "--knowledge-3d-filament-most",
  filamentLeastPoints: "--knowledge-3d-filament-least-points",
  filamentGrowPoints: "--knowledge-3d-filament-grow-points",
  /* 빛 번짐 사다리의 단 수와 세기(문턱 없음), 합성의 노출(테마마다). */
  bloomLevels: "--knowledge-3d-bloom-levels",
  bloomStrength: "--knowledge-3d-bloom-strength",
  /* 이름표(시안 `updateLabels`·`galaxyAnchor`·`showTip`, PORTING §2.7) — 들림이 이 값을 넘은 판에서만 선다. 명판:
   * 은하 가장자리 고리(반지름의 몫·점 수·보여야 할 점 수)의 화면에서 가장 낮은 곳 아래 틈만큼, 판 가장자리
   * 여백과 위·아래 띠, 둘레의 틈, 큰 명판을 입는 순위의 수, 가까이(처음 거리의 몫 밑) 가면 고른 것만. */
  labelLift: "--knowledge-3d-label-lift",
  plateRing: "--knowledge-3d-plate-ring",
  platePoints: "--knowledge-3d-plate-points",
  plateSeen: "--knowledge-3d-plate-seen",
  plateGap: "--knowledge-3d-plate-gap",
  plateEdge: "--knowledge-3d-plate-edge",
  plateTop: "--knowledge-3d-plate-top",
  plateBottom: "--knowledge-3d-plate-bottom",
  plateRoom: "--knowledge-3d-plate-room",
  plateLarge: "--knowledge-3d-plate-large",
  /* 「가까이」의 문턱 = max(처음 거리 × `close-in`, (은하로 날아온 거리 5.2R + 40) × `close-in-fly`) — R은 고른 은하(없으면
   * 고른 쪽이 든 은하)의 반지름. 날아온 자리는 은하의 크기와 상관없이 가깝고, 조금 더 물러나면 먼 모습이다
   * (디자이너 m-12642). */
  closeIn: "--knowledge-3d-close-in",
  closeInFly: "--knowledge-3d-close-in-fly",
  /* 쪽 이름: 풀, 쉴 때 이름을 다는 가장 밝은 별의 수, 후보 수와 화면 반지름 문턱, 초점의 이웃·찾은 것의 덤, 후보를
   * 고르는 위·아래 띠, 별에서 비킨 거리(반지름의 몫·최소·여백), 판 가장자리 여백, 놓는 위·아래 띠, 한 줄의 높이. */
  labels: "--knowledge-3d-labels",
  labelsRest: "--knowledge-3d-labels-rest",
  /* 고른 은하 가까이(쪽 초점도 찾기도 없을 때)의 이름 수 — 후보는 그 은하의 쪽 전부다(디자이너 m-12670). */
  labelsClose: "--knowledge-3d-labels-close",
  labelsCandidates: "--knowledge-3d-labels-candidates",
  labelLeast: "--knowledge-3d-label-least",
  labelNeighbour: "--knowledge-3d-label-neighbour",
  labelHit: "--knowledge-3d-label-hit",
  labelScan: "--knowledge-3d-label-scan",
  labelOff: "--knowledge-3d-label-off",
  labelOffLeast: "--knowledge-3d-label-off-least",
  labelOffPad: "--knowledge-3d-label-off-pad",
  labelEdge: "--knowledge-3d-label-edge",
  labelTop: "--knowledge-3d-label-top",
  labelBottom: "--knowledge-3d-label-bottom",
  labelTall: "--knowledge-3d-label-tall",
  /* 필라멘트 수: 굵은 몇 쌍, 가운데 자리(t)와 화살이 가리키는 t, 한 줄의 높이·둘레의 틈, 화살 각의 걸음(1/rad). */
  flowTags: "--knowledge-3d-flow-tags",
  flowTagAt: "--knowledge-3d-flow-tag-at",
  flowTagAhead: "--knowledge-3d-flow-tag-ahead",
  /* 가운데 자리가 막히면 그 앞 · 뒤로 이만큼 옮긴 자리를 차례로 시험한다(디자이너 m-12642). */
  flowTagShift: "--knowledge-3d-flow-tag-shift",
  flowTagTall: "--knowledge-3d-flow-tag-tall",
  flowTagRoom: "--knowledge-3d-flow-tag-room",
  flowTagTurn: "--knowledge-3d-flow-tag-turn",
  /* 팁: 포인터에서 오른쪽·위로 비킨 자리, 판 오른쪽 끝에서 비울 넓이, 위의 최소, 둘레의 틈. */
  tipRight: "--knowledge-3d-tip-right",
  tipUp: "--knowledge-3d-tip-up",
  tipRoom: "--knowledge-3d-tip-room",
  tipTop: "--knowledge-3d-tip-top",
  tipGap: "--knowledge-3d-tip-gap",
});

/* 테마마다 다른 수 — 테마가 바뀌면 다시 읽는다(`knowledgeUniverseInks`). */
const KNOWLEDGE_UNIVERSE_THEMED = Object.freeze({
  exposure: "--knowledge-3d-exposure",
});

/* 셰이더와 묶인 수 — 한 표(시안 `buildScene`·`materials.decor`, 디자이너 답 m-12467의 1). 은하·성단은 텍스처
 * 한 장의 줄이고(`texels` × `rows`, RGBA32F), 장식 입자의 번호 = 무리 × `groupSpan` + 차례가 float에서
 * 정확하려면 2^24 밑이어야 한다: 무리 0–127 은하·성단, 128 배경 별, 130부터 필라멘트. 떠돌이 별의 초점은
 * 어느 줄도 아닌 번호다. */
const KNOWLEDGE_UNIVERSE_SHAPE = Object.freeze({
  texels: 8,
  rows: 128,
  groupSpan: 65536,
  skyGroup: 128,
  bundleGroup: 130,
  bundles: 48,
  noGalaxy: 999,
  /* 별자리 선: 선 하나의 마디 수, 두 끝의 줄 번호를 한 수에 싣는 밑(줄 128과 떠돌이 번호 128보다 크게 — 시안은
   * 줄 32에 64를 썼다), 선마다의 흐름 위상을 짓는 합동식(시안 그대로). */
  lineSegments: 16,
  linePack: 256,
  lineSeed: 12345,
  lineMultiplier: 16807,
  lineModulus: 2147483647,
  /* 쪽 자리 텍스처 한 줄의 폭(시안 `TW`). */
  nodeWide: 1024,
  /* 이 깊이(w) 밑은 카메라 뒤다 — 투영하지 않는다(시안 `projectAll`). */
  behind: 0.1,
});

/* 별의 크기 — 셰이더(`materials.stars`)와 고르기(`starSizeCss`)가 같은 수를 읽는다: 흐름 = 광도 ×
 * clamp((기준 거리 / 거리)², 바닥, 천장), 크기 = clamp(배율 × 흐름^지수, 가장 작게, 가장 크게) CSS px. 고르기의 반지름은
 * max(크기의 반 × `pickShare`, `pickLeast` px), 겹치면 가까운 별이 먼저(깊이 × `pickDepth`). */
const KNOWLEDGE_UNIVERSE_STAR = Object.freeze({
  fluxLeast: 0.15,
  fluxMost: 6,
  sizeScale: 4,
  sizePower: 0.38,
  sizeLeast: 2.6,
  sizeMost: 34,
  pickShare: 0.8,
  pickLeast: 6,
  pickDepth: 0.0004,
});

/* 필라멘트의 곡선 — 장식 셰이더의 필라멘트 가지와 이름표 자리(`knowledgeUniverseFilamentPoint`, 시안
 * `filamentPoint`)가 같은 수를 이 표에서 읽는다: 두 은하의 반지름 × `reachShare`만큼 안에서 시작하고 끝나되 시작은
 * `startMost`, 끝은 `endLeast`를 넘지 않고, 옆으로 길이 × `side`(bend 다섯 단계), 위로 길이 × `up`만큼 휜다. 거의
 * 수직인 쌍(옆 방향의 길이 < `upright`)의 옆은 x축이다. */
const KNOWLEDGE_UNIVERSE_CURVE = Object.freeze({
  reachShare: 0.55,
  startMost: 0.42,
  endLeast: 0.58,
  side: 0.1,
  up: 0.04,
  upright: 0.001,
});

/* 쪽 이름 후보의 차례 — 초점의 별은 언제나 먼저, 쉴 때의 밝은 별은 제 차례대로(시안 `updateLabels`의 1e6·1e5). */
const KNOWLEDGE_UNIVERSE_LABEL_FIRST = Object.freeze({ focus: 1e6, bright: 1e5 });

/* 무대 위에 늘 서 있는 상자 — 이름표는 이 밑에 서지 않는다(시안의 고정 UI 상자 자리). 앱에서는 배율 상자와 열린
 * 범례이고, 놓는 프레임마다 잰다. */
const KNOWLEDGE_UNIVERSE_COVERS = Object.freeze([".knowledge-zoom", ".knowledge-legend"]);

/* 셰이더에 싣는 수 — GLSL은 정수 글자를 실수로 읽지 않으므로 소수점을 붙인다. */
function knowledgeUniverseFloat(value) {
  return Number.isInteger(value) ? value.toFixed(1) : String(value);
}

/* 은하의 형태 번호 — 셰이더가 텍셀 1의 w로 읽는다(시안 `kg-data.js`의 `TYPE`). */
const KNOWLEDGE_UNIVERSE_TYPES = Object.freeze({ spiral: 0, barred: 1, elliptical: 2, irregular: 3, cluster: 4 });
/* 형태의 낱말 — 번호가 곧 자리다(시안 `TYPE_NAME`). */
const KNOWLEDGE_UNIVERSE_TYPE_WORDS = Object.freeze([
  Object.freeze({ key: "knowledge.galaxySpiral", word: "나선 은하" }),
  Object.freeze({ key: "knowledge.galaxyBarred", word: "막대 나선 은하" }),
  Object.freeze({ key: "knowledge.galaxyElliptical", word: "타원 은하" }),
  Object.freeze({ key: "knowledge.galaxyIrregular", word: "불규칙 은하" }),
  Object.freeze({ key: "knowledge.galaxyCluster", word: "산개 성단" }),
]);

/* 인스펙터의 우주 절(디자이너 m-12467의 6): 필라멘트 몇 줄, 「얼마 전」의 문턱(시안 `ageText`: 하루 밑은 오늘, 두 주
 * 밑은 날, 두 달 밑은 주, 한 해 밑은 달, 그 위는 해). */
const KNOWLEDGE_UNIVERSE_LISTED = 6;
const KNOWLEDGE_UNIVERSE_AGE = Object.freeze({ today: 1, days: 14, weeks: 60, months: 365, week: 7, month: 30, year: 365 });

/* 이름 있는 별의 자리 — 시안 `universe` 3의 모양 수. 셰이더의 몸 입자와 같은 식이라 모양을 정하는 수이고,
 * 셰이더 안의 수처럼 이 표 한 곳에 산다: 핵(쪽의 몫·반지름·두께), 팔(안쪽·폭·거듭제곱·흔들림·각의 흔들림·
 * 두께·로그의 바닥), 막대의 흔들림, 타원(반지름·거듭제곱·눌림·두께), 불규칙(덩어리 수·돌림·안쪽·걸음·퍼짐·두께),
 * 성단(반지름·거듭제곱). */
const KNOWLEDGE_UNIVERSE_SEATS = Object.freeze({
  coreShare: 0.05,
  coreRadius: 0.07,
  coreLift: 0.05,
  armInner: 0.13,
  armSpan: 0.8,
  armPower: 0.85,
  armJitter: 0.14,
  armAngle: 0.34,
  armLift: 0.05,
  armFloor: 0.1,
  barJitter: 0.12,
  ellipticalRadius: 0.72,
  ellipticalPower: 0.75,
  ellipticalFlat: 0.8,
  ellipticalLift: 0.6,
  clumps: 4,
  clumpTurn: 1.9,
  clumpInner: 0.12,
  clumpStep: 0.2,
  clumpSpread: 0.26,
  clumpLift: 0.14,
  clusterRadius: 0.75,
  clusterPower: 0.65,
  /* 필라멘트 쌍마다의 휨(옆으로 다섯 단계)과 흐름의 위상 — 쌍의 두 줄 번호에서(시안 `uBundle2`). */
  bendLow: 7,
  bendHigh: 3,
  bendSteps: 5,
  phaseLow: 13,
  phaseSteps: 7,
});

/* 판마다 한 번 읽는 수 — 테마와 무관하다(색은 따로 읽는다). */
const knowledgeUniverseTunings = new WeakMap();
function knowledgeUniverseTuning(view) {
  const held = knowledgeUniverseTunings.get(view);
  if (held !== undefined) return held;
  const style = getComputedStyle(view);
  const tuning = Object.freeze(Object.fromEntries(Object.entries(KNOWLEDGE_UNIVERSE_TOKENS)
    .map(([key, name]) => [key, Number.parseFloat(style.getPropertyValue(name))])));
  knowledgeUniverseTunings.set(view, tuning);
  return tuning;
}

/* 사람이 고른 전체 지도의 얼굴 — `null`은 고른 적이 없다는 뜻이다(표의 기본 줄). 문을 열 때 이
 * 볼트가 기억하는 쪽을 읽는다(`applyKnowledgeEntry`). */
let knowledgeDimension = null;
/* 판마다 우주 하나. 판이 사라지면 함께 사라진다. */
const knowledgeUniverses = new WeakMap();
/* 우주를 짓다가 무너진 창(셰이더가 서지 않는 드라이버) — 이 창의 남은 시간 동안 3D는 까닭을 말하고
 * 서지 않는다. */
let knowledgeUniverseFailed = false;
/* 사람이 3D를 누른 판 — 다음에 서는 우주는 평면 자리에서 떠오른다(z가 나는 전환). 문을 연 판은
 * 곧바로 처음 자리에 선다. */
let knowledgeUniverseRising = false;
/* 사람의 「움직임」(시안 `setMotion`, 디자이너 m-12467의 5) — 쉬고 9초 뒤의 느린 돌기와 필라멘트의 흐름·별의 반짝임이
 * 한 단추에 묶인다. 이 창에서 한 번 고르면 다음에 서는 우주도 따른다. 움직임을 줄이라는 판에서는 늘 멈춘다. */
let knowledgeUniverseMotion = true;

/* 이 창이 우주를 그릴 수 있는가 — WebGL2와 렌더러(`knowledgeGlSupported`). */
function knowledgeUniverseAble() {
  return !knowledgeUniverseFailed && knowledgeGlSupported();
}

/* 지금 전체 지도가 서야 할 얼굴. 고른 것, 없으면 표의 기본 — 우주를 그릴 수 없는 판에서는 평면이다.
 * 그림의 크기는 읽지 않는다: 볼트의 크기가 사람의 화면을 바꾸지 않는다(설계 knowledge-graph-20260914). */
function knowledgeDimensionNow() {
  const wanted = knowledgeDimension ?? KNOWLEDGE_DIMENSION_FIRST;
  return wanted === "3d" && knowledgeUniverseAble() ? "3d" : "2d";
}

/* 이 판에 우주가 서야 하는가. */
function knowledgeUniverseWanted(view, layout) {
  return knowledgeMode === KNOWLEDGE_MODES[0].id && knowledgeDimensionNow() === "3d"
    && layout !== undefined && layout.count > 0 && !view.hidden;
}

/* 머리의 「2D | 3D」. 눌린 쪽은 지금 서 있는 얼굴이다(주변 탐색은 평면). 우주를 그릴 수 없는 판에서
 * 3D는 누를 수 없는 채로 서고(`aria-disabled` — 초점과 손은 받아 팁이 까닭을 말한다), 문맥을 잃은
 * 판에서는 누를 수 있다(돌아오면 연다고 말한다). */
function paintKnowledgeDimension(view) {
  const shown = knowledgeMode === KNOWLEDGE_MODES[0].id ? knowledgeDimensionNow() : "2d";
  const able = knowledgeUniverseAble();
  for (const step of view.querySelectorAll("[data-knowledge-dimension]")) {
    const id = step.dataset.knowledgeDimension;
    const on = id === shown;
    writeAttribute(step, "aria-pressed", String(on));
    step.classList.toggle("is-active", on);
    const row = KNOWLEDGE_DIMENSIONS.find((one) => one.id === id);
    if (id !== "3d" || row === undefined) continue;
    writeAttribute(step, "aria-disabled", String(!able));
    const words = able ? t(row.key, row.word)
      : t("knowledge.dimensionUnable", "이 창에서는 우주를 그릴 수 없어 평면 지도로 봅니다");
    writeAttribute(step, "aria-label", words);
    writeAttribute(step, "data-tip", words);
  }
}

/* 사람이 얼굴을 골랐다. 이 볼트의 기억에 적고(`noteKnowledgeExplore`), 3D면 전체 지도로 나와 평면
 * 자리에서 떠오르고, 2D면 우주가 평면 자세로 내려앉은 뒤 놓인다. */
function setKnowledgeDimension(view, id) {
  if (!KNOWLEDGE_DIMENSIONS.some((row) => row.id === id)) return;
  if (id === "3d" && !knowledgeUniverseAble()) return;
  const layout = knowledgeLayouts.get(view);
  const universe = knowledgeUniverses.get(view);
  knowledgeDimension = id;
  if (layout !== undefined) noteKnowledgeExplore(layout.model.vault);
  if (id === "3d") {
    if (universe?.lost) {
      universe.say(t("knowledge.universeWaiting", "그래픽 장치를 기다리는 중입니다 — 돌아오면 우주로 엽니다"));
      paintKnowledgeDimension(view);
      return;
    }
    knowledgeUniverseRising = true;
    if (knowledgeMode !== KNOWLEDGE_MODES[0].id) {
      setKnowledgeMode(view, KNOWLEDGE_MODES[0].id);
      return;
    }
    void paintKnowledgeView();
    return;
  }
  paintKnowledgeDimension(view);
  if (universe === undefined) return;
  universe.fold(() => leaveKnowledgeUniverse(view));
}

/* 우주를 놓는다 — 판의 옷을 평면으로 되돌리고 GPU의 것을 전부 놓는다. */
function leaveKnowledgeUniverse(view) {
  const universe = knowledgeUniverses.get(view);
  knowledgeUniverses.delete(view);
  view.classList.remove("is-universe");
  universe?.dispose();
  const layout = knowledgeLayouts.get(view);
  if (layout === undefined) return;
  paintKnowledgeUniverseInspector(view, layout);
  paintKnowledgeFrame(view, layout);
}

/* 그림의 문(`paintKnowledgeView`)이 끝에 부른다: 우주가 서야 하면 세우고(처음이면 짓는다), 아니면
 * 놓는다. 세운 우주는 위상이 바뀐 판에서 다시 짓고, 그 밖에는 옷(고름·밝힘·찾기)만 고친다. */
function paintKnowledgeUniverse(view, layout) {
  paintKnowledgeDimension(view);
  const wanted = knowledgeUniverseWanted(view, layout);
  let universe = knowledgeUniverses.get(view);
  if (!wanted) {
    knowledgeUniverseRising = false;
    if (universe !== undefined && !universe.folding) leaveKnowledgeUniverse(view);
    return;
  }
  if (universe === undefined) {
    universe = makeKnowledgeUniverse(view);
    if (!universe.mount()) {
      universe.dispose();
      knowledgeUniverseFailed = true;
      knowledgeUniverseRising = false;
      paintKnowledgeDimension(view);
      return;
    }
    knowledgeUniverses.set(view, universe);
    if (!universe.build(layout)) {
      knowledgeUniverses.delete(view);
      universe.dispose();
      knowledgeUniverseFailed = true;
      knowledgeUniverseRising = false;
      paintKnowledgeDimension(view);
      return;
    }
    universe.enter(knowledgeUniverseRising);
    knowledgeUniverseRising = false;
    paintKnowledgeUniverseInspector(view, layout);
    return;
  }
  if (universe.layout !== layout && !universe.build(layout)) {
    leaveKnowledgeUniverse(view);
    knowledgeUniverseFailed = true;
    paintKnowledgeDimension(view);
    return;
  }
  universe.dress();
  paintKnowledgeUniverseInspector(view, layout);
}

/* 옷이 바뀐 순간(고름·밝힘·찾기·군집) — 평면의 프레임 문(`paintKnowledgeFrame`)이 부른다. 서 있는
 * 우주는 다음 프레임 한 장으로 따라온다. */
function dressKnowledgeUniverse(view) {
  const universe = knowledgeUniverses.get(view);
  if (universe === undefined) return;
  universe.dress();
  const layout = knowledgeLayouts.get(view);
  if (layout !== undefined) paintKnowledgeUniverseInspector(view, layout);
}

/* 우주 안의 배율 단추 셋 — 서 있는 우주가 받으면 참이다(평면의 단추 손은 건너뛴다). */
function knowledgeUniverseZoom(view, what) {
  const universe = knowledgeUniverses.get(view);
  if (universe === undefined || universe.lost || universe.folding) return false;
  if (what === "fit") universe.goHome();
  else universe.zoomStep(what === "in" ? 1 / universe.tune.zoomStep : universe.tune.zoomStep);
  return true;
}

/* 캔버스의 키 — 서 있는 우주는 화살표로 돈다(설계 §4-e). 받으면 참이다. */
function knowledgeUniverseKey(view, event) {
  const universe = knowledgeUniverses.get(view);
  if (universe === undefined || universe.lost || universe.folding) return false;
  const turn = universe.tune.keyTurn;
  const turns = { ArrowLeft: [turn, 0], ArrowRight: [-turn, 0], ArrowUp: [0, -turn], ArrowDown: [0, turn] };
  const way = turns[event.key];
  if (way === undefined) return false;
  event.preventDefault();
  universe.turn(way[0], way[1]);
  return true;
}

/* 고친 때를 「얼마 전」으로(시안 `ageText`) — 말은 로케일의 상대 시각이다. */
function knowledgeUniverseAgo(ms, nowMs) {
  const A = KNOWLEDGE_UNIVERSE_AGE;
  const days = Math.max(0, (nowMs - ms) / 86_400_000);
  const words = new Intl.RelativeTimeFormat(locale, { numeric: "auto" });
  if (days < A.today) return words.format(0, "day");
  if (days < A.days) return words.format(-Math.round(days), "day");
  if (days < A.weeks) return words.format(-Math.round(days / A.week), "week");
  if (days < A.months) return words.format(-Math.round(days / A.month), "month");
  return words.format(-Math.round((days / A.year) * 10) / 10, "year");
}

/* 인스펙터의 우주 절 — 우주가 서 있는 동안만: 가장 굵은 필라멘트(보낸 은하 → 받는 은하) 여섯, 그리고 고른 은하의
 * 형태·쪽 수·최근 두 주에 고친 몫과 그 은하의 필라멘트. 「아름다운 3D, 글자로는 0」이라는 비판에 대한 글자의
 * 답이다(디자이너 m-12467의 6). 줄은 단추이고 누르면 그 흐름으로 난다(`knowledgeUniverseFlow`). */
function paintKnowledgeUniverseInspector(view, layout) {
  const overview = view.querySelector(".knowledge-overview");
  if (overview === null) return;
  const universe = knowledgeUniverses.get(view);
  const map = universe?.map ?? null;
  let flows = overview.querySelector(".knowledge-overview-filaments");
  let galaxy = overview.querySelector(".knowledge-overview-galaxy");
  if (flows === null) {
    flows = knowledgeListSection("knowledge-overview-filaments", "knowledge.filaments",
      t("knowledge.filaments", "가장 굵은 필라멘트 (보낸 은하 → 받는 은하)"));
    galaxy = document.createElement("section");
    galaxy.className = "knowledge-overview-galaxy";
    const head = document.createElement("h4");
    head.className = "knowledge-inspector-head";
    const about = document.createElement("p");
    about.className = "knowledge-overview-galaxy-about";
    /* 은하의 두 목록(시안 은하 카드): 그 은하의 굵은 필라멘트, 그리고 밝은 별 — 「어느 쪽이 중심인가」를 글자로
     * 답하는 줄(디자이너 m-12627). 별 줄을 누르면 그 쪽이 골라진다(인스펙터의 `[data-knowledge-key]` 손). */
    galaxy.append(head, about,
      knowledgeListSection("knowledge-overview-galaxy-flows", "knowledge.galaxyFilaments", t("knowledge.galaxyFilaments", "굵은 필라멘트")),
      knowledgeListSection("knowledge-overview-galaxy-stars", "knowledge.brightStars", t("knowledge.brightStars", "밝은 별")));
    overview.querySelector(".knowledge-overview-counts")?.after(galaxy, flows);
  }
  const rank = knowledgeClusterPicked;
  flows.hidden = map === null || map.filaments.length === 0;
  galaxy.hidden = map === null || rank < 0 || rank >= map.rows;
  if (map === null) return;
  const most = map.filaments[0]?.n ?? 1;
  const row = (one, at) => {
    const line = knowledgeListRow(`flow:${at}`, t("knowledge.flowPair", "{{from}} → {{to}}", {
      from: knowledgeClusterWord(layout, one.from), to: knowledgeClusterWord(layout, one.to) }),
    `${one.sent}/${one.n}`, one.n / most);
    line.querySelector("button").dataset.knowledgeFlow = `${one.from},${one.to}`;
    return line;
  };
  reconcileElementOrder(flows.querySelector(".knowledge-inspector-list"),
    map.filaments.slice(0, KNOWLEDGE_UNIVERSE_LISTED).map(row));
  if (galaxy.hidden) return;
  const picked = map.galaxy[rank];
  const shape = KNOWLEDGE_UNIVERSE_TYPE_WORDS[picked.type];
  writeTextContent(galaxy.querySelector(".knowledge-inspector-head"), t("knowledge.galaxyAbout",
    "{{name}} · {{shape}} · {{pages}}쪽", { name: knowledgeClusterWord(layout, rank), shape: t(shape.key, shape.word),
      pages: layout.communitySize[rank] }));
  writeTextContent(galaxy.querySelector(".knowledge-overview-galaxy-about"), t("knowledge.galaxyRecent",
    "최근 2주에 고친 쪽 {{percent}}", { percent: `${Math.round(picked.activity * 100)}%` }));
  const own = map.filaments.filter((one) => one.from === rank || one.to === rank).slice(0, KNOWLEDGE_UNIVERSE_LISTED);
  const ownFlows = galaxy.querySelector(".knowledge-overview-galaxy-flows");
  ownFlows.hidden = own.length === 0;
  reconcileElementOrder(ownFlows.querySelector(".knowledge-inspector-list"), own.map(row));
  const model = layout.model;
  const brightest = map.members[rank].slice(0, KNOWLEDGE_UNIVERSE_LISTED);
  const brightMost = model.degree[brightest[0]] || 1;
  reconcileElementOrder(galaxy.querySelector(".knowledge-overview-galaxy-stars .knowledge-inspector-list"),
    brightest.map((at) => knowledgeListRow(model.keys[at], model.titles[at], String(model.degree[at]),
      model.degree[at] / brightMost)));
}

/* 필라멘트 줄을 눌렀다(시안 `showFlow`): 보낸 은하를 고르고 그 흐름을 옆에서 보는 자리로 난다. */
function knowledgeUniverseFlow(view, from, to) {
  const universe = knowledgeUniverses.get(view);
  if (universe === undefined || universe.map === null) return;
  if (knowledgeClusterPicked !== from) toggleKnowledgeCluster(view, from);
  universe.flyToFlow(from, to);
}

/* 「보기」 메뉴의 우주 줄 — 우주가 서 있는 동안만 보인다(CSS `.knowledge-view.is-universe`). 「움직임」 하나. */
function buildKnowledgeUniverseControls() {
  const box = document.createElement("div");
  box.className = "knowledge-universe-controls";
  box.setAttribute("role", "group");
  box.dataset.i18nAria = "knowledge.universeControls";
  box.setAttribute("aria-label", t("knowledge.universeControls", "우주"));
  const motion = document.createElement("button");
  motion.type = "button";
  motion.className = "btn knowledge-flag knowledge-universe-motion";
  motion.setAttribute("aria-pressed", String(knowledgeUniverseMotion));
  motion.dataset.i18n = "knowledge.universeMotion";
  motion.textContent = t("knowledge.universeMotion", "움직임");
  motion.dataset.i18nTitle = "knowledge.universeMotionTip";
  motion.dataset.tip = t("knowledge.universeMotionTip", "쉬는 동안 천천히 돌고 필라멘트가 흐릅니다");
  motion.onclick = () => {
    const view = motion.closest(".knowledge-view");
    if (view !== null) setKnowledgeUniverseMotion(view, !knowledgeUniverseMotion);
  };
  box.appendChild(motion);
  return box;
}

function setKnowledgeUniverseMotion(view, on) {
  knowledgeUniverseMotion = on;
  const button = view.querySelector(".knowledge-universe-motion");
  if (button !== null) writeAttribute(button, "aria-pressed", String(on));
  knowledgeUniverses.get(view)?.setMotion(on);
}

/* 필라멘트 위의 한 점(시안 `filamentPoint`) — 장식 셰이더가 그리는 곡선과 같은 식이라 필라멘트 수가 그 빛 위에
 * 선다. `share`는 0(보낸 은하) → 1(받는 은하). */
function knowledgeUniverseFilamentPoint(one, two, bend, share) {
  const C = KNOWLEDGE_UNIVERSE_CURVE;
  const dx = two.x - one.x;
  const dy = two.y - one.y;
  const dz = two.z - one.z;
  const length = Math.max(Math.hypot(dx, dy, dz), 1);
  const ux = dx / length;
  const uy = dy / length;
  const uz = dz / length;
  let sx = -uz;
  let sz = ux;
  const across = Math.hypot(sx, sz);
  if (across < C.upright) {
    sx = 1;
    sz = 0;
  } else {
    sx /= across;
    sz /= across;
  }
  const qx = -sz * uy;
  const qy = sz * ux - sx * uz;
  const qz = sx * uy;
  const start = Math.min(C.startMost, (one.radius * C.reachShare) / length);
  const end = Math.max(C.endLeast, 1 - (two.radius * C.reachShare) / length);
  const along = start + (end - start) * share;
  const arc = Math.sin(Math.PI * along);
  const side = bend * length * C.side * arc;
  const up = length * C.up * arc;
  return [one.x + dx * along + sx * side + qx * up, one.y + dy * along + qy * up, one.z + dz * along + sz * side + qz * up];
}

/* 은하의 가장 굵은 필라멘트(시안 `strongestFlow`) — 명판의 둘째 줄: 이 은하가 더 많이 보냈으면 →, 받았으면 ←. */
function knowledgeUniverseStrongest(map, row) {
  let best = null;
  for (const one of map.filaments) {
    if (one.a !== row && one.b !== row) continue;
    const sent = one.a === row ? one.ab : one.ba;
    const taken = one.a === row ? one.ba : one.ab;
    if (best === null || one.n > best.n) best = { other: one.a === row ? one.b : one.a, n: one.n, out: sent >= taken };
  }
  return best;
}

/* 범례의 우주 네 줄(시안 index.html 157–160, 디자이너 m-12627) — 우주가 서 있는 동안만 보이고 평면의 줄은 가려진다
 * (CSS `.knowledge-view.is-universe`). 표본은 시안의 기호: 별빛, 고친 때의 빛깔, 필라멘트, 올린 쪽의 선. */
const KNOWLEDGE_UNIVERSE_LEGEND = Object.freeze([
  Object.freeze({ marks: ["star"], words: [{ key: "knowledge.legendBrightness", word: "밝기 = 연결 수" }] }),
  Object.freeze({ marks: ["temp"], words: [{ key: "knowledge.legendRecent", word: "푸를수록 최근에 고친 쪽" }] }),
  Object.freeze({ marks: ["filament"],
    words: [{ key: "knowledge.legendFilament", word: "필라멘트 = 주제 사이 연결 · 빛은 보낸 쪽 → 받는 쪽" }] }),
  Object.freeze({ marks: ["solid", "dots"], words: [{ key: "knowledge.legendPointed", word: "올린 쪽의 연결" },
    { key: "knowledge.legendInferred", word: "추론" }] }),
]);

function appendKnowledgeUniverseLegend(list) {
  for (const row of KNOWLEDGE_UNIVERSE_LEGEND) {
    const item = document.createElement("li");
    item.dataset.legendUniverse = "true";
    row.marks.forEach((kind, at) => {
      const mark = document.createElement("span");
      mark.className = `knowledge-legend-universe kind-${kind}`;
      mark.setAttribute("aria-hidden", "true");
      const said = document.createElement("span");
      said.dataset.i18n = row.words[at].key;
      said.textContent = t(row.words[at].key, row.words[at].word);
      item.append(mark, said);
    });
    list.appendChild(item);
  }
}

/* ---- 셰이더 — 시안의 글자 그대로 -----------------------------------------------------
 *
 * 글은 index.html(09-28 18:38)의 것을 한 자도 바꾸지 않고 옮겼다. 재질은 시안처럼 `ShaderMaterial`이다 —
 * three.js가 WebGL2에서 GLSL 300 es로 옮겨 싣는다(`attribute`·`varying`·`gl_FragColor`를 제 머리의 정의로).
 * 바뀐 것은 디자이너가 정한 자리뿐이다(PORTING.md §4, 우편 m-12467): 은하 줄 128, 장식 무리의 문턱(배경 별
 * 128·필라멘트 130부터), 초점 센티널 999 — 그 수는 `KNOWLEDGE_UNIVERSE_SHAPE` 한 곳에서 글자에 들어간다.
 * 필라멘트 곡선의 수(`KNOWLEDGE_UNIVERSE_CURVE`)와 별 크기의 수(`KNOWLEDGE_UNIVERSE_STAR`)도 같은 값을 표에서
 * 읽는다 — 이름표 자리와 고르기가 같은 표를 읽기 때문이다. */

/* 은하 텍셀 읽기·정수 해시·별 색(Wikipedia 분광형 표의 D65 색을 선형으로)·나선 팔 밭 — 시안 `GAL`. */
const KNOWLEDGE_UNIVERSE_GAL = `
uniform highp sampler2D tGal;
vec4 gal(int c, int t){ return texelFetch(tGal, ivec2(t, c), 0); }
uint hu(uint x){ x ^= x >> 16u; x *= 0x7feb352du; x ^= x >> 15u; x *= 0x846ca68bu; x ^= x >> 16u; return x; }
float hf(uint s){ return float(hu(s) >> 8u) * (1.0 / 16777216.0); }
float gauss(float a, float b){ return sqrt(-2.0 * log(max(a, 1e-6))) * cos(6.2831853 * b); }
vec3 starColor(float t){
  t = clamp(t, 0.0, 1.0) * 6.0;
  vec3 M = vec3(1.0, 0.493, 0.167), K = vec3(1.0, 0.731, 0.499), G = vec3(1.0, 0.867, 0.782), F = vec3(0.974, 0.933, 1.0);
  vec3 A = vec3(0.701, 0.759, 1.0), B = vec3(0.416, 0.542, 1.0), O = vec3(0.34, 0.477, 1.0);
  if (t < 1.0) return mix(M, K, t);
  if (t < 2.0) return mix(K, G, t - 1.0);
  if (t < 3.0) return mix(G, F, t - 2.0);
  if (t < 4.0) return mix(F, A, t - 3.0);
  if (t < 5.0) return mix(A, B, t - 4.0);
  return mix(B, O, t - 5.0);
}
float armField(float r, float th, float arms, float K, float th0){
  float lg = log(max(r, 0.1) / 0.1);
  return pow(0.5 + 0.5 * cos(arms * (th - th0 - K * lg)), 4.0) * smoothstep(0.08, 0.26, r);
}`;

/* 값 노이즈 fbm 4옥타브 — 시안 `NOISE`(성운과 불규칙 은하의 무늬). */
const KNOWLEDGE_UNIVERSE_NOISE = `
float h3(vec3 p){ return fract(sin(dot(p, vec3(127.1, 311.7, 74.7))) * 43758.5453); }
float vnoise(vec3 p){ vec3 i = floor(p), f = fract(p); f = f * f * (3.0 - 2.0 * f);
  float a = h3(i), b = h3(i + vec3(1.0, 0.0, 0.0)), c = h3(i + vec3(0.0, 1.0, 0.0)), d = h3(i + vec3(1.0, 1.0, 0.0));
  float e = h3(i + vec3(0.0, 0.0, 1.0)), f1 = h3(i + vec3(1.0, 0.0, 1.0)), g = h3(i + vec3(0.0, 1.0, 1.0)), h = h3(i + vec3(1.0, 1.0, 1.0));
  return mix(mix(mix(a, b, f.x), mix(c, d, f.x), f.y), mix(mix(e, f1, f.x), mix(g, h, f.x), f.y), f.z); }
float fbm(vec3 p){ float a = 0.5, s = 0.0; for (int i = 0; i < 4; i++) { s += a * vnoise(p); p = p * 2.03 + 1.7; a *= 0.5; } return s; }`;

/* 은하의 원반(기울어진 사각형 하나에 풀리지 않은 별빛·팔·먼지 띠·막대)과 핵의 빛, 성단의 반사 성운 — 시안
 * `materials.glows`. */
const KNOWLEDGE_UNIVERSE_GLOWS_VERT = `${KNOWLEDGE_UNIVERSE_GAL}
      attribute vec4 aBB; uniform float uLift; uniform float uFocusGal;
      varying vec2 vQ; varying float vType; varying vec4 vP1; varying vec4 vP2; varying vec3 vTint; varying vec3 vW; varying vec3 vN; varying float vVar; varying float vFoc; varying float vNear;
      void main(){
        int c = int(aBB.x + 0.5);
        vec4 G0 = gal(c, 0), G1 = gal(c, 1), G2 = gal(c, 2), G3 = gal(c, 3), G4 = gal(c, 4), G5 = gal(c, 5), G6 = gal(c, 6);
        vec3 center = mix(G4.xyz, G0.xyz, uLift);
        float R = G0.w * aBB.z;
        vNear = mix(0.3, 1.0, smoothstep(1.6, 5.5, length(cameraPosition - center) / max(G0.w, 1.0)));
        vec3 w;
        if (aBB.y < 0.5) { vec3 N = G1.xyz, Ux = G2.xyz, V = cross(N, Ux); w = center + (Ux * position.x + V * position.y) * R * uLift; vN = N; }
        else {
          vec3 right = vec3(viewMatrix[0][0], viewMatrix[1][0], viewMatrix[2][0]); vec3 up = vec3(viewMatrix[0][1], viewMatrix[1][1], viewMatrix[2][1]);
          w = center + (right * position.x + up * position.y) * R * uLift; vN = vec3(0.0);
        }
        vQ = position.xy * aBB.z; vType = aBB.y; vVar = aBB.w;
        vP1 = vec4(G2.w, G3.x, G6.y, G3.y); vP2 = vec4(G3.z, G3.w, G5.w, G1.w);
        vTint = G5.rgb; vW = w;
        vFoc = uFocusGal < -0.5 ? 1.0 : (abs(float(c) - uFocusGal) < 0.5 ? 1.12 : 0.3);
        gl_Position = projectionMatrix * viewMatrix * vec4(w, 1.0);
      }`;

const KNOWLEDGE_UNIVERSE_GLOWS_FRAG = `${KNOWLEDGE_UNIVERSE_NOISE}
      uniform float uLift; uniform float uTime;
      varying vec2 vQ; varying float vType; varying vec4 vP1; varying vec4 vP2; varying vec3 vTint; varying vec3 vW; varying vec3 vN; varying float vVar; varying float vFoc; varying float vNear;
      void main(){
        vec2 q = vQ; float r = length(q);
        vec3 o = vec3(0.0); float alpha = 0.0;
        if (vType < 0.5) {
          float arms = vP1.x, K = vP1.y, th0 = vP1.z, bar = vP1.w, act = vP2.y, dust = vP2.z, gtype = vP2.w;
          float th = atan(q.y, q.x);
          float lg = log(max(r, 0.1) / 0.1);
          float ph = arms * (th - th0 - K * lg);
          float edge = 1.0 - smoothstep(0.7, 1.12, r);
          float disk = exp(-r / 0.3) * edge;
          float armR = exp(-r / 0.55) * edge;
          float arm = pow(0.5 + 0.5 * cos(ph), 3.0) * smoothstep(0.06, 0.22, r);
          float lane = pow(0.5 + 0.5 * cos(ph + 0.85), 12.0) * smoothstep(0.12, 0.3, r) * (1.0 - smoothstep(0.6, 0.95, r)) * dust;
          float barL = 0.0;
          if (gtype > 0.5 && gtype < 1.5) { float cs = cos(th0), sn = sin(th0); vec2 b = vec2(cs * q.x + sn * q.y, -sn * q.x + cs * q.y); barL = exp(-pow(abs(b.x) / max(bar, 0.01), 3.0) - pow(b.y / 0.05, 2.0)); }
          if (gtype > 2.5) { arm = smoothstep(0.35, 0.8, fbm(vec3(q * 3.2, th0))); lane = 0.0; }
          vec3 warm = vec3(1.0, 0.86, 0.7), cool = vec3(0.6, 0.73, 1.0);
          vec3 light = warm * (disk * 0.22 + barL * 0.6) + mix(warm, cool, 0.75) * armR * arm * (0.5 + 0.8 * act);
          light += vec3(1.0, 0.26, 0.4) * armR * arm * arm * act * 0.4;
          light *= 1.0 - lane * 0.85;
          float facing = abs(dot(normalize(cameraPosition - vW), vN));
          o = light * vTint * 0.9 / max(facing, 0.35);
          alpha = min(0.55, lane * disk * 2.6);
        } else if (vType < 1.5) {
          float gtype = vP2.w;
          float I = gtype > 1.5 && gtype < 2.5 ? exp(-pow(r / 0.2, 0.75) * 2.3) * 1.25 : exp(-pow(r / 0.06, 0.8) * 2.0) * 1.9;
          I *= 1.0 - smoothstep(0.75, 1.0, r / max(vVar, 0.5));
          o = vec3(1.0, 0.8, 0.6) * vTint * I;
        } else {
          float n = fbm(vec3(q * (1.5 + vVar * 0.6) + vVar * 7.3, vVar * 3.1 + uTime * 0.004));
          float m = smoothstep(0.38, 0.95, n) * exp(-r * r * 2.4);
          vec3 cc = mix(vec3(0.3, 0.48, 1.0), vec3(1.0, 0.32, 0.5), step(1.5, vVar) * min(1.0, vP2.y * 3.0));
          o = cc * m * 0.1;
        }
        o *= vFoc * uLift * uLift * (vType < 1.5 ? vNear : 1.0);
        alpha *= vFoc * uLift;
        gl_FragColor = vec4(o, alpha);
      }`;

/* 이름 없는 별·먼지·H II 매듭·필라멘트의 가스·무한히 먼 배경 별 — 한 드로우, 번호에서 셰이더가 짓는다(입자마다
 * float 하나) — 시안 `materials.decor`. 무리의 문턱과 필라멘트 배열의 크기만 `KNOWLEDGE_UNIVERSE_SHAPE`에서 든다. */
const KNOWLEDGE_UNIVERSE_DECOR_VERT = `${KNOWLEDGE_UNIVERSE_GAL}
      uniform float uTime; uniform float uLift; uniform float uPx; uniform float uDpr; uniform float uRef; uniform float uFocus; uniform float uFlow; uniform float uFocusGal;
      uniform vec4 uBundle[${KNOWLEDGE_UNIVERSE_SHAPE.bundles}]; uniform vec4 uBundle2[${KNOWLEDGE_UNIVERSE_SHAPE.bundles}];
      varying vec3 vCol; varying float vA; varying float vKind;
      void main(){
        float code = position.x;
        float grp = floor(code / ${KNOWLEDGE_UNIVERSE_SHAPE.groupSpan}.0 + 1e-4);
        float j = code - grp * ${KNOWLEDGE_UNIVERSE_SHAPE.groupSpan}.0;
        uint s = hu(uint(code + 0.5) * 747796405u + 2891336453u);
        float r0 = hf(s), r1 = hf(s + 11u), r2 = hf(s + 23u), r3 = hf(s + 37u), r4 = hf(s + 41u), r5 = hf(s + 53u), r6 = hf(s + 67u);
        vec3 world = vec3(0.0); vec3 col = vec3(1.0); float inten = 0.0; float px = 1.6; float ext = 0.0; float dustA = 0.0; float kind = 0.0; float extR = 1.0; float extMax = 64.0;
        vA = 0.0; vKind = 0.0;
        float lift2 = uLift * uLift;
        if (grp > ${KNOWLEDGE_UNIVERSE_SHAPE.bundleGroup - 0.5}) {
          int b = int(grp - ${KNOWLEDGE_UNIVERSE_SHAPE.bundleGroup}.0 + 0.5);
          vec4 B0 = uBundle[b], B1 = uBundle2[b];
          int ga = int(B0.x + 0.5), gb = int(B0.y + 0.5);
          vec4 CA = gal(ga, 0), CB = gal(gb, 0);
          vec3 d = CB.xyz - CA.xyz; float L = max(length(d), 1.0); vec3 dir = d / L;
          vec3 side = cross(dir, vec3(0.0, 1.0, 0.0)); side = length(side) < ${knowledgeUniverseFloat(KNOWLEDGE_UNIVERSE_CURVE.upright)} ? vec3(1.0, 0.0, 0.0) : normalize(side);
          vec3 up2 = cross(side, dir);
          float t = fract(r0 + uTime * (4.0 + 4.0 * r1) / L);
          float ta = min(${knowledgeUniverseFloat(KNOWLEDGE_UNIVERSE_CURVE.startMost)}, CA.w * ${knowledgeUniverseFloat(KNOWLEDGE_UNIVERSE_CURVE.reachShare)} / L), tb = max(${knowledgeUniverseFloat(KNOWLEDGE_UNIVERSE_CURVE.endLeast)}, 1.0 - CB.w * ${knowledgeUniverseFloat(KNOWLEDGE_UNIVERSE_CURVE.reachShare)} / L);
          float tt = mix(ta, tb, t);
          float arc = sin(3.14159265 * tt);
          vec3 P = CA.xyz + d * tt + side * (B1.x * L * ${knowledgeUniverseFloat(KNOWLEDGE_UNIVERSE_CURVE.side)} * arc) + up2 * (L * ${knowledgeUniverseFloat(KNOWLEDGE_UNIVERSE_CURVE.up)} * arc);
          float strong = B0.z * B0.z;
          float w = (0.6 + 6.0 * pow(B0.z, 1.5)) * (0.45 + 0.55 * arc) * (0.65 + 0.35 * sin(tt * (9.0 + 5.0 * B1.y) + B1.y * 6.0));
          float gr = sqrt(-2.0 * log(max(r2, 1e-4))) * 0.5, ang = 6.2831853 * r3;
          world = P + (side * cos(ang) + up2 * sin(ang)) * gr * w;
          float fade = smoothstep(0.0, 0.1, t) * smoothstep(1.0, 0.86, t);
          inten = (0.035 + 0.22 * strong) * fade;
          col = mix(gal(ga, 5).rgb, gal(gb, 5).rgb, tt) * vec3(0.68, 0.78, 1.0);
          if (r4 < 0.05) { inten *= 5.0; px = 2.6; col = mix(col, vec3(1.0), 0.45); }
          else if (r4 < 0.3) { kind = 1.0; ext = 0.8 + 3.0 * B0.z; extMax = 20.0; inten = (0.01 + 0.05 * strong) * fade; col = mix(col, vec3(0.45, 0.58, 1.0), 0.35); }
          float onB = (abs(B0.x - uFocusGal) < 0.5 || abs(B0.y - uFocusGal) < 0.5) ? 1.0 : 0.0;
          inten *= uFocusGal < -0.5 ? 1.0 : mix(0.15, 1.9, onB);
          inten *= lift2;
        } else if (grp > ${KNOWLEDGE_UNIVERSE_SHAPE.skyGroup - 0.5}) {
          float z = r0 * 2.0 - 1.0, ph = 6.2831853 * r1, rr = sqrt(max(0.0, 1.0 - z * z));
          vec3 dir = vec3(rr * cos(ph), z, rr * sin(ph));
          vec4 clip = projectionMatrix * vec4(mat3(viewMatrix) * dir, 0.0);
          if (clip.w <= 0.0) { gl_Position = vec4(2.0, 2.0, 2.0, 1.0); gl_PointSize = 0.0; vCol = vec3(0.0); return; }
          clip.z = clip.w * 0.9999;
          gl_Position = clip;
          float m = pow(r2, 9.0);
          float I = (0.045 + 0.07 * r3 + 2.4 * m) * lift2 * (uFocus > 0.5 ? 0.55 : 1.0);
          if (j < 48.0) { vKind = 3.0; vCol = vec3(1.0, 0.88, 0.76) * (0.05 + 0.05 * r3) * lift2; vA = r5 * 3.1416; gl_PointSize = (4.0 + 6.0 * r4) * uDpr; return; }
          vCol = starColor(0.08 + 0.84 * r4) * I;
          gl_PointSize = (1.15 + 2.2 * sqrt(m)) * uDpr;
          return;
        } else {
          int c = int(grp + 0.5);
          vec4 G0 = gal(c, 0), G1 = gal(c, 1), G2 = gal(c, 2), G3 = gal(c, 3), G4 = gal(c, 4), G5 = gal(c, 5), G6 = gal(c, 6);
          float R = G0.w; int type = int(G1.w + 0.5);
          vec3 N = G1.xyz, Ux = G2.xyz, V = cross(N, Ux);
          float arms = G2.w, K = G3.x, bar = G3.y, bulge = G3.z, act = G3.w, dust = G5.w, count = G6.x, th0 = G6.y;
          float f = (j + 0.5) / max(count, 1.0);
          vec3 l = vec3(0.0);
          float spin = uTime;
          extR = R;
          if (type <= 1) {
            float fDust = 0.14 * dust / 0.6, fBulge = fDust + bulge, fYoung = fBulge + 0.2 + 0.12 * act, fHII = fYoung + 0.025 + 0.07 * act, fHalo = fHII + 0.03;
            if (f < fDust) {
              float r = clamp(0.1 + 0.34 * -log(1.0 - r1 * 0.93), 0.1, 0.95);
              float a = th0 + floor(r2 * arms) * 6.2831853 / arms + K * log(max(r, 0.1) / 0.1) - 0.26 + gauss(r3, r4) * 0.06;
              l = vec3(cos(a) * r, sin(a) * r, gauss(r5, r6) * 0.012);
              kind = 2.0; dustA = 0.3 * (1.0 - smoothstep(0.6, 0.95, r)); ext = 0.07; col = vec3(0.0);
            } else if (f < fBulge) {
              float r = 0.2 * pow(r1, 1.8);
              float z = r2 * 2.0 - 1.0, ph = 6.2831853 * r3 + spin * 0.05 / (0.05 + r), q = sqrt(max(0.0, 1.0 - z * z));
              l = vec3(q * cos(ph) * r, q * sin(ph) * r, z * r * 0.62);
              col = starColor(0.12 + 0.2 * r4); inten = 0.5 + 0.9 * r5 * r5; px = 1.6;
            } else if (f < fYoung) {
              float r = clamp(0.12 + 0.36 * -log(1.0 - r1 * 0.9), 0.12, 1.0);
              float a;
              if (type == 1 && r < bar) a = th0 + floor(r2 * 2.0) * 3.14159265 + gauss(r3, r4) * 0.06;
              else a = th0 + floor(r2 * arms) * 6.2831853 / arms + K * log(max(r, 0.1) / 0.1) + gauss(r3, r4) * 0.11;
              l = vec3(cos(a) * r, sin(a) * r, gauss(r5, r6) * 0.01);
              col = starColor(0.62 + 0.36 * r5); inten = (0.32 + 0.9 * pow(r6, 3.0)) * (0.7 + 0.8 * act); px = 1.5 + r6;
            } else if (f < fHII) {
              uint ks = hu(uint(c) * 9781u + uint(floor(j / 7.0)) * 6271u);
              float kr = clamp(0.18 + 0.3 * -log(1.0 - hf(ks) * 0.9), 0.15, 0.92);
              float ka = th0 + floor(hf(ks + 3u) * arms) * 6.2831853 / arms + K * log(kr / 0.1) + (hf(ks + 5u) - 0.5) * 0.12;
              l = vec3(cos(ka) * kr, sin(ka) * kr, 0.0) + vec3(gauss(r1, r2), gauss(r3, r4), gauss(r5, r6) * 0.3) * 0.018;
              kind = 1.0; col = vec3(1.0, 0.28, 0.42); inten = 0.16 + 0.2 * act; ext = 0.028;
            } else if (f < fHalo) {
              float r = 0.3 + 0.9 * r1;
              float z = r2 * 2.0 - 1.0, ph = 6.2831853 * r3, q = sqrt(max(0.0, 1.0 - z * z));
              l = vec3(q * cos(ph), q * sin(ph), z * 0.7) * r;
              col = starColor(0.08 + 0.15 * r4); inten = 0.1 + 0.12 * r5;
            } else {
              float r = clamp(0.03 + 0.3 * -log(1.0 - r1 * 0.96), 0.03, 1.05);
              float a = 6.2831853 * r2 + spin * 0.035 / (0.08 + r);
              if (type == 1 && r < bar && r3 < 0.35) a = th0 + floor(r4 * 2.0) * 3.14159265 + gauss(r5, r6) * 0.07;
              l = vec3(cos(a) * r, sin(a) * r, gauss(r5, r6) * 0.02 * (1.0 - 0.5 * r));
              float arm = armField(r, a, arms, K, th0);
              col = starColor(0.2 + 0.3 * r4 + 0.25 * arm); inten = (0.13 + 0.3 * r5) * (0.35 + 1.6 * arm); px = 1.3 + 0.6 * r6;
            }
          } else if (type == 2) {
            float r = 0.85 * pow(r1, 1.7);
            float z = r2 * 2.0 - 1.0, ph = 6.2831853 * r3 + spin * 0.02 / (0.1 + r), q = sqrt(max(0.0, 1.0 - z * z));
            l = vec3(q * cos(ph) * r, q * sin(ph) * r * 0.78, z * r * 0.6);
            col = starColor(0.1 + 0.22 * r4); inten = (0.3 + 0.55 * r5 * r5) * (1.0 - 0.6 * r); px = 1.4 + 0.5 * r6;
          } else if (type == 3) {
            float clump = floor(r1 * 4.0);
            float ca = th0 + clump * 1.9, cr = 0.12 + 0.2 * clump;
            l = vec3(cos(ca) * cr, sin(ca) * cr, 0.0) + vec3(gauss(r2, r3), gauss(r4, r5), gauss(r6, r1) * 0.35) * 0.2;
            bool hii = r6 < 0.1;
            col = hii ? vec3(1.0, 0.28, 0.42) : starColor(0.45 + 0.5 * r4);
            inten = hii ? 0.2 : 0.25 + 0.6 * r5 * r5; kind = hii ? 1.0 : 0.0; ext = hii ? 0.05 : 0.0;
          } else if (type == 4) {
            float r = 0.9 * pow(r1, 1.4);
            float z = r2 * 2.0 - 1.0, ph = 6.2831853 * r3, q = sqrt(max(0.0, 1.0 - z * z));
            l = vec3(q * cos(ph), q * sin(ph), z) * r;
            col = starColor(0.55 + 0.45 * r4); inten = 0.25 + 1.1 * pow(r5, 4.0); px = 1.4 + 0.8 * r6;
          }
          world = mix(G4.xyz, G0.xyz + (Ux * l.x + V * l.y + N * l.z) * R, uLift);
          col *= G5.rgb;
          float foc = uFocusGal < -0.5 ? 1.0 : (abs(float(c) - uFocusGal) < 0.5 ? 1.15 : 0.3);
          inten *= foc * lift2;
          dustA *= foc * uLift;
        }
        vec4 clip = projectionMatrix * viewMatrix * vec4(world, 1.0);
        gl_Position = clip;
        float dist = max(clip.w, 1.0);
        if (ext > 0.0) {
          float sz = ext * extR * uPx / dist, sc = clamp(sz, 1.5 * uDpr, extMax * uDpr), keep = min(1.0, (sz * sz) / (sc * sc)) * min(1.0, sc / sz + 0.35);
          gl_PointSize = sc; vCol = col * inten * keep; vA = dustA * keep;
        } else {
          gl_PointSize = px * uDpr;
          vCol = col * inten * clamp(pow(uRef / dist, 2.0), 0.1, 4.0);
          vA = 0.0;
        }
        vKind = kind;
      }`;

const KNOWLEDGE_UNIVERSE_DECOR_FRAG = `varying vec3 vCol; varying float vA; varying float vKind;
      void main(){
        vec2 p = gl_PointCoord - 0.5;
        float r2 = dot(p, p) * 4.0;
        float inside = step(r2, 1.0);
        vec4 o;
        if (vKind < 0.5) o = vec4(vCol * exp(-r2 * 4.5), 0.0);
        else if (vKind < 1.5) o = vec4(vCol * exp(-r2 * 3.5) * (1.0 - r2), 0.0);
        else if (vKind < 2.5) { float s = max(0.0, 1.0 - r2); o = vec4(0.0, 0.0, 0.0, vA * s * s); }
        else { float ca = cos(vA), sa = sin(vA); vec2 q = mat2(ca, -sa, sa, ca) * p; o = vec4(vCol * exp(-(q.x * q.x * 4.0 + q.y * q.y * 26.0) * 4.0), 0.0); }
        gl_FragColor = o * inside;
      }`;

/* 이름 있는 별(쪽): 밝기 = 연결 수(등급 척도), 색 = 최근 고침(흑체 색), 밝은 별은 빛살, 찾은 별은
 * 조준 고리 — 시안 `materials.stars`. */
const KNOWLEDGE_UNIVERSE_STARS_VERT = `${KNOWLEDGE_UNIVERSE_GAL}
      attribute vec3 aPos2; attribute vec4 aStar; attribute float aState;
      uniform float uLift; uniform float uDpr; uniform float uRef; uniform float uFocus;
      varying vec3 vCol; varying float vI; varying float vS; varying float vState; varying float vSpike;
      void main(){
        vec3 w = mix(aPos2, position, uLift);
        vec4 clip = projectionMatrix * viewMatrix * vec4(w, 1.0);
        gl_Position = clip;
        float st = aState;
        float flux = aStar.x * clamp(pow(uRef / max(clip.w, 1.0), 2.0), ${knowledgeUniverseFloat(KNOWLEDGE_UNIVERSE_STAR.fluxLeast)}, ${knowledgeUniverseFloat(KNOWLEDGE_UNIVERSE_STAR.fluxMost)});
        if (st > 1.5 && st < 2.5) flux = max(flux * 2.0, 4.0);
        float size = clamp(${knowledgeUniverseFloat(KNOWLEDGE_UNIVERSE_STAR.sizeScale)} * pow(flux, ${knowledgeUniverseFloat(KNOWLEDGE_UNIVERSE_STAR.sizePower)}), ${knowledgeUniverseFloat(KNOWLEDGE_UNIVERSE_STAR.sizeLeast)}, ${knowledgeUniverseFloat(KNOWLEDGE_UNIVERSE_STAR.sizeMost)});
        if (st > 2.5) size = max(size, 24.0);
        gl_PointSize = size * uDpr;
        vS = size * uDpr; vState = st;
        float focus = uFocus > 0.5 ? (st > 0.5 ? 1.3 : 0.14) : 1.0;
        vI = min(flux, 6.0) * 0.55 * focus;
        vSpike = smoothstep(3.0, 9.0, flux) + ((st > 1.5 && st < 2.5) ? 0.8 : 0.0);
        vec3 c = starColor(aStar.y);
        if (uFocus > 0.5 && st < 0.5) c = mix(c, vec3(dot(c, vec3(0.3, 0.59, 0.11))), 0.7);
        vCol = c;
      }`;

const KNOWLEDGE_UNIVERSE_STARS_FRAG = `uniform float uTime; uniform float uDpr;
      varying vec3 vCol; varying float vI; varying float vS; varying float vState; varying float vSpike;
      void main(){
        vec2 p = (gl_PointCoord - 0.5) * vS;
        float r = length(p);
        float px = uDpr;
        float core = exp(-r * r / (2.2 * px * px));
        float halo = 1.0 / (1.0 + r * r / ((0.1 * vS + px) * (0.1 * vS + px)) * 2.5) * (1.0 - smoothstep(0.32 * vS, 0.5 * vS, r));
        vec2 q = mat2(0.97, 0.24, -0.24, 0.97) * p;
        float len = 0.2 * vS;
        float sp = (exp(-abs(q.y) / (0.55 * px)) * exp(-abs(q.x) / len) + exp(-abs(q.x) / (0.55 * px)) * exp(-abs(q.y) / len)) * vSpike;
        float ring = vState > 2.5 ? exp(-pow((r - 0.38 * vS) / (1.2 * px), 2.0)) * (0.65 + 0.35 * sin(uTime * 4.0)) : 0.0;
        vec3 o = (mix(vCol, vec3(1.0), 0.22) * core * 1.6 + vCol * (halo * 0.5 + sp * 0.55)) * vI + vec3(0.6, 0.82, 1.0) * ring * 1.1;
        gl_FragColor = vec4(o, 0.0);
      }`;

/* 쪽의 자리 — 번호에서 텍셀 하나(평면과 우주 사이를 `uLift`로) — 시안 `NODE_AT`. */
const KNOWLEDGE_UNIVERSE_NODE_AT = `
uniform highp sampler2D tNode3; uniform highp sampler2D tNode2; uniform int uTexW; uniform float uLift;
vec4 nodeAt(int i){ ivec2 c = ivec2(i % uTexW, i / uTexW); vec4 p3 = texelFetch(tNode3, c, 0); if (uLift > 0.999) return p3; return mix(texelFetch(tNode2, c, 0), p3, uLift); }`;

/* 별자리 선: 올리거나 고른 쪽의 연결만(주제를 고르면 선 대신 그 은하의 필라멘트가 밝아진다). 빛은 보낸 쪽에서
 * 받는 쪽으로 흐른다 — 시안 `materials.lines`. 두 끝의 줄 번호를 푸는 밑만 `KNOWLEDGE_UNIVERSE_SHAPE`에서 든다. */
const KNOWLEDGE_UNIVERSE_LINES_VERT = `${KNOWLEDGE_UNIVERSE_NODE_AT}
      attribute vec4 aEdge;
      varying float vT; varying float vKind; varying float vPhase; varying float vCross;
      void main(){
        float t = position.x;
        int ia = int(aEdge.x + 0.5); int ib = int(aEdge.y + 0.5);
        float packed = floor(aEdge.z * 0.5 + 0.01); float ca = mod(packed, ${KNOWLEDGE_UNIVERSE_SHAPE.linePack}.0), cb = floor(packed / ${KNOWLEDGE_UNIVERSE_SHAPE.linePack}.0 + 0.001);
        vec3 A = nodeAt(ia).xyz, B = nodeAt(ib).xyz;
        vec3 p = mix(A, B, t);
        float cross = abs(ca - cb) > 0.5 ? 1.0 : 0.0;
        p.y += sin(3.14159265 * t) * length(B - A) * 0.1 * cross * uLift;
        gl_Position = projectionMatrix * viewMatrix * vec4(p, 1.0);
        vT = t; vKind = mod(aEdge.z, 2.0); vPhase = aEdge.w; vCross = cross;
      }`;

const KNOWLEDGE_UNIVERSE_LINES_FRAG = `uniform float uTime; uniform float uFlow; uniform float uDark;
      varying float vT; varying float vKind; varying float vPhase; varying float vCross;
      void main(){
        float a = 0.2 + 0.08 * vCross;
        if (vKind > 0.5) a *= step(fract(vT * 60.0), 0.5) * 1.4;
        float head = fract(uTime * 0.14 + vPhase);
        float behind = fract(head - vT);
        float glow = behind < 0.22 ? pow(1.0 - behind / 0.22, 2.0) : 0.0;
        vec3 c = vec3(0.62, 0.78, 1.0) * (a + glow * 0.95 * uFlow);
        gl_FragColor = vec4(c, 0.0);
      }`;

/* 후처리: 빛 번짐 사다리(Jimenez 2014 — 13탭 내리기 + 3×3 텐트 올리기, 첫 단은 Karis 평균) → 톤 매핑 —
 * 시안 `FS_VERT`·`down`·`up`·`comp`. */
const KNOWLEDGE_UNIVERSE_POST_VERT = `varying vec2 vUv; void main(){ vUv = position.xy * 0.5 + 0.5; gl_Position = vec4(position.xy, 0.0, 1.0); }`;

const KNOWLEDGE_UNIVERSE_DOWN_FRAG = `uniform sampler2D tSrc; uniform vec2 uTexel; uniform float uKaris; varying vec2 vUv;
    vec3 s(float x, float y){ return texture2D(tSrc, vUv + vec2(x, y) * uTexel).rgb; }
    float w(vec3 c){ return 1.0 / (1.0 + max(max(c.r, c.g), c.b)); }
    void main(){
      vec3 a = s(-2.0, 2.0), b = s(0.0, 2.0), c = s(2.0, 2.0), d = s(-2.0, 0.0), e = s(0.0, 0.0), f = s(2.0, 0.0), g = s(-2.0, -2.0), h = s(0.0, -2.0), i = s(2.0, -2.0);
      vec3 j = s(-1.0, 1.0), k = s(1.0, 1.0), l = s(-1.0, -1.0), m = s(1.0, -1.0);
      vec3 o;
      if (uKaris > 0.5) {
        vec3 g0 = (j + k + l + m) * 0.25, g1 = (a + b + d + e) * 0.25, g2 = (b + c + e + f) * 0.25, g3 = (d + e + g + h) * 0.25, g4 = (e + f + h + i) * 0.25;
        float w0 = w(g0) * 0.5, w1 = w(g1) * 0.125, w2 = w(g2) * 0.125, w3 = w(g3) * 0.125, w4 = w(g4) * 0.125;
        o = (g0 * w0 + g1 * w1 + g2 * w2 + g3 * w3 + g4 * w4) / (w0 + w1 + w2 + w3 + w4);
      } else o = e * 0.125 + (a + c + g + i) * 0.03125 + (b + d + f + h) * 0.0625 + (j + k + l + m) * 0.125;
      gl_FragColor = vec4(o, 1.0);
    }`;

const KNOWLEDGE_UNIVERSE_UP_FRAG = `uniform sampler2D tSrc; uniform vec2 uTexel; varying vec2 vUv;
    vec3 s(float x, float y){ return texture2D(tSrc, vUv + vec2(x, y) * uTexel).rgb; }
    void main(){
      vec3 o = s(0.0, 0.0) * 4.0 + (s(0.0, 1.0) + s(-1.0, 0.0) + s(1.0, 0.0) + s(0.0, -1.0)) * 2.0 + (s(-1.0, 1.0) + s(1.0, 1.0) + s(-1.0, -1.0) + s(1.0, -1.0));
      gl_FragColor = vec4(o / 16.0, 0.0);
    }`;

const KNOWLEDGE_UNIVERSE_COMP_FRAG = `uniform sampler2D tScene; uniform sampler2D tBloom; uniform float uBloom; uniform float uExposure; uniform float uDark; uniform vec3 uPaper; uniform float uTime; varying vec2 vUv;
    vec3 aces(vec3 x){ return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), 0.0, 1.0); }
    vec3 srgb(vec3 c){ c = clamp(c, 0.0, 1.0); return mix(c * 12.92, 1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055, step(vec3(0.0031308), c)); }
    float ign(vec2 p){ return fract(52.9829189 * fract(dot(p, vec2(0.06711056, 0.00583715)))); }
    void main(){
      vec3 c = (texture2D(tScene, vUv).rgb + texture2D(tBloom, vUv).rgb * uBloom) * uExposure;
      vec2 d = vUv - 0.5;
      vec3 m = aces(c) * (1.0 - 0.45 * dot(d, d));
      vec3 o;
      if (uDark > 0.5) o = srgb(m);
      else {
        float L = max(max(m.r, m.g), m.b);
        vec3 ink = vec3(0.07, 0.08, 0.11) + (m / max(L, 1e-4)) * 0.08;
        o = srgb(mix(uPaper, ink, clamp(L * 1.3, 0.0, 1.0)));
      }
      o += (ign(gl_FragCoord.xy + fract(uTime * 7.0) * 64.0) - 0.5) / 255.0;
      gl_FragColor = vec4(o, 1.0);
    }`;

/* ---- 우주의 자료 — 평면 지도에서 짓는다 ------------------------------------------------ */

/* 씨앗 하나에서 이어지는 수(시안 `kg-data.js`의 `prng`) — 난수는 없다: 같은 볼트는 같은 우주다. */
function knowledgeUniverseRandom(seed) {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) | 0;
    let mixed = Math.imul(state ^ (state >>> 15), 1 | state);
    mixed = (mixed + Math.imul(mixed ^ (mixed >>> 7), 61 | mixed)) ^ mixed;
    return ((mixed ^ (mixed >>> 14)) >>> 0) / 4294967296;
  };
}

/* 수 하나의 해시(시안 `universe`의 `hash`: 씨앗 x × 7919 + 17의 둘째 값). */
function knowledgeUniverseHash(seed) {
  const next = knowledgeUniverseRandom(seed * 7919 + 17);
  next();
  return next();
}

/* 볼트 크기의 몫(시안 `layout`의 spread) — (쪽 / `world-pages`)^`world-spread`. */
function knowledgeUniverseSpread(layout, pages) {
  const U = knowledgeUniverseTuning(layout.view);
  return (Math.max(1, pages) / U.worldPages) ** U.worldSpread;
}

/* 평면 자리를 세계로 옮기는 수: 평면 원반의 √쪽 당 반지름(`cluster-pitch`)을 시안의 r2 계수(`world-root` × 몫)로.
 * 원반을 넓히는 `cluster-spread`(t-12029)로는 나누지 않는다 — 넓힌 뒤 틈이 줄어든 원반의 자리를 그대로 옮기면 이웃
 * 은하가 겹친다; 틈 150을 이완한 자리를 v4의 은하 크기로 옮겨야 이웃 은하의 거리/반지름 합이 v4처럼 1.4를 넘는다
 * (디자이너 m-12586). 반지름은 원반에서가 아니라 쪽 수에서 v4의 식으로 셈한다(`knowledgeUniverseMap`). */
function knowledgeUniverseScale(layout, pages) {
  const U = knowledgeUniverseTuning(layout.view);
  return (U.worldRoot * knowledgeUniverseSpread(layout, pages)) / Math.max(1e-6, layout.tuning.clusterPitch);
}

function knowledgeUniverseLinear(value) {
  return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
}

/* 색과 테마마다의 수 — CSS에게 묻는다(평면의 GL 손과 같은 문). 은하 빛깔의 원천 여덟은 계산된 색으로 읽고
 * (`color-mix`까지 풀리게 견본의 `color`로), 선형으로 옮긴다. 테마가 바뀌면 다시 묻는다. */
function knowledgeUniverseInks(view, probe) {
  const style = getComputedStyle(view);
  const tints = new Float32Array(KNOWLEDGE_HUES * 3);
  const read = new Float32Array(4);
  for (let hue = 0; hue < KNOWLEDGE_HUES; hue += 1) {
    probe.style.color = `var(--knowledge-3d-tint-${hue})`;
    knowledgeGlColor(getComputedStyle(probe).color, read, 0);
    for (let channel = 0; channel < 3; channel += 1) tints[hue * 3 + channel] = knowledgeUniverseLinear(read[channel]);
  }
  probe.style.color = "var(--knowledge-3d-paper)";
  knowledgeGlColor(getComputedStyle(probe).color, read, 0);
  const paper = [0, 1, 2].map((channel) => knowledgeUniverseLinear(read[channel]));
  const neutral = style.getPropertyValue("--knowledge-3d-galaxy-tint-neutral").trim().split(/\s+/u).map(Number);
  const themed = Object.fromEntries(Object.entries(KNOWLEDGE_UNIVERSE_THEMED)
    .map(([key, name]) => [key, Number.parseFloat(style.getPropertyValue(name))]));
  const theme = document.documentElement.dataset.theme === "light" ? "light" : "dark";
  return { theme, dark: theme === "dark", tints, paper, neutral, ...themed };
}

/* 우주의 자료 — 평면 지도의 군집과 원반에서(시안 `universe`, PORTING.md §2.2·§4).
 *
 * 군집은 평면 지도가 이미 가른 것(Louvain 순위)이다. 이름 있는 군집 중 앞에서부터 은하, 그 뒤는 성단(텍스처
 * 줄이 닿는 데까지), 이름 없는 군집과 줄 밖은 떠돌이 별이다. 은하의 자리는 평면 원반의 자리(x·z)에 높이만
 * 더한 것이고, 씨앗(높이·기울기·팔 위상)은 순위가 아니라 군집의 대표 쪽 열쇠에서 뽑는다 — 쪽 하나가 늘어
 * 순위가 바뀌어도 은하가 뒤집히지 않는다. 이름 있는 별은 움직이지 않는다: 차례 0(연결이 가장 많은 쪽)이
 * 핵에 가장 가깝고, 나머지는 셰이더의 팔 식과 같은 식으로 팔 위에 앉는다. */
function knowledgeUniverseMap(layout, inks) {
  const U = knowledgeUniverseTuning(layout.view);
  const SHAPE = KNOWLEDGE_UNIVERSE_SHAPE;
  const TYPE = KNOWLEDGE_UNIVERSE_TYPES;
  const SEAT = KNOWLEDGE_UNIVERSE_SEATS;
  const { model, count, community, communitySize, namedCount, communityHomeX, communityHomeY, communityHomeR,
    communityCore, communityHue } = layout;
  let pages = 0;
  for (let at = 0; at < count; at += 1) if (model.kinds[at] === "page") pages += 1;
  const scale = knowledgeUniverseScale(layout, pages);
  const spread = knowledgeUniverseSpread(layout, pages);
  const rows = Math.min(namedCount, SHAPE.rows);
  const least = Math.max(U.galaxyLeast, Math.round((U.galaxyShare * pages) / U.worldPages));
  let galaxies = 0;
  while (galaxies < Math.min(rows, U.galaxyMax) && communitySize[galaxies] >= least) galaxies += 1;
  /* 닿는 거리 — 은하 원반의 가장 먼 끝(시안 `layout`의 reach는 주제만 센다). 은하가 없는 볼트는 줄 전부. */
  let reach = 0;
  const reachRows = galaxies > 0 ? galaxies : rows;
  for (let rank = 0; rank < reachRows; rank += 1) {
    reach = Math.max(reach, Math.hypot(communityHomeX[rank], communityHomeY[rank]) * scale
      + U.worldRoot * Math.sqrt(communitySize[rank]) * spread);
  }
  reach = Math.max(1, reach);
  /* 줄마다 쪽을 모은다 — 연결이 많은 차례로(같으면 열쇠 순), 차례 0이 허브다. */
  const members = Array.from({ length: rows }, () => []);
  for (let at = 0; at < count; at += 1) if (community[at] < rows) members[community[at]].push(at);
  for (const list of members) {
    list.sort((left, right) => model.degree[right] - model.degree[left]
      || (model.keys[left] < model.keys[right] ? -1 : 1));
  }
  /* 활동 — 최근 `activity-days`일 안에 고친 쪽의 몫. 고친 때를 모르는 쪽은 오래된 것이다. */
  const windowMs = U.activityDays * 86_400_000;
  const activity = new Float32Array(rows);
  for (let row = 0; row < rows; row += 1) {
    let recent = 0;
    for (const at of members[row]) {
      if (model.modified[at] > 0 && model.nowMs - model.modified[at] < windowMs) recent += 1;
    }
    activity[row] = members[row].length > 0 ? recent / members[row].length : 0;
  }
  /* 타원의 문턱 — 고정 토큰과 은하 활동의 가운데값(짝수면 가운데 둘의 평균) 중 큰 쪽: 바쁜 볼트에서도 그 안에서
   * 조용한 주제가 타원이 된다(디자이너 m-12546의 3). */
  const busy = Array.from(activity.subarray(0, galaxies)).sort((left, right) => left - right);
  const middleBusy = busy.length === 0 ? 0 : busy.length % 2 === 1 ? busy[busy.length >> 1]
    : (busy[busy.length / 2 - 1] + busy[busy.length / 2]) / 2;
  const quietBelow = Math.max(U.ellipticalActivity, middleBusy);
  /* 법선의 바탕: 처음 시점 쪽으로 `normal-lean`만큼 기울인 위쪽 — 처음 화면에서 모든 은하의 안이 보이고, 돌면
   * 옆모습이 된다. */
  const camera = [Math.cos(U.homePitch) * Math.sin(U.homeYaw), Math.sin(U.homePitch),
    Math.cos(U.homePitch) * Math.cos(U.homeYaw)];
  let bx = camera[0] * U.normalLean;
  let by = 1 + camera[1] * U.normalLean;
  let bz = camera[2] * U.normalLean;
  const bl = Math.hypot(bx, by, bz);
  bx /= bl;
  by /= bl;
  bz /= bl;
  let ax = by;
  let ay = -bx;
  const al = Math.hypot(ax, ay) || 1;
  ax /= al;
  ay /= al;
  const qx = -bz * ay;
  const qy = bz * ax;
  const qz = bx * ay - by * ax;
  const gal = new Float32Array(SHAPE.texels * SHAPE.rows * 4);
  const put = (row, texel, a, b, c, d) => {
    const at = (row * SHAPE.texels + texel) * 4;
    gal[at] = a;
    gal[at + 1] = b;
    gal[at + 2] = c;
    gal[at + 3] = d;
  };
  const pos3 = new Float32Array(count * 3);
  const rowOf = new Int32Array(count).fill(-1);
  const galaxy = [];
  const biggest = Math.max(1, communitySize[0] ?? 1);
  for (let row = 0; row < rows; row += 1) {
    const rank = row;
    const core = communityCore[rank];
    const seed = knowledgeHash(model.keys[core >= 0 ? core : members[row][0]]);
    const h1 = knowledgeUniverseHash(seed + 1);
    const h2 = knowledgeUniverseHash(seed + 101);
    const h3 = knowledgeUniverseHash(seed + 211);
    const h4 = knowledgeUniverseHash(seed + 307);
    const isGalaxy = row < galaxies;
    let type = TYPE.cluster;
    let arms = U.arms;
    let winding = U.winding;
    let bar = 0;
    let bulge = U.bulge;
    let dust = U.dust;
    let radius;
    let height;
    if (isGalaxy) {
      if (rank < U.barredRanks) {
        type = TYPE.barred;
        bar = U.bar;
        winding = U.windingBar;
        bulge = U.bulgeBar;
      } else if (galaxies >= U.irregularLeast && rank >= galaxies - U.irregularLast) {
        type = TYPE.irregular;
        dust = U.dustIrregular;
      } else if (rank >= U.ellipticalFrom && rank % U.ellipticalEvery === 0 && activity[row] < quietBelow) {
        type = TYPE.elliptical;
        bulge = U.bulgeElliptical;
        dust = 0;
      } else {
        type = TYPE.spiral;
        arms = rank % U.armsEvery === U.armsEvery - 1 ? U.armsWide : U.arms;
        winding = U.windingLeast + h3 * U.windingSpan;
        bulge = U.bulgeLeast + h4 * U.bulgeSpan;
      }
      radius = U.galaxyRadius * U.worldRoot * Math.sqrt(communitySize[rank]) * spread;
      height = (h1 - 0.5) * 2 * U.liftGalaxy * reach;
    } else {
      radius = U.clusterSphere * Math.sqrt(communitySize[rank]) * spread * U.clusterGrow + U.clusterPad;
      dust = 0;
      height = (h1 - 0.5) * 2 * U.liftCluster * reach;
    }
    /* 기울기: 바탕에서 씨앗 방향으로 `tilt-least` + 씨앗 × `tilt-span` rad. */
    const tilt = U.tiltLeast + h2 * U.tiltSpan;
    const phi = h3 * Math.PI * 2;
    const st = Math.sin(tilt);
    const ct = Math.cos(tilt);
    const cp = Math.cos(phi);
    const sp = Math.sin(phi);
    let nx = bx * ct + (ax * cp + qx * sp) * st;
    let ny = by * ct + (ay * cp + qy * sp) * st;
    let nz = bz * ct + qz * sp * st;
    const nl = Math.hypot(nx, ny, nz);
    nx /= nl;
    ny /= nl;
    nz /= nl;
    let ux = ny;
    let uy = -nx;
    const ul = Math.hypot(ux, uy) || 1;
    ux /= ul;
    uy /= ul;
    const uz = 0;
    const vx = ny * uz - nz * uy;
    const vy = nz * ux - nx * uz;
    const vz = nx * uy - ny * ux;
    const theta0 = h4 * Math.PI * 2;
    const size = members[row].length;
    const decor = type === TYPE.cluster ? U.decorCluster
      : Math.round(U.decorLeast + U.decorGrow * Math.sqrt(communitySize[rank] / biggest));
    const cx = communityHomeX[rank] * scale;
    const cz = communityHomeY[rank] * scale;
    const hue = communityHue[rank];
    const tint = hue >= 0 ? [0, 1, 2].map((channel) => U.galaxyTintBase
      + (1 - U.galaxyTintBase) * inks.tints[hue * 3 + channel]) : inks.neutral;
    put(row, 0, cx, height, cz, radius);
    put(row, 1, nx, ny, nz, type);
    put(row, 2, ux, uy, uz, arms);
    put(row, 3, winding, bar, bulge, activity[row]);
    /* 텍셀 4(평면 자리)는 평면 카메라의 눌림을 따라 `knowledgeUniverseFlat`이 채운다. */
    put(row, 5, tint[0], tint[1], tint[2], dust);
    put(row, 6, decor, theta0, knowledgeUniverseHash(seed + 401), 0);
    galaxy.push({ rank, type, radius, x: cx, y: height, z: cz, normal: [nx, ny, nz], across: [ux, uy, uz],
      along: [vx, vy, vz], arms, winding, bar, bulge, activity: activity[row], decor, theta0, hue, size });
    /* 이름 있는 별의 자리(시안 `universe` 3). 흔들림은 쪽마다 제 열쇠의 씨앗에서 — 쪽 하나가 늘어도 남의 별이
     * 옮지 않는다. */
    const nCore = Math.max(1, Math.round(size * SEAT.coreShare));
    for (let order = 0; order < size; order += 1) {
      const at = members[row][order];
      rowOf[at] = row;
      const jitter = knowledgeUniverseRandom(knowledgeHash(model.keys[at]));
      const t = (order + 0.5) / size;
      let x = 0;
      let y = 0;
      let h = 0;
      if (type === TYPE.spiral || type === TYPE.barred) {
        if (order < nCore) {
          const r = radius * SEAT.coreRadius * Math.sqrt((order + 0.5) / nCore);
          const a = order * KNOWLEDGE_GOLDEN_ANGLE;
          x = Math.cos(a) * r;
          y = Math.sin(a) * r;
          h = (jitter() - 0.5) * radius * SEAT.coreLift;
        } else {
          const u = (order - nCore + 0.5) / Math.max(1, size - nCore);
          const r = radius * (SEAT.armInner + SEAT.armSpan * u ** SEAT.armPower) * (1 + (jitter() - 0.5) * SEAT.armJitter);
          const arm = (order - nCore) % arms;
          let a;
          if (type === TYPE.barred && r < radius * bar) {
            a = theta0 + (arm % 2) * Math.PI + (jitter() - 0.5) * SEAT.barJitter;
          } else {
            a = theta0 + (arm * Math.PI * 2) / arms
              + winding * Math.log(Math.max(r, radius * SEAT.armFloor) / (radius * SEAT.armFloor))
              + (jitter() + jitter() - 1) * SEAT.armAngle;
          }
          x = Math.cos(a) * r;
          y = Math.sin(a) * r;
          h = (jitter() - 0.5) * radius * SEAT.armLift;
        }
      } else if (type === TYPE.elliptical) {
        const r = radius * SEAT.ellipticalRadius * t ** SEAT.ellipticalPower;
        const a = order * KNOWLEDGE_GOLDEN_ANGLE;
        const yy = 1 - 2 * t;
        const ring = Math.sqrt(Math.max(0, 1 - yy * yy));
        x = Math.cos(a) * ring * r;
        y = Math.sin(a) * ring * r * SEAT.ellipticalFlat;
        h = yy * r * SEAT.ellipticalLift;
      } else if (type === TYPE.irregular) {
        const clump = order % SEAT.clumps;
        const ca = theta0 + clump * SEAT.clumpTurn;
        const cr = radius * (SEAT.clumpInner + SEAT.clumpStep * clump);
        x = Math.cos(ca) * cr + (jitter() + jitter() - 1) * radius * SEAT.clumpSpread;
        y = Math.sin(ca) * cr + (jitter() + jitter() - 1) * radius * SEAT.clumpSpread;
        h = (jitter() - 0.5) * radius * SEAT.clumpLift;
      } else {
        const r = radius * SEAT.clusterRadius * t ** SEAT.clusterPower;
        const a = order * KNOWLEDGE_GOLDEN_ANGLE;
        const yy = 1 - 2 * t;
        const ring = Math.sqrt(Math.max(0, 1 - yy * yy));
        x = Math.cos(a) * ring * r;
        y = Math.sin(a) * ring * r;
        h = yy * r;
      }
      pos3[at * 3] = cx + ux * x + vx * y + nx * h;
      pos3[at * 3 + 1] = height + uy * x + vy * y + ny * h;
      pos3[at * 3 + 2] = cz + uz * x + vz * y + nz * h;
    }
  }
  /* 필라멘트(시안 `buildScene`과 `kg-data.js`의 flows): 두 은하 사이의 선(선언·추론 모두)을 보낸 쪽 → 받는 쪽으로
   * 세고, 합이 `filament-least` 이상인 쌍을 굵은 차례로 셰이더 배열이 담는 데까지. 빛은 많이 보낸 쪽에서 흐른다. */
  const flows = new Map();
  for (let at = 0; at < model.edgeCount; at += 1) {
    const a = rowOf[model.from[at]];
    const b = rowOf[model.to[at]];
    if (a < 0 || b < 0 || a === b || a >= galaxies || b >= galaxies) continue;
    const low = Math.min(a, b);
    const high = Math.max(a, b);
    const key = low * SHAPE.rows + high;
    const held = flows.get(key) ?? { a: low, b: high, ab: 0, ba: 0, n: 0 };
    if (a < b) held.ab += 1;
    else held.ba += 1;
    held.n += 1;
    flows.set(key, held);
  }
  const chosen = [...flows.values()].filter((flow) => flow.n >= U.filamentLeast)
    .sort((left, right) => right.n - left.n || left.a - right.a || left.b - right.b)
    .slice(0, Math.min(U.filamentMost, SHAPE.bundles));
  const most = chosen[0]?.n ?? 1;
  const filaments = chosen.map((flow) => {
    const forward = flow.ab >= flow.ba;
    const strength = Math.sqrt(flow.n / most);
    return { a: flow.a, b: flow.b, n: flow.n, ab: flow.ab, ba: flow.ba,
      from: forward ? flow.a : flow.b, to: forward ? flow.b : flow.a, sent: Math.max(flow.ab, flow.ba), strength,
      points: Math.round(U.filamentLeastPoints + U.filamentGrowPoints * strength),
      bend: ((flow.a * SEAT.bendLow + flow.b * SEAT.bendHigh) % SEAT.bendSteps) / 2 - 1,
      phase: ((flow.a * SEAT.phaseLow + flow.b) % SEAT.phaseSteps) / SEAT.phaseSteps };
  });
  return { scale, reach, rows, galaxies, pages, gal, pos3, rowOf, galaxy, members, filaments };
}

/* ---- 한 판의 우주 ---------------------------------------------------------------------- */

function makeKnowledgeUniverse(view) {
  const U = knowledgeUniverseTuning(view);
  return {
    view,
    tune: U,
    host: null,
    canvas: null,
    labels: null,
    notice: null,
    noticeTimer: 0,
    renderer: null,
    scene: null,
    camera: null,
    uniforms: null,
    materials: null,
    /* 지은 것 — 떠날 때 놓는 목록. */
    built: { geometries: [], objects: [] },
    layout: null,
    /* 평면 자리가 마지막으로 옮겨진 판의 도장 — 앉기가 끝나면 한 번 더 옮긴다. */
    seatsRevision: -1,
    scale: 1,
    reach: 1,
    pos2: null,
    pos3: null,
    starGeometry: null,
    starState: null,
    starStateAttr: null,
    /* 우주의 자료(`knowledgeUniverseMap`)와 그 텍스처, 색(`knowledgeUniverseInks`)과 견본, 후처리의 살림. */
    map: null,
    galTexture: null,
    decorCount: 0,
    filaments: [],
    /* 쪽 자리 텍스처 둘과 별자리 선의 살림(시안 `tNode3`·`tNode2`·`focusEdges`), 화면 투영(고르기·이름표). */
    node3: null,
    node2: null,
    nodeTextures: [],
    edgeData: null,
    focusEdges: null,
    focusAttr: null,
    lineGeometry: null,
    lineObject: null,
    proj: null,
    projVersion: -1,
    viewProjection: null,
    /* 초점: 올린 별(우주만의 것), 찾은 별의 수, 마지막으로 본 고름·군집(바뀌면 난다). */
    hover: -1,
    hits: 0,
    seenSelected: null,
    seenTopic: -1,
    pickQueued: false,
    /* 고른 쪽의 자리 — `refreshStates`가 적고 이름표가 읽는다. */
    selectedSeat: -1,
    /* 이름표(시안 `buildTopicCards`·`updateLabels`): 은하마다 명판, 쪽 이름의 풀, 굵은 필라멘트의 수, 올린 별의 팁.
     * 자리는 camVersion이 바뀐 프레임에만 다시 놓고(`labelVersion`), 겹침은 이미 놓인 상자들(`boxes`)로 막는다.
     * 명판의 크기는 한 번 재고(`measured`), 쪽 이름의 넓이는 쪽마다 한 번 잰다(`nameWidths`). */
    plates: [],
    names: [],
    flowTags: [],
    tip: null,
    tipBox: null,
    labelVersion: -1,
    measured: false,
    boxes: null,
    boxCount: 0,
    anchorX: null,
    anchorLow: null,
    anchorHigh: null,
    anchorSeen: null,
    order: null,
    candidates: null,
    candidateScores: null,
    bright: [],
    declared: null,
    inferred: null,
    nameWidths: null,
    nameFont: "",
    namePad: 0,
    nameHotPad: 0,
    nameContext: null,
    spot: new Float64Array(3),
    inks: null,
    probe: null,
    post: null,
    floatTargets: false,
    themeWatch: null,
    /* 카메라(시안 `S`): 궤도의 두 각, 거리, 목표점, 평면에서 떠오른 몫. */
    yaw: U.homeYaw,
    pitch: U.homePitch,
    dist: 300,
    tx: 0,
    ty: 0,
    tz: 0,
    lift: 1,
    home: null,
    saved: null,
    camVersion: 0,
    lastTouch: -1e9,
    dragging: false,
    tween: { on: false, t0: 0, dur: 1, from: {}, to: {}, done: null },
    glide: { on: false, vx: 0, vy: 0 },
    spin: 1,
    time: 0,
    raf: 0,
    lastNow: 0,
    frames: 0,
    width: 1,
    height: 1,
    lost: false,
    folding: false,
    /* 사람의 「움직임」 — 끈 판과 움직임을 줄이라는 판에서는 돌지도 흐르지도 않는다. */
    motion: knowledgeUniverseMotion,
    down: null,
    moved: { x: 0, y: 0, t: 0 },
    clickable: false,
    pointer: { x: -1, y: -1, inside: false },
    unlisten: [],

    /* 캔버스·이름표 층·렌더러를 세운다. 서지 못하면 거짓 — 부르는 쪽이 놓고 평면으로 선다. */
    mount() {
      const stage = this.view.querySelector(".knowledge-canvas");
      if (stage === null || typeof THREE !== "object") return false;
      const host = document.createElement("div");
      host.className = "knowledge-universe";
      const canvas = document.createElement("canvas");
      canvas.className = "knowledge-universe-gl";
      canvas.setAttribute("role", "img");
      const labels = document.createElement("div");
      labels.className = "knowledge-universe-labels";
      /* 색을 묻는 견본 하나 — 은하 빛깔의 원천과 종이를 계산된 색으로 읽는다(`knowledgeUniverseInks`). */
      const probe = document.createElement("span");
      probe.className = "knowledge-universe-probe";
      probe.setAttribute("aria-hidden", "true");
      host.append(canvas, labels, probe);
      this.probe = probe;
      stage.appendChild(host);
      this.host = host;
      this.canvas = canvas;
      this.labels = labels;
      let renderer = null;
      try {
        /* 깊이도 스텐실도 없다 — 모든 것이 부드러운 스프라이트이고 빛은 더해진다(시안 `makeRenderer`). */
        renderer = new THREE.WebGLRenderer({ canvas, antialias: false, alpha: false, depth: false, stencil: false,
          powerPreference: "high-performance" });
      } catch (trouble) {
        invoke("note_webview_error", { text: `knowledge universe: ${trouble}` }).catch(() => {});
        return false;
      }
      this.renderer = renderer;
      if (!renderer.capabilities.isWebGL2) return false;
      renderer.toneMapping = THREE.NoToneMapping;
      renderer.sortObjects = false;
      renderer.autoClear = false;
      renderer.info.autoReset = false;
      /* 셰이더가 서지 않으면 three.js는 콘솔에 적고 빈 그림을 그린다 — 이 손은 던져서 평면으로
       * 돌아간다(`build`의 `compile`이 받는다). */
      renderer.debug.onShaderError = (context, program, vertexShader, fragmentShader) => {
        throw new Error([context.getProgramInfoLog(program),
          context.getShaderInfoLog(vertexShader), context.getShaderInfoLog(fragmentShader)]
          .filter((word) => word).join(" / "));
      };
      this.scene = new THREE.Scene();
      /* 원근 카메라 하나(시안은 번들에 이것이 없어 직교 카메라의 투영을 갈아 끼웠다 — 앱의 번들은 이것을
       * 싣는다, ui/vendor/build-three.mjs). 비는 판 크기가 정한다(`resize`). */
      const camera = new THREE.PerspectiveCamera(U.fov, 1, U.near, U.far);
      this.camera = camera;
      /* 모든 재질이 함께 보는 값(시안 `U`): 평면↔우주의 몫, 시간, 초점과 초점 은하, 흐름, 세로 픽셀/초점 거리,
       * 기기 픽셀, 밝기 기준 거리(처음 자리 거리), 테마, 은하 텍셀, 쪽 자리 둘과 그 폭, 필라멘트 둘. */
      const SHAPE = KNOWLEDGE_UNIVERSE_SHAPE;
      this.uniforms = {
        uLift: { value: 1 }, uTime: { value: 0 }, uFocus: { value: 0 }, uFlow: { value: 1 }, uFocusGal: { value: -1 },
        uPx: { value: 800 }, uDpr: { value: 1 }, uRef: { value: 360 }, uDark: { value: 1 },
        tGal: { value: null }, tNode3: { value: null }, tNode2: { value: null }, uTexW: { value: 1 },
        uBundle: { value: Array.from({ length: SHAPE.bundles }, () => new THREE.Vector4()) },
        uBundle2: { value: Array.from({ length: SHAPE.bundles }, () => new THREE.Vector4()) },
      };
      /* 섞기: 미리 곱한 알파(ONE, ONE_MINUS_SRC_ALPHA) — 빛(α=0)은 더하고, 먼지(rgb=0, α>0)는 뒤를 가린다
       * (시안 `mat`). */
      const material = (vertexShader, fragmentShader) => new THREE.ShaderMaterial({
        uniforms: this.uniforms, vertexShader, fragmentShader, transparent: true, depthTest: false, depthWrite: false,
        blending: THREE.NormalBlending, premultipliedAlpha: true,
      });
      this.materials = {
        glows: material(KNOWLEDGE_UNIVERSE_GLOWS_VERT, KNOWLEDGE_UNIVERSE_GLOWS_FRAG),
        decor: material(KNOWLEDGE_UNIVERSE_DECOR_VERT, KNOWLEDGE_UNIVERSE_DECOR_FRAG),
        stars: material(KNOWLEDGE_UNIVERSE_STARS_VERT, KNOWLEDGE_UNIVERSE_STARS_FRAG),
        lines: material(KNOWLEDGE_UNIVERSE_LINES_VERT, KNOWLEDGE_UNIVERSE_LINES_FRAG),
      };
      this.floatTargets = renderer.extensions.has("EXT_color_buffer_float")
        || renderer.extensions.has("EXT_color_buffer_half_float");
      this.makePost();
      this.inks = knowledgeUniverseInks(this.view, this.probe);
      this.applyInks();
      /* 테마가 바뀌면 색을 다시 묻고 한 장을 그린다(평면의 GL 손과 같은 귀). */
      this.themeWatch = new MutationObserver(() => {
        if (this.renderer === null) return;
        this.inks = knowledgeUniverseInks(this.view, this.probe);
        this.applyInks();
        this.invalidate();
      });
      this.themeWatch.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
      this.wire();
      return true;
    },

    /* 후처리의 살림(시안 `down`·`up`·`comp`·`post`): 전면 삼각형 하나와 재질 셋. 목표는 판 크기를 따라
     * `makeTargets`가 짓는다. */
    makePost() {
      const U = this.tune;
      const pass = (options) => new THREE.ShaderMaterial({ vertexShader: KNOWLEDGE_UNIVERSE_POST_VERT,
        depthTest: false, depthWrite: false, transparent: false, ...options });
      const down = pass({
        uniforms: { tSrc: { value: null }, uTexel: { value: new THREE.Vector2() }, uKaris: { value: 0 } },
        fragmentShader: KNOWLEDGE_UNIVERSE_DOWN_FRAG,
      });
      const up = pass({
        transparent: true, blending: THREE.AdditiveBlending, premultipliedAlpha: true,
        uniforms: { tSrc: { value: null }, uTexel: { value: new THREE.Vector2() } },
        fragmentShader: KNOWLEDGE_UNIVERSE_UP_FRAG,
      });
      const comp = pass({
        uniforms: { tScene: { value: null }, tBloom: { value: null }, uBloom: { value: U.bloomStrength },
          uExposure: { value: 1 }, uDark: { value: 1 }, uPaper: { value: new THREE.Vector3() }, uTime: { value: 0 } },
        fragmentShader: KNOWLEDGE_UNIVERSE_COMP_FRAG,
      });
      const triangle = new THREE.BufferGeometry();
      triangle.setAttribute("position", new THREE.BufferAttribute(new Float32Array([-1, -1, 0, 3, -1, 0, -1, 3, 0]), 3));
      const quad = new THREE.Mesh(triangle, comp);
      quad.frustumCulled = false;
      const scene = new THREE.Scene();
      scene.add(quad);
      this.post = { scene, lens: new THREE.OrthographicCamera(-1, 1, 1, -1, 0, 1), quad, triangle, down, up, comp,
        target: null, mips: [] };
    },

    /* 장면의 HDR 목표 한 장과 빛 번짐 사다리(한 단마다 절반). 깊이·스텐실은 없다. 반정밀 부동소수를 쓸 수 없는
     * 판은 RGBA8로 내려앉는다(번짐이 약해진다). */
    makeTargets() {
      this.dropTargets();
      const post = this.post;
      const wide = Math.max(1, this.canvas.width);
      const tall = Math.max(1, this.canvas.height);
      const options = { depthBuffer: false, stencilBuffer: false };
      if (this.floatTargets) options.type = THREE.HalfFloatType;
      post.target = new THREE.WebGLRenderTarget(wide, tall, options);
      let mipWide = wide;
      let mipTall = tall;
      for (let level = 0; level < this.tune.bloomLevels; level += 1) {
        mipWide = Math.max(1, mipWide >> 1);
        mipTall = Math.max(1, mipTall >> 1);
        post.mips.push(new THREE.WebGLRenderTarget(mipWide, mipTall, options));
      }
    },

    dropTargets() {
      const post = this.post;
      if (post === null) return;
      post.target?.dispose();
      for (const mip of post.mips) mip.dispose();
      post.target = null;
      post.mips.length = 0;
    },

    /* 색이 바뀌었다(처음·테마): 합성의 테마·노출·종이, 그리고 은하 빛깔(텍셀 5). */
    applyInks() {
      const inks = this.inks;
      const comp = this.post.comp.uniforms;
      comp.uDark.value = inks.dark ? 1 : 0;
      comp.uExposure.value = inks.exposure;
      comp.uPaper.value.set(inks.paper[0], inks.paper[1], inks.paper[2]);
      this.uniforms.uDark.value = inks.dark ? 1 : 0;
      if (this.map === null) return;
      const U = this.tune;
      const SHAPE = KNOWLEDGE_UNIVERSE_SHAPE;
      for (const row of this.map.galaxy) {
        const at = (row.rank * SHAPE.texels + 5) * 4;
        for (let channel = 0; channel < 3; channel += 1) {
          this.map.gal[at + channel] = row.hue >= 0
            ? U.galaxyTintBase + (1 - U.galaxyTintBase) * inks.tints[row.hue * 3 + channel] : inks.neutral[channel];
        }
      }
      if (this.galTexture !== null) this.galTexture.needsUpdate = true;
    },

    /* 손과 귀. 우주 위의 몸짓은 우주의 것이다 — 평면 캔버스의 손(끌기·고르기·바퀴)에 닿지 않게
     * 여기서 멈춘다. */
    wire() {
      const on = (target, name, run, options) => {
        target.addEventListener(name, run, options);
        this.unlisten.push(() => target.removeEventListener(name, run, options));
      };
      const host = this.host;
      on(this.canvas, "webglcontextlost", (event) => {
        event.preventDefault();
        this.loseContext();
      });
      on(this.canvas, "webglcontextrestored", () => this.regainContext());
      on(document, "visibilitychange", () => {
        if (document.hidden) this.halt();
        else this.invalidate();
      });
      on(host, "pointerdown", (event) => {
        event.stopPropagation();
        if (event.target.closest("button, a, input")) return;
        if (event.button !== 0 && event.button !== 2) return;
        host.setPointerCapture?.(event.pointerId);
        this.down = { x: event.clientX, y: event.clientY, pan: event.shiftKey || event.button === 2 };
        this.moved = { x: event.clientX, y: event.clientY, t: performance.now() };
        this.clickable = true;
        this.glide.on = false;
        this.lastTouch = performance.now();
        this.view.querySelector(".knowledge-canvas")?.focus({ preventScroll: true });
      });
      on(host, "pointermove", (event) => {
        event.stopPropagation();
        const box = host.getBoundingClientRect();
        this.pointer.x = event.clientX - box.left;
        this.pointer.y = event.clientY - box.top;
        this.pointer.inside = true;
        if (this.down === null) {
          this.schedulePick();
          return;
        }
        this.drag(event);
      });
      const release = (event) => {
        event.stopPropagation();
        if (this.down === null) return;
        const dragged = this.dragging;
        this.dragging = false;
        host.classList.remove("is-dragging");
        this.down = null;
        if (dragged && !knowledgeMotionReduced() && performance.now() - this.moved.t < U.glideWindow) {
          this.glide.on = true;
          this.glide.vx = Math.max(-U.glideYaw, Math.min(U.glideYaw, this.glide.vx));
          this.glide.vy = Math.max(-U.glidePitch, Math.min(U.glidePitch, this.glide.vy))
            * U.glidePitchShare;
          this.invalidate();
        }
        if (this.clickable && event.type === "pointerup") this.press();
      };
      on(host, "pointerup", release);
      on(host, "pointercancel", release);
      on(host, "pointerleave", () => {
        this.pointer.inside = false;
        if (this.down === null) this.setHover(-1);
      });
      for (const name of ["click", "pointerover", "pointerout"]) on(host, name, (event) => event.stopPropagation());
      on(host, "dblclick", (event) => {
        event.stopPropagation();
        if (event.target.closest("button")) return;
        this.goHome();
      });
      on(host, "contextmenu", (event) => {
        event.preventDefault();
        event.stopPropagation();
      });
      on(host, "wheel", (event) => {
        event.preventDefault();
        event.stopPropagation();
        if (this.folding) return;
        this.lastTouch = performance.now();
        const step = Math.exp(event.deltaY * U.zoomWheel);
        this.dist = Math.max(U.zoomMin, Math.min(this.home.dist * U.zoomMax, this.dist * step));
        this.camVersion += 1;
        this.tween.on = false;
        this.invalidate();
      }, { passive: false });
      watchGraphResize(host, () => this.resize());
    },

    /* 끄는 손 — 궤도, 또는 Shift·오른쪽 끌기로 옮기기(시안의 손 그대로). */
    drag(event) {
      if (this.folding) return;
      const dx = event.clientX - this.moved.x;
      const dy = event.clientY - this.moved.y;
      if (Math.abs(event.clientX - this.down.x) + Math.abs(event.clientY - this.down.y) > U.dragSlop) {
        this.clickable = false;
        this.dragging = true;
        this.host.classList.add("is-dragging");
      }
      if (!this.dragging) return;
      const now = performance.now();
      const span = Math.max(1, now - this.moved.t);
      if (this.down.pan) {
        const step = this.dist * U.dragPan;
        const cos = Math.cos(this.yaw);
        const sin = Math.sin(this.yaw);
        this.tx -= (dx * cos - dy * sin) * step;
        this.tz += (-dx * sin - dy * cos) * step;
      } else {
        this.yaw -= dx * U.dragYaw;
        this.pitch = this.clampPitch(this.pitch + dy * U.dragPitch);
        this.glide.vx = (dx * U.dragYaw) / (span / 1000);
        this.glide.vy = (dy * U.dragPitch) / (span / 1000);
      }
      this.tween.on = false;
      this.camVersion += 1;
      this.moved = { x: event.clientX, y: event.clientY, t: now };
      this.lastTouch = now;
      this.invalidate();
    },

    clampPitch(pitch) {
      return Math.max(U.pitchLow, Math.min(U.pitchHigh, pitch));
    },

    /* 판 크기 — 캔버스의 기기 픽셀과 카메라의 비, 처음 자리. */
    resize() {
      if (this.renderer === null) return;
      const box = this.host.getBoundingClientRect();
      this.width = Math.max(1, Math.round(box.width));
      this.height = Math.max(1, Math.round(box.height));
      this.renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, U.pixelCap));
      this.renderer.setSize(this.width, this.height, false);
      this.camera.aspect = this.width / this.height;
      this.camera.updateProjectionMatrix();
      this.makeTargets();
      if (this.layout !== null) this.computeHome();
      this.camVersion += 1;
      this.invalidate();
    },

    /* 이 판의 그림을 짓는다 — 위상이 바뀐 판에서 다시. 셰이더가 서지 않으면 거짓. */
    build(layout) {
      this.clearScene();
      this.layout = layout;
      this.seatsRevision = -1;
      const model = layout.model;
      const count = layout.count;
      const SHAPE = KNOWLEDGE_UNIVERSE_SHAPE;
      const map = knowledgeUniverseMap(layout, this.inks);
      this.map = map;
      this.scale = map.scale;
      this.reach = map.reach;
      this.pos3 = map.pos3;
      this.pos2 = new Float32Array(count * 3);
      /* 은하 매개변수 텍스처 — 은하마다 텍셀 8개, 줄 128(RGBA32F, NEAREST). 셰이더가 번호로 읽는다. */
      const galTexture = new THREE.DataTexture(map.gal, SHAPE.texels, SHAPE.rows, THREE.RGBAFormat, THREE.FloatType);
      galTexture.minFilter = THREE.NearestFilter;
      galTexture.magFilter = THREE.NearestFilter;
      galTexture.needsUpdate = true;
      this.galTexture = galTexture;
      this.uniforms.tGal.value = galTexture;
      /* 이름 있는 별: 밝기 = ((1 + 연결) / (1 + 가운데값))^지수, 빛깔 = 최근 고침(오늘 1 → 1년 0, 로그). 고친 때를
       * 모르는 쪽은 오래된 것으로 친다(디자이너 답 m-12467). 가운데값은 볼트 전체의 것이다. */
      const sorted = Int32Array.from(model.degree).sort();
      const middle = count > 0 ? sorted[count >> 1] : 0;
      const nowMs = model.nowMs;
      const star = new Float32Array(count * 4);
      for (let at = 0; at < count; at += 1) {
        const days = model.modified[at] > 0 ? Math.max(0, (nowMs - model.modified[at]) / 86_400_000)
          : U.starAgeSpan;
        star[at * 4] = Math.min((1 + model.degree[at]) / (1 + middle), U.starRatioCap) ** U.starLumExp;
        star[at * 4 + 1] = Math.max(0, Math.min(1,
          1 - Math.log1p(days / U.starAgeScale) / Math.log1p(U.starAgeSpan / U.starAgeScale)));
        star[at * 4 + 2] = map.rowOf[at];
      }
      this.placeSeats();
      /* 그리는 차례(시안 `renderOrder`): 원반·성운 0 → 장식 입자 1 → 이름 있는 별 2 → 별자리 선 3. 장면에 넣는
       * 차례가 그 차례다(정렬은 끈다). */
      const add = (object, order, key) => {
        object.frustumCulled = false;
        object.renderOrder = order;
        object.userData.key = key;
        this.scene.add(object);
        this.built.objects.push(object);
        return object;
      };
      /* 원반·핵·성운(시안 `bb`): 나선·막대·불규칙은 원반과 핵, 타원은 핵 하나, 성단은 성운 셋 — 성운은 앞
       * `nebula-max` 성단에만(fbm 조각이 가장 비싸다, 디자이너 답 m-12467). */
      const TYPE = KNOWLEDGE_UNIVERSE_TYPES;
      const glows = [];
      let clusters = 0;
      for (const row of map.galaxy) {
        if (row.type === TYPE.cluster) {
          if (clusters < U.nebulaMax) {
            glows.push(row.rank, 2, U.nebulaOuter, 0, row.rank, 2, U.nebulaMid, 1, row.rank, 2, U.nebulaInner, 2);
          }
          clusters += 1;
        } else if (row.type === TYPE.elliptical) {
          glows.push(row.rank, 1, U.coreElliptical, 1);
        } else {
          glows.push(row.rank, 0, U.disk, 0, row.rank, 1, U.core, 1);
        }
      }
      const glowGeometry = new THREE.InstancedBufferGeometry();
      glowGeometry.setAttribute("position", new THREE.BufferAttribute(new Float32Array([-1, -1, 0, 1, -1, 0, 1, 1, 0, -1, 1, 0]), 3));
      glowGeometry.setIndex([0, 1, 2, 0, 2, 3]);
      glowGeometry.setAttribute("aBB", new THREE.InstancedBufferAttribute(new Float32Array(glows), 4));
      glowGeometry.instanceCount = glows.length / 4;
      this.built.geometries.push(glowGeometry);
      add(new THREE.Mesh(glowGeometry, this.materials.glows), 0, "glows");
      /* 몸의 이름 없는 별(시안 `buildScene`의 장식 입자): 번호 = 무리 × `groupSpan` + 차례 — 줄마다 제 장식 수만큼,
       * 그 뒤 배경 별. 필라멘트의 번호는 그 뒤에 선다(④). */
      let total = U.decorSky;
      for (const row of map.galaxy) total += row.decor;
      for (const filament of map.filaments) total += filament.points;
      const codes = new Float32Array(total);
      let code = 0;
      for (const row of map.galaxy) {
        for (let step = 0; step < row.decor; step += 1) codes[code++] = row.rank * SHAPE.groupSpan + step;
      }
      for (let step = 0; step < U.decorSky; step += 1) codes[code++] = SHAPE.skyGroup * SHAPE.groupSpan + step;
      /* 필라멘트의 입자와 셰이더의 두 배열(시안 `uBundle`·`uBundle2`): 보낸 은하, 받는 은하, 굵기 √(n/가장 굵은 n),
       * 보낸 쪽의 몫 / 휨, 흐름의 위상. 빈 칸은 0이다. */
      this.filaments = map.filaments;
      map.filaments.forEach((filament, index) => {
        for (let step = 0; step < filament.points; step += 1) codes[code++] = (SHAPE.bundleGroup + index) * SHAPE.groupSpan + step;
      });
      this.uniforms.uBundle.value.forEach((slot, index) => {
        const filament = map.filaments[index];
        if (filament === undefined) slot.set(0, 0, 0, 0);
        else slot.set(filament.from, filament.to, filament.strength, filament.sent / filament.n);
      });
      this.uniforms.uBundle2.value.forEach((slot, index) => {
        const filament = map.filaments[index];
        if (filament === undefined) slot.set(0, 0, 0, 0);
        else slot.set(filament.bend, filament.phase, 0, 0);
      });
      const decorGeometry = new THREE.BufferGeometry();
      decorGeometry.setAttribute("position", new THREE.BufferAttribute(codes, 1));
      this.built.geometries.push(decorGeometry);
      this.decorCount = total;
      add(new THREE.Points(decorGeometry, this.materials.decor), 1, "decor");
      const geometry = new THREE.BufferGeometry();
      geometry.setAttribute("position", new THREE.BufferAttribute(this.pos3, 3));
      geometry.setAttribute("aPos2", new THREE.BufferAttribute(this.pos2, 3));
      geometry.setAttribute("aStar", new THREE.BufferAttribute(star, 4));
      this.starState = new Float32Array(count);
      this.starStateAttr = new THREE.BufferAttribute(this.starState, 1);
      this.starStateAttr.setUsage(THREE.DynamicDrawUsage);
      geometry.setAttribute("aState", this.starStateAttr);
      this.built.geometries.push(geometry);
      this.starGeometry = geometry;
      add(new THREE.Points(geometry, this.materials.stars), 2, "stars");
      /* 쪽 자리 텍스처 둘(우주·평면, RGBA32F NEAREST) — 별자리 선이 번호로 읽는다(시안 `tNode3`·`tNode2`). */
      const wide = SHAPE.nodeWide;
      const tall = Math.max(1, Math.ceil(count / wide));
      this.node3 = new Float32Array(wide * tall * 4);
      this.node2 = new Float32Array(wide * tall * 4);
      const nodeTexture = (data) => {
        const texture = new THREE.DataTexture(data, wide, tall, THREE.RGBAFormat, THREE.FloatType);
        texture.minFilter = THREE.NearestFilter;
        texture.magFilter = THREE.NearestFilter;
        texture.needsUpdate = true;
        return texture;
      };
      this.nodeTextures = [nodeTexture(this.node3), nodeTexture(this.node2)];
      this.uniforms.tNode3.value = this.nodeTextures[0];
      this.uniforms.tNode2.value = this.nodeTextures[1];
      this.uniforms.uTexW.value = wide;
      this.fillNodes();
      /* 별자리 선의 자료(시안): 선마다 (보낸 쪽, 받는 쪽, 추론이면 1 + 2 × (보낸 쪽 줄 + 밑 × 받는 쪽 줄), 흐름
       * 위상). 떠돌이 별의 줄은 줄 수(`rows`)로 적는다 — 어느 은하와도 다르다. */
      const edgeCount = model.edgeCount;
      const inferred = KNOWLEDGE_EDGE_PROVENANCE_CODE.inferred;
      this.edgeData = new Float32Array(edgeCount * 4);
      /* 팁의 「선언 · 추론」 — 쪽마다 제 선을 두 갈래로 센다(합이 연결 수다). */
      this.declared = new Int32Array(count);
      this.inferred = new Int32Array(count);
      let seed = SHAPE.lineSeed;
      const rowFor = (seat) => (map.rowOf[seat] >= 0 ? map.rowOf[seat] : SHAPE.rows);
      for (let at = 0; at < edgeCount; at += 1) {
        const from = model.from[at];
        const to = model.to[at];
        const guessed = model.provenance[at] === inferred
          || (model.provenance[at] === KNOWLEDGE_PROVENANCE_UNKNOWN && model.kind[at] === KNOWLEDGE_EDGE_CODE.mentions);
        const tally = guessed ? this.inferred : this.declared;
        tally[from] += 1;
        tally[to] += 1;
        this.edgeData[at * 4] = from;
        this.edgeData[at * 4 + 1] = to;
        this.edgeData[at * 4 + 2] = (guessed ? 1 : 0) + 2 * (rowFor(from) + SHAPE.linePack * rowFor(to));
        seed = (seed * SHAPE.lineMultiplier) % SHAPE.lineModulus;
        this.edgeData[at * 4 + 3] = seed / SHAPE.lineModulus;
      }
      /* 초점의 선만 담는 인스턴스 통 — 가장 많은 연결 + 경로(쪽 수를 넘지 않는다). */
      let widest = 1;
      for (let at = 0; at < count; at += 1) widest = Math.max(widest, model.degree[at]);
      this.focusEdges = new Float32Array((widest + count) * 4);
      this.focusAttr = new THREE.InstancedBufferAttribute(this.focusEdges, 4);
      this.focusAttr.setUsage(THREE.DynamicDrawUsage);
      const segments = SHAPE.lineSegments;
      const base = new Float32Array(segments * 2 * 3);
      for (let step = 0; step < segments; step += 1) {
        base[step * 6] = step / segments;
        base[step * 6 + 3] = (step + 1) / segments;
      }
      const lineGeometry = new THREE.InstancedBufferGeometry();
      lineGeometry.setAttribute("position", new THREE.BufferAttribute(base, 3));
      lineGeometry.setAttribute("aEdge", this.focusAttr);
      lineGeometry.instanceCount = 0;
      this.built.geometries.push(lineGeometry);
      this.lineGeometry = lineGeometry;
      this.lineObject = add(new THREE.LineSegments(lineGeometry, this.materials.lines), 3, "lines");
      this.lineObject.visible = false;
      this.proj = new Float32Array(count * 4);
      this.projVersion = -1;
      this.viewProjection = new THREE.Matrix4();
      this.hover = -1;
      /* 쉬는 먼 자리에서 이름을 다는 별 — 연결이 가장 많은 쪽들(같으면 자리 순, 시안 `G.bright`). 제목이 제 은하의
       * 이름과 같은 별(이름 없는 군집은 허브 제목이 이름이다)은 명판과 두 번 서지 않게 건너뛴다(디자이너 m-12642). */
      const sameAsGalaxy = (seat) => map.rowOf[seat] >= 0
        && knowledgeShortWord(model.titles[seat], KNOWLEDGE_COMMUNITY.labelMax) === knowledgeClusterWord(layout, map.rowOf[seat]);
      this.bright = Array.from({ length: count }, (unused, at) => at)
        .sort((one, two) => model.degree[two] - model.degree[one] || one - two)
        .filter((seat) => !sameAsGalaxy(seat)).slice(0, U.labelsRest);
      this.applyInks();
      this.computeHome();
      this.buildLabels();
      writeAttribute(this.canvas, "aria-label", t("knowledge.universeSummary", "지식 그래프 우주 · 주제 {{topics}} · 쪽 {{pages}}",
        { topics: layout.namedCount, pages: count }));
      try {
        this.placeCamera();
        this.renderer.compile(this.scene, this.camera);
        this.renderer.compile(this.post.scene, this.post.lens);
      } catch (trouble) {
        invoke("note_webview_error", { text: `knowledge universe: ${trouble}` }).catch(() => {});
        return false;
      }
      return true;
    },

    /* 평면 자리(떠오르기 전)와 떠돌이 별의 자리. 평면 자리는 평면 지도가 그린 자리(좁은 판의 세로 눌림까지)이고,
     * 은하 몸은 평면에서 제 원반의 가운데로 모인다(텍셀 4). 떠돌이 별은 평면 자리 그대로에 높이만 — 앉기가 끝난
     * 판에서 한 번 더 옮긴다(`dress`). */
    placeSeats() {
      const layout = this.layout;
      const map = this.map;
      const { count, x, y, drawY, model } = layout;
      const SHAPE = KNOWLEDGE_UNIVERSE_SHAPE;
      const s = this.scale;
      const flat = U.flatHeight;
      const box = layout.viewBoxRect;
      const middleY = box?.middleY ?? 0;
      const yScale = box?.yScale ?? 1;
      for (let at = 0; at < count; at += 1) {
        this.pos2[at * 3] = x[at] * s;
        this.pos2[at * 3 + 1] = flat;
        this.pos2[at * 3 + 2] = drawY[at] * s;
        if (map.rowOf[at] >= 0) continue;
        const seed = knowledgeHash(model.keys[at]);
        this.pos3[at * 3] = x[at] * s;
        this.pos3[at * 3 + 1] = (knowledgeUniverseHash(seed + 5) - 0.5) * 2 * U.liftOrphan * this.reach;
        this.pos3[at * 3 + 2] = y[at] * s;
      }
      for (const row of map.galaxy) {
        const at = (row.rank * SHAPE.texels + 4) * 4;
        map.gal[at] = layout.communityHomeX[row.rank] * s;
        map.gal[at + 1] = flat;
        map.gal[at + 2] = (middleY + (layout.communityHomeY[row.rank] - middleY) * yScale) * s;
        map.gal[at + 3] = layout.communityHomeR[row.rank] * s;
      }
      this.seatsRevision = layout.geometryRevision;
      this.projVersion = -1;
      this.fillNodes();
      if (this.galTexture !== null) this.galTexture.needsUpdate = true;
      if (this.starGeometry !== null) {
        this.starGeometry.attributes.position.needsUpdate = true;
        this.starGeometry.attributes.aPos2.needsUpdate = true;
      }
    },

    /* 쪽 자리 텍스처를 채운다 — 우주의 자리(텍셀 = x, y, z, 1)와 평면의 자리. */
    fillNodes() {
      if (this.node3 === null) return;
      const count = this.layout.count;
      for (let at = 0; at < count; at += 1) {
        for (let axis = 0; axis < 3; axis += 1) {
          this.node3[at * 4 + axis] = this.pos3[at * 3 + axis];
          this.node2[at * 4 + axis] = this.pos2[at * 3 + axis];
        }
        this.node3[at * 4 + 3] = 1;
        this.node2[at * 4 + 3] = 1;
      }
      for (const texture of this.nodeTextures) texture.needsUpdate = true;
    },

    /* 처음 자리(시안 `computeHome`) — 닿는 거리의 구가 판에 들게. */
    computeHome() {
      const R = Math.max(this.reach + U.homePad, U.homeLeast);
      const vertical = Math.tan((THREE.MathUtils.DEG2RAD * this.camera.fov) / 2);
      const horizontal = vertical * Math.max(U.homeAspect, this.width / this.height);
      const dist = Math.max(R / horizontal, (R * U.homeTall) / vertical) * U.homeFit;
      this.home = { yaw: U.homeYaw, pitch: U.homePitch, dist, tx: 0, ty: 0, tz: R * U.homeLift };
      this.uniforms.uRef.value = dist;
      this.camera.far = Math.max(U.far, this.reach * U.farReach);
      this.camera.updateProjectionMatrix();
    },

    /* 평면 지도의 자세 — 지금 평면 카메라(`layout.viewBoxRect`)가 보는 그대로를 위에서 내려다본다. */
    flatPose() {
      const layout = this.layout;
      const box = layout.viewBoxRect;
      const s = this.scale;
      if (box === null) return { ...this.home, lift: 0 };
      const perWorld = box.scale / s;
      const dist = this.height / (2 * perWorld * Math.tan((THREE.MathUtils.DEG2RAD * this.camera.fov) / 2));
      return { yaw: 0, pitch: U.flatPitch, dist, tx: (box.x + box.wide / 2) * s, ty: U.flatHeight,
        tz: (box.y + box.tall / 2) * s, lift: 0 };
    },

    /* 판에 선다. 사람이 3D를 누른 판은 평면 자세에서 떠오르고(시안 `setMode("3d")`), 문을 연 판은 곧바로
     * 처음 자리에 선다(디자이너 답 8). */
    enter(rising) {
      this.view.classList.add("is-universe");
      /* 서는 순간의 고름·군집은 이미 본 것이다 — 우주는 처음 자리(또는 평면 자세)에서 선다. */
      this.seenSelected = knowledgeSelectedKey;
      this.seenTopic = knowledgeClusterPicked;
      this.resize();
      this.lastTouch = performance.now() - U.openRest;
      if (rising) {
        Object.assign(this, this.flatPose());
        this.camVersion += 1;
        this.flyTo({ ...(this.saved ?? this.home), lift: 1 }, U.rise);
      } else {
        Object.assign(this, this.home, { lift: 1 });
        this.camVersion += 1;
      }
      this.refreshStates();
      this.invalidate();
    },

    /* 평면으로 내려앉는다(시안 `setMode("2d")`) — 끝나면 부르는 쪽이 놓는다. 움직임을 줄인 판과 문맥을
     * 잃은 판은 곧바로. */
    fold(done) {
      if (this.folding) return;
      this.folding = true;
      this.saved = { yaw: this.yaw, pitch: this.pitch, dist: this.dist, tx: this.tx, ty: this.ty, tz: this.tz };
      if (this.lost || this.renderer === null || knowledgeMotionReduced()) {
        done();
        return;
      }
      this.flyTo(this.flatPose(), U.fold, done);
    },

    /* 비행(시안 `flyTo`). 움직임을 줄인 판은 곧바로 도착한다. */
    flyTo(to, dur = U.fly, done = null) {
      const from = this.tween.from;
      for (const key of ["yaw", "pitch", "dist", "tx", "ty", "tz", "lift"]) from[key] = this[key];
      const target = this.tween.to;
      for (const key of ["yaw", "pitch", "dist", "tx", "ty", "tz", "lift"]) target[key] = to[key] ?? this[key];
      let turn = target.yaw - from.yaw;
      while (turn > Math.PI) turn -= Math.PI * 2;
      while (turn < -Math.PI) turn += Math.PI * 2;
      target.yaw = from.yaw + turn;
      this.tween.done = done;
      if (knowledgeMotionReduced() || dur <= 0) {
        this.tween.on = false;
        this.applyTween(1);
        this.tween.done = null;
        done?.();
        this.invalidate();
        return;
      }
      this.tween.on = true;
      this.tween.t0 = performance.now();
      this.tween.dur = dur;
      this.invalidate();
    },

    applyTween(progress) {
      const eased = progress < 0.5 ? 4 * progress ** 3 : 1 - (-2 * progress + 2) ** 3 / 2;
      const from = this.tween.from;
      const to = this.tween.to;
      this.yaw = from.yaw + (to.yaw - from.yaw) * eased;
      this.pitch = from.pitch + (to.pitch - from.pitch) * eased;
      this.dist = from.dist * (to.dist / from.dist) ** eased;
      this.tx = from.tx + (to.tx - from.tx) * eased;
      this.ty = from.ty + (to.ty - from.ty) * eased;
      this.tz = from.tz + (to.tz - from.tz) * eased;
      this.lift = from.lift + (to.lift - from.lift) * eased;
      this.camVersion += 1;
    },

    /* 은하를 거의 정면에서 보는 자세(시안 `faceOn`): 법선과 지금 시선의 사이, pitch는 `face-pitch-*` 안. */
    faceOn(row, mix) {
      if (row === null || row === undefined) return { yaw: this.yaw, pitch: this.pitch };
      const cos = Math.cos(this.pitch);
      let x = row.normal[0] + mix * cos * Math.sin(this.yaw);
      let y = row.normal[1] + mix * Math.sin(this.pitch);
      let z = row.normal[2] + mix * cos * Math.cos(this.yaw);
      const length = Math.hypot(x, y, z) || 1;
      x /= length;
      y /= length;
      z /= length;
      return { yaw: Math.atan2(x, z), pitch: Math.max(U.facePitchLow, Math.min(U.facePitchHigh, Math.asin(y))) };
    },

    /* 누름(시안 `pointerup`의 고르기): 별이면 그 쪽을 고른다 — 평면 지도와 같은 고름이라 인스펙터가 카드를 열고,
     * 고름이 바뀐 것을 `dress`가 보고 그 별로 난다. 빈 곳이면 고름과 고른 군집을 놓는다(시안 `clearSelection`). */
    press() {
      if (this.folding || this.map === null) return;
      const seat = this.pickAt(this.pointer.x, this.pointer.y);
      if (seat >= 0) {
        selectKnowledgeNode(this.view, this.layout.model.keys[seat]);
        return;
      }
      if (knowledgeClusterPicked >= 0) toggleKnowledgeCluster(this.view, knowledgeClusterPicked);
      if (knowledgeSelectedKey !== null) selectKnowledgeNode(this.view, null);
    },

    /* 쪽으로 난다(시안 `select`) — 그 별에, 그 은하를 거의 정면으로. */
    flyToStar(seat) {
      const row = this.map.rowOf[seat] >= 0 ? this.map.galaxy[this.map.rowOf[seat]] : null;
      const face = this.faceOn(row, U.facePage);
      this.flyTo({ tx: this.pos3[seat * 3], ty: this.pos3[seat * 3 + 1], tz: this.pos3[seat * 3 + 2],
        dist: Math.max(U.flyPageDist, (row?.radius ?? 0) * U.flyPageReach), yaw: face.yaw, pitch: face.pitch }, U.flyPage);
    },

    /* 은하로 난다(시안 `focusTopic`). */
    flyToGalaxy(rank) {
      const row = this.map.galaxy[rank];
      if (row === undefined) return;
      const face = this.faceOn(row, U.faceGalaxy);
      this.flyTo({ tx: row.x, ty: row.y, tz: row.z, dist: row.radius * U.flyGalaxyReach + U.flyGalaxyPad,
        yaw: face.yaw, pitch: face.pitch }, U.fly);
    },

    /* 필라멘트 하나를 옆에서 보는 자리로 난다(시안 `showFlow`). */
    flyToFlow(from, to) {
      const a = this.map.galaxy[from];
      const b = this.map.galaxy[to];
      if (a === undefined || b === undefined) return;
      this.flyTo({ tx: (a.x + b.x) / 2, ty: (a.y + b.y) / 2, tz: (a.z + b.z) / 2,
        dist: Math.hypot(a.x - b.x, a.y - b.y, a.z - b.z) * U.flyFlowReach + U.flyFlowPad,
        yaw: Math.atan2(b.z - a.z, -(b.x - a.x)) + Math.PI, pitch: U.flyFlowPitch }, U.flyFlow);
    },

    /* 별 하나의 화면 크기(CSS px, 시안 `starSizeCss`) — 셰이더와 같은 표(`KNOWLEDGE_UNIVERSE_STAR`). */
    starSize(seat, depth) {
      const S = KNOWLEDGE_UNIVERSE_STAR;
      const lum = this.starGeometry.attributes.aStar.array[seat * 4];
      const flux = lum * Math.max(S.fluxLeast, Math.min(S.fluxMost, (this.uniforms.uRef.value / Math.max(depth, 1)) ** 2));
      return Math.max(S.sizeLeast, Math.min(S.sizeMost, S.sizeScale * flux ** S.sizePower));
    },

    /* 이름 있는 별을 화면에 투영한다(시안 `projectAll`) — 카메라가 움직인 뒤에만. 칸마다 x, y, 반지름, 깊이(뒤는 -1). */
    project(seat = -1) {
      if (this.projVersion !== this.camVersion) {
        this.placeCamera();
        this.projVersion = this.camVersion;
        const matrix = this.viewProjection.multiplyMatrices(this.camera.projectionMatrix, this.camera.matrixWorldInverse)
          .elements;
        const { pos2, pos3, proj, lift, width, height } = this;
        const count = this.layout.count;
        for (let at = 0; at < count; at += 1) {
          const x = pos2[at * 3] + (pos3[at * 3] - pos2[at * 3]) * lift;
          const y = pos2[at * 3 + 1] + (pos3[at * 3 + 1] - pos2[at * 3 + 1]) * lift;
          const z = pos2[at * 3 + 2] + (pos3[at * 3 + 2] - pos2[at * 3 + 2]) * lift;
          const depth = matrix[3] * x + matrix[7] * y + matrix[11] * z + matrix[15];
          if (depth <= KNOWLEDGE_UNIVERSE_SHAPE.behind) {
            proj[at * 4 + 3] = -1;
            continue;
          }
          proj[at * 4] = ((matrix[0] * x + matrix[4] * y + matrix[8] * z + matrix[12]) / depth * 0.5 + 0.5) * width;
          proj[at * 4 + 1] = (1 - ((matrix[1] * x + matrix[5] * y + matrix[9] * z + matrix[13]) / depth * 0.5 + 0.5)) * height;
          proj[at * 4 + 2] = this.starSize(at, depth) * 0.5;
          proj[at * 4 + 3] = depth;
        }
      }
      if (seat < 0) return null;
      return { x: this.proj[seat * 4], y: this.proj[seat * 4 + 1], r: this.proj[seat * 4 + 2], depth: this.proj[seat * 4 + 3] };
    },

    /* 포인터 아래의 별(시안 `pickAt`) — 맞힘 반지름 안에서 가깝고 앞선 것. */
    pickAt(x, y) {
      if (this.map === null || this.folding) return -1;
      const S = KNOWLEDGE_UNIVERSE_STAR;
      this.project();
      const proj = this.proj;
      let best = -1;
      let score = Number.POSITIVE_INFINITY;
      for (let at = 0; at < this.layout.count; at += 1) {
        const depth = proj[at * 4 + 3];
        if (depth < 0) continue;
        const dx = proj[at * 4] - x;
        const dy = proj[at * 4 + 1] - y;
        const reach = Math.max(proj[at * 4 + 2] * S.pickShare, S.pickLeast);
        const gap = dx * dx + dy * dy;
        if (gap >= reach * reach) continue;
        const mine = gap / (reach * reach) + depth * S.pickDepth;
        if (mine < score) {
          score = mine;
          best = at;
        }
      }
      return best;
    },

    /* 올림은 한 프레임에 한 번(시안 `schedulePick`). */
    schedulePick() {
      if (this.pickQueued) return;
      this.pickQueued = true;
      requestAnimationFrame(() => {
        this.pickQueued = false;
        if (this.renderer === null || !this.pointer.inside || this.down !== null) return;
        this.setHover(this.pickAt(this.pointer.x, this.pointer.y));
      });
    },

    setHover(seat) {
      if (seat === this.hover) return;
      this.hover = seat;
      this.host?.classList.toggle("is-pointing", seat >= 0);
      this.refreshStates();
      this.showTip(seat);
    },

    /* 올린 별의 팁(시안 `showTip`): 제목, 은하, 연결(선언·추론), 고친 때. 포인터의 오른쪽 위에 서고 판 끝에서는 안으로
     * 든다. 쪽 이름은 그 상자를 비켜 선다(`placeLabels`). */
    showTip(seat) {
      const tip = this.tip;
      if (tip === null) return;
      if (seat < 0 || this.dragging || this.map === null) {
        tip.classList.remove("is-on");
        this.tipBox = null;
        return;
      }
      const layout = this.layout;
      const model = layout.model;
      const row = this.map.rowOf[seat];
      const hue = row >= 0 ? this.map.galaxy[row].hue : -1;
      tip.style.setProperty("--knowledge-universe-ink", hue >= 0 ? `var(--knowledge-hue-${hue})` : "var(--ink-mist)");
      const words = { galaxy: row >= 0 ? knowledgeClusterWord(layout, row) : t("knowledge.rogueStar", "떠돌이 별"),
        links: model.degree[seat], declared: this.declared[seat], inferred: this.inferred[seat] };
      const modified = model.modified[seat];
      writeTextContent(tip.firstChild, model.titles[seat]);
      writeTextContent(tip.lastChild, modified > 0
        ? t("knowledge.starTipAgo", "{{galaxy}} · 연결 {{links}} (선언 {{declared}} · 추론 {{inferred}}) · 고친 때 {{ago}}",
          { ...words, ago: knowledgeUniverseAgo(modified, model.nowMs) })
        : t("knowledge.starTip", "{{galaxy}} · 연결 {{links}} (선언 {{declared}} · 추론 {{inferred}})", words));
      const x = Math.max(0, Math.min(this.width - U.tipRoom, this.pointer.x + U.tipRight));
      const y = Math.max(U.tipTop, this.pointer.y - U.tipUp);
      tip.style.transform = `translate3d(${Math.round(x)}px,${Math.round(y)}px,0)`;
      tip.classList.add("is-on");
      this.tipBox = [x, y, tip.offsetWidth, tip.offsetHeight];
    },

    /* 이름표를 짓는다(시안 `buildTopicCards`) — 은하마다 명판(누르면 그 은하를 고른다), 쪽 이름의 풀, 굵은
     * 필라멘트의 수, 팁. 우주의 그림을 새로 지을 때마다. */
    buildLabels() {
      const layer = this.labels;
      const layout = this.layout;
      const map = this.map;
      layer.textContent = "";
      this.plates = [];
      for (let row = 0; row < map.galaxies; row += 1) {
        const plate = document.createElement("button");
        plate.type = "button";
        plate.className = row < U.plateLarge ? "knowledge-universe-plate" : "knowledge-universe-plate is-minor";
        plate.dataset.knowledgeCluster = String(row);
        plate.tabIndex = -1;
        plate.setAttribute("aria-pressed", "false");
        const hue = map.galaxy[row].hue;
        plate.style.setProperty("--knowledge-universe-ink", hue >= 0 ? `var(--knowledge-hue-${hue})` : "var(--ink-mist)");
        const top = document.createElement("span");
        top.className = "knowledge-universe-plate-top";
        const name = document.createElement("span");
        name.className = "knowledge-universe-plate-name";
        name.textContent = knowledgeClusterWord(layout, row);
        const count = document.createElement("span");
        count.className = "knowledge-universe-plate-count";
        count.textContent = String(layout.communitySize[row]);
        top.append(name, count);
        const flow = document.createElement("span");
        flow.className = "knowledge-universe-plate-flow";
        const mark = document.createElement("i");
        mark.className = "knowledge-universe-plate-mark";
        const said = document.createElement("span");
        const best = knowledgeUniverseStrongest(map, row);
        if (best === null) {
          said.dataset.i18n = "knowledge.flowInside";
          said.textContent = t("knowledge.flowInside", "안에서 이어짐");
        } else {
          const other = document.createElement("em");
          other.textContent = knowledgeClusterWord(layout, best.other);
          said.append(`${best.out ? "→" : "←"} `, other, ` ${best.n}`);
        }
        flow.append(mark, said);
        plate.append(top, flow);
        plate.addEventListener("click", (event) => {
          event.stopPropagation();
          toggleKnowledgeCluster(this.view, row);
        });
        layer.appendChild(plate);
        this.plates.push({ element: plate, row, wide: 0, tall: 0, fullWide: 0, fullTall: 0, full: false, on: false,
          dim: false, pressed: false, x: -1, y: -1 });
      }
      this.names = [];
      for (let slot = 0; slot < U.labels; slot += 1) {
        const name = document.createElement("div");
        name.className = "knowledge-universe-name";
        name.setAttribute("aria-hidden", "true");
        layer.appendChild(name);
        this.names.push({ element: name, seat: -1, on: false, hot: false, x: -1, y: -1 });
      }
      this.flowTags = map.filaments.slice(0, U.flowTags).map((one) => {
        const tag = document.createElement("div");
        tag.className = "knowledge-universe-flow";
        tag.setAttribute("aria-hidden", "true");
        const arrow = document.createElementNS(SVG_NS, "svg");
        arrow.setAttribute("viewBox", "0 0 14 8");
        arrow.setAttribute("class", "knowledge-universe-flow-arrow");
        const path = document.createElementNS(SVG_NS, "path");
        path.setAttribute("d", "M1 4h11M9 1l3 3-3 3");
        arrow.appendChild(path);
        const count = document.createElement("span");
        count.textContent = String(one.n);
        tag.append(arrow, count);
        layer.appendChild(tag);
        const from = map.galaxy[one.from];
        const to = map.galaxy[one.to];
        /* 설 자리 셋 — 가운데, 그 앞, 그 뒤. 화살은 자리마다 흐름 쪽으로 조금 앞선 점을 가리킨다. */
        const lead = U.flowTagAhead - U.flowTagAt;
        const spots = [U.flowTagAt, U.flowTagAt - U.flowTagShift, U.flowTagAt + U.flowTagShift].map((at) => ({
          mid: knowledgeUniverseFilamentPoint(from, to, one.bend, at),
          ahead: knowledgeUniverseFilamentPoint(from, to, one.bend, at + lead) }));
        return { element: tag, arrow, wide: 0, on: false, x: -1, y: -1, turn: Number.NaN, spots };
      });
      const tip = document.createElement("div");
      tip.className = "knowledge-universe-tip";
      tip.setAttribute("aria-hidden", "true");
      const title = document.createElement("span");
      title.className = "knowledge-universe-tip-title";
      const note = document.createElement("span");
      note.className = "knowledge-universe-tip-note";
      tip.append(title, note);
      layer.appendChild(tip);
      this.tip = tip;
      this.tipBox = null;
      const galaxies = map.galaxies;
      this.anchorX = new Float64Array(galaxies);
      this.anchorLow = new Float64Array(galaxies);
      this.anchorHigh = new Float64Array(galaxies);
      this.anchorSeen = new Uint8Array(galaxies);
      this.order = new Int32Array(galaxies);
      this.candidates = new Int32Array(U.labelsCandidates);
      this.candidateScores = new Float64Array(U.labelsCandidates);
      this.boxes = new Float64Array((KNOWLEDGE_UNIVERSE_COVERS.length + galaxies + this.flowTags.length + this.names.length + 1)
        * 4);
      this.nameWidths = new Float32Array(layout.count);
      this.labelVersion = -1;
      this.measured = false;
      this.measureLabels();
    },

    /* 명판의 두 크기(보통·크게)와 필라멘트 수의 넓이, 쪽 이름의 글꼴과 여백 — 판이 보일 때 한 번. 쪽 이름의 넓이는
     * 시안의 어림(글자 수 × 10.6 + 14) 대신 글꼴로 잰다: 한글은 어림보다 넓고 로마자는 좁아, 어림이면 겹치거나 빈다. */
    measureLabels() {
      if (this.host === null || this.host.clientWidth === 0) return false;
      for (const plate of this.plates) {
        const element = plate.element;
        element.classList.remove("is-full");
        let box = element.getBoundingClientRect();
        plate.wide = box.width;
        plate.tall = box.height;
        element.classList.add("is-full");
        box = element.getBoundingClientRect();
        plate.fullWide = box.width;
        plate.fullTall = box.height;
        element.classList.toggle("is-full", plate.full);
      }
      for (const tag of this.flowTags) tag.wide = tag.element.getBoundingClientRect().width;
      const probe = this.names[0]?.element ?? null;
      if (probe !== null) {
        const style = getComputedStyle(probe);
        const pad = (one) => Number.parseFloat(one.paddingLeft) + Number.parseFloat(one.paddingRight)
          + Number.parseFloat(one.borderLeftWidth) + Number.parseFloat(one.borderRightWidth);
        this.nameFont = `${style.fontStyle} ${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
        this.namePad = pad(style);
        probe.classList.add("is-hot");
        this.nameHotPad = pad(getComputedStyle(probe));
        probe.classList.remove("is-hot");
      }
      this.measured = true;
      return true;
    },

    /* 쪽 이름의 글 — 평면처럼 `label-max`에서 말줄임으로 자른다(`knowledgeShortWord`). 온 제목은 팁·카드·인스펙터가 든다. */
    nameWord(seat) {
      return knowledgeShortWord(this.layout.model.titles[seat], this.layout.tuning.labelMax);
    },

    /* 쪽 이름 하나의 넓이(CSS px) — 자른 글로, 쪽마다 한 번 잰다. 초점의 이름은 테두리만큼 넓다. */
    nameWide(seat, hot) {
      let wide = this.nameWidths[seat];
      if (wide === 0) {
        this.nameContext ??= document.createElement("canvas").getContext("2d");
        this.nameContext.font = this.nameFont;
        wide = Math.ceil(this.nameContext.measureText(this.nameWord(seat)).width);
        this.nameWidths[seat] = wide;
      }
      return wide + (hot ? this.nameHotPad : this.namePad);
    },

    pushBox(x, y, wide, tall) {
      if ((this.boxCount + 1) * 4 > this.boxes.length) return;
      const at = this.boxCount * 4;
      this.boxes[at] = x;
      this.boxes[at + 1] = y;
      this.boxes[at + 2] = wide;
      this.boxes[at + 3] = tall;
      this.boxCount += 1;
    },

    hit(x, y, wide, tall) {
      const boxes = this.boxes;
      for (let at = 0; at < this.boxCount * 4; at += 4) {
        if (x < boxes[at] + boxes[at + 2] && x + wide > boxes[at] && y < boxes[at + 1] + boxes[at + 3]
          && y + tall > boxes[at + 1]) return true;
      }
      return false;
    },

    /* 무대 위에 늘 서 있는 상자(배율 상자·열린 범례)를 먼저 놓는다 — 프레임의 처음이라 레이아웃은 깨끗하다. */
    readCovers() {
      this.boxCount = 0;
      const host = this.host.getBoundingClientRect();
      for (const selector of KNOWLEDGE_UNIVERSE_COVERS) {
        const element = this.view.querySelector(selector);
        if (element === null || element.hidden) continue;
        const box = element.getBoundingClientRect();
        if (box.width === 0 || box.height === 0) continue;
        this.pushBox(box.left - host.left, box.top - host.top, box.width, box.height);
      }
    },

    /* 세계의 한 점을 화면으로(`project`가 세운 행렬) — 카메라 뒤면 거짓. 답은 `spot`(x, y, 깊이). */
    screenAt(x, y, z) {
      const m = this.viewProjection.elements;
      const depth = m[3] * x + m[7] * y + m[11] * z + m[15];
      if (depth <= KNOWLEDGE_UNIVERSE_SHAPE.behind) return false;
      this.spot[0] = ((m[0] * x + m[4] * y + m[8] * z + m[12]) / depth * 0.5 + 0.5) * this.width;
      this.spot[1] = (1 - ((m[1] * x + m[5] * y + m[9] * z + m[13]) / depth * 0.5 + 0.5)) * this.height;
      this.spot[2] = depth;
      return true;
    },

    /* 은하 가장자리 고리의 점들을 투영해 화면에서 가장 낮은 곳(명판 자리)과 가장 높은 곳(비켜 설 자리)을 찾는다
     * (시안 `galaxyAnchor`). 고리의 점이 충분히 보이고 가운데가 앞에 있어야 명판이 선다. */
    anchor(row) {
      const k = this.map.galaxy[row];
      const ring = k.radius * U.plateRing;
      let low = Number.NEGATIVE_INFINITY;
      let high = Number.POSITIVE_INFINITY;
      let seen = 0;
      for (let step = 0; step < U.platePoints; step += 1) {
        const angle = (step / U.platePoints) * Math.PI * 2;
        const c = Math.cos(angle) * ring;
        const s = Math.sin(angle) * ring;
        if (!this.screenAt(k.x + k.across[0] * c + k.along[0] * s, k.y + k.across[1] * c + k.along[1] * s,
          k.z + k.across[2] * c + k.along[2] * s)) continue;
        seen += 1;
        low = Math.max(low, this.spot[1]);
        high = Math.min(high, this.spot[1]);
      }
      const front = this.screenAt(k.x, k.y, k.z);
      this.anchorX[row] = this.spot[0];
      this.anchorLow[row] = low;
      this.anchorHigh[row] = high;
      this.anchorSeen[row] = front && seen >= U.plateSeen ? 1 : 0;
    },

    /* 자리는 `transform` 하나 — 온 픽셀이 바뀐 때만 쓴다. */
    moveLabel(label, x, y) {
      const qx = Math.round(x);
      const qy = Math.round(y);
      if (qx === label.x && qy === label.y) return;
      label.element.style.transform = `translate3d(${qx}px,${qy}px,0)`;
      label.x = qx;
      label.y = qy;
    },

    showLabel(label, on) {
      if (on === label.on) return;
      label.on = on;
      label.element.classList.toggle("is-on", on);
      /* 선 명판은 키보드로도 닿는다(단추) — 숨은 명판은 탭 차례에서 빠진다. */
      if (label.element.tagName === "BUTTON") label.element.tabIndex = on ? 0 : -1;
    },

    hideLabels() {
      if (this.labelVersion === -1) return;
      /* 숨은 명판은 탭 차례에서도 빠진다(`showLabel`). */
      this.tip?.classList.remove("is-on");
      this.tipBox = null;
      for (const plate of this.plates) this.showLabel(plate, false);
      for (const name of this.names) this.showLabel(name, false);
      for (const tag of this.flowTags) this.showLabel(tag, false);
      this.labelVersion = -1;
    },

    /* 이름표를 놓는다(시안 `updateLabels`) — 카메라나 초점이 바뀐 프레임에만. 차례: 무대의 상자 → 은하 명판(고른 것
     * 먼저, 그다음 순위) → 필라멘트 수(멀리서 쉴 때만) → 팁의 상자 → 쪽 이름(쉴 때는 연결이 가장 많은 별들,
     * 가까이 가거나 초점이 있으면 화면에서 크게 보이는 별부터). 먼저 놓인 것을 뒤의 것이 비켜 선다. */
    placeLabels() {
      if (this.labelVersion === this.camVersion || this.map === null) return;
      if (!this.measured && !this.measureLabels()) return;
      this.labelVersion = this.camVersion;
      const layout = this.layout;
      const model = layout.model;
      const map = this.map;
      const wide = this.width;
      const tall = this.height;
      this.readCovers();
      this.project();
      const picked = knowledgeClusterPicked >= 0 && knowledgeClusterPicked < map.rows ? knowledgeClusterPicked : -1;
      const selected = this.selectedSeat;
      const focusSeat = this.hover >= 0 ? this.hover : selected;
      const near = picked >= 0 ? picked : selected >= 0 ? map.rowOf[selected] : -1;
      const flown = near >= 0 ? (map.galaxy[near].radius * U.flyGalaxyReach + U.flyGalaxyPad) * U.closeInFly : 0;
      const closeIn = this.dist < Math.max(this.home.dist * U.closeIn, flown);
      /* 은하 명판 — 고른 은하가 먼저, 그다음 순위. 고른(또는 고른 쪽의) 은하는 크게 선다. */
      const galaxies = map.galaxies;
      const order = this.order;
      for (let row = 0; row < galaxies; row += 1) {
        this.anchor(row);
        order[row] = row;
      }
      if (picked >= 0 && picked < galaxies) {
        order.copyWithin(1, 0, picked);
        order[0] = picked;
      }
      const keep = picked >= 0 ? picked : selected >= 0 ? map.rowOf[selected] : -1;
      for (let at = 0; at < galaxies; at += 1) {
        const row = order[at];
        const plate = this.plates[row];
        const full = row === keep;
        if (full !== plate.full) {
          plate.full = full;
          plate.element.classList.toggle("is-full", full);
        }
        const plateWide = full ? plate.fullWide : plate.wide;
        const plateTall = full ? plate.fullTall : plate.tall;
        const x = this.anchorX[row] - plateWide / 2;
        let y = this.anchorLow[row] + U.plateGap;
        let on = this.anchorSeen[row] === 1 && x > U.plateEdge && x < wide - plateWide - U.plateEdge && y > U.plateTop
          && y < tall - plateTall - U.plateBottom && (!closeIn || row === keep);
        if (on && this.hit(x, y, plateWide, plateTall)) {
          const above = this.anchorHigh[row] - plateTall - U.plateGap;
          if (!this.hit(x, above, plateWide, plateTall) && above > U.plateTop) y = above;
          else on = false;
        }
        if (on) {
          this.pushBox(x - U.plateRoom, y - U.plateRoom, plateWide + U.plateRoom * 2, plateTall + U.plateRoom * 2);
          this.moveLabel(plate, x, y);
        }
        this.showLabel(plate, on);
        const dim = (picked >= 0 && row !== picked) || (selected >= 0 && map.rowOf[selected] !== row);
        if (dim !== plate.dim) {
          plate.dim = dim;
          plate.element.classList.toggle("is-dim", dim);
        }
        const pressed = row === picked;
        if (pressed !== plate.pressed) {
          plate.pressed = pressed;
          plate.element.setAttribute("aria-pressed", String(pressed));
        }
      }
      /* 굵은 필라멘트의 선 수와 흐름 방향(보낸 은하 → 받는 은하) — 멀리서, 아무 초점도 없이 쉴 때만. */
      const flowsShown = !closeIn && this.hover < 0 && selected < 0 && picked < 0 && this.hits === 0;
      for (const tag of this.flowTags) {
        let on = false;
        for (const spot of flowsShown ? tag.spots : []) {
          if (!this.screenAt(spot.mid[0], spot.mid[1], spot.mid[2])) continue;
          const mx = this.spot[0];
          const my = this.spot[1];
          const turn = this.screenAt(spot.ahead[0], spot.ahead[1], spot.ahead[2])
            ? Math.atan2(this.spot[1] - my, this.spot[0] - mx) : 0;
          const x = mx - tag.wide / 2;
          const y = my - U.flowTagTall / 2;
          if (x <= U.plateEdge || x >= wide - tag.wide - U.plateEdge || y <= U.labelTop || y >= tall - U.labelBottom
            || this.hit(x, y, tag.wide, U.flowTagTall)) continue;
          on = true;
          this.pushBox(x - U.flowTagRoom, y - U.flowTagRoom, tag.wide + U.flowTagRoom * 2, U.flowTagTall + U.flowTagRoom * 2);
          this.moveLabel(tag, x, y);
          const step = Math.round(turn * U.flowTagTurn) / U.flowTagTurn;
          if (step !== tag.turn) {
            tag.arrow.style.transform = `rotate(${step}rad)`;
            tag.turn = step;
          }
          break;
        }
        this.showLabel(tag, on);
      }
      /* 쪽 이름 — 가까이 왔거나 초점이 있으면 화면에서 밝게(크게) 보이는 별부터, 쉬는 먼 자리에서는 가장 밝은 별들. */
      const proj = this.proj;
      const state = this.starState;
      let candidates = this.candidates;
      const scores = this.candidateScores;
      const room = candidates.length;
      let found = 0;
      let most = this.names.length;
      /* 고른 은하 가까이, 쪽 초점도 찾기도 없으면(디자이너 m-12670): 후보는 그 은하의 쪽 전부이고 이름은
       * `labels-close`개까지 — 연결이 많은 차례(`members`)가 곧 화면에서 큰 차례에 가깝다(크기 = 광도 × 깊이). 화면
       * 크기 상위 40만 보면 핵 둘레에 몰린 허브들이 서로를 막아 다섯만 섰다. */
      const galaxyClose = closeIn && picked >= 0 && focusSeat < 0 && this.hits === 0;
      if (galaxyClose) {
        candidates = map.members[picked];
        found = candidates.length;
        most = Math.min(most, U.labelsClose);
      } else if (closeIn || focusSeat >= 0 || this.hits > 0) {
        for (let seat = 0; seat < layout.count; seat += 1) {
          if (proj[seat * 4 + 3] < 0) continue;
          const sx = proj[seat * 4];
          const sy = proj[seat * 4 + 1];
          if (sx < 0 || sx > wide || sy < U.labelScan || sy > tall - U.labelScan) continue;
          let score = proj[seat * 4 + 2];
          if (seat === focusSeat) {
            if (seat === this.hover) continue;
            score = KNOWLEDGE_UNIVERSE_LABEL_FIRST.focus;
          } else if (state[seat] >= 1 && focusSeat >= 0) {
            score += U.labelNeighbour;
          } else if (state[seat] === 3) {
            score += U.labelHit;
          } else if (!closeIn) {
            continue;
          } else if (picked >= 0 && map.rowOf[seat] !== picked) {
            continue;
          }
          if (score < U.labelLeast) continue;
          if (found < room) {
            candidates[found] = seat;
            scores[found] = score;
            found += 1;
          } else {
            let least = 0;
            for (let at = 1; at < room; at += 1) if (scores[at] < scores[least]) least = at;
            if (score > scores[least]) {
              candidates[least] = seat;
              scores[least] = score;
            }
          }
        }
        for (let at = 1; at < found; at += 1) {
          const seat = candidates[at];
          const score = scores[at];
          let back = at - 1;
          while (back >= 0 && scores[back] < score) {
            candidates[back + 1] = candidates[back];
            scores[back + 1] = scores[back];
            back -= 1;
          }
          candidates[back + 1] = seat;
          scores[back + 1] = score;
        }
      } else if (picked < 0) {
        for (const seat of this.bright) {
          if (found >= room) break;
          candidates[found] = seat;
          scores[found] = KNOWLEDGE_UNIVERSE_LABEL_FIRST.bright - found;
          found += 1;
        }
      }
      if (this.hover >= 0 && this.tipBox !== null) {
        const [x, y, tipWide, tipTall] = this.tipBox;
        this.pushBox(x - U.tipGap, y - U.tipGap, tipWide + U.tipGap * 2, tipTall + U.tipGap * 2);
      }
      let used = 0;
      for (let at = 0; at < found && used < most; at += 1) {
        const seat = candidates[at];
        if (proj[seat * 4 + 3] < 0) continue;
        if (galaxyClose && (proj[seat * 4] < 0 || proj[seat * 4] > wide || proj[seat * 4 + 1] < U.labelScan
          || proj[seat * 4 + 1] > tall - U.labelScan || proj[seat * 4 + 2] < U.labelLeast)) continue;
        const hot = seat === focusSeat;
        const nameWide = this.nameWide(seat, hot);
        const off = Math.max(U.labelOffLeast, proj[seat * 4 + 2] * U.labelOff) + U.labelOffPad;
        let x = proj[seat * 4] + off;
        const y = proj[seat * 4 + 1] - U.labelTall / 2;
        if (y < U.labelTop || y > tall - U.labelBottom) continue;
        if (x + nameWide > wide - U.labelEdge) x = proj[seat * 4] - off - nameWide;
        if (x < U.labelEdge) continue;
        if (!hot && this.hit(x, y, nameWide, U.labelTall)) continue;
        this.pushBox(x, y, nameWide, U.labelTall);
        const name = this.names[used];
        used += 1;
        if (name.seat !== seat) {
          name.element.textContent = this.nameWord(seat);
          name.seat = seat;
        }
        this.moveLabel(name, x, y);
        this.showLabel(name, true);
        if (hot !== name.hot) {
          name.hot = hot;
          name.element.classList.toggle("is-hot", hot);
        }
      }
      for (let at = used; at < this.names.length; at += 1) this.showLabel(this.names[at], false);
    },

    /* 초점(시안 `refreshStates`): 별마다 상태(0 보통 · 1 이웃/고른 주제/시간 창 안 · 2 올림/고름/경로 · 3 찾은 것),
     * 초점 은하, 초점 쪽(과 경로)의 선만. 상태는 평면 지도와 같은 자리(고른 쪽·고른 군집·찾기·시간 창·경로)에서
     * 읽는다 — 올림만 우주의 것이다. */
    refreshStates() {
      if (this.map === null || this.starState === null) return;
      const layout = this.layout;
      const model = layout.model;
      const map = this.map;
      const state = this.starState;
      state.fill(0);
      let focus = 0;
      if (knowledgeSlicerCutoff > 0) {
        for (let at = 0; at < layout.count; at += 1) if (layout.sliceMatch[at] === 1) state[at] = 1;
        focus = 1;
      }
      const topic = knowledgeClusterPicked >= 0 && knowledgeClusterPicked < map.rows ? knowledgeClusterPicked : -1;
      if (topic >= 0) {
        for (const at of map.members[topic]) state[at] = 1;
        focus = 1;
      }
      if (layout.pathShown) {
        for (let at = 0; at < layout.count; at += 1) if (layout.pathNode[at] === 1) state[at] = 2;
        focus = 1;
      }
      this.hits = 0;
      if (knowledgeQuery.trim() !== "") {
        for (let at = 0; at < layout.count; at += 1) {
          if (layout.searchMatch[at] !== 1) continue;
          state[at] = 3;
          this.hits += 1;
        }
        focus = 1;
      }
      const selected = knowledgeSelectedKey === null ? -1 : model.keys.indexOf(knowledgeSelectedKey);
      this.selectedSeat = selected;
      const focusSeat = this.hover >= 0 ? this.hover : selected;
      if (focusSeat >= 0) {
        if (topic >= 0) for (const at of map.members[topic]) if (state[at] === 1) state[at] = 0;
        for (let slot = model.start[focusSeat]; slot < model.start[focusSeat + 1]; slot += 1) {
          const at = model.neighbour[slot];
          state[at] = Math.max(state[at], 1);
        }
        state[focusSeat] = 2;
        if (selected >= 0 && selected !== focusSeat) state[selected] = 2;
        focus = 1;
      }
      this.uniforms.uFocus.value = focus;
      const galaxy = topic >= 0 ? topic : focusSeat >= 0 ? map.rowOf[focusSeat] : -1;
      this.uniforms.uFocusGal.value = galaxy >= 0 ? galaxy : focusSeat >= 0 ? KNOWLEDGE_UNIVERSE_SHAPE.noGalaxy : -1;
      const room = this.focusEdges.length / 4;
      let lines = 0;
      const put = (edge) => {
        if (lines >= room) return;
        for (let slot = 0; slot < 4; slot += 1) this.focusEdges[lines * 4 + slot] = this.edgeData[edge * 4 + slot];
        lines += 1;
      };
      if (focusSeat >= 0) {
        for (let slot = model.start[focusSeat]; slot < model.start[focusSeat + 1]; slot += 1) put(model.throughEdge[slot]);
      }
      if (layout.pathShown) {
        for (let edge = 0; edge < model.edgeCount; edge += 1) if (layout.pathEdge[edge] === 1) put(edge);
      }
      this.lineGeometry.instanceCount = lines;
      this.lineObject.visible = lines > 0;
      this.focusAttr.needsUpdate = true;
      this.starStateAttr.needsUpdate = true;
      this.camVersion += 1;
      this.invalidate();
    },

    goHome() {
      if (this.folding || this.home === null) return;
      this.lastTouch = performance.now() - U.homeRest;
      this.flyTo(this.home, U.homeMs);
    },

    zoomStep(step) {
      if (this.folding || this.home === null) return;
      this.lastTouch = performance.now();
      this.flyTo({ dist: Math.max(U.zoomMin, Math.min(this.home.dist * U.zoomMax, this.dist * step)) }, U.zoomStepMs);
    },

    turn(yaw, pitch) {
      if (this.folding) return;
      this.lastTouch = performance.now();
      this.tween.on = false;
      this.yaw += yaw;
      this.pitch = this.clampPitch(this.pitch + pitch);
      this.camVersion += 1;
      this.invalidate();
    },

    /* 옷이 바뀌었다 — 평면의 앉기가 끝났으면 자리를 한 번 옮기고, 다음 한 장을 그린다. */
    dress() {
      const layout = this.layout;
      if (layout !== null && layout.left === 0 && this.seatsRevision !== layout.geometryRevision && !this.tween.on) {
        this.placeSeats();
      }
      if (this.map !== null && !this.folding) {
        /* 고름과 군집이 바뀌면 그리로 난다 — 평면의 인스펙터·목록·찾기가 고른 것도 같다(시안 `select`·`focusTopic`). */
        const selected = knowledgeSelectedKey;
        if (selected !== this.seenSelected) {
          this.seenSelected = selected;
          const seat = selected === null ? -1 : layout.model.keys.indexOf(selected);
          if (seat >= 0) this.flyToStar(seat);
        }
        if (knowledgeClusterPicked !== this.seenTopic) {
          this.seenTopic = knowledgeClusterPicked;
          if (knowledgeClusterPicked >= 0 && selected === null) this.flyToGalaxy(knowledgeClusterPicked);
        }
        this.refreshStates();
      }
      this.invalidate();
    },

    /* 이 우주가 사람 앞에 서 있는가 — 가려진 판·숨은 문서·잃은 문맥에서는 그리지 않는다. */
    showing() {
      return this.renderer !== null && !this.lost && this.view.isConnected && !this.view.hidden && !document.hidden
        && this.host?.isConnected === true;
    },

    invalidate() {
      if (this.raf !== 0 || !this.showing()) return;
      this.raf = requestAnimationFrame((now) => this.frame(now));
    },

    halt() {
      if (this.raf !== 0) cancelAnimationFrame(this.raf);
      this.raf = 0;
      this.lastNow = 0;
    },

    moving() {
      return this.motion && !knowledgeMotionReduced();
    },

    /* 「움직임」을 켜고 끈다(시안 `setMotion`) — 켜면 이미 쉰 것으로 쳐서(`wake-rest`) 곧 돈다. */
    setMotion(on) {
      this.motion = on;
      if (on) this.lastTouch = performance.now() - U.wakeRest;
      this.invalidate();
    },

    /* 한 장(시안 `frame`). 비행·미끄러짐·움직임 중에만 다음 장을 청한다 — 쉬면 멈춘다. */
    frame(now) {
      this.raf = 0;
      if (!this.showing()) {
        this.lastNow = 0;
        return;
      }
      const dt = this.lastNow ? Math.min(U.frameMost, (now - this.lastNow) / 1000) : U.frameFirst;
      this.lastNow = now;
      let busy = false;
      if (this.tween.on) {
        const progress = Math.min(1, (performance.now() - this.tween.t0) / this.tween.dur);
        this.applyTween(progress);
        if (progress >= 1) {
          this.tween.on = false;
          const done = this.tween.done;
          this.tween.done = null;
          done?.();
          if (this.renderer === null) return;
        }
        busy = true;
      }
      if (this.glide.on && !this.dragging) {
        this.yaw -= this.glide.vx * dt;
        this.pitch = this.clampPitch(this.pitch + this.glide.vy * dt);
        this.glide.vx *= Math.exp(-dt * U.glideDecay);
        this.glide.vy *= Math.exp(-dt * U.glideDecay);
        if (Math.abs(this.glide.vx) + Math.abs(this.glide.vy) < U.glideStop) this.glide.on = false;
        this.camVersion += 1;
        busy = true;
      }
      if (this.moving() && !this.folding) {
        this.time += dt;
        const resting = !this.tween.on && !this.dragging && knowledgeSelectedKey === null
          && knowledgeClusterPicked < 0 && now - this.lastTouch > U.idleDelay;
        if (resting) {
          this.spin = Math.min(1, this.spin + dt * U.idleRamp);
          this.yaw += dt * U.idleSpin * this.spin;
          this.camVersion += 1;
        } else {
          this.spin = 0;
        }
        busy = true;
      } else if (this.hits > 0 && !knowledgeMotionReduced() && !this.folding) {
        /* 찾은 별의 조준 고리는 움직임을 끈 판에서도 깜박인다 — 움직임을 줄이라는 판에서만 멈춘다(시안 `frame`). */
        this.time += dt;
        busy = true;
      }
      this.uniforms.uTime.value = this.time;
      this.uniforms.uFlow.value = this.moving() ? 1 : 0;
      this.uniforms.uLift.value = this.lift;
      this.placeCamera();
      this.render();
      /* 이름표는 우주가 다 떠오른 판에서만(시안 `frame`: 들림 > 0.98) — 떠오르고 내려앉는 동안은 숨는다. */
      if (this.lift > U.labelLift) this.placeLabels();
      else this.hideLabels();
      this.paintZoom();
      this.frames += 1;
      if (busy) this.invalidate();
      else this.lastNow = 0;
    },

    placeCamera() {
      const cos = Math.cos(this.pitch);
      this.camera.position.set(this.tx + this.dist * cos * Math.sin(this.yaw), this.ty + this.dist * Math.sin(this.pitch),
        this.tz + this.dist * cos * Math.cos(this.yaw));
      this.camera.lookAt(this.tx, this.ty, this.tz);
      this.camera.updateMatrixWorld();
      this.uniforms.uPx.value = (this.height * this.renderer.getPixelRatio())
        / (2 * Math.tan((THREE.MathUtils.DEG2RAD * this.camera.fov) / 2));
      this.uniforms.uDpr.value = this.renderer.getPixelRatio();
    },

    /* 한 장(시안 `renderFrame`): 장면을 HDR 목표에 그리고, 빛 번짐 사다리를 내렸다가 올리고, 합성이 톤 매핑과
     * 테마(밝은 테마는 음화 인쇄)를 거쳐 캔버스에 쓴다. */
    render() {
      const renderer = this.renderer;
      const post = this.post;
      if (post.target === null) return;
      renderer.info.reset();
      renderer.setRenderTarget(post.target);
      renderer.setClearColor(0x000000, 1);
      renderer.clear(true, false, false);
      renderer.render(this.scene, this.camera);
      let source = post.target;
      post.quad.material = post.down;
      for (let level = 0; level < post.mips.length; level += 1) {
        const into = post.mips[level];
        post.down.uniforms.tSrc.value = source.texture;
        post.down.uniforms.uTexel.value.set(1 / source.width, 1 / source.height);
        post.down.uniforms.uKaris.value = level === 0 ? 1 : 0;
        renderer.setRenderTarget(into);
        renderer.render(post.scene, post.lens);
        source = into;
      }
      post.quad.material = post.up;
      for (let level = post.mips.length - 1; level > 0; level -= 1) {
        const from = post.mips[level];
        post.up.uniforms.tSrc.value = from.texture;
        post.up.uniforms.uTexel.value.set(1 / from.width, 1 / from.height);
        renderer.setRenderTarget(post.mips[level - 1]);
        renderer.render(post.scene, post.lens);
      }
      const comp = post.comp.uniforms;
      comp.tScene.value = post.target.texture;
      comp.tBloom.value = post.mips[0]?.texture ?? post.target.texture;
      comp.uBloom.value = post.mips.length > 0 ? this.tune.bloomStrength : 0;
      comp.uTime.value = this.uniforms.uTime.value;
      post.quad.material = post.comp;
      renderer.setRenderTarget(null);
      renderer.render(post.scene, post.lens);
    },

    /* 배율의 수 — 처음 자리에 대한 거리의 몫. */
    paintZoom() {
      const word = this.view.querySelector(".knowledge-zoom-label");
      if (word !== null && this.home !== null) writeTextContent(word, `${Math.round((this.home.dist / this.dist) * 100)}%`);
    },

    /* 한 줄 알림 — 우주 위의 판에 선다(평면으로 돌아온 뒤에도 읽히게). */
    say(words) {
      if (this.notice === null) {
        const notice = document.createElement("p");
        notice.className = "knowledge-universe-notice";
        notice.setAttribute("role", "status");
        notice.setAttribute("aria-live", "polite");
        this.view.querySelector(".knowledge-stage")?.appendChild(notice);
        this.notice = notice;
      }
      writeTextContent(this.notice, words);
      this.notice.classList.add("is-on");
      clearTimeout(this.noticeTimer);
      this.noticeTimer = setTimeout(() => this.notice?.classList.remove("is-on"), U.noticeMs);
    },

    /* 문맥을 잃었다: 평면으로 돌아와 한 줄로 말한다. 우주는 그대로 쥐고 있다가 문맥이 돌아오면 다시
     * 선다(시안의 문맥 잃음). */
    loseContext() {
      this.lost = true;
      this.halt();
      this.tween.on = false;
      this.view.classList.remove("is-universe");
      this.say(t("knowledge.universeLost", "그래픽 장치가 우주 그림을 놓아 평면 지도로 보여 줍니다 — 돌아오면 다시 섭니다"));
      paintKnowledgeDimension(this.view);
    },

    regainContext() {
      this.lost = false;
      for (const geometry of this.built.geometries) {
        for (const attribute of Object.values(geometry.attributes)) attribute.needsUpdate = true;
      }
      this.say(t("knowledge.universeBack", "그래픽 장치가 돌아와 우주를 다시 엽니다"));
      if (knowledgeUniverseWanted(this.view, knowledgeLayouts.get(this.view))) this.enter(true);
      paintKnowledgeDimension(this.view);
    },

    /* 지은 것을 놓는다 — 다시 짓기 전에, 그리고 떠날 때. */
    clearScene() {
      for (const object of this.built.objects) this.scene?.remove(object);
      for (const geometry of this.built.geometries) geometry.dispose();
      this.galTexture?.dispose();
      for (const texture of this.nodeTextures) texture.dispose();
      this.nodeTextures = [];
      this.built.objects.length = 0;
      this.built.geometries.length = 0;
      this.starGeometry = null;
      this.starStateAttr = null;
      this.galTexture = null;
      this.map = null;
      this.node3 = null;
      this.node2 = null;
      this.lineGeometry = null;
      this.lineObject = null;
      if (this.labels !== null) this.labels.textContent = "";
      this.plates = [];
      this.names = [];
      this.flowTags = [];
      this.tip = null;
      this.tipBox = null;
      this.labelVersion = -1;
      this.selectedSeat = -1;
      if (this.uniforms !== null) {
        this.uniforms.tGal.value = null;
        this.uniforms.tNode3.value = null;
        this.uniforms.tNode2.value = null;
      }
    },

    /* 떠난다 — 기하·재질(프로그램)·렌더러·문맥·손·캔버스 전부. 순서가 약속이다: 기하와 재질의
     * `dispose()`가 렌더러의 것보다 먼저다(렌더러가 제 장부를 갈아 끼운 뒤에는 GL 객체를 찾지 못한다). */
    dispose() {
      this.halt();
      clearTimeout(this.noticeTimer);
      this.clearScene();
      for (const material of Object.values(this.materials ?? {})) material.dispose();
      this.dropTargets();
      if (this.post !== null) {
        for (const material of [this.post.down, this.post.up, this.post.comp]) material.dispose();
        this.post.triangle.dispose();
      }
      this.renderer?.renderLists.dispose();
      this.themeWatch?.disconnect();
      this.themeWatch = null;
      for (const stop of this.unlisten) stop();
      this.unlisten.length = 0;
      const context = this.renderer?.getContext() ?? null;
      this.renderer?.dispose();
      context?.getExtension("WEBGL_lose_context")?.loseContext();
      this.host?.remove();
      this.notice?.remove();
      this.renderer = null;
      this.scene = null;
      this.camera = null;
      this.materials = null;
      this.uniforms = null;
      this.layout = null;
      this.pos2 = null;
      this.pos3 = null;
      this.starState = null;
      this.notice = null;
      this.post = null;
      this.probe = null;
      this.inks = null;
      this.folding = false;
    },
  };
}
