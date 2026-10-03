/* The conversation the person approved (2026-10-02 23:4x, t-22100): the page drawn after the side-by-side
 * with Hermes Agent's desktop app — a head that says what is running and for how long, the person's words
 * on a soft fill, a thought as one line that leads with how long it thought, steps as flat rows with a
 * state dot and a result that means something, a shell's first lines under its row, a fence with its
 * language and its copy on a flat panel, a live line that says what the agent is doing in the window's
 * words, a rail of the turns beside the list, the conversation's state over the composer, and the
 * composer as drawn — flat, from tokens, in both treatments. Each check reads the page a browser laid
 * out. The conversation is synthetic: the mockup's own (`mockupTurns`), shaped as the backend sends it. */
import { openConversation, installStepsProbe, openPaneConversation, stepsFixture } from "./conversation-parity.mjs";
import { conversationFixture, openFixtureConversation } from "./conversation-perf.mjs";
import { openWindowTestPage } from "./window-boot.mjs";

/* The eight results the mockup's search brought back — the shape Claude Code's WebSearch result has
 * (`Links: [{title, url}, …]`), each a page of the example domain. */
const MOCKUP_LINKS = Array.from({ length: 8 }, (_, at) => ({ title: `Bundle products ${at + 1}`, url: `https://example.com/bundles/${at + 1}` }));
const MOCKUP_GREP = [
  "src/screens/map/Carousel.tsx:16:const SCREEN_WIDTH = Dimensions.get('window').width",
  "src/screens/map/Carousel.tsx:17:const CARD_WIDTH = SCREEN_WIDTH - PADDING * 2",
  "src/components/Viewer.tsx:16:const { width: SCREEN_WIDTH } = Dimensions.get('window')",
].join("\n");
const MOCKUP_READS = ["Gallery", "Carousel", "Viewer", "List", "Card", "Bundle"];
const MOCKUP_ANSWER = "두 곳이 화면 폭을 고정값으로 씁니다.\n\n- `Carousel.tsx` 16·17행: 창 폭을 한 번 읽어 카드 폭을 계산합니다. 화면을 돌리면 갱신되지 않습니다.\n- `Viewer.tsx` 16행: 같은 방식입니다.\n\n```tsx\nconst SCREEN_WIDTH = Dimensions.get('window').width\nconst CARD_WIDTH = SCREEN_WIDTH - PADDING * 2\n```\n\n묶음 상품은 API가 이미 `bundle` 필드를 주므로 목록 카드에 표시만 추가하면 됩니다.";
export const MOCKUP_MODEL = "claude-sonnet-5-5";

/* The mockup's conversation (render-current.mjs's turns), with the kinds the backend reduces the calls to
 * (`hook::Tool::named`: WebFetch → `web`, WebSearch → `websearch`, Bash → `bash`, Read → `read`) and
 * the results the CLI writes: a page fetched carries its HTTP status and size beside its text
 * (`tool.facts`, read off the line's `toolUseResult`), a search its links, a grep its hits. Every line is
 * stamped 1.8 s after the one before it, from `start`. */
export function mockupTurns(start) {
  let at = start;
  const t = () => (at += 1800);
  const call = (id, name, kind, input, text) => ({
    role: "tool", text: `${name} · ${text}`, at_ms: t(),
    tool: { call_id: id, name, kind, input, is_error: false },
  });
  const result = (id, text, facts = null) => ({
    role: "tool_result", text, at_ms: t(), tool: { call_id: id, name: "", input: "", is_error: false, ...(facts && { facts }) },
  });
  return [
    { role: "user", text: "쇼핑 앱이 묶음 상품도 목록에 보여 줄 수 있는지 조사하고, 화면 폭을 고정값으로 쓰는 곳을 찾아 한국어로 보고해 줘.", at_ms: t() },
    { role: "thinking", text: "**목록 화면부터**\n먼저 목록 화면과 폭 상수를 찾고, 묶음 상품 API가 있는지 본다.", at_ms: t() },
    call("c1", "WebFetch", "web", JSON.stringify({ url: "https://example.com/products" }), "https://example.com/products"),
    result("c1", "The products page lists 24 products; a bundle shows its items under its card.", { status: 200, bytes: 262144 }),
    call("c2", "WebSearch", "websearch", JSON.stringify({ query: "bundle product list" }), "bundle product list"),
    result("c2", `Web search results for query: "bundle product list"\n\nLinks: ${JSON.stringify(MOCKUP_LINKS)}\n\nBundles are listed as one card.`),
    call("c3", "Bash", "bash", "grep -rn SCREEN_WIDTH src", "grep -rn SCREEN_WIDTH src"),
    result("c3", MOCKUP_GREP),
    ...MOCKUP_READS.flatMap((name, at) => [
      call(`r${at}`, "Read", "read", JSON.stringify({ file_path: `src/screens/${name}.tsx` }), `src/screens/${name}.tsx`),
      result(`r${at}`, `     1→import React from "react";\n     2→export const ${name} = () => null;`),
    ]),
    { role: "assistant", text: MOCKUP_ANSWER, at_ms: t() },
  ];
}

/* The mockup's page: a wire session at work on the mockup's conversation, its model named, the person's
 * request said `ago` ms before now. */
async function openMockup(page, { ago = 41_000, status = "working" } = {}) {
  const turns = mockupTurns(Date.now() - ago - 1800);
  await openConversation(page, turns, { status });
  await page.evaluate(async (model) => {
    window.__CONVERSATION__.model = model;
    await pollHelperPages();
    await window.__PAINTED__();
  }, MOCKUP_MODEL);
  await installStepsProbe(page);
}

/* A value the page resolves for a token, through a probe of the same property (its camel-case name,
 * which the style object takes and `setProperty` does not), stood where the token is read — the page
 * re-binds some per agent (`--chat-accent` on a Claude page is Claude's). */
const PROBE = `
  window.__PROBE__ = (name, property = "color", host = document.body) => {
    const probe = document.createElement("span");
    probe.style[property] = \`var(\${name})\`;
    host.appendChild(probe);
    const said = getComputedStyle(probe)[property];
    probe.remove();
    return said;
  };
`;

export async function testConversationRedesign(browser, origin, ok) {
  await redesignHead(browser, origin, ok);
  await redesignClocks(browser, origin, ok);
  await redesignRows(browser, origin, ok);
  await redesignShellOutput(browser, origin, ok);
  await redesignNow(browser, origin, ok);
  await redesignRail(browser, origin, ok);
  await redesignStack(browser, origin, ok);
  await redesignTreatments(browser, origin, ok);
  await redesignNoWorkPerToken(browser, origin, ok);
}

/* R1 — the head: the agent, its model chip, a chip that says it is running and for how long, and on the
 * right what the conversation did — the tool calls, each family of them, and how many failed. */
async function redesignHead(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openMockup(page);
    const seen = await page.evaluate(() => {
      const head = document.querySelector("#worker-view .worker-head");
      const words = (node) => (node?.textContent ?? "").replace(/\s+/g, " ").trim();
      const chip = document.querySelector("#worker-view .worker-composer-model-words");
      return {
        name: words(head?.querySelector(".worker-name")),
        model: words(head?.querySelector(".chat-model")),
        chipModel: (chip?.textContent ?? "").replace(/^\s*·\s*/, "").trim(),
        running: head?.querySelector(".chat-run")?.classList.contains("is-running") ?? false,
        state: words(head?.querySelector(".chat-run .worker-state")),
        wantState: t("worker.live", "실행 중"),
        clock: words(head?.querySelector(".chat-run .worker-elapsed")),
        total: words(head?.querySelector(".chat-tally .chat-tally-total")),
        kinds: [...(head?.querySelectorAll(".chat-tally .chat-tally-kind") ?? [])].map(words),
        failed: words(head?.querySelector(".chat-tally .chat-tally-failed")),
        want: {
          total: t("worker.tallyTotal", "도구 {{n}}", { n: 9 }),
          kinds: [
            `${t("worker.familyWeb", "웹")} 2`,
            `${t("worker.familyShell", "셸")} 1`,
            `${t("worker.familyFile", "파일")} 6`,
          ],
          failed: t("worker.tallyFailed", "실패 {{n}}", { n: 0 }),
        },
        hairline: head ? getComputedStyle(head).borderBottomWidth : null,
      };
    });
    const seconds = Number(/^(\d+)/.exec(seen.clock)?.[1] ?? NaN);
    ok(
      "R1: the head names the agent and wears its model as a chip — the same words the composer's chip says for it",
      seen.name !== "" && seen.model !== "" && seen.model === seen.chipModel,
      JSON.stringify(seen),
    );
    ok(
      "R1: while the agent works the head's chip says so and for how long — 「실행 중 · 41초」 counted from the person's request, not 「0초」 from the page's opening",
      seen.running && seen.state === seen.wantState && seconds >= 38 && seconds <= 46,
      JSON.stringify({ state: seen.state, clock: seen.clock }),
    );
    ok(
      "R1: on the right the head counts what was done — 「도구 9」, then each family (웹 2 · 셸 1 · 파일 6) in the order it first came, then 「실패 0」",
      seen.total === seen.want.total && JSON.stringify(seen.kinds) === JSON.stringify(seen.want.kinds) && seen.failed === seen.want.failed,
      JSON.stringify({ total: seen.total, kinds: seen.kinds, failed: seen.failed }),
    );
    ok("R1: the head stands on one hairline", seen.hairline === "1px", JSON.stringify({ hairline: seen.hairline }));
    ok("R1: the head raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* R1 — the clock is the turn's, and it moves. A pane's conversation was painted once when its turn began
 * and never ticked (its tab is a terminal tab, and only worker tabs ticked): 「0초」 for the whole turn. A
 * wire's page counted from the page's own opening, so its second turn said the page's age. */
async function redesignClocks(browser, origin, ok) {
  const pane = await openWindowTestPage(browser, origin);
  try {
    await pane.page.evaluate(() => {
      // The pane's turn began 41 s ago — the hook stamped it (`hookStamps`) when its state turned.
      window.__TURN_STAMP__ = Date.now() - 41_000;
    });
    await openPaneConversation(pane.page, stepsFixture().filter((turn) => turn.role !== "assistant"), { agent: "claude" });
    const seen = await pane.page.evaluate(async () => {
      const term = activeHelperPage()?.worker.term;
      hookStamps.set(term, window.__TURN_STAMP__);
      paintPaneChat(term);
      await window.__PAINTED__();
      const clock = () => document.querySelector(".pane-chat .worker-head .worker-elapsed")?.textContent ?? "";
      const first = clock();
      const deadline = performance.now() + 2600;
      while (clock() === first && performance.now() < deadline) await new Promise((done) => setTimeout(done, 100));
      return { first, later: clock() };
    });
    const secs = (said) => Number(/^(\d+)/.exec(said)?.[1] ?? NaN);
    ok(
      "R1: a pane's conversation counts its turn from the hook's stamp and the count moves while no new line arrives — the head never sits on 「0초」",
      secs(seen.first) >= 40 && secs(seen.first) <= 44 && secs(seen.later) > secs(seen.first),
      JSON.stringify(seen),
    );
    ok("R1: the pane's clock raised no page errors", pane.faults.length === 0, pane.faults.join("\n"));
  } finally {
    await pane.page.close();
  }
  const wire = await openWindowTestPage(browser, origin);
  try {
    await openConversation(wire.page, [{ role: "user", text: "고쳐 줘", at_ms: Date.now() - 600_000 }], { status: "idle" });
    const seen = await wire.page.evaluate(async () => {
      // The page has stood for two minutes; now its session takes a turn.
      activeHelperPage().worker.startedAt = Date.now() - 120_000;
      window.__CONVERSATION__.status = "working";
      await pollHelperPages();
      await window.__PAINTED__();
      return {
        clock: document.querySelector("#worker-view .worker-head .worker-elapsed")?.textContent ?? "",
        fresh: [0, 1].map((s) => t("worker.elapsedShort", "{{s}}초", { s })),
      };
    });
    ok(
      "R1: a wire's turn is counted from the moment it began — not from when the page opened",
      seen.fresh.includes(seen.clock),
      JSON.stringify(seen),
    );
    ok("R1: the wire's clock raised no page errors", wire.faults.length === 0, wire.faults.join("\n"));
  } finally {
    await wire.page.close();
  }
}

/* R2–R4, R6 — the rows: the person's words, the thought, the steps and the fence. */
async function redesignRows(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.addScriptTag({ content: PROBE });
    await openMockup(page);
    const seen = await page.evaluate(async () => {
      const { list, steps, thoughts, lineOf, settle } = window.__STEPS__;
      await settle();
      const probe = window.__PROBE__;
      const words = (node) => (node?.textContent ?? "").replace(/\s+/g, " ").trim();
      const seen = {};
      // The person's words.
      const said = list().querySelector(":scope > .helper-turn.is-user > .helper-said");
      const box = said.getBoundingClientRect();
      const listStyle = getComputedStyle(list());
      const content = list().getBoundingClientRect();
      const left = content.left + parseFloat(listStyle.paddingLeft);
      const width = content.width - parseFloat(listStyle.paddingLeft) - parseFloat(listStyle.paddingRight);
      const saidStyle = getComputedStyle(said);
      seen.person = {
        border: saidStyle.borderTopWidth,
        fill: saidStyle.backgroundColor,
        wantFill: probe("--chat-bubble", "backgroundColor"),
        left: Math.round(box.left - left),
        share: Math.round((box.width / width) * 100) / 100,
        max: parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--chat-said-max")) / 100,
      };
      // The thought.
      const thought = thoughts()[0];
      const line = lineOf(thought);
      seen.thought = {
        lead: line?.firstElementChild?.classList.contains("helper-step-res") ?? false,
        label: words(line?.querySelector(".helper-step-res")),
        wantLabel: thoughtLabel(thought.__turn),
        line: words(line?.querySelector(".helper-step-target")),
        wantLine: thoughtHeading(thought.__turn.text, true),
        mark: Boolean(line?.querySelector(".helper-step-dot, .helper-step-icon")),
      };
      line.click();
      await settle();
      seen.thought.opened = thought.open && words(thought.querySelector(".helper-thought-body")).length > 0;
      line.click();
      await settle();
      // The steps.
      const rows = steps();
      const res = (row) => words(lineOf(row)?.querySelector(".helper-step-res"));
      const kind = (row) => words(lineOf(row)?.querySelector(".helper-step-kind"));
      const dot = (row) => lineOf(row)?.querySelector(":scope > .helper-step-dot") ?? null;
      seen.steps = {
        count: rows.length,
        kinds: rows.map(kind),
        wantKinds: [
          t("worker.stepWeb", "웹 읽기"),
          t("worker.stepWebSearch", "웹 검색"),
          t("worker.stepShell", "셸"),
          t("worker.stepRun", "{{kind}} {{n}}개", { kind: t("worker.stepRead", "파일 읽기"), n: 6 }),
        ],
        results: rows.map(res),
        wantResults: [
          `200 · ${bytesLabel(262144)}`,
          t("worker.stepResults", "결과 {{n}}개", { n: 8 }),
          t("worker.stepFound", "{{n}}줄 찾음", { n: 3 }),
        ],
        statusInk: getComputedStyle(lineOf(rows[0])?.querySelector(".helper-step-status") ?? document.body).color,
        wantStatusInk: probe("--signal-ready-ink"),
        dots: rows.map((row) => Boolean(dot(row))),
        doneInk: dot(rows[0]) ? getComputedStyle(dot(rows[0])).backgroundColor : null,
        wantDoneInk: probe("--chat-dot-done", "backgroundColor"),
        marks: rows.filter((row) => lineOf(row)?.querySelector("[data-mark]")).length,
        argMono: rows.every((row) => /mono/i.test(getComputedStyle(lineOf(row).querySelector(".helper-step-target")).fontFamily)),
        boxed: rows.filter((row) => getComputedStyle(row).borderTopWidth !== "0px").length,
      };
      // Open, a step stands flat: no box drawn around it.
      lineOf(rows[0]).click();
      await settle();
      seen.steps.openBox = getComputedStyle(rows[0]).borderTopWidth;
      lineOf(rows[0]).click();
      await settle();
      // The fence.
      const frame = list().querySelector(":scope > .helper-turn.is-assistant .helper-code");
      const bar = frame?.querySelector(":scope > .helper-code-bar");
      const copy = bar?.querySelector(".helper-code-copy");
      seen.fence = {
        language: words(bar?.querySelector(".helper-code-lang")),
        copyWords: words(copy),
        wantCopy: t("mdview.copy", "복사"),
        shown: copy ? getComputedStyle(copy).opacity : null,
        fill: frame ? getComputedStyle(frame).backgroundColor : null,
        wantFill: probe("--chat-code-bg", "backgroundColor"),
        edge: frame ? getComputedStyle(frame).borderTopWidth : null,
      };
      copy?.click();
      await settle();
      seen.fence.copied = window.__CLIPBOARD_WRITES__?.at(-1) ?? null;
      seen.fence.wantCopied = frame?.querySelector("pre")?.textContent ?? null;
      seen.fence.said = words(copy);
      seen.fence.wantSaid = t("worker.copied", "복사됨");
      return seen;
    });
    ok(
      "R2: the person's words stand on a soft fill with no border, left aligned, no wider than the measure — not a bordered box as wide as the list",
      seen.person.border === "0px" && seen.person.fill === seen.person.wantFill && seen.person.left === 0 &&
        seen.person.share <= seen.person.max + 0.01,
      JSON.stringify(seen.person),
    );
    ok(
      "R3: a thought is one line that leads with how long it thought — 「생각 2초」 — then what it was about, with no mark; a press opens it in place",
      seen.thought.lead && seen.thought.label === seen.thought.wantLabel && seen.thought.label !== "" &&
        seen.thought.line === seen.thought.wantLine && !seen.thought.mark && seen.thought.opened,
      JSON.stringify(seen.thought),
    );
    ok(
      "R4: each step is a flat row with a state dot — no kind icon — its kind in the window's words, its argument in mono",
      seen.steps.count === 4 && JSON.stringify(seen.steps.kinds) === JSON.stringify(seen.steps.wantKinds) &&
        seen.steps.dots.every(Boolean) && seen.steps.marks === 0 && seen.steps.doneInk === seen.steps.wantDoneInk &&
        seen.steps.argMono && seen.steps.boxed === 0 && seen.steps.openBox === "0px",
      JSON.stringify(seen.steps),
    );
    ok(
      "R4: what came of a step means something — a page read says its HTTP status and size (「200 · 256 KB」, the status in the ready signal's text ink), a search how many results it found (「결과 8개」, not a byte count), a grep in the shell how many lines it found (「3줄 찾음」)",
      JSON.stringify(seen.steps.results.slice(0, 3)) === JSON.stringify(seen.steps.wantResults) &&
        seen.steps.statusInk === seen.steps.wantStatusInk,
      JSON.stringify({ results: seen.steps.results, want: seen.steps.wantResults, ink: seen.steps.statusInk }),
    );
    ok(
      "R6: a fence wears a flat panel — the code ground, no edge — with its language and a 「복사」 that is there without hovering, which writes the code and says it did",
      seen.fence.language === "tsx" && seen.fence.copyWords === seen.fence.wantCopy && seen.fence.shown === "1" &&
        seen.fence.fill === seen.fence.wantFill && seen.fence.edge === "0px" &&
        seen.fence.copied === seen.fence.wantCopied && seen.fence.said === seen.fence.wantSaid,
      JSON.stringify(seen.fence),
    );
    ok("R2–R6: the rows raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
  // A step that failed wears the failure's dot.
  const failed = await openWindowTestPage(browser, origin);
  try {
    await failed.page.addScriptTag({ content: PROBE });
    await openConversation(failed.page, stepsFixture());
    await installStepsProbe(failed.page);
    const seen = await failed.page.evaluate(async () => {
      const { steps, lineOf, settle } = window.__STEPS__;
      await settle();
      const row = steps().find((one) => one.classList.contains("is-failed"));
      const dot = lineOf(row)?.querySelector(":scope > .helper-step-dot");
      return {
        ink: dot ? getComputedStyle(dot).backgroundColor : null,
        want: window.__PROBE__("--chat-dot-failed", "backgroundColor"),
      };
    });
    ok("R4: a step that failed wears the failure's dot", seen.ink !== null && seen.ink === seen.want, JSON.stringify(seen));
    ok("R4: the failed step raised no page errors", failed.faults.length === 0, failed.faults.join("\n"));
  } finally {
    await failed.page.close();
  }
}

/* R5 — a shell step shows its first lines under its row while the row is closed; a `path:line` in them is
 * a door to the file at that line, by pointer and by keyboard, and pressing it does not open the row. The
 * rest is behind the row's chevron. */
async function redesignShellOutput(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openMockup(page);
    const seen = await page.evaluate(async () => {
      const { steps, lineOf, settle } = window.__STEPS__;
      await settle();
      const opened = [];
      const inner = window.openPath;
      window.openPath = (path, options) => opened.push({ path, line: options?.line ?? null });
      try {
        const row = steps().find((one) => one.__kind === "bash");
        const out = lineOf(row)?.querySelector(".helper-step-out");
        const doors = [...(out?.querySelectorAll(".helper-out-path") ?? [])];
        const seen = {
          shown: out ? out.checkVisibility({ checkVisibilityCSS: true }) : false,
          closed: row.open === false,
          lines: (out?.textContent ?? "").split("\n").filter((one) => one.trim() !== "").length,
          doors: doors.map((door) => door.textContent),
          mono: out ? /mono/i.test(getComputedStyle(out).fontFamily) : false,
        };
        // The lines stand inside their row — their hairline at the words' axis, their box no wider than
        // the line less that indent — and nothing the step shows pushes the list sideways.
        const list = row.parentElement;
        const rowBox = row.getBoundingClientRect();
        const outBox = out?.getBoundingClientRect() ?? null;
        seen.overshoot = outBox ? Math.round(outBox.right - rowBox.right) : null;
        seen.inside = outBox !== null && outBox.left >= rowBox.left && outBox.right <= rowBox.right + 0.5;
        seen.noSideScroll = list.scrollWidth <= list.clientWidth + 1;
        doors[0]?.click();
        await settle();
        seen.byPointer = opened.at(-1) ?? null;
        seen.stillClosed = row.open === false;
        doors[2]?.focus();
        seen.focusable = document.activeElement === doors[2];
        doors[2]?.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
        await settle();
        seen.byKey = opened.at(-1) ?? null;
        seen.closedAfterKey = row.open === false;
        return seen;
      } finally {
        window.openPath = inner;
      }
    });
    ok(
      "R5: a shell step's first lines stand under its row while the row is closed, in mono",
      seen.shown && seen.closed && seen.lines === 3 && seen.mono,
      JSON.stringify(seen),
    );
    ok(
      "R5: a shell step's first lines stand inside its row — they never reach past it, and never push the list sideways",
      seen.inside && seen.noSideScroll,
      JSON.stringify(seen),
    );
    ok(
      "R5: each `path:line` is a door to that file at that line — pressed or reached by Tab and Enter — and pressing it leaves the row closed",
      JSON.stringify(seen.doors) === JSON.stringify(["src/screens/map/Carousel.tsx:16", "src/screens/map/Carousel.tsx:17", "src/components/Viewer.tsx:16"]) &&
        seen.byPointer?.path.endsWith("src/screens/map/Carousel.tsx") && seen.byPointer?.line === 16 && seen.stillClosed &&
        seen.focusable && seen.byKey?.path.endsWith("src/components/Viewer.tsx") && seen.byKey?.line === 16 && seen.closedAfterKey,
      JSON.stringify(seen),
    );
    ok("R5: the shell's lines raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
  const long = await openWindowTestPage(browser, origin);
  try {
    await openConversation(long.page, stepsFixture());
    await installStepsProbe(long.page);
    const seen = await long.page.evaluate(async () => {
      const { steps, lineOf, settle } = window.__STEPS__;
      await settle();
      const row = steps().find((one) => one.__kind === "bash");
      const inline = (lineOf(row)?.querySelector(".helper-step-out")?.textContent ?? "").split("\n").filter((one) => one.trim() !== "").length;
      lineOf(row).click();
      await settle();
      const whole = (row.querySelector(".helper-tool-output")?.textContent ?? "").split("\n").filter((one) => one.trim() !== "").length;
      return { inline, whole, clip: CHAT_CLIP.lines };
    });
    ok(
      "R5: a longer output shows its first lines under the row — as many as the clip's — and the whole of it behind the chevron",
      seen.inline === seen.clip && seen.whole === 8,
      JSON.stringify(seen),
    );
    ok("R5: the long output raised no page errors", long.faults.length === 0, long.faults.join("\n"));
  } finally {
    await long.page.close();
  }
}

/* R7 — the live line says 「지금」 and what the agent is doing, from what the window knows: the call that
 * is out, the answer being written, else that it is working. Never the CLI's turning English verbs. */
async function redesignNow(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openMockup(page);
    const seen = await page.evaluate(async () => {
      const { settle } = window.__STEPS__;
      await settle();
      const status = () => document.querySelector("#worker-view .helper-status");
      const read = () => ({
        lead: (status()?.querySelector(".helper-status-lead")?.textContent ?? "").trim(),
        doing: (status()?.querySelector(".helper-status-now")?.textContent ?? "").trim(),
        all: (status()?.innerText ?? "").replace(/\s+/g, " ").trim(),
      });
      // The CLI's verbs, as its catalog row carries them.
      const verbs = installedAgents().find((row) => row.id === "claude")?.spinner_verbs ?? [];
      const seen = { want: { lead: t("worker.now", "지금"), busy: t("worker.busy", "작업 중…"), writing: t("worker.nowWriting", "답을 쓰는 중") } };
      seen.nothing = read();
      seen.spoke = verbs.filter((verb) => seen.nothing.all.includes(verb)).length;
      window.__CONVERSATION__.live = [{ role: "assistant", text: "두 곳이" }];
      await pollHelperPages();
      await settle();
      seen.writing = read();
      window.__CONVERSATION__.live = [];
      window.__CONVERSATION__.turns.push({ role: "tool", text: "Bash · cargo test -p shop", tool: { call_id: "now-1", name: "Bash", kind: "bash", input: "cargo test -p shop", is_error: false } });
      await pollHelperPages();
      await settle();
      seen.calling = read();
      seen.wantCalling = `${t("worker.stepShell", "셸")} cargo test -p shop`;
      return seen;
    });
    ok(
      "R7: with nothing known the live line says the window's 「지금 작업 중…」 — none of the CLI's turning verbs",
      seen.nothing.lead === seen.want.lead && seen.nothing.doing === seen.want.busy && seen.spoke === 0,
      JSON.stringify(seen.nothing),
    );
    ok(
      "R7: while the answer is being written it says so, and while a call is out it names the call in the row's own words",
      seen.writing.doing === seen.want.writing && seen.calling.doing === seen.wantCalling,
      JSON.stringify({ writing: seen.writing, calling: seen.calling }),
    );
    ok("R7: the live line raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* R8 — the turn rail: a tick per row beside the list, the person's longer, the rows in view on the accent,
 * a failed step marked; a press or the keyboard goes to a row; a long conversation keeps only the ticks
 * the rail has room for. */
async function redesignRail(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.addScriptTag({ content: PROBE });
    await openConversation(page, stepsFixture());
    await installStepsProbe(page);
    const seen = await page.evaluate(async () => {
      const { list, settle } = window.__STEPS__;
      await settle();
      const rail = document.querySelector("#worker-view .chat-rail");
      const rows = () => [...list().querySelectorAll(":scope > [data-turn]")].filter((row) => !row.hidden);
      const ticks = () => [...(rail?.querySelectorAll(".chat-rail-tick") ?? [])];
      const seen = {
        rail: Boolean(rail),
        beside: rail ? Math.round(rail.getBoundingClientRect().right) <= Math.round(list().getBoundingClientRect().left) + 1 : false,
        ticks: ticks().length,
        rows: rows().length,
      };
      const userAt = rows().findIndex((row) => row.classList.contains("is-user"));
      const failedAt = rows().findIndex((row) => row.classList.contains("is-failed"));
      const plainAt = rows().findIndex((row) => !row.classList.contains("is-user") && !row.classList.contains("is-failed"));
      seen.user = ticks()[userAt]?.classList.contains("is-user") ?? false;
      seen.longer = (ticks()[userAt]?.getBoundingClientRect().width ?? 0) > (ticks()[plainAt]?.getBoundingClientRect().width ?? 0);
      seen.failed = ticks()[failedAt]?.classList.contains("is-failed") ?? false;
      seen.current = ticks().filter((tick) => tick.classList.contains("is-current")).length;
      const current = ticks().find((tick) => tick.classList.contains("is-current"));
      seen.currentInk = current ? getComputedStyle(current).backgroundColor : null;
      seen.wantInk = window.__PROBE__("--chat-accent", "backgroundColor", document.querySelector("#worker-view"));
      // A press on the first tick brings the first row to the list's top.
      ticks()[0]?.click();
      await settle();
      await new Promise((done) => setTimeout(done, 50));
      const top = list().getBoundingClientRect().top;
      seen.pressed = Math.round(rows()[0].getBoundingClientRect().top - top);
      seen.pad = parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--chat-list-pad-top"));
      // The keyboard: the rail's handle takes focus, End goes to the last row, Home to the first.
      const handle = rail?.querySelector("[role=\"slider\"]");
      handle?.focus();
      seen.focused = document.activeElement === handle;
      seen.ring = handle ? (() => {
        const style = getComputedStyle(handle);
        return style.outlineStyle !== "none" || style.boxShadow !== "none";
      })() : false;
      handle?.dispatchEvent(new KeyboardEvent("keydown", { key: "End", bubbles: true, cancelable: true }));
      await settle();
      await new Promise((done) => setTimeout(done, 50));
      const last = rows().at(-1).getBoundingClientRect();
      const view = list().getBoundingClientRect();
      seen.end = last.top < view.bottom && last.bottom > view.top;
      seen.value = Number(handle?.getAttribute("aria-valuenow"));
      seen.max = Number(handle?.getAttribute("aria-valuemax"));
      handle?.dispatchEvent(new KeyboardEvent("keydown", { key: "Home", bubbles: true, cancelable: true }));
      await settle();
      await new Promise((done) => setTimeout(done, 50));
      seen.home = Math.round(rows()[0].getBoundingClientRect().top - list().getBoundingClientRect().top);
      return seen;
    });
    ok(
      "R8: a rail stands beside the list with one tick per row; the person's tick is longer, a failed step's is marked, and the rows in view wear the accent",
      seen.rail && seen.beside && seen.ticks === seen.rows && seen.user && seen.longer && seen.failed &&
        seen.current > 0 && seen.currentInk === seen.wantInk,
      JSON.stringify(seen),
    );
    ok(
      "R8: a press on a tick goes to its row; the rail's handle takes the keyboard, its ring shows, End and Home go to the last and the first row",
      Math.abs(seen.pressed - seen.pad) <= 2 && seen.focused && seen.ring && seen.end && seen.value === seen.max &&
        Math.abs(seen.home - seen.pad) <= 2,
      JSON.stringify(seen),
    );
    ok("R8: the rail raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
  const long = await openWindowTestPage(browser, origin);
  try {
    await openFixtureConversation(long.page, conversationFixture());
    const seen = await long.page.evaluate(() => {
      const list = document.querySelector("#worker-view .helper-turns");
      const rail = document.querySelector("#worker-view .chat-rail");
      const ticks = rail?.querySelectorAll(".chat-rail-tick").length ?? 0;
      const rows = list.querySelectorAll(":scope > [data-turn]").length;
      const style = rail ? getComputedStyle(rail) : null;
      const tick = rail?.querySelector(".chat-rail-tick");
      const pitch = tick ? tick.getBoundingClientRect().height + parseFloat(style.rowGap) : 0;
      const room = rail ? Math.floor((rail.clientHeight - parseFloat(style.paddingTop) - parseFloat(style.paddingBottom) + parseFloat(style.rowGap)) / pitch) : 0;
      return { ticks, rows, room };
    });
    ok(
      "R8: a 400-turn conversation keeps only the ticks the rail has room for in the page — not one per row",
      seen.ticks > 0 && seen.ticks <= seen.room && seen.ticks < seen.rows,
      JSON.stringify(seen),
    );
    ok("R8: the long rail raised no page errors", long.faults.length === 0, long.faults.join("\n"));
  } finally {
    await long.page.close();
  }
}

/* R9, R10 — the state over the composer, and the composer. A pane's conversation that has a todo list, a
 * queue and a checkout: 「할 일 2/3」 and the items, 「배경 작업 없음 · 대기 메시지 N」, the branch and its
 * changes. The stack opens to the items and keeps that per conversation; with nothing to say it is not
 * there. The composer says one sentence whether the agent works or rests. */
async function redesignStack(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const turns = [
      { role: "user", text: "목록 화면을 고쳐 줘.", at_ms: 1_790_000_000_000 },
      {
        role: "tool", text: "TodoWrite · todos", at_ms: 1_790_000_001_000,
        tool: {
          call_id: "todo-1", name: "TodoWrite", kind: "TodoWrite", is_error: false,
          input: JSON.stringify({ todos: [
            { content: "목록 화면 찾기", status: "completed", activeForm: "찾는 중" },
            { content: "폭 상수 찾기", status: "completed", activeForm: "찾는 중" },
            { content: "보고", status: "in_progress", activeForm: "보고하는 중" },
          ] }),
        },
      },
      { role: "tool_result", text: "Todos have been modified successfully.", at_ms: 1_790_000_001_200, tool: { call_id: "todo-1", is_error: false } },
    ];
    await openPaneConversation(page, turns, { agent: "claude" });
    const seen = await page.evaluate(async () => {
      const settle = async () => {
        await window.__PAINTED__();
        await window.__PAINTED__();
      };
      const stack = () => document.querySelector(".pane-chat .chat-stack");
      const words = (node) => (node?.textContent ?? "").replace(/\s+/g, " ").trim();
      const term = activeHelperPage().worker.term;
      const tab = tabOfTerm(term);
      const branch = worktreeAt(tab?.worktree)?.branch ?? null;
      const seen = {
        shown: stack() ? !stack().hidden : false,
        todo: words(stack()?.querySelector(".chat-stack-todo-count")),
        wantTodo: t("worker.stackTodos", "할 일 {{done}}/{{total}}", { done: 2, total: 3 }),
        items: words(stack()?.querySelector(".chat-stack-todo-words")),
        wantItems: ["목록 화면 찾기", "폭 상수 찾기", "보고"].join(" · "),
        work: words(stack()?.querySelector(".chat-stack-work")),
        wantWork: t("worker.stackNoWork", "배경 작업 없음"),
        queue: words(stack()?.querySelector(".chat-stack-queue")),
        wantQueue: t("worker.stackQueue", "대기 메시지 {{n}}", { n: 0 }),
        git: words(stack()?.querySelector(".chat-stack-git")),
        branch,
      };
      // Open: the items, one row each with its state.
      const toggle = stack()?.querySelector(".chat-stack-toggle");
      toggle?.focus();
      seen.toggleFocus = document.activeElement === toggle;
      toggle?.click();
      await settle();
      seen.openItems = stack()?.querySelectorAll(".chat-stack-item").length ?? 0;
      // The stack stands anew when what it says moves; its button is the one standing now, and it kept the keyboard.
      seen.expanded = stack()?.querySelector(".chat-stack-toggle")?.getAttribute("aria-expanded");
      seen.focusKept = document.activeElement === stack()?.querySelector(".chat-stack-toggle");
      // Kept per conversation: away to another tab and back.
      const here = activeTabId;
      const other = await openTermTab({ placement: "tab" });
      setActiveTab(tabOfTerm(other).id);
      await settle();
      setActiveTab(here);
      await settle();
      seen.keptOpen = stack()?.querySelector(".chat-stack-toggle")?.getAttribute("aria-expanded") === "true";
      // The queue: a message typed while the agent works waits, and the stack counts it.
      hookStates.set(term, "working");
      queueComposerMessage(activeHelperPage().worker, "다음 것도");
      syncWorkerComposers(activeHelperPage().worker);
      await settle();
      seen.queued = words(stack()?.querySelector(".chat-stack-queue"));
      seen.wantQueued = t("worker.stackQueue", "대기 메시지 {{n}}", { n: 1 });
      // The composer says one sentence whether the agent works or rests.
      const box = document.querySelector(".pane-chat .worker-composer-box");
      seen.placeholder = box?.getAttribute("placeholder");
      hookStates.set(term, "idle");
      syncWorkerComposers(activeHelperPage().worker);
      await settle();
      seen.restingPlaceholder = box?.getAttribute("placeholder");
      seen.wantPlaceholder = t("composer.placeholder", "다음 지시를 쓰세요. 일하는 중이면 대기열에 들어갑니다");
      return seen;
    });
    ok(
      "R9: over the composer the conversation's state — 「할 일 2/3」 and the items, 「배경 작업 없음」, 「대기 메시지 0」, and the checkout's branch",
      seen.shown && seen.todo === seen.wantTodo && seen.items === seen.wantItems && seen.work === seen.wantWork &&
        seen.queue === seen.wantQueue && (seen.branch === null ? seen.git === "" : seen.git.includes(seen.branch)),
      JSON.stringify(seen),
    );
    ok(
      "R9: the stack opens to the items by its own button (the keyboard reaches it), keeps that choice for this conversation across tabs, and counts a waiting message",
      seen.toggleFocus && seen.openItems === 3 && seen.expanded === "true" && seen.focusKept && seen.keptOpen && seen.queued === seen.wantQueued,
      JSON.stringify(seen),
    );
    ok(
      "R10: the composer says 「다음 지시를 쓰세요. 일하는 중이면 대기열에 들어갑니다」 whether the agent works or rests",
      seen.placeholder === seen.wantPlaceholder && seen.restingPlaceholder === seen.wantPlaceholder,
      JSON.stringify({ working: seen.placeholder, resting: seen.restingPlaceholder, want: seen.wantPlaceholder }),
    );
    ok("R9: the stack raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
  const bare = await openWindowTestPage(browser, origin);
  try {
    // A session in a folder no worktree the window lists stands in.
    await openConversation(bare.page, [{ role: "user", text: "안녕" }, { role: "assistant", text: "안녕하세요." }], { cwd: "/tmp/zerocode-window-elsewhere" });
    const seen = await bare.page.evaluate(() => {
      const stack = document.querySelector("#worker-view .chat-stack");
      return { stands: Boolean(stack), hidden: stack?.hidden ?? null };
    });
    ok(
      "R9: a conversation with no todos, no work in the background, nothing waiting and no checkout the window knows has no stack to show",
      seen.stands && seen.hidden === true,
      JSON.stringify(seen),
    );
    ok("R9: the bare stack raised no page errors", bare.faults.length === 0, bare.faults.join("\n"));
  } finally {
    await bare.page.close();
  }
}

/* R11 — flat in both treatments, and still for a person who asked for less motion. */
async function redesignTreatments(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.addScriptTag({ content: PROBE });
    await page.emulateMedia({ reducedMotion: "reduce" });
    await openMockup(page);
    const seen = await page.evaluate(async () => {
      const { list, settle } = window.__STEPS__;
      const read = () => {
        const said = list().querySelector(":scope > .helper-turn.is-user > .helper-said");
        const page = document.querySelector("#worker-view");
        return {
          fill: getComputedStyle(said).backgroundColor,
          ground: getComputedStyle(page).backgroundColor,
          wantFill: window.__PROBE__("--chat-bubble", "backgroundColor"),
        };
      };
      const seen = {};
      document.documentElement.dataset.theme = "dark";
      await settle();
      seen.dark = read();
      document.documentElement.dataset.theme = "light";
      await settle();
      seen.light = read();
      document.documentElement.dataset.theme = "dark";
      await settle();
      const ring = document.querySelector("#worker-view .helper-status-ring");
      const dot = document.querySelector("#worker-view .chat-run-dot");
      seen.still = [ring, dot].map((node) => (node ? getComputedStyle(node).animationName : "missing"));
      return seen;
    });
    ok(
      "R11: the person's fill is the bubble token and stands apart from the ground in both treatments",
      ["dark", "light"].every((name) => seen[name].fill === seen[name].wantFill && seen[name].fill !== seen[name].ground),
      JSON.stringify(seen),
    );
    ok(
      "R11: asked for less motion, the live line's ring and the head's running dot hold still",
      seen.still.every((name) => name === "none"),
      JSON.stringify(seen.still),
    );
    ok("R11: the treatments raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* The rail and the stack do no work for a streamed token: fifty deltas into the 400-turn conversation,
 * read the way the backend sends them (`wire:update` → `wire_log`), with the reader up the list (so the
 * words move no row), and neither paints once. While the list follows the words, rows leave the view as
 * the answer grows, and the rail paints for those — counted here and said, not judged: that is the
 * scroll's work, not the token's. */
async function redesignNoWorkPerToken(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openFixtureConversation(page, conversationFixture());
    const seen = await page.evaluate(async () => {
      const list = document.querySelector("#worker-view .helper-turns");
      const rail = document.querySelector("#worker-view .chat-rail");
      const stack = document.querySelector("#worker-view .chat-stack");
      const frame = () => new Promise((done) => requestAnimationFrame(() => done()));
      const feed = async (count) => {
        let text = "";
        for (let at = 0; at < count; at += 1) {
          text += `낱말${at} `;
          window.__PERF_LIVE__ = [{ role: "assistant", text }];
          for (const heard of window.__LISTENERS__["wire:update"] ?? []) heard({ payload: { id: 31 } });
          await frame();
        }
        await frame();
        await frame();
      };
      const paints = () => ({ rail: rail?.__paints ?? null, stack: stack?.__paints ?? null });
      // Up the list: the reader is away from the foot, so nothing the words do moves the rows.
      list.scrollTop = Math.floor(list.scrollHeight / 3);
      list.dispatchEvent(new WheelEvent("wheel", { deltaY: -120, bubbles: true }));
      await frame();
      await frame();
      await new Promise((done) => setTimeout(done, 120));
      const before = paints();
      await feed(50);
      const away = paints();
      // Back at the foot, following: the same fifty deltas once more.
      list.scrollTop = list.scrollHeight;
      list.dispatchEvent(new WheelEvent("wheel", { deltaY: 120, bubbles: true }));
      await frame();
      await frame();
      await new Promise((done) => setTimeout(done, 120));
      const atFoot = paints();
      await feed(50);
      const following = paints();
      return {
        before, away, stands: Boolean(rail) && Boolean(stack),
        followingRail: following.rail - atFoot.rail, followingStack: following.stack - atFoot.stack,
      };
    });
    ok(
      "R8/R9: fifty streamed deltas paint the rail and the stack zero times; following the words, the stack still never paints",
      seen.stands && seen.before.rail !== null && seen.away.rail === seen.before.rail && seen.away.stack === seen.before.stack &&
        seen.followingStack === 0,
      JSON.stringify(seen),
    );
    ok("R8/R9: the streaming count raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}
