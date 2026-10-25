/* A line-oriented browser-script boundary for the real Rust recipe runner.
 * Each line is one browser argv; replies have the CLI's TeamAnswer shape.
 * It owns only this fixture's window harness and never a desktop app. */
import { createInterface } from 'node:readline';
import { readFile, mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { openExpenseWindow } from './expense-workflow-window.mjs';
import { clickScript, fieldsScript, fillPasses, marksScript, pressInForm, readScript, uploadScript, waitScript, need, OPENED_PREFIX } from './browser-scripts.mjs';

const desk = await openExpenseWindow();
const run = async (script) => JSON.parse(await desk.guest.evaluate(script));
const lines = createInterface({ input: process.stdin });
let known = null;
let currentLabel = null;
let minted = 800;
const reply = (value) => process.stdout.write(`${JSON.stringify(value)}\n`);
reply({ ready: true, url: desk.fixtureUrl, receipt: fileURLToPath(new URL('fixtures/expense-workflow/receipt.txt', import.meta.url)) });
try {
  for await (const line of lines) {
    const argv = JSON.parse(line);
    if (argv[0] === 'finish') { reply({ exit_code: 0, stdout: '', stderr: '' }); break; }
    try {
      let value;
      let stdout;
      if (!['reset', 'open', 'verify', 'snapshot'].includes(argv[0]) && argv[1] !== currentLabel) throw new Error(`recipe_tab_missing: ${argv[1]} is closed or belongs to another tab`);
      switch (argv[0]) {
        case 'reset': if (!desk.guest.isClosed()) await desk.guest.close(); currentLabel = null; known = null; value = true; break;
        case 'open': currentLabel = `browser-${++minted}`; await desk.reopenGuest(argv[1], currentLabel); known = null; stdout = `${need('BROWSER_OPENED_PREFIX', OPENED_PREFIX)}${currentLabel}\n`; value = true; break;
        case 'close': await desk.guest.close(); currentLabel = null; known = null; value = true; break;
        case 'goto': await desk.guest.goto(argv[2]); known = null; value = true; break;
        case 'read': value = await run(readScript()); break;
        case 'marks': value = await run(marksScript()); break;
        case 'fields': value = await run(fieldsScript()); known = value.value.fingerprint; break;
        // Match press_in_form: a changed form does not replace the form the
        // caller knew. Its next fields call must refresh that fingerprint.
        case 'click': value = known === null ? await run(clickScript(argv[2])) : { ok: true, value: (await pressInForm(run, argv[2])).pressed }; break;
        case 'fill': {
          const report = await fillPasses(run, JSON.parse(argv.at(-1)), known);
          if (report.stale || report.left.length) throw new Error(JSON.stringify(report));
          known = report.fingerprint;
          value = report;
          break;
        }
        case 'upload': {
          const data = await readFile(argv[3]);
          value = await run(uploadScript(argv[2], [{ name: 'receipt.txt', mime: 'text/plain', data: data.toString('base64') }], false, known));
          break;
        }
        case 'wait': {
          const until = Date.now() + Number(argv[3] || 5000);
          while (Date.now() < until && !(await run(waitScript(argv[2]))).value) await new Promise((done) => setTimeout(done, 25));
          if (!(await run(waitScript(argv[2]))).value) throw new Error(`wait timed out: ${argv[2]}`);
          value = true;
          break;
        }
        case 'verify': value = JSON.parse(await desk.guest.locator('#result').innerText()); break;
        case 'snapshot': await mkdir(argv[1], { recursive: true }); await desk.snapshot(argv[1], 'recipe-complete'); value = true; break;
        default: throw new Error(`unsupported fixture driver verb: ${argv[0]}`);
      }
      if (value?.ok === false) throw new Error(JSON.stringify(value));
      reply({ exit_code: 0, stdout: stdout ?? `${JSON.stringify(value)}\n`, stderr: '' });
    } catch (error) { reply({ exit_code: 1, stdout: '', stderr: error.message }); }
  }
} finally { lines.close(); process.stdin.destroy(); await desk.close(); }
