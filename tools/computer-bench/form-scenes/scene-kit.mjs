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

/* A scene's card and expected result, and the name its folder gives it. */
export async function readScene(folder) {
  const root = resolve(folder);
  const card = JSON.parse(await readFile(join(root, "card.json"), "utf8"));
  const expected = JSON.parse(await readFile(join(root, "expected.json"), "utf8"));
  return { folder: root, name: root.split(sep).filter(Boolean).pop(), card, expected };
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
