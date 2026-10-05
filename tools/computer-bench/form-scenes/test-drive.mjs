/* The scripted drivers (drive.mjs) held to the rules a model takes from the door's
 * words and the skill (t-41592), on two small scenes made here — neither a scene
 * of the bench: a page whose fill brings a field of its own ("양식 바뀜") is read
 * again before anything is pressed, and a field the page keeps off until its terms
 * are read to the end is switched on by scrolling that box the way the skill
 * teaches — then the step goes on. The button that moves a step on is chosen by what
 * the door said of the buttons (t-41656) — a page's own submit, the one the fill turned on, the
 * page's last — and waited for, a fixed number of times, while it is off and the door has said
 * new text; a group of chips is written with fill. A value the page splits across fields is cut at the symbols
 * the door says stand between the parts, a date and a time go to parts the page names by units, a fact that names
 * two fields is told by the kind of field its value fits (and a card that does not tell is stopped on), a field the
 * door says has new text after a press is given its own button once, and a field whose read-back the door cannot tell
 * is not written again (t-41720). Both roads of the driver, the verbs and the
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

/* A form with a required field the card has nothing for, and a button the page lets through anyway: what is left is the page's, and the script road's answer
 * carries the same buttons twice (the read's and the fill's) — which a page's answer says only once. */
const LEFT_OVER = await scene("left-over", `<!doctype html><html lang="en"><meta charset="utf-8"><form onsubmit="return false">
  <label for="name">Name</label> <input id="name">
  <label for="nick">Nickname</label> <input id="nick" required>
  <button type="button" id="go">Finish</button>
</form>
<script>
  document.getElementById("go").addEventListener("click", () => { window.__sceneResult = { name: document.getElementById("name").value }; });
</script>`, [{ says: "Name", value: "Kim" }], { name: "Kim" });

/* A value the page splits across fields (t-41720): the door says the symbol the page draws between the parts, and the driver cuts the value
 * there — an address into a name, a symbol, a domain text and a list of domains (the list takes the rest when it has it, and the page writes
 * the text), and a serial of letters and numbers into three boxes. */
const JOINTS = await scene("joints", `<!doctype html><html lang="en"><meta charset="utf-8"><form onsubmit="return false">
  <div class="row"><span>Contact</span> <input id="who" size="12"><span class="sep">@</span><input id="site" size="12" placeholder="Own domain">
    <select id="pick"><option value="">Own domain</option><option value="mail.test">mail.test</option><option value="post.test">post.test</option></select></div>
  <div class="row"><span>Serial</span> <input id="s1" size="2"> - <input id="s2" size="2"> - <input id="s3" size="2"></div>
  <button type="button" id="go">Send</button>
</form>
<script>
  const $ = (id) => document.getElementById(id);
  $("pick").addEventListener("change", () => { $("site").value = $("pick").value; $("site").readOnly = Boolean($("pick").value); });
  $("go").addEventListener("click", () => {
    window.__sceneResult = { contact: $("who").value + "@" + $("site").value, serial: [$("s1").value, $("s2").value, $("s3").value].join("-") };
  });
</script>`, [{ says: "Contact", value: "ann@mail.test" }, { says: "Serial", value: "AB-12-CD" }],
{ contact: "ann@mail.test", serial: "AB-12-CD" });

/* The parts a caption names — each told by its own short words, `Window — From` — are a split value too: the driver cuts the value at the symbol the door says stands
 * between them. */
const JOINTS_NAMED = await scene("joints-named", `<!doctype html><html lang="en"><meta charset="utf-8"><form onsubmit="return false">
  <fieldset><legend>Window</legend><input id="w1" aria-label="From" size="4"> ~ <input id="w2" aria-label="To" size="4"></fieldset>
  <button type="button" id="go">Send</button>
</form>
<script>
  const $ = (id) => document.getElementById(id);
  $("go").addEventListener("click", () => { window.__sceneResult = { window: $("w1").value + "~" + $("w2").value }; });
</script>`, [{ says: "Window", value: "08~17" }], { window: "08~17" });

/* A date and a time written into parts the page names by units — in English and in Korean, with a half of the day to press. */
const numbers = (from, to, word = "") => Array.from({ length: to - from + 1 }, (_, at) => `<option>${from + at}${word}</option>`).join("");
const UNITS = await scene("units", `<!doctype html><html lang="en"><meta charset="utf-8"><form onsubmit="return false">
  <div class="row"><span>Born</span> <select id="by" aria-label="Year"><option value="">-</option><option>1985</option><option>1987</option><option>1989</option></select>
    <select id="bm" aria-label="Month"><option value="">-</option>${numbers(1, 12)}</select><select id="bd" aria-label="Day"><option value="">-</option>${numbers(1, 31)}</select></div>
  <div class="row"><span>Alarm</span> <div id="ap" role="radiogroup" aria-label="AM or PM"><button type="button" role="radio" aria-checked="false">AM</button><button type="button" role="radio" aria-checked="false">PM</button></div>
    <select id="ah" aria-label="Hour"><option value="">-</option>${numbers(1, 12)}</select><select id="am" aria-label="Minute"><option value="">-</option><option>00</option><option>05</option><option>30</option></select></div>
  <div class="row"><span>기상</span> <select id="kh" aria-label="시"><option value="">-</option>${numbers(5, 7, "시")}</select><select id="km" aria-label="분"><option value="">-</option><option>00분</option><option>30분</option></select></div>
  <button type="button" id="go">Send</button>
</form>
<script>
  const $ = (id) => document.getElementById(id);
  for (const button of document.querySelectorAll("#ap button")) button.addEventListener("click", () => {
    for (const other of document.querySelectorAll("#ap button")) other.setAttribute("aria-checked", String(other === button));
  });
  $("go").addEventListener("click", () => {
    const on = document.querySelector('#ap [aria-checked="true"]');
    window.__sceneResult = { born: [$("by").value, $("bm").value, $("bd").value].join("-"), alarm: (on ? on.textContent : "") + " " + $("ah").value + ":" + $("am").value,
      wake: $("kh").value + " " + $("km").value };
  });
</script>`, [{ says: "Born", value: "1987-03-09" }, { says: "Alarm", value: "7:05 PM" }, { says: "기상", value: "06:30" }],
{ born: "1987-3-9", alarm: "PM 7:05", wake: "6시 30분" });

/* A fact that names two fields — an item's name is in the words of its checkbox and of its count: the value is a number, so it goes to the field that takes
 * one — and one that names two fields of one kind, which the card does not tell apart: the driver stops and says which fields. */
const ITEM = (name, fields) => `<li><p class="name">${name}</p>${fields}</li>`;
const COUNT = (name) => `<input type="checkbox" checked aria-label="${name} select"><div class="step"><button type="button" aria-label="Fewer">-</button><input class="n" aria-label="Count" value="1"><button type="button" aria-label="More">+</button></div>`;
const TWINS = await scene("twins", `<!doctype html><html lang="en"><meta charset="utf-8"><form onsubmit="return false">
  <ul id="list">${ITEM("Oat bar pack", COUNT("Oat bar pack"))}${ITEM("Tea tin", COUNT("Tea tin"))}</ul>
  <button type="button" id="go">Order</button>
</form>
<script>
  document.getElementById("go").addEventListener("click", () => {
    const counts = [...document.querySelectorAll("input.n")].map((input) => input.value);
    window.__sceneResult = { oat: counts[0], tea: counts[1] };
  });
</script>`, [{ says: "Oat bar pack", value: "3" }, { says: "Tea tin", value: "2" }], { oat: "3", tea: "2" });
const LINES = (name) => `<input aria-label="First line"><input aria-label="Second line">`;
const TWINS_OPEN = await scene("twins-open", `<!doctype html><html lang="en"><meta charset="utf-8"><form onsubmit="return false">
  <ul id="list">${ITEM("Pen box", LINES())}${ITEM("Ink bottle", LINES())}</ul>
  <button type="button" id="go">Order</button>
</form>
<script>
  document.getElementById("go").addEventListener("click", () => { window.__sceneResult = { lines: document.querySelectorAll("input").length }; });
</script>`, [{ says: "Pen box", value: "2" }], { lines: 4 });

/* A button that a field's own value turns on — the "Fewer" beside a count that was 1 and is now 3 — is that field's, not the button that moves the step on: the door says
 * whose it is (`beside`), and the driver does not take it for the button the fill turned on. */
const STEP_ITEM = (name) => `<li><p class="name">${name}</p><div class="step"><button type="button" class="less" aria-label="Fewer" disabled>-</button><input class="n" aria-label="Count" value="1"><button type="button" class="more" aria-label="More">+</button></div></li>`;
const STEPPER = await scene("stepper", `<!doctype html><html lang="en"><meta charset="utf-8"><form onsubmit="return false">
  <ul id="list">${STEP_ITEM("Ink bottle")}${STEP_ITEM("Pen box")}</ul>
  <button type="button" id="go">Order</button>
</form>
<script>
  const rows = [...document.querySelectorAll("#list li")];
  const show = (row) => { row.querySelector(".less").disabled = Number(row.querySelector(".n").value) <= 1; };
  for (const row of rows) {
    row.querySelector(".n").addEventListener("input", () => show(row));
    row.querySelector(".less").addEventListener("click", () => { row.querySelector(".n").value = Number(row.querySelector(".n").value) - 1; show(row); });
    row.querySelector(".more").addEventListener("click", () => { row.querySelector(".n").value = Number(row.querySelector(".n").value) + 1; show(row); });
  }
  document.getElementById("go").addEventListener("click", () => {
    window.__sceneResult = { ink: rows[0].querySelector(".n").value, pen: rows[1].querySelector(".n").value };
  });
</script>`, [{ says: "Ink bottle: Count", value: "3" }, { says: "Pen box: Count", value: "2" }], { ink: "3", pen: "2" });

/* The code that goes to the person's phone (t-41720): the button that sends it is the one the door says stands beside the phone field the driver wrote — a field of
 * kind `tel` — and the button that confirms it is the one beside the field the code was written into; neither is found by a list of words. A page whose buttons say
 * no place is held to the few short words that are left. */
const CODE_CARD = (facts) => ({ "card.json": JSON.stringify({ task: "Fill it in.", facts, personTurns: ["code"] }) });
const SEND_BESIDE_FACTS = [{ says: "Mobile", value: "031-7788-2290" }];
const SEND_BESIDE = await scene("send-beside", `<!doctype html><html lang="en"><meta charset="utf-8"><form onsubmit="return false">
  <div class="row"><label for="ph">Mobile</label><div class="line"><input id="ph" type="tel"><button type="button" id="text">Text me</button></div></div>
  <div class="row" id="cr" hidden><label for="cd">Number we texted</label><div class="line"><input id="cd"></div></div>
  <button type="button" id="finish">Finish</button>
</form>
<script>
  const $ = (id) => document.getElementById(id);
  let sent = "";
  $("text").addEventListener("click", () => { sent = "483920"; window.__personPhone = sent; $("cr").hidden = false; });
  $("finish").addEventListener("click", () => {
    if (!sent || $("cd").value !== sent) return;
    window.__sceneResult = { mobile: $("ph").value, code: $("cd").value };
  });
</script>`, SEND_BESIDE_FACTS, { mobile: "031-7788-2290", code: "483920" }, CODE_CARD(SEND_BESIDE_FACTS));
const CONFIRM_BESIDE = await scene("confirm-beside", `<!doctype html><html lang="en"><meta charset="utf-8"><form onsubmit="return false">
  <div class="row"><label for="ph">Mobile</label> <input id="ph" type="tel"> <button type="button" id="send">Send code</button></div>
  <div class="row" id="cr" hidden><label for="cd">Number we texted</label><div class="line"><input id="cd"><button type="button" id="check">Check</button></div></div>
  <p id="top"></p>
  <button type="button" id="finish">Finish</button>
</form>
<script>
  const $ = (id) => document.getElementById(id);
  let sent = "", ok = false;
  $("send").addEventListener("click", () => { sent = "483920"; window.__personPhone = sent; $("cr").hidden = false; });
  $("check").addEventListener("click", () => { ok = $("cd").value === sent; });
  $("finish").addEventListener("click", () => {
    if (!ok) { $("top").textContent = "Check the number first"; return; }
    window.__sceneResult = { mobile: $("ph").value, code: $("cd").value, verified: ok };
  });
</script>`, SEND_BESIDE_FACTS, { mobile: "031-7788-2290", code: "483920", verified: true }, CODE_CARD(SEND_BESIDE_FACTS));
const CODE_WORDS = await scene("code-words", `<!doctype html><html lang="en"><meta charset="utf-8"><form onsubmit="return false">
  <div class="row"><label for="ph">Mobile</label> <input id="ph"> <button type="button" id="send">Send code</button></div>
  <div class="row" id="cr" hidden><label for="cd">Code</label><input id="cd"><p>Enter the number</p><button type="button" id="v">Verify</button></div>
  <p id="top"></p>
  <button type="button" id="finish">Finish</button>
</form>
<script>
  const $ = (id) => document.getElementById(id);
  let sent = "", ok = false;
  $("send").addEventListener("click", () => { sent = "483920"; window.__personPhone = sent; $("cr").hidden = false; });
  $("v").addEventListener("click", () => { ok = $("cd").value === sent; });
  $("finish").addEventListener("click", () => {
    if (!ok) { $("top").textContent = "Verify the number first"; return; }
    window.__sceneResult = { mobile: $("ph").value, code: $("cd").value, verified: ok };
  });
</script>`, SEND_BESIDE_FACTS, { mobile: "031-7788-2290", code: "483920", verified: true }, CODE_CARD(SEND_BESIDE_FACTS));

/* A field the page answers about after a press, with a button of its own beside it: the door says whose the button is, and the driver presses it once and
 * reads the form again — the press that ends the form is made again after it. */
const OWN = await scene("own", `<!doctype html><html lang="en"><meta charset="utf-8"><form id="f" onsubmit="return false">
  <div class="row"><label for="who">Name</label> <input id="who"></div>
  <div class="row"><label for="tag">Tag</label><div class="line"><input id="tag"><button type="button" id="apply">Apply</button></div><p id="note"></p></div>
  <button type="submit" id="finish">Finish</button>
</form>
<script>
  const $ = (id) => document.getElementById(id);
  let applied = "";
  $("tag").addEventListener("input", () => { applied = ""; $("note").textContent = ""; });
  $("apply").addEventListener("click", () => { applied = $("tag").value; $("note").textContent = applied ? "Tag applied" : ""; });
  $("f").addEventListener("submit", (event) => {
    event.preventDefault();
    if ($("tag").value && applied !== $("tag").value) { $("note").textContent = "Apply the tag or clear it."; return; }
    window.__sceneResult = { who: $("who").value, tag: $("tag").value, applied };
  });
</script>`, [{ says: "Name", value: "Kim" }, { says: "Tag", value: "NEW5" }], { who: "Kim", tag: "NEW5", applied: "NEW5" });

/* A field whose read-back the door cannot tell — a date the page draws without its year: the driver goes on to the button, and does not write it again. */
const DRAWN = await scene("drawn", `<!doctype html><html lang="en"><meta charset="utf-8"><form onsubmit="return false">
  <label for="day">Arrival</label> <input id="day"><button type="button" id="go">Send</button>
</form>
<script>
  const $ = (id) => document.getElementById(id);
  let kept = "";
  $("day").addEventListener("change", () => {
    const parts = $("day").value.match(/^(\\d{4})-(\\d\\d)-(\\d\\d)$/);
    if (parts) { kept = $("day").value; $("day").value = parts[2] + "/" + parts[3]; }
  });
  $("go").addEventListener("click", () => { window.__sceneResult = { arrival: kept }; });
</script>`, [{ says: "Arrival", value: "2027-02-17" }], { arrival: "2027-02-17" });

/* One run of the drivers on every scene, once. */
const out = join(root, "rows.json");
const done = spawnSync(process.execPath, [join(HERE, "drive.mjs"), "--scene", REVEAL, "--scene", TERMS, "--scene", LEFT, "--scene", MADE, "--scene", MADE_EMPTY,
  "--scene", ADVANCE, "--scene", ADVANCE_SUBMIT, "--scene", LOADING, "--scene", CHIPS, "--scene", SLOTS, "--scene", LATE, "--scene", REFUSES, "--scene", MASKED, "--scene", LEFT_OVER,
  "--scene", JOINTS, "--scene", JOINTS_NAMED, "--scene", STEPPER, "--scene", SEND_BESIDE, "--scene", CONFIRM_BESIDE, "--scene", CODE_WORDS, "--scene", UNITS, "--scene", TWINS, "--scene", TWINS_OPEN, "--scene", OWN, "--scene", DRAWN, "--out", out], { encoding: "utf8", timeout: 720_000 });
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

await test("a driver goes on to the button when what is left belongs to the page, though its answer holds the buttons only once", () => {
  for (const road of ["verbs", "script"]) {
    const one = row("left-over", road);
    assert(one && one.ok && one.pass, `the ${road} road presses the button the page lets through`, one && { ok: one.ok, stuck: one.stuck, result: one.result });
  }
  return JSON.stringify(row("left-over", "script").result);
});

await test("a driver cuts a value the page splits across fields at the symbols the door says stand between the parts and gives the rest to the list that has it", () => {
  for (const road of ["verbs", "script"]) {
    const one = row("joints", road);
    assert(one && one.ok && one.pass, `the ${road} road gives each part its piece of the address and of the serial`, one && { ok: one.ok, stuck: one.stuck, result: one.result });
  }
  return JSON.stringify(row("joints", "verbs").result);
});

await test("a driver cuts a value across the parts a caption names at the symbol the door says stands between them", () => {
  for (const road of ["verbs", "script"]) {
    const one = row("joints-named", road);
    assert(one && one.ok && one.pass, `the ${road} road gives each part named by the caption its piece`, one && { ok: one.ok, stuck: one.stuck, result: one.result });
  }
  return JSON.stringify(row("joints-named", "verbs").result);
});

await test("a driver writes a date and a time into the parts the page names by units, in English and in Korean, and presses the half of the day", () => {
  for (const road of ["verbs", "script"]) {
    const one = row("units", road);
    assert(one && one.ok && one.pass, `the ${road} road gives the year, the month and the day, the half of the day, the hour and the minute their numbers`, one && { ok: one.ok, stuck: one.stuck, result: one.result });
  }
  return JSON.stringify(row("units", "verbs").result);
});

await test("a driver gives a number to the field that takes numbers when the name of an item is in the words of two fields", () => {
  for (const road of ["verbs", "script"]) {
    const one = row("twins", road);
    assert(one && one.ok && one.pass, `the ${road} road writes the counts and leaves the checkboxes`, one && { ok: one.ok, stuck: one.stuck, result: one.result });
  }
  return JSON.stringify(row("twins", "verbs").result);
});

await test("a driver that finds one item answered to by two fields of one kind stops at once and says which fields", () => {
  for (const road of ["verbs", "script"]) {
    const one = row("twins-open", road);
    assert(one && typeof one.stuck === "string" && /"Pen box" answers to 2 fields \(/.test(one.stuck) && /does not say which/.test(one.stuck), `the ${road} road says the card does not tell`, one && { stuck: one.stuck });
    assert(one.roundTrips <= 2, `and stops before it presses anything (${road})`, { roundTrips: one.roundTrips });
  }
  return row("twins-open", "verbs").stuck;
});

await test("a driver sends the code of the person by the button the door says stands beside the phone field it wrote whatever that button says", () => {
  for (const road of ["verbs", "script"]) {
    const one = row("send-beside", road);
    assert(one && one.ok && one.pass, `the ${road} road presses the button beside the phone field and gets the code`, one && { ok: one.ok, stuck: one.stuck, result: one.result });
    assert(one.handoff === 1, `and asks the person for the code once (${road})`, { handoff: one.handoff });
  }
  return JSON.stringify(row("send-beside", "verbs").result);
});

await test("a driver confirms the code of the person by the button the door says stands beside the field the code went into whatever that button says", () => {
  for (const road of ["verbs", "script"]) {
    const one = row("confirm-beside", road);
    assert(one && one.ok && one.pass, `the ${road} road presses the button beside the code field before it finishes`, one && { ok: one.ok, stuck: one.stuck, result: one.result });
  }
  return JSON.stringify(row("confirm-beside", "verbs").result);
});

await test("a driver whose page says no place for the buttons of the code of the person goes by the few short words that are left", () => {
  for (const road of ["verbs", "script"]) {
    const one = row("code-words", road);
    assert(one && one.ok && one.pass, `the ${road} road sends and confirms the code by the short words`, one && { ok: one.ok, stuck: one.stuck, result: one.result });
  }
  return JSON.stringify(row("code-words", "verbs").result);
});

await test("a driver does not take a button its own field turned on for the button that moves the step", () => {
  for (const road of ["verbs", "script"]) {
    const one = row("stepper", road);
    assert(one && one.ok && one.pass, `the ${road} road leaves the counts it wrote and presses the order button`, one && { ok: one.ok, stuck: one.stuck, result: one.result });
  }
  return JSON.stringify(row("stepper", "verbs").result);
});

await test("a driver presses the one button beside a field the door says has new text after a press, once, and then makes the press again", () => {
  for (const road of ["verbs", "script"]) {
    const one = row("own", road);
    assert(one && one.ok && one.pass, `the ${road} road applies the tag the page asked to be applied and then finishes`, one && { ok: one.ok, stuck: one.stuck, result: one.result, trail: one.trail });
    assert(one.trail.some((entry) => entry.own === "Apply"), `by the button the door said stands beside the field (${road})`, one.trail);
  }
  return JSON.stringify(row("own", "verbs").result);
});

await test("a driver does not write again a field whose read-back the door says it cannot tell, and goes on to the button", () => {
  for (const road of ["verbs", "script"]) {
    const one = row("drawn", road);
    assert(one && one.ok && one.pass, `the ${road} road gets the date through a page that draws it without its year`, one && { ok: one.ok, stuck: one.stuck, result: one.result });
  }
  assert(row("drawn", "verbs").fill === 1, "the verbs road wrote it once", { fill: row("drawn", "verbs").fill });
  assert(row("drawn", "script").eval === 1, "and the script road in one step", { eval: row("drawn", "script").eval });
  return `fill ${row("drawn", "verbs").fill}, eval ${row("drawn", "script").eval}`;
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
