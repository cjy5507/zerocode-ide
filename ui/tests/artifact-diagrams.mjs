/* A ```mermaid fence is drawn as a diagram in every Markdown view of the Artifacts tab and in the
 * skill detail (t-58355): the person's report showed its flowchart as code in the reader of the tab
 * by task, and 47 of 765 reports on the machine held a fence that the drawer drew as code.
 *
 * Checked on the synthetic catalog and tasks of `artifact-cards.mjs` and `artifact-tasks.mjs`, with
 * `render_mermaid` answered here (the layout itself is tested in Rust):
 *   1. the reader of the tab by task, a read-only tab and the skill detail draw the fence where it
 *      stood: a `.mmd-block` holding its svg, no mermaid code left, the drawing within the pane;
 *   2. the text paints first: no layout is asked before the frame that shows it, and a fence on
 *      screen is asked before one off it;
 *   3. while the layout is on its way, and when it is refused or comes back empty, the fence stays
 *      code; a failed layout writes one line to the window log, without the diagram's source;
 *   4. the same report opened again keeps its drawing and asks for nothing;
 *   5. a drawing that arrives after the person moved to another card never lands in that card —
 *      neither when it arrives after the new card's text, nor before it;
 *   6. in the drawer's column a drawing shrinks only until its smallest label reads at the window's
 *      caption size (`--text-3xs`); a wider one keeps that scale, its block scrolls sideways and
 *      fades the edges it is cut at, as the tab strip does, and the fade follows the column's width.
 *
 * Every wait is a condition read once a frame, bounded; an answer the test needs to hold is held
 * until the test releases it. Every read of the page is defensive (`?.`, `?? null`): on a tree that
 * does not draw these diagrams the checks fail at their assertions and the suite runs to its end.
 *
 *   WINDOW_SUITES='^artifact-diagrams$' node ui/tests/window.mjs */
import { openWindowTestPage } from "./window-boot.mjs";
import { REAL_WINDOW, standCatalog } from "./artifact-cards.mjs";
import { standTasks } from "./artifact-tasks.mjs";

/* How many frames a condition is given before its check reads what stands (4 s at 60 frames a
 * second): the answers here come at once or when released, so a green tree meets each in a few. */
const FRAMES = 240;

/* The diagrams the backend answers with, the reports that hold them, and the window helpers the
 * checks share — stood in the page once. */
function standDiagrams(page) {
  return page.evaluate((frames) => {
    const seen = (window.__DIAGRAMS__ = {
      asked: [], waiting: [], hold: false, empty: false, logged: [], previewHold: null, previewRelease: null,
    });
    const labelOf = (source) => /\[([^\]]+)\]/.exec(source)?.[1] ?? "";
    // What a hostile label carries after its words: an image with a handler, and a script after a
    // closed text element. Reports and repository files can be untrusted, so it must arrive as text.
    const hostile = `<img src=x onerror="window.__PWNED__ = 2"></text><script>window.__PWNED__ = 3</script>`;
    // Wider than any pane here unless the label asks for a size: a narrow drawing, one a little
    // wider than the drawer's column, and by default one far wider. Each carries a node label and
    // an edge label, the smallest text a drawing sets.
    const widthOf = (label) => (label.startsWith("좁은") ? 300 : label.startsWith("조금") ? 600 : 1200);
    const drawn = (label) => ({
      drawing: {
        width: widthOf(label), height: 160,
        marks: [
          { mark: "rect", x: 10, y: 40, width: 220, height: 60, radius: 4, class: "mmd-node" },
          { mark: "text", x: 120, y: 75, text: `${label}${hostile}`, class: "mmd-label", anchor: "middle" },
          { mark: "polyline", points: [[230, 70], [widthOf(label) - 10, 70]], class: "mmd-edge", arrow: true },
          { mark: "text", x: widthOf(label) / 2, y: 64, text: "예", class: "mmd-edge-label", anchor: "middle" },
        ],
      },
      undrawn: null,
    });
    window.__ANSWER__.render_mermaid = ({ source }) => {
      const label = labelOf(source);
      seen.asked.push(label);
      const answer = seen.empty ? null : drawn(label);
      if (!seen.hold) return answer;
      return new Promise((done) => seen.waiting.push({ label, release: () => done(answer) }));
    };
    window.__ANSWER__.log_window_error = ({ message }) => {
      seen.logged.push(String(message));
      return null;
    };
    // A report with one fence among the blocks a report holds: a raw HTML line just before it, a
    // paragraph after it, and a rust fence that stays code.
    const report = (title, label, said) => [
      `# ${title}`, "", said, "", "<img src=x onerror=\"window.__PWNED__ = 1\">", "",
      "```mermaid", "flowchart LR", `  A[${label}] --> B{남은 일이 있나}`, "  B -- 아니요 --> C[바로 닫기]", "```", "",
      "그림 다음 문단.", "", "```rust", "fn close() {}", "```", "",
    ].join("\n");
    window.__REPORT__ = report;
    // A document outside the open project, opened read-only (t-16006): each its own tab.
    window.__DOC_ROW__ = (id) => ({
      id, kind: "report", title: id, path: `/elsewhere/${id}.md`, bytes: 1024, created_ms: Date.now(),
      modified_ms: Date.now(), origin: {}, tags: [], preview: { kind: "none" }, source: "worker_report",
    });
    const reports = {
      "r-review": report("검증 보고 — 큐가 비워지는가", "가-그림", "보고서 가 문단."),
      "r-old": report("옛 빌드가 적은 보고서", "나-그림", "보고서 나 문단."),
    };
    for (const id of Object.keys(reports)) artifactPreviews.delete(id);
    window.__REPORTS__ = reports;
    const previewed = window.__ANSWER__.artifact_preview;
    window.__ANSWER__.artifact_preview = (args) => {
      const text = reports[args.id];
      if (text === undefined) return previewed(args);
      const answer = { kind: "markdown", text, bytes: text.length, truncated: false };
      if (seen.previewHold !== args.id) return answer;
      seen.previewHold = null;
      return new Promise((done) => { seen.previewRelease = () => done(answer); });
    };
    // A condition read once a frame, at most `frames` frames.
    window.__UNTIL__ = async (test, most = frames) => {
      for (let at = 0; at < most; at += 1) {
        if (test()) return true;
        await new Promise(requestAnimationFrame);
      }
      return Boolean(test());
    };
    window.__RELEASE__ = (label) => {
      const at = seen.waiting.findIndex((one) => one.label === label);
      if (at < 0) return false;
      seen.waiting.splice(at, 1)[0].release();
      return true;
    };
    // A layout this page has made is remembered by its source (t-58437), so a card opened again draws
    // at once and asks nothing. The checks that hold a card's answer on its way forget what the page
    // has drawn before they start, so both cards ask again; what they assert is unchanged.
    window.__FORGET_LAYOUTS__ = () => {
      if (typeof mermaidLayout !== "object") return;
      mermaidLayout.kept.clear();
      mermaidLayout.keptMarks = 0;
    };
    // What one painted body shows of its diagram: where the block stands, what it holds, and whether
    // the drawing stays in the body's column — fitted to it, or scrolling inside its own block.
    window.__READ_DIAGRAM__ = (body, label) => {
      const block = body?.querySelector(".mmd-block") ?? null;
      const svg = block?.querySelector("svg.mmd") ?? null;
      const named = [...(svg?.querySelectorAll("text.mmd-label") ?? [])];
      return {
        drawn: svg !== null,
        before: block?.previousElementSibling?.textContent.trim() ?? null,
        after: block?.nextElementSibling?.textContent.trim() ?? null,
        labelled: named.length === 1 && named[0].textContent === `${label}${hostile}`,
        mermaidCode: body?.querySelectorAll('pre.md-block[data-language="mermaid"]').length ?? null,
        heldCode: block?.querySelector('pre.md-block[data-language="mermaid"]')?.textContent.includes(label) ?? false,
        rustCode: body?.querySelector('pre.md-block[data-language="rust"]') !== null,
        contained: svg !== null && body !== null && svg.getBoundingClientRect().width > 0
          && (block.clientWidth >= svg.getBoundingClientRect().width - 1 || block.scrollWidth > block.clientWidth + 1)
          && body.scrollWidth <= body.clientWidth + 1,
        markup: body?.querySelectorAll("img, script").length ?? 0,
        rawText: [...(body?.querySelectorAll(".md-para") ?? [])].some((one) => one.textContent.includes("onerror")),
        pwned: window.__PWNED__ ?? null,
      };
    };
  }, FRAMES);
}

/* The tab by task, fresh, as a person opens it from the sidebar; and one task's line pressed. */
async function openTasksTab(page) {
  return page.evaluate(async () => {
    dropTab("artifacts");
    Object.assign(artifactFilter, { query: "", origin: null, project: null, agent: null, period: "all", showMissing: false });
    if (typeof ARTIFACT_TASKS_TAB === "string") artifactFilter.tab = ARTIFACT_TASKS_TAB;
    artifactTabPicked = false;
    artifactSelectedId = null;
    if (typeof closeArtifactTask === "function") closeArtifactTask();
    el("nav-artifacts").click();
    await window.__PAINTED__();
    return window.__UNTIL__(() => artifactsView()?.querySelector('.artifact-task[data-task="t-502"]'));
  });
}

export async function testArtifactDiagrams(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.setViewportSize(REAL_WINDOW);
    await standCatalog(page);
    await standTasks(page);
    await standDiagrams(page);
    const tabbed = await openTasksTab(page);

    /* ---- 1 + 2 + 3: the reader of the tab by task — the person's view ------------------------- */
    const reader = await page.evaluate(async () => {
      const seen = window.__DIAGRAMS__;
      const view = artifactsView();
      const md = view?.querySelector(".artifacts-drawer .artifact-preview-md") ?? null;
      const said = "보고서 가 문단.";
      const asked = () => seen.asked.filter((one) => one === "가-그림").length;
      seen.hold = true;
      // The moment the report's words stand in the reader, and the start of the frame that first
      // shows them: a layout asked for in either moment would come before the words are seen.
      let asksAtText = null;
      const watch = md ? new MutationObserver(() => {
        if (!md.textContent.includes(said)) return;
        watch.disconnect();
        asksAtText = asked();
      }) : null;
      watch?.observe(md, { childList: true, subtree: true });
      view?.querySelector('.artifact-task[data-task="t-502"]')?.click();
      await window.__UNTIL__(() => asksAtText !== null);
      const asksAtFrame = asked();
      watch?.disconnect();
      const askedLater = await window.__UNTIL__(() => asked() === 1);
      const held = window.__READ_DIAGRAM__(md, "가-그림");
      window.__RELEASE__("가-그림");
      await window.__UNTIL__(() => md?.querySelector(".mmd-block svg.mmd"));
      return { asksAtText, asksAtFrame, askedLater, held, drawn: window.__READ_DIAGRAM__(md, "가-그림") };
    });
    ok("the tab by task opens on the task line the reader is read from", tabbed === true);
    ok(
      "the reader of the tab by task draws the report's ```mermaid fence as a diagram where the fence stood: a .mmd-block holding its svg between the line before and the paragraph after, no mermaid code left, the rust fence still code, the drawing kept in the pane's column",
      reader.drawn.drawn && reader.drawn.labelled && reader.drawn.mermaidCode === 0 && reader.drawn.rustCode
        && reader.drawn.before?.startsWith("<img") === true && reader.drawn.after === "그림 다음 문단." && reader.drawn.contained,
      JSON.stringify(reader.drawn),
    );
    ok(
      "the text paints first: no layout is asked when the report's words stand or at the start of the frame that first shows them, and the fence is asked once after that frame",
      reader.asksAtText === 0 && reader.asksAtFrame === 0 && reader.askedLater,
      JSON.stringify({ atText: reader.asksAtText, atFrame: reader.asksAtFrame, later: reader.askedLater }),
    );
    ok(
      "while its layout is on its way the fence stays code, inside the block it will fill, where it stood",
      reader.held.heldCode && !reader.held.drawn && reader.held.after === "그림 다음 문단.",
      JSON.stringify(reader.held),
    );
    ok(
      "the report's raw HTML line stays text, and a diagram label holding <img src=x onerror=...> enters as text: no img or script element, no handler ran",
      reader.drawn.drawn && reader.drawn.labelled && reader.drawn.markup === 0 && reader.drawn.pwned === null && reader.drawn.rawText,
      JSON.stringify({ drawn: reader.drawn.drawn, labelled: reader.drawn.labelled, markup: reader.drawn.markup, pwned: reader.drawn.pwned, rawText: reader.drawn.rawText }),
    );

    /* ---- 4: the same report opened again keeps its drawing ------------------------------------ */
    const again = await page.evaluate(async () => {
      const seen = window.__DIAGRAMS__;
      const view = artifactsView();
      const md = view?.querySelector(".artifacts-drawer .artifact-preview-md") ?? null;
      const svg = md?.querySelector(".mmd-block svg.mmd") ?? null;
      const asks = seen.asked.length;
      closeArtifactTask();
      paintArtifactTasks(view);
      paintArtifactDrawer(view);
      await new Promise(requestAnimationFrame);
      view?.querySelector('.artifact-task[data-task="t-502"]')?.click();
      const shown = await window.__UNTIL__(() => {
        const preview = view?.querySelector(".artifacts-drawer .artifact-preview");
        return preview && !preview.hidden && md?.textContent.includes("보고서 가 문단.");
      });
      for (let at = 0; at < 3; at += 1) await new Promise(requestAnimationFrame);
      return { held: svg !== null, shown, same: svg !== null && md?.querySelector(".mmd-block svg.mmd") === svg && svg.isConnected, asked: seen.asked.length - asks };
    });
    ok(
      "the same report opened again keeps its painted body: the same svg stands and no layout is asked",
      again.held && again.shown && again.same && again.asked === 0,
      JSON.stringify(again),
    );

    /* ---- 5: a drawing for the card the person left never lands in the next one --------------- */
    const leftAfter = await page.evaluate(async () => {
      const seen = window.__DIAGRAMS__;
      const view = artifactsView();
      const md = view?.querySelector(".artifacts-drawer .artifact-preview-md") ?? null;
      const open = (task) => view?.querySelector(`.artifact-task[data-task="${task}"]`)?.click();
      const showing = (said) => {
        const preview = view?.querySelector(".artifacts-drawer .artifact-preview");
        return Boolean(preview && !preview.hidden && md?.textContent.includes(said));
      };
      const waits = (label) => seen.waiting.some((one) => one.label === label);
      const labels = () => [...(md?.querySelectorAll("svg.mmd text.mmd-label") ?? [])].map((one) => one.textContent.split("<")[0]);
      window.__FORGET_LAYOUTS__();
      seen.hold = true;
      open("t-503");
      const bAsked = await window.__UNTIL__(() => showing("보고서 나 문단.") && waits("나-그림"));
      open("t-502");
      const aAsked = await window.__UNTIL__(() => showing("보고서 가 문단.") && waits("가-그림"));
      // The card left behind is 나; its drawing comes now, after 가's words stand.
      const released = window.__RELEASE__("나-그림");
      for (let at = 0; at < 3; at += 1) await new Promise(requestAnimationFrame);
      const meanwhile = { labels: labels(), code: window.__READ_DIAGRAM__(md, "가-그림").heldCode };
      window.__RELEASE__("가-그림");
      const own = await window.__UNTIL__(() => labels().includes("가-그림"));
      return { bAsked, aAsked, released, meanwhile, own, after: labels() };
    });
    ok(
      "a drawing that arrives after the next card's words stand never lands in that card: while it waits for its own the next card holds no drawing and keeps its fence as code, then it draws its own and only its own",
      leftAfter.bAsked && leftAfter.aAsked && leftAfter.released && leftAfter.meanwhile.labels.length === 0
        && leftAfter.meanwhile.code && leftAfter.own && leftAfter.after.join("|") === "가-그림",
      JSON.stringify(leftAfter),
    );

    const leftBefore = await page.evaluate(async () => {
      const seen = window.__DIAGRAMS__;
      const view = artifactsView();
      const md = view?.querySelector(".artifacts-drawer .artifact-preview-md") ?? null;
      const open = (task) => view?.querySelector(`.artifact-task[data-task="${task}"]`)?.click();
      const showing = (said) => {
        const preview = view?.querySelector(".artifacts-drawer .artifact-preview");
        return Boolean(preview && !preview.hidden && md?.textContent.includes(said));
      };
      const waits = (label) => seen.waiting.some((one) => one.label === label);
      const labels = () => [...(md?.querySelectorAll("svg.mmd text.mmd-label") ?? [])].map((one) => one.textContent.split("<")[0]);
      window.__FORGET_LAYOUTS__();
      seen.hold = true;
      open("t-501");
      const away = await window.__UNTIL__(() => showing("한 줄 요약"));
      open("t-502");
      const aAsked = await window.__UNTIL__(() => showing("보고서 가 문단.") && waits("가-그림"));
      // 나's words are held on their way, so 가's drawing comes while the reader waits for them.
      artifactPreviews.delete("r-old");
      seen.previewHold = "r-old";
      open("t-503");
      const waiting = await window.__UNTIL__(() => seen.previewRelease !== null);
      const released = window.__RELEASE__("가-그림");
      for (let at = 0; at < 3; at += 1) await new Promise(requestAnimationFrame);
      seen.previewRelease?.();
      seen.previewRelease = null;
      const bAsked = await window.__UNTIL__(() => showing("보고서 나 문단.") && waits("나-그림"));
      const meanwhile = { labels: labels(), code: window.__READ_DIAGRAM__(md, "나-그림").heldCode };
      window.__RELEASE__("나-그림");
      const own = await window.__UNTIL__(() => labels().includes("나-그림"));
      seen.hold = false;
      return { away, aAsked, waiting, released, bAsked, meanwhile, own, after: labels() };
    });
    ok(
      "a drawing that arrives before the next card's words never lands in that card either: the next card stands with no drawing and its fence as code, then draws its own and only its own",
      leftBefore.away && leftBefore.aAsked && leftBefore.waiting && leftBefore.released && leftBefore.bAsked
        && leftBefore.meanwhile.labels.length === 0 && leftBefore.meanwhile.code && leftBefore.own
        && leftBefore.after.join("|") === "나-그림",
      JSON.stringify(leftBefore),
    );

    /* ---- 6: a drawing in the drawer stays legible at any width -------------------------------- */
    const sizes = await page.evaluate(async () => {
      const seen = window.__DIAGRAMS__;
      const view = artifactsView();
      const md = view?.querySelector(".artifacts-drawer .artifact-preview-md") ?? null;
      const open = (task) => view?.querySelector(`.artifact-task[data-task="${task}"]`)?.click();
      const showing = (said) => {
        const preview = view?.querySelector(".artifacts-drawer .artifact-preview");
        return Boolean(preview && !preview.hidden && md?.textContent.includes(said));
      };
      seen.hold = false;
      open("t-502");
      const away = await window.__UNTIL__(() => showing("보고서 가 문단."));
      const fence = (label) => ["```mermaid", "flowchart LR", `  A[${label}] -- 예 --> B[끝]`, "```", ""];
      window.__REPORTS__["r-old"] = ["# 크기 보고", "", "크기 문단.", "", ...fence("좁은-그림"), ...fence("조금-넓은-그림"),
        ...fence("넓은-그림"), "끝 문단.", ""].join("\n");
      artifactPreviews.delete("r-old");
      open("t-503");
      const stood = await window.__UNTIL__(() => showing("크기 문단.") && md.querySelectorAll(".mmd-block svg.mmd").length === 3);
      // A fade is set where the frame that lays a drawing out reports its sizes, which comes after
      // that frame's animation callbacks, where the check above ran; so the blocks are read a frame on.
      await new Promise(requestAnimationFrame);
      // The window's caption size, which no label may be drawn below.
      const caption = parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--text-3xs"));
      const blocks = [...(md?.querySelectorAll(".mmd-block") ?? [])].map((block) => {
        const svg = block.querySelector("svg.mmd");
        const natural = svg?.viewBox.baseVal.width ?? 0;
        const width = svg?.getBoundingClientRect().width ?? 0;
        const set = [...(svg?.querySelectorAll("text") ?? [])].map((text) => parseFloat(getComputedStyle(text).fontSize));
        return {
          natural, width: Math.round(width * 10) / 10, column: block.clientWidth, smallestSet: Math.min(...set),
          smallestShown: natural > 0 ? Math.round((Math.min(...set) * width / natural) * 100) / 100 : 0,
          scrolls: block.scrollWidth > block.clientWidth + 1,
          faded: /gradient/.test(getComputedStyle(block).maskImage),
        };
      });
      return { away, stood, caption, blocks, readerScrolls: md ? md.scrollWidth > md.clientWidth + 1 : null };
    });
    // Fitted to the column while the smallest label still reads at the caption size; past that,
    // drawn at that scale and scrolled.
    const sized = sizes.blocks.map((one) => {
      const expected = one.natural <= one.column ? one.natural : Math.max(one.column, one.natural * sizes.caption / one.smallestSet);
      return { natural: one.natural, width: one.width, expected: Math.round(expected * 10) / 10, scrolls: one.scrolls,
        right: Math.abs(one.width - expected) <= 1 && one.scrolls === (expected > one.column + 1) };
    });
    ok(
      "every label of a drawing in the drawer, the smallest edge label included, is drawn at the window's caption size (--text-3xs) or larger, however wide the drawing is",
      sizes.stood && sizes.caption > 0 && sizes.blocks.length === 3 && sizes.blocks.every((one) => one.smallestShown >= sizes.caption - 0.05),
      JSON.stringify(sizes),
    );
    ok(
      "a drawing narrower than the drawer's column keeps its own size and one a little wider shrinks to the column, neither scrolling; a far wider one stops shrinking at the caption size and its block scrolls sideways, while the reader itself does not",
      sizes.stood && sized.length === 3 && sized.every((one) => one.right) && !sized[0].scrolls && !sized[1].scrolls && sized[2].scrolls
        && sizes.readerScrolls === false,
      JSON.stringify({ sized, column: sizes.blocks[0]?.column, readerScrolls: sizes.readerScrolls }),
    );
    ok(
      "a drawing that fits the drawer's column keeps its edges, and one wider than its block fades the edges it is cut at, as the tab strip does: macOS hides the scrollbar until it is used",
      sizes.stood && sizes.blocks.length === 3 && !sizes.blocks[0].faded && !sizes.blocks[1].faded && sizes.blocks[2].faded,
      JSON.stringify(sizes.blocks.map((one) => ({ natural: one.natural, scrolls: one.scrolls, faded: one.faded }))),
    );

    /* ---- 6: the fade follows the column's width ---------------------------------------------- */
    const resized = await page.evaluate(async () => {
      const seen = window.__DIAGRAMS__;
      const box = document.createElement("div");
      box.style.cssText = "position: fixed; left: 0; top: 0; width: 420px; height: 400px; overflow: auto; z-index: 99999";
      document.body.appendChild(box);
      const faded = () => {
        const block = box.querySelector(".mmd-block");
        return block ? /gradient/.test(getComputedStyle(block).maskImage) : null;
      };
      const settle = async () => {
        for (let at = 0; at < 3; at += 1) await new Promise(requestAnimationFrame);
      };
      seen.hold = false;
      if (typeof paintMarkdownView === "function") {
        paintMarkdownView(box, ["```mermaid", "flowchart LR", "  A[조금-넓은-그림] -- 예 --> B[끝]", "```", ""].join("\n"));
      }
      const stood = await window.__UNTIL__(() => box.querySelector(".mmd-block svg.mmd"));
      await settle();
      const narrow = faded();
      box.style.width = "700px";
      await settle();
      const wide = faded();
      box.style.width = "420px";
      await settle();
      const narrowAgain = faded();
      box.remove();
      return { stood, narrow, wide, narrowAgain };
    });
    ok(
      "the fade follows the column: a 600 px drawing in a 420 px column fades, keeps its edges once the column widens to 700 px, and fades again when it narrows",
      resized.stood && resized.narrow === true && resized.wide === false && resized.narrowAgain === true,
      JSON.stringify(resized),
    );

    /* ---- 1: a read-only tab (a document outside the project, t-16006) ------------------------- */
    const readOnly = await page.evaluate(async () => {
      const body = (said) => [...document.querySelectorAll(".file-body--markdown")]
        .find((one) => !one.hidden && one.textContent.includes(said)) ?? null;
      openArtifactReadOnly(window.__DOC_ROW__("outside-1"), { text: window.__REPORT__("다음 작업 브리프", "다-그림", "읽기 전용 문단.") });
      const stood = await window.__UNTIL__(() => body("읽기 전용 문단."));
      await window.__UNTIL__(() => body("읽기 전용 문단.")?.querySelector(".mmd-block svg.mmd"));
      return { stood, readOnly: currentTab()?.readOnly === true, drawn: window.__READ_DIAGRAM__(body("읽기 전용 문단."), "다-그림") };
    });
    ok(
      "a read-only tab draws its ```mermaid fence as a diagram where the fence stood, with no mermaid code left and the drawing kept in the page's column",
      readOnly.stood && readOnly.readOnly && readOnly.drawn.drawn && readOnly.drawn.labelled && readOnly.drawn.mermaidCode === 0
        && readOnly.drawn.after === "그림 다음 문단." && readOnly.drawn.contained,
      JSON.stringify(readOnly),
    );

    /* ---- 3: a refused layout and an empty one leave code and say so in the window log --------- */
    const failed = await page.evaluate(async () => {
      const seen = window.__DIAGRAMS__;
      const body = (said) => [...document.querySelectorAll(".file-body--markdown")]
        .find((one) => !one.hidden && one.textContent.includes(said)) ?? null;
      const lines = () => seen.logged.filter((line) => line.startsWith("mermaid"));
      const asks = () => window.__COUNTS__.render_mermaid ?? 0;
      const visibleCode = (said, label) => {
        const fence = body(said)?.querySelector('pre.md-block[data-language="mermaid"]') ?? null;
        return Boolean(fence && fence.textContent.includes(label) && fence.getBoundingClientRect().height > 0);
      };
      seen.logged.length = 0;
      const asksBefore = asks();
      window.__FAIL__.add("render_mermaid");
      openArtifactReadOnly(window.__DOC_ROW__("outside-2"), { text: window.__REPORT__("큐 닫기 — 진행 보고", "라-그림", "거절된 문단.") });
      await window.__UNTIL__(() => body("거절된 문단."));
      const refusedLogged = await window.__UNTIL__(() => lines().length > 0);
      window.__FAIL__.delete("render_mermaid");
      const refused = {
        logged: refusedLogged, lines: lines().length, asks: asks() - asksBefore, code: visibleCode("거절된 문단.", "라-그림"),
        drawn: Boolean(body("거절된 문단.")?.querySelector("svg.mmd")),
        sourceless: lines().every((line) => !line.includes("라-그림")),
      };
      seen.empty = true;
      const asksBetween = asks();
      openArtifactReadOnly(window.__DOC_ROW__("outside-3"), { text: window.__REPORT__("옛 빌드가 적은 보고서", "마-그림", "빈 답의 문단.") });
      await window.__UNTIL__(() => body("빈 답의 문단."));
      const emptyLogged = await window.__UNTIL__(() => lines().length > refused.lines);
      seen.empty = false;
      const empty = {
        logged: emptyLogged, lines: lines().length - refused.lines, asks: asks() - asksBetween, code: visibleCode("빈 답의 문단.", "마-그림"),
        drawn: Boolean(body("빈 답의 문단.")?.querySelector("svg.mmd")),
        sourceless: lines().every((line) => !line.includes("마-그림")),
      };
      return { refused, empty, said: lines() };
    });
    ok(
      "a refused layout leaves the fence as code, never an empty slot, and writes one line to the window log for each refusal, without the diagram's source",
      failed.refused.logged && failed.refused.asks >= 1 && failed.refused.lines === failed.refused.asks && failed.refused.code
        && !failed.refused.drawn && failed.refused.sourceless,
      JSON.stringify(failed),
    );
    ok(
      "a layout that comes back empty leaves the fence as code too, with one line of its own in the window log for each empty answer",
      failed.empty.logged && failed.empty.asks >= 1 && failed.empty.lines === failed.empty.asks && failed.empty.code
        && !failed.empty.drawn && failed.empty.sourceless,
      JSON.stringify(failed),
    );

    /* ---- 1: the skill detail ------------------------------------------------------------------ */
    const skill = await page.evaluate(async () => {
      const variant = {
        id: "copy-0", name: "Diagram steps", description: "A skill whose SKILL.md draws its steps.",
        file: "/home/skills/diagram-steps/SKILL.md", directory: "/home/skills/diagram-steps", source_id: "repo-agents",
        source_label: "Project", source_kind: "repo", providers: ["claude"], version: "1.0.0", digest: "d".repeat(64),
        roots: [], files: 1, updated_at: 1,
      };
      const report = {
        families: [{ id: "family-0", name: "Diagram steps", variants: [variant], conflict: false, latest_version: "1.0.0",
          usage: { count_7d: 0, last_used_ms: null, agents: [] } }],
        sources: [{ id: "repo-agents", path: "/project/.agents/skills", kind: "repo", found: 1, exists: true }],
        agents: [["claude", "Claude"]], external_agents: [["claude", "Claude"]], external_unavailable: [], required: [],
        policy: { terminal_rows: 24, terminal_cols: 96 }, reads: 1, capped: false, evidence_scope: "hooks-only",
        evidence_count: 0, plans: [],
      };
      const markdown = ["# Diagram steps", "", "스킬 문단.", "", "```mermaid", "flowchart LR", "  A[바-그림] --> B[끝]", "```", "",
        "그림 다음 문단.", ""].join("\n");
      window.__ANSWER__.skills_list = () => report;
      window.__ANSWER__.skills_rescan = () => report;
      window.__ANSWER__.skill_detail = () => ({ skill: variant, markdown, evidence: [] });
      skillsReport = null;
      if (skillsPending) await skillsPending;
      document.getElementById("nav-skills")?.click();
      await refreshSkills();
      const tab = tabs.find((one) => one.kind === "skills");
      const view = tab ? docHost(tab.pane, "skills") : null;
      if (view) await selectSkillFamily(view, "family-0");
      const body = () => view?.querySelector(".skills-markdown") ?? null;
      const stood = await window.__UNTIL__(() => body()?.textContent.includes("스킬 문단."));
      await window.__UNTIL__(() => body()?.querySelector(".mmd-block svg.mmd"));
      return { stood, drawn: window.__READ_DIAGRAM__(body(), "바-그림") };
    });
    ok(
      "the skill detail draws its SKILL.md's ```mermaid fence as a diagram where the fence stood, with no mermaid code left and the drawing kept in the pane's column",
      skill.stood && skill.drawn.drawn && skill.drawn.labelled && skill.drawn.mermaidCode === 0
        && skill.drawn.before === "스킬 문단." && skill.drawn.after === "그림 다음 문단." && skill.drawn.contained,
      JSON.stringify(skill),
    );

    /* ---- 2: a fence on screen is asked before one off it -------------------------------------- */
    const order = await page.evaluate(async () => {
      const seen = window.__DIAGRAMS__;
      const box = document.createElement("div");
      box.style.cssText = "position: fixed; left: 0; top: 0; width: 420px; height: 240px; overflow: auto; z-index: 99999";
      document.body.appendChild(box);
      const filler = Array.from({ length: 80 }, (_, at) => `채움 문단 ${at}.`).join("\n\n");
      const text = ["```mermaid", "flowchart LR", "  A[위-그림] --> B[끝]", "```", "", filler, "",
        "```mermaid", "flowchart LR", "  A[아래-그림] --> B[끝]", "```", ""].join("\n");
      const asks = seen.asked.length;
      const painter = typeof paintMarkdownView === "function";
      if (painter) paintMarkdownView(box, text);
      // The lower fence is on screen, the upper one scrolled away above it.
      box.scrollTop = box.scrollHeight;
      await window.__UNTIL__(() => seen.asked.length - asks >= 2);
      const asked = seen.asked.slice(asks);
      box.remove();
      return { painter, asked };
    });
    ok(
      "a fence on screen is asked before a fence off it, whatever their order in the page",
      order.painter && order.asked.join("|") === "아래-그림|위-그림",
      JSON.stringify(order),
    );

    ok("the artifact-diagrams window raised no errors", faults.length === 0, faults.slice(0, 3).join(" | "));
  } finally {
    await page.close();
  }
}
