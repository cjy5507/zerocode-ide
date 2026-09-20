/* The console reads the evidence writer's Step values. Its maps are a bounded
 * view cache, never a second recording; closing it starts no timer or capture.
 * The left column remains the stage's existing browser/device mirror. */
const FLOW_LIVE_RUNS = 20;
const FLOW_LIVE_STEPS = 500;
const flowRuns = new Map();
let flowRunDir = null;
let flowConsoleFrame = null;
let flowMirrorTab = null;
let flowConsoleReading = 0;
let flowLaunchTail = Promise.resolve();
const flowPending = [];

function flowRunView(dir) {
  if (!flowRuns.has(dir)) {
    flowRuns.set(dir, { dir, steps: new Map(), report: null, walk: null, cwd: null });
    if (flowRuns.size > FLOW_LIVE_RUNS) flowRuns.delete(flowRuns.keys().next().value);
  }
  return flowRuns.get(dir);
}
function rememberFlowSteps(run, steps) {
  for (const step of steps) {
    if (Number.isSafeInteger(step?.n) && step.n > 0) run.steps.set(step.n, step);
  }
  const excess = run.steps.size - FLOW_LIVE_STEPS;
  if (excess > 0) {
    for (const n of [...run.steps.keys()].sort((a, b) => a - b).slice(0, excess)) run.steps.delete(n);
  }
}
function scheduleFlowConsole() {
  if (el("flow-console").hidden || flowConsoleFrame !== null) return;
  flowConsoleFrame = requestAnimationFrame(() => { flowConsoleFrame = null; paintFlowConsole(); });
}
/* These notices are delivered into the main webview itself. They are not
 * broadcast through Tauri's Any listeners, which guest views can hold. */
window.addEventListener("flow:begin", ({ detail: payload }) => {
  if (!payload?.dir) return;
  flowConsoleReading += 1;
  Object.assign(flowRunView(payload.dir), payload, { walk: null, report: null });
  flowRunDir = payload.dir;
  scheduleFlowConsole();
});
window.addEventListener("flow:step", ({ detail: payload }) => {
  const [dir, step] = payload ?? [];
  if (typeof dir !== "string") return;
  const run = flowRunView(dir);
  rememberFlowSteps(run, [step]);
  scheduleFlowConsole();
});
window.addEventListener("flow:walk", ({ detail: payload }) => {
  const [dir, walk] = payload ?? [];
  if (!dir || !walk) return;
  const run = flowRunView(dir);
  Object.assign(run, { walk, name: walk.name, kind: walk.kind, cwd: walk.cwd ?? run.cwd });
  scheduleFlowConsole();
});
window.addEventListener("flow:report", ({ detail: payload }) => {
  if (!payload?.dir) return;
  flowRunView(payload.dir).report = payload.report;
  scheduleFlowConsole();
});

function openFlowConsole() {
  setSettingsOpen(false);
  el("stage").classList.add("has-flow-console");
  el("flow-console").hidden = false;
  const mirrors = flowMirrors();
  const current = currentTab();
  const mirror = mirrors.find((tab) => tab.id === current?.id)
    ?? mirrors.find((tab) => tab.id === flowMirrorTab) ?? mirrors[0];
  if (mirror) { flowMirrorTab = mirror.id; setActiveTab(mirror.id); }
  paintFlowConsole();
  el("flow-goal").focus();
}
function flowMirrors() {
  return tabs.filter((tab) => ["browser", "emulator"].includes(tab.kind)
    && tab.worktree === activeWorktreePath);
}
function paintFlowMirrorPicker() {
  const pick = el("flow-console-mirror");
  const mirrors = flowMirrors();
  pick.replaceChildren();
  const hint = flowText("option", t("flow.live.mirror", "브라우저 · 기기 화면"));
  hint.value = "";
  pick.append(hint);
  for (const tab of mirrors) {
    const option = flowText("option", tab.deviceName ?? tab.title ?? tab.label ?? tab.id);
    option.value = tab.id;
    pick.append(option);
  }
  pick.value = mirrors.some((tab) => tab.id === flowMirrorTab) ? flowMirrorTab : "";
  pick.setAttribute("aria-label", t("flow.live.mirror", "브라우저 · 기기 화면"));
}
function closeFlowConsole() {
  el("flow-console").hidden = true;
  el("stage").classList.remove("has-flow-console");
  if (flowConsoleFrame !== null) cancelAnimationFrame(flowConsoleFrame);
  flowConsoleFrame = null;
  flowConsoleReading += 1;
}
function flowText(tag, text, cls = "") {
  const node = document.createElement(tag);
  node.className = cls;
  node.textContent = text;
  return node;
}
function flowButton(text, action, cls = "btn") {
  const button = flowText("button", text, cls);
  button.type = "button";
  button.addEventListener("click", action);
  return button;
}
function flowMs(value) {
  return Number.isFinite(value) ? `${value} ms` : t("flow.live.unmeasured", "미측정");
}
function flowStepCard(run, step) {
  const card = flowText("article", "", "flow-step");
  card.dataset.n = step.n;
  card.append(flowText("strong", `${step.n} · ${step.tool} ${step.verb}`));
  card.append(flowText("p", step.argv?.join(" ") ?? "", "flow-step-command"));
  if (step.error) card.append(flowText("p", step.error, "orch-error"));
  const observed = step.observation ?? {};
  if (observed.elapsed_ms !== undefined) card.append(flowText("p", t("flow.live.elapsed", "명령 {{ms}}", { ms: flowMs(observed.elapsed_ms) })));
  card.append(flowText("p", t("flow.live.look", "보기 {{ms}}", { ms: flowMs(observed.look_ms) })));
  card.append(flowText("p", observed.judgment?.asked === false
    ? t("flow.live.noJudgment", "판단 없음")
    : t("flow.live.judgment", "판단 {{ms}} · 확신 {{conf}}", {
      ms: flowMs(observed.judgment?.ms), conf: observed.judgment?.confidence ?? t("flow.live.unmeasured", "미측정"),
    })));
  card.append(flowText("p", t("flow.live.act", "누름 {{ms}}", { ms: flowMs(observed.act_ms) })));
  // The row owns its origin. A shared folder's newest walk (or a guessed
  // evidence_n) cannot lend its recipe, line or workspace to an older card.
  const origin = observed.retry;
  if (typeof origin?.name === "string" && origin.name
    && Number.isSafeInteger(origin.step) && origin.step > 0
    && typeof origin.cwd === "string" && origin.cwd) {
    card.append(flowButton(t("flow.live.retry", "이 걸음만 다시"), () => void flowLaunch(["recipe-run", "--name", origin.name, "--start", String(origin.step), "--end", String(origin.step)], `${origin.name} · ${t("flow.live.retry", "이 걸음만 다시")}`, origin.cwd), "btn flow-step-retry"));
  }
  return card;
}
function paintFlowSteps(run) {
  const host = el("flow-console-steps");
  if (host.dataset.dir !== run?.dir) {
    host.replaceChildren();
    host.dataset.dir = run?.dir ?? "";
  }
  const existing = new Map([...host.children].map((card) => [Number(card.dataset.n), card]));
  const ordered = [...(run?.steps.values() ?? [])].sort((a, b) => a.n - b.n);
  ordered.forEach((step, index) => {
    let card = existing.get(step.n);
    if (!card || card._step !== step || card._walk !== run.walk || card._locale !== locale) {
      const next = flowStepCard(run, step);
      next._step = step; next._walk = run.walk; next._locale = locale;
      if (card) card.replaceWith(next);
      card = next;
    }
    if (host.children[index] !== card) host.insertBefore(card, host.children[index] ?? null);
    existing.delete(step.n);
  });
  for (const card of existing.values()) card.remove();
}
function paintFlowConsole() {
  const run = flowRuns.get(flowRunDir);
  paintFlowMirrorPicker();
  el("flow-goal").placeholder = t("flow.live.goal", "이 화면에서 이룰 목표");
  el("flow-goal").setAttribute("aria-label", t("flow.live.goal", "이 화면에서 이룰 목표"));
  paintFlowSteps(run);
  el("flow-console-status").textContent = run?.walk?.stop?.kind ?? (run?.walk?.flow?.verdict
    ? (run.walk.flow.verdict.pass && run.walk.done && run.walk.complete !== false ? t("flow.pass", "합격") : t("flow.fail", "불합격")) : t("flow.noVerdict", "판정 없음"));
  const report = el("flow-console-report");
  report.hidden = !run?.report;
  report.textContent = t("flow.report", "리포트 열기");
  const queue = el("flow-console-queue");
  queue.replaceChildren();
  for (const item of flowPending) queue.append(flowText("p", item.label));
  for (const item of [...flowRuns.values()].reverse()) {
    queue.append(flowButton(item.name ?? item.dir, () => { flowConsoleReading += 1; flowRunDir = item.dir; paintFlowConsole(); }));
  }
  for (const flow of flowData?.flows ?? []) {
    const row = flowText("div", "", "flow-console-saved");
    row.append(flowButton(flow.name, () => void flowLaunch(["recipe-run", "--name", flow.slug], flow.name)));
    row.firstChild.disabled = Boolean((flowData.policies ?? []).find((option) => option.value === flow.policy)?.runnable?.ok === false);
    if (flow.last?.report) row.append(flowButton(t("flow.report", "리포트 열기"), () => void readFlowEvidence(flow.last.report)));
    queue.append(row);
  }
}
async function readFlowEvidence(report) {
  const reading = ++flowConsoleReading;
  try {
    const held = await invoke("flow_evidence", { report });
    if (reading !== flowConsoleReading) return;
    const run = flowRunView(held.dir);
    rememberFlowSteps(run, held.steps.slice(-FLOW_LIVE_STEPS));
    Object.assign(run, { report: held.report, walk: held.walk, name: held.walk?.name, kind: held.walk?.kind, cwd: held.walk?.cwd ?? null });
    flowRunDir = held.dir;
    paintFlowConsole();
  } catch (error) { showError(error); }
}
/* Requests wait in this view, then take the window's existing ComputerRequest
 * queue. Completion is the runner's reply, not a command typed into a shell. */
function flowLaunch(argv, label = argv[2], worktree = activeWorktreePath) {
  const item = { argv, label, worktree };
  flowPending.push(item);
  openFlowConsole();
  const launch = flowLaunchTail.then(() => sendFlowCommand(argv, worktree));
  flowLaunchTail = launch.catch(() => {});
  return launch.finally(() => {
    flowPending.splice(flowPending.indexOf(item), 1);
    scheduleFlowConsole();
  });
}
async function sendFlowCommand(argv, worktree) {
  try { await invoke("flow_execute", { argv, worktree }); }
  catch (error) { showError(error); }
}
el("flow-console-open").addEventListener("click", openFlowConsole);
el("flow-console-close").addEventListener("click", closeFlowConsole);
el("flow-console-report").addEventListener("click", () => {
  const report = flowRuns.get(flowRunDir)?.report;
  if (report) void openBrowserTab(pathAsFileUrl(report));
});
el("flow-goal-form").addEventListener("submit", (event) => {
  event.preventDefault();
  const goal = el("flow-goal").value.trim();
  const tab = currentTab();
  const app = flowBySlug(flowSelected)?.fingerprint?.apps?.[0];
  const target = tab?.kind === "emulator"
    ? tab.platform && tab.deviceId ? ["--platform", tab.platform, "--device", tab.deviceId] : null
    : tab?.kind === "browser" && tab.label ? ["--pane", tab.label] : app ? ["--app", app] : null;
  if (goal && target) void flowLaunch(["walk", "--goal", goal, ...target], goal);
  else showError(t("flow.live.target", "브라우저·기기 화면이나 Flow의 앱을 먼저 선택하세요."));
});

el("flow-console-mirror").addEventListener("change", (event) => {
  const mirror = flowMirrors().find((tab) => tab.id === event.target.value);
  if (mirror) { flowMirrorTab = mirror.id; setActiveTab(mirror.id); }
});
