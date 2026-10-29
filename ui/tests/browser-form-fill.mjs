/* The form fill's page half, driven in real Chromium (t-79331 ③).
 *
 * `ui/browser-form-fill.js` finds the fields a saved address can fill and
 * writes into them. This proves the part the pane relies on: the right kinds
 * are found by label words in Korean and by autocomplete marks, the form a
 * page is about wins over its search box, a sign-in form holds too few kinds
 * to be offered anything, card / password / one-time-code fields are never
 * written, a field the page already filled is left alone, the values never
 * come back (only counts do), and a page's own listeners hear the write.
 *
 *   node ui/tests/browser-form-fill.mjs
 *
 * The pages are `tests/fixtures/browser-profile/address-*.html`; the values
 * are synthetic markers — the ones `browser_form_fill::fill_values` gives the
 * fixture's saved address. Nothing here reads a real browser profile.
 */

import { endRun } from "./end-run.mjs";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "./playwright-chromium.mjs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const SOURCE = (await readFile(resolve(ROOT, "ui/browser-form-fill.js"), "utf8")).trim();
const PAGES = resolve(ROOT, "tests/fixtures/browser-profile");

// What the backend hands the page for the fixture's most used address.
const VALUES = {
  name: "[합성-이름-나]",
  "given-name": "[합성-이름]",
  "family-name": "[합성-성]",
  tel: "[합성-전화]",
  "postal-code": "[합성-우편번호]",
  "street-address": "[합성-주소-1줄]",
  "address-line1": "[합성-주소-1줄]",
  "address-line2": "[합성-주소-2줄]",
  "address-level2": "[합성-도시]",
  email: "[합성-이메일-나]",
  country: "KR",
};
const ENTRIES = { nickname: "[합성-별명]", cardno: "[합성-카드값]", otp: "[합성-인증번호]" };

const site = createServer(async (request, response) => {
  const name = new URL(request.url ?? "/", "http://127.0.0.1").pathname.slice(1);
  try {
    const body = await readFile(resolve(PAGES, name));
    response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
    response.end(body);
  } catch {
    response.writeHead(404);
    response.end();
  }
});
await new Promise((done) => site.listen(0, "127.0.0.1", done));
const origin = `http://127.0.0.1:${site.address().port}`;

const browser = await chromium.launch({ headless: true });
let failed = 0;
let passed = 0;
const check = (name, pass, detail = "") => {
  console.log(`${pass ? "PASS" : "FAIL"}  ${name}${pass ? "" : `\n      ${detail}`}`);
  if (pass) passed += 1; else failed += 1;
};

const run = async (page, op, args) =>
  page.evaluate(([source, o, a]) => (0, eval)(`(${source})`)(o, a), [SOURCE, op, args ?? null]);
const fieldValues = (page) => page.evaluate(() => Object.fromEntries(
  [...document.querySelectorAll("input, select, textarea")].map((one) => [one.name, one.value])));

try {
  const page = await browser.newPage();

  // 1. Korean labels, no marks: the shipping form wins over the search box.
  await page.goto(`${origin}/address-labels.html`);
  const labels = await run(page, "detect");
  check("the shipping form's six kinds are found by their Korean labels",
    ["name", "tel", "postal-code", "street-address", "address-line2", "email"].every((kind) => labels.kinds.includes(kind))
      && labels.kinds.length === 6, JSON.stringify(labels));
  check("the other empty fields are named for past entries, the card field is not",
    labels.names.includes("nickname") && labels.names.includes("memo") && !labels.names.includes("cardno")
      && !labels.names.includes("q"), JSON.stringify(labels.names));
  check("detect answers no field value", !JSON.stringify(labels).includes("[합성"), JSON.stringify(labels));
  const before = await fieldValues(page);
  check("before the fill every field of the form is empty",
    Object.values(before).every((value) => value === ""), JSON.stringify(before));
  const filled = await run(page, "fill", { values: VALUES, entries: ENTRIES });
  const after = await fieldValues(page);
  check("the fill answers only how many fields took a value",
    filled.fields === 6 && filled.entries === 1 && Object.keys(filled).sort().join() === "entries,fields",
    JSON.stringify(filled));
  check("the answer carries no value", !JSON.stringify(filled).includes("합성"), JSON.stringify(filled));
  check("each field took its kind's value",
    after.receiver === VALUES.name && after.mobile === VALUES.tel && after.zip === VALUES["postal-code"]
      && after.addr1 === VALUES["street-address"] && after.addr2 === VALUES["address-line2"]
      && after.mail === VALUES.email && after.nickname === ENTRIES.nickname,
    JSON.stringify(after));
  check("the search box, the memo and the card field stay empty",
    after.q === "" && after.memo === "" && after.cardno === "", JSON.stringify({ q: after.q, memo: after.memo, card: after.cardno.length }));
  const counted = Object.values(after).filter((value) => value !== "").length - Object.values(before).filter((value) => value !== "").length;
  check("the form went from 0 filled fields to 7", counted === 7, String(counted));

  // 2. a second fill leaves what is already written alone.
  const again = await run(page, "fill", { values: { name: "[합성-다른-이름]" }, entries: {} });
  check("a field that holds a value is never overwritten",
    again.fields === 0 && (await fieldValues(page)).receiver === VALUES.name, JSON.stringify(again));

  // 3. autocomplete marks, a select, a pre-filled field, and the refused marks.
  await page.goto(`${origin}/address-autocomplete.html`);
  await page.evaluate(() => {
    window.__HEARD__ = 0;
    document.querySelector("[name=f1]").addEventListener("input", () => { window.__HEARD__ += 1; });
  });
  const marks = await run(page, "detect");
  check("autocomplete marks name the kinds, with tel-national read as tel and the hidden field skipped",
    ["given-name", "family-name", "address-line1", "address-line2", "address-level2", "postal-code", "country", "tel", "email"]
      .every((kind) => marks.kinds.includes(kind)) && !marks.kinds.includes("street-address"), JSON.stringify(marks));
  const checkout = await run(page, "fill", { values: VALUES, entries: ENTRIES });
  const written = await fieldValues(page);
  check("eight fields take a value; the select picks the country by its value",
    checkout.fields === 8 && written.f7 === "KR" && written.f1 === VALUES["given-name"] && written.f8 === VALUES.tel,
    JSON.stringify(checkout));
  check("the field the page already filled keeps its own value", written.f9 === "[합성-이미-쓴-값]", written.f9);
  check("a new-password and a one-time-code field are never written",
    written.pw === "" && written.otp === "" && checkout.entries === 0, JSON.stringify({ pw: written.pw.length, otp: written.otp.length }));
  check("the page's own listener heard the write", await page.evaluate(() => window.__HEARD__ === 1));

  // 4. a sign-in form holds one kind: too few to offer an address.
  await page.goto(`${origin}/address-signin.html`);
  const signin = await run(page, "detect");
  check("a sign-in form shows one kind (email), below the two an offer needs",
    signin.kinds.length === 1 && signin.kinds[0] === "email", JSON.stringify(signin));

  // 5. a street kept in two lines welds into one line in a one-line field.
  await page.setContent("<!doctype html><form><input autocomplete=name><input autocomplete=street-address name=s></form>");
  await run(page, "fill", { values: { name: "[합성-이름]", "street-address": "[합성-주소-1줄]\n[합성-주소-2줄]" } });
  const street = await page.evaluate(() => document.querySelector("[name=s]").value);
  check("a two-line street goes into a one-line field with a space between its lines",
    street === "[합성-주소-1줄] [합성-주소-2줄]", street);

  await page.setContent("<!doctype html><p>no form here</p>");
  const empty = await run(page, "detect");
  check("a page with no fields finds nothing", empty.kinds.length === 0 && empty.names.length === 0, JSON.stringify(empty));

  // The page-side cost: one detection on a page with many inputs.
  await page.setContent(`<form>${"<input type=text name=f>".repeat(400)}<input autocomplete=postal-code></form>`);
  const cost = await page.evaluate(([source]) => {
    const fn = (0, eval)(`(${source})`);
    const started = performance.now();
    fn("detect", null);
    return performance.now() - started;
  }, [SOURCE]);
  console.log(`      detection on a 401-input page took ${cost.toFixed(1)} ms`);
  check("one detection on a 401-input page stays under 50 ms", cost < 50, `${cost} ms`);
} finally {
  await browser.close();
  site.close();
}

console.log(`\n${passed}/${passed + failed} passed`);
endRun(failed === 0 ? 0 : 1);
