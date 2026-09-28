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
