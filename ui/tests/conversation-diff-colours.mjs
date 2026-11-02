/* The conversation's diff in colour (t-83721, the person: 「컬러풀하게」), and the four small things
 * the review of t-83633 left beside it (t-84460).
 *
 *   1. A diff whose file the editor knows (read off the tool's own path) wears the code colours of an
 *      answer's fences, line by line: keywords, strings, numbers, comments — over the line's wash and
 *      under its word mark. A file it does not know stands as it did. The words a copy takes are the
 *      code alone. Every role reads 4.5:1 or more on the line's ground, with and without the word
 *      mark, in the dark theme and the light one.
 *   2. Only the lines a person can see are coloured: a long diff's clip, and the rows its well scrolls
 *      to. A diff in a row still being streamed stays plain.
 *   3. The word mark keeps its edge in the dark theme: as far from the line's wash as it stood when
 *      that wash was 14%.
 *   4. A wrapped line's continuation starts under the code, not under the sign.
 *   5. The send beside a permission card (words, no way to answer in them) says so and cannot be
 *      pressed; beside a question it still sends. After 「확인 후 입력칸 다시 사용」 the button wears the
 *      face the turn asks for.
 *
 * Synthetic conversations only. The words the page translates are asked of the page by key, never
 * spelled here: a scene before this one may have changed the language.
 *
 *   WINDOW_SUITES='^conversation-diff-colours$' node ui/tests/window.mjs
 *   WINDOW_SUITES='^conversation-send-face$' node ui/tests/window.mjs
 *   node ui/tests/conversation-diff-colours.mjs [--engine webkit] [diff|send] */
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";
import { openConversation } from "./conversation-parity.mjs";
import { webkitType } from "./conversation-perf.mjs";

/* The roles the diff wears (the fence's ramp, less the two it leaves in the code's ink). */
const DIFF_ROLES = ["keyword", "string", "number", "comment", "function", "type", "constant", "operator", "tag", "attribute", "meta", "link", "invalid"];

const TS_LINES = [
  { kind: "ctx", old: null, new: null, text: "export function total(items: number[]) {" },
  { kind: "del", old: null, new: null, text: "  const label = \"old\"; // before" },
  { kind: "add", old: null, new: null, text: "  const label = \"new\"; // after" },
  { kind: "add", old: null, new: null, text: "  return items.length + 1;" },
  { kind: "ctx", old: null, new: null, text: "}" },
];

/* What each line of `TS_LINES` must wear: the words and the role every character of them has. */
const TS_WEARS = [
  [["export", "keyword"], ["function", "keyword"]],
  [["const", "keyword"], ["\"old\"", "string"], ["// before", "comment"]],
  [["const", "keyword"], ["\"new\"", "string"], ["// after", "comment"]],
  [["return", "keyword"], ["1", "number"]],
  [],
];

const WRAPPED = `${"wrapped words keep going and going ".repeat(12)}end`;

const RUST_LINES = Array.from({ length: 120 }, (_, at) => ({
  kind: at % 3 === 0 ? "del" : at % 3 === 1 ? "add" : "ctx", old: null, new: null,
  text: `fn value_${at}() -> u32 { ${at} } // c${at}`,
}));

const step = (id, path, lines) => [
  { role: "tool", text: `Edit · ${path}`, tool: { call_id: id, name: "Edit", kind: "edit", input: "{}", is_error: false, edits: [{ path, lines }] } },
  { role: "tool_result", text: "updated", tool: { call_id: id, is_error: false } },
  { role: "assistant", text: `${id} 끝.` },
];

/* The page-side hands both checks share: contrast of a node's ink against the ground it stands on. */
const PROBE = () => {
  const channels = (value) => {
    const srgb = /color\(srgb (-?[\d.]+) (-?[\d.]+) (-?[\d.]+)(?: \/ ([\d.]+))?/.exec(value);
    if (srgb) return [...srgb.slice(1, 4).map((one) => Math.min(1, Math.max(0, Number(one))) * 255), srgb[4] === undefined ? 1 : Number(srgb[4])];
    const rgb = /rgba?\(([\d.]+),? ([\d.]+),? ([\d.]+)(?:,? ([\d.]+))?/.exec(value);
    return [...rgb.slice(1, 4).map(Number), rgb[4] === undefined ? 1 : Number(rgb[4])];
  };
  const ground = (node) => {
    const layers = [];
    for (let at = node; at; at = at.parentElement) {
      const layer = channels(getComputedStyle(at).backgroundColor);
      if (layer[3] > 0) layers.push(layer);
      if (layer[3] === 1) break;
    }
    return layers.reduceRight((under, top) => [0, 1, 2].map((at) => top[at] * top[3] + under[at] * (1 - top[3])), [255, 255, 255]);
  };
  const luminance = (colour) => colour.slice(0, 3).map((value) => {
    const unit = value / 255;
    return unit <= 0.04045 ? unit / 12.92 : ((unit + 0.055) / 1.055) ** 2.4;
  }).reduce((sum, unit, at) => sum + unit * [0.2126, 0.7152, 0.0722][at], 0);
  const ratio = (ink, under) => {
    const [high, low] = [luminance(ink), luminance(under)].sort((one, other) => other - one);
    return Math.round(((high + 0.05) / (low + 0.05)) * 100) / 100;
  };
  const frames = async (count) => {
    for (let beat = 0; beat < count; beat += 1) await window.__PAINTED__();
  };
  /* Until no fence is in line for colour, and the frames after it. */
  const settle = async () => {
    const until = performance.now() + 3000;
    await frames(3);
    while (performance.now() < until) {
      const idle = typeof codeColour === "undefined" || (codeColour.queue.length === 0 && codeColour.job === null && !codeColour.asked);
      if (idle) break;
      await frames(1);
    }
    await frames(3);
  };
  /* The role of every character of a row's code, read off the nodes it stands in. */
  const rolesOf = (text) => {
    const roles = [];
    const walk = (node, role) => {
      if (node.nodeType === Node.TEXT_NODE) {
        for (let at = 0; at < node.data.length; at += 1) roles.push(role);
        return;
      }
      const mine = /(?:^|\s)diff-code-([a-z]+)(?:\s|$)/.exec(node.className ?? "")?.[1] ?? role;
      for (const child of node.childNodes) walk(child, mine);
    };
    walk(text, "");
    return roles;
  };
  const theme = (name) => {
    const root = document.documentElement;
    root.setAttribute("data-theme", name);
    return window.__PAINTED__();
  };
  window.__DIFFS__ = { channels, ground, luminance, ratio, frames, settle, rolesOf, theme };
};

export async function testConversationDiffColours(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openConversation(page, [
      { role: "user", text: "고쳐 줘" },
      ...step("e1", "src/colour.ts", TS_LINES),
      ...step("e2", "docs/plain.zzz", [...TS_LINES, { kind: "add", old: null, new: null, text: WRAPPED }]),
      ...step("e3", "src/long.rs", RUST_LINES),
    ]);
    await page.evaluate(PROBE);
    const seen = await page.evaluate(async ({ ts, wears, roles, rust, wrapped }) => {
      const { ground, ratio, channels, settle, frames, rolesOf, theme } = window.__DIFFS__;
      const seen = {};
      const face = document.querySelector("#worker-view");
      const list = face.querySelector(".helper-turns");
      const tools = [...list.querySelectorAll(":scope > .helper-turn.is-tool")];
      const [colourStep, plainStep, longStep] = tools;
      for (const row of tools) row.open = true;
      await settle();

      // 1. The known language wears its colours, line by line, under the word mark.
      const diffRows = (row) => [...row.querySelectorAll(".helper-tool-diff-rows .diff-line")];
      const textOf = (row) => row.querySelector(".diff-text");
      seen.ts = diffRows(colourStep).map((row, at) => {
        const text = textOf(row);
        const code = text.textContent;
        const per = rolesOf(text);
        const missing = [];
        for (const [words, role] of wears[at] ?? []) {
          const from = code.indexOf(words);
          if (from < 0 || per.slice(from, from + words.length).some((one) => one !== role)) missing.push(`${role}:${words}`);
        }
        const word = text.querySelector(".diff-word");
        return {
          same: code === ts[at].text, missing, spans: text.querySelectorAll("[class^='diff-code-']").length,
          wordText: word?.textContent ?? null, wordSpans: word?.querySelectorAll("[class^='diff-code-']").length ?? 0,
        };
      });

      // A file the editor has no language for stands as it did: the word mark and the words, no role.
      seen.plain = diffRows(plainStep).map((row) => ({
        spans: textOf(row).querySelectorAll("[class^='diff-code-']").length,
        children: [...textOf(row).childNodes].every((node) => node.nodeType === Node.TEXT_NODE || node.className === "diff-word"),
      }));

      // 2. A long diff: the clip's rows are coloured and the rows behind it are not built; opened, the
      // rows the well shows are coloured and the ones far below wait until it scrolls to them.
      const coloured = (row) => row.querySelector("[class^='diff-code-']") !== null;
      const longRows = () => diffRows(longStep);
      const shownIn = (row, well) => {
        const box = row.getBoundingClientRect();
        const edge = well.getBoundingClientRect();
        return Math.min(box.bottom, edge.bottom) - Math.max(box.top, edge.top) >= 4;
      };
      const wellOf = () => longStep.querySelector(".helper-tool-diff-rows");
      const reading = () => {
        const rows = longRows();
        const shown = rows.filter((row) => shownIn(row, wellOf()));
        return { built: rows.length, shown: shown.length, shownColoured: shown.filter(coloured).length, coloured: rows.filter(coloured).length };
      };
      wellOf().scrollIntoView({ block: "center" });
      await settle();
      seen.clip = reading();
      longStep.querySelector(".helper-expand")?.click();
      await settle();
      const well = wellOf();
      well.scrollIntoView({ block: "center" });
      await settle();
      seen.opened = { ...reading(), far: coloured(longRows()[110]), scrolls: getComputedStyle(well).overflowY === "auto" };
      well.scrollTop = well.scrollHeight;
      await frames(3);
      await settle();
      seen.scrolled = { ...reading(), last: coloured(longRows()[119]) };
      well.scrollTop = 0;
      await frames(2);

      // A diff in a row still being streamed stays plain.
      const streaming = document.createElement("div");
      streaming.className = "helper-turn is-assistant is-streaming";
      list.append(streaming);
      streaming.append(toolDiffNode([{ path: "src/streamed.ts", lines: ts }]));
      await settle();
      seen.streaming = streaming.querySelectorAll("[class^='diff-code-']").length;
      streaming.remove();

      // 3. The roles read 4.5:1 or more on their ground — on the line's wash and on the word mark —
      // and the word mark keeps its edge from the line's wash in both themes.
      const scene = document.createElement("div");
      scene.className = "helper-tool-diff";
      const sceneRows = document.createElement("div");
      sceneRows.className = "helper-tool-diff-rows";
      scene.append(sceneRows);
      list.append(scene);
      const dress = (kind, role, inWord) => {
        const row = diffLineNode({ kind, text: "ab cd", old: null, new: null }, [3, 5]);
        const host = inWord ? row.querySelector(".diff-word") : row.querySelector(".diff-text");
        const span = document.createElement("span");
        span.className = `diff-code-${role}`;
        span.textContent = "ab";
        host.replaceChildren(span);
        sceneRows.append(row);
        return { row, span };
      };
      seen.ratios = {};
      seen.edge = {};
      seen.dressed = {};
      for (const name of ["dark", "light"]) {
        await theme(name);
        const worst = [];
        seen.dressed[name] = 0;
        for (const kind of ["add", "del"]) {
          for (const inWord of [false, true]) {
            for (const role of roles) {
              const { row, span } = dress(kind, role, inWord);
              worst.push([ratio(channels(getComputedStyle(span).color), ground(span)), `${kind}${inWord ? "+word" : ""}:${role}`]);
              // A role is coloured when its ink is not the code's own.
              if (getComputedStyle(span).color !== getComputedStyle(row.querySelector(".diff-text")).color) seen.dressed[name] += 1;
              row.remove();
            }
            const { row, span } = dress(kind, "keyword", inWord);
            worst.push([ratio(channels(getComputedStyle(row.querySelector(".diff-text")).color), ground(span)), `${kind}${inWord ? "+word" : ""}:ink`]);
            row.remove();
          }
          const probe = diffLineNode({ kind, text: "ab cd", old: null, new: null }, [3, 5]);
          sceneRows.append(probe);
          seen.edge[`${name}-${kind}`] = ratio(ground(probe.querySelector(".diff-word")), ground(probe));
          probe.remove();
        }
        worst.sort((one, other) => one[0] - other[0]);
        seen.ratios[name] = worst.slice(0, 3);
      }
      // What the word mark stood at when the dark wash was 14%: the line, then the mark over it, per kind.
      await theme("dark");
      const mix = (top, under, share) => top.map((one, at) => one * share + under[at] * (1 - share));
      const base = ground(sceneRows);
      seen.edgeThen = {};
      for (const kind of ["add", "del"]) {
        const accent = getComputedStyle(document.documentElement).getPropertyValue(`--diff-${kind}`).trim();
        const hex = [1, 3, 5].map((at) => parseInt(accent.slice(at, at + 2), 16));
        const lineThen = mix(hex, base, 0.14);
        seen.edgeThen[kind] = ratio(mix(hex, lineThen, 0.32), lineThen);
      }
      scene.remove();

      // 5. A comment on a changed line of the side-by-side diff (the file viewer's merge view) reads
      // 4.5:1 or more on the added line's wash, the removed one's and the active line's band.
      seen.merge = {};
      const host = document.createElement("div");
      host.style.cssText = "position:fixed;left:0;top:0;width:640px;height:200px;z-index:2147483000;background:var(--surface-abyss)";
      document.body.append(host);
      const tab = { path: "probe.ts", version: null };
      const view = new window.CM6.MergeView({
        a: { doc: "// removed comment\nconst a = 1;\n", extensions: diffExtensions(tab, false) },
        b: { doc: "// added comment\nconst a = 2;\n", extensions: [...diffExtensions(tab, false), window.CM6.highlightActiveLine()] },
        parent: host,
        orientation: "a-b",
        revertControls: false,
      });
      for (const name of ["dark", "light"]) {
        await theme(name);
        await frames(2);
        const read = {};
        for (const [side, label] of [["a", "removed"], ["b", "added"]]) {
          const line = host.querySelector(`.cm-merge-${side} .cm-line.cm-changedLine`);
          const span = line?.querySelector("span");
          read[label] = span ? ratio(channels(getComputedStyle(span).color), ground(span)) : null;
        }
        const active = host.querySelector(".cm-merge-b .cm-activeLine");
        const activeSpan = active?.querySelector("span");
        read.band = activeSpan ? ratio(channels(getComputedStyle(activeSpan).color), ground(activeSpan)) : null;
        seen.merge[name] = read;
      }
      view.destroy();
      host.remove();

      // 4. A wrapped line: its second line starts under the code, not under the sign.
      document.documentElement.setAttribute("data-diff-wrap", "on");
      await frames(2);
      const wrappedRow = diffRows(plainStep).find((row) => row.querySelector(".diff-text").textContent === wrapped);
      const text = wrappedRow.querySelector(".diff-text");
      const range = document.createRange();
      range.selectNodeContents(text);
      const lefts = [];
      for (const box of range.getClientRects()) {
        if (!lefts.some((line) => Math.abs(line.top - box.top) < 2)) lefts.push({ top: box.top, left: box.left });
        else lefts.find((line) => Math.abs(line.top - box.top) < 2).left = Math.min(lefts.find((line) => Math.abs(line.top - box.top) < 2).left, box.left);
      }
      seen.wrap = { lines: lefts.length, first: lefts[0]?.left ?? 0, next: lefts[1]?.left ?? 0, sign: getComputedStyle(text, "::before").content };
      document.documentElement.removeAttribute("data-diff-wrap");
      await theme("dark");
      return seen;
    }, { ts: TS_LINES, wears: TS_WEARS, roles: DIFF_ROLES, rust: RUST_LINES, wrapped: WRAPPED });

    const wearing = seen.ts.length === TS_LINES.length && seen.ts.every((line) => line.same && line.missing.length === 0);
    ok(
      "t-83721: the lines of a diff whose file the editor knows wear the code colours of a fence — keywords, strings, numbers, comments — each character the role it should have, the word mark kept (its words and the colours inside it), and the words a copy takes are the code alone",
      wearing && seen.ts[1].wordText !== null && seen.ts[1].wordSpans > 0 && seen.ts[2].wordSpans > 0,
      JSON.stringify(seen.ts),
    );
    ok(
      "t-83721: a diff whose file the editor has no language for stands as it did — no role span, only its words and the word mark",
      seen.plain.length === TS_LINES.length + 1 && seen.plain.every((line) => line.spans === 0 && line.children),
      JSON.stringify(seen.plain),
    );
    ok(
      "t-83721: only the lines a person can see are coloured — a long diff's clip, then the rows its opened well shows, and the rows far below wait until the well scrolls to them; a diff in a streaming row stays plain",
      seen.clip.built > 0 && seen.clip.built < 40 && seen.clip.shown > 0 && seen.clip.shownColoured === seen.clip.shown &&
        seen.clip.coloured <= seen.clip.shown + 1 &&
        seen.opened.built === RUST_LINES.length && seen.opened.scrolls && seen.opened.shown > 0 &&
        seen.opened.shownColoured === seen.opened.shown && !seen.opened.far && seen.opened.coloured < RUST_LINES.length / 2 &&
        seen.scrolled.last && seen.scrolled.shownColoured === seen.scrolled.shown && seen.streaming === 0,
      JSON.stringify({ clip: seen.clip, opened: seen.opened, scrolled: seen.scrolled, streaming: seen.streaming }),
    );
    // Every role of every kind and place wears a colour of its own (13 roles x add/del x line/word).
    const readable = (name) => (seen.ratios[name] ?? []).length > 0 && seen.ratios[name][0][0] >= 4.5 &&
      seen.dressed[name] >= DIFF_ROLES.length * 4 - 4;
    ok(
      "t-83721: every role the diff wears, and the code's own ink, reads 4.5:1 or more on the line's ground — with and without the word mark, on an added line and a removed one — in the dark theme and the light one",
      readable("dark") && readable("light"),
      JSON.stringify({ worst: seen.ratios, dressed: seen.dressed }),
    );
    ok(
      "t-84460 ④: in the dark theme the word mark stands as far from its line's wash as it did when that wash was 14% (the wash is 20% now)",
      seen.edge["dark-add"] >= seen.edgeThen.add && seen.edge["dark-del"] >= seen.edgeThen.del,
      JSON.stringify({ edge: seen.edge, then: seen.edgeThen }),
    );
    ok(
      "t-84460 ③: with the diff wrapped, a long line's second line starts where its code starts, under no sign — and the sign is still drawn",
      seen.wrap.lines >= 3 && Math.abs(seen.wrap.next - seen.wrap.first) <= 1 && seen.wrap.sign !== "none" && seen.wrap.sign !== "normal",
      JSON.stringify(seen.wrap),
    );
    const commentsRead = (name) => ["removed", "added"].every((label) => seen.merge[name]?.[label] >= 4.5);
    ok(
      "t-83721 (the run's added line): a comment on a changed line of the side-by-side diff reads 4.5:1 or more on the removed line's wash and the added one's, in the dark theme and the light one",
      commentsRead("dark") && commentsRead("light"),
      JSON.stringify(seen.merge),
    );
    ok("the page raised no errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* The send's face beside a card, and after the delivery reset (t-84460 ①②). */
export async function testConversationSendFace(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openConversation(page, [
      { role: "user", text: "첫째 부탁" },
      { role: "assistant", text: "네." },
    ], { status: "working" });
    const seen = await page.evaluate(async () => {
      const seen = {};
      const settle = async () => {
        await pollHelperPages();
        for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
      };
      const face = document.querySelector("#worker-view");
      const box = face.querySelector(".worker-composer-box");
      const send = () => face.querySelector(".worker-composer-send");
      const look = () => ({
        disabled: send().disabled, stop: send().classList.contains("is-stop"), name: send().getAttribute("aria-label"), tip: send().dataset.tip,
      });
      const type = (words) => {
        box.value = words;
        box.dispatchEvent(new Event("input", { bubbles: true }));
      };
      const names = {
        first: t("worker.askAnswerFirst", "먼저 위 질문에 답하세요"), send: t("worker.send", "보내기"),
        stop: t("worker.stop", "중지"), queue: t("composer.queue.add", "대기열에 추가 — 멈추려면 Esc"),
      };
      seen.names = names;
      const log = window.__CONVERSATION__;
      log.status = "working";
      await settle();

      // A permission card (no way to answer in words) with words in the box.
      log.asks = [{ id: "a1", kind: "approval", tool: "Bash", summary: "ls", options: [] }];
      await settle();
      type("그건 안 돼요");
      seen.permission = look();
      type("");
      seen.permissionEmpty = look();

      // A question takes words as its answer: the button sends them.
      log.asks = [{ id: "q1", kind: "question", questions: [{ question: "무엇을?", header: "고르기", options: [{ label: "하나", description: "" }] }] }];
      await settle();
      type("하나로 해요");
      seen.question = look();
      type("");

      // No card: the words queue (the face of t-83633).
      log.asks = [];
      await settle();
      type("다음 일");
      seen.none = look();
      type("");
      seen.noneEmpty = look();

      // The delivery reset: a send that could not be confirmed holds the button; the reset gives it the
      // face the running turn asks for — stop, with an empty box.
      const run = tabs.find((one) => one.id === activeTabId).worker;
      run.sendUncertain = true;
      syncWorkerComposers(run);
      seen.held = look();
      face.querySelector(".worker-delivery .worker-composer-pill").click();
      await settle();
      seen.reset = look();
      return seen;
    });
    ok(
      "t-84460 ①: beside a permission card, words in the box leave the button saying 「먼저 위 질문에 답하세요」 (by key) and unpressable — it no longer wears the send face and answers with a toast; an empty box keeps the stop; beside a question the words are its answer and the button sends",
      seen.permission.disabled && seen.permission.name === seen.names.first && seen.permission.tip === seen.names.first &&
        seen.permissionEmpty.stop && !seen.permissionEmpty.disabled &&
        !seen.question.disabled && seen.question.name === seen.names.send &&
        !seen.none.disabled && seen.none.name === seen.names.queue && seen.noneEmpty.stop,
      JSON.stringify(seen),
    );
    ok(
      "t-84460 ②: 「확인 후 입력칸 다시 사용」 draws the button again — held and unpressable while the send is unconfirmed, and then the stop the running turn asks for (an empty box), not the send face it was left with",
      seen.held.disabled && !seen.held.stop && !seen.reset.disabled && seen.reset.stop && seen.reset.name === seen.names.stop,
      JSON.stringify({ held: seen.held, reset: seen.reset, names: seen.names }),
    );
    ok("the page raised no errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const all = { testConversationDiffColours, testConversationSendFace };
  const argv = process.argv.slice(2);
  const at = argv.indexOf("--engine");
  const engine = at >= 0 ? argv.splice(at, 2)[1] : "chromium";
  const wanted = argv.map((word) => word.toLowerCase());
  const { files, origin } = await createWindowServer();
  const browser = await (engine === "webkit" ? webkitType() : chromium).launch({ headless: true });
  let failed = 0;
  try {
    for (const [name, check] of Object.entries(all)) {
      if (wanted.length && !wanted.some((word) => name.toLowerCase().includes(word))) continue;
      await check(browser, origin, (said, pass, details = "") => {
        const told = !pass || process.env.PARITY_DETAILS === "1";
        console.log(`${pass ? "PASS" : "FAIL"} ${said}${told ? `\n  ${details}` : ""}`);
        if (!pass) failed += 1;
      });
    }
  } finally {
    await browser.close();
    files.close();
  }
  if (failed) process.exitCode = 1;
}
