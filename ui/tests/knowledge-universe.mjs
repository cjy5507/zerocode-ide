/* ---- 지식 그래프의 우주 보기 (t-12443, 승인된 시안 v4 「실제 우주」) ---------------------------
 *
 * 전체 지도는 WebGL2가 서는 판에서 우주로 열리고, 머리의 「2D | 3D」가 평면 지도와 우주를 오간다.
 * 이 파일이 묻는 것은 계약이다 — 어느 판에서 우주가 서는가, 오가도 평면 지도의 자리가 그대로인가,
 * 사람이 고른 쪽을 기억하는가, 쉴 때 한 장도 그리지 않는가, 가려지면 멈추는가, 문맥을 잃으면
 * 평면으로 돌아와 한 줄로 말하는가, 떠날 때 GPU의 것을 다 놓는가. 그림이 시안과 같은지는 사진이
 * 말하고(`knowledge-gpu.mjs --universe`), 시간은 진짜 GPU에서 잰다. 여기(소프트웨어 GL)의 수는
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
        lost = { ...standing(), lost: lossy.lost, notice: notice?.textContent ?? "",
          noticeShown: notice?.classList.contains("is-on") ?? false,
          wanted: t("knowledge.universeLost", "그래픽 장치가 우주 그림을 놓아 평면 지도로 보여 줍니다 — 돌아오면 다시 섭니다") };
        lose.restoreContext();
        await until(() => lossy.lost === false, 240);
        await frames(4);
        restored = { ...standing(), lost: lossy.lost,
          notice: view.querySelector(".knowledge-universe-notice")?.textContent ?? "",
          wanted: t("knowledge.universeBack", "그래픽 장치가 돌아와 우주를 다시 엽니다") };
      }

      knowledgePainterKind = heldPainter;
      knowledgeDimension = heldDimension;
      secondBrainVault = heldVault;
      await paintKnowledgeView();
      return { opened, idleFrames, pickDrew, pickSettled, flat, released, rememberedFlat, back, remembered3d,
        sameSeats, sameCamera, reopenedFlat, reopened3d, local, fromLocal, lost, restored };
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

/* ---- ② 은하 짓기·별·빛 번짐과 합성 ---------------------------------------------------------------
 *
 * 규칙이 맞게 섰는지를 셈으로 묻는다(그림의 결은 사진이 말한다). 볼트는 여기서 짓는다: 쪽 수가 다른 스물두
 * 무리(무리 안은 촘촘하고 무리 사이는 잇지 않는다)와 홀로 선 쪽 여섯 — 평면 지도의 군집이 그대로 무리가 되도록. 무리
 * 9와 12는 오래전에 고쳤고(활동 0), 무리 15는 절반만 요즘 고쳤으며(활동 0.5 — 고정 문턱 0.12보다 바쁘지만 은하
 * 활동의 가운데값보다 조용하다), 나머지는 오늘 고쳤다. */
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
      /* 무리마다 사슬 하나와 앞선 쪽 둘로의 가지(결정적), 이웃 무리로 한 가닥. */
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
        /* 무리 사이는 잇지 않는다 — 한 가닥이라도 이으면 평면 지도(Louvain)가 작은 이웃 무리를 합쳐 순위가 밀린다. */
      });
      /* 무리 0의 첫 쪽은 제 무리 전부를 가리키는 허브다 — 연결 비가 광도의 천장(`star-ratio-cap`)을 넘는다. */
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
      const universe = knowledgeUniverses.get(view);
      const map = universe.map;
      const tune = knowledgeUniverseTuning(view);
      const pages = layout.model.kinds.filter((kind) => kind === "page").length;
      const least = Math.max(tune.galaxyLeast, Math.round((tune.galaxyShare * pages) / tune.worldPages));
      let wantGalaxies = 0;
      while (wantGalaxies < Math.min(layout.namedCount, tune.galaxyMax)
        && layout.communitySize[wantGalaxies] >= least) wantGalaxies += 1;
      /* 형태의 규칙을 표 밖에서 다시 센다. */
      const activityOf = (rank) => {
        let recent = 0;
        let all = 0;
        for (let seat = 0; seat < layout.count; seat += 1) {
          if (layout.community[seat] !== rank) continue;
          all += 1;
          if (layout.model.modified[seat] > 0
            && layout.model.nowMs - layout.model.modified[seat] < tune.activityDays * 86_400_000) recent += 1;
        }
        return all > 0 ? recent / all : 0;
      };
      const T = KNOWLEDGE_UNIVERSE_TYPES;
      /* 타원의 문턱 = max(토큰, 은하 활동의 가운데값 — 짝수면 가운데 둘의 평균)(디자이너 m-12546의 3). */
      const busy = Array.from({ length: wantGalaxies }, (unused, rank) => activityOf(rank)).sort((a, b) => a - b);
      const middleBusy = busy.length === 0 ? 0 : busy.length % 2 === 1 ? busy[busy.length >> 1]
        : (busy[busy.length / 2 - 1] + busy[busy.length / 2]) / 2;
      const quietBelow = Math.max(tune.ellipticalActivity, middleBusy);
      const wantType = (rank) => {
        if (rank >= wantGalaxies) return T.cluster;
        if (rank < tune.barredRanks) return T.barred;
        if (wantGalaxies >= tune.irregularLeast && rank >= wantGalaxies - tune.irregularLast) return T.irregular;
        if (rank >= tune.ellipticalFrom && rank % tune.ellipticalEvery === 0
          && activityOf(rank) < quietBelow) return T.elliptical;
        return T.spiral;
      };
      const rows = map.galaxy.map((row) => ({ rank: row.rank, type: row.type, want: wantType(row.rank),
        radius: Math.round(row.radius * 1000) / 1000,
        wantRadius: Math.round((row.rank < wantGalaxies
          ? layout.communityHomeR[row.rank] * map.scale * tune.galaxyRadius
          : layout.communityHomeR[row.rank] * map.scale * (tune.clusterSphere / tune.worldRoot) * tune.clusterGrow
            + tune.clusterPad) * 1000) / 1000 }));
      /* 떠돌이: 이름 없는 군집의 쪽은 줄이 없다. */
      let strays = 0;
      let strayRows = 0;
      for (let seat = 0; seat < layout.count; seat += 1) {
        if (layout.community[seat] < layout.namedCount) continue;
        strays += 1;
        if (map.rowOf[seat] !== -1) strayRows += 1;
      }
      /* 이름 있는 별의 밝기·빛깔(시안 `buildScene`). */
      const star = universe.starGeometry.attributes.aStar.array;
      const sorted = Int32Array.from(layout.model.degree).sort();
      const middle = sorted[layout.count >> 1];
      let lumWorst = 0;
      let tempRange = [1, 0];
      let unknownTemp = -1;
      let capped = 0;
      for (let seat = 0; seat < layout.count; seat += 1) {
        const ratio = (1 + layout.model.degree[seat]) / (1 + middle);
        if (ratio > tune.starRatioCap) capped += 1;
        const want = Math.min(ratio, tune.starRatioCap) ** tune.starLumExp;
        lumWorst = Math.max(lumWorst, Math.abs(star[seat * 4] - want) / want);
        tempRange = [Math.min(tempRange[0], star[seat * 4 + 1]), Math.max(tempRange[1], star[seat * 4 + 1])];
        if (layout.model.modified[seat] <= 0) unknownTemp = star[seat * 4 + 1];
      }
      /* 같은 볼트는 같은 우주다 — 다시 지어도 텍셀과 별 자리가 한 자도 같다. */
      const galBefore = Float32Array.from(map.gal);
      const seatsBefore = Float32Array.from(universe.pos3);
      universe.build(layout);
      const rebuilt = galBefore.every((value, index) => value === universe.map.gal[index])
        && seatsBefore.every((value, index) => value === universe.pos3[index]);
      /* 은하 빛깔: 색 칸이 있으면 어두운 테마 값에서, 없으면 고정값 — 두 테마에서 같다. */
      const tintAt = (rank) => Array.from(universe.map.gal.slice((rank * KNOWLEDGE_UNIVERSE_SHAPE.texels + 5) * 4,
        (rank * KNOWLEDGE_UNIVERSE_SHAPE.texels + 5) * 4 + 3)).map((value) => Math.round(value * 1000) / 1000);
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
        paper: universe.post.comp.uniforms.uPaper.value.toArray().map((value) => Math.round(value * 100) / 100) };
      setTheme("dark");
      await frame();
      /* 한 장의 드로우: 별 1 + 번짐 내리기(단 수) + 올리기(단 수 − 1) + 합성 1. 목표는 장면 한 장과 단 수. */
      universe.invalidate();
      await frame();
      await frame();
      const draws = universe.renderer.info.render.calls;
      const sceneDraws = universe.scene.children.filter((object) => object.visible).length;
      const targets = { scene: universe.post.target !== null, mips: universe.post.mips.length,
        half: universe.post.target?.texture.type === THREE.HalfFloatType, floats: universe.floatTargets };
      knowledgeDimension = heldDimension;
      secondBrainVault = heldVault;
      knowledgePainterKind = heldPainter;
      await paintKnowledgeView();
      return { named: layout.namedCount, communities: layout.communityCount, sizes: Array.from(layout.communitySize),
        galaxies: map.galaxies, wantGalaxies, least, rowsCount: map.rows, rows, strays, strayRows,
        lumWorst, capped, tempRange, unknownTemp, rebuilt, quietBelow, darkTints, lightTints, darkComp, lightComp,
        neutral: knowledgeUniverseInks(view, universe.probe ?? document.body).neutral, draws, sceneDraws, targets,
        bloomLevels: tune.bloomLevels };
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  }, { sizes: GALAXY_SIZES, alone: GALAXY_ALONE, quiet: GALAXY_QUIET, half: GALAXY_HALF });
  const detail = JSON.stringify(seen);
  const T = { spiral: 0, barred: 1, elliptical: 2, irregular: 3, cluster: 4 };
  ok("t-12443 ②: the named clusters become galaxies from the front (at most the token's count, at least the page floor) and the rest clusters, each with the prototype's radius",
    !seen.thrown && seen.galaxies === seen.wantGalaxies && seen.galaxies === 18
      && seen.rowsCount === seen.named && seen.rows.every((row) => row.radius === row.wantRadius),
    detail);
  ok("t-12443 ②: the shape rule — ranks 0 and 1 barred, the last two galaxies irregular, ranks 9, 12 and 15 elliptical when quieter than the galaxies' median (and the token's floor), the rest spiral, clusters beyond",
    !seen.thrown && seen.rows.every((row) => row.type === row.want)
      && seen.rows[0]?.type === T.barred && seen.rows[1]?.type === T.barred
      && seen.rows[16]?.type === T.irregular && seen.rows[17]?.type === T.irregular
      && seen.rows[9]?.type === T.elliptical && seen.rows[12]?.type === T.elliptical
      && seen.rows[15]?.type === T.elliptical && seen.rows[3]?.type === T.spiral && seen.rows[18]?.type === T.cluster,
    detail);
  ok("t-12443 ②: a page of an unnamed cluster is a stray star with no galaxy row",
    !seen.thrown && seen.strays > 0 && seen.strayRows === 0, detail);
  ok("t-12443 ②: a star's light is its links over the median, capped at the token's ratio, to the token's power, its colour its last edit, and an unknown edit reads as the oldest",
    !seen.thrown && seen.capped >= 1 && seen.lumWorst < 1e-5 && seen.tempRange[0] >= 0 && seen.tempRange[1] <= 1
      && seen.tempRange[1] > 0.9 && seen.unknownTemp === 0,
    detail);
  ok("t-12443 ②: the same vault builds the same universe — every galaxy texel and every star seat",
    !seen.thrown && seen.rebuilt, detail);
  ok("t-12443 ②: a galaxy's tint is the same in both themes — its hue's dark value, or the neutral tint for a cluster without a hue — while the composite turns to the light theme's exposure and paper",
    !seen.thrown && JSON.stringify(seen.darkTints) === JSON.stringify(seen.lightTints)
      && seen.darkTints.lit.every((value) => value >= 0.7 && value <= 1)
      && (seen.darkTints.quiet === null || seen.darkTints.quiet.join(",") === seen.neutral.join(","))
      && seen.darkComp.dark === 1 && seen.darkComp.exposure === 1
      && seen.lightComp.dark === 0 && seen.lightComp.exposure === 1.25
      && seen.lightComp.paper.join(",") === "0.8,0.77,0.7",
    detail);
  ok("t-12443 ②: one frame is the scene plus the bloom ladder down and up and the composite, into one scene target and the token's levels",
    !seen.thrown && seen.sceneDraws >= 1 && seen.draws === seen.sceneDraws + seen.bloomLevels + (seen.bloomLevels - 1) + 1
      && seen.targets.scene && seen.targets.mips === seen.bloomLevels
      && seen.targets.half === seen.targets.floats,
    detail);
}

/* ---- ③ 은하의 몸 — 원반·핵·성운과 이름 없는 별·배경 별 ------------------------------------------
 *
 * 몸은 셰이더가 번호에서 짓는다(입자마다 float 하나): 줄마다 제 장식 수만큼, 그 뒤 배경 별. 원반·핵·성운은
 * 사각형 인스턴스이고, 성운은 앞 열두 성단에만 두른다(fbm 조각이 가장 비싸다). 한 장은 시안의 열셋이다. */
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
      /* 큰 무리는 이웃 무리와 한 가닥으로 잇고, 작은 무리(성단이 될 것)는 홀로 선 완전 그래프로 둔다 — 서로 이으면
       * 평면 지도가 작은 무리들을 한 군집으로 합친다. */
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
      const universe = knowledgeUniverses.get(view);
      const map = universe.map;
      const tune = knowledgeUniverseTuning(view);
      const SHAPE = KNOWLEDGE_UNIVERSE_SHAPE;
      const T = KNOWLEDGE_UNIVERSE_TYPES;
      const keyed = (key) => universe.scene.children.find((object) => object.userData.key === key) ?? null;
      const decor = keyed("decor");
      const glows = keyed("glows");
      const codes = decor?.geometry.attributes.position.array ?? new Float32Array(0);
      /* 줄마다의 장식 수(텍셀 6의 x)와 번호의 무리가 맞는가, 그 뒤 배경 별이 토큰의 수만큼인가. */
      const perRow = new Map();
      let sky = 0;
      let largest = 0;
      for (const code of codes) {
        largest = Math.max(largest, code);
        const group = Math.floor(code / SHAPE.groupSpan + 1e-4);
        if (group === SHAPE.skyGroup) sky += 1;
        else if (group < SHAPE.rows) perRow.set(group, (perRow.get(group) ?? 0) + 1);
      }
      const decorAgrees = map.galaxy.every((row) => (perRow.get(row.rank) ?? 0) === row.decor
        && map.gal[(row.rank * SHAPE.texels + 6) * 4] === row.decor);
      const wantDecor = map.galaxy.every((row) => row.decor === (row.type === T.cluster ? tune.decorCluster
        : Math.round(tune.decorLeast + tune.decorGrow * Math.sqrt(knowledgeLayouts.get(view).communitySize[row.rank]
          / knowledgeLayouts.get(view).communitySize[0]))));
      /* 원반·핵·성운의 인스턴스: 나선·막대·불규칙은 원반과 핵, 타원은 핵 하나, 성운은 앞 열두 성단에 셋씩. */
      const bb = glows?.geometry.attributes.aBB.array ?? new Float32Array(0);
      const instances = glows?.geometry.instanceCount ?? 0;
      const byRow = new Map();
      for (let one = 0; one < instances; one += 1) {
        const row = bb[one * 4];
        const kind = bb[one * 4 + 1];
        const list = byRow.get(row) ?? [];
        list.push(kind);
        byRow.set(row, list);
      }
      let clustersSeen = 0;
      const glowsAgree = map.galaxy.every((row) => {
        const kinds = (byRow.get(row.rank) ?? []).join(",");
        if (row.type === T.cluster) {
          clustersSeen += 1;
          return kinds === (clustersSeen <= tune.nebulaMax ? "2,2,2" : "");
        }
        return kinds === (row.type === T.elliptical ? "1" : "0,1");
      });
      universe.invalidate();
      await frame();
      await frame();
      const draws = universe.renderer.info.render.calls;
      const points = universe.renderer.info.render.points;
      const orders = universe.scene.children.map((object) => `${object.userData.key}:${object.renderOrder}`).join(" ");
      const stars = knowledgeLayouts.get(view)?.count;
      knowledgeDimension = heldDimension;
      secondBrainVault = heldVault;
      await paintKnowledgeView();
      return { rows: map.rows, galaxies: map.galaxies, codes: codes.length, sky, largest, decorAgrees, wantDecor,
        instances, glowsAgree, clusters: map.rows - map.galaxies, draws, points, stars, orders,
        bloomLevels: tune.bloomLevels, decorSky: tune.decorSky };
    } catch (error) {
      return { thrown: String(error?.stack ?? error) };
    }
  }, { sizes: [...GALAXY_SIZES, 6, 5, 5, 4, 4, 4, 3, 3, 3, 3, 3, 3, 3, 3], alone: GALAXY_ALONE,
    galaxySizes: GALAXY_SIZES.length });
  const detail = JSON.stringify(seen);
  ok("t-12443 ③: every galaxy and cluster carries its body's nameless stars in the prototype's count (texel 6), and the sky carries the token's count after them",
    !seen.thrown && seen.decorAgrees && seen.wantDecor && seen.sky === seen.decorSky
      && seen.largest < 2 ** 24,
    detail);
  ok("t-12443 ③: discs and cores stand for the galaxies (one core for an elliptical) and nebulae only around the first twelve clusters",
    !seen.thrown && seen.clusters > 12 && seen.glowsAgree, detail);
  ok("t-12443 ③: one frame is the prototype's thirteen draws — discs, bodies and stars in that order, the bloom ladder and the composite",
    !seen.thrown && seen.draws === 3 + seen.bloomLevels + (seen.bloomLevels - 1) + 1
      && seen.orders === "glows:0 decor:1 stars:2",
    detail);
}
