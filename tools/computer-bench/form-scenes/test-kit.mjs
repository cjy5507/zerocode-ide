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

let failed = 0;
for (const one of results) {
  if (!one.pass) failed += 1;
  console.log(`${one.pass ? "PASS" : "FAIL"}  ${one.name}${one.detail ? `  — ${one.detail}` : ""}`);
}
console.log(`\n${results.length - failed}/${results.length} passed`);
process.exit(failed ? 1 : 0);
