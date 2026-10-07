/* The scene kit's oracle (t-41592), held to its own rules without a browser:
 * what a scene's page records that no card asked for. A page puts every key it
 * knows into `window.__sceneResult` — the values it makes itself (a booking
 * number, a total) and the fields an agent can write that the card leaves
 * alone. A scene declares the first (`made.json`) and holds the second in its
 * expected result at the value the card left them; so the page's whole result
 * is the expected one but for the declared keys, and a key only the result
 * holds that nothing declares is an extra key — the hole of an oracle that
 * compares only the keys the card names.
 *
 *   node test-kit.mjs
 */

import { mkdtemp, mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as kit from "./scene-kit.mjs";

const results = [];
async function test(name, run) {
  try { results.push({ name, pass: true, detail: (await run()) ?? "" }); }
  catch (error) { results.push({ name, pass: false, detail: error?.stack ?? String(error) }); }
}
function assert(condition, message, detail = undefined) {
  if (!condition) throw new Error(detail === undefined ? message : `${message}: ${JSON.stringify(detail)}`);
}

const expected = { name: "Kim", memo: "", agree: true };
const result = { name: "Kim", memo: "", agree: true, reservationNo: "PG-0142" };

await test("a key the scene says its page makes is no extra key and no difference of the whole result", () => {
  assert(!kit.sameWhole(result, expected), "without a declaration the page's own key breaks the whole result", result);
  assert(kit.sameWhole(result, expected, ["reservationNo"]), "declared, it breaks nothing", result);
  assert(kit.sameAsSet({ agree: true, memo: "", name: "Kim", reservationNo: "x" }, expected, ["reservationNo"]),
    "nor does the order of the keys, once it is declared");
  assert(JSON.stringify(kit.extraKeys(result, expected)) === JSON.stringify(["reservationNo"]), "undeclared, it is an extra key", kit.extraKeys(result, expected));
  assert(kit.extraKeys(result, expected, ["reservationNo"]).length === 0, "declared, it is not", kit.extraKeys(result, expected, ["reservationNo"]));
  return "declared";
});

await test("a key only the result holds that nothing declares is an extra key and breaks the whole result", () => {
  const written = { ...result, memo: "later", note: "x" };
  assert(!kit.sameWhole(written, expected, ["reservationNo"]), "a field an agent wrote is a difference", written);
  assert(JSON.stringify(kit.extraKeys(written, expected, ["reservationNo"])) === JSON.stringify(["note"]), "the key no card or declaration names is extra", kit.extraKeys(written, expected, ["reservationNo"]));
  assert(JSON.stringify(kit.wrongKeys(written, expected)) === JSON.stringify(["memo"]), "and a card key that holds another value is wrong", kit.wrongKeys(written, expected));
  return "undeclared";
});

await test("the declared keys a result holds are said so a pass names what only the page made", () => {
  assert(typeof kit.madeKeys === "function", "the kit says the declared keys a result holds");
  assert(JSON.stringify(kit.madeKeys(result, ["reservationNo", "total"])) === JSON.stringify(["reservationNo"]), "only those the page recorded", kit.madeKeys(result, ["reservationNo", "total"]));
  assert(kit.madeKeys(null, ["total"]).length === 0, "a page that took nothing made nothing");
  return "said";
});

await test("a scene declaration is read with the scene and a scene with none declares nothing", async () => {
  const root = await mkdtemp(join(tmpdir(), "form-kit-"));
  const made = join(root, "made");
  const plain = join(root, "plain");
  for (const folder of [made, plain]) {
    await mkdir(folder, { recursive: true });
    await writeFile(join(folder, "card.json"), JSON.stringify({ task: "x", facts: [], personTurns: [] }));
    await writeFile(join(folder, "expected.json"), JSON.stringify(expected));
  }
  await writeFile(join(made, "made.json"), JSON.stringify({ keys: ["reservationNo", "total"] }));
  const declared = await kit.readScene(made);
  assert(JSON.stringify(declared.made) === JSON.stringify(["reservationNo", "total"]), "the keys the folder declares", declared.made);
  const none = await kit.readScene(plain);
  assert(Array.isArray(none.made) && none.made.length === 0, "none declared, none made", none.made);
  return "read";
});

await test("the shapes a declaration gives are read with it, and a shape that is no regular expression or is given to a key nothing declares is refused", async () => {
  const root = await mkdtemp(join(tmpdir(), "form-kit-shapes-"));
  const scene = async (name, declaration) => {
    const folder = join(root, name);
    await mkdir(folder, { recursive: true });
    await writeFile(join(folder, "card.json"), JSON.stringify({ task: "x", facts: [], personTurns: [] }));
    await writeFile(join(folder, "expected.json"), JSON.stringify(expected));
    if (declaration) await writeFile(join(folder, "made.json"), JSON.stringify(declaration));
    return folder;
  };
  const shaped = await kit.readScene(await scene("shaped", { keys: ["ref"], shapes: { ref: "^R-\\d{4}$" } }));
  assert(shaped.shapes?.ref === "^R-\\d{4}$", "the shape the folder gives the key", shaped.shapes);
  const keysOnly = await kit.readScene(await scene("keys-only", { keys: ["ref"] }));
  assert(keysOnly.shapes && Object.keys(keysOnly.shapes).length === 0, "a declaration with no shape asks only that something is there", keysOnly.shapes);
  const nothing = await kit.readScene(await scene("nothing", null));
  assert(nothing.shapes && Object.keys(nothing.shapes).length === 0, "a scene with none has none", nothing.shapes);
  const refused = (folder) => kit.readScene(folder).then(() => null, (error) => error);
  const broken = await refused(await scene("broken", { keys: ["ref"], shapes: { ref: "(" } }));
  assert(broken instanceof Error && broken.message.includes("ref"), "a shape that is no regular expression is refused, by the key", broken && broken.message);
  const stray = await refused(await scene("stray", { keys: ["ref"], shapes: { other: "x" } }));
  assert(stray instanceof Error && stray.message.includes("other"), "a shape for a key nothing declares is refused, by the key", stray && stray.message);
  return "read";
});

await test("the first rule is its own verdict, the whole result of the page as the expected one key by key and in the same order with nothing declared", () => {
  assert(typeof kit.wholeRaw === "function", "the kit says the first rule by a name of its own");
  assert(kit.wholeRaw(expected, expected), "the same result is the same", expected);
  assert(!kit.wholeRaw(result, expected), "a key the page makes is a difference of the first rule", result);
  assert(!kit.wholeRaw({ memo: "", agree: true, name: "Kim" }, expected), "so is the order of the keys");
  assert(!kit.wholeRaw(null, expected), "and a page that took nothing");
  return "kept";
});

const shapes = { reservationNo: "^PG-\\d{4}$" };
await test("a declared key is still asked for, told when it is absent or empty or not of its shape, and then the whole result of the declared rule fails", () => {
  assert(typeof kit.madeProblems === "function", "the kit says what is wrong with a declared key");
  assert(kit.madeProblems(result, ["reservationNo"], shapes).length === 0, "there, filled and of its shape", kit.madeProblems(result, ["reservationNo"], shapes));
  const said = (value, given = shapes) => JSON.stringify(kit.madeProblems(value, ["reservationNo"], given));
  assert(said(expected) === JSON.stringify(["reservationNo:absent"]), "a page that never recorded it", said(expected));
  assert(said({ ...expected, reservationNo: "" }) === JSON.stringify(["reservationNo:empty"]), "an empty text", said({ ...expected, reservationNo: "" }));
  assert(said({ ...expected, reservationNo: "  " }) === JSON.stringify(["reservationNo:empty"]), "a blank text");
  assert(said({ ...expected, reservationNo: null }) === JSON.stringify(["reservationNo:empty"]), "a null");
  assert(said({ ...expected, reservationNo: "XX-1" }) === JSON.stringify(["reservationNo:shape"]), "a text of another shape", said({ ...expected, reservationNo: "XX-1" }));
  assert(said({ ...expected, reservationNo: "XX-1" }, {}) === "[]", "a declaration with no shape asks only that something is there");
  assert(!kit.sameWhole(expected, expected, ["reservationNo"]), "a declared key the page never recorded is no whole result");
  assert(!kit.sameWhole({ ...expected, reservationNo: "" }, expected, ["reservationNo"]), "nor is an empty one");
  assert(!kit.sameWhole({ ...expected, reservationNo: "XX-1" }, expected, ["reservationNo"], shapes), "nor one of another shape");
  assert(kit.sameWhole(result, expected, ["reservationNo"], shapes), "there, filled and shaped, it is the whole result but for that key");
  return "asked";
});

await test("the verdicts are told side by side, and a pass only when the expected keys hold, nothing else was recorded and the whole result holds", () => {
  assert(typeof kit.verdicts === "function", "the kit says every verdict of one result");
  const plain = kit.verdicts(expected, expected);
  assert(plain.ok && plain.wholeRaw && plain.whole && plain.sameSet && plain.pass && plain.extra.length === 0 && plain.made.length === 0 && plain.madeProblems.length === 0, "a result that is the expected one holds under all", plain);
  const made = kit.verdicts(result, expected, ["reservationNo"], shapes);
  assert(made.ok && !made.wholeRaw && made.whole && made.pass && made.extra.length === 0 && JSON.stringify(made.made) === JSON.stringify(["reservationNo"]),
    "a declared key breaks the first rule alone — and the pass is the declared rule's", made);
  const extra = kit.verdicts({ ...expected, note: "x" }, expected);
  assert(extra.ok && !extra.wholeRaw && !extra.whole && !extra.pass && JSON.stringify(extra.extra) === JSON.stringify(["note"]), "a key nothing names leaves the expected keys holding and is no pass", extra);
  const wrong = kit.verdicts({ ...expected, name: "Lee" }, expected);
  assert(!wrong.ok && !wrong.pass && JSON.stringify(wrong.wrong) === JSON.stringify(["name"]), "a card key that holds another value is no pass", wrong);
  const none = kit.verdicts(null, expected);
  assert(!none.tookResult && !none.ok && !none.pass, "a page that took nothing is no pass", none);
  const empty = kit.verdicts({ ...expected, reservationNo: "" }, expected, ["reservationNo"], shapes);
  assert(empty.ok && !empty.whole && !empty.pass && JSON.stringify(empty.madeProblems) === JSON.stringify(["reservationNo:empty"]), "a declared key left empty is no pass", empty);
  return "side by side";
});

await test("the words of the verdicts are one line every runner prints", () => {
  assert(typeof kit.verdictWords === "function", "the kit says the verdicts in words");
  assert(kit.verdictWords(kit.verdicts(result, expected, ["reservationNo"], shapes)) === "ok=true wholeRaw=false whole=true sameSet=true extra=- made=reservationNo",
    "a declared key made by the page", kit.verdictWords(kit.verdicts(result, expected, ["reservationNo"], shapes)));
  assert(kit.verdictWords(kit.verdicts({ ...expected, note: "x" }, expected)) === "ok=true wholeRaw=false whole=false sameSet=false extra=note made=-",
    "a key nothing names", kit.verdictWords(kit.verdicts({ ...expected, note: "x" }, expected)));
  assert(kit.verdictWords(kit.verdicts({ ...expected, reservationNo: "" }, expected, ["reservationNo"], shapes)) === "ok=true wholeRaw=false whole=false sameSet=false extra=- made=reservationNo madeProblems=reservationNo:empty",
    "a declared key left empty", kit.verdictWords(kit.verdicts({ ...expected, reservationNo: "" }, expected, ["reservationNo"], shapes)));
  return "one line";
});

let failed = 0;
for (const one of results) {
  if (!one.pass) failed += 1;
  console.log(`${one.pass ? "PASS" : "FAIL"}  ${one.name}${one.detail ? `  — ${one.detail}` : ""}`);
}
console.log(`\n${results.length - failed}/${results.length} passed`);
process.exit(failed ? 1 : 0);
