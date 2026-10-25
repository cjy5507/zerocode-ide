/* Replay through the installed window's own CLI. Opens and closes only its
 * synthetic tab. Run after publishing the upload verb; old builds stop there.
 * Scripted decisions: no model latency is included in these measurements. */
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { performance } from 'node:perf_hooks';

const out = resolve(process.argv[2] || 'output/expense-workflow-cli');
await mkdir(out, { recursive: true });
const fixture = new URL('fixtures/expense-workflow/index.html#edit', import.meta.url).href;
const receipt = fileURLToPath(new URL('fixtures/expense-workflow/receipt.txt', import.meta.url));
const trace = [];
let label;
let ok = false;
const began = performance.now();
const scrub = (text) => text.replaceAll(fileURLToPath(new URL('../../', import.meta.url)), '<worktree>/');
const command = (args, input, record = true) => {
  const start = performance.now();
  const result = spawnSync('zerocode-browser', args, { input, encoding: 'utf8', timeout: 15000, maxBuffer: 1024 * 1024 });
  if (record) trace.push({ verb: args[0], ms: +(performance.now() - start).toFixed(2), rc: result.status, stdout: scrub(result.stdout || ''), stderr: scrub(result.stderr || '') });
  assert.equal(result.status, 0, `${args[0]} failed: ${result.stderr || result.stdout || result.error}`);
  return result.stdout;
};
const click = (words) => {
  const look = JSON.parse(command(['marks', label, '--json']));
  const item = look.items.find((one) => one.label.trim() === words);
  assert.ok(item, `no visible control: ${words}`);
  command(['click', label, '--mark', String(item.mark)]);
};
const wait = (selector) => command(['wait', label, selector]);
const read = () => command(['read', label]);
const fill = (values) => {
  const fields = JSON.parse(command(['fields', label, '--json']));
  const bundle = {};
  for (const [words, value] of Object.entries(values)) {
    const field = fields.fields.find((one) => one.label === words);
    assert.ok(field, words);
    bundle[field.handle] = value;
  }
  command(['fill', label, '--value-stdin'], JSON.stringify(bundle));
};
try {
  const opened = command(['open', fixture]);
  label = opened.match(/browser-\d+/)?.[0];
  assert.ok(label);
  read();
  click('전자결재');
  click('내 문서함');
  wait('#period');
  fill({ '기간': '지난달' });
  click('문서 열기');
  wait('#copy');
  click('사본 만들기');
  wait('#expense');
  fill({ '지출일': '2027-02-03', '금액': '18500', '양식': '지출결의 · 환급신청서', '결재선': '팀 검토 → 재무 검토' });
  click('증빙 첨부');
  wait('#uploader[open]');
  const fields = JSON.parse(command(['fields', label, '--json']));
  const file = fields.fields.find((one) => one.kind === 'file');
  assert.ok(file);
  command(['upload', label, file.handle, receipt]);
  assert.match(read(), /receipt.txt/);
  click('첨부 완료');
  click('상신 내용 확인');
  wait('#confirm');
  assert.match(read(), /18500원/);
  click('시험 상신 확정');
  wait('#complete');
  assert.match(read(), /시험 상신 완료/);
  ok = true;
} catch (error) {
  console.error(scrub(error.message));
  process.exitCode = 1;
} finally {
  const elapsedMs = +(performance.now() - began).toFixed(2);
  if (label) {
    // A fresh observation before cleanup also respects a tab's revision.
    try { command(['read', label], undefined, false); command(['close', label], undefined, false); }
    catch (error) { console.error(`cleanup: ${scrub(error.message)}`); process.exitCode = 1; }
  }
  const result = { ok, elapsedMs, calls: trace.length, logicalSteps: 9, modelRoundTrips: 0, trace };
  await writeFile(resolve(out, 'measurements.json'), `${JSON.stringify(result, null, 2)}\n`);
  console.log(JSON.stringify({ ...result, trace: undefined }));
}
