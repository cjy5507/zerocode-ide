/* The shipped browser scripts against an offline, signed-in SPA. */
import assert from 'node:assert/strict';
import { chromium } from './playwright-chromium.mjs';
import { clickScript, fieldsScript, fillPasses, marksScript, uploadScript, waitScript } from './browser-scripts.mjs';

const receipt = { name: 'receipt.txt', mime: 'text/plain', data: Buffer.from('Synthetic receipt 18500 KRW').toString('base64') };
const browser = await chromium.launch();
const results = [];
const test = async (name, run) => { try { await run(); results.push({ name, ok: true }); } catch (error) { results.push({ name, ok: false, error: error.message }); } };
const page = await browser.newPage();
const run = async (script) => JSON.parse(await page.evaluate(script));
const click = async (selector) => assert.equal((await run(clickScript(selector))).ok, true);
const openEdit = async () => {
  await page.goto('about:blank');
  await page.goto(new URL('fixtures/expense-workflow/index.html#edit', import.meta.url).href);
  assert.equal(new URL(page.url()).hash, '#home', 'direct URL must return home');
  await click('#approval-menu');
  await click('a[href="#documents"]');
  await page.waitForSelector('#period');
  const filled = await fillPasses(run, { '#period': '지난달' });
  assert.equal(filled.left.length, 0);
  await click('#open-document');
  await page.waitForSelector('#copy');
  await click('#copy');
  await page.waitForSelector('#expense');
};
const upload = (selector, drop = false, files = [receipt]) => {
  return run(uploadScript(selector, files, drop));
};
await test('fields/fill updates controlled date and amount in application state', async () => {
  await openEdit();
  const fields = (await run(fieldsScript())).value.fields;
  assert.ok(fields.some((field) => field.label.includes('金額') || field.label.includes('금액')));
  const report = await fillPasses(run, { '#date': '2027-02-03', '#amount': '18500', '#template': '지출결의 · 환급신청서', '#approver': '팀 검토 → 재무 검토' });
  assert.equal(report.left.length, 0, JSON.stringify(report));
  await page.locator('#amount').blur();
  assert.equal(await page.inputValue('#amount'), '18500');
  assert.equal(await page.inputValue('#date'), '2027-02-03');
});
await test('wait observes a delayed SPA route without navigating to its URL', async () => {
  await openEdit();
  assert.equal((await run(waitScript('#expense'))).value, true);
  assert.equal((await run(waitScript('#complete'))).value, false);
});
await test('modal file input receives bytes and the SPA submits the chosen form', async () => {
  await openEdit();
  await run(fieldsScript());
  assert.equal((await fillPasses(run, { '#date': '2027-02-03', '#amount': '18500', '#approver': '팀 검토 → 재무 검토' })).left.length, 0);
  await click('#attach');
  const answer = await upload('#receipt');
  assert.equal(answer.ok, true, JSON.stringify(answer));
  assert.equal(answer.value.verified, true);
  assert.match(await page.locator('#upload-status').innerText(), /receipt.txt/);
  assert.equal(await page.locator('#receipt').evaluate(async (input) => input.files[0].text()), 'Synthetic receipt 18500 KRW');
  await click('#upload-done');
  await click('#review');
  await page.waitForSelector('#confirm');
  assert.match(await page.locator('main').innerText(), /18500원/);
  await click('#confirm');
  await page.waitForSelector('#complete');
  const submitted = JSON.parse(await page.locator('#result').innerText());
  assert.equal(submitted.submitted, true);
  assert.equal(submitted.template, 'refund');
  assert.equal(submitted.files[0].name, receipt.name);
});
await test('numbered look includes modal controls and excludes covered page controls', async () => {
  await openEdit();
  await click('#attach');
  const look = await run(marksScript());
  assert.equal(look.ok, true, JSON.stringify(look));
  const hittable = look.value.faces.filter((face) => face.hit);
  assert.ok(hittable.some((face) => face.selector === '#upload-done'));
  assert.ok(hittable.some((face) => face.selector === '#receipt'));
  assert.ok(!hittable.some((face) => face.selector === '#review' || face.selector === '#approval-menu'));
});
await test('modal drop target receives a file through drag and drop events', async () => {
  await openEdit();
  await click('#attach');
  const answer = await upload('#drop-zone', true);
  assert.equal(answer.ok, true, JSON.stringify(answer));
  assert.equal(answer.value.verified, false, 'drop delivery must not claim site acceptance');
  assert.match(await page.locator('#upload-status').innerText(), /receipt.txt/);
});
await test('hidden file input is reached only through its visible associated label', async () => {
  await openEdit();
  await click('#attach');
  await page.locator('#receipt').evaluate((input) => { input.style.display = 'none'; });
  assert.equal((await upload('#receipt')).ok, true);
  assert.match(await page.locator('#upload-status').innerText(), /receipt.txt/);
});
await test('covered inputs and non-upload controls receive no file or event', async () => {
  await openEdit();
  await click('#attach');
  const refused = await upload('#amount');
  assert.equal(refused.ok, false);
  assert.equal(await page.inputValue('#amount'), '12000');
  await page.locator('#receipt').evaluate((input) => { input.disabled = true; });
  const disabled = await upload('#receipt');
  assert.equal(disabled.code, 'element_disabled');
  assert.equal(await page.locator('#receipt').evaluate((input) => input.files.length), 0);
});
await test('an upload refuses a stale form before changing its FileList', async () => {
  await openEdit();
  const before = (await run(fieldsScript())).value.fingerprint;
  await click('#attach');
  const answer = await run(uploadScript('#receipt', [receipt], false, before));
  assert.equal(answer.code, 'form_stale');
  assert.equal(await page.locator('#receipt').evaluate((input) => input.files.length), 0);
});
await browser.close();
for (const result of results) console.log(JSON.stringify(result));
process.exitCode = results.every((result) => result.ok) ? 0 : 1;
