/* The form bench's stand-in for the window's browser door (t-41387): one
 * Chromium page holding a scene, and a loopback server that answers what an
 * agent's `zerocode-browser` and `zerocode-computer handoff` ask — with the
 * page scripts the pane runs (ui/tests/browser-scripts.mjs, built from the
 * Rust they ship in) and the words the window answers with (the core's own
 * formatters, through the `door_text` example), so a model on this desk is
 * told what a model in the window is told. A verb it does not answer is
 * refused in words and counted, never guessed at.
 *
 * What it keeps from the person: the page reaches only the bench's own site
 * (every other request is aborted and counted), nothing but `handoff` of the
 * computer door is answered — the desktop is never touched — and where the
 * window hands the agent a turn to a person the desk plays one: a code read
 * off the phone and typed into the field that asks for it, after the time a
 * person takes, which the tally keeps apart from the agent's.
 */

import { createServer } from "node:http";
import { spawnSync } from "node:child_process";
import { setTimeout as sleep } from "node:timers/promises";
import { chromium } from "../../../ui/tests/playwright-chromium.mjs";
import {
  CLICK_SAID, EXPRESSION_CAP, FILL_POLL_MS, TYPED_SAID, WAIT_DEFAULT_MS, WAIT_MAX_MS, WAIT_MIN_MS, WAIT_TIMED_OUT,
  clickScript, evalScript, fieldsScript, fillPasses, readScript, scrollScript, typeScript, waitScript,
} from "../../../ui/tests/browser-scripts.mjs";
import { serveScene } from "./scene-kit.mjs";

/* The one pane this window holds, and the source the window fences the list
 * of every pane under (`EVERY_TAB`). */
export const PANE = "browser-1";
const EVERY_TAB = "browser tabs";
/* What a turn of the person's costs the run, in the time a person takes to
 * read a code off a phone and type it. */
export const PERSON_TURN_MS = 5_000;
/* The words a field asks for a one-time code in: what the person looks for;
 * and the codes a page also asks for that are not the one sent to a phone. */
const CODE_ASKED = /(인증|코드|code|otp|passcode|verif|일회)/i;
const CODE_NOT_ASKED = /(post|zip|promo|coupon|country|area|우편|쿠폰|프로모)/i;
/* What the window says when it holds no pane. */
const NO_PANE = "(열린 브라우저 판이 없습니다 — `zerocode-browser open <url>`)\n";
/* The bench's browser: a desktop's size. */
const VIEWPORT = { width: 1280, height: 800 };
const SEPARATOR = "\x1f";

/* `page_failure` (cmd/browser.rs): what the window says of a page script that
 * answered `ok: false`, by the code the script gave. */
const PAGE_FAILURES = {
  persons_entry: "비밀번호 칸입니다 — 사람이 입력합니다; 사용자가 이 칸의 비밀을 주었을 때만 `type <label> <css> --value-stdin`로 씁니다",
  invalid_selector: "셀렉터가 올바르지 않습니다",
  selector_not_found: "셀렉터와 맞는 요소가 없습니다",
  element_not_visible: "고른 요소가 보이지 않습니다",
  element_obscured: "고른 요소가 다른 요소에 가려져 있습니다",
  element_disabled: "고른 요소가 비활성화되어 있습니다",
  element_read_only: "고른 요소는 읽기 전용입니다",
  element_not_editable: "고른 요소에는 글을 입력할 수 없습니다",
  input_cancelled: "페이지가 입력을 거부했습니다",
  document_moving: "페이지가 읽는 동안 계속 바뀌어 한 상태로 읽을 수 없습니다 — 다시 `zerocode-browser marks`",
  document_replaced: "그 번호를 읽은 문서가 아닙니다 — 페이지가 바뀌었으니 다시 `zerocode-browser marks`",
  value_changed: "그 칸의 값이 marks 때와 다릅니다 — 다시 `zerocode-browser marks`",
  text_too_long: "입력 글이 요소의 최대 길이를 넘습니다",
  async_value: "비동기 값은 이 eval 왕복에서 돌려줄 수 없습니다",
  evaluation_failed: "페이지 식을 평가하지 못했습니다",
  serialization_failed: "페이지 식의 값을 직렬화할 수 없습니다",
  result_too_large: "페이지 식의 값이 너무 큽니다",
};
const PAGE_FAILED = "페이지 자동화가 실패했습니다";
const PAGE_ANSWER_UNREADABLE = "브라우저 판의 답을 읽을 수 없습니다";
const EVAL_NOT_ACCEPTED = "식이 문법에 맞지 않거나 판이 받지 않았습니다";
const NOT_OUR_PANE = "이 창이 만든 브라우저 판이 아닙니다";
/* The selector's length, as `checked_selector` bounds it. */
const SELECTOR_CAP = 4_000;
/* What a press or a typing says of its way in (`input_report`): whether the
 * page's events were trusted, and the limitation the window adds. */
const INPUT_WAYS = {
  "dom-activation": [false, "DOM 클릭 이벤트는 isTrusted 검사를 요구하는 페이지에서 거부될 수 있습니다"],
  "editing-command": [true, null],
  "synthetic-events": [false, "합성 input fallback은 isTrusted 검사를 요구하는 페이지에서 거부될 수 있습니다"],
  "value-setter": [false, "값 setter는 되읽기 없이 쓰며 isTrusted 검사를 요구하는 페이지에서 거부될 수 있습니다"],
};
const WAYS_OF = { click: ["dom-activation"], keys: ["editing-command", "synthetic-events"], setter: ["value-setter"] };

/* A page's refusal, in the window's words. */
class Refused extends Error {}

const pageFailure = (reply) => PAGE_FAILURES[reply?.code] || PAGE_FAILED;

/* `input_said`: what a press or a typing did, then the way in and its limit. */
function inputSaid(what, report) {
  const [trusted, limitation] = INPUT_WAYS[report.method];
  const rect = Array.isArray(report.rect) && report.rect.length === 4 && typeof report.dpr === "number"
    ? `, rect=${report.rect.join(",")}, dpr=${report.dpr}` : "";
  return `${what} (method=${report.method}, trusted-events=${trusted}${rect})${limitation ? ` — ${limitation}` : ""}`;
}

/* The page's report of a press or a typing, kept to the ways its road may answer. */
function inputReport(value, road) {
  if (!value || !WAYS_OF[road].includes(value.method)) throw new Refused("페이지 입력 결과를 읽을 수 없습니다");
  return value;
}

/* `checked_expression`: the credential stores an eval does not read, matched
 * at identifier boundaries. */
function checkedExpression(expression) {
  if ([...expression].length > EXPRESSION_CAP) throw new Refused(`페이지 식은 ${EXPRESSION_CAP}자를 넘을 수 없습니다`);
  const words = expression.toLowerCase().split(/[^a-z0-9_$]+/).filter(Boolean);
  const alone = ["cookiestore", "localstorage", "sessionstorage"];
  const after = [["document", "cookie"], ["navigator", "credentials"]];
  if (words.some((word) => alone.includes(word)) || words.some((word, at) => after.some(([first, second]) => word === first && words[at + 1] === second))) {
    throw new Refused("페이지 자격증명 저장소는 eval 결과로 읽을 수 없습니다");
  }
}

function checkedSelector(selector) {
  if (!selector.trim() || [...selector].length > SELECTOR_CAP) throw new Refused(`셀렉터는 1~${SELECTOR_CAP}자여야 합니다`);
}

/* The words after each `--flag`, and the bare `--switch`es, of one argv. */
function flagsOf(argv) {
  const flags = {};
  for (let at = 1; at < argv.length; at += 1) {
    if (!argv[at].startsWith("--")) continue;
    const next = argv[at + 1];
    if (next === undefined || next.startsWith("--")) flags[argv[at].slice(2)] = true;
    else { flags[argv[at].slice(2)] = next; at += 1; }
  }
  return flags;
}

const said = (text) => ({ status: 200, text });
const refused = (text) => ({ status: 409, text });
const pretty = (value) => `${JSON.stringify(value, null, 2)}\n`;

/* The desk for one scene. `doorText` is the built `door_text` example. */
export async function startFormDesk({ scene, doorText, headless = true }) {
  const site = await serveScene(scene.folder);
  const origin = new URL(site.url).origin;
  const browser = await chromium.launch({ headless });
  const context = await browser.newContext({ viewport: VIEWPORT });
  const tally = { requests: 0, verbs: {}, unwired: [], refusals: 0, faults: [], handoffs: 0, personMs: 0, blocked: [], dialogs: 0, popups: 0 };
  const trail = [];
  await context.route("**/*", (route) => {
    const url = route.request().url();
    if (url.startsWith(origin) || /^(data|blob|about):/.test(url)) return route.continue();
    tally.blocked.push(url.slice(0, 120));
    return route.abort();
  });
  const page = await context.newPage();
  context.on("page", (popup) => { tally.popups += 1; popup.close().catch(() => {}); });
  page.on("dialog", (dialog) => { tally.dialogs += 1; dialog.dismiss().catch(() => {}); });
  await page.goto(site.url);
  let form = null;
  let closed = false;

  /* What the core says: one request to `door_text`, one answer. */
  const ask = (request) => {
    const done = spawnSync(doorText, [], { input: JSON.stringify(request), encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
    if (done.status !== 0) throw new Error(`door_text failed (${done.status}): ${String(done.stderr).slice(0, 300)}`);
    return JSON.parse(done.stdout);
  };
  const fence = (words, source = PANE) => ask({ op: "fence", label: source, words }).words;
  const usage = () => refused(ask({ op: "usage", door: "browser" }).words);

  /* One page script, its answer decoded: a refusal of the page's in the window's words. */
  async function call(source, failure = PAGE_ANSWER_UNREADABLE) {
    let reply;
    try {
      reply = JSON.parse(await page.evaluate(source));
    } catch {
      throw new Refused(failure);
    }
    if (!reply || reply.ok !== true) throw new Refused(pageFailure(reply));
    return reply.value ?? null;
  }
  /* The same for a script handed to `fillPasses`, which wants the whole reply. */
  async function run(source) {
    const reply = JSON.parse(await page.evaluate(source));
    if (!reply || reply.ok !== true) throw new Refused(pageFailure(reply));
    return reply;
  }

  const wrongPane = (label) => (closed || label !== PANE ? refused(`zerocode-browser: ${NOT_OUR_PANE}\n`) : null);
  const sameOrigin = (url) => { try { return new URL(url).origin === origin; } catch { return false; } };
  const parse = (argv) => ask({ op: "parse", argv });
  const unwired = (verb) => {
    tally.unwired.push(verb);
    return refused(`zerocode-browser: this bench's stand-in window does not answer \`${verb}\` — it answers list, tabs, open, goto, fields, fill, click, type, read, eval, wait and scroll\n`);
  };

  async function browserDoor(argv) {
    const verb = argv[0] ?? "";
    const words = argv.length;
    if ((verb === "list" || verb === "tabs") && words === 1 && closed) return said(NO_PANE);
    if (verb === "list" && words === 1) return said(fence(`${PANE}\t${page.url()}\n`, EVERY_TAB));
    if (verb === "tabs" && words === 1) {
      return said(fence(`${PANE}\tfinished\t${(await page.title()).replace(/[\t\n\r]/g, " ")}\t${page.url()}\t-\t-\n`, EVERY_TAB));
    }
    if (verb === "open" && words === 2) {
      if (!sameOrigin(argv[1])) return refused("zerocode-browser: this bench's stand-in window opens only the scene's own page\n");
      // The one pane, opened again where it stands (its page is not loaded anew, so what it took is kept).
      if (closed) {
        closed = false;
        if (page.url() !== argv[1]) await page.goto(argv[1]);
      }
      return said(`열림 ${PANE}\n`);
    }
    if (verb === "goto" && words === 3) {
      const gone = wrongPane(argv[1]);
      if (gone) return gone;
      if (!sameOrigin(argv[2])) return refused("zerocode-browser: this bench's stand-in window goes only to the scene's own page\n");
      await page.goto(argv[2]);
      return said("가는 중\n");
    }
    if (verb === "close" && words === 2) {
      const gone = wrongPane(argv[1]);
      if (gone) return gone;
      closed = true;
      return said(`닫힘 ${PANE}\n`);
    }
    if (verb === "fields") {
      const parsed = parse(argv);
      if (!parsed.ok) return refused(parsed.words);
      const gone = wrongPane(parsed.label);
      if (gone) return gone;
      const read = await call(fieldsScript());
      form = read.fingerprint || null;
      return said(ask({ op: "fields", label: parsed.label, json: parsed.json, read }).words);
    }
    if (verb === "fill") {
      const parsed = parse(argv);
      if (!parsed.ok) return refused(parsed.words);
      const gone = wrongPane(parsed.label);
      if (gone) return gone;
      const filled = await fillPasses(run, parsed.entries, form);
      const text = argv[2] === "--value" ? argv[3] : argv[2];
      const answer = ask({ op: "fill", label: parsed.label, text, passes: filled.rounds });
      if (!filled.stale) form = filled.fingerprint || null;
      return answer.ok ? said(answer.words) : refused(answer.words);
    }
    if (verb === "click" && words >= 3 && words <= 5) {
      const parsed = parse(argv);
      if (!parsed.ok) return refused(parsed.words);
      const gone = wrongPane(argv[1]);
      if (gone) return gone;
      if (parsed.mark) return unwired("click --mark");
      checkedSelector(parsed.css);
      return said(`${inputSaid(CLICK_SAID, inputReport(await call(clickScript(parsed.css)), "click"))}\n`);
    }
    if (verb === "type" && (words === 4 || (words === 5 && argv[3] === "--value"))) {
      const gone = wrongPane(argv[1]);
      if (gone) return gone;
      checkedSelector(argv[2]);
      const road = words === 5 ? "setter" : "keys";
      const value = await call(typeScript(argv[2], argv[words - 1], road));
      if (road === "keys" && value?.secureField) throw new Refused(pageFailure({ code: "persons_entry" }));
      return said(`${inputSaid(TYPED_SAID, inputReport(value, road))}\n`);
    }
    if (verb === "eval" && (words === 3 || words === 4)) {
      const gone = wrongPane(argv[1]);
      if (gone) return gone;
      const expression = words === 3 ? argv[2] : argv[2] === "--value" ? argv[3] : null;
      if (expression === null) return refused("zerocode-browser: eval <label> <expr> 또는 eval <label> --value-stdin\n");
      checkedExpression(expression);
      return said(fence(pretty(await call(evalScript(expression), EVAL_NOT_ACCEPTED))));
    }
    if (verb === "read" && (words === 2 || words === 3)) {
      const parsed = parse(argv);
      if (!parsed.ok) return refused(parsed.words);
      const gone = wrongPane(parsed.label);
      if (gone) return gone;
      if (parsed.selector) checkedSelector(parsed.selector);
      const report = await call(readScript(parsed.selector));
      const detail = report.dom ? `\nDOM:\n${report.dom}\n` : "";
      return said(fence(`제목: ${report.title}\n주소: ${report.url}\n\n${report.text}\n${detail}`, parsed.label));
    }
    if (verb === "wait" && (words === 3 || words === 4)) {
      const gone = wrongPane(argv[1]);
      if (gone) return gone;
      if (words === 4 && !/^\d+$/.test(argv[3])) return refused("zerocode-browser: 대기 시간은 밀리초 정수여야 합니다\n");
      const timeout = words === 4 ? Number(argv[3]) : WAIT_DEFAULT_MS;
      if (timeout < WAIT_MIN_MS || timeout > WAIT_MAX_MS) return refused(`zerocode-browser: 대기 시간은 ${WAIT_MIN_MS}~${WAIT_MAX_MS}ms여야 합니다\n`);
      checkedSelector(argv[2]);
      const until = Date.now() + timeout;
      for (;;) {
        if ((await call(waitScript(argv[2]))) === true) return said("조건이 충족됐습니다\n");
        const left = until - Date.now();
        if (left <= 0) throw new Refused(WAIT_TIMED_OUT);
        await sleep(Math.min(FILL_POLL_MS, left));
      }
    }
    if (verb === "scroll" && words === 3) {
      const gone = wrongPane(argv[1]);
      if (gone) return gone;
      const target = parse(argv);
      if (!target.ok) return refused(target.words);
      if (target.kind === "selector") checkedSelector(target.selector);
      const at = await call(scrollScript(target));
      return said(`스크롤 x=${at.x} y=${at.y}\n`);
    }
    if (["find", "marks", "screenshot", "console", "network", "viewport", "diagnose"].includes(verb)) return unwired(verb);
    return usage();
  }

  /* The person, where the window hands the agent a turn to one: the code on
   * the phone typed into the empty field that asks for it — as a person's
   * keys do — or nothing, when no code was sent or no field asks. */
  async function personTypes() {
    const code = await page.evaluate(() => window.__personPhone || null);
    if (!code) return { code: false, typed: false };
    const mark = await page.evaluate(({ asks, not }) => {
      const drawn = (el) => el.checkVisibility({ visibilityProperty: true, checkVisibilityCSS: true });
      const skipped = ["hidden", "checkbox", "radio", "submit", "button", "file", "password", "image", "reset"];
      const wordsOf = (el) => [el.getAttribute("aria-label"), el.placeholder, el.name, el.id, el.getAttribute("autocomplete"),
        ...[...(el.labels || [])].map((label) => label.textContent), (el.closest("label") || {}).textContent].join(" ");
      const open = [...document.querySelectorAll("input")].filter((el) => drawn(el) && !el.disabled && !el.readOnly
        && !skipped.includes(el.type) && el.value === "");
      const asking = open.filter((el) => new RegExp(asks.source, asks.flags).test(wordsOf(el)));
      // The field that says it takes a one-time code first, then the ones whose words ask for a code that is not a postcode's or a coupon's.
      const pick = open.find((el) => el.getAttribute("autocomplete") === "one-time-code")
        || asking.find((el) => !new RegExp(not.source, not.flags).test(wordsOf(el))) || asking[0];
      if (!pick) return false;
      pick.setAttribute("data-bench-person", "");
      return true;
    }, { asks: { source: CODE_ASKED.source, flags: CODE_ASKED.flags }, not: { source: CODE_NOT_ASKED.source, flags: CODE_NOT_ASKED.flags } });
    if (!mark) return { code: true, typed: false };
    const field = page.locator("[data-bench-person]").first();
    await field.click();
    await page.keyboard.type(code, { delay: 30 });
    await field.evaluate((el) => el.removeAttribute("data-bench-person"));
    return { code: true, typed: true };
  }

  async function computerDoor(argv) {
    if (argv[0] !== "handoff") return refused("zerocode-computer: this bench's stand-in window answers only `handoff` — the desktop is not reachable from here\n");
    const flags = flagsOf(argv);
    const reason = typeof flags.reason === "string" ? flags.reason.trim() : "";
    if (!reason) return refused("zerocode-computer: handoff needs --reason: what the person has to do\n");
    await sleep(PERSON_TURN_MS);
    tally.personMs += PERSON_TURN_MS;
    tally.handoffs += 1;
    const turn = await personTypes();
    trail.push({ person: turn });
    const result = { resumed: true, reason };
    return said(flags.json ? `${JSON.stringify({ ok: true, result })}\n` : pretty(result));
  }

  /* One request at a time: a page takes one script at a time, and a model's
   * second call waits for its first. */
  let chain = Promise.resolve();
  const exclusive = (job) => (chain = chain.then(job, job));

  const server = createServer((request, response) => {
    const chunks = [];
    request.on("data", (chunk) => chunks.push(chunk));
    request.on("end", () => {
      exclusive(async () => {
        const door = request.url.replace(/^\//, "");
        const argv = Buffer.concat(chunks).toString("utf8").split(SEPARATOR)
          .filter((word, at, all) => !(at === all.length - 1 && word === ""));
        const began = performance.now();
        let answer;
        try {
          if (door === "browser") answer = await browserDoor(argv);
          else if (door === "computer") answer = await computerDoor(argv);
          else answer = { status: 404, text: "" };
        } catch (error) {
          if (error instanceof Refused) answer = refused(`zerocode-${door}: ${error.message}\n`);
          else {
            tally.faults.push(String(error?.message || error).slice(0, 300));
            answer = refused(`zerocode-${door}: ${PAGE_FAILED}\n`);
          }
        }
        const verb = argv[0] ?? "";
        tally.requests += 1;
        tally.verbs[`${door}/${verb}`] = (tally.verbs[`${door}/${verb}`] || 0) + 1;
        if (answer.status !== 200) tally.refusals += 1;
        trail.push({ n: tally.requests, door, verb, words: argv.length, ms: Math.round(performance.now() - began), status: answer.status, bytes: answer.text.length });
        response.writeHead(answer.status, { "content-type": "text/plain; charset=utf-8" }).end(answer.text);
      });
    });
  });
  await new Promise((done) => server.listen(0, "127.0.0.1", done));

  return {
    port: server.address().port,
    url: site.url,
    page,
    tally,
    trail,
    /* What the page took: its own result, or null when it never took one. */
    result: () => page.evaluate(() => window.__sceneResult || null),
    async close() {
      server.close();
      await browser.close();
      site.close();
    },
  };
}
