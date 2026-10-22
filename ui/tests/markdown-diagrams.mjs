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
 * Then the six panels, each in its own column: a CI check's output, a notebook's Markdown cell and
 * output, and the GitHub and GitLab bodies and comment cards draw the fence where it stood, its
 * label holding `<img src=x onerror=...>` as text (these are someone else's words), the drawing kept
 * in the panel's column and legible at the window's caption size; the check pane and the GitHub item,
 * painted again, draw in that paint with no request; and a remote body whose layout is refused keeps
 * its fence as code with one line in the window log.
 *
 * Checked on bodies the checks paint themselves and on the panels' own painters, with
 * `render_mermaid` answered here (the layout itself is tested in Rust). A fence is named by its first
 * node's label up to its first space or `<`,
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

/* The diagrams the backend answers with and the hands the checks share — stood in the page once
 * (and in the conversation's, `conversation-diagrams.mjs`). */
export function standDiagrams(page) {
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
    // A dialog stands still: its entrance (`surface-enter`, a 0.95 scale over --motion-dialog) has
    // ended on the scrim and on its card, and neither wears a transform, so a size read is the size a
    // person sees — not one scaled mid-entrance.
    window.__STILL__ = (scrim) => window.__UNTIL__(() => {
      const card = scrim?.querySelector(".qo") ?? null;
      if (!scrim || !card || scrim.hidden) return false;
      const moving = [scrim, card].some((node) => node.getAnimations().some((one) => one.playState === "running" || one.playState === "pending"));
      return !moving && getComputedStyle(card).transform === "none" && scrim.style.transform === "";
    });
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
    // What a hostile label carries: an image with a handler. Remote bodies (a check's output, an
    // issue, a comment) are someone else's words, so it must arrive as text.
    window.__HOSTILE__ = `<img src=x onerror="window.__PWNED__ = 7">`;
    // A body's words around a diagram named `name`, its label carrying the hostile markup.
    window.__AROUND__ = (before, name, after) => [before, "", window.__FENCE__(`${name} ${window.__HOSTILE__}`), after, ""].join("\n");
    // What one painted body shows of the diagram named `name`: whether it stands where its fence stood,
    // its label as text, no code of it left, the drawing kept in the body's column (fitted, or
    // scrolling inside its own block) and legible at the window's caption size, and no markup run.
    window.__READ_DIAGRAM__ = (body, name) => {
      const svg = [...(body?.querySelectorAll(".mmd-block svg.mmd") ?? [])]
        .find((one) => nameOf(one.querySelector("text.mmd-label")?.textContent ?? "") === name) ?? null;
      const block = svg?.closest(".mmd-block") ?? null;
      const width = svg?.getBoundingClientRect().width ?? 0;
      const natural = svg?.viewBox.baseVal.width ?? 0;
      const sizes = [...(svg?.querySelectorAll("text") ?? [])].map((text) => parseFloat(getComputedStyle(text).fontSize));
      const caption = parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--text-3xs"));
      return {
        drawn: svg !== null,
        labelled: svg?.querySelector("text.mmd-label")?.textContent === `${name} ${window.__HOSTILE__}`,
        before: block?.previousElementSibling?.textContent.trim() ?? null,
        after: block?.nextElementSibling?.textContent.trim() ?? null,
        codeLeft: [...(body?.querySelectorAll('pre.md-block[data-language="mermaid"]') ?? [])].filter((pre) => pre.textContent.includes(name)).length,
        contained: svg !== null && width > 0 && (block.clientWidth >= width - 1 || block.scrollWidth > block.clientWidth + 1)
          && body.scrollWidth <= body.clientWidth + 1,
        legible: natural > 0 && sizes.length > 0 && caption > 0 && (Math.min(...sizes) * width) / natural >= caption - 0.05,
        markup: body?.querySelectorAll("img, script").length ?? null,
        pwned: window.__PWNED__ ?? null,
        // The sizes the reads above came from, for a reader of a failure.
        sizes: {
          svg: Math.round(width * 10) / 10, natural, block: [block?.clientWidth ?? null, block?.scrollWidth ?? null],
          body: [body?.clientWidth ?? null, body?.scrollWidth ?? null], smallestPx: sizes.length ? Math.min(...sizes) : null, caption,
        },
      };
    };
  }, FRAMES);
}

/* A panel's diagram, read: drawn where its fence stood between the words before and after it, its
 * label text, no code of it left, kept in its column, legible, nothing run. */
function readsRight(read, before, after) {
  return read.drawn && read.labelled && read.before === before && read.after === after && read.codeLeft === 0
    && read.contained && read.legible && read.markup === 0 && read.pwned === null;
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

    /* ---- the six panels: each its own column ------------------------------------------------ */
    await page.setViewportSize({ width: 1440, height: 900 });

    // A CI check's output in the source-control column (`checkOutputEl`): the pane is built again
    // with each paint of the list — a poll whose answer changed, a row opened — so the diagram must
    // stand in that paint, with no request.
    const check = await page.evaluate(async () => {
      const seen = window.__DIAGRAMS__;
      seen.hold = false;
      const summary = window.__AROUND__("검사 앞 문단.", "검사-그림", "검사 뒤 문단.");
      window.__CHECKS__ = {
        review: { number: 42, title: "그림 검사", url: "https://github.com/o/r/pull/42", state: "open", draft: false, head_sha: "abc", owner_repo: "o/r" },
        checks: [{ name: "lint (diagram)", status: "completed", conclusion: "failure", url: "https://github.com/o/r/actions/runs/1", check_run_id: 11, workflow_run_id: 1 }],
        at_page_limit: false,
      };
      window.__CHECK_DETAILS__ = {
        "lint (diagram)": { name: "lint (diagram)", status: "completed", conclusion: "failure", title: "그림 하나", summary, annotations: [], jobs: [] },
      };
      revealActivity("checks");
      const row = () => document.querySelector("#checks-list .check-row");
      const listed = await window.__UNTIL__(() => row());
      row()?.click();
      const prose = () => document.querySelector("#checks-list .check-pane .check-prose");
      const stood = await window.__UNTIL__(() => prose()?.textContent.includes("검사 뒤 문단."));
      await window.__UNTIL__(() => window.__DRAWN__(prose()) === 1);
      const drawn = window.__READ_DIAGRAM__(prose(), "검사-그림");
      const asks = seen.asked.length;
      paintChecks();
      const again = { drawn: window.__DRAWN__(prose()), code: window.__CODE__(prose()) };
      await window.__FRAMES__(3);
      again.asked = seen.asked.length - asks;
      const list = document.getElementById("checks-list");
      return { listed: Boolean(listed), stood, drawn, again, listSideways: list ? list.scrollWidth > list.clientWidth + 1 : null };
    });
    ok(
      "a CI check's output draws its ```mermaid fence as a diagram where the fence stood, its label holding <img src=x onerror=...> as text, kept in the check pane's column and legible, the list not pushed sideways",
      check.listed && check.stood && readsRight(check.drawn, "검사 앞 문단.", "검사 뒤 문단.") && check.listSideways === false,
      JSON.stringify(check),
    );
    ok(
      "the check pane, built again with each paint of the list, stands with its diagram drawn in that paint and asks nothing",
      check.again.drawn === 1 && check.again.code === 0 && check.again.asked === 0,
      JSON.stringify(check.again),
    );

    // A notebook: a Markdown cell and a Markdown output (`notebookCellEl`, `notebookItemEl`).
    const notebook = await page.evaluate(async () => {
      const book = {
        nbformat: 4,
        metadata: { kernelspec: { display_name: "Python 3", language: "python" } },
        cells: [
          { cell_type: "markdown", source: window.__AROUND__("셀 앞 문단.", "셀-그림", "셀 뒤 문단.") },
          { cell_type: "code", execution_count: 1, source: "show()\n", outputs: [
            { output_type: "display_data", data: { "text/markdown": window.__AROUND__("출력 앞 문단.", "넓은출력-그림", "출력 뒤 문단.") } },
          ] },
        ],
      };
      const read = window.__ANSWER__.read_text_file;
      window.__ANSWER__.read_text_file = (args) => (args.path === "/w/repo/diagrams.ipynb" ? { text: JSON.stringify(book), version: 1 } : read?.(args));
      await openPath("/w/repo/diagrams.ipynb");
      const board = () => [...document.querySelectorAll(".ipynb-view")].find((one) => !one.hidden)?.querySelector(".ipynb-page") ?? null;
      const stood = await window.__UNTIL__(() => board()?.textContent.includes("출력 뒤 문단."));
      await window.__UNTIL__(() => window.__DRAWN__(board()) === 2);
      const boxes = [...(board()?.querySelectorAll(".nb-md") ?? [])];
      const seen = {
        stood,
        cell: window.__READ_DIAGRAM__(boxes[0] ?? null, "셀-그림"),
        output: window.__READ_DIAGRAM__(boxes[1] ?? null, "넓은출력-그림"),
        boardSideways: board() ? board().scrollWidth > board().clientWidth + 1 : null,
      };
      window.__ANSWER__.read_text_file = read;
      return seen;
    });
    ok(
      "a notebook's Markdown cell and its Markdown output draw their fences as diagrams where they stood, labels as text, each kept in its cell's column and legible, a drawing wider than the cell scrolling inside its block",
      notebook.stood && readsRight(notebook.cell, "셀 앞 문단.", "셀 뒤 문단.") && readsRight(notebook.output, "출력 앞 문단.", "출력 뒤 문단.")
        && notebook.boardSideways === false,
      JSON.stringify(notebook),
    );

    // A GitHub issue's body and a comment on it (`paintGithubItem`, `itemVoiceCard`), and the item
    // painted again from what it told.
    const github = await page.evaluate(async () => {
      const seen = window.__DIAGRAMS__;
      const body = window.__AROUND__("본문 앞 문단.", "넓은깃허브-그림", "본문 뒤 문단.");
      const comment = window.__AROUND__("댓글 앞 문단.", "깃허브댓글-그림", "댓글 뒤 문단.");
      const item = { kind: "issue", number: 7, title: "그림이 든 이슈", url: "https://github.com/acme/app/issues/7", state: "open", author: "kim" };
      window.__ANSWER__.github_work_item_detail = () => ({
        ...item, opened_at: "2026-08-01T09:00:00Z", body,
        comments: [{ author: "lee", written_at: "2026-08-02T10:00:00Z", body: comment }],
      });
      openGithubItem(item);
      const card = () => el("gh-item-comments")?.querySelector(".gh-item-comment-body") ?? null;
      const stood = await window.__UNTIL__(() => el("gh-item-body")?.textContent.includes("본문 뒤 문단.") && card()?.textContent.includes("댓글 뒤 문단."));
      await window.__UNTIL__(() => window.__DRAWN__(el("gh-item-body")) === 1 && window.__DRAWN__(card()) === 1);
      // Sizes are read once the dialog has finished arriving, as a person reads it.
      const still = await window.__STILL__(el("gh-item-scrim"));
      const read = { still, body: window.__READ_DIAGRAM__(el("gh-item-body"), "넓은깃허브-그림"), comment: window.__READ_DIAGRAM__(card(), "깃허브댓글-그림") };
      const asks = seen.asked.length;
      if (ghItem?.told) paintGithubItem(ghItem.told);
      const again = { body: window.__DRAWN__(el("gh-item-body")), comment: window.__DRAWN__(card()) };
      await window.__FRAMES__(3);
      again.asked = seen.asked.length - asks;
      closeGithubItem();
      delete window.__ANSWER__.github_work_item_detail;
      return { stood, ...read, again };
    });
    ok(
      "a GitHub issue's body and its comment card draw their fences as diagrams where they stood, labels holding <img src=x onerror=...> as text, kept in the dialog's column and legible",
      github.stood && github.still && readsRight(github.body, "본문 앞 문단.", "본문 뒤 문단.") && readsRight(github.comment, "댓글 앞 문단.", "댓글 뒤 문단."),
      JSON.stringify(github),
    );
    ok(
      "the GitHub item painted again from what it told stands with both diagrams drawn in that paint and asks nothing",
      github.again.body === 1 && github.again.comment === 1 && github.again.asked === 0,
      JSON.stringify(github.again),
    );

    // A GitLab merge request's body and a comment on it (`paintGitlabItem`, `itemVoiceCard`); then
    // one whose layout is refused.
    const gitlab = await page.evaluate(async () => {
      const seen = window.__DIAGRAMS__;
      const lines = () => seen.logged.filter((line) => line.startsWith("mermaid"));
      const asks = () => window.__COUNTS__.render_mermaid ?? 0;
      const item = (number, body, comments) => ({
        kind: "mr", number, title: `그림이 든 MR ${number}`, state: "opened", author: "hana", created_at: "2026-08-01T09:00:00Z",
        url: `https://gitlab.com/acme/app/-/merge_requests/${number}`, labels: [], body, comments,
      });
      const card = () => el("gl-item-comments")?.querySelector(".gl-item-comment-body") ?? null;
      let detail = item(42, window.__AROUND__("본문 앞 문단.", "깃랩-그림", "본문 뒤 문단."), [
        { author: "lee", created_at: "2026-08-02T10:00:00Z", body: window.__AROUND__("댓글 앞 문단.", "넓은깃랩댓글-그림", "댓글 뒤 문단."), resolved: null },
      ]);
      window.__ANSWER__.gitlab_item_detail = () => detail;
      openGitlabItem({ kind: "mr", number: 42, title: detail.title, state: "opened", url: detail.url });
      const stood = await window.__UNTIL__(() => el("gl-item-body")?.textContent.includes("본문 뒤 문단.") && card()?.textContent.includes("댓글 뒤 문단."));
      await window.__UNTIL__(() => window.__DRAWN__(el("gl-item-body")) === 1 && window.__DRAWN__(card()) === 1);
      // Sizes are read once the dialog has finished arriving. The body stands on the first tab; the
      // comments stand on 「대화」, which a person presses to read them.
      const still = await window.__STILL__(el("gl-item-scrim"));
      const body = window.__READ_DIAGRAM__(el("gl-item-body"), "깃랩-그림");
      el("gl-item-tab-talk")?.click();
      const talk = await window.__UNTIL__(() => el("gl-item-pane-talk")?.hidden === false && (card()?.getBoundingClientRect().width ?? 0) > 0);
      await window.__FRAMES__(2);
      const read = { still, talk, body, comment: window.__READ_DIAGRAM__(card(), "넓은깃랩댓글-그림") };
      closeGitlabItem();
      // The dialog closes animated; the next item opens once it has gone.
      await window.__UNTIL__(() => el("gl-item-scrim")?.hidden === true);
      // Refused: the body's fence stays code, one line in the window log for the one request, without the source.
      seen.logged.length = 0;
      detail = item(43, window.__AROUND__("거절 앞 문단.", "거절본문-그림", "거절 뒤 문단."), []);
      window.__FAIL__.add("render_mermaid");
      const before = asks();
      openGitlabItem({ kind: "mr", number: 43, title: detail.title, state: "opened", url: detail.url });
      const refusedStood = await window.__UNTIL__(() => el("gl-item-body")?.textContent.includes("거절 뒤 문단."));
      const logged = await window.__UNTIL__(() => lines().length > 0);
      await window.__FRAMES__(3);
      window.__FAIL__.delete("render_mermaid");
      const fence = [...(el("gl-item-body")?.querySelectorAll('pre.md-block[data-language="mermaid"]') ?? [])].find((pre) => pre.textContent.includes("거절본문-그림")) ?? null;
      const refused = {
        stood: refusedStood, logged, asks: asks() - before, lines: lines().length,
        code: Boolean(fence && fence.getBoundingClientRect().height > 0), drawn: window.__DRAWN__(el("gl-item-body")),
        sourceless: lines().every((line) => !line.includes("거절본문-그림")),
      };
      closeGitlabItem();
      delete window.__ANSWER__.gitlab_item_detail;
      return { stood, ...read, refused };
    });
    ok(
      "a GitLab merge request's body and its comment card draw their fences as diagrams where they stood, labels holding <img src=x onerror=...> as text, kept in the dialog's column and legible",
      gitlab.stood && gitlab.still && gitlab.talk && readsRight(gitlab.body, "본문 앞 문단.", "본문 뒤 문단.")
        && readsRight(gitlab.comment, "댓글 앞 문단.", "댓글 뒤 문단."),
      JSON.stringify(gitlab),
    );
    ok(
      "a remote body whose layout is refused keeps its fence as code, visible, with one line in the window log for the one request and no source in it",
      gitlab.refused.stood && gitlab.refused.logged && gitlab.refused.asks === 1 && gitlab.refused.lines === 1 && gitlab.refused.code
        && gitlab.refused.drawn === 0 && gitlab.refused.sourceless,
      JSON.stringify(gitlab.refused),
    );

    ok("the markdown-diagrams window raised no errors", faults.length === 0, faults.slice(0, 3).join(" | "));
  } finally {
    await page.close();
  }
}
