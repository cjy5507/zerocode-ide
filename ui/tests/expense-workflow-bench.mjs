/* Offline comparison. These are scripted tool decisions, not model timings.
 * Window chrome is real; native browser IPC is a harness boundary. */
import assert from 'node:assert/strict';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { performance } from 'node:perf_hooks';
import { openExpenseWindow } from './expense-workflow-window.mjs';
import { clickScript, fieldsScript, fillPasses, marksScript, readScript, uploadScript, waitScript } from './browser-scripts.mjs';

const out = resolve(process.argv[2] || 'output/expense-workflow-bench');
await mkdir(out, { recursive: true });
const receiptPath = new URL('fixtures/expense-workflow/receipt.txt', import.meta.url);
const receipt = { name: 'receipt.txt', mime: 'text/plain', data: (await readFile(receiptPath)).toString('base64') };
const desk = await openExpenseWindow();
const { guest, fixtureUrl } = desk;

const measurements = [];
const rounds = Number(process.env.ZC_EXPENSE_ROUNDS || 3);
assert.ok(Number.isInteger(rounds) && rounds >= 1 && rounds <= 10);
try {
  for (let round = 1; round <= rounds; round += 1) {
  for (const mode of round % 2 ? ['evaluate', 'browser'] : ['browser', 'evaluate']) {
    const calls = [];
    const call = async (verb, work) => {
      const began = performance.now();
      const row = { verb };
      calls.push(row);
      try { const value = await work(); row.ok = true; return value; }
      catch (error) { row.ok = false; throw error; }
      finally { row.ms = +(performance.now() - began).toFixed(2); }
    };
    const run = async (script) => JSON.parse(await guest.evaluate(script));
    const evaluate = (fn, arg) => call('evaluate', () => guest.evaluate(fn, arg));
    const read = () => mode === 'browser' ? call('read', () => run(readScript())) : evaluate(() => ({ title: document.title, text: document.body.innerText, url: location.href }));
    const click = async (words) => {
      let selector;
      if (mode === 'browser') {
        const look = await call('marks', () => run(marksScript()));
        const face = look.value.faces.find((one) => one.hit && one.label.trim() === words);
        assert.ok(face, words);
        selector = face.selector;
        assert.equal((await call('click', () => run(clickScript(selector)))).ok, true);
      } else {
        selector = await evaluate((words) => {
          const element = [...document.querySelectorAll('button,a')].find((one) => one.innerText.trim() === words);
          return element?.id ? `#${element.id}` : element?.getAttribute('href') ? `a[href="${element.getAttribute('href')}"]` : null;
        }, words);
        assert.ok(selector, words);
        await evaluate((selector) => document.querySelector(selector).click(), selector);
      }
    };
    const wait = (selector) => call(mode === 'browser' ? 'wait' : 'evaluate-wait', async () => {
      // Repeat exactly the door's page predicate on a local clock. Polls are
      // internal callbacks of ONE wait, not additional model/tool requests.
      const until = performance.now() + 5000;
      while (performance.now() < until) {
        const ready = mode === 'browser' ? (await run(waitScript(selector))).value : await guest.evaluate((selector) => !!document.querySelector(selector), selector);
        if (ready) return;
        await new Promise((done) => setTimeout(done, 25));
      }
      throw new Error(`wait timed out: ${selector}`);
    });
    const fill = async (values) => {
      if (mode === 'browser') {
        const fields = await call('fields', () => run(fieldsScript()));
        const bundle = {};
        for (const [label, value] of Object.entries(values)) {
          const field = fields.value.fields.find((one) => one.label === label);
          assert.ok(field, label);
          bundle[field.handle] = value;
        }
        const report = await call('fill', () => fillPasses(run, bundle, fields.value.fingerprint));
        assert.equal(report.stale, false, JSON.stringify(report));
        assert.equal(report.left.length, 0, JSON.stringify(report));
      } else {
        const fields = await evaluate(() => [...document.querySelectorAll('input,select')].map((input) => ({ id: input.id, label: input.labels?.[0]?.childNodes[0]?.textContent?.trim(), tag: input.tagName, options: input.options ? [...input.options].map(({ text, value }) => ({ text, value })) : [] })));
        for (const [label, value] of Object.entries(values)) {
          const field = fields.find((one) => one.label === label);
          assert.ok(field, label);
          const written = field.options.find((option) => option.text === value)?.value ?? value;
          await evaluate(({ id, written }) => {
            const element = document.getElementById(id);
            Object.getOwnPropertyDescriptor(Object.getPrototypeOf(element), 'value').set.call(element, written);
            element.dispatchEvent(new Event('input', { bubbles: true }));
            element.dispatchEvent(new Event('change', { bubbles: true }));
          }, { id: field.id, written });
          assert.equal(await evaluate((id) => document.getElementById(id).value, field.id), written);
        }
      }
    };
    await guest.goto('about:blank');
    const began = performance.now();
    await call('open', () => guest.goto(fixtureUrl));
    await read();
    await click('전자결재');
    await click('내 문서함');
    await wait('#period');
    await fill({ '기간': '지난달' });
    await click('문서 열기');
    await wait('#copy');
    await click('사본 만들기');
    await wait('#expense');
    await fill({ '지출일': '2027-02-03', '금액': '18500', '양식': '지출결의 · 환급신청서', '결재선': '팀 검토 → 재무 검토' });
    await click('증빙 첨부');
    await wait('#uploader[open]');
    if (mode === 'browser') {
      const form = await call('fields', () => run(fieldsScript()));
      const input = form.value.fields.find((field) => field.kind === 'file');
      assert.ok(input);
      assert.equal((await call('upload', () => run(uploadScript(input.handle, [receipt], false, form.value.fingerprint)))).value.verified, true);
    } else {
      const selector = await evaluate(() => `#${document.querySelector('dialog input[type=file]').id}`);
      await evaluate(({ selector, receipt }) => {
        const element = document.querySelector(selector), data = new DataTransfer();
        data.items.add(new File([Uint8Array.from(atob(receipt.data), (char) => char.charCodeAt(0))], receipt.name, { type: receipt.mime }));
        element.files = data.files;
        element.dispatchEvent(new Event('change', { bubbles: true }));
      }, { selector, receipt });
    }
    await read();
    await click('첨부 완료');
    await click('상신 내용 확인');
    await wait('#confirm');
    await read();
    await click('시험 상신 확정');
    await wait('#complete');
    const completed = await read();
    const elapsedMs = +(performance.now() - began).toFixed(2);
    const result = JSON.parse(await guest.locator('#result').innerText());
    assert.equal(result.submitted, true);
    assert.equal(result.date, '2027-02-03');
    assert.equal(result.amount, '18500');
    assert.equal(result.files[0].name, 'receipt.txt');
    assert.match(JSON.stringify(completed), /시험 상신 완료/);
    measurements.push({ round, mode, elapsedMs, calls: calls.length, logicalSteps: 9, modelRoundTrips: 0, trace: calls });
    await guest.screenshot({ path: resolve(out, `${mode}-complete.png`) });
  }
  }
  await desk.snapshot(out);
  await writeFile(resolve(out, 'measurements.json'), `${JSON.stringify({ boundary: 'Real window renderer; guest commands use shipped Rust scripts in Chromium. Native IPC and model latency excluded.', measurements }, null, 2)}\n`);
  console.log(JSON.stringify(measurements.map(({ trace, ...measurement }) => measurement)));
} finally {
  await desk.close();
}
