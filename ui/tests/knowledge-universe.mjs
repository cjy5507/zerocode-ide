/* ---- 지식 그래프의 우주 보기 (t-12443의 뼈대, 11-12의 「빛의 그물」) -----------------------------
 *
 * 전체 지도는 WebGL2가 서는 판에서 우주로 열리고, 머리의 「2D | 3D」가 평면 지도와 우주를 오간다.
 * 이 파일이 묻는 것은 계약이다 — 어느 판에서 우주가 서는가, 오가도 평면 지도의 자리가 그대로인가,
 * 사람이 고른 쪽을 기억하는가, 쉴 때 한 장도 그리지 않는가, 가려지면 멈추는가, 문맥을 잃으면
 * 평면으로 돌아와 한 줄로 말하는가, 떠날 때 GPU의 것을 다 놓는가 — 그리고 그림이 자료에서 나오는가:
 * 군집이 제 허브의 나무인가, 모든 링크가 그림의 한 통에 드는가, 올리면 그 쪽의 링크가 켜지는가. 그림의
 * 결은 사진이 말하고, 시간은 진짜 GPU에서 잰다(`knowledge-universe-gpu.mjs`). 여기(소프트웨어 GL)의 수는
 * 세는 수(프레임·드로우·해제)뿐이다. */

/* WebGL2가 없는 판(게이트의 크로미엄): 3D 단추는 서되 눌리지 않고 까닭을 말한다 — 죽은 컨트롤은
 * 없다. 전체 지도는 평면이다. */
export async function testKnowledgeUniverseUnable(page, ok) {
  const seen = await page.evaluate(async () => {
    try {
      const view = document.querySelector(".knowledge-view:not([hidden])");
      const frame = () => new Promise((done) => requestAnimationFrame(done));
      const heldMode = knowledgeMode;
      setKnowledgeMode(view, "global", { paint: false });
      await paintKnowledgeView();
      await frame();
      const three = view.querySelector('[data-knowledge-dimension="3d"]');
      const two = view.querySelector('[data-knowledge-dimension="2d"]');
      const before = { mounted: knowledgeUniverses.has(view), host: view.querySelector(".knowledge-universe") !== null };
      three?.click();
      await frame();
      await frame();
      const seen = {
        able: knowledgeGlSupported(),
        group: view.querySelector(".knowledge-head .knowledge-dimension") !== null,
        /* 누를 수 없는 채로 서되 초점과 손은 받아 팁이 까닭을 말한다 — 관계 탭의 입체 토글과 같은 손(`aria-disabled`). */
        threeDisabled: three?.getAttribute("aria-disabled") === "true",
        threeTip: three?.dataset.tip ?? "",
        wantedTip: t("knowledge.dimensionUnable", "이 창에서는 우주를 그릴 수 없어 평면 지도로 봅니다"),
        twoPressed: two?.getAttribute("aria-pressed"),
        before,
        afterClick: { mounted: knowledgeUniverses.has(view), host: view.querySelector(".knowledge-universe") !== null,
          universe: view.classList.contains("is-universe") },
      };
      setKnowledgeMode(view, heldMode, { paint: false });
      await paintKnowledgeView();
      return seen;
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  });
  ok("t-12443 ①: where the universe cannot be drawn the 3D step is there but cannot be pressed, says why, and the whole map stays flat",
    !seen.thrown && seen.able === false && seen.group && seen.threeDisabled
      && seen.threeTip === seen.wantedTip && seen.threeTip !== "" && seen.twoPressed === "true"
      && !seen.before.mounted && !seen.before.host
      && !seen.afterClick.mounted && !seen.afterClick.host && !seen.afterClick.universe,
    JSON.stringify(seen));
}

/* WebGL2가 서는 판(소프트웨어 GL). 움직임을 줄인 판에서 묻는다 — 오가는 비행과 흐름이 즉시로 접혀
 * 세는 수가 기계의 빠르기와 무관해진다. 움직임이 있는 판은 「가려지면 멈춘다」 하나만 묻는다. */
export async function testKnowledgeUniverse(page, ok) {
  await page.emulateMedia({ reducedMotion: "reduce" });
  const seen = await page.evaluate(async () => {
    try {
      const view = document.querySelector(".knowledge-view:not([hidden])");
      const frame = () => new Promise((done) => requestAnimationFrame(done));
      const frames = async (count) => {
        for (let at = 0; at < count; at += 1) await frame();
      };
      const pause = (ms) => new Promise((done) => setTimeout(done, ms));
      const until = async (wanted, rounds = 600) => {
        for (let round = 0; round < rounds; round += 1) {
          if (wanted()) return true;
          await frame();
        }
        return false;
      };
      const pressed = (id) => view.querySelector(`[data-knowledge-dimension="${id}"]`)?.getAttribute("aria-pressed");
      const standing = () => ({
        mounted: knowledgeUniverses.has(view),
        host: view.querySelector(".knowledge-canvas .knowledge-universe canvas") !== null,
        universe: view.classList.contains("is-universe"),
        pressed3d: pressed("3d"),
        pressed2d: pressed("2d"),
      });
      const heldPainter = knowledgePainterKind;
      const heldDimension = knowledgeDimension;
      const heldVault = secondBrainVault;
      knowledgePainterKind = null;
      /* 사람이 아직 고르지 않은 판 — 기본값은 표의 한 줄이다. */
      knowledgeDimension = null;
      knowledgeQuery = "";
      knowledgeTagsPicked.clear();
      knowledgeOrphansOnly = knowledgeGhostsOnly = knowledgeTypedOnly = false;
      knowledgeAliveOnly = knowledgeColdOnly = knowledgeMergeOnly = false;
      knowledgeShowSources = false;
      knowledgeLintLens = knowledgeSelectedKey = knowledgePath = null;
      knowledgeClusterPicked = -1;
      knowledgeSlicerCutoff = 0;
      const answer = window.__buildVaultGraph__({ path: "/universe", sources: false },
        { pages: 120, linksPer: 3, ghosts: 4, tags: ["core", "reading", "tools"] });
      noteKnowledgeExploreLines({});
      /* 문의 기억(`applyKnowledgeEntry`)은 설정의 볼트와 답의 볼트가 같을 때만 읽힌다 — 이 판의 볼트로 선다. */
      secondBrainVault = answer.vault;
      setKnowledgeMode(view, "global", { paint: false });
      knowledgeReport = answer;
      await paintKnowledgeView();
      await until(() => (knowledgeLayouts.get(view)?.left ?? 1) === 0);
      await frames(2);
      const defaultRow = KNOWLEDGE_DIMENSIONS.find((row) => row.first === true)?.id ?? null;
      const opened = { ...standing(), defaultRow, now: knowledgeDimensionNow() };
      const universe = knowledgeUniverses.get(view);
      /* 우주가 덮는 동안 평면의 GL 손은 그리기 버퍼를 돌려준다 — 고르기 한 번 뒤에도 그대로다(가려진 그림을 다시
       * 그리지 않는다). */
      const flatBuffer = () => {
        const canvas = view.querySelector("canvas.knowledge-gl");
        return canvas === null ? null : [canvas.width, canvas.height];
      };
      const resting = { painter: knowledgePainterFor(view).id, buffer: flatBuffer() };

      /* 쉴 때 0프레임 — 비행이 끝난 뒤 한 초 동안 그린 장 수. 고르기 하나는 한두 장을 그리고 다시 쉰다. */
      await until(() => universe?.tween?.on === false);
      await frames(3);
      const idleFrom = universe?.frames ?? -1;
      await pause(1000);
      const idleFrames = (universe?.frames ?? -1) - idleFrom;
      const layout = knowledgeLayouts.get(view);
      const pickFrom = universe?.frames ?? -1;
      selectKnowledgeNode(view, layout.model.keys[0]);
      await frames(4);
      const pickDrew = (universe?.frames ?? -1) - pickFrom;
      const afterPick = universe?.frames ?? -1;
      await pause(600);
      const pickSettled = (universe?.frames ?? -1) - afterPick;
      resting.afterPick = flatBuffer();
      selectKnowledgeNode(view, null);
      await frames(2);

      /* 평면으로 갔다가 돌아와도 평면 지도의 자리와 카메라는 한 자도 바뀌지 않는다. */
      const xs = Float32Array.from(layout.x);
      const ys = Float32Array.from(layout.y);
      const camera = { zoom: layout.zoom, panX: layout.panX, panY: layout.panY };
      const held = universe;
      const heldInfo = held?.renderer?.info ?? null;
      const heldGl = held?.renderer?.getContext() ?? null;
      view.querySelector('[data-knowledge-dimension="2d"]').click();
      await frames(3);
      const flat = standing();
      resting.flat = flatBuffer();
      const released = {
        renderer: held === undefined ? "no universe" : held.renderer,
        geometries: heldInfo?.memory.geometries ?? -1,
        textures: heldInfo?.memory.textures ?? -1,
        programs: heldInfo?.programs?.length ?? -1,
        contextLost: heldGl?.isContextLost() ?? null,
        canvasInDom: held?.canvas?.isConnected ?? false,
      };
      await pause(KNOWLEDGE_EXPLORE.persistMs + 150);
      const rememberedFlat = JSON.parse(knowledgeExploreLines[answer.vault] ?? "{}").dimension ?? null;
      view.querySelector('[data-knowledge-dimension="3d"]').click();
      await frames(3);
      const back = standing();
      await pause(KNOWLEDGE_EXPLORE.persistMs + 150);
      const remembered3d = JSON.parse(knowledgeExploreLines[answer.vault] ?? "{}").dimension ?? null;
      view.querySelector('[data-knowledge-dimension="2d"]').click();
      await frames(3);
      const sameSeats = layout === knowledgeLayouts.get(view)
        && xs.every((value, at) => value === layout.x[at]) && ys.every((value, at) => value === layout.y[at]);
      const sameCamera = camera.zoom === layout.zoom && camera.panX === layout.panX && camera.panY === layout.panY;

      /* 문을 다시 열면 이 볼트가 기억하는 쪽으로 선다. */
      await pause(KNOWLEDGE_EXPLORE.persistMs + 150);
      knowledgeDimension = null;
      knowledgeEntryPending = true;
      await paintKnowledgeView();
      await frames(2);
      const reopenedFlat = standing();
      noteKnowledgeExploreLines({ [answer.vault]: JSON.stringify({ mode: "global", dimension: "3d" }) });
      knowledgeDimension = null;
      knowledgeEntryPending = true;
      await paintKnowledgeView();
      await frames(2);
      const reopened3d = standing();

      /* 주변 탐색은 평면이다 — 들어가면 우주가 접히고, 3D를 누르면 전체 지도의 우주로 나온다. */
      setKnowledgeMode(view, "local");
      await frames(3);
      const local = { ...standing(), mode: knowledgeMode };
      view.querySelector('[data-knowledge-dimension="3d"]').click();
      await frames(3);
      const fromLocal = { ...standing(), mode: knowledgeMode };

      /* 문맥을 잃으면 평면으로 돌아와 한 줄로 말하고, 되찾으면 우주로 돌아간다. */
      const lossy = knowledgeUniverses.get(view);
      const lose = lossy?.renderer?.getContext().getExtension("WEBGL_lose_context") ?? null;
      let lost = null;
      let restored = null;
      if (lose !== null) {
        lose.loseContext();
        await until(() => lossy.lost === true, 120);
        await frames(2);
        const notice = view.querySelector(".knowledge-universe-notice");
        lost = { ...standing(), lost: lossy.lost, flat: flatBuffer(), notice: notice?.textContent ?? "",
          noticeShown: notice?.classList.contains("is-on") ?? false,
          wanted: t("knowledge.universeLost", "그래픽 장치가 우주 그림을 놓아 평면 지도로 보여 줍니다 — 돌아오면 다시 섭니다") };
        lose.restoreContext();
        await until(() => lossy.lost === false, 240);
        await frames(4);
        restored = { ...standing(), lost: lossy.lost, flat: flatBuffer(),
          notice: view.querySelector(".knowledge-universe-notice")?.textContent ?? "",
          wanted: t("knowledge.universeBack", "그래픽 장치가 돌아와 우주를 다시 엽니다") };
      }

      knowledgePainterKind = heldPainter;
      knowledgeDimension = heldDimension;
      secondBrainVault = heldVault;
      await paintKnowledgeView();
      return { opened, idleFrames, pickDrew, pickSettled, flat, released, rememberedFlat, back, remembered3d,
        sameSeats, sameCamera, reopenedFlat, reopened3d, local, fromLocal, lost, restored, resting };
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  });
  const detail = JSON.stringify(seen);
  ok("t-12443 ①: the whole map opens as the universe where WebGL2 stands, from the one default row of the dimension table, with 3D pressed",
    !seen.thrown && seen.opened.defaultRow === "3d" && seen.opened.now === "3d" && seen.opened.mounted
      && seen.opened.host && seen.opened.universe && seen.opened.pressed3d === "true"
      && seen.opened.pressed2d === "false",
    detail);
  ok("t-12443 ①: a resting universe draws no frame in a second, and a pick draws its frames and rests again",
    !seen.thrown && seen.idleFrames === 0 && seen.pickDrew >= 1 && seen.pickSettled === 0,
    detail);
  ok("t-12443 ①: 2D and back leave every flat seat and the flat camera exactly where they were",
    !seen.thrown && !seen.flat.mounted && !seen.flat.host && !seen.flat.universe && seen.flat.pressed2d === "true"
      && seen.back.mounted && seen.back.universe && seen.back.pressed3d === "true"
      && seen.sameSeats && seen.sameCamera,
    detail);
  ok("universe ①: while the universe covers the flat map the flat painter rests — its drawing buffer goes back, a pick does not repaint it — and it stands at its size again when the universe leaves",
    !seen.thrown && seen.resting.painter === "gl" && seen.resting.buffer?.join("x") === "1x1"
      && seen.resting.afterPick?.join("x") === "1x1" && seen.resting.flat !== null
      && seen.resting.flat[0] > 1 && seen.resting.flat[1] > 1,
    detail);
  ok("t-12443 ①: leaving the universe lets go of its renderer, geometries, textures, programs, canvas and context",
    !seen.thrown && seen.released.renderer === null && seen.released.geometries === 0
      && seen.released.textures === 0 && seen.released.programs === 0 && seen.released.contextLost === true
      && seen.released.canvasInDom === false,
    detail);
  ok("t-12443 ①: the person's choice is this vault's to remember, and the door opens on it",
    !seen.thrown && seen.rememberedFlat === "2d" && seen.remembered3d === "3d"
      && !seen.reopenedFlat.mounted && seen.reopenedFlat.pressed2d === "true"
      && seen.reopened3d.mounted && seen.reopened3d.universe,
    detail);
  ok("t-12443 ①: exploring nearby is flat, and 3D from there opens the whole map's universe",
    !seen.thrown && seen.local.mode === "local" && !seen.local.mounted && !seen.local.universe
      && seen.local.pressed2d === "true"
      && seen.fromLocal.mode === "global" && seen.fromLocal.mounted && seen.fromLocal.universe,
    detail);
  ok("t-12443 ①: a lost context falls back to the flat map and says so in one line; a restored one brings the universe back",
    !seen.thrown && seen.lost !== null && seen.lost.lost === true && !seen.lost.universe
      && seen.lost.notice === seen.lost.wanted && seen.lost.noticeShown
      && seen.restored.lost === false && seen.restored.universe && seen.restored.notice === seen.restored.wanted,
    detail);
  ok("universe ①: the flat map a lost context falls back to is painted at its size — the painter that rested under the universe draws again — and rests once more when the universe returns",
    !seen.thrown && seen.lost !== null && seen.lost.flat !== null && seen.lost.flat[0] > 1 && seen.lost.flat[1] > 1
      && seen.restored.flat?.join("x") === "1x1",
    detail);

  /* 움직임이 있는 판: 쉬는 우주는 아주 느리게 돌고(프레임이 선다), 판이 가려지면 멈춘다. */
  await page.emulateMedia({ reducedMotion: "no-preference" });
  const moving = await page.evaluate(async () => {
    try {
      const view = document.querySelector(".knowledge-view:not([hidden])");
      const pause = (ms) => new Promise((done) => setTimeout(done, ms));
      const frame = () => new Promise((done) => requestAnimationFrame(done));
      const heldDimension = knowledgeDimension;
      knowledgeDimension = "3d";
      setKnowledgeMode(view, "global", { paint: false });
      await paintKnowledgeView();
      const universe = knowledgeUniverses.get(view);
      universe?.invalidate();
      const from = universe?.frames ?? -1;
      await pause(600);
      const moved = (universe?.frames ?? -1) - from;
      view.hidden = true;
      await frame();
      await frame();
      const hiddenFrom = universe?.frames ?? -1;
      await pause(600);
      const hiddenFrames = (universe?.frames ?? -1) - hiddenFrom;
      view.hidden = false;
      await paintKnowledgeView();
      const shownFrom = universe?.frames ?? -1;
      await pause(400);
      const shownFrames = (universe?.frames ?? -1) - shownFrom;
      knowledgeDimension = heldDimension;
      await paintKnowledgeView();
      return { moved, hiddenFrames, shownFrames, after: knowledgeUniverses.has(view) };
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  });
  await page.emulateMedia({ reducedMotion: "reduce" });
  ok("t-12443 ①: with motion the universe keeps drawing, a hidden pane draws nothing, and showing it again resumes",
    !moving.thrown && moving.moved > 0 && moving.hiddenFrames === 0 && moving.shownFrames > 0,
    JSON.stringify(moving));
}

/* ---- ② 군집 짓기 · 나무 · 별 · 빛 번짐과 합성 ------------------------------------------------------
 *
 * 규칙이 맞게 섰는지를 셈으로 묻는다(그림의 결은 사진이 말한다). 볼트는 여기서 짓는다: 쪽 수가 다른 스물두
 * 무리(무리 안은 촘촘하고 무리 사이는 잇지 않는다)와 홀로 선 쪽 여섯 — 평면 지도의 군집이 그대로 무리가 되도록. 무리
 * 9와 12는 오래전에 고쳤고(활동 0), 무리 15는 절반만 요즘 고쳤으며, 나머지는 오늘 고쳤다. */
const GALAXY_SIZES = Object.freeze([80, 56, 52, 48, 45, 42, 39, 36, 33, 30, 28, 26, 24, 22, 20, 18, 16, 14, 12, 10, 5, 4]);
const GALAXY_ALONE = 6;
const GALAXY_QUIET = Object.freeze([9, 12]);
const GALAXY_HALF = 15;

export async function testKnowledgeUniverseGalaxies(page, ok) {
  await page.emulateMedia({ reducedMotion: "reduce" });
  const seen = await page.evaluate(async ({ sizes, alone, quiet, half }) => {
    try {
      const view = document.querySelector(".knowledge-view:not([hidden])");
      const frame = () => new Promise((done) => requestAnimationFrame(done));
      const until = async (wanted, rounds = 900) => {
        for (let round = 0; round < rounds; round += 1) {
          if (wanted()) return true;
          await frame();
        }
        return false;
      };
      const heldDimension = knowledgeDimension;
      const heldVault = secondBrainVault;
      const heldPainter = knowledgePainterKind;
      knowledgePainterKind = null;
      knowledgeQuery = "";
      knowledgeTagsPicked.clear();
      knowledgeOrphansOnly = knowledgeGhostsOnly = knowledgeTypedOnly = false;
      knowledgeAliveOnly = knowledgeColdOnly = knowledgeMergeOnly = false;
      knowledgeShowSources = false;
      knowledgeLintLens = knowledgeSelectedKey = knowledgePath = null;
      knowledgeClusterPicked = -1;
      knowledgeSlicerCutoff = 0;
      /* 무리마다 사슬 하나와 앞선 쪽 둘로의 가지(결정적). 무리 사이는 잇지 않는다 — 한 가닥이라도 이으면 평면
       * 지도(Louvain)가 작은 이웃 무리를 합쳐 순위가 밀린다. */
      const edges = [];
      const starts = [];
      let at = 0;
      sizes.forEach((size) => { starts.push(at); at += size; });
      const total = at + alone;
      sizes.forEach((size, group) => {
        const first = starts[group];
        for (let step = 1; step < size; step += 1) {
          edges.push({ from: first + step, to: first + step - 1, kind: "mentions" });
          if (step > 2) edges.push({ from: first + step, to: first + ((step * 7) % (step - 1)), kind: "mentions" });
          if (step > 4) edges.push({ from: first + step, to: first + ((step * 13) % (step - 3)), kind: "related" });
        }
      });
      /* 무리 0의 첫 쪽은 제 무리 전부를 가리키는 허브다 — 연결 비가 크기의 천장(`star-ratio-cap`)을 넘는다. */
      for (let step = 2; step < sizes[0]; step += 1) edges.push({ from: 0, to: step, kind: "mentions" });
      const nowMs = Date.now();
      const modifiedMs = [];
      sizes.forEach((size, group) => {
        for (let step = 0; step < size; step += 1) {
          const old = quiet.includes(group) || (group === half && step % 2 === 1);
          modifiedMs.push(old ? nowMs - 400 * 86_400_000 : nowMs - (step % 5) * 3_600_000);
        }
      });
      for (let one = 0; one < alone; one += 1) modifiedMs.push(0);
      const answer = window.__buildVaultGraph__({ path: "/galaxies", sources: false },
        { pages: total, ghosts: 0, tags: [], customEdges: edges, modifiedMs });
      noteKnowledgeExploreLines({});
      secondBrainVault = answer.vault;
      knowledgeDimension = "3d";
      setKnowledgeMode(view, "global", { paint: false });
      knowledgeReport = answer;
      await paintKnowledgeView();
      await until(() => (knowledgeLayouts.get(view)?.left ?? 1) === 0);
      await paintKnowledgeView();
      await frame();
      await frame();
      const layout = knowledgeLayouts.get(view);
      const model = layout.model;
      const universe = knowledgeUniverses.get(view);
      const map = universe.map;
      const tune = knowledgeUniverseTuning(view);
      const B = KNOWLEDGE_UNIVERSE_BURST;
      const pages = model.kinds.filter((kind) => kind === "page").length;
      const least = Math.max(tune.galaxyLeast, Math.round((tune.galaxyShare * pages) / tune.worldPages));
      let wantGalaxies = 0;
      while (wantGalaxies < Math.min(layout.namedCount, tune.galaxyMax)
        && layout.communitySize[wantGalaxies] >= least) wantGalaxies += 1;
      /* 반지름은 쪽 수에서(평면 원반의 반지름이 아니라 — m-12586): 큰 군집 `galaxy-radius` × 2.7√m × 몫, 작은 군집
       * 2.1√m × 몫 × 1.25 + 2. 자리 축척은 2.7 × 몫 / cluster-pitch. */
      const spread = (pages / tune.worldPages) ** tune.worldSpread;
      const scaleWant = (tune.worldRoot * spread) / layout.tuning.clusterPitch;
      const rows = map.galaxy.map((row) => ({ rank: row.rank, large: row.large,
        texelPages: map.gal[(row.rank * KNOWLEDGE_UNIVERSE_SHAPE.texels + 3) * 4 + 1], pages: map.members[row.rank].length,
        radius: Math.round(row.radius * 1000) / 1000,
        wantRadius: Math.round((row.rank < wantGalaxies
          ? tune.galaxyRadius * tune.worldRoot * Math.sqrt(layout.communitySize[row.rank]) * spread
          : tune.clusterSphere * Math.sqrt(layout.communitySize[row.rank]) * spread * tune.clusterGrow
            + tune.clusterPad) * 1000) / 1000 }));
      /* 붐빔 — 표 밖에서 다시 셈한다. 군집의 별 사이 간격 = √(π r² / 쪽), r = 반지름 × 초점 거리 / 군집 가운데의 깊이.
       * 별의 자리는 그 간격과 형제 사이 간격(× 초점 거리 / 그 별의 깊이) 가운데 좁은 쪽이다. 허브가 아닌 별(연결이 가장
       * 적은 쪽)의 속은 자리의 `crowd-ratio`를 넘지 않되 `crowdSize`배 밑으로는 줄지 않는다. 그 별에 바짝 다가가면 자리가
       * 벌어져 별이 제 크기로 돌아오고, 멀리 물러나면 줄어든다. */
      const S = KNOWLEDGE_UNIVERSE_STAR;
      const starOf = universe.starGeometry.attributes.aStar.array;
      const sized = (seat, gap) => {
        const spot = universe.project(seat);
        const near = Math.max(S.nearLeast, Math.min(S.nearMost, universe.uniforms.uRef.value / Math.max(spot.depth, 1)));
        const rest = Math.min(S.coreCeil,
          Math.max(S.coreMin, Math.min(S.coreMost, S.coreLeast + S.coreGrow * starOf[seat * 4] ** S.corePower)) * near);
        const room = (map.room[seat] * universe.focal) / Math.max(spot.depth, 1);
        const size = Math.max(S.crowdSize, Math.min(1, (tune.crowdRatio * Math.min(gap, room)) / rest));
        const most = Math.max(size, Math.min(1, (KNOWLEDGE_UNIVERSE_CROWD.hub * room) / rest));
        const step = Math.max(0, Math.min(1, (starOf[seat * 4] - S.hubFrom) / (S.hubTo - S.hubFrom)));
        const hub = step * step * (3 - 2 * step);
        return { core: spot.r, rest, want: rest * size, whole: rest * (size + (most - size) * hub), hub, room, gap,
          capped: most < 1, lum: starOf[seat * 4] };
      };
      const gapOf = (row) => {
        universe.project();
        const m = universe.viewProjection.elements;
        const depth = Math.max(m[3] * row.x + m[7] * row.y + m[11] * row.z + m[15], row.radius * KNOWLEDGE_UNIVERSE_CROWD.near);
        const reach = (row.radius * universe.focal) / depth;
        return Math.sqrt((Math.PI * reach * reach) / row.size);
      };
      const lastOf = (row) => map.members[row.rank][map.members[row.rank].length - 1];
      const crowding = map.galaxy.filter((row) => row.large).map((row) => {
        const gap = gapOf(row);
        return { gap: universe.gaps[row.rank], wantGap: gap, ...sized(lastOf(row), gap) };
      });
      const heldCamera = { tx: universe.tx, ty: universe.ty, tz: universe.tz, dist: universe.dist };
      const big = map.galaxy[0];
      const lookFrom = (dist, seat = -1) => {
        Object.assign(universe, seat < 0 ? { tx: big.x, ty: big.y, tz: big.z, dist }
          : { tx: universe.pos3[seat * 3], ty: universe.pos3[seat * 3 + 1], tz: universe.pos3[seat * 3 + 2], dist });
        universe.camVersion += 1;
        const gap = gapOf(big);
        return { gap, ...sized(lastOf(big), gap) };
      };
      const closeUp = lookFrom(map.room[lastOf(big)] * 4, lastOf(big));
      const farOff = lookFrom(universe.home.dist * 4);
      Object.assign(universe, heldCamera);
      universe.camVersion += 1;
      /* 모든 별의 크기를 표 밖에서 다시 셈한다 — 허브는 제 크기를 지키되 형제 사이 간격의 `hub`배를 넘지 않는다. */
      let sizeWorst = 0;
      let tight = 0;
      let hubsCapped = 0;
      let hubsWhole = 0;
      for (const row of map.galaxy.filter((one) => one.large)) {
        const gap = gapOf(row);
        for (const seat of map.members[row.rank]) {
          const one = sized(seat, gap);
          sizeWorst = Math.max(sizeWorst, Math.abs(one.core - one.whole));
          if (one.room < one.gap) tight += 1;
          if (one.hub > 0 && one.capped) hubsCapped += 1;
          if (one.hub > 0 && !one.capped) hubsWhole += 1;
        }
      }
      /* 형제 사이 간격 — 표 밖에서 다시 셈한다: 부모의 자식 k가 길이 ℓ의 갓(반각 θ) 위에 서므로 ℓ sinθ √(π / k), 허브의
       * 자식은 온 구라 ℓ √(π / k). 형제가 없는 별(군집의 허브 · 떠돌이 별)은 `roomy`다. 셰이더가 읽는 속성이 그 배열이다. */
      const kidsOf = new Map();
      for (let seat = 0; seat < layout.count; seat += 1) {
        if (map.parent[seat] >= 0) kidsOf.set(map.parent[seat], (kidsOf.get(map.parent[seat]) ?? 0) + 1);
      }
      let roomWorst = 0;
      let roomed = 0;
      let roomyAgree = true;
      for (let seat = 0; seat < layout.count; seat += 1) {
        const parent = map.parent[seat];
        if (parent < 0) {
          if (map.room[seat] !== KNOWLEDGE_UNIVERSE_SHAPE.roomy) roomyAgree = false;
          continue;
        }
        const kids = kidsOf.get(parent);
        const length = Math.hypot(universe.pos3[seat * 3] - universe.pos3[parent * 3],
          universe.pos3[seat * 3 + 1] - universe.pos3[parent * 3 + 1], universe.pos3[seat * 3 + 2] - universe.pos3[parent * 3 + 2]);
        const half = Math.min(B.capMost, B.capLeast + B.capGrow * Math.sqrt(kids));
        const want = length * (map.parent[parent] < 0 ? 1 : Math.sin(half)) * Math.sqrt(Math.PI / kids);
        roomWorst = Math.max(roomWorst, Math.abs(map.room[seat] - want) / want);
        roomed += 1;
      }
      const rooms = { worst: roomWorst, roomed, roomyAgree, sizeWorst, tight, hubsCapped, hubsWhole,
        shared: universe.starGeometry.attributes.aRoom.array === map.room,
        shader: universe.materials.stars.vertexShader.includes("aRoom * uFocal") };
      /* 나무: 허브(연결이 가장 많은 쪽)가 군집의 가운데에 서고, 그 밖의 쪽은 제 군집 안에서 진짜로 이어진 부모를
       * 가지며 걸음은 부모보다 하나 깊다. 가장 먼 쪽은 반지름의 `fill`에 닿고, 넘는 쪽은 없다. 허브에서 뻗은 굵은
       * 가지(딸린 쪽이 있는 자식)와 가장 깊은 걸음을 표 밖에서 다시 센다. */
      const linked = (one, two) => {
        for (let slot = model.start[one]; slot < model.start[one + 1]; slot += 1) {
          if (model.neighbour[slot] === two) return true;
        }
        return false;
      };
      const trees = map.galaxy.map((row) => {
        const members = map.members[row.rank];
        const hub = members[0];
        const centre = [row.x, row.y, row.z];
        const apart = (seat) => Math.hypot(universe.pos3[seat * 3] - centre[0], universe.pos3[seat * 3 + 1] - centre[1],
          universe.pos3[seat * 3 + 2] - centre[2]);
        let fathered = 0;
        let stepped = 0;
        let loose = 0;
        let far = 0;
        let deepest = 0;
        let branches = 0;
        let direct = 0;
        for (const seat of members) {
          far = Math.max(far, apart(seat));
          deepest = Math.max(deepest, map.depth[seat]);
          if (seat === hub) continue;
          const parent = map.parent[seat];
          if (map.loose[seat] === 1) loose += 1;
          else if (layout.community[parent] === row.rank && linked(seat, parent)) fathered += 1;
          if (map.depth[seat] === map.depth[parent] + 1) stepped += 1;
          if (parent === hub && map.loose[seat] === 0) direct += 1;
          if (parent === hub && map.sub[seat] > 1) branches += 1;
        }
        return { rank: row.rank, size: members.length, hubMost: members.every((seat) => model.degree[seat] <= model.degree[hub]),
          hubAt: apart(hub), hubRoot: map.parent[hub] === -1 && map.depth[hub] === 0, fathered, stepped, loose,
          reach: Math.round((far / row.radius) * 1e4) / 1e4, deepest, wantDeepest: row.deepest, branches,
          wantBranches: row.branches, direct, wantDirect: row.direct, sub: map.sub[hub] };
      });
      /* 떠돌이: 이름 없는 군집의 쪽은 줄이 없다. */
      let strays = 0;
      let strayRows = 0;
      for (let seat = 0; seat < layout.count; seat += 1) {
        if (layout.community[seat] < layout.namedCount) continue;
        strays += 1;
        if (map.rowOf[seat] !== -1) strayRows += 1;
      }
      /* 별: 크기의 바탕 · 최근 · 줄 번호 · 빛의 몫(색 칸이 있는 군집 1, 없는 군집과 떠돌이 별은 `quiet-share`). */
      const star = universe.starGeometry.attributes.aStar.array;
      const sorted = Int32Array.from(model.degree).sort();
      const middle = sorted[layout.count >> 1];
      let lumWorst = 0;
      let tempRange = [1, 0];
      let unknownTemp = -1;
      let capped = 0;
      let rowsAgree = true;
      let weightsAgree = true;
      for (let seat = 0; seat < layout.count; seat += 1) {
        const ratio = (1 + model.degree[seat]) / (1 + middle);
        if (ratio > tune.starRatioCap) capped += 1;
        const want = Math.min(ratio, tune.starRatioCap) ** tune.starLumExp;
        lumWorst = Math.max(lumWorst, Math.abs(star[seat * 4] - want) / want);
        tempRange = [Math.min(tempRange[0], star[seat * 4 + 1]), Math.max(tempRange[1], star[seat * 4 + 1])];
        if (model.modified[seat] <= 0) unknownTemp = star[seat * 4 + 1];
        const row = map.rowOf[seat];
        if (star[seat * 4 + 2] !== row) rowsAgree = false;
        const weight = row >= 0 && layout.communityHue[row] >= 0 ? 1 : tune.quietShare;
        if (Math.abs(star[seat * 4 + 3] - weight) > 1e-6) weightsAgree = false;
      }
      /* 같은 볼트는 같은 우주다 — 다시 지어도 텍셀과 별 자리가 한 자도 같다. */
      const galBefore = Float32Array.from(map.gal);
      const seatsBefore = Float32Array.from(universe.pos3);
      universe.build(layout);
      const rebuilt = galBefore.every((value, index) => value === universe.map.gal[index])
        && seatsBefore.every((value, index) => value === universe.pos3[index]);
      /* 군집 빛깔(텍셀 2): 색 칸이 있으면 어두운 테마 값에서, 없으면 고정값 — 두 테마에서 같다. */
      const tintAt = (rank) => Array.from(universe.map.gal.slice((rank * KNOWLEDGE_UNIVERSE_SHAPE.texels + 2) * 4,
        (rank * KNOWLEDGE_UNIVERSE_SHAPE.texels + 2) * 4 + 3)).map((value) => Math.round(value * 1000) / 1000);
      const quietRank = layout.communityHue.findIndex((hue, rank) => hue < 0 && rank < universe.map.rows);
      const darkTints = { lit: tintAt(0), quiet: quietRank >= 0 ? tintAt(quietRank) : null };
      const darkComp = { dark: universe.post.comp.uniforms.uDark.value,
        exposure: universe.post.comp.uniforms.uExposure.value };
      setTheme("light");
      await frame();
      await frame();
      const lightTints = { lit: tintAt(0), quiet: quietRank >= 0 ? tintAt(quietRank) : null };
      const lightComp = { dark: universe.post.comp.uniforms.uDark.value,
        exposure: universe.post.comp.uniforms.uExposure.value,
        bloom: universe.post.comp.uniforms.uBloom.value, halo: universe.uniforms.uHalo.value,
        paper: universe.post.comp.uniforms.uPaper.value.toArray().map((value) => Math.round(value * 100) / 100) };
      setTheme("dark");
      await frame();
      /* 한 장의 드로우: 장면 + 번짐 내리기(단 수) + 올리기(단 수 − 1) + 합성 1. 목표는 장면 한 장과 단 수. */
      universe.invalidate();
      await frame();
      await frame();
      const draws = universe.renderer.info.render.calls;
      const sceneDraws = universe.scene.children.filter((object) => object.visible).length;
      const targets = { scene: universe.post.target !== null, mips: universe.post.mips.length,
        half: universe.post.target?.texture.type === THREE.HalfFloatType, floats: universe.floatTargets };
      /* 번짐 둘은 테마마다의 수다 — 어두운 테마로 돌아와 그린 장에서 다시 읽는다. */
      darkComp.bloom = universe.post.comp.uniforms.uBloom.value;
      darkComp.halo = universe.uniforms.uHalo.value;
      knowledgeDimension = heldDimension;
      secondBrainVault = heldVault;
      knowledgePainterKind = heldPainter;
      await paintKnowledgeView();
      return { named: layout.namedCount, rooms, sizes: Array.from(layout.communitySize),
        galaxies: map.galaxies, wantGalaxies, least, rowsCount: map.rows, rows, trees, fill: B.fill, strays, strayRows,
        scale: map.scale, scaleWant,
        lumWorst, capped, tempRange, unknownTemp, rowsAgree, weightsAgree, rebuilt, darkTints, lightTints, darkComp, lightComp,
        crowding, closeUp, farOff, crowdFloor: KNOWLEDGE_UNIVERSE_STAR.crowdSize, hubFrom: KNOWLEDGE_UNIVERSE_STAR.hubFrom,
        neutral: knowledgeUniverseInks(view, universe.probe ?? document.body).neutral, draws, sceneDraws, targets,
        bloomLevels: tune.bloomLevels };
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  }, { sizes: GALAXY_SIZES, alone: GALAXY_ALONE, quiet: GALAXY_QUIET, half: GALAXY_HALF });
  const detail = JSON.stringify(seen);
  ok("universe ②: the named clusters are rows, the front ones large (at most the token's count, at least the page floor), each with its radius from its pages and its page count beside its activity, spaced by the map's scale",
    !seen.thrown && seen.galaxies === seen.wantGalaxies && seen.galaxies === 18
      && seen.rowsCount === seen.named && seen.rows.every((row) => row.radius === row.wantRadius)
      && seen.rows.every((row) => row.large === (row.rank < seen.galaxies))
      && seen.rows.every((row) => row.texelPages === row.pages && row.pages > 0)
      && Math.abs(seen.scale - seen.scaleWant) < 1e-9,
    detail);
  ok("universe ②: crowding is measured on the screen — a cluster's gap is the side of the screen room one page has, a star that is no hub never outgrows the token's share of its room (that gap or the room among its siblings, whichever is tighter — nor shrinks below the floor), comes back to its own size when the camera closes in on it and shrinks when it backs away",
    !seen.thrown && seen.crowding.length === seen.galaxies
      && seen.crowding.every((one) => Math.abs(one.gap - one.wantGap) < one.wantGap * 1e-4 && one.lum <= seen.hubFrom
        && Math.abs(one.core - one.want) < 1e-4 && one.core <= one.rest + 1e-6)
      && Math.abs(seen.closeUp.core - seen.closeUp.rest) < 1e-4 && seen.closeUp.gap > seen.farOff.gap
      && seen.farOff.core < seen.farOff.rest - 1e-4 && seen.farOff.core >= seen.farOff.rest * seen.crowdFloor - 1e-6
      && Math.abs(seen.farOff.core - seen.farOff.want) < 1e-4,
    detail);
  ok("universe ②: a page's room among its siblings is what a recount of the tree finds — the cap its parent spreads its children on, seen from the front and shared among them — a cluster's hub and a stray star have none, and the shader reads the very array the picker does",
    !seen.thrown && seen.rooms.roomed > 0 && seen.rooms.worst < 5e-3 && seen.rooms.roomyAgree && seen.rooms.shared
      && seen.rooms.shader,
    detail);
  ok("universe ②: every star of a large cluster is the size a recount finds — small where its room is tight, and a hub keeps its own size only as far as its room among its siblings lets it",
    !seen.thrown && seen.rooms.sizeWorst < 1e-4 && seen.rooms.tight > 0 && seen.rooms.hubsCapped + seen.rooms.hubsWhole > 0,
    detail);
  ok("universe ②: a cluster is its hub's tree — the most linked page stands at the centre, every other page hangs from a page it is really linked to inside the cluster, one step deeper, and the farthest page reaches the fill of the radius",
    !seen.thrown && seen.trees.length === seen.rowsCount
      && seen.trees.every((tree) => tree.hubMost && tree.hubRoot && tree.hubAt < 1e-3
        && tree.fathered + tree.loose === tree.size - 1 && tree.stepped === tree.size - 1 && tree.sub === tree.size
        && Math.abs(tree.reach - seen.fill) < 2e-3)
      && seen.trees.some((tree) => tree.deepest >= 2),
    detail);
  ok("universe ②: the tree's own words — the pages linked straight to the hub, those that branch on, and the deepest step — are what a recount finds",
    !seen.thrown && seen.trees.every((tree) => tree.deepest === tree.wantDeepest && tree.branches === tree.wantBranches
      && tree.direct === tree.wantDirect && tree.direct >= 1)
      && seen.trees.some((tree) => tree.branches >= 1),
    detail);
  ok("universe ②: a page of an unnamed cluster is a stray star with no row",
    !seen.thrown && seen.strays > 0 && seen.strayRows === 0, detail);
  ok("universe ②: a star's size is its links over the median, capped at the token's ratio, to the token's power; its recency its last edit (an unknown edit reads as the oldest); it carries its row and its share of light",
    !seen.thrown && seen.capped >= 1 && seen.lumWorst < 1e-5 && seen.tempRange[0] >= 0 && seen.tempRange[1] <= 1
      && seen.tempRange[1] > 0.9 && seen.unknownTemp === 0 && seen.rowsAgree && seen.weightsAgree,
    detail);
  ok("universe ②: the same vault builds the same universe — every cluster texel and every star seat",
    !seen.thrown && seen.rebuilt, detail);
  ok("universe ②: a cluster's tint is the same in both themes — its hue's dark value, or the neutral tint for a cluster without a hue — while the composite turns to the light theme's exposure, paper and bloom, and the stars to its share of halo",
    !seen.thrown && JSON.stringify(seen.darkTints) === JSON.stringify(seen.lightTints)
      && (seen.darkTints.quiet === null || seen.darkTints.quiet.join(",") === seen.neutral.join(","))
      && seen.darkComp.dark === 1 && seen.darkComp.exposure === 1 && seen.darkComp.bloom === 0.2 && seen.darkComp.halo === 1
      && seen.lightComp.dark === 0 && seen.lightComp.exposure === 0.9 && seen.lightComp.bloom === 0.08
      && seen.lightComp.halo === 0.3
      && seen.lightComp.paper.join(",") === "0.8,0.77,0.7",
    detail);
  ok("universe ②: one frame is the scene plus the bloom ladder down and up and the composite, into one scene target and the token's levels",
    !seen.thrown && seen.sceneDraws >= 1 && seen.draws === seen.sceneDraws + seen.bloomLevels + (seen.bloomLevels - 1) + 1
      && seen.targets.scene && seen.targets.mips === seen.bloomLevels
      && seen.targets.half === seen.targets.floats,
    detail);
}

/* ---- ③ 링크의 두 통 — 가지 · 현 · 올, 그리고 둘레 빛과 배경 별 ------------------------------------
 *
 * 모든 링크가 그림의 어느 한 통에 든다: 군집 안의 링크와 떠돌이 별에 닿는 링크는 곧은 통(끝점 둘), 서로 다른 두
 * 군집을 잇는 링크는 다발의 올(인스턴스 하나). 곧은 통에서 나무의 부모–자식을 잇는 것만 쉴 때 보이는 가지이고
 * (자식마다 하나), 나머지는 현이다(무게 음수). 한 장의 장면은 둘레 빛 · 배경 별 · 곧은 링크 · 올 · 별의 다섯이다. */
export async function testKnowledgeUniverseBodies(page, ok) {
  await page.emulateMedia({ reducedMotion: "reduce" });
  const seen = await page.evaluate(async ({ sizes, alone, galaxySizes }) => {
    try {
      const view = document.querySelector(".knowledge-view:not([hidden])");
      const frame = () => new Promise((done) => requestAnimationFrame(done));
      const until = async (wanted, rounds = 900) => {
        for (let round = 0; round < rounds; round += 1) {
          if (wanted()) return true;
          await frame();
        }
        return false;
      };
      const heldDimension = knowledgeDimension;
      const heldVault = secondBrainVault;
      const edges = [];
      const starts = [];
      let at = 0;
      sizes.forEach((size) => { starts.push(at); at += size; });
      /* 큰 무리는 이웃 무리와 한 가닥으로 잇고, 작은 무리는 홀로 선 완전 그래프로 둔다 — 서로 이으면 평면 지도가
       * 작은 무리들을 한 군집으로 합친다. */
      sizes.forEach((size, group) => {
        const first = starts[group];
        const small = group >= galaxySizes;
        for (let step = 1; step < size; step += 1) {
          edges.push({ from: first + step, to: first + step - 1, kind: "mentions" });
          if (small) {
            for (let back = 0; back < step - 1; back += 1) edges.push({ from: first + step, to: first + back, kind: "mentions" });
          } else if (step > 2) {
            edges.push({ from: first + step, to: first + ((step * 7) % (step - 1)), kind: "mentions" });
          }
        }
        if (group > 0 && !small) edges.push({ from: first, to: starts[group - 1], kind: "mentions" });
      });
      const answer = window.__buildVaultGraph__({ path: "/bodies", sources: false },
        { pages: at + alone, ghosts: 0, tags: [], customEdges: edges });
      noteKnowledgeExploreLines({});
      secondBrainVault = answer.vault;
      knowledgeDimension = "3d";
      setKnowledgeMode(view, "global", { paint: false });
      knowledgeReport = answer;
      await paintKnowledgeView();
      await until(() => (knowledgeLayouts.get(view)?.left ?? 1) === 0);
      await paintKnowledgeView();
      await frame();
      const layout = knowledgeLayouts.get(view);
      const model = layout.model;
      const universe = knowledgeUniverses.get(view);
      const map = universe.map;
      const links = universe.edges;
      const tune = knowledgeUniverseTuning(view);
      const E = KNOWLEDGE_UNIVERSE_EDGE;
      const keyed = (key) => universe.scene.children.find((object) => object.userData.key === key) ?? null;
      /* 통을 표 밖에서 다시 센다. */
      let both = 0;
      let neither = 0;
      let wrongBin = 0;
      let crossing = 0;
      for (let edge = 0; edge < model.edgeCount; edge += 1) {
        if (layout.drawnEdge !== null && layout.drawnEdge[edge] === 0) continue;
        const thread = links.threadOf[edge] >= 0;
        const fiber = links.fiberOf[edge] >= 0;
        if (thread && fiber) both += 1;
        if (!thread && !fiber) neither += 1;
        const a = map.rowOf[model.from[edge]];
        const b = map.rowOf[model.to[edge]];
        const across = a >= 0 && b >= 0 && a !== b;
        if (across) crossing += 1;
        if (across !== fiber) wrongBin += 1;
      }
      /* 가지: 자식마다 꼭 하나, 부모와 자식을 잇는 진짜 링크 위에, 부모 쪽 끝이 더 밝다. 현은 두 끝이 음수다. */
      const seat = links.seat;
      const claimed = new Int32Array(layout.count);
      let branchWrong = 0;
      let brighterParent = 0;
      let branches = 0;
      let chords = 0;
      for (let edge = 0; edge < model.edgeCount; edge += 1) {
        const slot = links.threadOf[edge];
        if (slot < 0) continue;
        const from = seat[slot * 6];
        const to = seat[slot * 6 + 3];
        const fromWeight = seat[slot * 6 + 2];
        const toWeight = seat[slot * 6 + 5];
        if (from !== model.from[edge] || to !== model.to[edge]) branchWrong += 1;
        if (fromWeight < 0 && toWeight < 0) {
          chords += 1;
          continue;
        }
        branches += 1;
        const child = map.parent[to] === from ? to : map.parent[from] === to ? from : -1;
        if (child < 0 || map.loose[child] === 1 || fromWeight < 0 || toWeight < 0) {
          branchWrong += 1;
          continue;
        }
        claimed[child] += 1;
        const parentWeight = child === to ? fromWeight : toWeight;
        const childWeight = child === to ? toWeight : fromWeight;
        if (parentWeight > childWeight && childWeight >= E.twigFrom - 1e-6 && parentWeight <= E.trunkFrom + E.trunkGrow + 1e-6) {
          brighterParent += 1;
        }
      }
      let wantBranches = 0;
      let claimedOnce = true;
      for (const row of map.galaxy) {
        for (const member of map.members[row.rank]) {
          if (member === row.hub || map.loose[member] === 1) {
            if (claimed[member] !== 0) claimedOnce = false;
            continue;
          }
          wantBranches += 1;
          if (claimed[member] !== 1) claimedOnce = false;
        }
      }
      const aura = keyed("aura");
      const sky = keyed("sky");
      const threads = keyed("threads");
      const fibers = keyed("fibers");
      /* 통의 앞쪽 — 가지가 곧은 링크의 앞, 굵은 다발의 올이 올의 앞이다. 쉬는 판은 그 앞쪽만 싣고(현과 그 밖의 올은
       * 빛이 없다), 별 하나를 올리면 그 링크가 켜져야 하므로 두 통을 다 싣는다. */
      let frontWrong = 0;
      for (let slot = 0; slot < links.threads; slot += 1) {
        if ((seat[slot * 6 + 2] >= 0) !== (slot < links.branches)) frontWrong += 1;
      }
      for (let slot = 0; slot < links.fibers; slot += 1) {
        if ((links.fiber[slot * 4 + 2] >= 0) !== (slot < links.thick)) frontWrong += 1;
      }
      const span = () => ({ threads: threads?.geometry.drawRange.count ?? -1, fibers: fibers?.geometry.instanceCount ?? -1,
        nearest: universe.nearest, closeFar: E.closeFar * universe.uniforms.uRef.value });
      /* 카메라를 처음 거리의 몫으로 옮겨 한 장을 그린다 — 물러선 판에는 문턱 안에 든 별이 없고, 별들 사이로 들어간
       * 판에는 있다. */
      const pull = async (share) => {
        universe.tween.on = false;
        universe.dist = universe.home.dist * share;
        universe.camVersion += 1;
        universe.invalidate();
        await frame();
        await frame();
      };
      await pull(1.6);
      const restSpan = span();
      universe.setHover(map.members[0][1]);
      await frame();
      await frame();
      const litSpan = span();
      universe.setHover(-1);
      await frame();
      await frame();
      const backSpan = span();
      const draws = universe.renderer.info.render.calls;
      await pull(0.2);
      const closeSpan = span();
      await pull(1);
      const orders = universe.scene.children.map((object) => `${object.userData.key}:${object.renderOrder}`).join(" ");
      knowledgeDimension = heldDimension;
      secondBrainVault = heldVault;
      await paintKnowledgeView();
      return { edges: model.edgeCount, threads: links.threads, fibers: links.fibers, both, neither, wrongBin, crossing,
        branches, saidBranches: links.branches, wantBranches, chords, branchWrong, brighterParent, claimedOnce,
        auraInstances: aura?.geometry.instanceCount ?? -1, rows: map.rows,
        skyCount: sky?.geometry.attributes.position.count ?? -1, decorSky: tune.decorSky,
        threadVertices: threads?.geometry.attributes.position.count ?? -1,
        litVertices: threads?.geometry.attributes.aLit.count ?? -1,
        thick: links.thick, frontWrong, restSpan, litSpan, backSpan, closeSpan,
        fiberSegments: (fibers?.geometry.attributes.position.count ?? 0) / 2, wantSegments: KNOWLEDGE_UNIVERSE_SHAPE.fiberSegments,
        draws, orders, bloomLevels: tune.bloomLevels };
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  }, { sizes: [...GALAXY_SIZES, 6, 5, 5, 4, 4, 4, 3, 3, 3, 3, 3, 3, 3, 3], alone: GALAXY_ALONE,
    galaxySizes: GALAXY_SIZES.length });
  const detail = JSON.stringify(seen);
  ok("universe ③: every link is in exactly one bin — a link between two clusters is a fibre, any other a straight thread",
    !seen.thrown && seen.threads + seen.fibers === seen.edges && seen.both === 0 && seen.neither === 0
      && seen.wrongBin === 0 && seen.fibers === seen.crossing && seen.crossing > 0
      && seen.threadVertices === seen.threads * 2 && seen.litVertices === seen.threads * 2
      && seen.fiberSegments === seen.wantSegments,
    detail);
  ok("universe ③: a resting far view loads only what shines — the branches and the thick bundles' fibres stand first in their bins — a pointed star loads both bins whole so its own links can light, and a camera among the stars loads every thread so the chords can show",
    !seen.thrown && seen.frontWrong === 0 && seen.thick <= seen.fibers
      && seen.restSpan.nearest >= seen.restSpan.closeFar
      && seen.restSpan.threads === seen.branches * 2 && seen.restSpan.fibers === seen.thick
      && seen.litSpan.threads === seen.threads * 2 && seen.litSpan.fibers === seen.fibers
      && seen.backSpan.threads === seen.branches * 2 && seen.backSpan.fibers === seen.thick
      && seen.closeSpan.nearest < seen.closeSpan.closeFar && seen.closeSpan.threads === seen.threads * 2
      && seen.closeSpan.fibers === seen.thick,
    detail);
  ok("universe ③: the branches are the tree — one real link for every page that hangs from a parent, brighter at the parent's end — and every other thread is a chord that rests dark",
    !seen.thrown && seen.branches === seen.wantBranches && seen.saidBranches === seen.branches && seen.claimedOnce
      && seen.branchWrong === 0 && seen.brighterParent === seen.branches && seen.chords === seen.threads - seen.branches
      && seen.chords > 0,
    detail);
  ok("universe ③: one frame is five scene draws — a glow for every row, the token's sky, the threads, the fibres (none when no fibre is loaded) and the stars, in that order — then the bloom ladder and the composite",
    !seen.thrown && seen.auraInstances === seen.rows && seen.skyCount === seen.decorSky
      && seen.draws === 5 - (seen.backSpan.fibers === 0 ? 1 : 0) + seen.bloomLevels + (seen.bloomLevels - 1) + 1
      && seen.orders === "aura:0 sky:1 threads:2 fibers:3 stars:4",
    detail);
}

/* ---- ③-나 가려진 쪽 — 평면 지도가 접은 목차 · 일지는 우주에도 서지 않는다 ------------------------------
 *
 * `wiki/index.md`와 `wiki/log.md`는 볼트의 거의 모든 쪽을 가리킨다. 평면 지도는 그 둘을 접어 두고(「목차·일지
 * 표시」를 켜기 전까지), 우주도 같은 가림을 따른다: 그 별은 빛이 없고 고를 수 없으며, 그 링크는 어느 통에도
 * 없어 올려도 켜지지 않는다. 가림을 풀면 같은 우주가 다시 지어져 그 둘과 그 링크가 선다. */
export async function testKnowledgeUniverseNav(page, ok) {
  await page.emulateMedia({ reducedMotion: "reduce" });
  const seen = await page.evaluate(async () => {
    try {
      const view = document.querySelector(".knowledge-view:not([hidden])");
      const frame = () => new Promise((done) => requestAnimationFrame(done));
      const until = async (wanted, rounds = 900) => {
        for (let round = 0; round < rounds; round += 1) {
          if (wanted()) return true;
          await frame();
        }
        return false;
      };
      const heldDimension = knowledgeDimension;
      const heldVault = secondBrainVault;
      const heldNav = knowledgeNavShown;
      /* 무리 셋(40 · 30 · 20쪽, 안은 사슬과 허브) 뒤에 목차와 일지 — 둘 다 모든 쪽을 가리킨다. */
      const sizes = [40, 30, 20];
      const starts = [0, 40, 70];
      const pages = 90;
      const edges = [];
      const ids = [];
      sizes.forEach((size, group) => {
        const first = starts[group];
        for (let step = 1; step < size; step += 1) {
          edges.push({ from: first + step, to: first, kind: "mentions" });
          if (step > 1) edges.push({ from: first + step, to: first + step - 1, kind: "mentions" });
        }
      });
      for (let one = 0; one < 9; one += 1) edges.push({ from: 5 + one, to: 45 + one, kind: "mentions" });
      for (let at = 0; at < pages; at += 1) {
        edges.push({ from: pages, to: at, kind: "mentions" });
        edges.push({ from: pages + 1, to: at, kind: "mentions" });
      }
      ids[pages] = "wiki/index.md";
      ids[pages + 1] = "wiki/log.md";
      const answer = window.__buildVaultGraph__({ path: "/nav", sources: false },
        { pages: pages + 2, ghosts: 0, tags: [], customEdges: edges, ids });
      noteKnowledgeExploreLines({});
      secondBrainVault = answer.vault;
      knowledgeDimension = "3d";
      knowledgeNavShown = false;
      knowledgeQuery = "";
      knowledgeSelectedKey = null;
      knowledgeClusterPicked = -1;
      knowledgePath = null;
      setKnowledgeMode(view, "global", { paint: false });
      knowledgeReport = answer;
      await paintKnowledgeView();
      await until(() => (knowledgeLayouts.get(view)?.left ?? 1) === 0);
      await paintKnowledgeView();
      await frame();
      const read = () => {
        const layout = knowledgeLayouts.get(view);
        const model = layout.model;
        const universe = knowledgeUniverses.get(view);
        const map = universe.map;
        const links = universe.edges;
        const nav = ["wiki/index.md", "wiki/log.md"].map((key) => model.keys.indexOf(key));
        const star = universe.starGeometry.attributes.aStar.array;
        universe.project();
        let navLinks = 0;
        let navSeated = 0;
        let seated = 0;
        for (let edge = 0; edge < model.edgeCount; edge += 1) {
          const here = links.threadOf[edge] >= 0 || links.fiberOf[edge] >= 0;
          if (here) seated += 1;
          if (nav.includes(model.from[edge]) || nav.includes(model.to[edge])) {
            navLinks += 1;
            if (here) navSeated += 1;
          }
        }
        /* 목차를 올려도 켜지는 링크가 없다(가려진 동안) — 풀면 그 링크가 다 켜진다. */
        universe.setHover(nav[0]);
        const lit = universe.litCount;
        universe.setHover(-1);
        return { universe, nav, navLinks, navSeated, seated, edges: model.edgeCount, lit,
          weights: nav.map((seat) => star[seat * 4 + 3]), rows: nav.map((seat) => map.rowOf[seat]),
          depths: nav.map((seat) => universe.proj[seat * 4 + 3]),
          filaments: map.filaments.map((one) => one.n), stamp: map.stamp,
          named: map.rows, members: map.members.reduce((sum, list) => sum + list.length, 0) };
      };
      const folded = read();
      const standing = folded.universe;
      knowledgeNavShown = true;
      await paintKnowledgeView();
      await frame();
      const open = read();
      const same = open.universe === standing;
      delete folded.universe;
      delete open.universe;
      knowledgeNavShown = heldNav;
      knowledgeDimension = heldDimension;
      secondBrainVault = heldVault;
      await paintKnowledgeView();
      return { folded, open, same };
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  });
  const detail = JSON.stringify(seen);
  ok("universe ③: the pages the flat map folds away (index and log) are not in the universe — their stars carry no light, no row and cannot be picked, and their links sit in no bin and never light",
    !seen.thrown && seen.folded.nav.every((seat) => seat >= 0) && seen.folded.navLinks >= 180
      && seen.folded.navSeated === 0 && seen.folded.seated === seen.folded.edges - seen.folded.navLinks
      && seen.folded.weights.every((weight) => weight === 0) && seen.folded.rows.every((row) => row === -1)
      && seen.folded.depths.every((depth) => depth === -1) && seen.folded.lit === 0
      && seen.folded.filaments.length === 1 && seen.folded.filaments[0] === 9,
    detail);
  ok("universe ③: unfolding them rebuilds the same universe with those pages and every one of their links in it",
    !seen.thrown && seen.same && seen.open.stamp !== seen.folded.stamp && seen.open.seated === seen.open.edges
      && seen.open.navSeated === seen.open.navLinks && seen.open.weights.every((weight) => weight > 0)
      && seen.open.depths.some((depth) => depth > 0) && seen.open.lit >= 90,
    detail);
}

/* ---- ④ 필라멘트 — 군집 사이 링크의 다발 ------------------------------------------------------------
 *
 * 두 큰 군집 사이의 링크(선언·추론 모두)를 보낸 쪽 → 받는 쪽으로 세고, 합이 문턱(8) 이상인 쌍을 굵은 차례로 48까지
 * 필라멘트로 세운다. 빛은 많이 보낸 쪽에서 흐른다. 그 쌍의 올은 쉴 때 보이고(링크가 많은 다발의 올일수록 가늘다),
 * 문턱 밑 쌍의 올은 쉴 때 꺼져 있다(무게 음수). 한 쌍의 올은 같은 휨을 지닌다. 볼트: 촘촘한 무리 넷
 * (80·64·52·40쪽), A→B 18·B→A 6, B→C 9(C→B 0), A→D 12(모두 D→A), C→D 5(문턱 밑). */
export async function testKnowledgeUniverseFilaments(page, ok) {
  await page.emulateMedia({ reducedMotion: "reduce" });
  const seen = await page.evaluate(async () => {
    try {
      const view = document.querySelector(".knowledge-view:not([hidden])");
      const frame = () => new Promise((done) => requestAnimationFrame(done));
      const until = async (wanted, rounds = 900) => {
        for (let round = 0; round < rounds; round += 1) {
          if (wanted()) return true;
          await frame();
        }
        return false;
      };
      const heldDimension = knowledgeDimension;
      const heldVault = secondBrainVault;
      const sizes = [80, 64, 52, 40];
      const starts = [0, 80, 144, 196];
      const edges = [];
      sizes.forEach((size, group) => {
        const first = starts[group];
        for (let step = 1; step < size; step += 1) {
          for (let back = Math.max(0, step - 6); back < step; back += 1) {
            edges.push({ from: first + step, to: first + back, kind: "mentions" });
          }
        }
      });
      const cross = (from, to, count) => {
        for (let one = 0; one < count; one += 1) {
          edges.push({ from: starts[from] + 10 + one, to: starts[to] + 20 + one, kind: one % 3 === 0 ? "related" : "mentions" });
        }
      };
      cross(0, 1, 18);
      cross(1, 0, 6);
      cross(1, 2, 9);
      cross(3, 0, 12);
      cross(2, 3, 5);
      const answer = window.__buildVaultGraph__({ path: "/filaments", sources: false },
        { pages: 236, ghosts: 0, tags: [], customEdges: edges });
      noteKnowledgeExploreLines({});
      secondBrainVault = answer.vault;
      knowledgeDimension = "3d";
      setKnowledgeMode(view, "global", { paint: false });
      knowledgeReport = answer;
      await paintKnowledgeView();
      await until(() => (knowledgeLayouts.get(view)?.left ?? 1) === 0);
      await paintKnowledgeView();
      await frame();
      const layout = knowledgeLayouts.get(view);
      const model = layout.model;
      const universe = knowledgeUniverses.get(view);
      const map = universe.map;
      const links = universe.edges;
      const tune = knowledgeUniverseTuning(view);
      const E = KNOWLEDGE_UNIVERSE_EDGE;
      /* 표 밖에서 다시 센다: 큰 군집 줄 사이의 링크를 보낸 쪽 → 받는 쪽으로. */
      const pairs = new Map();
      for (let at = 0; at < model.edgeCount; at += 1) {
        const a = map.rowOf[model.from[at]];
        const b = map.rowOf[model.to[at]];
        if (a < 0 || b < 0 || a === b || a >= map.galaxies || b >= map.galaxies) continue;
        const key = Math.min(a, b) * 1000 + Math.max(a, b);
        const held = pairs.get(key) ?? { a: Math.min(a, b), b: Math.max(a, b), ab: 0, ba: 0, n: 0 };
        if (a < b) held.ab += 1; else held.ba += 1;
        held.n += 1;
        pairs.set(key, held);
      }
      const want = [...pairs.values()].filter((pair) => pair.n >= tune.filamentLeast).sort((x, y) => y.n - x.n)
        .slice(0, tune.filamentMost);
      const most = want[0]?.n ?? 1;
      const said = (universe.filaments ?? []).map((one) => ({ from: one.from, to: one.to, n: one.n, sent: one.sent,
        strength: Math.round(one.strength * 1e4) / 1e4 }));
      const wanted = want.map((pair) => {
        const forward = pair.ab >= pair.ba;
        return { from: forward ? pair.a : pair.b, to: forward ? pair.b : pair.a, n: pair.n, sent: Math.max(pair.ab, pair.ba),
          strength: Math.round(Math.sqrt(pair.n / most) * 1e4) / 1e4 };
      });
      /* 올: 굵은 다발의 올은 가늚(clamp((fiberFull / n)^fiberFade, fiberLeast, 1)) — 링크가 fiberMost를 넘는 다발은 그
       * 가운데 fiberMost올만 빛을 들고 다발 전체의 빛(n × 가늚)을 나눠 든다 —, 문턱 밑 쌍의 올과 뽑히지 않은 올은 음수.
       * 한 쌍의 올은 같은 휨. */
      const countOf = new Map(want.map((pair) => [pair.a * 1000 + pair.b, pair.n]));
      const pickedOf = new Map();
      let weightWrong = 0;
      let bendWrong = 0;
      let endsWrong = 0;
      let shown = 0;
      let hidden = 0;
      for (let at = 0; at < model.edgeCount; at += 1) {
        const slot = links.fiberOf[at];
        if (slot < 0) continue;
        const a = map.rowOf[model.from[at]];
        const b = map.rowOf[model.to[at]];
        const low = Math.min(a, b);
        const high = Math.max(a, b);
        const linkCount = countOf.get(low * 1000 + high);
        const weight = links.fiber[slot * 4 + 2];
        if (linkCount === undefined) {
          hidden += 1;
          if (weight >= 0) weightWrong += 1;
        } else if (weight < 0) {
          hidden += 1;
          if (linkCount <= E.fiberMost) weightWrong += 1;
        } else {
          shown += 1;
          pickedOf.set(low * 1000 + high, (pickedOf.get(low * 1000 + high) ?? 0) + 1);
          const thinness = Math.max(E.fiberLeast, Math.min(1, (E.fiberFull / linkCount) ** E.fiberFade));
          if (Math.abs(weight - (linkCount * thinness) / Math.min(linkCount, E.fiberMost)) > 1e-5) weightWrong += 1;
        }
        if (Math.abs(links.fiber[slot * 4 + 3] - knowledgeUniverseBend(low, high)) > 1e-6) bendWrong += 1;
        if (links.fiber[slot * 4] !== model.from[at] || links.fiber[slot * 4 + 1] !== model.to[at]) endsWrong += 1;
      }
      for (const [key, linkCount] of countOf) {
        if ((pickedOf.get(key) ?? 0) !== Math.min(linkCount, E.fiberMost)) weightWrong += 1;
      }
      /* 링크가 fiberMost를 넘는 다발 — 지어낸 작은 모델로 잰다(두 군집의 쪽 둘 사이에 링크 n개): 빛을 드는 올은
       * fiberMost개, 차례대로 고르게 뽑히고, 그 빛의 합은 다발 전체의 빛(n × 가늚)이다. 나머지는 음수로 뒤에 선다. */
      const many = E.fiberMost * 3 + 5;
      const bundle = knowledgeUniverseEdges(
        { edgeCount: many, from: new Int32Array(many), to: new Int32Array(many).fill(1), provenance: new Uint8Array(many),
          kind: new Uint8Array(many) },
        { rowOf: Int32Array.of(0, 1), filaments: [{ a: 0, b: 1, n: many }], shownEdge: null, parent: Int32Array.of(-1, -1),
          loose: new Uint8Array(2), sub: Int32Array.of(1, 1), galaxy: [{ size: 1 }, { size: 1 }] });
      const pickedAt = [];
      for (let at = 0; at < many; at += 1) if (bundle.fiberOf[at] < bundle.thick) pickedAt.push(at);
      let light = 0;
      let dark = 0;
      for (let slot = 0; slot < bundle.fibers; slot += 1) {
        if (bundle.fiber[slot * 4 + 2] >= 0) light += bundle.fiber[slot * 4 + 2];
        else dark += 1;
      }
      const sampled = { many, most: E.fiberMost, thick: bundle.thick, fibers: bundle.fibers, dark, light,
        wantLight: many * Math.max(E.fiberLeast, Math.min(1, (E.fiberFull / many) ** E.fiberFade)),
        widestGap: pickedAt.reduce((widest, at, nth) => (nth === 0 ? widest : Math.max(widest, at - pickedAt[nth - 1])), 0),
        darkBehind: Array.from({ length: bundle.fibers - bundle.thick },
          (unused, back) => bundle.fiber[(bundle.thick + back) * 4 + 2]).every((weight) => weight < 0) };
      /* 다발의 등뼈 — 이름표가 서는 길: 0에서 보낸 군집의 가운데, 1에서 받는 군집의 가운데. */
      const first = universe.filaments[0];
      const one = map.galaxy[first.from];
      const two = map.galaxy[first.to];
      const apart = (point, row) => Math.hypot(point[0] - row.x, point[1] - row.y, point[2] - row.z);
      const spine = { start: apart(knowledgeUniverseFilamentPoint(one, two, first.bend, 0), one),
        end: apart(knowledgeUniverseFilamentPoint(one, two, first.bend, 1), two),
        middle: knowledgeUniverseFilamentPoint(one, two, first.bend, 0.5).every((value) => Number.isFinite(value)),
        tags: universe.flowTags.length, wantTags: Math.min(tune.flowTags, universe.filaments.length) };
      universe.invalidate();
      await frame();
      await frame();
      const draws = universe.renderer.info.render.calls;
      knowledgeDimension = heldDimension;
      secondBrainVault = heldVault;
      await paintKnowledgeView();
      return { galaxies: map.galaxies, sizes: Array.from(layout.communitySize), said, wanted, weightWrong, bendWrong, endsWrong,
        shown, hidden, spine, sampled, draws, bloomLevels: tune.bloomLevels };
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  });
  const detail = JSON.stringify(seen);
  ok("universe ④: a bundle of more links than the table's most carries its light on that many fibres, picked evenly in link order — together as bright as the whole bundle — and the rest stand dark behind them",
    !seen.thrown && seen.sampled.thick === seen.sampled.most && seen.sampled.fibers === seen.sampled.many
      && seen.sampled.dark === seen.sampled.many - seen.sampled.most && seen.sampled.darkBehind
      && Math.abs(seen.sampled.light - seen.sampled.wantLight) < 1e-3
      && seen.sampled.widestGap <= Math.ceil(seen.sampled.many / seen.sampled.most),
    detail);
  ok("universe ④: large-cluster pairs of eight links or more become filaments, thickest first, flowing from the side that sent more",
    !seen.thrown && seen.galaxies === 4 && seen.wanted.length === 3
      && JSON.stringify(seen.said) === JSON.stringify(seen.wanted),
    detail);
  ok("universe ④: a filament's fibres are its own links — shown at rest, thinner the more links the bundle holds — a pair under the floor keeps its fibres dark, and every fibre of a pair bends the same way",
    !seen.thrown && seen.shown === seen.wanted.reduce((sum, one) => sum + one.n, 0) && seen.hidden === 5
      && seen.weightWrong === 0 && seen.bendWrong === 0 && seen.endsWrong === 0
      && seen.draws === 5 + seen.bloomLevels + (seen.bloomLevels - 1) + 1,
    detail);
  ok("universe ④: a bundle's spine runs from the sending cluster's centre to the receiving cluster's, and the thickest bundles carry their count on it",
    !seen.thrown && seen.spine.start < 1e-6 && seen.spine.end < 1e-6 && seen.spine.middle
      && seen.spine.tags === seen.spine.wantTags && seen.spine.tags >= 1,
    detail);
}

/* ---- ⑤ 고르기·초점·켜지는 링크·찾기·경로 -----------------------------------------------------------
 *
 * 우주의 초점은 평면 지도와 같은 상태(고른 쪽·고른 군집·찾기·시간 창·경로)에서 나오고, 올림은 우주만의 것이다.
 * 별의 상태(0 보통 · 1 이웃/고른 군집/창 안 · 2 올림/고름/경로 · 3 찾은 것)와 초점 군집, 초점 쪽의 링크만 켜지는가
 * (곧은 통의 끝점 둘 또는 올 하나), 그리고 카메라가 그 별·군집으로 나는지를 묻는다(움직임을 줄인 판이라 비행은
 * 곧바로 끝난다). 고른 쪽이 있을 때 Shift로 누른 다른 별은 경로다 — 평면 지도의 손 그대로. */
export async function testKnowledgeUniverseFocus(page, ok) {
  await page.emulateMedia({ reducedMotion: "reduce" });
  const seen = await page.evaluate(async () => {
    try {
      const view = document.querySelector(".knowledge-view:not([hidden])");
      const frame = () => new Promise((done) => requestAnimationFrame(done));
      const frames = async (count) => {
        for (let at = 0; at < count; at += 1) await frame();
      };
      const until = async (wanted, rounds = 900) => {
        for (let round = 0; round < rounds; round += 1) {
          if (wanted()) return true;
          await frame();
        }
        return false;
      };
      const heldDimension = knowledgeDimension;
      const heldVault = secondBrainVault;
      const sizes = [60, 48, 36, 24];
      const starts = [0, 60, 108, 144];
      const edges = [];
      const titles = [];
      sizes.forEach((size, group) => {
        const first = starts[group];
        for (let step = 0; step < size; step += 1) titles.push(`별 ${group}-${step}`);
        for (let step = 1; step < size; step += 1) {
          edges.push({ from: first + step, to: first, kind: step % 4 === 0 ? "related" : "mentions" });
          if (step > 1) edges.push({ from: first + step, to: first + step - 1, kind: "mentions" });
        }
      });
      for (let one = 0; one < 10; one += 1) edges.push({ from: 10 + one, to: 70 + one, kind: "mentions" });
      titles[150] = "찾을 별 하나";
      const answer = window.__buildVaultGraph__({ path: "/focus", sources: false },
        { pages: 168, ghosts: 0, tags: [], customEdges: edges, titles });
      /* 경로의 대역(`second_brain_paths`)은 마지막으로 지은 답 위를 걷는다. */
      window.__GRAPH_BUILT__ = answer;
      noteKnowledgeExploreLines({});
      secondBrainVault = answer.vault;
      knowledgeDimension = "3d";
      knowledgeQuery = "";
      knowledgeSelectedKey = null;
      knowledgeClusterPicked = -1;
      knowledgeSlicerCutoff = 0;
      knowledgePath = null;
      setKnowledgeMode(view, "global", { paint: false });
      knowledgeReport = answer;
      await paintKnowledgeView();
      await until(() => (knowledgeLayouts.get(view)?.left ?? 1) === 0);
      await paintKnowledgeView();
      await frames(2);
      const layout = knowledgeLayouts.get(view);
      const model = layout.model;
      const universe = knowledgeUniverses.get(view);
      const map = universe.map;
      const tune = knowledgeUniverseTuning(view);
      const state = () => universe.starGeometry.attributes.aState.array;
      /* 켜진 링크의 수 — 곧은 통은 끝점 둘이 함께, 올은 인스턴스 하나가 링크 하나다. */
      const lit = (value) => {
        let count = 0;
        for (let slot = 0; slot < universe.edges.threads; slot += 1) {
          if (universe.threadLit[slot * 2] === value && universe.threadLit[slot * 2 + 1] === value) count += 1;
        }
        for (let slot = 0; slot < universe.edges.fibers; slot += 1) if (universe.fiberLit[slot] === value) count += 1;
        return count;
      };
      const hub = model.keys.indexOf(answer.graph.nodes[starts[1]].id);
      const neighbours = new Set();
      for (let slot = model.start[hub]; slot < model.start[hub + 1]; slot += 1) neighbours.add(model.neighbour[slot]);
      const stranger = model.keys.indexOf(answer.graph.nodes[starts[3] + 5].id);
      /* 고르기: 투영한 자리에서 그 별이 잡히고, 빈 구석은 아무것도 아니다. */
      const spot = universe.project(hub);
      const picked = universe.pickAt(spot.x, spot.y);
      const nothing = universe.pickAt(1, 1);
      /* 올림: 그 별 2, 이웃 1, 남 0, 초점과 초점 은하, 그 별의 선만. */
      universe.setHover(hub);
      await frame();
      const hovered = { hub: state()[hub], neighboursLit: [...neighbours].every((seat) => state()[seat] === 1),
        stranger: state()[stranger], focus: universe.uniforms.uFocus.value, galaxy: universe.uniforms.uFocusGal.value,
        row: map.rowOf[hub], lines: lit(1), page: universe.uniforms.uFocusPage.value,
        gain: universe.uniforms.uLitGain.value,
        wantGain: Math.max(tune.litLeast, Math.min(1, (tune.litFull / model.degree[hub]) ** tune.litFade)),
        degree: model.degree[hub] };
      universe.setHover(-1);
      await frame();
      const rested = { hub: state()[hub], focus: universe.uniforms.uFocus.value, lines: lit(1) + lit(2),
        page: universe.uniforms.uFocusPage.value };
      /* 누르기: 그 별이 골라지고(평면과 같은 상태) 카메라가 그 별로 난다. */
      const box = universe.host.getBoundingClientRect();
      const press = (x, y, shiftKey = false) => {
        const at = { bubbles: true, clientX: box.left + x, clientY: box.top + y, button: 0, pointerId: 1, shiftKey };
        universe.host.dispatchEvent(new PointerEvent("pointermove", at));
        universe.host.dispatchEvent(new PointerEvent("pointerdown", at));
        universe.host.dispatchEvent(new PointerEvent("pointerup", at));
      };
      const flat = { zoom: layout.zoom, panX: layout.panX, panY: layout.panY };
      press(spot.x, spot.y);
      await frames(3);
      const row = map.galaxy[map.rowOf[hub]];
      const star = [universe.pos3[hub * 3], universe.pos3[hub * 3 + 1], universe.pos3[hub * 3 + 2]];
      const selected = { key: knowledgeSelectedKey, want: model.keys[hub], card: !view.querySelector(".knowledge-card")?.hidden,
        target: [universe.tx, universe.ty, universe.tz], star, dist: universe.dist,
        wantDist: Math.max(tune.flyPageDist, row.radius * tune.flyPageReach), hub: state()[hub] };
      /* 고른 쪽이 있을 때 Shift로 누른 다른 별 — 그 둘 사이의 경로가 켜지고 고름은 그대로다. */
      let other = -1;
      for (const seat of map.members[map.rowOf[hub]]) {
        if (seat === hub) continue;
        const at = universe.project(seat);
        if (at.depth > 0 && at.x > 8 && at.y > 8 && at.x < box.width - 8 && at.y < box.height - 8
          && universe.pickAt(at.x, at.y) === seat) {
          other = seat;
          break;
        }
      }
      const otherSpot = universe.project(other);
      press(otherSpot.x, otherSpot.y, true);
      await until(() => knowledgePath !== null && knowledgePath.report !== null, 300);
      await frames(2);
      const shiftRoute = knowledgeRoute(model, knowledgePath);
      const shifted = { other, key: knowledgeSelectedKey, from: knowledgePath?.sourceKey ?? null,
        to: knowledgePath?.targetKey ?? null, wantTo: model.keys[other], edges: shiftRoute?.edges.length ?? 0,
        lines: lit(2), nodes: (shiftRoute?.nodes ?? []).every((seat) => state()[seat] === 2) };
      /* 빈 곳을 누르면 풀린다 — 경로도. */
      const corner = universe.project(hub);
      press(2, box.height - 2);
      await frames(2);
      const cleared = { key: knowledgeSelectedKey, focus: universe.uniforms.uFocus.value, lines: lit(1) + lit(2),
        path: knowledgePath, corner: corner.x > 0 };
      /* 군집 고르기: 그 은하의 별 1, 초점 은하, 카메라가 그 은하로 — 평면 지도의 카메라는 그대로다. */
      toggleKnowledgeCluster(view, 1);
      await frames(3);
      const members = map.members[1];
      const topic = { picked: knowledgeClusterPicked, membersLit: members.every((seat) => state()[seat] === 1),
        galaxy: universe.uniforms.uFocusGal.value, target: [universe.tx, universe.ty, universe.tz],
        centre: [map.galaxy[1].x, map.galaxy[1].y, map.galaxy[1].z],
        wantDist: map.galaxy[1].radius * tune.flyGalaxyReach + tune.flyGalaxyPad, dist: universe.dist,
        flatKept: flat.zoom === layout.zoom && flat.panX === layout.panX && flat.panY === layout.panY };
      toggleKnowledgeCluster(view, 1);
      await frames(2);
      /* 찾기: 맞은 별 3. */
      const found = model.keys.indexOf(answer.graph.nodes[150].id);
      knowledgeQuery = "찾을 별";
      await paintKnowledgeView();
      await frames(2);
      const search = { found: state()[found], focus: universe.uniforms.uFocus.value, other: state()[hub] };
      knowledgeQuery = "";
      await paintKnowledgeView();
      await frames(2);
      /* 경로: 경로의 별 2, 경로의 선이 별자리 선에. */
      await runKnowledgeShortestPath(view, layout, model.keys[starts[0] + 5], model.keys[starts[1] + 7]);
      await frames(2);
      const route = knowledgeRoute(model, knowledgePath);
      const path = { nodes: route?.nodes.length ?? 0, lit: (route?.nodes ?? []).every((seat) => state()[seat] === 2),
        lines: lit(2), edges: route?.edges.length ?? 0, focus: universe.uniforms.uFocus.value };
      clearKnowledgePath(view, layout);
      await paintKnowledgeView();
      knowledgeDimension = heldDimension;
      secondBrainVault = heldVault;
      await paintKnowledgeView();
      return { picked, want: hub, nothing, hovered, rested, selected, shifted, cleared, topic, search, path };
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  });
  const detail = JSON.stringify(seen);
  const near = (one, two) => one.every((value, at) => Math.abs(value - two[at]) < 1e-3);
  ok("t-12443 ⑤: a star is picked where it is projected, and an empty corner picks nothing",
    !seen.thrown && seen.picked === seen.want && seen.nothing === -1, detail);
  ok("universe ⑤: pointing at a star lights it (2), its neighbours (1), dims the rest, focuses its cluster as a page's focus and lights exactly its own links — dimmed by the token's rule when they are many; letting go rests",
    !seen.thrown && seen.hovered.hub === 2 && seen.hovered.neighboursLit && seen.hovered.stranger === 0
      && seen.hovered.focus === 1 && seen.hovered.galaxy === seen.hovered.row && seen.hovered.page === 1
      && seen.hovered.lines === seen.hovered.degree && seen.hovered.degree > 0
      && Math.abs(seen.hovered.gain - seen.hovered.wantGain) < 1e-6 && seen.hovered.gain < 1
      && seen.rested.hub === 0 && seen.rested.focus === 0 && seen.rested.lines === 0 && seen.rested.page === 0,
    detail);
  ok("t-12443 ⑤: pressing a star picks its page (the same selection as the flat map, the card opens) and flies to it",
    !seen.thrown && seen.selected.key === seen.selected.want && seen.selected.card && seen.selected.hub === 2
      && near(seen.selected.target, seen.selected.star) && Math.abs(seen.selected.dist - seen.selected.wantDist) < 1e-3,
    detail);
  ok("universe ⑤: with a page picked, Shift-pressing another star asks for the path between the two — the pick stays, the path's stars and links light",
    !seen.thrown && seen.shifted.other >= 0 && seen.shifted.key === seen.selected.want
      && seen.shifted.from === seen.selected.want && seen.shifted.to === seen.shifted.wantTo
      && seen.shifted.edges >= 1 && seen.shifted.lines === seen.shifted.edges && seen.shifted.nodes,
    detail);
  ok("t-12443 ⑤: pressing empty space lets the pick go — and the path with it",
    !seen.thrown && seen.cleared.key === null && seen.cleared.focus === 0 && seen.cleared.lines === 0
      && seen.cleared.path === null,
    detail);
  ok("t-12443 ⑤: picking a cluster lights its stars, focuses it and flies there, leaving the flat camera alone",
    !seen.thrown && seen.topic.picked === 1 && seen.topic.membersLit && seen.topic.galaxy === 1
      && near(seen.topic.target, seen.topic.centre) && Math.abs(seen.topic.dist - seen.topic.wantDist) < 1e-3
      && seen.topic.flatKept,
    detail);
  ok("t-12443 ⑤: a search's hits wear the aiming ring (3) under a focus",
    !seen.thrown && seen.search.found === 3 && seen.search.focus === 1 && seen.search.other === 0, detail);
  ok("t-12443 ⑤: a shortest path lights its stars (2) and its links (2)",
    !seen.thrown && seen.path.nodes > 1 && seen.path.lit && seen.path.lines === seen.path.edges && seen.path.focus === 1,
    detail);
}

/* ---- ⑤ 인스펙터와 평면의 일 ------------------------------------------------------------------------
 *
 * 「Beautiful 3D graph. Zero output.」 비판에 대한 글자의 답(디자이너 m-12467의 6): 우주가 서 있는 동안 개요는
 * 가장 굵은 필라멘트(보낸 은하 → 받는 은하) 여섯을 글로 적고, 누르면 그 흐름으로 난다. 은하를 고르면 그 은하의
 * 형태·최근 2주에 고친 쪽의 몫·그 은하의 필라멘트가 선다. 쪽 카드는 고친 때를 날짜와 「얼마 전」으로 말한다. 관계
 * 편집은 평면의 일이다 — 우주에서 누르면 평면의 같은 점으로 내려앉는다(설계 §4-f). */
export async function testKnowledgeUniverseInspector(page, ok) {
  await page.emulateMedia({ reducedMotion: "reduce" });
  const seen = await page.evaluate(async () => {
    try {
      const view = document.querySelector(".knowledge-view:not([hidden])");
      const frame = () => new Promise((done) => requestAnimationFrame(done));
      const frames = async (count) => {
        for (let at = 0; at < count; at += 1) await frame();
      };
      const until = async (wanted, rounds = 900) => {
        for (let round = 0; round < rounds; round += 1) {
          if (wanted()) return true;
          await frame();
        }
        return false;
      };
      const heldDimension = knowledgeDimension;
      const heldVault = secondBrainVault;
      const sizes = [60, 48, 36, 24];
      const starts = [0, 60, 108, 144];
      const edges = [];
      sizes.forEach((size, group) => {
        const first = starts[group];
        for (let step = 1; step < size; step += 1) {
          edges.push({ from: first + step, to: first, kind: "mentions" });
          if (step > 1) edges.push({ from: first + step, to: first + step - 1, kind: "mentions" });
        }
      });
      /* 굵은 흐름 둘 — 서로 다른 쪽들 사이라 한 쪽이 두 흐름에 걸려 무리를 옮기지 않는다. */
      for (let one = 0; one < 12; one += 1) edges.push({ from: 10 + one, to: 70 + one, kind: "mentions" });
      for (let one = 0; one < 12; one += 1) edges.push({ from: 110 + one, to: 30 + one, kind: "related" });
      const nowMs = Date.now();
      const modifiedMs = Array.from({ length: 168 }, (unused, at) => (at < 60 && at % 2 === 0 ? nowMs - 3 * 86_400_000
        : nowMs - 90 * 86_400_000));
      const answer = window.__buildVaultGraph__({ path: "/inspector", sources: false },
        { pages: 168, ghosts: 0, tags: [], customEdges: edges, modifiedMs });
      window.__GRAPH_BUILT__ = answer;
      noteKnowledgeExploreLines({});
      secondBrainVault = answer.vault;
      knowledgeDimension = "3d";
      knowledgeQuery = "";
      knowledgeSelectedKey = null;
      knowledgeClusterPicked = -1;
      setKnowledgeMode(view, "global", { paint: false });
      knowledgeReport = answer;
      await paintKnowledgeView();
      await until(() => (knowledgeLayouts.get(view)?.left ?? 1) === 0);
      await paintKnowledgeView();
      await frames(2);
      const layout = knowledgeLayouts.get(view);
      const universe = knowledgeUniverses.get(view);
      const map = universe.map;
      const flows = view.querySelector(".knowledge-overview-filaments");
      const rows = [...(flows?.querySelectorAll("[data-knowledge-flow]") ?? [])];
      const overview = { shown: flows !== null && !flows.hidden, rows: rows.length, filaments: map.filaments.length,
        first: rows[0]?.textContent ?? "", note: rows[0]?.parentElement.querySelector(".knowledge-inspector-note")?.textContent ?? "",
        want: map.filaments[0] === undefined ? "" : `${knowledgeClusterWord(layout, map.filaments[0].from)}`,
        wantNote: map.filaments[0] === undefined ? "" : `${map.filaments[0].sent}/${map.filaments[0].n}` };
      /* 줄을 누르면 보낸 은하가 골라지고 그 흐름을 옆에서 보는 자리로 난다. */
      rows[0]?.click();
      await frames(3);
      const one = map.filaments[0];
      const a = map.galaxy[one?.from ?? 0];
      const b = map.galaxy[one?.to ?? 0];
      const flown = { picked: knowledgeClusterPicked, want: one?.from,
        target: [universe.tx, universe.ty, universe.tz], middle: [(a.x + b.x) / 2, (a.y + b.y) / 2, (a.z + b.z) / 2] };
      /* 은하 칸: 형태·쪽 수·최근 2주의 몫, 그 은하의 필라멘트. */
      const galaxy = view.querySelector(".knowledge-overview-galaxy");
      const galaxyRows = [...(galaxy?.querySelectorAll("[data-knowledge-flow]") ?? [])];
      const galaxyWords = galaxy?.textContent ?? "";
      const recent = map.galaxy[knowledgeClusterPicked]?.activity ?? -1;
      const pickedRow = map.galaxy[knowledgeClusterPicked];
      const card = { shown: galaxy !== null && !galaxy.hidden, words: galaxyWords, rows: galaxyRows.length,
        head: galaxy?.querySelector(".knowledge-inspector-head")?.textContent ?? "",
        wantHead: t("knowledge.galaxyAbout", "{{name}} · {{pages}}쪽", { name: knowledgeClusterWord(layout, knowledgeClusterPicked),
          pages: layout.communitySize[knowledgeClusterPicked] }),
        about: galaxy?.querySelector(".knowledge-overview-galaxy-about")?.textContent ?? "",
        wantAbout: t("knowledge.galaxyRecent", "허브에 바로 이어진 쪽 {{direct}}(가지를 친 쪽 {{branches}}) · 가장 먼 쪽까지 {{steps}}걸음 · 최근 2주에 고친 쪽 {{percent}}",
          { direct: pickedRow?.direct, branches: pickedRow?.branches, steps: pickedRow?.deepest,
            percent: `${Math.round(recent * 100)}%` }),
        direct: pickedRow?.direct ?? -1, size: pickedRow?.size ?? -1, branches: pickedRow?.branches ?? -1,
        steps: pickedRow?.deepest ?? -1,
        percent: `${Math.round(recent * 100)}%` };
      /* 은하 칸의 밝은 별: 연결이 많은 차례로 여섯(디자이너 m-12627, 시안 은하 카드) — 줄을 누르면 그 쪽이 골라진다. */
      const stars = [...(galaxy?.querySelectorAll(".knowledge-overview-galaxy-stars [data-knowledge-key]") ?? [])];
      const members = map.members[knowledgeClusterPicked] ?? [];
      const brightest = { rows: stars.length, want: Math.min(6, members.length),
        keys: stars.map((one) => one.dataset.knowledgeKey), wantKeys: members.slice(0, 6).map((at) => layout.model.keys[at]),
        notes: stars.map((one) => one.parentElement.querySelector(".knowledge-inspector-note")?.textContent ?? ""),
        wantNotes: members.slice(0, 6).map((at) => String(layout.model.degree[at])), selected: null };
      stars[0]?.click();
      await frames(2);
      brightest.selected = knowledgeSelectedKey;
      selectKnowledgeNode(view, null);
      await frames(2);
      toggleKnowledgeCluster(view, knowledgeClusterPicked);
      await frames(2);
      /* 쪽 카드의 고친 때: 날짜와 「얼마 전」. */
      const seat = layout.model.keys.indexOf(answer.graph.nodes[2].id);
      selectKnowledgeNode(view, layout.model.keys[seat]);
      await frames(2);
      const modified = view.querySelector(".knowledge-card .knowledge-modified")?.textContent ?? "";
      const ago = new Intl.RelativeTimeFormat(locale, { numeric: "auto" }).format(-3, "day");
      /* 관계 편집은 평면의 일: 우주에서 열면 평면의 같은 점으로. */
      openKnowledgeLinkForm(view);
      await frames(3);
      const editing = { mounted: knowledgeUniverses.has(view), universe: view.classList.contains("is-universe"),
        selected: knowledgeSelectedKey === layout.model.keys[seat], form: !view.querySelector(".knowledge-link-form")?.hidden };
      closeKnowledgeLinkForm(view);
      /* 평면에서는 우주의 절이 서지 않는다. */
      const flatFlows = view.querySelector(".knowledge-overview-filaments");
      const flat = { hidden: flatFlows === null || flatFlows.hidden };
      selectKnowledgeNode(view, null);
      knowledgeDimension = heldDimension;
      secondBrainVault = heldVault;
      await paintKnowledgeView();
      return { overview, flown, card, brightest, modified, ago, editing, flat };
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  });
  const detail = JSON.stringify(seen);
  const near = (one, two) => one.every((value, at) => Math.abs(value - two[at]) < 1e-3);
  ok("t-12443 ⑤: while the universe stands the overview lists the thickest filaments as sender → receiver with sent/lines, and a row flies to that flow",
    !seen.thrown && seen.overview.shown && seen.overview.filaments >= 2
      && seen.overview.rows === Math.min(6, seen.overview.filaments)
      && seen.overview.first.includes(seen.overview.want) && seen.overview.note === seen.overview.wantNote
      && seen.flown.picked === seen.flown.want && near(seen.flown.target, seen.flown.middle),
    detail);
  ok("universe ⑤: a picked cluster says its pages, its tree in words (the pages linked straight to the hub, those that branch on, the deepest step) and the share changed in two weeks, and lists its own filaments",
    !seen.thrown && seen.card.shown && seen.card.head === seen.card.wantHead && seen.card.about === seen.card.wantAbout
      && seen.card.direct >= 1 && seen.card.direct <= seen.card.size - 1 && seen.card.branches <= seen.card.direct
      && seen.card.steps >= 1 && seen.card.words.includes(seen.card.percent)
      && seen.card.rows >= 1,
    detail);
  ok("t-12443 ⑥: a picked cluster names its six largest stars by links, and a row picks that page",
    !seen.thrown && seen.brightest.rows === seen.brightest.want && seen.brightest.rows > 0
      && seen.brightest.keys.every((key, at) => key === seen.brightest.wantKeys[at])
      && seen.brightest.notes.every((note, at) => note === seen.brightest.wantNotes[at])
      && seen.brightest.selected === seen.brightest.wantKeys[0],
    detail);
  ok("t-12443 ⑤: a page's card says when it was changed as a date and as how long ago",
    !seen.thrown && seen.modified.includes(seen.ago), detail);
  ok("t-12443 ⑤: editing a relation is flat work — from the universe it lands on the flat map with the same page picked and the form open",
    !seen.thrown && !seen.editing.mounted && !seen.editing.universe && seen.editing.selected && seen.editing.form
      && seen.flat.hidden,
    detail);
}

/* ⑥ 이름표(시안 `updateLabels`·`galaxyAnchor`·`showTip`) — 시안과 같은 모양의 합성 볼트(850쪽) 위에서.
 * 자리 잡는 차례는 고정 UI 상자 → 은하 명판(고른 것 먼저, 그다음 순위) → 필라멘트 수(멀리서 쉴 때만) → 쪽 이름
 * (쉴 때는 연결이 가장 많은 다섯, 가까이 가거나 초점이 있으면 풀 30)이고, 쉬는 화면의 글자는 서로 겹치지 않는다. */
export async function testKnowledgeUniverseLabels(page, ok) {
  await page.emulateMedia({ reducedMotion: "reduce" });
  const seen = await page.evaluate(async () => {
    try {
      const view = document.querySelector(".knowledge-view:not([hidden])");
      const frame = () => new Promise((done) => requestAnimationFrame(done));
      const frames = async (count) => {
        for (let at = 0; at < count; at += 1) await frame();
      };
      const until = async (wanted, rounds = 900) => {
        for (let round = 0; round < rounds; round += 1) {
          if (wanted()) return true;
          await frame();
        }
        return false;
      };
      const heldDimension = knowledgeDimension;
      const heldVault = secondBrainVault;
      const answer = window.__buildVaultGraph__({ path: "/labels", sources: false },
        window.__universeVaultSpec__(850, Date.now()));
      window.__GRAPH_BUILT__ = answer;
      noteKnowledgeExploreLines({});
      secondBrainVault = answer.vault;
      knowledgeDimension = "3d";
      knowledgeQuery = "";
      knowledgeSelectedKey = null;
      knowledgeClusterPicked = -1;
      knowledgeSlicerCutoff = 0;
      knowledgePath = null;
      setKnowledgeMode(view, "global", { paint: false });
      knowledgeReport = answer;
      await paintKnowledgeView();
      await until(() => (knowledgeLayouts.get(view)?.left ?? 1) === 0, 3000);
      await paintKnowledgeView();
      await frames(3);
      const layout = knowledgeLayouts.get(view);
      const model = layout.model;
      const universe = knowledgeUniverses.get(view);
      const map = universe.map;
      const host = universe.host.getBoundingClientRect();
      const box = (element) => {
        const rect = element.getBoundingClientRect();
        return { x: rect.left - host.left, y: rect.top - host.top, w: rect.width, h: rect.height };
      };
      const shown = (selector) => [...universe.host.querySelectorAll(selector)].filter((one) => one.classList.contains("is-on"));
      const overlap = (one, two) => one.x < two.x + two.w - 0.5 && one.x + one.w > two.x + 0.5
        && one.y < two.y + two.h - 0.5 && one.y + one.h > two.y + 0.5;
      const clashes = (boxes) => {
        let count = 0;
        for (let one = 0; one < boxes.length; one += 1) {
          for (let two = one + 1; two < boxes.length; two += 1) if (overlap(boxes[one], boxes[two])) count += 1;
        }
        return count;
      };
      const zoom = box(view.querySelector(".knowledge-zoom"));
      /* 쉬는 처음 자리. */
      const restPlates = shown(".knowledge-universe-plate");
      const restFlows = shown(".knowledge-universe-flow");
      const restNames = shown(".knowledge-universe-name");
      const restBoxes = [...restPlates, ...restFlows, ...restNames].map(box);
      /* 쉬는 밝은 별: 연결이 가장 많은 쪽들 — 제목이 제 은하의 이름과 같은 별(이름 없는 군집은 허브 제목이 이름이다)은
       * 명판과 두 번 서지 않게 건너뛴다(디자이너 m-12642). 이름은 평면처럼 `label-max`에서 자른다. */
      const cut = (seat) => knowledgeShortWord(model.titles[seat], layout.tuning.labelMax);
      const sameAsGalaxy = (seat) => map.rowOf[seat] >= 0
        && knowledgeShortWord(model.titles[seat], KNOWLEDGE_COMMUNITY.labelMax) === knowledgeClusterWord(layout, map.rowOf[seat]);
      const bright = Array.from({ length: layout.count }, (unused, at) => at)
        .sort((one, two) => model.degree[two] - model.degree[one] || one - two)
        .filter((at) => !sameAsGalaxy(at)).slice(0, 5).map(cut);
      const long = Array.from({ length: layout.count }, (unused, at) => at)
        .filter((at) => model.titles[at].length > layout.tuning.labelMax).length;
      const rest = { plates: restPlates.length, galaxies: map.galaxies, flows: restFlows.length,
        filaments: map.filaments.length, names: restNames.map((one) => one.textContent), bright, long,
        longest: Math.max(...restNames.map((one) => one.textContent.length), 0), labelMax: layout.tuning.labelMax,
        clashes: clashes(restBoxes), onZoom: restBoxes.filter((one) => overlap(one, zoom)).length,
        plateNames: restPlates.map((one) => one.querySelector(".knowledge-universe-plate-name")?.textContent ?? ""),
        wantNames: restPlates.map((one) => knowledgeClusterWord(layout, Number(one.dataset.knowledgeCluster))),
        plateCounts: restPlates.map((one) => one.querySelector(".knowledge-universe-plate-count")?.textContent ?? ""),
        wantCounts: restPlates.map((one) => String(layout.communitySize[Number(one.dataset.knowledgeCluster)])),
        flowCounts: restFlows.map((one) => one.textContent.trim()),
        wantFlows: map.filaments.slice(0, 6).map((one) => String(one.n)),
        pressed: restPlates.filter((one) => one.getAttribute("aria-pressed") === "true").length,
        full: restPlates.filter((one) => one.classList.contains("is-full")).length,
        /* 선 명판은 키보드로도 닿고(단추, Tab), 숨은 명판은 닿지 않는다. */
        tabbable: restPlates.every((one) => one.tabIndex === 0)
          && [...universe.host.querySelectorAll(".knowledge-universe-plate:not(.is-on)")].every((one) => one.tabIndex === -1) };
      /* 올리기 — 순위 1 은하의 허브에 포인터를 둔다(시안 06-hover). */
      const hub = map.members[1][0];
      const spot = universe.project(hub);
      universe.host.dispatchEvent(new PointerEvent("pointermove",
        { bubbles: true, clientX: host.left + spot.x, clientY: host.top + spot.y, pointerId: 1 }));
      await until(() => universe.hover === hub, 60);
      await frames(2);
      const neighbours = new Set();
      for (let slot = model.start[hub]; slot < model.start[hub + 1]; slot += 1) neighbours.add(cut(model.neighbour[slot]));
      const tip = universe.host.querySelector(".knowledge-universe-tip");
      const hoverNames = shown(".knowledge-universe-name").map((one) => one.textContent);
      const hoverBoxes = [...shown(".knowledge-universe-plate"), ...shown(".knowledge-universe-name")].map(box);
      /* 팁은 명판 위에 설 수 있다(시안 06-hover: 팁 뒤에 명판) — 쪽 이름만 팁을 비킨다. */
      const tipBox = tip === null ? null : box(tip);
      const underTip = tipBox === null ? 0
        : shown(".knowledge-universe-name").map(box).filter((one) => overlap(one, tipBox)).length;
      const hovered = { hover: universe.hover, tipOn: tip?.classList.contains("is-on") ?? false,
        tipWords: tip?.textContent ?? "", title: model.titles[hub], degree: String(model.degree[hub]),
        cluster: knowledgeClusterWord(layout, map.rowOf[hub]), flows: shown(".knowledge-universe-flow").length,
        names: hoverNames.length, neighbourNames: hoverNames.filter((word) => neighbours.has(word)).length,
        ownName: hoverNames.includes(cut(hub)),
        clashes: clashes(hoverBoxes), underTip,
        tipInside: tipBox !== null && tipBox.x >= 0 && tipBox.x + tipBox.w <= host.width + 0.5,
        tipRoom: tipBox === null ? 0 : tipBox.w, room: knowledgeUniverseTuning(view).tipRoom };
      universe.host.dispatchEvent(new PointerEvent("pointerleave", { bubbles: false, pointerId: 1 }));
      await frames(2);
      const left = { tipOn: tip?.classList.contains("is-on") ?? false, flows: shown(".knowledge-universe-flow").length };
      /* 은하 고르기 — 날아온 자리는 은하의 크기와 상관없이 「가까이」다(문턱 = max(처음 거리 × close-in, (5.2R + 40) ×
       * close-in-fly), 디자이너 m-12642): 그 명판만 크게, 쪽 이름은 그 은하의 별만(시안 05-close). 합성 볼트의 순위 0은 두
       * 주제가 합쳐진 큰 은하라 처음 거리의 문턱만으로는 밖이다. */
      const small = 0;
      toggleKnowledgeCluster(view, small);
      await until(() => !universe.tween.on, 400);
      await frames(2);
      const closePlates = shown(".knowledge-universe-plate");
      const closeNames = shown(".knowledge-universe-name").map((one) => one.textContent);
      const members = new Set(map.members[small].map(cut));
      const reach = (map.galaxy[small].radius * knowledgeUniverseTuning(view).flyGalaxyReach
        + knowledgeUniverseTuning(view).flyGalaxyPad) * knowledgeUniverseTuning(view).closeInFly;
      const close = { closeIn: universe.dist < Math.max(universe.home.dist * knowledgeUniverseTuning(view).closeIn, reach),
        homeOnly: universe.dist < universe.home.dist * knowledgeUniverseTuning(view).closeIn,
        plates: closePlates.map((one) => Number(one.dataset.knowledgeCluster)),
        full: closePlates.map((one) => one.classList.contains("is-full")),
        pressed: closePlates.map((one) => one.getAttribute("aria-pressed")),
        names: closeNames.length, strangers: closeNames.filter((word) => !members.has(word)).length,
        most: knowledgeUniverseTuning(view).labelsClose,
        flows: shown(".knowledge-universe-flow").length,
        small, clashes: clashes([...closePlates, ...shown(".knowledge-universe-name")].map(box)) };
      toggleKnowledgeCluster(view, small);
      universe.goHome();
      await until(() => !universe.tween.on, 400);
      await frames(2);
      /* 명판을 누르면 그 은하가 골라진다(시안 `focusTopic`). */
      const plate = shown(".knowledge-universe-plate").find((one) => Number(one.dataset.knowledgeCluster) === 1)
        ?? shown(".knowledge-universe-plate")[0];
      const plateRank = Number(plate?.dataset.knowledgeCluster ?? -1);
      plate?.click();
      await frames(2);
      const pressed = { rank: plateRank, picked: knowledgeClusterPicked };
      toggleKnowledgeCluster(view, knowledgeClusterPicked);
      universe.goHome();
      await until(() => !universe.tween.on, 400);
      /* 떠오르거나 내려앉는 동안(들림 0.98 밑)에는 이름표가 서지 않는다. */
      universe.tween.on = false;
      universe.lift = 0.5;
      universe.camVersion += 1;
      universe.invalidate();
      await frames(2);
      const rising = { on: universe.host.querySelectorAll(".knowledge-universe-labels .is-on").length };
      universe.lift = 1;
      universe.camVersion += 1;
      universe.invalidate();
      await frames(2);
      /* 범례: 우주가 서 있는 동안 우주의 네 줄만, 평면에서는 평면의 줄만(디자이너 m-12627). */
      const legendRows = [...view.querySelectorAll(".knowledge-legend-list > li")];
      const legendShown = () => legendRows.filter((one) => getComputedStyle(one).display !== "none")
        .map((one) => one.dataset.legendUniverse === "true");
      const legend = { universe: legendRows.filter((one) => one.dataset.legendUniverse === "true").length,
        up: legendShown(), flat: [] };
      setKnowledgeDimension(view, "2d");
      await until(() => !knowledgeUniverses.has(view), 200);
      await frames(2);
      legend.flat = legendShown();
      knowledgeDimension = heldDimension;
      secondBrainVault = heldVault;
      await paintKnowledgeView();
      return { rest, hovered, left, close, pressed, rising, legend };
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  });
  const detail = JSON.stringify(seen).slice(0, 4000);
  ok("t-12443 ⑥: at rest every galaxy that fits wears its plate (name and pages), the thickest filaments their line counts, and the five most linked pages their names — nothing overlaps, nothing sits on the zoom box",
    !seen.thrown && seen.rest.plates >= Math.min(4, seen.rest.galaxies) && seen.rest.plates <= seen.rest.galaxies
      && seen.rest.longest <= seen.rest.labelMax
      && seen.rest.plateNames.every((word, at) => word === seen.rest.wantNames[at])
      && seen.rest.plateCounts.every((word, at) => word === seen.rest.wantCounts[at])
      && seen.rest.flows >= 1 && seen.rest.flows <= Math.min(6, seen.rest.filaments)
      && seen.rest.flowCounts.every((word) => seen.rest.wantFlows.includes(word))
      && seen.rest.names.length >= 1 && seen.rest.names.length <= 5
      && seen.rest.names.every((word) => seen.rest.bright.includes(word))
      && seen.rest.clashes === 0 && seen.rest.onZoom === 0 && seen.rest.pressed === 0 && seen.rest.full === 0
      && seen.rest.tabbable,
    detail);
  ok("t-12443 ⑥: pointing at a star shows its tip (title, galaxy, links) beside it, names its neighbours but not itself, and lets the filament counts go; leaving takes the tip away",
    !seen.thrown && seen.hovered.tipOn && seen.hovered.tipWords.includes(seen.hovered.title)
      && seen.hovered.tipWords.includes(seen.hovered.cluster) && seen.hovered.tipWords.includes(seen.hovered.degree)
      && seen.hovered.flows === 0 && seen.hovered.neighbourNames >= 1 && !seen.hovered.ownName
      && seen.hovered.clashes === 0 && seen.hovered.underTip === 0 && !seen.left.tipOn && seen.left.flows >= 1
      && seen.hovered.tipInside && seen.hovered.tipRoom <= seen.hovered.room,
    detail);
  ok("t-12443 ⑥: a picked galaxy is close wherever it flew — even the biggest, outside the home-distance threshold — so only its plate stays, full and pressed, and its own stars carry names spread over it, up to labels-close (designer m-12670)",
    !seen.thrown && seen.close.closeIn && !seen.close.homeOnly && seen.close.plates.length === 1 && seen.close.plates[0] === seen.close.small
      && seen.close.full[0] === true && seen.close.pressed[0] === "true" && seen.close.names >= 8
      && seen.close.names <= seen.close.most
      && seen.close.strangers === 0 && seen.close.flows === 0 && seen.close.clashes === 0,
    detail);
  ok("universe ⑥: while the universe stands the legend shows its five rows (size, recency, branches, filaments, pointed links) and hides the flat rows; flat shows the flat rows only",
    !seen.thrown && seen.legend.universe === 5 && seen.legend.up.length === 5 && seen.legend.up.every((one) => one)
      && seen.legend.flat.length >= 1 && seen.legend.flat.every((one) => !one),
    detail);
  ok("t-12443 ⑥: pressing a plate picks its galaxy, and no label stands while the universe rises or folds",
    !seen.thrown && seen.pressed.rank >= 0 && seen.pressed.picked === seen.pressed.rank && seen.rising.on === 0,
    detail);
}

/* ⑦ 「움직임」과 밝은 테마(시안 `setMotion`·`applyTheme`, 디자이너 m-12467의 5) — 「보기」의 단추 하나가 쉬고 9초 뒤의 느린
 * 돌기와 필라멘트의 흐름을 함께 켜고 끈다. 끈 판은 쉬는 동안 한 장도 그리지 않고, 켜면 이미 쉰 것으로 쳐 곧 돈다.
 * 고른 값은 다음에 서는 우주도 따른다. 밝은 테마에서 이름표는 종이 위의 먹빛이다. */
export async function testKnowledgeUniverseMotion(page, ok) {
  await page.emulateMedia({ reducedMotion: "no-preference" });
  const seen = await page.evaluate(async () => {
    try {
      const view = document.querySelector(".knowledge-view:not([hidden])");
      const frame = () => new Promise((done) => requestAnimationFrame(done));
      const frames = async (count) => {
        for (let at = 0; at < count; at += 1) await frame();
      };
      const pause = (ms) => new Promise((done) => setTimeout(done, ms));
      const until = async (wanted, rounds = 900) => {
        for (let round = 0; round < rounds; round += 1) {
          if (wanted()) return true;
          await frame();
        }
        return false;
      };
      const heldDimension = knowledgeDimension;
      const heldVault = secondBrainVault;
      const heldTheme = document.documentElement.dataset.theme ?? "dark";
      const answer = window.__buildVaultGraph__({ path: "/motion", sources: false },
        window.__universeVaultSpec__(200, Date.now()));
      window.__GRAPH_BUILT__ = answer;
      noteKnowledgeExploreLines({});
      secondBrainVault = answer.vault;
      knowledgeDimension = "3d";
      knowledgeQuery = "";
      knowledgeSelectedKey = null;
      knowledgeClusterPicked = -1;
      setKnowledgeMode(view, "global", { paint: false });
      knowledgeReport = answer;
      await paintKnowledgeView();
      /* 문을 연 판의 첫 그림 — 문이 그것을 불렀는가는 시각으로 묻지 않는다(부른 때가 적혔는가). */
      const newborn = knowledgeUniverses.get(view);
      const birth = { door: (newborn?.birth.t0 ?? 0) > 0, on: null, from: -1, labels: -1, done: false, to: -1,
        back: -1, same: false, risen: null };
      await until(() => (knowledgeLayouts.get(view)?.left ?? 1) === 0, 3000);
      await paintKnowledgeView();
      await until(() => knowledgeUniverses.get(view)?.tween.on === false, 600);
      await until(() => knowledgeUniverses.get(view)?.birth.on === false, 900);
      await frames(2);
      const universe = knowledgeUniverses.get(view);
      /* 한 번 더 태어나게 해 처음부터 본다 — 별이 허브에서 퍼지는 동안(`uBirth` 0 → 1) 이름표는 서지 않고, 다 퍼지면
       * 돌아온다. */
      universe.bear();
      birth.on = universe.birth.on;
      birth.from = universe.uniforms.uBirth.value;
      universe.invalidate();
      await frame();
      birth.labels = universe.birth.on ? universe.host.querySelectorAll(".knowledge-universe-labels .is-on").length : 0;
      birth.done = await until(() => universe.birth.on === false, 900);
      await frames(3);
      birth.to = universe.uniforms.uBirth.value;
      birth.back = universe.host.querySelectorAll(".knowledge-universe-labels .is-on").length;
      birth.same = universe === newborn;
      const tune = knowledgeUniverseTuning(view);
      const button = view.querySelector(".knowledge-universe-motion");
      const showing = () => button !== null && getComputedStyle(button.closest(".knowledge-universe-controls") ?? button)
        .display !== "none";
      const up = { exists: button !== null, shown: showing(), pressed: button?.getAttribute("aria-pressed") ?? "",
        moving: universe.moving() };
      /* 끈다 — 쉬는 동안 한 장도 그리지 않는다. */
      button?.click();
      await frames(3);
      const before = universe.frames;
      await frames(30);
      const off = { pressed: button?.getAttribute("aria-pressed") ?? "", motion: universe.motion,
        drawn: universe.frames - before, flow: universe.uniforms.uFlow.value };
      /* 켠다 — 이미 쉰 것으로 쳐서(시안 8000 ms) 곧 돈다. */
      button?.click();
      const rested = performance.now() - universe.lastTouch;
      await frames(3);
      /* 쉬는 동안의 움직임은 화면의 박자가 아니라 `ambient-gap`의 박자로 그린다. */
      const beforeOn = universe.frames;
      const began = performance.now();
      await pause(900);
      const on = { pressed: button?.getAttribute("aria-pressed") ?? "", motion: universe.motion, rested,
        wake: tune.wakeRest, drawn: universe.frames - beforeOn, span: performance.now() - began, gap: tune.ambientGap };
      /* 고른 값은 다음 우주도 따른다 — 끈 채로 평면에 갔다가 돌아온다. */
      button?.click();
      setKnowledgeDimension(view, "2d");
      await until(() => !knowledgeUniverses.has(view), 200);
      await frames(2);
      const flatShown = showing();
      setKnowledgeDimension(view, "3d");
      await until(() => knowledgeUniverses.get(view)?.tween.on === false, 600);
      await frames(2);
      const next = knowledgeUniverses.get(view);
      /* 평면에서 떠오른 우주는 태어나지 않는다 — 떠오름이 그 일을 한다. */
      birth.risen = (next?.birth.t0 ?? -1) === 0 && next?.uniforms.uBirth.value === 1;
      const again = { motion: next?.motion ?? null, pressed: button?.getAttribute("aria-pressed") ?? "" };
      button?.click();
      /* 내려앉는 도중에 다시 3D — 접힘을 거두고 같은 우주가 그 자리에서 다시 선다. 끝난 접힘이 새로 누른 우주를
       * 떠나보내면 3D 단추는 눌린 채 판은 평면이다. */
      const standing = knowledgeUniverses.get(view);
      setKnowledgeDimension(view, "2d");
      await frames(5);
      const midFold = standing?.folding ?? false;
      setKnowledgeDimension(view, "3d");
      await frames(2);
      await until(() => {
        const now = knowledgeUniverses.get(view);
        return now === undefined || !now.tween.on;
      }, 400);
      await frames(3);
      const risen = knowledgeUniverses.get(view);
      const refold = { midFold, same: risen === standing, folding: risen?.folding ?? null,
        universe: view.classList.contains("is-universe"), lift: risen?.lift ?? 0 };
      /* 밝은 테마 — 이름표의 글자는 종이 위의 먹빛이다. */
      const plate = next?.host.querySelector(".knowledge-universe-plate") ?? null;
      const shade = (element) => {
        const [r, g, b] = getComputedStyle(element).color.match(/[\d.]+/g).map(Number);
        return (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255;
      };
      const dark = plate === null ? -1 : shade(plate);
      setTheme("light");
      await frames(3);
      const light = plate === null ? -1 : shade(plate);
      setTheme(heldTheme);
      await frames(2);
      knowledgeDimension = heldDimension;
      secondBrainVault = heldVault;
      await paintKnowledgeView();
      return { up, off, on, flatShown, again, refold, dark, light, birth };
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  });
  await page.emulateMedia({ reducedMotion: "reduce" });
  const detail = JSON.stringify(seen).slice(0, 3000);
  ok("universe ⑦: a universe opened at the door unfurls — the stars spread from their hubs while no label stands, the labels return once it is fully spread — and one that rose from the flat map does not",
    !seen.thrown && seen.birth.door && seen.birth.on === true && seen.birth.from === 0 && seen.birth.labels === 0
      && seen.birth.done && seen.birth.to === 1 && seen.birth.back > 0 && seen.birth.same && seen.birth.risen === true,
    detail);
  ok("t-12443 ⑦: while the universe stands the view menu carries 「움직임」, pressed; switched off the universe draws nothing at rest and its light stops flowing",
    !seen.thrown && seen.up.exists && seen.up.shown && seen.up.pressed === "true" && seen.up.moving
      && seen.off.pressed === "false" && !seen.off.motion && seen.off.drawn === 0 && seen.off.flow === 0,
    detail);
  ok("universe ⑦: switched back on it counts as already rested (v4's 8000 ms) and keeps drawing at the ambient pace — never faster than one frame per ambient gap; the choice holds for the next universe; the flat map has no such switch",
    !seen.thrown && seen.on.pressed === "true" && seen.on.motion && seen.on.rested >= seen.on.wake - 50
      && seen.on.drawn >= 3 && seen.on.drawn <= Math.ceil(seen.on.span / seen.on.gap) + 1
      && seen.again.motion === false && seen.again.pressed === "false" && !seen.flatShown,
    detail);
  ok("t-12443 ⑦: pressing 3D while the universe folds takes the fold back — the same universe rises again and stays",
    !seen.thrown && seen.refold.midFold && seen.refold.same && seen.refold.folding === false && seen.refold.universe
      && seen.refold.lift === 1,
    detail);
  ok("t-12443 ⑦: in the light theme the labels are ink on paper — darker than in the dark theme",
    !seen.thrown && seen.dark > 0.6 && seen.light >= 0 && seen.light < 0.4, detail);
}

/* ---- ⑧ 빛깔 — 군집도 별도 제 군집의 색으로 (t-14081) ----------------------------------------------------
 *
 * 사람의 말(09-29): 「은하의 색상 별의 색상이 조금 명확했으면」. 묻는 것: (가) 군집 빛깔이 범례 칩·이름표 점과 같은
 * 색 칸 토큰의 색상을 그 채도의 반 넘게 싣고 밝기는 색 없는 군집과 같은가, (나) 합성이 밝은 색의 색상(10° 안)과
 * 채도(6할)를 지키는가, (다) 종이 위에서 그 색이 제 색상(20° 안)과 채도(반)의 먹빛으로 남되 흰빛의 먹보다 옅지
 * 않은가(글자·선의 대비), (라) 가장 큰 별(허브)의 가운데가 흰색으로 날지 않고 제 군집의 색상으로 남는가. 볼트는
 * ②의 것이다. */
export async function testKnowledgeUniverseColour(page, ok) {
  await page.emulateMedia({ reducedMotion: "reduce" });
  const seen = await page.evaluate(async ({ sizes, alone, quiet, half }) => {
    try {
      const view = document.querySelector(".knowledge-view:not([hidden])");
      const frame = () => new Promise((done) => requestAnimationFrame(done));
      const until = async (wanted, rounds = 900) => {
        for (let round = 0; round < rounds; round += 1) {
          if (wanted()) return true;
          await frame();
        }
        return false;
      };
      const heldDimension = knowledgeDimension;
      const heldVault = secondBrainVault;
      const heldPainter = knowledgePainterKind;
      const heldTheme = document.documentElement.dataset.theme ?? "dark";
      knowledgePainterKind = null;
      knowledgeQuery = "";
      knowledgeTagsPicked.clear();
      knowledgeOrphansOnly = knowledgeGhostsOnly = knowledgeTypedOnly = false;
      knowledgeAliveOnly = knowledgeColdOnly = knowledgeMergeOnly = false;
      knowledgeShowSources = false;
      knowledgeLintLens = knowledgeSelectedKey = knowledgePath = null;
      knowledgeClusterPicked = -1;
      knowledgeSlicerCutoff = 0;
      const edges = [];
      const starts = [];
      let at = 0;
      sizes.forEach((size) => { starts.push(at); at += size; });
      sizes.forEach((size, group) => {
        const first = starts[group];
        for (let step = 1; step < size; step += 1) {
          edges.push({ from: first + step, to: first + step - 1, kind: "mentions" });
          if (step > 2) edges.push({ from: first + step, to: first + ((step * 7) % (step - 1)), kind: "mentions" });
          if (step > 4) edges.push({ from: first + step, to: first + ((step * 13) % (step - 3)), kind: "related" });
        }
      });
      for (let step = 2; step < sizes[0]; step += 1) edges.push({ from: 0, to: step, kind: "mentions" });
      const nowMs = Date.now();
      const modifiedMs = [];
      sizes.forEach((size, group) => {
        for (let step = 0; step < size; step += 1) {
          const old = quiet.includes(group) || (group === half && step % 2 === 1);
          modifiedMs.push(old ? nowMs - 400 * 86_400_000 : nowMs - (step % 5) * 3_600_000);
        }
      });
      for (let one = 0; one < alone; one += 1) modifiedMs.push(0);
      const answer = window.__buildVaultGraph__({ path: "/colours", sources: false },
        { pages: at + alone, ghosts: 0, tags: [], customEdges: edges, modifiedMs });
      noteKnowledgeExploreLines({});
      secondBrainVault = answer.vault;
      knowledgeDimension = "3d";
      setKnowledgeMode(view, "global", { paint: false });
      knowledgeReport = answer;
      await paintKnowledgeView();
      await until(() => (knowledgeLayouts.get(view)?.left ?? 1) === 0);
      await paintKnowledgeView();
      await until(() => knowledgeUniverses.get(view)?.tween?.on === false);
      await frame();
      const universe = knowledgeUniverses.get(view);
      const layout = knowledgeLayouts.get(view);
      /* 색의 잣대(선형 RGB): 색상(HSV의 각), 채도(1 − 가장 약한 채널 / 가장 센 채널), 휘도(Rec. 709). */
      const hueOf = ([r, g, b]) => {
        const most = Math.max(r, g, b);
        const span = most - Math.min(r, g, b);
        if (span < 1e-6) return 0;
        const side = most === r ? ((g - b) / span + 6) % 6 : most === g ? (b - r) / span + 2 : (r - g) / span + 4;
        return side * 60;
      };
      const satOf = (rgb) => (Math.max(...rgb) <= 0 ? 0 : 1 - Math.min(...rgb) / Math.max(...rgb));
      const lumOf = ([r, g, b]) => 0.2126 * r + 0.7152 * g + 0.0722 * b;
      const turn = (one, two) => {
        const apart = Math.abs(one - two) % 360;
        return Math.min(apart, 360 - apart);
      };
      const round = (value, digits = 3) => Math.round(value * 10 ** digits) / 10 ** digits;
      const fromBytes = (bytes, offset) => [0, 1, 2].map((channel) => knowledgeUniverseLinear(bytes[offset + channel] / 255));
      /* (가) 빛깔과 그 토큰 — 범례 칩·이름표 점과 같은 색 칸(`--knowledge-hue-*`)이 원천이다. */
      const probe = universe.probe;
      const colourOf = (value) => {
        probe.style.color = value;
        return getComputedStyle(probe).color;
      };
      const hues = Array.from({ length: KNOWLEDGE_HUES }, (unused, hue) => hue);
      const hueOfPlate = (plate) => universe.map.galaxy[plate.row].hue;
      const dotsNow = () => universe.plates.filter((plate) => hueOfPlate(plate) >= 0).map((plate) => ({
        hue: hueOfPlate(plate),
        dot: getComputedStyle(plate.element.querySelector(".knowledge-universe-plate-name"), "::before").backgroundColor,
      }));
      const darkHue = hues.map((hue) => colourOf(`var(--knowledge-hue-${hue})`));
      const darkTint = hues.map((hue) => colourOf(`var(--knowledge-3d-tint-${hue})`));
      const darkDots = dotsNow();
      const sourceOf = (hue) => Array.from(universe.inks.tints.slice(hue * 3, hue * 3 + 3));
      const neutral = universe.inks.neutral;
      const tints = universe.map.galaxy.filter((row) => row.hue >= 0).map((row) => {
        const texel = (row.rank * KNOWLEDGE_UNIVERSE_SHAPE.texels + 2) * 4;
        const tint = Array.from(universe.map.gal.slice(texel, texel + 3));
        const source = sourceOf(row.hue);
        return { rank: row.rank, hue: row.hue, turn: round(turn(hueOf(tint), hueOf(source)), 2),
          share: round(satOf(tint) / satOf(source)), light: round(lumOf(tint) / lumOf(neutral)) };
      });
      /* (나)·(다) 합성 한 장 — 알려진 HDR 색의 줄(1 × n 텍셀)을 장면 목표 자리에 두고 합성만 그린다(번짐 0). 색은
       * 색 칸 여덟, 가장 센 채널 4(밝은 핵과 별의 가운데 자리). */
      const composite = (inputs) => {
        const renderer = universe.renderer;
        const post = universe.post;
        const comp = post.comp.uniforms;
        const data = new Float32Array(inputs.length * 4);
        inputs.forEach((rgb, index) => data.set([...rgb, 1], index * 4));
        const scene = new THREE.DataTexture(data, inputs.length, 1, THREE.RGBAFormat, THREE.FloatType);
        scene.needsUpdate = true;
        const target = new THREE.WebGLRenderTarget(inputs.length, 1, { depthBuffer: false, stencilBuffer: false });
        const held = { scene: comp.tScene.value, bloom: comp.tBloom.value, strength: comp.uBloom.value,
          material: post.quad.material };
        comp.tScene.value = scene;
        comp.tBloom.value = scene;
        comp.uBloom.value = 0;
        post.quad.material = post.comp;
        renderer.setRenderTarget(target);
        renderer.render(post.scene, post.lens);
        const bytes = new Uint8Array(inputs.length * 4);
        renderer.readRenderTargetPixels(target, 0, 0, inputs.length, 1, bytes);
        renderer.setRenderTarget(null);
        comp.tScene.value = held.scene;
        comp.tBloom.value = held.bloom;
        comp.uBloom.value = held.strength;
        post.quad.material = held.material;
        scene.dispose();
        target.dispose();
        return inputs.map((unused, index) => fromBytes(bytes, index * 4));
      };
      const bright = hues.map((hue) => {
        const source = sourceOf(hue);
        const peak = Math.max(...source);
        return source.map((value) => (value / peak) * 4);
      });
      const shown = composite(bright);
      const kept = bright.map((input, hue) => ({ hue, turn: round(turn(hueOf(shown[hue]), hueOf(input)), 1),
        share: round(satOf(shown[hue]) / satOf(input), 2) }));
      /* (라) 화면 안에 선 허브 둘(색 칸이 다른 군집의 가장 큰 별)의 가운데 — 장면에서 별만 남기고 한 장을 그려 투영한
       * 자리의 가장 밝은 픽셀을 읽는다. */
      const star = universe.starGeometry.attributes.aStar.array;
      universe.project();
      const onScreen = (seat) => universe.proj[seat * 4 + 3] > 0 && universe.proj[seat * 4] > 4
        && universe.proj[seat * 4 + 1] > 4 && universe.proj[seat * 4] < universe.width - 4
        && universe.proj[seat * 4 + 1] < universe.height - 4;
      const hubs = universe.map.galaxy.filter((row) => row.hue >= 0 && onScreen(row.hub)).slice(0, 2);
      const others = universe.scene.children.filter((object) => object.userData.key !== "stars");
      const visible = others.map((object) => object.visible);
      for (const object of others) object.visible = false;
      universe.render();
      const gl = universe.renderer.getContext();
      const ratio = universe.canvas.width / Math.max(1, universe.width);
      const coreOf = (seat) => {
        const spot = universe.project(seat);
        const x = Math.round(spot.x * ratio);
        const y = universe.canvas.height - 1 - Math.round(spot.y * ratio);
        const bytes = new Uint8Array(25 * 4);
        gl.readPixels(x - 2, y - 2, 5, 5, gl.RGBA, gl.UNSIGNED_BYTE, bytes);
        let best = [0, 0, 0];
        for (let index = 0; index < bytes.length; index += 4) {
          const rgb = fromBytes(bytes, index);
          if (Math.max(...rgb) > Math.max(...best)) best = rgb;
        }
        return { seat, light: round(star[seat * 4], 2), rgb: best.map((value) => round(value)), hue: round(hueOf(best), 1),
          sat: round(satOf(best), 2) };
      };
      const cores = hubs.map((row) => {
        const texel = (row.rank * KNOWLEDGE_UNIVERSE_SHAPE.texels + 2) * 4;
        const tint = Array.from(universe.map.gal.slice(texel, texel + 3));
        const core = coreOf(row.hub);
        return { ...core, rank: row.rank, cell: row.hue, wantHue: round(hueOf(tint), 1),
          turn: round(turn(core.hue, hueOf(tint)), 1), tintSat: round(satOf(tint), 2) };
      });
      others.forEach((object, index) => { object.visible = visible[index]; });
      universe.invalidate();
      await frame();
      /* 밝은 테마 — 빛깔의 원천은 같은 어두운 테마 값이고, 이름표 점은 종이 위의 색 칸이다. */
      setTheme("light");
      await frame();
      await frame();
      const lightHue = hues.map((hue) => colourOf(`var(--knowledge-hue-${hue})`));
      const lightTint = hues.map((hue) => colourOf(`var(--knowledge-3d-tint-${hue})`));
      const lightDots = dotsNow();
      const printed = composite([...bright, [4, 4, 4]]);
      const whiteInk = lumOf(printed[bright.length]);
      const inks = bright.map((input, hue) => ({ hue, turn: round(turn(hueOf(printed[hue]), hueOf(input)), 1),
        share: round(satOf(printed[hue]) / satOf(input), 2), light: round(lumOf(printed[hue]) / whiteInk) }));
      setTheme(heldTheme);
      await frame();
      knowledgeDimension = heldDimension;
      secondBrainVault = heldVault;
      knowledgePainterKind = heldPainter;
      await paintKnowledgeView();
      return { tints, darkHue, darkTint, lightHue, lightTint, darkDots, lightDots, kept, inks, cores };
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  }, { sizes: GALAXY_SIZES, alone: GALAXY_ALONE, quiet: GALAXY_QUIET, half: GALAXY_HALF });
  const detail = JSON.stringify(seen);
  ok("t-14081 ⑧: a cluster's tint is its colour cell's own hue (the token its legend chip and plate dot wear, in both themes) carrying at least half the cell's saturation, and it is as bright as a cluster without a colour",
    !seen.thrown && seen.tints.length >= 8
      && seen.tints.every((row) => row.turn <= 1 && row.share >= 0.5 && Math.abs(row.light - 1) <= 0.01)
      && seen.darkTint.every((colour, hue) => colour === seen.darkHue[hue])
      && seen.lightTint.every((colour, hue) => colour === seen.darkHue[hue])
      && seen.darkDots.length > 0 && seen.darkDots.every((row) => row.dot === seen.darkHue[row.hue])
      && seen.lightDots.length === seen.darkDots.length && seen.lightDots.every((row) => row.dot === seen.lightHue[row.hue]),
    detail);
  ok("t-14081 ⑧: the composite keeps a bright colour's hue within 10° and at least 60% of its saturation — a bright core does not wash out to white",
    !seen.thrown && seen.kept.length === 8 && seen.kept.every((row) => row.turn <= 10 && row.share >= 0.6), detail);
  ok("t-14081 ⑧: on paper a bright colour prints as ink of its own hue (within 20°) with at least half its saturation, never lighter than the white ink",
    !seen.thrown && seen.inks.length === 8 && seen.inks.every((row) => row.turn <= 20 && row.share >= 0.5 && row.light <= 1.02),
    detail);
  ok("universe ⑧: a hub's core keeps its cluster's colour — its hue within 25° of the cluster's tint and a clear share of its saturation — it does not wash out to white",
    !seen.thrown && seen.cores.length === 2 && seen.cores[0].cell !== seen.cores[1].cell
      && seen.cores.every((core) => core.seat >= 0 && core.turn <= 25 && core.sat >= 0.2),
    detail);
}
