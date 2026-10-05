/* The scripted drivers (drive.mjs) held to the rules a model takes from the door's
 * words and the skill (t-41592), on two small scenes made here — neither a scene
 * of the bench: a page whose fill brings a field of its own ("양식 바뀜") is read
 * again before anything is pressed, and a field the page keeps off until its terms
 * are read to the end is switched on by scrolling that box the way the skill
 * teaches — then the step goes on. Both roads of the driver, the verbs and the
 * script, are held to both.
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
async function scene(name, html, facts, expected) {
  const folder = join(root, name);
  await mkdir(folder, { recursive: true });
  await writeFile(join(folder, "scene.html"), html);
  await writeFile(join(folder, "card.json"), JSON.stringify({ task: "Fill it in.", facts, personTurns: [] }));
  await writeFile(join(folder, "expected.json"), JSON.stringify(expected));
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

/* One run of the drivers on both scenes, once. */
const out = join(root, "rows.json");
const done = spawnSync(process.execPath, [join(HERE, "drive.mjs"), "--scene", REVEAL, "--scene", TERMS, "--out", out], { encoding: "utf8", timeout: 240_000 });
const rows = await readFile(out, "utf8").then(JSON.parse, () => []);
const row = (scene, road) => rows.find((one) => one.scene === scene && one.road === road);

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

await test("the recipe a driver scrolls a box by is the one the skill teaches", () => {
  assert(SKILL.includes("document.querySelector(String.raw`<handle>`).scrollTop = 1e9"), "the skill teaches the recipe in these words");
  assert(scrollToEnd("#terms") === "document.querySelector(String.raw`#terms`).scrollTop = 1e9", "a handle is read by the recipe as the skill says", scrollToEnd("#terms"));
  assert(scrollToEnd("#card >> #box") === "document.querySelector(String.raw`#card`).contentDocument.querySelector(String.raw`#box`).scrollTop = 1e9",
    "and a handle with the frame's separator is the frame and the box inside it", scrollToEnd("#card >> #box"));
  assert(SKILL.includes("document.querySelector(String.raw`<frame>`).contentDocument.querySelector(String.raw`<inner>`)"), "the skill teaches the frame's form in these words");
  return "same";
});

void done;
let failed = 0;
for (const one of results) {
  if (!one.pass) failed += 1;
  console.log(`${one.pass ? "PASS" : "FAIL"}  ${one.name}${one.detail ? `  — ${one.detail}` : ""}`);
}
console.log(`\n${results.length - failed}/${results.length} passed`);
process.exit(failed ? 1 : 0);
