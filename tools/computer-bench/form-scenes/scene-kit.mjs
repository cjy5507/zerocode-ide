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
import { extname, join, resolve, sep } from "node:path";

const TYPES = { ".html": "text/html; charset=utf-8", ".js": "text/javascript", ".css": "text/css", ".json": "application/json" };

/* A scene's card and expected result, the keys its page makes itself, and the
 * name its folder gives it. `made.json` is the scene's own declaration —
 * `{"keys": [...]}`: what the page puts into its result that no field an agent
 * can write holds (a booking number the page counts up, a total it adds), so
 * the oracle can tell such a key from a field the card left alone. A scene
 * with none declares nothing. */
export async function readScene(folder) {
  const root = resolve(folder);
  const card = JSON.parse(await readFile(join(root, "card.json"), "utf8"));
  const expected = JSON.parse(await readFile(join(root, "expected.json"), "utf8"));
  const made = await readFile(join(root, "made.json"), "utf8").then((text) => JSON.parse(text).keys, () => []);
  return { folder: root, name: root.split(sep).filter(Boolean).pop(), card, expected, made: Array.isArray(made) ? made : [] };
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

/* The oracle: the keys of `expected` the page's result does not hold, by
 * JSON equality — a result the page never set holds none of them. Extra keys
 * in the result are the page's own. */
export const wrongKeys = (result, expected) =>
  Object.keys(expected).filter((key) => !result || JSON.stringify(result[key]) !== JSON.stringify(expected[key]));

/* The first oracle (before e29f7f84a): the page's whole result is the expected
 * one, key by key and in the same order — but for the keys the scene declares
 * its page makes itself (`made`, from `made.json`), which belong to neither
 * side. Kept beside `wrongKeys` so a pass is said under both, and `extraKeys`
 * says what only the page recorded: a key no card names and no declaration
 * excuses is a field an agent could have written, and counts against the pass. */
const without = (value, made) => Object.fromEntries(Object.entries(value || {}).filter(([key]) => !made.includes(key)));
export const sameWhole = (result, expected, made = []) =>
  Boolean(result) && JSON.stringify(without(result, made)) === JSON.stringify(without(expected, made));
const sortedByKey = (value) => JSON.stringify(Object.fromEntries(Object.entries(value || {}).sort(([a], [b]) => (a < b ? -1 : 1))));
/* The same, without the order of the keys. */
export const sameAsSet = (result, expected, made = []) =>
  Boolean(result) && sortedByKey(without(result, made)) === sortedByKey(without(expected, made));
/* The keys the page recorded that the expected result does not hold and the
 * scene does not declare. */
export const extraKeys = (result, expected, made = []) =>
  Object.keys(result || {}).filter((key) => !(key in expected) && !made.includes(key));
/* The declared keys the page did record — said beside a pass. */
export const madeKeys = (result, made = []) => made.filter((key) => key in (result || {}));
