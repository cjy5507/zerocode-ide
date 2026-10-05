/* The scripted drivers (drive.mjs) held to the rules a model takes from the door's
 * words and the skill (t-41592), on two small scenes made here — neither a scene
 * of the bench: a page whose fill brings a field of its own ("양식 바뀜") is read
 * again before anything is pressed, and a field the page keeps off until its terms
 * are read to the end is switched on by scrolling that box the way the skill
 * teaches — then the step goes on. The button that moves a step on is chosen by what
 * the door said of the buttons (t-41656) — a page's own submit, the one the fill turned on, the
 * page's last — and waited for, a fixed number of times, while it is off and the door has said
 * new text; a group of chips is written with fill. Both roads of the driver, the verbs and the
 * script, are held to all of it. The driver's row and a person's road are held to the
 * oracle's three verdicts, said side by side, and to keeping the page's whole
 * result, so any verdict can be counted again later.
 *
 *   node test-drive.mjs
 */

import { spawnSync } from "node:child_process";
import { mkdtemp, mkdir, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { scrollToEnd } from "./door-recipes.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const SKILL = await readFile(join(HERE, "../../../skills/computer-use/SKILL.md"), "utf8");

const results = [];
async function test(name, run) {
  try { results.push({ name, pass: true, detail: (await run()) ?? "" }); }
  catch (error) { results.push({ name, pass: false, detail: error?.stack ?? String(error) }); }
}
function assert(condition, message, detail = undefined) {
  if (!condition) throw new Error(detail === undefined ? message : `${message}: ${JSON.stringify(detail)}`);
}

const root = await mkdtemp(join(tmpdir(), "form-drive-"));
async function scene(name, html, facts, expected, files = {}) {
  const folder = join(root, name);
  await mkdir(folder, { recursive: true });
  await writeFile(join(folder, "scene.html"), html);
  await writeFile(join(folder, "card.json"), JSON.stringify({ task: "Fill it in.", facts, personTurns: [] }));
  await writeFile(join(folder, "expected.json"), JSON.stringify(expected));
  for (const [file, text] of Object.entries(files)) await writeFile(join(folder, file), text);
  return folder;
}
const REVEAL = await scene("reveal", `<!doctype html><html lang="en"><meta charset="utf-8"><form onsubmit="return false">
  <label for="name">Name</label> <input id="name">
  <label><input type="checkbox" id="invoice"> I need an invoice</label>
  <div id="more" hidden><label for="company">Company</label> <input id="company"></div>
  <button type="button" id="send" disabled>Submit</button>
</form>
<script>
  const name = document.getElementById("name"), invoice = document.getElementById("invoice"), company = document.getElementById("company");
  const ready = () => { document.getElementById("send").disabled = !(name.value && (!invoice.checked || company.value)); };
  invoice.addEventListener("change", () => { document.getElementById("more").hidden = !invoice.checked; ready(); });
  for (const el of [name, company]) el.addEventListener("input", ready);
  document.getElementById("send").addEventListener("click", () => {
    window.__sceneResult = { name: name.value, invoice: invoice.checked, company: company.value };
  });
</script>`, [{ says: "Name", value: "Kim" }, { says: "I need an invoice", value: true }, { says: "Company", value: "Acme" }],
{ name: "Kim", invoice: true, company: "Acme" });
const TERMS = await scene("terms", `<!doctype html><html lang="en"><meta charset="utf-8"><form onsubmit="return false">
  <label for="mail">Email</label> <input id="mail">
  <div id="terms" style="height:50px;overflow:auto"><p>1</p><p>2</p><p>3</p><p>4</p><p>5</p><p>6</p><p>7</p><p>8</p><p>9</p><p>10</p><p>11</p><p>12</p></div>
  <div class="field"><label><input type="checkbox" id="agree" disabled> I agree</label> <span class="note">Read the terms to the end to switch this on.</span></div>
  <button type="button" id="go" disabled>Submit</button>
</form>
<script>
  const terms = document.getElementById("terms"), agree = document.getElementById("agree"), mail = document.getElementById("mail"), go = document.getElementById("go");
  const ready = () => { go.disabled = !(mail.value && agree.checked); };
  terms.addEventListener("scroll", () => { if (terms.scrollTop + terms.clientHeight >= terms.scrollHeight - 1) agree.disabled = false; });
  mail.addEventListener("input", ready);
  agree.addEventListener("change", ready);
  go.addEventListener("click", () => { window.__sceneResult = { mail: mail.value, agree: agree.checked }; });
</script>`, [{ says: "Email", value: "kim@example.com" }, { says: "I agree", value: true }],
{ mail: "kim@example.com", agree: true });

/* Three small pages that record a result of their own kind: one keeps a field the card leaves alone (a memo), one a reference of its own making that its
 * scene declares with a shape, one the same reference left empty. */
const SEND = (record) => `<!doctype html><html lang="en"><meta charset="utf-8"><form onsubmit="return false">
  <label for="name">Name</label> <input id="name">
  <label for="memo">Memo</label> <textarea id="memo"></textarea>
  <button type="button" id="send">Submit</button>
</form>
<script>
  const name = document.getElementById("name"), memo = document.getElementById("memo");
  document.getElementById("send").addEventListener("click", () => { window.__sceneResult = ${record}; });
</script>`;
const FACT_NAME = [{ says: "Name", value: "Kim" }];
const LEFT = await scene("left", SEND("{ name: name.value, memo: memo.value }"), FACT_NAME, { name: "Kim" });
const MADE = await scene("made", SEND("{ name: name.value, ref: 'R-' + (1000 + Math.floor(Math.random() * 9000)) }"), FACT_NAME, { name: "Kim" },
  { "made.json": JSON.stringify({ keys: ["ref"], shapes: { ref: "^R-\\d{4}$" } }) });
const MADE_EMPTY = await scene("madeempty", SEND("{ name: name.value, ref: '' }"), FACT_NAME, { name: "Kim" },
  { "made.json": JSON.stringify({ keys: ["ref"], shapes: { ref: "^R-\\d{4}$" } }) });

/* A person's road through two of them: the page, and what a person does on it. */
const SOLVE = "export async function solveAsPerson(page) { await page.fill('#name', 'Kim'); await page.click('#send'); }\n";
const PERSON_KEPT = await scene("person-kept", SEND("{ name: name.value }"), FACT_NAME, { name: "Kim" }, { "selfcheck.mjs": SOLVE });
const PERSON_LEFT = await scene("person-left", SEND("{ name: name.value, memo: memo.value }"), FACT_NAME, { name: "Kim" }, { "selfcheck.mjs": SOLVE });

/* The button that moves a step on, four pages that name no word a list could know (t-41656): a step whose button is
 * off until the fill turns it on, with a button that goes back before it, and a last step whose button is on from the
 * start; a form whose submit button the page declares, with two other buttons after it that must not be pressed;
 * a form whose button stays off for a moment while a check the page runs writes "Checking…" beside the field;
 * and groups of chips — facts that name the options, a text that lists them, one choice of several. */
const ADVANCE = await scene("advance", `<!doctype html><html lang="en"><meta charset="utf-8"><form onsubmit="return false">
  <div id="one"><label for="name">Name</label> <input id="name">
    <button type="button" id="back1">Back</button><button type="button" id="go" disabled>Go ahead</button></div>
  <div id="two" hidden><label for="mail">Email</label> <input id="mail">
    <button type="button" id="back2">Back</button><button type="button" id="wrap">Wrap it up</button></div>
</form>
<script>
  const $ = (id) => document.getElementById(id);
  $("name").addEventListener("input", () => { $("go").disabled = !$("name").value; });
  $("go").addEventListener("click", () => { $("one").hidden = true; $("two").hidden = false; });
  for (const id of ["back1", "back2"]) $(id).addEventListener("click", () => { window.__sceneResult = { name: "back" }; });
  $("wrap").addEventListener("click", () => { window.__sceneResult = { name: $("name").value, mail: $("mail").value }; });
</script>`, [{ says: "Name", value: "Kim" }, { says: "Email", value: "kim@example.com" }], { name: "Kim", mail: "kim@example.com" });
const ADVANCE_SUBMIT = await scene("advance-submit", `<!doctype html><html lang="en"><meta charset="utf-8"><form id="f" onsubmit="return false">
  <label for="name">Name</label> <input id="name">
  <button type="submit" id="save">Zzz</button><button type="button" id="cancel">Cancel</button><button type="button" id="help">Help</button>
</form>
<script>
  const $ = (id) => document.getElementById(id);
  $("f").addEventListener("submit", (event) => { event.preventDefault(); window.__sceneResult = { name: $("name").value }; });
  $("cancel").addEventListener("click", () => { window.__sceneResult = { name: "cancelled" }; });
  $("help").addEventListener("click", () => { window.__sceneResult = { name: "help" }; });
</script>`, [{ says: "Name", value: "Kim" }], { name: "Kim" });
const LOADING = await scene("loading", `<!doctype html><html lang="en"><meta charset="utf-8"><form onsubmit="return false">
  <div class="row"><label for="name">Name</label> <input id="name"><span id="note"></span></div>
  <div class="row"><label for="mail">Email</label> <input id="mail"></div>
  <button type="button" id="go" disabled>Continue</button>
</form>
<script>
  const $ = (id) => document.getElementById(id);
  let timer = 0;
  $("name").addEventListener("input", () => {
    $("go").disabled = true;
    $("note").textContent = "Checking…";
    clearTimeout(timer);
    timer = setTimeout(() => { $("go").disabled = !$("name").value; $("note").textContent = "Looks fine"; }, 1600);
  });
  $("go").addEventListener("click", () => { window.__sceneResult = { name: $("name").value }; });
</script>`, [{ says: "Name", value: "Kim" }], { name: "Kim" });
const CHIPS = await scene("chips", `<!doctype html><html lang="en"><meta charset="utf-8"><form onsubmit="return false">
  <label for="name">Name</label> <input id="name">
  <div id="cl">Pick colours</div>
  <div id="colours" role="group" aria-labelledby="cl"><button type="button" aria-pressed="false">Red</button><button type="button" aria-pressed="true">Green</button><button type="button" aria-pressed="false">Blue</button></div>
  <div id="sl">Size</div>
  <div id="size" role="group" aria-labelledby="sl"><button type="button" aria-pressed="false">Small</button><button type="button" aria-pressed="true">Medium</button><button type="button" aria-pressed="false">Large</button></div>
  <div id="xl">Extras</div>
  <div id="extras" role="group" aria-labelledby="xl"><button type="button" aria-pressed="false">Tea</button><button type="button" aria-pressed="false">Cake</button><button type="button" aria-pressed="false">Jam</button></div>
  <button type="button" id="send">Submit</button>
</form>
<script>
  const $ = (id) => document.getElementById(id);
  const held = (id) => [...document.querySelectorAll("#" + id + " button")].filter((b) => b.getAttribute("aria-pressed") === "true").map((b) => b.textContent);
  for (const id of ["colours", "extras"]) $(id).addEventListener("click", (event) => {
    const button = event.target.closest("button");
    if (button) button.setAttribute("aria-pressed", String(button.getAttribute("aria-pressed") !== "true"));
  });
  $("size").addEventListener("click", (event) => {
    const button = event.target.closest("button");
    if (button) for (const other of document.querySelectorAll("#size button")) other.setAttribute("aria-pressed", String(other === button));
  });
  $("send").addEventListener("click", () => { window.__sceneResult = { name: $("name").value, colours: held("colours"), size: held("size")[0] || "", extras: held("extras") }; });
</script>`, [{ says: "Name", value: "Kim" }, { says: "Red", value: true }, { says: "Green", value: false }, { says: "Blue", value: true },
  { says: "Size", value: "Large" }, { says: "Extras", value: "Tea, Jam" }],
{ name: "Kim", colours: ["Red", "Blue"], size: "Large", extras: ["Tea", "Jam"] });

/* A fact that names no field of the page, whose value is one option of a group of chips the page titles in other words: a time
 * among slots listed under "Morning" and "Afternoon", one choice across both groups. */
const SLOTS = await scene("slots", `<!doctype html><html lang="en"><meta charset="utf-8"><form onsubmit="return false">
  <label for="name">Name</label> <input id="name">
  <div class="g"><h4>Morning</h4><div id="am"><button type="button" aria-pressed="false">09:00</button><button type="button" aria-pressed="false">09:30</button><button type="button" aria-pressed="false">10:00</button></div></div>
  <div class="g"><h4>Afternoon</h4><div id="pm"><button type="button" aria-pressed="false">14:00</button><button type="button" aria-pressed="false">14:30</button></div></div>
  <button type="button" id="send">Submit</button>
</form>
<script>
  const $ = (id) => document.getElementById(id);
  const slots = () => [...document.querySelectorAll("#am button, #pm button")];
  for (const button of slots()) button.addEventListener("click", () => { for (const other of slots()) other.setAttribute("aria-pressed", String(other === button)); });
  $("send").addEventListener("click", () => {
    const held = slots().filter((button) => button.getAttribute("aria-pressed") === "true").map((button) => button.textContent);
    window.__sceneResult = { name: $("name").value, time: held[0] || "" };
  });
</script>`, [{ says: "Name", value: "Kim" }, { says: "Visit time", value: "14:30" }], { name: "Kim", time: "14:30" });

/* What the real pages showed of the press that ends a form (t-41656): a page that takes the booking a moment after the press — its button goes off
 * ("Working…") and the form is replaced by a page with no button — a page that refuses the press every time, and a field the page rewrites to its own mask,
 * so the read-back never matches and the answer is the same twice. */
const LATE = await scene("late", `<!doctype html><html lang="en"><meta charset="utf-8"><form onsubmit="return false">
  <label for="name">Name</label> <input id="name"><button type="button" id="go">Finish</button>
</form>
<script>
  const $ = (id) => document.getElementById(id);
  $("go").addEventListener("click", () => {
    const name = $("name").value;
    $("go").disabled = true;
    $("go").textContent = "Working…";
    setTimeout(() => { document.body.innerHTML = "<p>Done</p>"; window.__sceneResult = { name }; }, 2200);
  });
</script>`, [{ says: "Name", value: "Kim" }], { name: "Kim" });
const REFUSES = await scene("refuses", `<!doctype html><html lang="en"><meta charset="utf-8"><form onsubmit="return false">
  <label for="name">Name</label> <input id="name"><button type="button" id="go">Finish</button><p id="why"></p>
</form>
<script>
  document.getElementById("go").addEventListener("click", () => { document.getElementById("why").textContent = "Not yet"; });
</script>`, [{ says: "Name", value: "Kim" }], { name: "Kim" });
const MASKED = await scene("masked", `<!doctype html><html lang="en"><meta charset="utf-8"><form onsubmit="return false">
  <label for="code">Code</label> <input id="code"><button type="button" id="go">Submit</button>
</form>
<script>
  const $ = (id) => document.getElementById(id);
  $("code").addEventListener("input", () => { $("code").value = "A1"; });
  $("go").addEventListener("click", () => { window.__sceneResult = { code: $("code").value }; });
</script>`, [{ says: "Code", value: "B2" }], { code: "A1" });

/* One run of the drivers on every scene, once. */
const out = join(root, "rows.json");
const done = spawnSync(process.execPath, [join(HERE, "drive.mjs"), "--scene", REVEAL, "--scene", TERMS, "--scene", LEFT, "--scene", MADE, "--scene", MADE_EMPTY,
  "--scene", ADVANCE, "--scene", ADVANCE_SUBMIT, "--scene", LOADING, "--scene", CHIPS, "--scene", SLOTS, "--scene", LATE, "--scene", REFUSES, "--scene", MASKED, "--out", out], { encoding: "utf8", timeout: 720_000 });
const rows = await readFile(out, "utf8").then(JSON.parse, () => []);
const row = (scene, road) => rows.find((one) => one.scene === scene && one.road === road);
/* One run of a person's road through two scenes, its lines and the rows it keeps. */
const roadOut = join(root, "road.json");
const road = spawnSync(process.execPath, [join(HERE, "person-road.mjs"), "--scene", PERSON_KEPT, "--scene", PERSON_LEFT, "--out", roadOut], { encoding: "utf8", timeout: 240_000 });
const roadRows = await readFile(roadOut, "utf8").then(JSON.parse, () => []);

await test("a driver reads a form again before it presses when its fill brought a field of its own", () => {
  for (const road of ["verbs", "script"]) {
    const one = row("reveal", road);
    assert(one && one.ok, `the ${road} road finishes a form whose fill brings a field`, one && { ok: one.ok, stuck: one.stuck, wrong: one.wrong });
  }
  const verbs = row("reveal", "verbs");
  assert(verbs.fields >= 2 && verbs.fill >= 2, "it read the form again and filled what the read showed", { fields: verbs.fields, fill: verbs.fill });
  return `fields ${verbs.fields}, fill ${verbs.fill}`;
});

await test("a driver scrolls a box to its end by the recipe the skill teaches when a field is off and the read names the box", () => {
  for (const road of ["verbs", "script"]) {
    const one = row("terms", road);
    assert(one && one.ok, `the ${road} road switches the field on and goes on`, one && { ok: one.ok, stuck: one.stuck, wrong: one.wrong });
  }
  const verbs = row("terms", "verbs");
  assert(verbs.eval === 1, "the box was scrolled by one eval", { eval: verbs.eval });
  return `eval ${verbs.eval}`;
});

await test("a driver presses the button that moves a step on by what the door said of the buttons, not by a list of words", () => {
  for (const road of ["verbs", "script"]) {
    const one = row("advance", road);
    assert(one && one.ok && one.pass, `the ${road} road gets through two steps whose buttons carry no word a list knows, past the button that goes back`, one && { ok: one.ok, stuck: one.stuck, result: one.result });
  }
  return `verbs click ${row("advance", "verbs").click}, script eval ${row("advance", "script").eval}`;
});

await test("a driver prefers the button the page declares a submit to the last button of the page", () => {
  for (const road of ["verbs", "script"]) {
    const one = row("advance-submit", road);
    assert(one && one.ok && one.pass, `the ${road} road presses the submit the page declares, not the buttons after it`, one && { ok: one.ok, stuck: one.stuck, result: one.result });
  }
  return "submit";
});

await test("a driver waits a fixed number of times for the button that moves the form on while it is off and the door has said new text, then reads the form again", () => {
  for (const road of ["verbs", "script"]) {
    const one = row("loading", road);
    assert(one && one.ok && one.pass, `the ${road} road waits out the check and presses the button when it is on`, one && { ok: one.ok, stuck: one.stuck, wait: one.wait });
    assert(one.wait >= 1 && one.wait <= 3, `and waited no more than a fixed number of times (${road})`, { wait: one.wait });
  }
  return `waits ${row("loading", "verbs").wait} / ${row("loading", "script").wait}`;
});

await test("a driver writes a group of chips with fill — the options a fact names, the options a text lists, one choice of several", () => {
  for (const road of ["verbs", "script"]) {
    const one = row("chips", road);
    assert(one && one.ok && one.pass, `the ${road} road leaves the page holding the options the card names`, one && { ok: one.ok, stuck: one.stuck, result: one.result });
  }
  return JSON.stringify(row("chips", "verbs").result);
});

await test("a driver writes a fact that names no field to the group of chips whose options hold its value", () => {
  for (const road of ["verbs", "script"]) {
    const one = row("slots", road);
    assert(one && one.ok && one.pass, `the ${road} road gives the slot the card names, though the groups carry other titles`, one && { ok: one.ok, stuck: one.stuck, result: one.result });
  }
  return JSON.stringify(row("slots", "verbs").result);
});

await test("a driver that pressed a button which then went off waits for the page and takes the booking a page finishes later", () => {
  for (const road of ["verbs", "script"]) {
    const one = row("late", road);
    assert(one && one.ok && one.pass, `the ${road} road reads the result of a page that finished after the press, its form gone`, one && { ok: one.ok, stuck: one.stuck, wait: one.wait });
    assert(one.wait >= 1 && one.wait <= 3, `after a wait or two (${road})`, { wait: one.wait });
  }
  return `waits ${row("late", "verbs").wait} / ${row("late", "script").wait}`;
});

await test("a driver that pressed the same button twice in a form that stayed gives up and says so", () => {
  for (const road of ["verbs", "script"]) {
    const one = row("refuses", road);
    assert(one && typeof one.stuck === "string" && /pressed twice/.test(one.stuck), `the ${road} road says it pressed the button twice and the form stayed`, one && { stuck: one.stuck });
    assert(one.roundTrips <= 10, `and does not press it again to the end of its round trips (${road})`, { roundTrips: one.roundTrips });
  }
  return `round trips ${row("refuses", "verbs").roundTrips} / ${row("refuses", "script").roundTrips}`;
});

await test("a driver told twice the same answer about a field goes on to the button that moves the step", () => {
  for (const road of ["verbs", "script"]) {
    const one = row("masked", road);
    assert(one && one.ok && one.pass, `the ${road} road goes on when a field never reads back as it was asked`, one && { ok: one.ok, stuck: one.stuck, result: one.result });
  }
  return JSON.stringify(row("masked", "script").result);
});

await test("the recipe a driver scrolls a box by is the one the skill teaches", () => {
  assert(SKILL.includes("document.querySelector(String.raw`<handle>`).scrollTop = 1e9"), "the skill teaches the recipe in these words");
  assert(scrollToEnd("#terms") === "document.querySelector(String.raw`#terms`).scrollTop = 1e9", "a handle is read by the recipe as the skill says", scrollToEnd("#terms"));
  assert(scrollToEnd("#card >> #box") === "document.querySelector(String.raw`#card`).contentDocument.querySelector(String.raw`#box`).scrollTop = 1e9",
    "and a handle with the frame's separator is the frame and the box inside it", scrollToEnd("#card >> #box"));
  assert(SKILL.includes("document.querySelector(String.raw`<frame>`).contentDocument.querySelector(String.raw`<inner>`)"), "the skill teaches the frame's form in these words");
  return "same";
});

await test("the row of a driver says the three verdicts side by side and keeps the whole result of the page", () => {
  for (const road of ["verbs", "script"]) {
    const one = row("reveal", road);
    assert(one && typeof one.pass === "boolean", `the ${road} row says its pass`, one && Object.keys(one));
    assert(JSON.stringify(one.result) === JSON.stringify({ name: "Kim", invoice: true, company: "Acme" }), `the ${road} row keeps what the page recorded, whole`, one.result);
    assert(one.ok === true && one.wholeRaw === true && one.whole === true && one.pass === true && one.extra.length === 0, `a result that is the expected one holds under all three (${road})`, one);
  }
  const verbs = row("reveal", "verbs");
  assert(verbs.madeProblems?.length === 0, "and no declared key is wrong", verbs.madeProblems);
  return "kept";
});

await test("the row of a driver is no pass when the page recorded a key that nothing names, though the expected keys hold", () => {
  for (const road of ["verbs", "script"]) {
    const one = row("left", road);
    assert(one && typeof one.pass === "boolean", `the ${road} row says its pass`, one && Object.keys(one));
    assert(one.ok === true && one.pass === false && one.wholeRaw === false && one.whole === false, `the expected keys hold and the pass does not (${road})`, one);
    assert(JSON.stringify(one.extra) === JSON.stringify(["memo"]), `the key is said (${road})`, one.extra);
    assert(one.result && one.result.memo === "", `and the result shows it (${road})`, one.result);
  }
  return "extra";
});

await test("a declared key is held to its shape, made and shaped it passes by the declared rule alone, left empty it does not", () => {
  for (const road of ["verbs", "script"]) {
    const made = row("made", road);
    assert(made && typeof made.pass === "boolean", `the ${road} row says its pass`, made && Object.keys(made));
    assert(made.ok === true && made.wholeRaw === false && made.whole === true && made.pass === true && JSON.stringify(made.made) === JSON.stringify(["ref"]), `a made, shaped key breaks the first rule alone (${road})`, made);
    assert(/^R-\d{4}$/.test(made.result?.ref ?? ""), `and its value is in the row (${road})`, made.result);
    const empty = row("madeempty", road);
    assert(empty && empty.ok === true && empty.whole === false && empty.pass === false && JSON.stringify(empty.madeProblems) === JSON.stringify(["ref:empty"]), `an empty one is no pass, and says why (${road})`, empty);
  }
  return "shaped";
});

await test("the road of a person says the three verdicts, passes only by all of them and keeps the whole result of the page", () => {
  const lines = String(road.stdout).split("\n");
  const kept = lines.find((line) => line.includes("person-kept")) || "";
  const left = lines.find((line) => line.includes("person-left")) || "";
  assert(kept.startsWith("PASS") && kept.includes("ok=true wholeRaw=true whole=true"), "a road that records only what the card names passes, and says all three", kept);
  assert(left.startsWith("FAIL") && left.includes("ok=true wholeRaw=false whole=false") && left.includes("extra=memo"), "one that records a key nothing names does not, and says which", left);
  const rowOf = (name) => roadRows.find((one) => one.scene === name);
  assert(rowOf("person-kept")?.pass === true && JSON.stringify(rowOf("person-kept").result) === JSON.stringify({ name: "Kim" }), "its rows keep the page's result, whole", roadRows);
  assert(rowOf("person-left")?.pass === false && JSON.stringify(rowOf("person-left").result) === JSON.stringify({ name: "Kim", memo: "" }), "the failing one too", roadRows);
  return "kept";
});

void done;
let failed = 0;
for (const one of results) {
  if (!one.pass) failed += 1;
  console.log(`${one.pass ? "PASS" : "FAIL"}  ${one.name}${one.detail ? `  — ${one.detail}` : ""}`);
}
console.log(`\n${results.length - failed}/${results.length} passed`);
process.exit(failed ? 1 : 0);
