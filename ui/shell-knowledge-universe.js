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
  starAgeScale: "--knowledge-3d-star-age-scale",
  starAgeSpan: "--knowledge-3d-star-age-span",
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
  if (layout !== undefined) paintKnowledgeFrame(view, layout);
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
    return;
  }
  if (universe.layout !== layout && !universe.build(layout)) {
    leaveKnowledgeUniverse(view);
    knowledgeUniverseFailed = true;
    paintKnowledgeDimension(view);
    return;
  }
  universe.dress();
}

/* 옷이 바뀐 순간(고름·밝힘·찾기·군집) — 평면의 프레임 문(`paintKnowledgeFrame`)이 부른다. 서 있는
 * 우주는 다음 프레임 한 장으로 따라온다. */
function dressKnowledgeUniverse(view) {
  knowledgeUniverses.get(view)?.dress();
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

/* ---- 셰이더 — 시안의 글자 그대로 ----------------------------------------------------- */

/* 별 색(Wikipedia 분광형 표의 D65 색을 선형으로) — 시안 `GAL`의 한 토막. */
const KNOWLEDGE_UNIVERSE_STAR_COLOR = `
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
}`;

/* 이름 있는 별(쪽): 밝기 = 연결 수(등급 척도), 색 = 최근 고침(흑체 색), 밝은 별은 빛살, 찾은 별은
 * 조준 고리 — 시안 `materials.stars` 그대로. */
const KNOWLEDGE_UNIVERSE_STARS_VERT = `${KNOWLEDGE_UNIVERSE_STAR_COLOR}
      attribute vec3 aPos2; attribute vec4 aStar; attribute float aState;
      uniform float uLift; uniform float uDpr; uniform float uRef; uniform float uFocus;
      varying vec3 vCol; varying float vI; varying float vS; varying float vState; varying float vSpike;
      void main(){
        vec3 w = mix(aPos2, position, uLift);
        vec4 clip = projectionMatrix * viewMatrix * vec4(w, 1.0);
        gl_Position = clip;
        float st = aState;
        float flux = aStar.x * clamp(pow(uRef / max(clip.w, 1.0), 2.0), 0.15, 6.0);
        if (st > 1.5 && st < 2.5) flux = max(flux * 2.0, 4.0);
        float size = clamp(4.0 * pow(flux, 0.38), 2.6, 34.0);
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

/* ---- 우주의 자료 — 평면 지도에서 짓는다 ------------------------------------------------ */

/* 해시 하나(시안 `kg-data.js`의 `prng`의 둘째 값) — 난수는 없다: 같은 볼트는 같은 우주다. */
function knowledgeUniverseHash(seed) {
  let state = seed >>> 0;
  let value = 0;
  for (let round = 0; round < 2; round += 1) {
    state = (state + 0x6d2b79f5) | 0;
    let mixed = Math.imul(state ^ (state >>> 15), 1 | state);
    mixed = (mixed + Math.imul(mixed ^ (mixed >>> 7), 61 | mixed)) ^ mixed;
    value = ((mixed ^ (mixed >>> 14)) >>> 0) / 4294967296;
  }
  return value;
}

/* 평면 자리를 세계로 옮기는 수와 우주가 닿는 거리. 은하의 반지름이 시안의 r2(√쪽 × `worldRoot` ×
 * 볼트 크기의 몫)가 되도록 평면의 원반 반지름(√쪽 × `cluster-pitch` × `cluster-spread`)에 곱하는 수다. */
function knowledgeUniverseScale(layout) {
  const U = knowledgeUniverseTuning(layout.view);
  const { clusterPitch, clusterSpread } = layout.tuning;
  const spread = (Math.max(1, layout.count) / U.worldPages) ** U.worldSpread;
  return (U.worldRoot * spread) / Math.max(1e-6, clusterPitch * clusterSpread);
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
    motion: true,
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
      host.append(canvas, labels);
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
      /* 모든 재질이 함께 보는 값(시안 `U`). 이 조각이 쓰는 것만 서고, 뒤의 조각이 더한다. */
      this.uniforms = {
        uLift: { value: 1 }, uTime: { value: 0 }, uFocus: { value: 0 }, uDpr: { value: 1 }, uRef: { value: 360 },
      };
      const material = (vertexShader, fragmentShader) => new THREE.ShaderMaterial({
        uniforms: this.uniforms, vertexShader, fragmentShader, transparent: true, depthTest: false, depthWrite: false,
        blending: THREE.NormalBlending, premultipliedAlpha: true,
      });
      this.materials = { stars: material(KNOWLEDGE_UNIVERSE_STARS_VERT, KNOWLEDGE_UNIVERSE_STARS_FRAG) };
      this.wire();
      return true;
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
        if (this.down === null) return;
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
      };
      on(host, "pointerup", release);
      on(host, "pointercancel", release);
      on(host, "pointerleave", () => {
        this.pointer.inside = false;
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
      this.scale = knowledgeUniverseScale(layout);
      /* 닿는 거리 — 이름 있는 원반의 가장 먼 끝(시안 `layout`의 reach). */
      let reach = 0;
      for (let rank = 0; rank < layout.namedCount; rank += 1) {
        reach = Math.max(reach, Math.hypot(layout.communityHomeX[rank], layout.communityHomeY[rank])
          + layout.communityHomeR[rank]);
      }
      this.reach = Math.max(1, reach * this.scale);
      /* 이름 있는 별: 밝기 = ((1 + 연결) / (1 + 가운데값))^1.6, 빛깔 = 최근 고침(오늘 1 → 1년 0). 고친 때를
       * 모르는 쪽은 오래된 것으로 친다(디자이너 답 m-12467). */
      const sorted = Int32Array.from(model.degree).sort();
      const middle = count > 0 ? sorted[count >> 1] : 0;
      const nowMs = model.nowMs;
      this.pos2 = new Float32Array(count * 3);
      this.pos3 = new Float32Array(count * 3);
      const star = new Float32Array(count * 4);
      for (let at = 0; at < count; at += 1) {
        const days = model.modified[at] > 0 ? Math.max(0, (nowMs - model.modified[at]) / 86_400_000)
          : U.starAgeSpan;
        star[at * 4] = ((1 + model.degree[at]) / (1 + middle)) ** U.starLumExp;
        star[at * 4 + 1] = Math.max(0, Math.min(1,
          1 - Math.log1p(days / U.starAgeScale) / Math.log1p(U.starAgeSpan / U.starAgeScale)));
        star[at * 4 + 2] = layout.community[at];
      }
      this.placeSeats();
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
      const stars = new THREE.Points(geometry, this.materials.stars);
      stars.frustumCulled = false;
      this.scene.add(stars);
      this.built.objects.push(stars);
      this.computeHome();
      writeAttribute(this.canvas, "aria-label", t("knowledge.universeSummary", "지식 그래프 우주 · 주제 {{topics}} · 쪽 {{pages}}",
        { topics: layout.namedCount, pages: count }));
      try {
        this.placeCamera();
        this.renderer.compile(this.scene, this.camera);
      } catch (trouble) {
        invoke("note_webview_error", { text: `knowledge universe: ${trouble}` }).catch(() => {});
        return false;
      }
      return true;
    },

    /* 평면 자리(떠오르기 전)와 세계 자리. 평면 자리는 평면 지도의 그린 자리(눌림까지)이고, 세계 자리는
     * 은하의 높이 위에 선다 — 앉기가 끝난 판에서 한 번 더 옮긴다(`dress`). */
    placeSeats() {
      const layout = this.layout;
      const { count, x, y, drawY, community, namedCount, model } = layout;
      const s = this.scale;
      const flat = U.flatHeight;
      for (let at = 0; at < count; at += 1) {
        const rank = community[at];
        this.pos2[at * 3] = x[at] * s;
        this.pos2[at * 3 + 1] = flat;
        this.pos2[at * 3 + 2] = drawY[at] * s;
        const named = rank < namedCount;
        const core = named ? layout.communityCore[rank] : -1;
        const seed = knowledgeHash(model.keys[core >= 0 ? core : at]);
        const share = 2 * (named ? U.liftGalaxy : U.liftOrphan);
        this.pos3[at * 3] = x[at] * s;
        this.pos3[at * 3 + 1] = (knowledgeUniverseHash(seed) - 0.5) * share * this.reach;
        this.pos3[at * 3 + 2] = y[at] * s;
      }
      this.seatsRevision = layout.geometryRevision;
      if (this.starGeometry !== null) {
        this.starGeometry.attributes.position.needsUpdate = true;
        this.starGeometry.attributes.aPos2.needsUpdate = true;
      }
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
      }
      this.uniforms.uTime.value = this.time;
      this.uniforms.uLift.value = this.lift;
      this.placeCamera();
      this.render();
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
      this.uniforms.uDpr.value = this.renderer.getPixelRatio();
    },

    render() {
      const renderer = this.renderer;
      renderer.info.reset();
      renderer.setRenderTarget(null);
      renderer.setClearColor(0x000000, 1);
      renderer.clear(true, false, false);
      renderer.render(this.scene, this.camera);
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
      this.built.objects.length = 0;
      this.built.geometries.length = 0;
      this.starGeometry = null;
      this.starStateAttr = null;
    },

    /* 떠난다 — 기하·재질(프로그램)·렌더러·문맥·손·캔버스 전부. 순서가 약속이다: 기하와 재질의
     * `dispose()`가 렌더러의 것보다 먼저다(렌더러가 제 장부를 갈아 끼운 뒤에는 GL 객체를 찾지 못한다). */
    dispose() {
      this.halt();
      clearTimeout(this.noticeTimer);
      this.clearScene();
      for (const material of Object.values(this.materials ?? {})) material.dispose();
      this.renderer?.renderLists.dispose();
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
      this.folding = false;
    },
  };
}
