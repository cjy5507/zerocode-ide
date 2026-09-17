/* 흐르는 화면 위에서 친 키 — 당김 박자의 결정적 시험 (t-4418).
 *
 * docs/design/terminal-display-paced-frames-20260917.md §4.1 규칙 6, §6, §7.2.
 * 흐르는 화면은 알림을 기다리지 않고 제 화면 프레임(rAF)에 당긴다. 그 박자는
 * 흐르는 출력에는 맞고, 그 위에서 친 키의 에코에는 한 프레임이 늦다: 에코는
 * 다음 화면 프레임의 당김에 실려 그 프레임이 그린 뒤에 닿고, 그 다음 프레임에야
 * 그려진다. M2(`terminal-pacing.mjs`)가 그 값을 쟀다 — 6배 scroll 에코 p95
 * 35.9 → 41.1 ms, 4배 redraw 31.1 → 41.1 ms.
 *
 * 그래서 백엔드의 펌프는 셸이 키에 답한 출력을 파싱한 라운드에서 흐르는 창에도
 * 알리고(`zerocode_pty::Look::Chase`, 펌프의 `ChaseRound`), 창은 키가 간 뒤의 그
 * 알림을 곧바로 당긴다. 이 파일은 창의 그 절반을 박자 없이 고정한다 — 화면
 * 프레임은 시험이 줄 때만 온다.
 * 숫자는 M2의 몫이고, 여기서 지키는 것은 모양이다:
 *
 *   - 키 없이 흐르는 화면은 알림에 당기지 않는다(화면 프레임 하나에 당김 하나).
 *   - 키가 간 뒤의 알림은 다음 화면 프레임을 기다리지 않고 당긴다.
 *   - 당김이 날고 있는 사이에 온 에코 알림도, 그 답이 닿자마자 당긴다.
 *   - 그 값은 칠하기로 센다(설계 §6 합격선 2): 흐르는 출력만으로는 한 화면
 *     프레임이 두 번 칠해지지 않고, 키 하나가 더하는 칠은 많아야 하나다. */
import { installTerminalWaits } from "./terminal-grid-selection.mjs";

/* The rig every scene here stands on: a shell tab this window reads, frames
 * of it, and display frames that come only when the scene gives one. */
async function installCadenceRig(page) {
  await installTerminalWaits(page);
  await page.evaluate(() => {
    const ROWS = 3;
    const COLS = 24;
    window.__CADENCE_RIG__ = {
      wait: (ms) => new Promise((done) => setTimeout(done, ms)),
      frame: (text, full = false) => ({
        rows: [{ index: ROWS - 1, cells: [...text.padEnd(COLS, " ")].map((ch) => ({ ch })) }],
        scrolled_lines: 0,
        cursor: [ROWS - 1, text.length],
        title: null,
        alt_screen: false,
        size: [ROWS, COLS],
        full,
        cursor_visible: true,
        mouse_tracking: "off",
        mouse_sgr: false,
        bell: false,
        view_offset: 0,
        scrollback_len: 0,
        folds: [],
      }),
      /* A shell tab in front, read, with the keyboard on it — and the display
       * frames taken over. `giveFrame` runs every callback asked for so far,
       * as one display frame would; `close` hands everything back. */
      async open() {
        const priorSnapshot = window.__ANSWER__.term_snapshot;
        window.__ANSWER__.term_snapshot = () => null;
        const term = await openTermTab({ placement: "tab" });
        const view = termView(term);
        setActiveTab(tabOfTerm(term).id);
        await window.__TERMINAL_READY__(term, view);
        const frames = window.requestAnimationFrame;
        const cancelFrame = window.cancelAnimationFrame;
        const withheld = new Map();
        let frameSeq = 0;
        window.requestAnimationFrame = (callback) => {
          frameSeq += 1;
          withheld.set(frameSeq, callback);
          return frameSeq;
        };
        window.cancelAnimationFrame = (id) => {
          withheld.delete(id);
        };
        return {
          term,
          view,
          routed: keyboardTarget()?.term === term,
          shown: () => view.pre.querySelectorAll(":scope > .term-row")[ROWS - 1]?.textContent.trimEnd() ?? null,
          giveFrame() {
            const due = [...withheld.values()];
            withheld.clear();
            for (const callback of due) callback(performance.now());
          },
          close() {
            window.requestAnimationFrame = frames;
            window.cancelAnimationFrame = cancelFrame;
            for (const callback of withheld.values()) frames(callback);
            delete window.__ANSWER__.term_pull;
            if (priorSnapshot) window.__ANSWER__.term_snapshot = priorSnapshot;
            else delete window.__ANSWER__.term_snapshot;
            for (const tab of [...tabs]) dropTab(tab.id);
            for (const at of [...termViews.keys()]) dropTermView(at);
          },
        };
      },
    };
  });
}

export async function testTerminalCadence(page, ok) {
  await installCadenceRig(page);
  const seen = await page.evaluate(async () => {
    const { wait, frame } = window.__CADENCE_RIG__;
    const rig = await window.__CADENCE_RIG__.open();
    const { term, shown, giveFrame } = rig;
    const out = { routed: rig.routed };
    // Every pull the window makes, in order.
    let pulls = 0;
    const countPulls = (args) => {
      pulls += 1;
      delete window.__ANSWER__.term_pull;
      try {
        return window.__TAURI__.core.invoke("term_pull", args);
      } finally {
        window.__ANSWER__.term_pull = countPulls;
      }
    };
    window.__ANSWER__.term_pull = countPulls;
    try {
      // A quiet screen is told and pulls at once; what it brought makes it a
      // flowing one, whose next pull waits for its display frame.
      await window.__TERM_FEED__(term, frame("$ make", true));
      out.flowing = termPull.frame !== 0 && shown() === "$ make";

      // Output with no key behind it: told, and still left to the frame.
      const pullsBefore = pulls;
      const printed = window.__TERM_FEED__(term, frame("building 1"));
      await wait(40);
      out.outputWaitsForTheFrame = pulls === pullsBefore && shown() === "$ make";
      giveFrame();
      await printed;
      out.outputAfterTheFrame = shown();

      // A key goes to the pty, and the chase tells the flowing screen its echo.
      routeText("x");
      const echoed = window.__TERM_FEED__(term, frame("building 1 x"));
      const landed = await Promise.race([echoed.then(() => true), wait(40).then(() => false)]);
      out.echoWithoutAFrame = landed && shown() === "building 1 x";
      giveFrame();
      await echoed;

      // The echo told while a pull is in the air: that pull answers what it
      // left with, and the echo is pulled the moment it lands.
      let release = null;
      window.__ANSWER__.term_pull = (args) => {
        pulls += 1;
        delete window.__ANSWER__.term_pull;
        const answer = window.__TAURI__.core.invoke("term_pull", args);
        window.__ANSWER__.term_pull = countPulls;
        return new Promise((resolve) => {
          release = () => resolve(answer);
        });
      };
      const flowing = window.__TERM_FEED__(term, frame("building 2"));
      giveFrame();
      for (let tries = 0; release === null && tries < 50; tries += 1) {
        // eslint-disable-next-line no-await-in-loop
        await wait(2);
      }
      out.heldInTheAir = release !== null;
      routeText("y");
      const midFlight = window.__TERM_FEED__(term, frame("building 2 y"));
      await wait(10);
      release?.();
      await flowing;
      const caught = await Promise.race([midFlight.then(() => true), wait(40).then(() => false)]);
      out.midFlightEchoWithoutAFrame = caught && shown() === "building 2 y";
      giveFrame();
      await midFlight;
    } finally {
      rig.close();
    }
    return out;
  });
  ok("terminal-cadence: 키는 흐르는 판으로 간다, 그 판은 흐른다", seen.routed && seen.flowing, JSON.stringify(seen));
  ok(
    "a flowing screen leaves output with no key behind it to its next display frame",
    seen.outputWaitsForTheFrame && seen.outputAfterTheFrame === "building 1",
    JSON.stringify(seen),
  );
  ok(
    "the echo of a key typed over a flowing screen is pulled at the chase's notice, not at the next display frame",
    seen.echoWithoutAFrame,
    JSON.stringify(seen),
  );
  ok(
    "and an echo told while a pull is in the air is pulled as that pull lands, not on the frame after",
    seen.heldInTheAir && seen.midFlightEchoWithoutAFrame,
    JSON.stringify(seen),
  );

  /* The price of that, counted in paints (design §6, line 2). A display frame
   * that draws two paints drew one nobody saw. Output alone must never cost
   * that — it is the stale-frame queue the pull road exists to end — and a key
   * may cost at most one: its echo's paint, in the frame the output's paint
   * already took. Output here comes twice per display frame, faster than the
   * screen paints, and a key lands between two frames in some of them. */
  const painted = await page.evaluate(async () => {
    const { wait, frame } = window.__CADENCE_RIG__;
    const rig = await window.__CADENCE_RIG__.open();
    const { term, view, giveFrame } = rig;
    const applyFrames = view.applyFrames;
    let applies = 0;
    let drawnTwice = 0;
    view.applyFrames = function countedApply(frames) {
      applies += 1;
      return applyFrames.call(this, frames);
    };
    // A display frame draws whatever was painted since the last one.
    const drawFrame = () => {
      drawnTwice += Math.max(0, applies - 1);
      applies = 0;
      giveFrame();
    };
    // Room for a notice, its pull and the answer's paint to finish.
    const settle = () => wait(15);
    const flow = async (rounds, typeOn) => {
      drawnTwice = 0;
      let keys = 0;
      for (let round = 0; round < rounds; round += 1) {
        void window.__TERM_FEED__(term, frame(`${typeOn.name} ${round} a`));
        // eslint-disable-next-line no-await-in-loop
        await settle();
        void window.__TERM_FEED__(term, frame(`${typeOn.name} ${round} b`));
        // eslint-disable-next-line no-await-in-loop
        await settle();
        if (typeOn(round)) {
          routeText("k");
          keys += 1;
          void window.__TERM_FEED__(term, frame(`${typeOn.name} ${round} b k`));
          // eslint-disable-next-line no-await-in-loop
          await settle();
        }
        drawFrame();
        // eslint-disable-next-line no-await-in-loop
        await settle();
      }
      drawFrame();
      await settle();
      drawFrame();
      return { keys, drawnTwice };
    };
    try {
      const noKeys = function output() {
        return false;
      };
      const everyThird = function typing(round) {
        return round % 3 === 1;
      };
      return { output: await flow(12, noKeys), typing: await flow(12, everyThird) };
    } finally {
      view.applyFrames = applyFrames;
      rig.close();
    }
  });
  ok(
    "output alone never paints a display frame twice, however fast it comes",
    painted.output.keys === 0 && painted.output.drawnTwice === 0,
    JSON.stringify(painted),
  );
  ok(
    "and a key typed over it adds at most one paint to a frame: its echo's",
    painted.typing.keys > 0 && painted.typing.drawnTwice <= painted.typing.keys,
    JSON.stringify(painted),
  );
}
