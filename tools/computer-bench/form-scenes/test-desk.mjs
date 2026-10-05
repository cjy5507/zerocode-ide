/* The form bench's stand-in window, its shim and the box a real agent runs in,
 * held to what the window answers (t-41387) — without a model. A scripted
 * caller stands where an agent's shell stands: it runs the shim (`zerocode-
 * browser`, `zerocode-computer`) with the stdin and flags a model uses, against
 * a small page of every kind of field the verbs touch, and reads what comes back
 * — the words of the window's own formatters, the refusals on stderr with exit 1,
 * the person's turn, the network the page may not use. The box's rules are
 * checked as data.
 *
 *   node test-desk.mjs --door-text PATH
 */

import { execFile } from "node:child_process";
import { mkdtemp, mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import { SYSTEM_PATH, STAND_IN_NAMES, ALLOWED_TOOLS, argsProblems, boxEnv, checkPath, claudeArgs, installStandIn } from "./agent-box.mjs";
import { PANE, PERSON_TURN_MS, startFormDesk } from "./door-desk.mjs";
import { doorTextFromArgv, writeFixtureScene } from "./fixture-scene.mjs";

const doorText = doorTextFromArgv();


const results = [];
async function test(name, run) {
  try { results.push({ name, pass: true, detail: (await run()) ?? "" }); }
  catch (error) { results.push({ name, pass: false, detail: error?.stack ?? String(error) }); }
}
function assert(condition, message, detail = undefined) {
  if (!condition) throw new Error(detail === undefined ? message : `${message}: ${JSON.stringify(detail)}`);
}

// The scene on disk, the stand-in's folder, and the desk.
const root = await mkdtemp(join(tmpdir(), "form-desk-"));
const folder = join(root, "scene");
await writeFixtureScene(folder, { name: "Kim", size: "l", agree: true, verified: true });
const bin = join(root, "bin");
await installStandIn({ bin, doorText });
const desk = await startFormDesk({ scene: { folder, name: "fixture", card: {}, expected: {} }, doorText });
const env = { PATH: `${bin}:${SYSTEM_PATH.join(":")}`, FORM_DESK: `http://127.0.0.1:${desk.port}` };

/* The shim, as an agent's shell runs it: its words, its stdin, what it printed, its code. */
const shim = (door, words, input = "") => new Promise((done) => {
  const child = execFile(join(bin, `zerocode-${door}`), words, { env, encoding: "utf8", maxBuffer: 16 * 1024 * 1024 },
    (error, stdout, stderr) => done({ code: error ? error.code : 0, stdout, stderr }));
  child.stdin.end(input);
});
const browser = (...words) => shim("browser", words);
const page = (source) => desk.page.evaluate(source);
const fenced = (text) => text.startsWith("<<<BEGIN UNTRUSTED EXTERNAL CONTENT (") && text.trimEnd().endsWith(">>>");

await test("--help answers locally from the core's manual, and a missing window is said, not guessed", async () => {
  const help = await shim("browser", ["--help"]);
  assert(help.code === 0 && help.stdout.includes("zerocode-browser fields") && help.stdout.includes("fill"), "the manual is the core's", help.stdout.slice(0, 200));
  const lost = await new Promise((done) => execFile(join(bin, "zerocode-browser"), ["list"], { env: { PATH: env.PATH }, encoding: "utf8" },
    (error, stdout, stderr) => done({ code: error ? error.code : 0, stderr })));
  assert(lost.code === 1 && lost.stderr.includes("not inside a ZeroCode window"), "no window: said on stderr, exit 1", lost);
  return "manual and refusal as the window's shim has them";
});

await test("list, tabs and open answer as the window does, the page's words fenced", async () => {
  const list = await browser("list");
  assert(list.code === 0 && fenced(list.stdout) && list.stdout.includes(`${PANE}\thttp://127.0.0.1:`), "list: a fenced line per pane", list.stdout);
  const tabs = await browser("tabs");
  assert(tabs.stdout.includes(`${PANE}\tfinished\tfixture\t`), "tabs: label, state, title, address", tabs.stdout);
  const open = await browser("open", desk.url);
  assert(open.stdout === `열림 ${PANE}\n`, "open: the pane's label", open);
  const stranger = await browser("open", "https://example.com/");
  assert(stranger.code === 1 && stranger.stderr.includes("opens only the scene's own page"), "another site is refused", stranger);
  return "ok";
});

await test("fields reads the form, in the core's words and as JSON", async () => {
  const read = await browser("fields", PANE);
  assert(read.code === 0 && fenced(read.stdout) && read.stdout.includes("Name") && read.stdout.includes("Verification code") === false, "fenced lines, the hidden code box not yet", read.stdout);
  const json = JSON.parse((await browser("fields", PANE, "--json")).stdout);
  assert(json.untrustedExternalContent === true && json.fields.some((field) => field.label === "Name"), "JSON carries the fence's flag", Object.keys(json));
  const wrong = await browser("fields", "browser-9");
  assert(wrong.code === 1 && wrong.stderr.includes("이 창이 만든 브라우저 판이 아닙니다"), "another pane's label is refused", wrong);
  return `${json.fields.length} fields`;
});

await test("fill writes a bundle read from stdin, says each field, and refuses on stderr what did not take", async () => {
  const ok = await shim("browser", ["fill", PANE, "--value-stdin"], '{"#name":"Kim","#size":"Large","#agree":true}\n');
  assert(ok.code === 0 && fenced(ok.stdout) && ok.stdout.includes("채움 3/3칸") && ok.stdout.includes('= "Kim"'), "every field took: stdout, fenced, each with what it holds", ok);
  assert(ok.stdout.includes("양식 그대로 — 버튼: #send 「Send code」 켜짐, #book 「Book」 켜짐"), "and ends with the form read and the buttons now", ok.stdout);
  assert(await page(() => document.getElementById("name").value) === "Kim" && await page(() => document.getElementById("size").value) === "l"
    && await page(() => document.getElementById("agree").checked) === true, "and the page holds them");
  const none = await shim("browser", ["fill", PANE, "--value-stdin"], '{"#size":"Gigantic"}');
  assert(none.code === 1 && none.stderr.includes("그 값의 선택지가 없음"), "a choice that is not there: stderr, exit 1", none);
  const bad = await shim("browser", ["fill", PANE, "--value-stdin"], "not json");
  assert(bad.code === 1 && bad.stderr.includes("fill 값은 JSON입니다"), "the core's own refusal for a bundle that is none", bad);
  return "ok";
});

await test("a fill held to a form that changed writes nothing and says so", async () => {
  await browser("fields", PANE);
  await page(() => { document.getElementById("f").insertAdjacentHTML("beforeend", '<label>Extra <input id="extra"></label>'); });
  const stale = await shim("browser", ["fill", PANE, "--value-stdin"], '{"#name":"Lee"}');
  assert(stale.code === 1 && stale.stderr.includes("form_stale"), "stale: stderr, exit 1", stale);
  assert(await page(() => document.getElementById("name").value) === "Kim", "and nothing was written");
  await page(() => document.getElementById("extra").closest("label").remove());
  return "ok";
});

await test("click, type, eval, read, wait and scroll say what the window says", async () => {
  const click = await browser("click", PANE, "#send");
  assert(click.stdout.startsWith("클릭 이벤트를 보냈습니다 (method=dom-activation, trusted-events=false, rect="), "click: the press and its way in", click.stdout);
  const refused = await browser("type", PANE, "#pw", "hunter2");
  assert(refused.code === 1 && refused.stderr.includes("비밀번호 칸입니다"), "a password is not typed on the keys road", refused);
  const secret = await shim("browser", ["type", PANE, "#pw", "--value-stdin"], "hunter2\n");
  assert(secret.stdout.startsWith("입력했습니다 (method=value-setter"), "but the stdin road writes it", secret);
  assert(await page(() => document.getElementById("pw").value) === "hunter2", "and the page holds it");
  const title = await browser("eval", PANE, "document.title");
  assert(fenced(title.stdout) && title.stdout.includes('"fixture"'), "eval: the value as pretty JSON, fenced", title.stdout);
  const store = await browser("eval", PANE, "localStorage.getItem('x')");
  assert(store.code === 1 && store.stderr.includes("자격증명 저장소"), "a credential store is refused", store);
  const pair = await shim("browser", ["eval", PANE, "--value-stdin"], "zerocode.fields().fields.length");
  assert(pair.code === 0 && Number(pair.stdout.split("\n").find((line) => /^\d+$/.test(line))) >= 4, "an eval that names the form pair reads the form", pair.stdout);
  const read = await browser("read", PANE);
  assert(fenced(read.stdout) && read.stdout.includes("제목: fixture"), "read: title, address, text", read.stdout.slice(0, 200));
  const waited = await browser("wait", PANE, "#name", "500");
  assert(waited.stdout === "조건이 충족됐습니다\n", "wait: the condition", waited);
  const timedOut = await browser("wait", PANE, "#nothing", "300");
  assert(timedOut.code === 1 && timedOut.stderr.includes("시간 안에 셀렉터가 보이지 않았습니다"), "wait: the time ran out", timedOut);
  const scrolled = await browser("scroll", PANE, "#last");
  assert(/^스크롤 x=\d+ y=\d+\n$/.test(scrolled.stdout), "scroll: where the page stands", scrolled);
  return "ok";
});

await test("a verb the stand-in does not answer is refused in words and counted, never guessed", async () => {
  const marks = await browser("marks", PANE);
  assert(marks.code === 1 && marks.stderr.includes("does not answer `marks`"), "marks: refused", marks);
  assert(desk.tally.unwired.includes("marks"), "and counted", desk.tally.unwired);
  const nonsense = await browser("frobnicate");
  assert(nonsense.code === 1 && nonsense.stderr.includes("zerocode-browser"), "an unknown verb: the manual, on stderr", nonsense.stderr.slice(0, 120));
  return "ok";
});

await test("the person's turn: the code on the phone is typed into the field that asks for it", async () => {
  const began = Date.now();
  const turn = await shim("computer", ["handoff", "--reason", "휴대폰의 인증번호"]);
  assert(turn.code === 0 && turn.stdout.includes('"resumed": true'), "handoff: resumed", turn);
  assert(Date.now() - began >= PERSON_TURN_MS - 200, "after the time a person takes", Date.now() - began);
  assert(await page(() => document.getElementById("code").value) === "123456", "the code is in its field");
  assert(desk.tally.personMs === PERSON_TURN_MS && desk.tally.handoffs === 1, "and the person's time is kept apart", desk.tally);
  const nothing = await shim("computer", ["handoff"]);
  assert(nothing.code === 1 && nothing.stderr.includes("handoff needs --reason"), "a handoff names what the person must do", nothing);
  const desktop = await shim("computer", ["observe", "--app", "Finder"]);
  assert(desktop.code === 1 && desktop.stderr.includes("answers only `handoff`"), "the desktop is not reachable", desktop);
  return "ok";
});

await test("the page reaches only the bench's own site, and the oracle reads what it took", async () => {
  await page(() => { new Image().src = "https://example.invalid/pixel.png?x=1"; });
  await sleep(300);
  assert(desk.tally.blocked.some((url) => url.startsWith("https://example.invalid/")), "an outside request was aborted and counted", desk.tally.blocked);
  assert((await desk.result()) === null, "no result before the booking");
  await browser("click", PANE, "#verify");
  await browser("click", PANE, "#book");
  assert(JSON.stringify(await desk.result()) === JSON.stringify({ name: "Kim", size: "l", agree: true, verified: true }), "the page's own result", await desk.result());
  return `${desk.tally.requests} requests, ${desk.tally.refusals} refused, ${desk.tally.faults.length} faults`;
});

await test("a pane that was closed can be opened again, and the scene's site answers its folder and a bad address without dying", async () => {
  const closed = await browser("close", PANE);
  assert(closed.stdout === `닫힘 ${PANE}\n`, "close: gone", closed);
  assert((await browser("list")).stdout.includes("열린 브라우저 판이 없습니다") && (await browser("tabs")).stdout.includes("열린 브라우저 판이 없습니다"), "list and tabs: no pane now");
  assert((await browser("fields", PANE)).code === 1, "a closed pane's label is refused");
  const reopened = await browser("open", desk.url);
  assert(reopened.stdout === `열림 ${PANE}\n` && (await browser("fields", PANE)).code === 0, "open: the pane is back and answers", reopened);
  const origin = new URL(desk.url).origin;
  const folder = await fetch(`${origin}/`);
  assert(folder.status === 200 && String(folder.headers.get("content-type")).startsWith("text/html"), "the folder is its scene, told as a page", [folder.status, folder.headers.get("content-type")]);
  const bad = await fetch(`${origin}/%E0%A4%A`);
  assert(bad.status === 400, "a bad escape is a 400, not a crash", bad.status);
  assert((await fetch(desk.url)).status === 200, "and the site still answers");
  return "ok";
});

await test("the box: a narrow tool list, no widening flag, an environment from nothing, and a PATH that holds", async () => {
  const args = claudeArgs({ model: "claude-haiku-4-5-20251001" });
  assert(argsProblems(args).length === 0, "the arguments hold the box", argsProblems(args));
  assert(args.includes("--allowedTools") && ALLOWED_TOOLS.every((tool) => args.includes(tool)), "the tools are the narrow ones");
  assert(!args.some((word) => /^Bash$|^Bash\(\*\)$/.test(word) && args[args.indexOf(word) - 1] === "--allowedTools"), "Bash as a whole is not allowed");
  assert(argsProblems([...args, "--dangerously-skip-permissions"]).length > 0, "a flag that skips permission checks is refused");
  assert(argsProblems(args.map((word) => (word === ALLOWED_TOOLS[1] ? "Bash" : word))).length > 0, "Bash as a whole is refused");
  const names = Object.keys(boxEnv({ bin, home: "/h", config: "/c", tmp: "/t", desk: "http://127.0.0.1:1", user: "u" }));
  assert(!names.some((name) => /(TOKEN|SECRET|PASSWORD)/.test(name)) && !names.includes("ANTHROPIC_API_KEY"), "no token travels in the environment", names);
  assert(!names.some((name) => /SECURESTORAGE/.test(name)), "and no login is named: the real-model road is closed", names);
  const holds = checkPath(env.PATH, bin);
  assert(holds.holds && STAND_IN_NAMES.every((name) => holds.lines.some((line) => line.startsWith(`PATH ${name} -> ${join(bin, name)}`))), "the stand-in alone answers to the door's names", holds.lines);
  const real = join(root, "real");
  await mkdir(real);
  await writeFile(join(real, "zerocode-orc"), "#!/bin/sh\n", { mode: 0o755 });
  assert(!checkPath(`${bin}:${real}:${SYSTEM_PATH.join(":")}`, bin).holds, "a real window command on PATH breaks the check");
  return holds.lines.join(" | ");
});

/* A fill's answer is the page after it has settled (t-41387): a page that turns a
 * button on in a microtask after its input handler has returned shows it on. */
await test("a fill answers with the buttons the page settled on, not those of the instant it was written", async () => {
  await page(() => {
    const name = document.getElementById("name");
    const book = document.getElementById("book");
    book.disabled = true;
    name.addEventListener("input", () => queueMicrotask(() => { book.disabled = name.value === ""; }));
  });
  await browser("fields", PANE);
  const filled = await shim("browser", ["fill", PANE, "--value-stdin"], '{"#name":"Lee"}');
  assert(filled.code === 0 && filled.stdout.includes("#book 「Book」 켜짐"), "the button the page turned on is on in the answer", filled.stdout);
  assert(!filled.stdout.includes("아직 바뀌는 중"), "and the page stood still when it was read", filled.stdout);
  assert(filled.stdout.includes("그 뒤에 뜨는 오류는 못 봅니다") && filled.stdout.includes("제출 전에 fields로 다시 읽으세요"),
    "and says what a read after a quiet window cannot see", filled.stdout);
  return "ok";
});

/* What a fill says when an error shows later than the page stood still (t-41387, m-41521):
 * it does not see it, and says so — the contract for the case the settle cannot see. */
await test("a fill that waited says an error shown later is not in its answer, and the next read has it", async () => {
  await page(() => {
    const name = document.getElementById("name");
    name.addEventListener("input", () => setTimeout(() => name.setAttribute("aria-invalid", "true"), 150));
  });
  await browser("fields", PANE);
  const filled = await shim("browser", ["fill", PANE, "--value-stdin"], '{"#name":"Park"}');
  assert(filled.code === 0 && !filled.stdout.includes("aria-invalid"), "the error that shows 150 ms later is not in the answer", filled.stdout);
  assert(filled.stdout.includes("그 뒤에 뜨는 오류는 못 봅니다") && filled.stdout.includes("제출 전에 fields로 다시 읽으세요"),
    "and the answer says so", filled.stdout);
  await sleep(300);
  const next = await browser("fields", PANE);
  assert(next.stdout.includes("aria-invalid"), "the next read has it", next.stdout);
  return "ok";
});

await desk.close();
let failed = 0;
for (const result of results) {
  if (!result.pass) failed += 1;
  console.log(`${result.pass ? "PASS" : "FAIL"}  ${result.name}${result.detail ? `  — ${result.detail}` : ""}`);
}
console.log(`\n${results.length - failed}/${results.length} passed`);
process.exit(failed ? 1 : 0);
