/* 짝지어 재는 운전자 (6차 P0).
 *
 * 「한 번의 값은 적지 않는다」가 5차가 남긴 규약이다(볼트의
 * `paint-timings-quantize-so-compare-same-round-pairs`): 같은 코드가 붐비는 기계에서
 * 89ms와 136ms를 낸다. 그래서 전과 후를 **같은 세션에서 번갈아** 돌리고 중앙값만
 * 적으며, 첫 판은 캔버스를 처음 굽는 값이라 버린다.
 *
 * 재는 자(`knowledge-performance.mjs`의 `measureKnowledgeScenes`)는 언제나 **작업
 * 트리의 것**이다 — 전과 후에서 다른 자를 쓰면 그 표는 코드가 아니라 자를 잰다.
 * 바뀌는 것은 제품(`ui/`의 나머지)뿐이다.
 *
 *   node ui/tests/knowledge-ab.mjs --before HEAD~1 --rounds 3 [--painter gl]
 *
 * 전은 `target/knowledge-ab/before/`에 편다(`target/`은 git이 안 보는 곳이고,
 * 워크트리 안이라 `node_modules`가 위로 걸어 올라가 풀린다).
 */
import { execFileSync } from "node:child_process";
import { cp, mkdir, rm } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const UI = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const ROOT = resolve(UI, "..");
const STAGE = join(ROOT, "target", "knowledge-ab");

const flag = (name, fallback) => {
  const at = process.argv.indexOf(`--${name}`);
  return at < 0 ? fallback : process.argv[at + 1];
};
const before = flag("before", "HEAD~1");
/* 후의 기본은 작업 트리다. 커밋을 주면 그쪽도 펴 둔다 — 그래야 재는 동안 다음
 * 단계를 이어 써도 표가 흔들리지 않는다(재는 중에 `ui/`를 고치면 뒤의 판이 다른
 * 제품을 잰다). */
const after = flag("after", "");
const rounds = Number(flag("rounds", "3"));

/* 한 커밋의 `ui/`와 하네스가 읽는 픽스처를 그대로 펴고, 자만 작업 트리의 것으로
 * 덮는다. 픽스처(`fixtures/vault-lint`)는 백엔드의 lint 표를 견주는 시험이 읽는 것이라
 * `ui/` 밖에 살지만 자와 함께 움직인다. */
const stage = async (ref, where) => {
  await mkdir(join(STAGE, where), { recursive: true });
  execFileSync("/bin/sh", ["-c",
    `git archive ${ref} ui fixtures | tar -x -C ${JSON.stringify(join(STAGE, where))}`],
  { cwd: ROOT, stdio: "inherit" });
  await cp(join(UI, "tests"), join(STAGE, where, "ui", "tests"), { recursive: true });
};

/* 한 판 = 하네스 한 번. 돌려주는 것은 시나리오 줄들이고, 실패한 판은 값이 아니라
 * 소리를 낸다 — 붉은 하네스에서 나온 수는 수가 아니다. */
const hand = flag("painter", "svg");
const runOnce = (where) => {
  const out = execFileSync(process.execPath, [join(where, "ui", "tests", "test-knowledge-graph.mjs")],
    /* 시간의 표를 읽는 판이다 — 게이트의 몫(`KNOWLEDGE_GATE`)이 아니라 전체 훑기를 청한다. */
    { cwd: ROOT, encoding: "utf8", maxBuffer: 64 * 1024 * 1024,
      env: { ...process.env, KNOWLEDGE_SWEEP: "full" } });
  const line = out.split("\n").find((row) => row.startsWith(`METRIC knowledge scenes (${hand}):`));
  if (!line) throw new Error(`no ${hand} scene metric in the harness output from ${where}`);
  const scale = out.split("\n").find((row) => row.startsWith("METRIC knowledge graph 1020 nodes:"));
  return { scenes: JSON.parse(line.slice(line.indexOf("["))), scale: scale ?? "" };
};

const median = (rows) => rows.slice().sort((one, two) => one - two)[Math.floor(rows.length / 2)];

await rm(STAGE, { recursive: true, force: true });
await stage(before, "before");
if (after !== "") await stage(after, "after");
const samples = { before: [], after: [] };
const scaleLines = { before: [], after: [] };
/* 첫 판은 버린다(캔버스·JIT를 처음 굽는 값). 그 뒤로 전·후를 번갈아 — 같은 순간의
 * 기계 부하를 두 쪽이 나눠 가진다. */
for (let round = 0; round <= rounds; round += 1) {
  for (const side of ["before", "after"]) {
    const where = side === "before" ? join(STAGE, "before")
      : after === "" ? ROOT : join(STAGE, "after");
    const took = runOnce(where);
    process.stderr.write(`round ${round} ${side}: ${took.scenes.map(
      (row) => `${row.name} ${row.firstPaintMs}ms/p95 ${row.cameraP95Ms}`).join(" · ")}\n`);
    if (round === 0) continue;
    samples[side].push(took.scenes);
    scaleLines[side].push(took.scale);
  }
}

const names = samples.after[0].map((row) => row.name);
const numbers = ["firstPaintMs", "frameP95Ms", "frameWorstMs", "frameMedianMs",
  "cameraP95Ms", "cameraWorstMs", "cameraMedianMs", "draws",
  "labelOverlaps", "discOverlapPairs", "outsideDisc", "labelsShown", "heapMb", "sceneMs"];
const table = {};
for (const name of names) {
  table[name] = {};
  for (const key of numbers) {
    const pick = (side) => median(samples[side].map((rows) => rows.find((row) => row.name === name)[key]));
    table[name][key] = { before: pick("before"), after: pick("after") };
  }
}
console.log(`\n# paired medians of ${rounds} rounds (before = ${before}, after = ${after || "working tree"})\n`);
console.log("| scene | metric | before | after | after/before |");
console.log("|---|---|---|---|---|");
for (const name of names) {
  for (const key of numbers) {
    const { before: was, after: now } = table[name][key];
    const ratio = was === 0 || was === null ? "—" : `${Math.round((now / was) * 100)}%`;
    console.log(`| ${name} | ${key} | ${was} | ${now} | ${ratio} |`);
  }
}
console.log(`\nbefore scale: ${scaleLines.before.join("\n              ")}`);
console.log(`after  scale: ${scaleLines.after.join("\n              ")}`);
