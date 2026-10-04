/* A small page with one of every kind of field the browser door's verbs touch
 * (t-41387), and a way to stand it in a folder as a scene — for the tests of
 * the stand-in window (test-desk.mjs) and of the runner that stands an agent
 * on a scene (test-real.mjs). The page keeps the booking a person would make:
 * a name, a size, an agreement, a password, a code sent to the person's phone
 * (`window.__personPhone`) and its check, and a "Book" that sets
 * `window.__sceneResult`. */

import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";

export const FIXTURE_HTML = `<!doctype html><html lang="en"><meta charset="utf-8"><title>fixture</title>
<form id="f" novalidate>
  <label for="name">Name</label> <input id="name" required>
  <label for="size">Size</label> <select id="size"><option value="">choose</option><option value="s">Small</option><option value="l">Large</option></select>
  <label><input type="checkbox" id="agree"> I agree</label>
  <label for="pw">Password</label> <input id="pw" type="password">
  <button type="button" id="send">Send code</button>
  <div id="codebox" hidden><label for="code">Verification code</label> <input id="code"> <button type="button" id="verify">Verify</button></div>
  <button type="button" id="book">Book</button>
</form>
<div id="terms" style="height:60px;overflow:auto"><p>one</p><p>two</p><p>three</p><p>four</p><p id="last">last</p></div>
<div style="height:1500px"></div>
<script>
  document.getElementById("send").addEventListener("click", () => {
    window.__personPhone = "123456";
    document.getElementById("codebox").hidden = false;
  });
  document.getElementById("verify").addEventListener("click", () => { window.__verified = document.getElementById("code").value === window.__personPhone; });
  document.getElementById("book").addEventListener("click", () => {
    window.__sceneResult = { name: document.getElementById("name").value, size: document.getElementById("size").value,
      agree: document.getElementById("agree").checked, verified: window.__verified === true };
  });
</script>`;

/* The fixture as a scene folder: its page, a card, and what it must take. */
export async function writeFixtureScene(folder, expected) {
  await mkdir(folder, { recursive: true });
  await writeFile(join(folder, "scene.html"), FIXTURE_HTML);
  await writeFile(join(folder, "card.json"), JSON.stringify({ task: "Book a size Large for me.", facts: [{ says: "Name", value: "Kim" }, { says: "Size", value: "Large" }], personTurns: ["code"] }));
  await writeFile(join(folder, "expected.json"), JSON.stringify(expected));
}
