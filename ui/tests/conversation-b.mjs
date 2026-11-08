/* The conversation the person passed on 2026-10-29 (t-81525, draft B): the
 * answers read as bare prose in one column, every one in the full ink (draft
 * C2 of 10-29 took back the newest-only step, t-82195); the
 * tool work between two answers is one pill; the person's words stand on the
 * right; a code block opens with its own band; the window's own note is a thin
 * line. Read off the laid-out page, on the folded page (the product's
 * default), at seven window widths and in both treatments: nothing pushes the
 * page sideways, the reading steps with the width — the column widens with a
 * wide page (t-81786 P3-16) — every word reads at 4.5:1, and the page builds
 * no more than it shows.
 *
 * `CONVERSATION_B_SHOTS=<folder>` keeps a picture of each treatment at 390,
 * 1440 and 2000 px there; without it nothing is written. */
import { mkdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { openConversation } from "./conversation-parity.mjs";
import { mockupTurns } from "./conversation-redesign.mjs";
import { chromium, createWindowServer, foldConversations, openWindowTestPage } from "./window-boot.mjs";

export const B_WIDTHS = Object.freeze([390, 720, 1000, 1200, 1440, 2000, 2560]);
const SHOT_WIDTHS = Object.freeze([390, 1440, 2000]);

/* The mockup's turns with what draft B adds to them: a second answer before
 * the last (so one answer is not the newest), a fenced block named
 * `typescript` and one named nothing, a long unbroken path in a chip, and the
 * window's note to the agent — and what draft C reads (t-82195): the answer
 * before the last writes three levels of heading, a rule and a link, and the
 * last one's typescript opens with a comment. */
function draftTurns() {
  const turns = mockupTurns(Date.now() - 60_000);
  const last = turns.pop();
  const path = `/Users/dev/${"very-long-folder-name/".repeat(6)}file.ts`;
  turns.push(
    {
      role: "assistant",
      text: `먼저 목록을 찾았습니다. 경로는 \`${path}\` 입니다.\n\n# 목록 화면\n\n## 폭 상수\n\n### 고칠 곳\n\n두 파일입니다.\n\n---\n\n나머지는 [문서](https://example.com/docs)에 있습니다.`,
      at_ms: last.at_ms - 900,
    },
    { role: "window", text: "The window restarted and cut your last turn short. Continue exactly where you left off.", at_ms: last.at_ms - 600 },
    // The prompts nobody typed besides it (2026-11-06): the window's mail pointer
    // and the summary a CLI hands itself when it compacts.
    { role: "window_mail", text: "You have 2 orchestration messages. Run `zerocode-orc check`.", at_ms: last.at_ms - 500 },
    { role: "summary", text: "This session is being continued from a previous conversation that ran out of context.", at_ms: last.at_ms - 400 },
    { role: "user", text: "코드도 보여 줘.", at_ms: last.at_ms - 300 },
    { ...last, text: `${last.text}\n\n\`\`\`typescript\n// the screen less its two margins\nexport const width = (screen: { width: number }) => screen.width - 32;\n\`\`\`\n\n\`\`\`\nplain words in a fence with no language\n\`\`\`` },
  );
  return turns;
}

/* What one width and one treatment look like, read in the page. */
function readPage() {
  const page = document.querySelector("#worker-view");
  const list = page.querySelector(".helper-turns");
  const box = (node) => node.getBoundingClientRect();
  const column = (() => {
    const style = getComputedStyle(list);
    const at = box(list);
    return { left: at.left + parseFloat(style.paddingLeft), right: at.right - parseFloat(style.paddingRight) };
  })();
  const inside = (node) => {
    const at = box(node);
    return at.left >= column.left - 1 && at.right <= column.right + 1;
  };
  // A colour's sRGB channels (0 to 1, as the screen clips them) and its
  // alpha, from `rgb()`, `rgba()` or `color(srgb …)`.
  const parsed = (value) => {
    const srgb = /color\(srgb (-?[\d.]+) (-?[\d.]+) (-?[\d.]+)(?: \/ ([\d.]+))?/.exec(value);
    if (srgb) {
      return {
        rgb: srgb.slice(1, 4).map((one) => Math.min(1, Math.max(0, Number(one)))),
        alpha: srgb[4] === undefined ? 1 : Number(srgb[4]),
      };
    }
    const rgb = /rgba?\(([\d.]+),? ([\d.]+),? ([\d.]+)(?:,? ([\d.]+))?/.exec(value);
    return rgb ? { rgb: rgb.slice(1, 4).map((one) => Number(one) / 255), alpha: rgb[4] === undefined ? 1 : Number(rgb[4]) } : null;
  };
  // Any other notation — the light accent is `oklch()` — the page itself
  // turns into sRGB, through a probe that stands for one breath.
  const paint = (value) => {
    if (parsed(value)) return parsed(value);
    const probe = page.appendChild(document.createElement("i"));
    probe.style.color = `color-mix(in srgb, ${value} 100%, transparent)`;
    const srgb = getComputedStyle(probe).color;
    probe.remove();
    return parsed(srgb);
  };
  const channels = (value) => paint(value).rgb;
  // A see-through ground laid over the one under it, as the page paints it.
  const over = (top, under) => {
    const [a, b] = [paint(top), paint(under)];
    return `color(srgb ${a.rgb.map((one, at) => one * a.alpha + b.rgb[at] * (1 - a.alpha)).join(" ")})`;
  };
  // A colour as linear-light luminance.
  const luminance = (value) => {
    const [r, g, b] = channels(value).map((one) => (one <= 0.04045 ? one / 12.92 : ((one + 0.055) / 1.055) ** 2.4));
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
  };
  const contrast = (ink, ground) => {
    const [a, b] = [luminance(ink), luminance(ground)].sort((x, y) => y - x);
    return Math.round(((a + 0.05) / (b + 0.05)) * 100) / 100;
  };
  const ground = getComputedStyle(page).backgroundColor;
  const ink = (node) => (node ? contrast(getComputedStyle(node).color, ground) : null);
  // A code comment against its block's ground (draft C11): the block's
  // see-through well over the page.
  const comment = (() => {
    const span = list.querySelector(".helper-code pre > .chat-code-comment");
    const well = span?.closest(".helper-code");
    return span ? contrast(getComputedStyle(span).color, over(getComputedStyle(well).backgroundColor, ground)) : null;
  })();
  // A path chip's door (draft C7): the chip's words in the link's ink, on the
  // chip's own see-through ground over the page.
  const chipDoor = (() => {
    const code = list.querySelector(".helper-turn.is-assistant .helper-said .helper-link.is-chip > code");
    return code ? contrast(getComputedStyle(code).color, over(getComputedStyle(code).backgroundColor, ground)) : null;
  })();
  const answers = [...list.querySelectorAll(":scope > .helper-turn.is-assistant")];
  const latest = answers.find((row) => row.classList.contains("is-latest"));
  const earlier = answers.find((row) => !row.classList.contains("is-latest"));
  const said = latest?.querySelector(":scope > .helper-said");
  const style = said ? getComputedStyle(said) : null;
  const person = [...list.querySelectorAll(":scope > .helper-turn.is-user")].at(-1)?.querySelector(":scope > .helper-said") ?? null;
  const pills = [...list.querySelectorAll(":scope > .helper-group")];
  const codes = [...list.querySelectorAll(".helper-code")];
  const note = list.querySelector(":scope > .is-window-note .helper-window-note-words");
  // The empty box's sentence, measured in the box's own font against the
  // room inside its padding: one line, never a second one hidden under it.
  const composer = page.querySelector(".worker-composer-box");
  const say = (() => {
    if (!composer) return null;
    const style = getComputedStyle(composer);
    const pen = document.createElement("canvas").getContext("2d");
    pen.font = `${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
    const room = composer.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
    return {
      words: composer.placeholder,
      fits: pen.measureText(composer.placeholder).width <= room,
      oneLine: getComputedStyle(composer, "::placeholder").whiteSpace === "nowrap",
    };
  })();
  const dock = page.querySelector(".chat-dock");
  const root = getComputedStyle(document.documentElement);
  const space = (step) => parseFloat(root.getPropertyValue(`--space-${step}`));
  return {
    pageWidth: Math.round(box(page).width),
    // The column's measure at this page's width, and the base one (t-81786 P3-16).
    measure: window.__READING_MEASURE__(box(page).width),
    // The prose size the page's width asks for (2026-11-06): the terminal's,
    // raised on a wide page to its column over the line's measure.
    readingSize: window.__READING_SIZE__(box(page).width),
    baseMeasure: window.__READING_MEASURE__(0),
    dockMax: dock ? parseFloat(getComputedStyle(dock).maxWidth) : null,
    fadeMax: parseFloat(getComputedStyle(page, "::after").maxWidth),
    pageOverflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
    listOverflow: list.scrollWidth - list.clientWidth,
    columnWidth: Math.round(column.right - column.left),
    proseSize: style ? parseFloat(style.fontSize) : null,
    proseLeading: style ? Math.round((parseFloat(style.lineHeight) / parseFloat(style.fontSize)) * 100) / 100 : null,
    // A pill's plate hangs its own padding into the gutter (t-85975); what stays in the column is
    // its dot — the padding edge — and its right side.
    pillsInside: pills.length > 0 && pills.every((pill) => {
      const cap = pill.firstElementChild;
      const at = box(cap);
      return at.left + parseFloat(getComputedStyle(cap).paddingLeft) >= column.left - 1 && at.right <= column.right + 1;
    }),
    codesInside: codes.length >= 2 && codes.every(inside),
    // What a block's code runs past its own box — the block's inside scroll,
    // which neither the page's nor the list's width counts.
    codeOverflow: Math.max(...codes.map((code) => {
      const pre = code.querySelector(":scope > pre");
      return pre ? pre.scrollWidth - pre.clientWidth : 0;
    })),
    codeWrap: codes[0] ? getComputedStyle(codes[0].querySelector(":scope > pre")).whiteSpace : null,
    codeLanguages: codes.slice(-2).map((code) => code.querySelector(".helper-code-lang")?.textContent ?? ""),
    codeWord: t("mdview.code", "코드"),
    // The rail's handle names the window's note by its line.
    noteRail: railRowWords(list.querySelector(":scope > .is-window-note")),
    // Every prompt nobody typed, one thin line each, and the size the lines read at.
    notes: [...list.querySelectorAll(":scope > .is-window-note")].map((row) => ({
      words: row.querySelector(".helper-window-note-words")?.textContent ?? "",
      person: row.classList.contains("is-user"),
      size: parseFloat(getComputedStyle(row).fontSize),
    })),
    wantNotes: [
      `${t("worker.windowNote", "창 알림 · 창이 다시 시작되어 이어 가라고 알렸습니다")} · ${t("worker.windowNoteOpen", "원문")}`,
      `${t("worker.windowNoteMail", "창 알림 · 원장 편지 {{n}}통", { n: 2 })} · ${t("worker.windowNoteOpen", "원문")}`,
      `${t("worker.summaryNote", "이어가기 요약 · 이전 대화가 길어 요약을 넘겼습니다")} · ${t("worker.summaryNoteOpen", "펼치기")}`,
    ],
    noteWords: t("worker.windowNote", "창 알림 · 창이 다시 시작되어 이어 가라고 알렸습니다"),
    personRight: person ? Math.round(column.right - box(person).right) : null,
    personInside: person ? inside(person) : false,
    answerDots: answers.filter((row) => ["::before", "::after"].some((part) => getComputedStyle(row, part).content !== "none")).length,
    // How the newest answer, the person's last words and a pill are smoothed:
    // antialiased, as the chat apps draw them (2026-11-08, decision 3).
    smoothing: [said, person, pills[0]?.querySelector(".helper-group-words")]
      .map((node) => (node ? getComputedStyle(node).getPropertyValue("-webkit-font-smoothing") : null)),
    // The headings an answer writes (draft C3), and the steps of air they
    // should stand in: a section's above, a heading's own below.
    heads: [...list.querySelectorAll(".helper-turn.is-assistant .helper-said .md-head")].map((head) => {
      const style = getComputedStyle(head);
      return {
        level: head.localName,
        size: parseFloat(style.fontSize),
        weight: style.fontWeight,
        above: parseFloat(style.marginTop),
        below: parseFloat(style.marginBottom),
      };
    }),
    headAir: { above: space(5), below: space(2) },
    // An inline code chip's size (draft C5), beside the terminal's.
    chip: (() => {
      const code = list.querySelector(".helper-turn.is-assistant .helper-said :not(pre) > code");
      return code ? parseFloat(getComputedStyle(code).fontSize) : null;
    })(),
    termSize: parseFloat(root.getPropertyValue("--term-font-size")),
    // A rule an answer draws (draft C4), and the ink of the page's own
    // divider, read off a probe that stands for one breath.
    rule: (() => {
      const hr = list.querySelector(".helper-turn.is-assistant .helper-said hr");
      if (!hr) return null;
      const style = getComputedStyle(hr);
      const probe = page.appendChild(document.createElement("i"));
      probe.style.color = "var(--chat-divider)";
      const divider = getComputedStyle(probe).color;
      probe.remove();
      return {
        top: [style.borderTopStyle, parseFloat(style.borderTopWidth), style.borderTopColor],
        sides: [style.borderRightStyle, style.borderBottomStyle, style.borderLeftStyle],
        divider,
        width: parseFloat(root.getPropertyValue("--rule-width")),
        above: parseFloat(style.marginTop),
        below: parseFloat(style.marginBottom),
        air: space(5),
      };
    })(),
    say,
    sayWhole: t("composer.placeholder", "다음 지시를 쓰세요. 일하는 중이면 대기열에 들어갑니다"),
    sayShort: t("composer.placeholderShort", "다음 지시를 쓰세요"),
    rowsInView: list.querySelectorAll(":scope > [data-turn]").length,
    listElements: list.getElementsByTagName("*").length,
    stepRows: list.querySelectorAll(".helper-turn:is(.is-tool, .is-thinking)").length,
    sticky: [...list.querySelectorAll(":scope > .helper-turn")].filter((row) => getComputedStyle(row).position === "sticky").length,
    // The person's last words flow with the rows (t-81786 P3-17): no ground
    // of their own over the rows, no fade hanging under them.
    personFlows: (() => {
      const row = person?.parentElement;
      if (!row) return null;
      const style = getComputedStyle(row);
      return {
        position: style.position,
        ground: style.backgroundColor,
        fade: getComputedStyle(row, "::after").content,
      };
    })(),
    contrast: {
      latest: ink(said),
      earlier: ink(earlier?.querySelector(":scope > .helper-said")),
      pill: ink(pills[0]?.querySelector(".helper-group-words")),
      note: ink(note),
      code: ink(codes[0]?.querySelector(".helper-code-lang")),
      stack: ink(page.querySelector(".chat-stack")),
      link: ink(list.querySelector(".helper-turn.is-assistant .helper-said .md-link")),
      comment,
      chipDoor,
    },
  };
}

export async function testConversationB(browser, origin, ok, { shots = process.env.CONVERSATION_B_SHOTS ?? "" } = {}) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await foldConversations(page);
    // The disk says the long path a chip names is a file (draft C7).
    await page.evaluate(() => {
      window.__ANSWER__.path_kinds = ({ paths }) => paths.map((path) => (path.endsWith("/file.ts") ? "file" : "missing"));
    });
    await openConversation(page, draftTurns());
    if (shots) mkdirSync(shots, { recursive: true });
    const seen = {};
    for (const theme of ["dark", "light"]) {
      await page.evaluate((theme) => {
        document.documentElement.dataset.theme = theme;
      }, theme);
      for (const width of B_WIDTHS) {
        await page.setViewportSize({ width, height: 900 });
        await page.evaluate(async () => {
          for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
          // The fences take their colours a slice at a time; a comment's is
          // read below (draft C11).
          const until = performance.now() + 3000;
          const colouring = () => typeof codeColour !== "undefined" && (codeColour.queue.length > 0 || codeColour.job !== null || codeColour.asked);
          while (colouring() && performance.now() < until) await window.__PAINTED__();
        });
        seen[`${theme}-${width}`] = await page.evaluate(readPage);
        if (shots && SHOT_WIDTHS.includes(width)) {
          await page.screenshot({ path: join(shots, `conversation-b-${theme}-${width}.png`) });
        }
      }
    }
    const all = Object.values(seen);
    ok(`B: at ${B_WIDTHS.join(", ")} px, in both treatments, nothing pushes the page or the column sideways`,
      all.every((one) => one.pageOverflow === 0 && one.listOverflow === 0),
      JSON.stringify(Object.fromEntries(Object.entries(seen).map(([key, one]) => [key, [one.pageOverflow, one.listOverflow]]))));
    ok("B: on a page under 560 px a line of code folds inside its block — nothing to scroll sideways inside it — and a wider page keeps the line whole, to scroll",
      all.every((one) => (one.pageWidth < 560 ? one.codeOverflow === 0 && one.codeWrap === "pre-wrap" : one.codeWrap === "pre")) &&
        all.some((one) => one.pageWidth < 560),
      JSON.stringify(Object.fromEntries(Object.entries(seen).map(([key, one]) => [key, [one.pageWidth, one.codeOverflow, one.codeWrap]]))));
    ok("B: the pills, the code blocks and the person's bubble stay inside the column at every width — the bubble on its right edge (2026-11-08, decision 1)",
      all.every((one) => one.pillsInside && one.codesInside && one.personInside && one.personRight <= 1),
      JSON.stringify(Object.fromEntries(Object.entries(seen).map(([key, one]) => [key, [one.pillsInside, one.codesInside, one.personInside, one.personRight]]))));
    const dark = (width) => seen[`dark-${width}`];
    // The steps are the PAGE's width, not the window's: the window's side
    // columns take their share first.
    const step = (one) => (one.pageWidth < 560 ? "narrow" : one.pageWidth >= 1000 ? "wide" : "middle");
    // The prose is the terminal's size (2026-11-05, 「우리 화면에 맞게 글짜 크기」):
    // the same pane reads alike in its two modes, a pixel smaller under 560 px.
    // A wide page raises it with its column (2026-11-06, 「대화 화면이 실제 화면에
    // 비해 글자가 작아 보이는데」): the column over `--chat-measure`, to the pixel.
    ok("B: the reading steps with the page's own width — the terminal's size, a pixel smaller under 560 px but never under 12, raised on a wide page so its column holds the measure, the approved 1.65 line spacing (2026-11-08, decision 3), the column never wider than its measure",
      B_WIDTHS.every((width) => {
        const one = dark(width);
        const want = step(one);
        return one.columnWidth > 0 && one.columnWidth <= one.measure && one.termSize >= 12 &&
          (want === "narrow" ? one.proseSize === Math.max(12, one.termSize - 1) : one.proseSize >= one.termSize) &&
          one.proseSize === one.readingSize && one.proseLeading === 1.65;
      }) && new Set(B_WIDTHS.map((width) => step(dark(width)))).size >= 2 &&
        // The widest page reads larger than the terminal; the 1440 px window's does not.
        dark(2560).proseSize > dark(2560).termSize && dark(1440).proseSize === dark(1440).termSize,
      JSON.stringify(B_WIDTHS.map((width) => [width, dark(width).pageWidth, dark(width).proseSize, dark(width).readingSize, dark(width).termSize, dark(width).proseLeading, dark(width).columnWidth])));
    ok("B: a wide page widens its column to the step its width reached — 880, 960, 1040, then at most 1120 px — and the composer and the fade under it follow; under 1000 px the column stays at 760 px (t-81786 P3-16)",
      all.every((one) => one.measure > 0 && one.dockMax === one.measure && one.fadeMax === one.measure &&
        (one.pageWidth >= 1000 ? Math.abs(one.columnWidth - one.measure) <= 1 : one.columnWidth <= one.baseMeasure)) &&
        new Set(B_WIDTHS.map((width) => dark(width).measure)).size >= 3,
      JSON.stringify(B_WIDTHS.map((width) => [width, dark(width).pageWidth, dark(width).measure, dark(width).columnWidth, dark(width).dockMax, dark(width).fadeMax])));
    ok("B: the empty box's sentence stays on its one line at every width — the whole of it where it fits, its first half on a box too narrow for it, cut short rather than folded under the box",
      all.every((one) => one.say?.fits && one.say.oneLine) &&
        all.some((one) => one.say.words === one.sayShort) && dark(1440).say.words === dark(1440).sayWhole,
      JSON.stringify(Object.fromEntries(Object.entries(seen).map(([key, one]) => [key, one.say]))));
    ok("B: the person's last words flow with the rows — no row sticks to the list's top, none paints a ground over the rows or hangs a fade under them (t-81786 P3-17, the person's call of 10-29)",
      all.every((one) => one.sticky === 0 && one.personFlows && !["sticky", "fixed"].includes(one.personFlows.position) &&
        one.personFlows.ground === "rgba(0, 0, 0, 0)" && one.personFlows.fade === "none"),
      JSON.stringify(Object.fromEntries(Object.entries(seen).map(([key, one]) => [key, [one.sticky, one.personFlows]]))));
    ok("B: the answer, the person's words and a pill are drawn antialiased, as the chat apps draw them (2026-11-08, decision 3 — it replaces draft C1's platform smoothing; the terminal keeps its own)",
      all.every((one) => one.smoothing.length === 3 && one.smoothing.every((value) => value === "antialiased")),
      JSON.stringify(Object.fromEntries(Object.entries(seen).map(([key, one]) => [key, one.smoothing]))));
    ok("B: a heading in an answer stands at the prose's own size — the terminal's on a 1440 px window — h1 and h2 at weight 700, h3 at 600, with a section's air above and a step under it; not the browser's 32 and 24 px (draft C3, t-82195)",
      dark(1440).proseSize === dark(1440).termSize &&
        all.every((one) => ["h1", "h2", "h3"].every((level) => one.heads.some((head) => head.level === level)) &&
          one.heads.every((head) => head.size === one.proseSize &&
            head.weight === (["h1", "h2"].includes(head.level) ? "700" : "600") &&
            head.above === one.headAir.above && head.below === one.headAir.below)),
      JSON.stringify(Object.fromEntries(["dark-390", "dark-1440", "light-2000"].map((key) => [key, [seen[key].proseSize, seen[key].heads, seen[key].headAir]]))));
    ok("B: a rule in an answer is one solid line in the page's divider ink with a section's air above and under it — not the browser's two-tone inset box (draft C4, t-82195)",
      all.every((one) => one.rule !== null && one.rule.top[0] === "solid" && one.rule.top[1] === one.rule.width &&
        one.rule.top[2] === one.rule.divider && one.rule.sides.every((side) => side === "none") &&
        one.rule.above === one.rule.air && one.rule.below === one.rule.air),
      JSON.stringify(Object.fromEntries(["dark-1440", "light-1440"].map((key) => [key, seen[key].rule]))));
    ok("B: inline code stays legible — 12 px or seven eighths of the prose, whichever is larger — and never exceeds the prose size",
      all.every((one) => one.chip !== null && one.chip >= 12 && one.chip <= one.proseSize) &&
        Math.abs(dark(1440).chip - Math.max(12, dark(1440).proseSize * 0.875)) < 0.01,
      JSON.stringify(Object.fromEntries(Object.entries(seen).map(([key, one]) => [key, [one.proseSize, one.chip, one.termSize]]))));
    ok("B: an answer wears no dot and the tool work stands as pills with no step built",
      all.every((one) => one.answerDots === 0 && one.stepRows === 0),
      JSON.stringify(dark(1440)));
    ok("B: the prompts nobody typed — the window's restart nudge, its mail pointer and a CLI's compaction summary — stand as one thin line each with their own words, none as the person's bubble, read two pixels under the prose and never under 12 px (2026-11-06)",
      all.every((one) => JSON.stringify(one.notes.map((note) => note.words)) === JSON.stringify(one.wantNotes) &&
        one.notes.every((note) => !note.person && note.size === Math.max(12, one.proseSize - 2))),
      JSON.stringify(Object.fromEntries(["dark-390", "dark-1440", "dark-2560"].map((key) => [key, [seen[key].proseSize, seen[key].notes]]))));
    ok("B: the rail's handle, standing on the window's note, says the note's line — not nothing, not its door",
      dark(1440).noteRail === dark(1440).noteWords, JSON.stringify([dark(1440).noteRail, dark(1440).noteWords]));
    ok("B: a code block opens with the language its fence names — 「코드」 when it names none",
      dark(1440).codeLanguages[0] === "typescript" && dark(1440).codeLanguages[1] === dark(1440).codeWord,
      JSON.stringify(dark(1440).codeLanguages));
    for (const theme of ["dark", "light"]) {
      const read = seen[`${theme}-1440`].contrast;
      ok(`B: every word reads at 4.5:1 or more in the ${theme} treatment — the newest answer, the ones before it, a pill, the window's note, a code band, the state line, a link, a code comment and a path chip's door`,
        Object.values(read).every((ratio) => ratio === null || ratio >= 4.5) && read.latest !== null && read.earlier !== null && read.pill !== null && read.note !== null &&
          read.link !== null && read.comment !== null && read.chipDoor !== null,
        JSON.stringify(read));
      // t-81525 B3 stepped the answers before the newest back to 88% of the
      // ink; the person passed draft C, which reads every answer the same
      // (draft C2, t-82195).
      ok(`B: an answer before the newest reads in the newest's own ink in the ${theme} treatment — no answer is dimmed (draft C2, t-82195)`,
        B_WIDTHS.every((width) => {
          const at = seen[`${theme}-${width}`].contrast;
          return at.earlier !== null && at.earlier === at.latest;
        }),
        JSON.stringify(B_WIDTHS.map((width) => [width, seen[`${theme}-${width}`].contrast.latest, seen[`${theme}-${width}`].contrast.earlier])));
    }
    // On paper the answer's link and a code comment stood under AA (4.22 and
    // 4.09:1, measured for draft C); they take inks the window already has —
    // the link the step argument's lean toward the words, the comment the
    // secondary ink — and the dark treatment keeps its own (draft C11). The
    // survey's 5.76 for the link was WebKit's painted pixels; Chromium's
    // computed colour reads the same token at 5.75.
    const light = seen["light-1440"].contrast;
    ok("B: on paper an answer's link reads at 5.75:1 or more and a code comment at 7.27:1 or more on their grounds — both were under 4.5 (draft C11, t-82195)",
      light.link !== null && light.link >= 5.75 && light.comment !== null && light.comment >= 7.27,
      JSON.stringify({ light: [light.link, light.comment], dark: [seen["dark-1440"].contrast.link, seen["dark-1440"].contrast.comment] }));
    ok("B: the page raised no errors", faults.length === 0, faults.join("\n"));
    return seen;
  } finally {
    await page.close();
  }
}

/* What the pills of the page wear under their line, read in the page: each
 * pill's tail — the rows that stand, what they say, the state they wear — and
 * how many elements the tail built. */
function readPillTails() {
  const list = document.querySelector("#worker-view .helper-turns");
  const pills = [...list.querySelectorAll(":scope > .helper-group")];
  let last = list.lastElementChild;
  while (last && last.dataset.turn === undefined) last = last.previousElementSibling;
  return {
    pills: pills.length,
    lastIsPill: last?.classList.contains("helper-group") ?? false,
    tailNodes: list.querySelectorAll(".helper-group-tail").length,
    tails: pills.map((pill) => {
      const tail = pill.querySelector(":scope > .helper-group-tail");
      if (!tail) return null;
      const rows = [...tail.children].filter((row) => getComputedStyle(row).display !== "none");
      return {
        last: pill === last,
        shown: getComputedStyle(tail).display !== "none",
        hidden: tail.getAttribute("aria-hidden"),
        elements: tail.getElementsByTagName("*").length + 1,
        rows: rows.map((row) => ({
          state: ["is-live", "is-done", "is-failed"].find((name) => row.classList.contains(name)) ?? null,
          mark: row.children[0]?.textContent ?? "",
          what: row.children[1]?.textContent ?? "",
          when: row.children[2]?.textContent ?? "",
          mono: getComputedStyle(row.children[1]).fontFamily.includes("Mono"),
          size: parseFloat(getComputedStyle(row.children[1]).fontSize),
        })),
        live: pill.querySelector(".helper-group-live")?.textContent ?? "",
      };
    }),
    words: {
      live: t("worker.stepLive", "진행 중"),
      took: t("worker.elapsedShort", "{{s}}초", { s: "1.8" }),
      shell: t("worker.stepShell", "명령"),
    },
  };
}

/* Draft C6 (t-82196, the person's answer to question 2): while the agent
 * works and its pill is the last thing on the page, the pill's last three
 * steps stand under it a line each — the step in the terminal's own mono,
 * how long it took, the one still out 「진행 중」 — and when the answer
 * stands, the pill is one line again. A pill that is not the last, a page
 * whose agent stopped, and an opened pill wear no tail. */
export async function testConversationBTail(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await foldConversations(page);
    const start = Date.now() - 120_000;
    const turns = mockupTurns(start);
    const answer = turns.pop();
    const at = answer.at_ms;
    // The person asks again; the agent runs two commands and a third is out.
    turns.push(
      answer,
      { role: "user", text: "테스트도 돌려 줘", at_ms: at + 1800 },
      { role: "tool", text: "Bash · rg -n phoneChanged src", at_ms: at + 3600, tool: { call_id: "t1", name: "Bash", kind: "bash", input: "rg -n phoneChanged src", is_error: false } },
      { role: "tool_result", text: "src/a.ts:1:phoneChanged", at_ms: at + 5400, tool: { call_id: "t1", name: "", input: "", is_error: false } },
      { role: "tool", text: "Bash · npm run build", at_ms: at + 7200, tool: { call_id: "t2", name: "Bash", kind: "bash", input: "npm run build", is_error: false } },
      { role: "tool_result", text: "built", at_ms: at + 9000, tool: { call_id: "t2", name: "", input: "", is_error: false } },
      { role: "tool", text: "Bash · npm test", at_ms: at + 10_800, tool: { call_id: "t3", name: "Bash", kind: "bash", input: "npm test", is_error: false } },
    );
    await openConversation(page, turns, { status: "working" });
    const working = await page.evaluate(readPillTails);
    const tail = working.tails.at(-1);
    ok("C6: while the agent works, the pill at the foot shows its last three steps under it — the two that came back with how long each took, the one out 「진행 중」",
      working.lastIsPill && tail !== null && tail.last && tail.shown && tail.rows.length === 3 &&
        tail.rows.map((row) => row.state).join() === "is-done,is-done,is-live" &&
        tail.rows[0].what === `${working.words.shell} rg -n phoneChanged src` && tail.rows[2].what === `${working.words.shell} npm test` &&
        tail.rows[0].when === working.words.took && tail.rows[1].when === working.words.took && tail.rows[2].when === working.words.live &&
        new Set(tail.rows.map((row) => row.mark)).size === 2,
      JSON.stringify(working));
    ok("C6: a step in the tail reads in the terminal's mono at 13 px, and the tail is for the eye — the pill's own steps are what a screen reader opens",
      tail !== null && tail.rows.every((row) => row.mono && row.size === 13) && tail.hidden === "true",
      JSON.stringify(tail));
    ok("C6: the tail builds 13 elements at most — itself and three rows of four — and only the last pill wears one",
      tail !== null && tail.elements <= 13 && working.tails.slice(0, -1).every((one) => one === null),
      JSON.stringify(working.tails.map((one) => one?.elements ?? null)));
    // An opened pill shows its steps whole: the tail steps aside.
    const opened = await page.evaluate(async () => {
      const pill = [...document.querySelectorAll("#worker-view .helper-turns > .helper-group")].at(-1);
      pill.querySelector(".helper-group-cap").click();
      await window.__PAINTED__();
      const tail = pill.querySelector(":scope > .helper-group-tail");
      const shown = tail ? getComputedStyle(tail).display !== "none" : false;
      pill.querySelector(".helper-group-cap").click();
      await window.__PAINTED__();
      return shown;
    });
    ok("C6: an opened pill shows its steps in its body and its tail steps aside", opened === false, String(opened));
    // More steps: the tail keeps the newest three.
    const four = await page.evaluate(async (at) => {
      const held = window.__CONVERSATION__;
      held.turns.push(
        { role: "tool_result", text: "ok", at_ms: at + 12_600, tool: { call_id: "t3", name: "", input: "", is_error: false } },
        { role: "tool", text: "Bash · npm run lint", at_ms: at + 14_400, tool: { call_id: "t4", name: "Bash", kind: "bash", input: "npm run lint", is_error: false } },
      );
      await pollHelperPages();
      await window.__PAINTED__();
    }, at).then(() => page.evaluate(readPillTails));
    const kept = four.tails.at(-1);
    ok("C6: a fourth step pushes the oldest out — the tail keeps the newest three, the newest out",
      kept != null && kept.rows.length === 3 && kept.rows[0].what === `${four.words.shell} npm run build` &&
        kept.rows[2].what === `${four.words.shell} npm run lint` && kept.rows[2].state === "is-live",
      JSON.stringify(kept));
    // The answer stands: the pill is one line again.
    const answered = await page.evaluate(async (at) => {
      const held = window.__CONVERSATION__;
      held.turns.push(
        { role: "tool_result", text: "clean", at_ms: at + 16_200, tool: { call_id: "t4", name: "", input: "", is_error: false } },
        { role: "assistant", text: "테스트가 모두 통과했습니다.", at_ms: at + 18_000 },
      );
      await pollHelperPages();
      await window.__PAINTED__();
    }, at).then(() => page.evaluate(readPillTails));
    ok("C6: once the answer stands under the pill, the pill is one line again — no tail anywhere on the page",
      answered.pills >= 2 && answered.tails.every((one) => one === null) && answered.tailNodes === 0,
      JSON.stringify(answered));
    // The agent works again and then stops with a call out: no tail on a page whose agent is not working.
    const stopped = await page.evaluate(async (at) => {
      const held = window.__CONVERSATION__;
      held.turns.push(
        { role: "user", text: "한 번 더", at_ms: at + 19_800 },
        { role: "tool", text: "Bash · npm test", at_ms: at + 21_600, tool: { call_id: "t5", name: "Bash", kind: "bash", input: "npm test", is_error: false } },
      );
      await pollHelperPages();
      await window.__PAINTED__();
      const during = document.querySelectorAll("#worker-view .helper-group-tail").length;
      held.status = "idle";
      await pollHelperPages();
      await window.__PAINTED__();
      return { during, after: document.querySelectorAll("#worker-view .helper-group-tail").length };
    }, at);
    ok("C6: the tail stands only while the agent works — it goes when the agent stops, even with a call left out",
      stopped.during === 1 && stopped.after === 0, JSON.stringify(stopped));
    ok("C6: the page raised no errors", faults.length === 0, faults.join("\n"));
    return { working, four, answered, stopped };
  } finally {
    await page.close();
  }
}

/* Tables and code take the page's room (2026-11-06, the person's 「⑤번도 다음
 * 판에 넣어줘」; 설정 › 모양 › 「표·코드는 넓게」): with the root's mark on, an
 * answer's wide table and long code grow past the column, centred on its axis
 * and inside the list's room — the list less its gutters; a table and a code
 * block that fit stay exactly on the column, and the prose keeps it. A page
 * with no room beside its column draws as before, the answer still streaming
 * reads the same, and with the mark off nothing leaves the column. */
const WIDE_COLUMNS = ["판 폭", "대화 폭", "글자", "한 줄", "측정한 창", "넓은 칸 하나", "넓은 칸 둘", "넓은 칸 셋", "넓은 칸 넷", "넓은 칸 다섯"];
function wideBlocksAnswer() {
  const row = (at) => `| ${1000 + at * 300}px | ${880 + at * 80}px | ${14 + at}px | ${70 + at}자 | 2560×1332pt 창 | ${"표가 넓으면 칸이 접히지 않고 한 줄로 읽힙니다 ".repeat(2).trim()} | 넓은 화면에서만 판 폭까지 넓어집니다 | 글은 읽기 폭 그대로 남습니다 | 짧은 표와 코드는 대화 칸에 있습니다 | 좁은 화면은 지금과 같습니다 |`;
  return [
    "넓은 표와 긴 코드를 보여 드립니다.",
    "",
    `| ${WIDE_COLUMNS.join(" | ")} |`,
    `|${WIDE_COLUMNS.map(() => "---").join("|")}|`,
    ...[0, 1, 2].map(row),
    "",
    "짧은 표입니다.",
    "",
    "| 이름 | 값 |",
    "|---|---|",
    "| 폭 | 1120 |",
    "",
    "```rust",
    `fn wide() -> &'static str { "${"a line of code far longer than any column the page draws ".repeat(5).trim()}" }`,
    "```",
    "",
    "```",
    "npm test",
    "```",
    "",
    "끝입니다.",
    "",
  ].join("\n");
}

/* The blocks of the newest answer — settled, or the one still streaming —
 * against the column and the list's room. */
function readWideBlocks(streaming) {
  const page = document.querySelector("#worker-view");
  const list = page.querySelector(".helper-turns");
  const style = getComputedStyle(list);
  const at = list.getBoundingClientRect();
  const column = { left: at.left + parseFloat(style.paddingLeft), right: at.right - parseFloat(style.paddingRight) };
  const gutter = parseFloat(style.getPropertyValue("--helper-gutter")) || parseFloat(style.getPropertyValue("--chat-list-pad-x"));
  const room = { left: at.left + gutter, right: at.left + list.clientWidth - gutter };
  const row = streaming
    ? list.querySelector(":scope > .is-streaming.is-assistant")
    : [...list.querySelectorAll(":scope > .helper-turn.is-assistant:not(.is-streaming)")].at(-1);
  const holder = streaming ? row?.querySelector(".helper-said-settled") : row?.querySelector(":scope > .helper-said");
  const edges = (node) => {
    if (!node) return null;
    const box = node.getBoundingClientRect();
    return { left: Math.round(box.left * 2) / 2, right: Math.round(box.right * 2) / 2, width: Math.round(box.width) };
  };
  const tables = [...(holder?.querySelectorAll(":scope > .md-table") ?? [])];
  const codes = [...(holder?.querySelectorAll(":scope > .helper-code") ?? [])];
  return {
    pageWidth: Math.round(page.getBoundingClientRect().width),
    column: { left: Math.round(column.left * 2) / 2, right: Math.round(column.right * 2) / 2, width: Math.round(column.right - column.left) },
    room: { left: Math.round(room.left * 2) / 2, right: Math.round(room.right * 2) / 2, width: Math.round(room.right - room.left) },
    wideTable: edges(tables[0]),
    shortTable: edges(tables[1]),
    longCode: edges(codes[0]),
    shortCode: edges(codes[1]),
    prose: [...(holder?.querySelectorAll(":scope > .md-para") ?? [])].map(edges),
    pageOverflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
    listOverflow: list.scrollWidth - list.clientWidth,
  };
}

export async function testConversationWideBlocks(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await foldConversations(page);
    const turns = mockupTurns(Date.now() - 60_000);
    const last = turns.pop();
    turns.push({ ...last, text: wideBlocksAnswer() });
    await openConversation(page, turns, { status: "working" });
    const settle = () => page.evaluate(async () => {
      for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
    });
    const seen = {};
    for (const mark of ["on", "off"]) {
      await page.evaluate((mark) => {
        if (mark === "on") document.documentElement.dataset.chatWideBlocks = "on";
        else delete document.documentElement.dataset.chatWideBlocks;
      }, mark);
      for (const width of [2560, 2000, 1440]) {
        await page.setViewportSize({ width, height: 900 });
        await settle();
        seen[`${mark}-${width}`] = await page.evaluate(readWideBlocks, false);
      }
    }
    // The same answer, still streaming: its closed blocks stand in their holder.
    await page.evaluate(() => {
      document.documentElement.dataset.chatWideBlocks = "on";
    });
    await page.setViewportSize({ width: 2560, height: 900 });
    await page.evaluate(async (text) => {
      window.__CONVERSATION__.live = [{ role: "assistant", text: `${text}\n\n아직 쓰는 중` }];
      await pollHelperPages();
      for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
    }, wideBlocksAnswer());
    seen.streaming = await page.evaluate(readWideBlocks, true);
    // A short code block still being written stands where it will settle: the
    // fence closing does not narrow or move it (v1.1.129 W1, review t-98866 P3-2).
    const codeBox = async (lines, closed) => page.evaluate(async ({ text, lines, closed }) => {
      const fence = closed ? "\n```\n\nnext" : "";
      window.__CONVERSATION__.live = [{ role: "assistant", text: `${text}\n\n\`\`\`js\n${lines.join("\n")}${fence}` }];
      await pollHelperPages();
      for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
      const holder = closed ? ".helper-said-settled" : ".helper-said-tail";
      const codes = [...document.querySelectorAll(`.is-chat-page ${holder} > .helper-code`)];
      const box = codes.at(-1)?.getBoundingClientRect();
      return box ? { left: Math.round(box.left), right: Math.round(box.right) } : null;
    }, { text: wideBlocksAnswer(), lines, closed });
    // Kept beside `seen`, whose every entry is a page read the overflow check walks.
    const tail = {
      writing: await codeBox(["const a = 1;"], false),
      settled: await codeBox(["const a = 1;"], true),
    };
    // A finished helper's report card is a bordered summary: 「표·코드는 넓게」 does not widen
    // it, and a wide table in it scrolls inside the card instead (v1.1.129 W2).
    const card = await page.evaluate(() => {
      const said = document.createElement("div");
      said.className = "helper-said helper-report-said";
      said.style.width = "320px";
      const table = document.createElement("table");
      table.className = "md-table";
      table.innerHTML = `<tr>${"<td>wide cell text</td>".repeat(12)}</tr>`;
      said.append(table);
      document.body.append(said);
      const style = getComputedStyle(table);
      const out = { overflowX: style.overflowX, display: style.display, width: Math.round(table.getBoundingClientRect().width) };
      said.remove();
      return out;
    });
    // When the Markdown painter throws, the answer's words still stand as one
    // paragraph, so the prose rule keeps them on the column (v1.1.129 W3).
    const thrown = await page.evaluate(() => {
      const real = paintMarkdown;
      const host = document.createElement("div");
      try {
        paintMarkdown = () => {
          throw new Error("a painter that fails");
        };
        paintHelperProse(host, "words the painter could not draw", null, { colour: false });
      } finally {
        paintMarkdown = real;
      }
      const first = host.firstElementChild;
      return { tag: first?.tagName ?? null, cls: first?.className ?? null, text: host.textContent, loose: [...host.childNodes].some((one) => one.nodeType === 3) };
    });
    await page.evaluate(() => {
      delete document.documentElement.dataset.chatWideBlocks;
    });

    const near = (a, b) => Math.abs(a - b) <= 1;
    const onColumn = (box, one) => box && near(box.left, one.column.left) && near(box.right, one.column.right);
    const inColumn = (box, one) => box && box.left >= one.column.left - 1 && box.right <= one.column.right + 1;
    const centred = (box, one) => box && near((box.left + box.right) / 2, (one.column.left + one.column.right) / 2);
    const inRoom = (box, one) => box && box.left >= one.room.left - 1 && box.right <= one.room.right + 1;
    const grown = (one) => one.wideTable.width > one.column.width + 1 && inRoom(one.wideTable, one) && centred(one.wideTable, one) &&
      near(one.longCode.left, one.room.left) && near(one.longCode.right, one.room.right) &&
      onColumn(one.shortTable, one) && onColumn(one.shortCode, one) &&
      one.prose.length >= 3 && one.prose.every((box) => onColumn(box, one));
    const kept = (one) => [one.wideTable, one.shortTable, one.longCode, one.shortCode, ...one.prose].every((box) => inColumn(box, one)) &&
      onColumn(one.longCode, one);
    const wide = ["on-2560", "on-2000"].map((key) => seen[key]);
    ok("B-wide: with 「표·코드는 넓게」 on, a wide page's wide table grows past the column, centred on its axis and inside the list's room, its long code takes the room, and what fits — a short table, a short block, the prose — stays exactly on the column",
      wide.every(grown) && wide.every((one) => one.room.width > one.column.width + 2),
      JSON.stringify(Object.fromEntries(["on-2560", "on-2000"].map((key) => [key, seen[key]]))));
    ok("B-wide: a page with no room beside its column, or the setting off, keeps every block on the column",
      seen["on-1440"].room.width <= seen["on-1440"].column.width + 2 && ["on-1440", "off-2560", "off-2000", "off-1440"].every((key) => kept(seen[key])),
      JSON.stringify(Object.fromEntries(["on-1440", "off-2560", "off-2000", "off-1440"].map((key) => [key, seen[key]]))));
    ok("B-wide: the answer still streaming lays its closed blocks out the same way",
      Boolean(seen.streaming.wideTable && seen.streaming.longCode) && grown(seen.streaming), JSON.stringify(seen.streaming));
    ok("B-wide: a short code block still being written stands where it settles, so the fence closing does not narrow it (v1.1.129 W1)",
      Boolean(tail.writing && tail.settled) && near(tail.writing.left, tail.settled.left) && near(tail.writing.right, tail.settled.right),
      JSON.stringify(tail));
    ok("B-wide: a wide table in a helper's report card scrolls inside the card rather than running past its border (v1.1.129 W2)",
      card.overflowX === "auto" && card.display === "block" && card.width <= 320, JSON.stringify(card));
    ok("B-wide: an answer the Markdown painter could not draw still stands as one paragraph, not loose text across the room (v1.1.129 W3)",
      thrown.tag === "P" && (thrown.cls ?? "").includes("md-para") && thrown.text === "words the painter could not draw" && !thrown.loose,
      JSON.stringify(thrown));
    ok("B-wide: nothing pushes the page or the list sideways",
      Object.values(seen).every((one) => one.pageOverflow === 0 && one.listOverflow === 0),
      JSON.stringify(Object.fromEntries(Object.entries(seen).map(([key, one]) => [key, [one.pageOverflow, one.listOverflow]]))));
    ok("B-wide: the page raised no errors", faults.length === 0, faults.join("\n"));
    return seen;
  } finally {
    await page.close();
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failed = 0;
  try {
    const report = (name, pass, details = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass ? `\n  ${details}` : ""}`);
      if (!pass) failed++;
    };
    await testConversationWideBlocks(browser, origin, report);
    const tails = await testConversationBTail(browser, origin, report);
    if (process.argv.includes("--json")) console.log(JSON.stringify(tails));
    const seen = await testConversationB(browser, origin, (name, pass, details = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass ? `\n  ${details}` : ""}`);
      if (!pass) failed++;
    });
    if (process.argv.includes("--json")) console.log(JSON.stringify(seen));
  } finally {
    await browser.close();
    files.close();
  }
  if (failed) process.exitCode = 1;
}
