/* A ```mermaid fence is drawn as a diagram in the seven Markdown views step A (t-58355) left as code
 * (t-58437): the conversation's prose, a CI check's output, a notebook's Markdown output and cell, and
 * the GitHub and GitLab bodies and comment cards. They share the road every Markdown view takes, and
 * the road is what the first checks hold:
 *   1. layouts are asked a few at a time, the rest in line: six fences on screen make two requests
 *      while their answers are held, and each answer frees a slot for the next;
 *   2. a source is asked once however many fences wait for it;
 *   3. the fence on screen is asked first whichever body it stands in — a conversation paints a body
 *      a row, so one body's fences off screen must not go before another's on screen;
 *   4. a layout already made is drawn as the words are painted: no request, no frame of code;
 *   5. a refused or empty layout keeps every fence that waited on it as code and writes one line to
 *      the window log for its one request, without the source; and it is asked again next time.
 *
 * Checked on bodies the checks paint themselves, with `render_mermaid` answered here (the layout
 * itself is tested in Rust). A fence is named by its first node's label up to its first space or `<`,
 * and the drawing carries that label whole as its node's text. Every wait is a condition read once a
 * frame, bounded; an answer the test needs to hold is held until the test releases it. Every read of
 * the page is defensive (`?.`, `?? null`): on a tree that does not draw these diagrams the checks fail
 * at their assertions and the suite runs to its end.
 *
 *   WINDOW_SUITES='^markdown-diagrams$' node ui/tests/window.mjs */
import { openWindowTestPage } from "./window-boot.mjs";

/* How many frames a condition is given before its check reads what stands (4 s at 60 frames a
 * second): the answers here come at once or when released, so a green tree meets each in a few. */
const FRAMES = 240;

/* The diagrams the backend answers with and the hands the checks share — stood in the page once. */
function standDiagrams(page) {
  return page.evaluate((frames) => {
    const seen = (window.__DIAGRAMS__ = { asked: [], waiting: [], hold: false, empty: false, logged: [], most: 0 });
    const labelOf = (source) => /\[([^\]]+)\]/.exec(source)?.[1] ?? "";
    const nameOf = (label) => label.split(/[\s<"]/, 1)[0];
    window.__NAME_OF__ = nameOf;
    // A drawing as wide as its name asks: a `넓은-` one is wider than any panel here, a `높은-` one
    // stands taller than the code it replaces by far more than a conversation's 50 px of slack.
    const sizeOf = (name) => (name.startsWith("넓은") ? { width: 1200, height: 160 }
      : name.startsWith("높은") ? { width: 420, height: 420 } : { width: 360, height: 160 });
    const drawn = (label) => {
      const { width, height } = sizeOf(nameOf(label));
      return {
        drawing: {
          width, height,
          marks: [
            { mark: "rect", x: 10, y: 40, width: 220, height: 60, radius: 4, class: "mmd-node" },
            { mark: "text", x: 120, y: 75, text: label, class: "mmd-label", anchor: "middle" },
            { mark: "polyline", points: [[230, 70], [width - 10, 70]], class: "mmd-edge", arrow: true },
            { mark: "text", x: width / 2, y: 64, text: "예", class: "mmd-edge-label", anchor: "middle" },
          ],
        },
        undrawn: null,
      };
    };
    // The harness's own count of the requests in flight (`__IN_FLIGHT__`), read as each layout is
    // asked: the most there ever were at once.
    window.__IN_FLIGHT__ = new Map();
    window.__ANSWER__.render_mermaid = ({ source }) => {
      const label = labelOf(source);
      seen.asked.push(nameOf(label));
      seen.most = Math.max(seen.most, window.__IN_FLIGHT__.get("render_mermaid") ?? 0);
      const answer = seen.empty ? null : drawn(label);
      if (!seen.hold) return answer;
      return new Promise((done) => seen.waiting.push({ name: nameOf(label), release: () => done(answer) }));
    };
    window.__ANSWER__.log_window_error = ({ message }) => {
      seen.logged.push(String(message));
      return null;
    };
    // A condition read once a frame, at most `frames` frames.
    window.__UNTIL__ = async (test, most = frames) => {
      for (let at = 0; at < most; at += 1) {
        if (test()) return true;
        await new Promise(requestAnimationFrame);
      }
      return Boolean(test());
    };
    window.__FRAMES__ = async (count) => {
      for (let at = 0; at < count; at += 1) await new Promise(requestAnimationFrame);
    };
    window.__RELEASE__ = (name) => {
      const at = seen.waiting.findIndex((one) => one.name === name);
      if (at < 0) return false;
      seen.waiting.splice(at, 1)[0].release();
      return true;
    };
    window.__RELEASE_ALL__ = () => {
      for (const one of seen.waiting.splice(0)) one.release();
    };
    // A body of the checks' own, fixed on screen and scrolling inside itself.
    window.__BOX__ = (top, height) => {
      const box = document.createElement("div");
      box.style.cssText = `position: fixed; left: 0; top: ${top}px; width: 420px; height: ${height}px; overflow: auto; z-index: 99999`;
      document.body.appendChild(box);
      return box;
    };
    window.__FENCE__ = (label) => ["```mermaid", "flowchart LR", `  A[${label}] --> B[끝]`, "```", ""].join("\n");
    window.__CODE__ = (body) => body?.querySelectorAll('pre.md-block[data-language="mermaid"]').length ?? null;
    window.__DRAWN__ = (body) => body?.querySelectorAll(".mmd-block svg.mmd").length ?? null;
  }, FRAMES);
}

export async function testMarkdownDiagrams(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await standDiagrams(page);

    /* ---- 1: a few at a time ------------------------------------------------------------------- */
    const few = await page.evaluate(async () => {
      const seen = window.__DIAGRAMS__;
      const box = window.__BOX__(0, 640);
      const names = ["하나-그림", "둘-그림", "셋-그림", "넷-그림", "다섯-그림", "여섯-그림"];
      seen.hold = true;
      seen.most = 0;
      const asks = seen.asked.length;
      if (typeof paintMarkdownView === "function") paintMarkdownView(box, names.map((name) => window.__FENCE__(name)).join("\n"));
      // The words stand, and the frame that first shows them begins: nothing is asked yet.
      const atPaint = seen.asked.length - asks;
      const atFrame = await new Promise((done) => requestAnimationFrame(() => done(seen.asked.length - asks)));
      const twoAsked = await window.__UNTIL__(() => seen.asked.length - asks >= 2);
      await window.__FRAMES__(5);
      const whileHeld = seen.asked.length - asks;
      // Released one at a time: each answer frees a slot for the next in line.
      for (let at = 0; at < names.length; at += 1) {
        if (!(await window.__UNTIL__(() => seen.waiting.length > 0, 60))) break;
        window.__RELEASE__(seen.waiting[0].name);
        await window.__FRAMES__(2);
      }
      seen.hold = false;
      window.__RELEASE_ALL__();
      const allDrawn = await window.__UNTIL__(() => window.__DRAWN__(box) === names.length);
      const seen6 = { atPaint, atFrame, twoAsked, whileHeld, most: seen.most, asked: seen.asked.slice(asks), allDrawn };
      box.remove();
      return seen6;
    });
    ok(
      "layouts are asked a few at a time: six fences on screen make no request before the frame that shows them, then two while their answers are held, each answer frees a slot for the next, and all six draw",
      few.atPaint === 0 && few.atFrame === 0 && few.twoAsked && few.whileHeld === 2 && few.most === 2
        && few.asked.length === 6 && new Set(few.asked).size === 6 && few.allDrawn,
      JSON.stringify(few),
    );

    /* ---- 2: once a source -------------------------------------------------------------------- */
    const same = await page.evaluate(async () => {
      const seen = window.__DIAGRAMS__;
      const box = window.__BOX__(0, 480);
      seen.hold = true;
      const asks = seen.asked.length;
      if (typeof paintMarkdownView === "function") {
        paintMarkdownView(box, [window.__FENCE__("같은-그림"), "사이 문단.", "", window.__FENCE__("같은-그림")].join("\n"));
      }
      const asked = await window.__UNTIL__(() => seen.asked.length > asks);
      await window.__FRAMES__(5);
      const requests = seen.asked.slice(asks).filter((one) => one === "같은-그림").length;
      const heldCode = box.querySelectorAll('.mmd-block pre.md-block[data-language="mermaid"]').length;
      seen.hold = false;
      window.__RELEASE_ALL__();
      const bothDrawn = await window.__UNTIL__(() => window.__DRAWN__(box) === 2);
      box.remove();
      return { asked, requests, heldCode, bothDrawn };
    });
    ok(
      "a source is asked once however many fences wait for it: two fences of one diagram make one request, wait as code together, and both draw from its answer",
      same.asked && same.requests === 1 && same.heldCode === 2 && same.bothDrawn,
      JSON.stringify(same),
    );

    /* ---- 3: on screen first, whichever body ---------------------------------------------------- */
    const across = await page.evaluate(async () => {
      const seen = window.__DIAGRAMS__;
      // Two bodies, the first painted first: its three fences stand below its fold, off screen, and
      // the second's one fence on screen.
      const first = window.__BOX__(0, 200);
      const second = window.__BOX__(240, 200);
      const filler = Array.from({ length: 40 }, (_, at) => `채움 문단 ${at}.`).join("\n\n");
      seen.hold = true;
      const asks = seen.asked.length;
      if (typeof paintMarkdownView === "function") {
        paintMarkdownView(first, [filler, "", window.__FENCE__("아래1-그림"), window.__FENCE__("아래2-그림"), window.__FENCE__("아래3-그림")].join("\n"));
        paintMarkdownView(second, window.__FENCE__("보이는-그림"));
      }
      await window.__UNTIL__(() => seen.asked.length - asks >= 2);
      await window.__FRAMES__(5);
      const whileHeld = seen.asked.slice(asks);
      seen.hold = false;
      window.__RELEASE_ALL__();
      const drawn = await window.__UNTIL__(() => window.__DRAWN__(first) === 3 && window.__DRAWN__(second) === 1);
      const asked = seen.asked.slice(asks);
      first.remove();
      second.remove();
      return { whileHeld, asked, drawn };
    });
    ok(
      "the fence on screen is asked first whichever body it stands in: after a body with three fences off screen, the next body's one fence on screen is the first request, and one more is all that is asked while the answers are held",
      across.whileHeld.length === 2 && across.whileHeld[0] === "보이는-그림" && across.asked.length === 4 && across.drawn,
      JSON.stringify(across),
    );

    /* ---- 4: a layout already made ------------------------------------------------------------- */
    const again = await page.evaluate(async () => {
      const seen = window.__DIAGRAMS__;
      seen.hold = false;
      const box = window.__BOX__(0, 320);
      if (typeof paintMarkdownView === "function") paintMarkdownView(box, window.__FENCE__("다시-그림"));
      const first = await window.__UNTIL__(() => window.__DRAWN__(box) === 1);
      box.remove();
      const asks = seen.asked.length;
      const next = window.__BOX__(0, 320);
      if (typeof paintMarkdownView === "function") paintMarkdownView(next, window.__FENCE__("다시-그림"));
      // Read in the paint's own task: no frame has passed, and nothing was awaited.
      const atPaint = { drawn: window.__DRAWN__(next), code: window.__CODE__(next) };
      await window.__FRAMES__(5);
      const later = { asked: seen.asked.length - asks, drawn: window.__DRAWN__(next) };
      next.remove();
      return { first, atPaint, later };
    });
    ok(
      "a layout already made is drawn as the words are painted: the same diagram painted again stands drawn in the paint's own task, never as code, and asks nothing",
      again.first && again.atPaint.drawn === 1 && again.atPaint.code === 0 && again.later.asked === 0 && again.later.drawn === 1,
      JSON.stringify(again),
    );

    /* ---- 5: a refused or empty layout --------------------------------------------------------- */
    const failed = await page.evaluate(async () => {
      const seen = window.__DIAGRAMS__;
      const lines = () => seen.logged.filter((line) => line.startsWith("mermaid"));
      const asks = () => window.__COUNTS__.render_mermaid ?? 0;
      const pair = (name) => [window.__FENCE__(name), "사이 문단.", "", window.__FENCE__(name)].join("\n");
      const paint = (box, text) => {
        if (typeof paintMarkdownView === "function") paintMarkdownView(box, text);
      };
      seen.hold = false;
      seen.logged.length = 0;
      // Refused: two fences of one diagram.
      window.__FAIL__.add("render_mermaid");
      const box = window.__BOX__(0, 480);
      const before = asks();
      paint(box, pair("거절-그림"));
      const logged = await window.__UNTIL__(() => lines().length > 0);
      await window.__FRAMES__(5);
      window.__FAIL__.delete("render_mermaid");
      const refused = {
        logged, asks: asks() - before, lines: lines().length, code: window.__CODE__(box), drawn: window.__DRAWN__(box),
        sourceless: lines().every((line) => !line.includes("거절-그림")),
      };
      box.remove();
      // Empty: the same, with an answer that is not a render.
      seen.empty = true;
      const second = window.__BOX__(0, 480);
      const between = asks();
      const linesBefore = lines().length;
      paint(second, pair("빈-그림"));
      const emptyLogged = await window.__UNTIL__(() => lines().length > linesBefore);
      await window.__FRAMES__(5);
      seen.empty = false;
      const empty = {
        logged: emptyLogged, asks: asks() - between, lines: lines().length - linesBefore, code: window.__CODE__(second),
        drawn: window.__DRAWN__(second), sourceless: lines().every((line) => !line.includes("빈-그림")),
      };
      second.remove();
      // Neither is remembered: painted again, the refused one is asked again and draws.
      const third = window.__BOX__(0, 320);
      const later = asks();
      paint(third, window.__FENCE__("거절-그림"));
      const drawn = await window.__UNTIL__(() => window.__DRAWN__(third) === 1);
      const retried = { drawn, asks: asks() - later };
      third.remove();
      return { refused, empty, retried, said: lines() };
    });
    ok(
      "a refused layout keeps both fences that waited on it as code and writes one line to the window log for its one request, without the diagram's source",
      failed.refused.logged && failed.refused.asks === 1 && failed.refused.lines === 1 && failed.refused.code === 2
        && failed.refused.drawn === 0 && failed.refused.sourceless,
      JSON.stringify(failed),
    );
    ok(
      "an empty layout does the same: one request, one line in the window log without the source, both fences still code",
      failed.empty.logged && failed.empty.asks === 1 && failed.empty.lines === 1 && failed.empty.code === 2
        && failed.empty.drawn === 0 && failed.empty.sourceless,
      JSON.stringify(failed),
    );
    ok(
      "a failed layout is not remembered: the same diagram painted again is asked again, once, and draws",
      failed.retried.drawn && failed.retried.asks === 1,
      JSON.stringify(failed.retried),
    );

    ok("the markdown-diagrams window raised no errors", faults.length === 0, faults.slice(0, 3).join(" | "));
  } finally {
    await page.close();
  }
}
