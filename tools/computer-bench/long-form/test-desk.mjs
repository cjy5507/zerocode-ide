/* The long-form bench end to end without a model (t-37883): the page in
 * Chromium, the fake desk, the fake shim and the oracle. A scripted hand that
 * fills the card through the desktop verbs an agent uses passes; a run that
 * does nothing fails; a value written without the events a person's typing
 * makes is scored wrong, because the page keeps its own state.
 *
 *   NODE_PATH=…/node_modules node tools/computer-bench/long-form/test-desk.mjs
 */

import { spawn, spawnSync } from "node:child_process";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { startDesk } from "./desk.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const SPEC = JSON.parse(await readFile(join(HERE, "spec.json"), "utf8"));
const EXPECTED = SPEC.card.expected;
const PERSON_ATTACH_MS = 10;
let failures = 0;

function check(name, held, detail = "") {
  console.log(`${held ? "PASS" : "FAIL"}  ${name}${held ? "" : `  ${detail}`}`);
  if (!held) failures += 1;
}

async function standing(fn) {
  const out = await mkdtemp(join(tmpdir(), "long-form-"));
  const server = spawn("python3", [join(HERE, "server.py"), "--out", out], { stdio: ["ignore", "pipe", "inherit"] });
  const url = await new Promise((done) => server.stdout.on("data", (chunk) => {
    const said = String(chunk).match(/listening (\S+)/);
    if (said) done(said[1]);
  }));
  const desk = await startDesk({ url, out, personAttachMs: PERSON_ATTACH_MS });
  try {
    await fn({ desk, out });
  } finally {
    await desk.close();
    server.kill();
  }
  const scored = JSON.parse(spawnSync("python3", [join(HERE, "oracle.py"), "--out", out, "--json"], { encoding: "utf8" }).stdout);
  await rm(out, { recursive: true, force: true });
  return scored;
}

/* The fake shim, called as an agent calls it — never synchronously: the
 * desk answering it runs in this same process. */
function shim(desk, argv) {
  return new Promise((done) => {
    const child = spawn(join(HERE, "bin", "zerocode-computer"), argv, {
      env: { ...process.env, LONG_FORM_DESK: `http://127.0.0.1:${desk.port}` },
    });
    let out = "";
    let err = "";
    child.stdout.on("data", (chunk) => (out += chunk));
    child.stderr.on("data", (chunk) => (err += chunk));
    child.on("close", (code) => done({ code, out, err }));
  });
}

const label = (id) => SPEC.sections.flatMap((section) => section.fields).find((field) => field.id === id).label;

/* A hand that knows the page: each field by what it reads, then its value
 * typed or chosen as a person would. The yes/no questions all read "Yes"
 * and "No", so they are pressed by where they stand (`yesNoClicks`). */
function fillSteps() {
  const steps = [];
  const option = (field, code) => SPEC.options[field.options].find(([value]) => value === code)[1];
  for (const field of SPEC.sections.flatMap((section) => section.fields).filter((each) => !each.shows_when)) {
    const value = EXPECTED[field.id];
    const name = field.label;
    const click = ["click", "--app", "Google Chrome", "--label", name, "--mouse-button", "left", "--click-count", "1", "--no-screenshot", "--json"];
    if (["text", "email", "tel", "textarea"].includes(field.kind)) {
      if (value) steps.push(click, ["type", "--text", value, "--json"]);
    } else if (field.kind === "date") {
      const [year, month, day] = value.split("-");
      steps.push(click, ["type", "--text", `${month}${day}${year}`, "--json"]);
    } else if (field.kind === "datepick") {
      const [year, month, day] = value.split("-");
      steps.push(click, ["type", "--text", `${day}/${month}/${year}`, "--json"]);
    } else if (field.kind === "select") {
      steps.push(click, ["type", "--text", option(field, value), "--json"]);
    } else if (field.kind === "combobox") {
      steps.push(click, ["type", "--text", option(field, value).slice(0, 8), "--json"], ["key", "--key", "Return", "--json"]);
    } else if (field.kind === "radio") {
      steps.push(["click", "--app", "Google Chrome", "--label", option(field, value), "--role", "radio", "--json"]);
    } else if (field.kind === "counter") {
      for (let count = 0; count < value; count += 1) steps.push(["click", "--app", "Google Chrome", "--label", `Increase: ${name}`, "--json"]);
    } else if (field.kind === "checkbox") {
      if (value) steps.push(["click", "--app", "Google Chrome", "--label", name, "--role", "checkbox", "--json"]);
    }
  }
  return steps;
}

// A run that did nothing fails.
const idle = await standing(async () => {});
check("a run that did nothing fails", idle.pass === false && idle.submits === 0, JSON.stringify(idle.reasons));

// A value placed without the events a person's typing makes never reaches the page's state.
const silent = await standing(async ({ desk }) => {
  await desk.page.evaluate(() => { document.getElementById("family_name").value = "SAMPLE"; });
  await desk.page.evaluate(() => document.getElementById("form").requestSubmit());
});
check("a value written without its events is not what the page holds", silent.submits === 0 || silent.fields.family_name !== "ok",
  JSON.stringify(silent));

// A scripted hand through the desktop verbs fills the card and passes.
const filled = await standing(async ({ desk }) => {
  const answers = [];
  for (const step of fillSteps()) answers.push(await shim(desk, step));
  for (const field of SPEC.sections.flatMap((section) => section.fields).filter((each) => each.kind === "yesno")) {
    const radio = desk.page.locator(`#${field.id}-${EXPECTED[field.id]}`);
    await radio.scrollIntoViewIfNeeded();
    const box = await radio.boundingBox();
    if (box) answers.push(await shim(desk, ["mouse-click", "--x", String(box.x + 2), "--y", String(box.y + 2), "--mouse-button", "left", "--click-count", "1", "--json"]));
  }
  // The other nationality appears once the dual question is answered.
  answers.push(await shim(desk, ["click", "--app", "Google Chrome", "--label", label("second_nationality"), "--json"]));
  answers.push(await shim(desk, ["type", "--text", "Canada", "--json"]));
  answers.push(await shim(desk, ["handoff", "--reason", "attach the passport scan", "--json"]));
  answers.push(await shim(desk, ["click", "--app", "Google Chrome", "--label", "Submit registration", "--json"]));
  const refused = answers.filter((answer) => answer.code !== 0);
  check("every scripted step was answered", refused.length === 0, refused.map((answer) => answer.err).slice(0, 3).join(" | "));
  const look = JSON.parse((await shim(desk, ["observe", "--diff", "--settle", "--json"])).out);
  check("a look answers a picture with its frame", Boolean(look.result.screenshot.path) && look.result.screenshot.width === 1280,
    JSON.stringify(look.result.screenshot));
});
check("the card filled through the desk passes the oracle", filled.pass === true, JSON.stringify(filled.reasons));

// A batch stops at its first refusal and says where.
await standing(async ({ desk }) => {
  const commands = JSON.stringify([["wait", "--ms", "1", "--json"], ["click", "--app", "Google Chrome", "--label", "No such control", "--json"], ["wait", "--ms", "1", "--json"]]);
  const answer = await shim(desk, ["batch", "--commands", commands, "--json"]);
  const said = JSON.parse(answer.err);
  check("a batch stops at its first refusal", answer.code === 1 && said.result.refusedAt === 2 && said.result.ran === 1, answer.err);
});

console.log(failures ? `${failures} FAILED` : "all passed");
process.exit(failures ? 1 : 0);
