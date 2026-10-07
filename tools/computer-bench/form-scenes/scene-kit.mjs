/* What every runner of a form scene shares (t-37883, t-41387): a scene's
 * files, the loopback site that serves them and the oracle that says whether
 * the page took what was asked — so the scripted drivers (drive.mjs), the
 * stand-in window for a real model (door-desk.mjs) and a person's own check
 * all judge one scene one way.
 *
 * A scene folder holds `scene.html` (and what it loads, same origin),
 * `card.json` (what the person gives the agent) and `expected.json` (what the
 * page must take). The page sets `window.__sceneResult` when it takes the
 * booking and `window.__personPhone` when a code goes to the person's phone.
 */

import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { extname, join, resolve, sep } from "node:path";

const TYPES = { ".html": "text/html; charset=utf-8", ".js": "text/javascript", ".css": "text/css", ".json": "application/json" };

/* A scene's card and expected result, the keys its page makes itself, and the
 * name its folder gives it. `made.json` is the scene's own declaration —
 * `{"keys": [...], "shapes": {key: "regular expression"}}`: what the page puts
 * into its result that differs from one run to the next and that no field an
 * agent can write holds, so the oracle can tell such a key from a field the
 * card left alone. A value the input fixes (a total the page adds from the
 * prices, a number it makes of the date and the phone) is no such key: it goes
 * into `expected.json` at its value. A declared key is still asked for — there,
 * filled, and of its shape when the scene gives one. A scene with none declares
 * nothing. */
export async function readScene(folder) {
  const root = resolve(folder);
  const card = JSON.parse(await readFile(join(root, "card.json"), "utf8"));
  const expected = JSON.parse(await readFile(join(root, "expected.json"), "utf8"));
  const declared = await readFile(join(root, "made.json"), "utf8").then(JSON.parse, (error) => {
    if (error.code === "ENOENT") return {};
    throw error;
  });
  const made = Array.isArray(declared.keys) ? declared.keys : [];
  const shapes = declared.shapes && typeof declared.shapes === "object" ? declared.shapes : {};
  for (const [key, source] of Object.entries(shapes)) {
    if (!made.includes(key)) throw new Error(`made.json: a shape for ${key}, which it does not declare`);
    try { new RegExp(source); } catch { throw new Error(`made.json: the shape of ${key} is not a regular expression: ${source}`); }
  }
  return { folder: root, name: root.split(sep).filter(Boolean).pop(), card, expected, made, shapes };
}

/* Serve one folder over loopback, nothing outside it. */
export async function serveScene(folder) {
  const root = resolve(folder);
  const server = createServer(async (request, response) => {
    let pathname;
    try {
      pathname = decodeURIComponent(new URL(request.url, "http://x").pathname);
    } catch {
      return response.writeHead(400).end();
    }
    const path = resolve(root, "." + pathname);
    if (path !== root && !path.startsWith(root + sep)) return response.writeHead(403).end();
    // The folder itself is its scene; a file is told by its own name.
    const file = path.endsWith(sep) || path === root ? join(path, "scene.html") : path;
    try {
      response.writeHead(200, { "content-type": TYPES[extname(file)] || "application/octet-stream" }).end(await readFile(file));
    } catch {
      response.writeHead(404).end();
    }
  });
  await new Promise((done) => server.listen(0, "127.0.0.1", done));
  const port = server.address().port;
  return { port, url: `http://127.0.0.1:${port}/scene.html`, close: () => server.close() };
}

/* The third verdict (`ok`): the keys of `expected` the page's result does not
 * hold, by JSON equality — a result the page never set holds none of them. It
 * looks at no other key of the result; `extraKeys` says those, and a pass
 * counts them. */
export const wrongKeys = (result, expected) =>
  Object.keys(expected).filter((key) => !result || JSON.stringify(result[key]) !== JSON.stringify(expected[key]));

/* The first rule of success, as it was and under a name of its own: the page's
 * whole result is the expected one, key by key and in the same order — nothing
 * declared, nothing excused. */
export const wholeRaw = (result, expected) => Boolean(result) && JSON.stringify(result) === JSON.stringify(expected);

/* The second rule: the page's whole result is the expected one but for the keys
 * the scene declares its page makes itself (`made`, from `made.json`), which
 * belong to neither side — and each of them is there, filled and of its shape
 * (`madeProblems`). `extraKeys` says what only the page recorded: a key no card
 * names and no declaration excuses is a field an agent could have written, and
 * counts against the pass. */
const without = (value, made) => Object.fromEntries(Object.entries(value || {}).filter(([key]) => !made.includes(key)));
export const sameWhole = (result, expected, made = [], shapes = {}) =>
  Boolean(result) && madeProblems(result, made, shapes).length === 0 && JSON.stringify(without(result, made)) === JSON.stringify(without(expected, made));
const sortedByKey = (value) => JSON.stringify(Object.fromEntries(Object.entries(value || {}).sort(([a], [b]) => (a < b ? -1 : 1))));
/* The same, without the order of the keys — and with the declared keys asked for as well. */
export const sameAsSet = (result, expected, made = [], shapes = {}) =>
  Boolean(result) && madeProblems(result, made, shapes).length === 0 && sortedByKey(without(result, made)) === sortedByKey(without(expected, made));
/* The keys the page recorded that the expected result does not hold and the
 * scene does not declare. */
export const extraKeys = (result, expected, made = []) =>
  Object.keys(result || {}).filter((key) => !(key in expected) && !made.includes(key));
/* The declared keys the page did record — said beside a pass. */
export const madeKeys = (result, made = []) => made.filter((key) => key in (result || {}));
/* What is wrong with the declared keys: a key the page never recorded, one left
 * empty, one that is not of the shape its scene gives (`key:absent|empty|shape`). */
export function madeProblems(result, made = [], shapes = {}) {
  const problems = [];
  for (const key of made) {
    if (!result || !(key in result)) { problems.push(`${key}:absent`); continue; }
    const value = result[key];
    const empty = value === null || value === undefined || (typeof value === "string" && value.trim() === "")
      || (typeof value === "object" && Object.keys(value).length === 0);
    if (empty) problems.push(`${key}:empty`);
    else if (key in shapes && !new RegExp(shapes[key]).test(String(value))) problems.push(`${key}:shape`);
  }
  return problems;
}

/* One result under every rule, side by side: the first rule (`wholeRaw`), the
 * whole result but for the declared keys (`whole`), the expected keys alone
 * (`ok`) — with the keys only the page recorded and the declared ones it made.
 * A pass is told only when the expected keys hold, nothing else was recorded
 * and the whole result holds. */
export function verdicts(result, expected, made = [], shapes = {}) {
  const wrong = wrongKeys(result, expected);
  const extra = extraKeys(result, expected, made);
  const ok = Boolean(result) && wrong.length === 0;
  const whole = sameWhole(result, expected, made, shapes);
  return {
    tookResult: Boolean(result), ok, wholeRaw: wholeRaw(result, expected), whole, sameSet: sameAsSet(result, expected, made, shapes),
    extra, made: madeKeys(result, made), madeProblems: madeProblems(result, made, shapes), wrong,
    pass: ok && extra.length === 0 && whole,
  };
}
/* The verdicts in words — one line every runner prints. */
export const verdictWords = (v) =>
  `ok=${v.ok} wholeRaw=${v.wholeRaw} whole=${v.whole} sameSet=${v.sameSet} extra=${v.extra.join("|") || "-"} made=${v.made.join("|") || "-"}${v.madeProblems.length ? ` madeProblems=${v.madeProblems.join("|")}` : ""}`;

/* ---- what a row keeps of a result that holds a secret (t-41720, 2026-10-06) ----
 * The words of a card that say a fact is a secret — a password, a PIN, a payment card's number, security code or expiry, a one-time code: the kinds
 * the door keeps out of its answers. A rule of the bench for the records the bench keeps, beside what a run typed as a secret; not the door's rule.
 * The card's holder, its type, a card's name are no secret: the words are the number's, not the card's (seen on B at 5a478cc54, where the holder's name was kept as a digest). */
export const SECRET_SAYS = /password|passcode|비밀\s*번호|\bpin\b|card\s*(?:number|no\b)|카드\s*번호|\bcv[cv]2?\b|security\s*code|보안\s*코드|expir|valid\s*thru|유효\s*기간|one[\s-]?time|\botp\b|인증\s*번호|verification\s*code/i;
export const secretFacts = (card) => (card?.facts || []).filter((fact) => SECRET_SAYS.test(String(fact?.says ?? ""))).map((fact) => String(fact?.value ?? ""));
const digitsOf = (text) => String(text).replace(/\D/g, "");
/* Whether a value of the page's result is made of secrets: equal to one (of three characters or more), or its digits tiled left to right by the digits of
 * secrets, each used once — a card number the page built from its four boxes, an expiry from its month and year. A number that only shares a part with a
 * secret is not, and a lone value of two characters (a month, a quantity) is one only beside another. */
export function madeOfSecrets(value, secrets) {
  const text = String(value ?? "");
  if (!text) return false;
  const kept = secrets.map((one) => String(one ?? "")).filter(Boolean);
  if (text.length >= 3 && kept.includes(text)) return true;
  const digits = digitsOf(text);
  if (digits.length < 2) return false;
  const left = kept.map(digitsOf).filter((one) => one.length >= 2);
  let at = 0, used = 0;
  while (at < digits.length) {
    const found = left.filter((one) => digits.startsWith(one, at)).sort((a, b) => b.length - a.length)[0];
    if (!found) return false;
    left.splice(left.indexOf(found), 1);
    at += found.length;
    used += 1;
  }
  return used >= 2 || digits.length >= 3;
}
/* The result a row keeps: the page's whole result, but a value made of secrets kept as its sha256 (the first sixteen hex) and whether it matched what was
 * expected — so a verdict can be counted again later, by the digest, with no secret in the record. */
export function keptResult(result, expected, secrets) {
  if (!result || typeof result !== "object") return { result, secretKeys: {} };
  const out = Array.isArray(result) ? [...result] : { ...result };
  const secretKeys = {};
  for (const [key, value] of Object.entries(result)) {
    if (!madeOfSecrets(value, secrets)) continue;
    const text = String(value);
    const sha256 = createHash("sha256").update(text).digest("hex").slice(0, 16);
    secretKeys[key] = { matched: expected && typeof expected === "object" ? String(expected[key]) === text : null, sha256 };
    out[key] = `[secret ${sha256}]`;
  }
  return { result: out, secretKeys };
}
