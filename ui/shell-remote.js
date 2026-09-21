/* ---- SSH hosts ----------------------------------------------------------
 *
 * Public endpoint metadata comes from SettingsRepository; passwords never
 * cross back from the native vault. The renderer therefore owns only this
 * report and a short-lived form value. A server key must be observed before
 * authentication and is invalidated whenever its endpoint changes. */
const sshHostScrim = el("ssh-host-scrim");
let sshHostReport = { hosts: [] };
let sshHostLoading = false;
let sshHostError = null;
let sshHostGeneration = 0;
let sshHostMutation = null;
let sshHostProbeGeneration = 0;
let sshHostAuthentication = "agent";
const sshHostActions = new Map();

function normalizeSshHostReport(answer) {
  const rawHosts = Array.isArray(answer?.hosts) ? answer.hosts : [];
  return {
    hosts: rawHosts
      .filter((host) => host && typeof host === "object" && typeof host.id === "string")
      .map((host) => ({
        id: host.id,
        label: String(host.label ?? ""),
        host: String(host.host ?? ""),
        port: Number(host.port) || 22,
        user: String(host.user ?? ""),
        authentication: host.authentication === "password" ? "password" : "agent",
        key_algorithm: String(host.keyAlgorithm ?? host.key_algorithm ?? ""),
        encoded_key: String(host.encodedKey ?? host.encoded_key ?? ""),
        fingerprint: String(host.fingerprint ?? ""),
        credential_status: String(
          host.credentialStatus ?? host.credential_status ?? "unreadable",
        ),
      })),
  };
}

function sshHostEndpoint(host) {
  return `${host.user}@${host.host}:${host.port}`;
}

function sshHostCredentialCopy(host) {
  if (host.authentication === "agent") {
    return t("settings.sshHosts.agentReady", "SSH 에이전트 사용");
  }
  return {
    available: () => t("settings.sshHosts.passwordAvailable", "비밀번호 저장됨"),
    missing: () => t("settings.sshHosts.passwordMissing", "비밀번호 필요"),
    unreadable: () => t("settings.sshHosts.passwordUnreadable", "비밀번호를 읽을 수 없음"),
  }[host.credential_status]?.() ?? t("settings.sshHosts.passwordUnreadable", "비밀번호를 읽을 수 없음");
}

function sshHostCanConnect(host) {
  return host.authentication === "agent" || host.credential_status === "available";
}

function integrationActionButton(className, label, disabled, action) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = `btn ${className}`;
  button.disabled = disabled;
  say(button, () => label);
  button.addEventListener("click", action);
  return button;
}

function sshHostNode(host) {
  const row = document.createElement("div");
  row.className = "integration-site";
  row.dataset.hostId = host.id;
  row.setAttribute("role", "listitem");

  const identity = document.createElement("button");
  identity.type = "button";
  identity.className = "integration-site-pick ssh-host-edit";
  identity.dataset.hostId = host.id;
  identity.disabled = sshHostMutation !== null;
  say(identity, () => `${host.label} · ${sshHostEndpoint(host)}`);
  identity.addEventListener("click", () => openSshHostDialog(host));
  row.appendChild(identity);

  const summary = document.createElement("span");
  summary.className = "integration-site-summary";
  say(summary, () => {
    const action = sshHostActions.get(host.id);
    const state = action?.state === "connected"
      ? t("settings.integrations.connected", "연결됨")
      : sshHostCredentialCopy(host);
    return [host.fingerprint, state].filter(Boolean).join(" · ");
  });
  row.appendChild(summary);

  const actions = document.createElement("div");
  actions.className = "integration-site-actions";
  const busy = sshHostActions.get(host.id)?.busy === true;
  actions.appendChild(integrationActionButton(
    "ssh-host-test",
    busy
      ? t("settings.integrations.testing", "처리 중…")
      : t("settings.integrations.test", "연결 검사"),
    busy || sshHostMutation !== null || !sshHostCanConnect(host),
    () => void testSshHost(host),
  ));
  actions.appendChild(integrationActionButton(
    "ssh-host-open",
    t("settings.sshHosts.open", "터미널 열기"),
    busy || sshHostMutation !== null || !sshHostCanConnect(host),
    () => void openSshHostTerminal(host),
  ));
  actions.appendChild(integrationActionButton(
    "ssh-host-files", t("sftp.title", "SFTP 파일"),
    busy || sshHostMutation !== null, () => void openSftpManager(host),
  ));
  actions.appendChild(integrationActionButton(
    "btn--halt ssh-host-remove",
    t("settings.sshHosts.remove", "삭제"),
    busy || sshHostMutation !== null,
    () => void removeSshHost(host),
  ));
  row.appendChild(actions);
  return row;
}

function paintSshHosts() {
  const summary = el("ssh-hosts-summary");
  if (sshHostLoading) {
    summary.dataset.state = "checking";
    say(summary, () => t("settings.integrations.checking", "확인 중…"));
  } else if (sshHostError) {
    summary.dataset.state = "error";
    say(summary, () => t("settings.integrations.checkFailed", "확인 실패"));
  } else {
    summary.dataset.state = sshHostReport.hosts.length > 0 ? "connected" : "disconnected";
    say(summary, () => t("settings.sshHosts.count", "{{count}}개 호스트", {
      count: sshHostReport.hosts.length,
    }));
  }
  const error = el("ssh-hosts-error");
  error.hidden = !sshHostError;
  if (sshHostError) say(error, () => sshHostError);
  el("ssh-host-list").replaceChildren(...sshHostReport.hosts.map(sshHostNode));
  el("ssh-host-empty").hidden = sshHostLoading || sshHostReport.hosts.length > 0;
  el("ssh-host-refresh").disabled = sshHostLoading || sshHostMutation !== null;
  el("ssh-host-add").disabled = sshHostMutation !== null;
}

async function refreshSshHosts(preservedError = null) {
  if (sshHostMutation !== null) return false;
  const generation = ++sshHostGeneration;
  sshHostLoading = true;
  sshHostError = null;
  paintSshHosts();
  let refreshed = false;
  try {
    const answer = await invoke("ssh_hosts");
    if (generation !== sshHostGeneration) return false;
    sshHostReport = normalizeSshHostReport(answer);
    sshHostActions.clear();
    refreshed = true;
  } catch (error) {
    if (generation !== sshHostGeneration) return false;
    sshHostError = preservedError || String(error);
  }
  if (generation !== sshHostGeneration) return false;
  sshHostLoading = false;
  if (!sshHostError) sshHostError = preservedError;
  paintSshHosts();
  return refreshed;
}

function sshHostMatchesInput(host, input) {
  return (input.id === null || host.id === input.id)
    && host.label === input.label
    && host.host === input.host
    && host.port === input.port
    && host.user === input.user
    && host.authentication === input.authentication
    && host.key_algorithm === input.keyAlgorithm
    && host.encoded_key === input.encodedKey;
}

function sshHostsMeetExpectation(hosts, expectation) {
  if (expectation.saved) {
    return hosts.some((host) => sshHostMatchesInput(host, expectation.saved));
  }
  return !hosts.some((host) => host.id === expectation.removed);
}

async function reconcileSshHostMutation(error, expectation) {
  sshHostMutation = null;
  const refreshed = await refreshSshHosts(String(error));
  if (!refreshed || !sshHostsMeetExpectation(sshHostReport.hosts, expectation)) return false;
  sshHostError = null;
  paintSshHosts();
  return true;
}

function selectSshHostAuthentication(authentication) {
  sshHostAuthentication = authentication === "password" ? "password" : "agent";
  selectSegment("ssh-host-authentication", sshHostAuthentication);
  el("ssh-host-password-field").hidden = sshHostAuthentication !== "password";
  if (sshHostAuthentication === "agent") el("ssh-host-password").value = "";
  paintSshHostDialog();
}

function selectedSshHost() {
  const id = el("ssh-host-id").value;
  return sshHostReport.hosts.find((host) => host.id === id) ?? null;
}

function sshHostDialogCanSave() {
  const existing = selectedSshHost();
  const port = Number(el("ssh-host-port").value);
  const passwordRequired = sshHostAuthentication === "password"
    && (existing === null || existing.credential_status !== "available");
  return sshHostMutation === null
    && el("ssh-host-label").value.trim() !== ""
    && el("ssh-host-address").value.trim() !== ""
    && el("ssh-host-user").value.trim() !== ""
    && Number.isInteger(port) && port > 0 && port <= 65535
    && el("ssh-host-key-algorithm").value !== ""
    && el("ssh-host-encoded-key").value !== ""
    && (!passwordRequired || el("ssh-host-password").value !== "");
}

function paintSshHostDialog() {
  el("ssh-host-save").disabled = !sshHostDialogCanSave();
  el("ssh-host-probe").disabled = sshHostMutation === "probing"
    || el("ssh-host-address").value.trim() === ""
    || el("ssh-host-user").value.trim() === "";
  const hint = el("ssh-host-password-hint");
  hint.hidden = selectedSshHost()?.credential_status !== "available";
}

function clearSshHostDialogError() {
  const error = el("ssh-host-dialog-error");
  error.hidden = true;
  error.textContent = "";
}

function invalidateSshHostKey() {
  ++sshHostProbeGeneration;
  if (sshHostMutation === "probing") sshHostMutation = null;
  el("ssh-host-key-algorithm").value = "";
  el("ssh-host-encoded-key").value = "";
  el("ssh-host-fingerprint").value = "";
  clearSshHostDialogError();
  paintSshHostDialog();
}

function openSshHostDialog(host = null) {
  sshHostMutation = null;
  sshHostAuthentication = host?.authentication === "password" ? "password" : "agent";
  el("ssh-host-id").value = host?.id ?? "";
  el("ssh-host-label").value = host?.label ?? "";
  el("ssh-host-address").value = host?.host ?? "";
  el("ssh-host-user").value = host?.user ?? "";
  el("ssh-host-port").value = String(host?.port ?? 22);
  el("ssh-host-password").value = "";
  el("ssh-host-key-algorithm").value = host?.key_algorithm ?? "";
  el("ssh-host-encoded-key").value = host?.encoded_key ?? "";
  el("ssh-host-fingerprint").value = host?.fingerprint ?? "";
  selectSegment("ssh-host-authentication", sshHostAuthentication);
  el("ssh-host-password-field").hidden = sshHostAuthentication !== "password";
  say(el("ssh-host-dialog-title"), () => host
    ? t("settings.sshHosts.editTitle", "SSH 호스트 편집")
    : t("settings.sshHosts.add", "SSH 호스트 추가"));
  clearSshHostDialogError();
  paintSshHostDialog();
  showModal(sshHostScrim);
}

function closeSshHostDialog() {
  ++sshHostProbeGeneration;
  sshHostMutation = null;
  el("ssh-host-password").value = "";
  clearSshHostDialogError();
  hideModal(sshHostScrim);
  paintSshHosts();
}

function reportSshHostDialogError(error) {
  const message = String(error);
  const output = el("ssh-host-dialog-error");
  say(output, () => message);
  output.hidden = false;
}

async function probeSshHostKey() {
  const generation = ++sshHostProbeGeneration;
  sshHostMutation = "probing";
  clearSshHostDialogError();
  paintSshHostDialog();
  try {
    const key = await invoke("probe_ssh_host", {
      input: {
        host: el("ssh-host-address").value.trim(),
        port: Number(el("ssh-host-port").value),
        user: el("ssh-host-user").value.trim(),
      },
    });
    if (generation !== sshHostProbeGeneration) return;
    el("ssh-host-key-algorithm").value = String(key?.keyAlgorithm ?? "");
    el("ssh-host-encoded-key").value = String(key?.encodedKey ?? "");
    el("ssh-host-fingerprint").value = String(key?.fingerprint ?? "");
  } catch (error) {
    if (generation !== sshHostProbeGeneration) return;
    reportSshHostDialogError(error);
  } finally {
    if (generation === sshHostProbeGeneration) {
      sshHostMutation = null;
      paintSshHostDialog();
    }
  }
}

async function saveSshHost() {
  if (!sshHostDialogCanSave()) return;
  const generation = ++sshHostGeneration;
  sshHostMutation = "saving";
  clearSshHostDialogError();
  paintSshHostDialog();
  const input = {
    id: el("ssh-host-id").value || null,
    label: el("ssh-host-label").value.trim(),
    host: el("ssh-host-address").value.trim(),
    port: Number(el("ssh-host-port").value),
    user: el("ssh-host-user").value.trim(),
    authentication: sshHostAuthentication,
    keyAlgorithm: el("ssh-host-key-algorithm").value,
    encodedKey: el("ssh-host-encoded-key").value,
  };
  try {
    const password = el("ssh-host-password").value;
    const answer = await invoke("save_ssh_host", {
      input: {
        ...input,
        password: password === "" ? null : password,
      },
    });
    if (generation !== sshHostGeneration) return;
    sshHostReport = normalizeSshHostReport(answer);
    closeSshHostDialog();
  } catch (error) {
    if (generation !== sshHostGeneration) return;
    if (await reconcileSshHostMutation(error, { saved: input })) {
      closeSshHostDialog();
      return;
    }
    reportSshHostDialogError(error);
    paintSshHostDialog();
  } finally {
    el("ssh-host-password").value = "";
  }
}

async function removeSshHost(host) {
  if (sshHostMutation !== null) return;
  const generation = ++sshHostGeneration;
  sshHostMutation = host.id;
  sshHostError = null;
  paintSshHosts();
  try {
    const answer = await invoke("remove_ssh_host", { id: host.id });
    if (generation !== sshHostGeneration) return;
    sshHostReport = normalizeSshHostReport(answer);
  } catch (error) {
    if (generation !== sshHostGeneration) return;
    await reconcileSshHostMutation(error, { removed: host.id });
    return;
  }
  if (generation !== sshHostGeneration) return;
  sshHostMutation = null;
  paintSshHosts();
}

async function testSshHost(host) {
  if (!sshHostCanConnect(host) || sshHostActions.get(host.id)?.busy) return;
  sshHostActions.set(host.id, { busy: true });
  sshHostError = null;
  paintSshHosts();
  try {
    await invoke("test_ssh_host", { id: host.id });
    sshHostActions.set(host.id, { state: "connected" });
  } catch (error) {
    sshHostActions.delete(host.id);
    sshHostError = String(error);
  }
  paintSshHosts();
}

async function openSshHostTerminal(host) {
  if (!sshHostCanConnect(host) || sshHostActions.get(host.id)?.busy) return;
  sshHostActions.set(host.id, { busy: true });
  sshHostError = null;
  paintSshHosts();
  try {
    const term = await invoke("open_ssh_terminal", {
      id: host.id,
      rows: 24,
      cols: 96,
    });
    sshHostActions.delete(host.id);
    setSettingsOpen(false);
    mountTermTab(term, {
      label: host.label,
      remoteHost: host.id,
    }, { placement: "tab" });
  } catch (error) {
    sshHostActions.delete(host.id);
    sshHostError = String(error);
    paintSshHosts();
  }
}

el("ssh-host-refresh").addEventListener("click", () => void refreshSshHosts());
el("ssh-host-add").addEventListener("click", () => openSshHostDialog());
el("ssh-host-cancel").addEventListener("click", closeSshHostDialog);
el("ssh-host-probe").addEventListener("click", () => void probeSshHostKey());
el("ssh-host-save").addEventListener("click", () => void saveSshHost());
sshHostScrim.addEventListener("mousedown", (event) => {
  if (event.target === sshHostScrim && sshHostMutation === null) closeSshHostDialog();
});
el("ssh-host-authentication").addEventListener("click", (event) => {
  const button = event.target.closest(".segment-btn");
  if (button) selectSshHostAuthentication(button.dataset.value);
});
for (const id of ["ssh-host-label", "ssh-host-password"]) {
  el(id).addEventListener("input", () => {
    clearSshHostDialogError();
    paintSshHostDialog();
  });
}
for (const id of ["ssh-host-address", "ssh-host-user", "ssh-host-port"]) {
  el(id).addEventListener("input", invalidateSshHostKey);
}

/* ---- SSH targets ----------------------------------------------------------
 *
 * Orca's `SshPane`: the machines this window can be asked to run on. The list
 * and the form are this file's; no target here can be connected to yet, which
 * is why every card reads the same disconnected word.
 *
 * The list has TWO authors. One is the form. The other is `~/.ssh/config`,
 * which most people who have machines have already written, and which is read
 * the moment this pane appears — silently, because nobody pressed anything.
 * 가져오기 is the same read with the amnesty turned on: it forgives every
 * alias somebody deleted and adopts them back. Only the pressed one speaks.
 *
 * Rust is the gate. The form checks what it can so a refusal lands on the
 * field that caused it, but `ssh_save_target` re-checks everything and a
 * draft that slipped past this file is still refused where it is written —
 * which is why the error line below maps the backend's CODES rather than
 * matching its prose. Which rows the file may rewrite is Rust's too: a row
 * somebody typed is never one of them.
 *
 * A target is configuration, not a credential: the key file is named by PATH
 * and never read here, and a passphrase belongs to a connection attempt and
 * to memory. That is why this form has no field that could hold a secret. */
const sshTargetScrim = el("ssh-target-scrim");
let sshTargets = [];
let sshTargetLoading = false;
let sshTargetError = null;
let sshTargetGeneration = 0;
let sshTargetMutation = null;
let sshTargetSyncing = false;
/* One probe answer per card. A test never changes connection state (measured:
 * Orca's bare probe suppresses lifecycle broadcasts), so what it learned is a
 * note on the card, not a state the dot may claim. */
const sshTargetProbes = new Map();

/* What each target's connection is doing. Filled once when the pane opens,
 * moved by `ssh:link-changed` events after — the backend's book is the truth
 * and this is the mirror the cards read. */
const sshLinkStates = new Map();

function sshLinkOf(id) {
  return sshLinkStates.get(id) ?? { state: "disconnected", error: null };
}

/* Nine rungs on the measured reconnection ladder — the denominator the
 * reconnecting state counts against. */
const SSH_RECONNECT_RUNGS = 9;

/* The dot's word for each lifecycle state Orca names. Reconnecting carries
 * its attempt count when the event brought one. */
function sshLinkStateSaid(link) {
  if (link.state === "connecting") return t("settings.ssh.connecting", "연결 중…");
  if (link.state === "connected") return t("settings.ssh.connected", "연결됨");
  if (link.state === "reconnecting") {
    const word = t("settings.ssh.reconnecting", "재연결 중…");
    const attempt = Number(link.attempt);
    return Number.isInteger(attempt) && attempt > 0
      ? `${word} (${attempt}/${SSH_RECONNECT_RUNGS})`
      : word;
  }
  if (link.state === "reconnection-failed") {
    return t("settings.ssh.reconnectionFailed", "재연결 실패");
  }
  if (link.state === "auth-failed") return t("settings.ssh.authFailed", "인증 실패");
  if (link.state === "error") return t("settings.ssh.linkError", "오류");
  return t("settings.ssh.disconnected", "연결 안 됨");
}

async function refreshSshLinkStates() {
  try {
    const answer = await invoke("ssh_link_states");
    sshLinkStates.clear();
    for (const report of Array.isArray(answer) ? answer : []) {
      if (typeof report?.id === "string") sshLinkStates.set(report.id, report);
    }
  } catch {
    /* A window that cannot ask still paints: silence reads as disconnected. */
  }
}

/* Connect and Disconnect share one shape. State moves by EVENTS while the
 * link lives, and the REPLY is the state the moment the command returns —
 * the dial answers with its own report, so the row wears it at once instead
 * of waiting for an event that may already have passed it by (2026-09-07:
 * three masters stood since 11:22 while the sidebar still said 연결 안 됨).
 * A refusal lands on the card's note line AND on the row, as an error state
 * carrying its sentence. A fresh dial retires the old probe note, so a stale
 * "연결 성공" can never sit on top of the refusal the dial is about to
 * report. Answers the report, or null when the command refused. */
async function askSshLink(command, target) {
  sshTargetProbes.delete(target.id);
  try {
    const report = await invoke(command, { id: target.id });
    if (typeof report?.id === "string" && typeof report?.state === "string") {
      sshLinkStates.set(report.id, report);
    }
    paintSshLinkSurfaces();
    return report ?? null;
  } catch (error) {
    sshTargetProbes.set(target.id, { running: false, said: String(error), tone: "halt" });
    sshLinkStates.set(target.id, { id: target.id, state: "error", error: String(error) });
    paintSshLinkSurfaces();
    return null;
  }
}

/* Every surface that wears a link state, in one call — the settings cards
 * when that pane is open, and the sidebar rows always. The event listener
 * and the reply road both end here, so the two can never disagree. */
function paintSshLinkSurfaces() {
  if (settingsPane === "ssh" && !settingsView.hidden) paintSshTargets();
  paintSidebarSshHosts();
}

/* A terminal ON the target, as one more tab. The terminal is the point:
 * the settings sheet steps aside the moment the tab exists. */
async function openSshRemoteTerm(target) {
  let term;
  try {
    term = await invoke("ssh_open_remote_term", { id: target.id, rows: 24, cols: 96 });
  } catch (error) {
    sshTargetProbes.set(target.id, { running: false, said: String(error), tone: "halt" });
    paintSshLinkSurfaces();
    return;
  }
  setSettingsOpen(false);
  mountTermTab(term, {}, { placement: "tab" });
}

/* The sidebar's machines (recon §D: hosts are a sidebar presence, not only a
 * settings page). Same targets, same link states as the settings cards; the
 * row is a door — a standing connection opens a terminal, a resting one
 * dials, and a dial already under way is left to finish. */
let sidebarSshTargets = [];
let sshHostsFolded = false;

async function refreshSidebarSshHosts() {
  try {
    sidebarSshTargets = normalizeSshTargets(await invoke("ssh_targets"));
  } catch {
    sidebarSshTargets = [];
  }
  // The backend's book, not this window's memory: a master that a previous
  // click (or a previous window) left standing wears its state from the
  // first paint, instead of "연결 안 됨" until the next event happens by.
  await refreshSshLinkStates();
  paintSidebarSshHosts();
}
void refreshSidebarSshHosts();

function sshHostRowNode(target) {
  // A BUTTON, not a wired div: the keyboard reaches it for free, which is
  // the whole point of the row-wiring gate this deliberately isn't.
  const door = document.createElement("button");
  door.type = "button";
  door.className = "ssh-host-row";
  door.dataset.targetId = target.id;
  door.setAttribute("role", "listitem");
  const link = sshLinkOf(target.id);
  const dot = document.createElement("span");
  dot.className = "ssh-host-dot";
  dot.dataset.state = link.state;
  const name = document.createElement("span");
  name.className = "ssh-host-name";
  say(name, () => target.label || sshTargetEndpoint(target));
  const word = document.createElement("span");
  word.className = "ssh-host-word";
  say(word, () => sshLinkStateSaid(link));
  // The sentence behind a refusal rides the row as its tooltip, so a person
  // reads WHY without opening the settings sheet — `data-tip`, the tooltip
  // this window draws itself, never a native `title`.
  const note = sshTargetProbes.get(target.id);
  const why = link.error || (note?.tone === "halt" ? note.said : "");
  if (why) door.dataset.tip = why;
  door.append(dot, name, word);
  // One door, as Orca's is: a press on a resting host dials it and, the
  // moment the dial answers connected, opens the terminal ON it — nobody
  // has to press twice. A standing host opens the terminal at once; a host
  // already on its way is left to arrive.
  door.addEventListener("click", async () => {
    if (link.state === "connecting" || link.state === "reconnecting") return;
    if (link.state !== "connected") {
      const report = await askSshLink("ssh_link_connect", target);
      if (report?.state !== "connected") return;
    }
    await openSshRemoteTerm(target);
  });
  return door;
}

function paintSidebarSshHosts() {
  const stack = el("ssh-hosts-stack");
  stack.hidden = sidebarSshTargets.length === 0;
  const rows = el("ssh-hosts-rows");
  rows.hidden = sshHostsFolded;
  el("ssh-hosts-head").setAttribute("aria-expanded", String(!sshHostsFolded));
  el("ssh-hosts-head").querySelector(".icon").classList.toggle("is-open", !sshHostsFolded);
  rows.replaceChildren(...(sshHostsFolded ? [] : sidebarSshTargets.map(sshHostRowNode)));
}

/* Which of the two ways a target names its machine is in force. An alias is
 * the user's own `~/.ssh/config` entry and outranks the address beside it,
 * exactly as the backend resolves it. */
function sshTargetHost(target) {
  return target.configHost || target.host || "";
}

/* The second line of a card: where this goes, and the key file when one is
 * named. Orca puts the timeout here too; that belongs to the relay, which
 * arrives with the connection lifecycle. */
function sshTargetEndpoint(target) {
  const user = target.username ? `${target.username}@` : "";
  const port = Number(target.port) > 0 ? Number(target.port) : 22;
  const host = sshTargetHost(target);
  const where = `${user}${host}:${port}`;
  return target.identityFile ? `${where} • ${target.identityFile}` : where;
}

/* A target the window can draw. The backend normalizes before it answers, so
 * this only refuses a shape that is not a target at all. */
function normalizeSshTargets(answer) {
  return Array.isArray(answer)
    ? answer.filter((target) => target !== null
      && typeof target === "object"
      && typeof target.id === "string")
    : [];
}

/* Read a host field the way a person actually types one.
 *
 * Pure, and separate from the field it serves, because this is the part with
 * cases: `ssh://` URLs, `user@host:port` split at the LAST `@` so a username
 * containing one survives, a bracketed IPv6 literal, and a bare address. The
 * answer says only what it FOUND — `null` for a part the text did not carry —
 * and the caller decides what may overwrite what.
 *
 * `error` is a code, never a sentence: the field that reports it is the one
 * that knows which language it is speaking. */
function parseSshHostInput(raw) {
  const text = String(raw ?? "").trim();
  const nothing = { host: text, username: null, port: null, error: null };
  if (text === "") return { ...nothing, host: "" };
  if (/^ssh:\/\//i.test(text)) {
    let url = null;
    try {
      url = new URL(text);
    } catch {
      // The WHATWG parser refuses a port above 65535 and a malformed
      // authority the same way, and both are the same repair for a person:
      // look at what you typed.
      return { ...nothing, error: "url" };
    }
    const host = sshBareHost(url.hostname);
    if (host === "") return { ...nothing, error: "url" };
    const port = url.port === "" ? null : sshPortOf(url.port);
    if (url.port !== "" && port === null) return { ...nothing, error: "port" };
    return {
      host,
      username: url.username === "" ? null : decodeURIComponent(url.username),
      port,
      error: null,
    };
  }

  const at = text.lastIndexOf("@");
  const username = at === -1 ? null : text.slice(0, at);
  const rest = at === -1 ? text : text.slice(at + 1);
  if (at !== -1 && (username === "" || rest === "")) return nothing;

  // `[::1]:2222` — brackets are what let a colon-bearing address carry a
  // port at all, so they are read here and dropped from what is stored.
  if (rest.startsWith("[")) {
    const closes = rest.indexOf("]");
    if (closes === -1) return nothing;
    const host = rest.slice(1, closes);
    const tail = rest.slice(closes + 1);
    const port = tail.startsWith(":") ? sshPortOf(tail.slice(1)) : null;
    if (host === "" || (tail !== "" && !tail.startsWith(":")) || (tail !== "" && port === null)) {
      return nothing;
    }
    return { host, username, port, error: null };
  }

  // One colon splits a port off; several mean a bare IPv6 literal, which
  // cannot carry one without the brackets above.
  const colon = rest.indexOf(":");
  if (colon !== -1 && colon === rest.lastIndexOf(":")) {
    const port = sshPortOf(rest.slice(colon + 1));
    const host = rest.slice(0, colon);
    if (host !== "" && port !== null) return { host, username, port, error: null };
  }
  return { host: rest, username, port: null, error: null };
}

function sshPortOf(text) {
  if (!/^\d+$/.test(text)) return null;
  const port = Number(text);
  return port >= 1 && port <= 65535 ? port : null;
}

function sshBareHost(host) {
  return host.startsWith("[") && host.endsWith("]") ? host.slice(1, -1) : host;
}

async function runSshTargetProbe(target) {
  sshTargetProbes.set(target.id, { running: true, said: null, tone: null });
  paintSshTargets();
  try {
    await invoke("ssh_probe_target", { id: target.id });
    sshTargetProbes.set(target.id, { running: false, said: null, tone: "ready" });
  } catch (error) {
    sshTargetProbes.set(target.id, { running: false, said: String(error), tone: "halt" });
  }
  paintSshTargets();
}

function sshTargetAction(name, glyph, tip, act) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = `btn ssh-target-act ${name}`;
  button.disabled = sshTargetMutation !== null;
  // `data-tip`, not `title`: the tooltip this window draws is its own.
  button.dataset.tip = tip;
  button.setAttribute("aria-label", tip);
  button.innerHTML = icon(glyph);
  button.addEventListener("click", act);
  return button;
}

function sshTargetNode(target) {
  const row = document.createElement("div");
  row.className = "ssh-target-row";
  row.dataset.targetId = target.id;
  row.setAttribute("role", "listitem");

  const mark = document.createElement("span");
  mark.className = "ssh-target-mark";
  mark.innerHTML = icon("server");
  row.appendChild(mark);

  const identity = document.createElement("div");
  identity.className = "ssh-target-identity";
  const name = document.createElement("p");
  name.className = "ssh-target-name";
  const label = document.createElement("span");
  label.className = "ssh-target-label";
  say(label, () => target.label || sshTargetEndpoint(target));
  const link = sshLinkOf(target.id);
  const dot = document.createElement("span");
  dot.className = "ssh-target-dot";
  dot.dataset.state = link.state;
  const state = document.createElement("span");
  state.className = "ssh-target-state";
  say(state, () => sshLinkStateSaid(link));
  name.append(label, dot, state);
  const endpoint = document.createElement("span");
  endpoint.className = "ssh-target-endpoint";
  say(endpoint, () => sshTargetEndpoint(target));
  identity.append(name, endpoint);
  const note = sshTargetProbes.get(target.id)
    ?? (link.error ? { running: false, said: link.error, tone: "halt" } : null);
  if (note) {
    const answer = document.createElement("span");
    answer.className = "ssh-target-probe";
    answer.dataset.tone = note.running ? "wait" : note.tone;
    if (note.running) say(answer, () => t("settings.ssh.testing", "테스트 중…"));
    else if (note.tone === "ready") say(answer, () => t("settings.ssh.testOk", "연결 성공"));
    else say(answer, () => note.said ?? "");
    identity.appendChild(answer);
  }
  row.appendChild(identity);

  const acts = document.createElement("div");
  acts.className = "ssh-target-acts";
  // The card's face follows the lifecycle (measured): a dial under way — or a
  // ladder mid-climb — offers only Edit and Remove; a standing connection
  // offers Disconnect; everything else may Test or Connect.
  const busy = link.state === "connecting" || link.state === "reconnecting";
  if (!busy && link.state !== "connected") {
    const test = sshTargetAction(
      "ssh-target-test",
      "check",
      t("settings.ssh.test", "연결 테스트"),
      () => void runSshTargetProbe(target),
    );
    if (sshTargetProbes.get(target.id)?.running) test.disabled = true;
    acts.append(test, sshTargetAction(
      "ssh-target-connect",
      "plug",
      t("settings.ssh.connect", "연결"),
      () => void askSshLink("ssh_link_connect", target),
    ));
  }
  if (link.state === "connected") {
    acts.append(sshTargetAction(
      "ssh-target-term",
      "terminal",
      t("settings.ssh.openTerm", "원격 터미널"),
      () => void openSshRemoteTerm(target),
    ), sshTargetAction(
      "ssh-target-disconnect",
      "circle-x",
      t("settings.ssh.disconnect", "연결 해제"),
      () => void askSshLink("ssh_link_disconnect", target),
    ));
  }
  acts.append(
    sshTargetAction("ssh-target-files", "folder", t("sftp.title", "SFTP 파일"), () => void openSftpTarget(target)),
    sshTargetAction(
      "ssh-target-edit",
      "pencil",
      t("settings.ssh.edit", "대상 편집"),
      () => openSshTargetDialog(target),
    ),
    sshTargetAction(
      "ssh-target-remove",
      "trash",
      t("settings.ssh.remove", "대상 제거"),
      () => void requestRemoveSshTarget(target),
    ),
  );
  row.appendChild(acts);
  return row;
}

function paintSshTargets() {
  const error = el("ssh-target-error");
  error.hidden = sshTargetError === null;
  if (sshTargetError !== null) say(error, () => sshTargetError ?? "");
  el("ssh-target-list").replaceChildren(...sshTargets.map(sshTargetNode));
  el("ssh-target-empty").hidden = sshTargetLoading || sshTargets.length > 0;
  el("ssh-target-add").disabled = sshTargetMutation !== null;
  el("ssh-target-import").disabled = sshTargetMutation !== null || sshTargetSyncing;
}

async function refreshSshTargets() {
  if (sshTargetMutation !== null) return;
  const generation = ++sshTargetGeneration;
  sshTargetLoading = true;
  sshTargetError = null;
  paintSshTargets();
  try {
    const answer = await invoke("ssh_targets");
    if (generation !== sshTargetGeneration) return;
    sshTargets = normalizeSshTargets(answer);
    for (const id of [...sshTargetProbes.keys()]) {
      if (!sshTargets.some((target) => target.id === id)) sshTargetProbes.delete(id);
    }
  } catch (error) {
    if (generation !== sshTargetGeneration) return;
    sshTargetError = String(error);
  }
  sshTargetLoading = false;
  paintSshTargets();
}

/* Let `~/.ssh/config` catch the list up.
 *
 * `reAdopt` is the whole difference between the two ways this runs. The
 * silent pass — every time the pane opens — respects the aliases somebody
 * deleted, and says NOTHING: a machine with no config file is the ordinary
 * case, not a failure worth a pill in the corner. The button forgives those
 * aliases and reports, because a person who presses a button is owed an
 * answer even when the answer is "nothing moved".
 *
 * The list is re-read rather than taken from the reply: `ssh_targets` is the
 * one command that says what the targets are, and a second shape carrying
 * them would be a second thing to keep true. */
async function syncSshTargetsFromConfig(reAdopt) {
  // One at a time. Leaving and re-entering the pane while a read is in
  // flight would otherwise run the sync against a list it already changed.
  if (sshTargetSyncing || sshTargetMutation !== null) return;
  sshTargetSyncing = true;
  paintSshTargets();
  try {
    const answer = await invoke("ssh_import_config", { reAdopt });
    const changed = Number(answer?.changed) || 0;
    // Down before the repaint: the read is over, and what follows is the
    // list catching up with what it found.
    sshTargetSyncing = false;
    if (changed > 0) await refreshSshTargets();
    if (reAdopt) {
      toast(changed > 0
        ? t("settings.ssh.importSyncedN", "{{n}}개 서버를 동기화했습니다", { n: changed })
        : t("settings.ssh.importSynced", "~/.ssh/config와 이미 동기화되어 있습니다"));
    }
  } catch {
    if (reAdopt) toast(t("settings.ssh.importFailed", "가져오기에 실패했습니다"), "halt");
  } finally {
    sshTargetSyncing = false;
    paintSshTargets();
  }
}

/* What opening the pane does, in the order a person reads it: the rows that
 * are already stored, and then whatever the file has to add to them. */
async function enterSshTargetsPane() {
  await refreshSshLinkStates();
  await refreshSshTargets();
  await syncSshTargetsFromConfig(false);
}

/* The backend answers with a CODE so the two sides do not have to agree on a
 * sentence. Anything not listed here is a fault rather than a refusal, and is
 * shown as it arrived — a message nobody can read is still better than one
 * this file invented. */
function sshTargetRefusal(error) {
  const code = String(error?.message ?? error).trim();
  if (code === "host_required") {
    return t("settings.ssh.errHost", "호스트 또는 SSH config 별칭이 필요합니다");
  }
  if (code === "grace_out_of_bounds") {
    return t("settings.ssh.errGrace", "시간 제한은 60초에서 604800초 사이여야 합니다");
  }
  if (code === "port_out_of_range") {
    return t("settings.ssh.errPort", "포트는 1에서 65535 사이여야 합니다");
  }
  return code;
}

function reportSshTargetDialogError(said) {
  const output = el("ssh-target-dialog-error");
  say(output, () => said);
  output.hidden = false;
}

function clearSshTargetDialogError() {
  const output = el("ssh-target-dialog-error");
  output.hidden = true;
  say(output, () => "");
}

/* Keep-alive and a countdown are one answer with two spellings, so the
 * toggle owns the field: while terminals are kept until reset the seconds
 * mean nothing and the box says so rather than sitting there editable. */
function paintSshTargetGrace() {
  const kept = el("ssh-target-keep-alive").checked;
  const grace = el("ssh-target-grace");
  grace.disabled = kept;
  if (kept) grace.value = "";
  else if (grace.value.trim() === "") grace.value = "86400";
}

function openSshTargetDialog(target = null) {
  sshTargetMutation = null;
  el("ssh-target-id").value = target?.id ?? "";
  el("ssh-target-label").value = target?.label ?? "";
  el("ssh-target-host").value = target ? sshTargetHost(target) : "";
  el("ssh-target-username").value = target?.username ?? "";
  el("ssh-target-port").value = String(target?.port ?? 22);
  el("ssh-target-identity").value = target?.identityFile ?? "";
  el("ssh-target-proxy").value = target?.proxyCommand ?? "";
  el("ssh-target-jump").value = target?.jumpHost ?? "";
  el("ssh-target-reuse").checked = target?.systemSshConnectionReuse ?? true;
  el("ssh-target-keep-alive").checked = target?.relayKeepAliveUntilReset ?? true;
  el("ssh-target-grace").value = target?.relayKeepAliveUntilReset === false
    ? String(target.relayGracePeriodSeconds || 86400)
    : "";
  // Advanced opens for a target that HAS something advanced set, so an edit
  // never hides a field the person is about to look for.
  el("ssh-target-advanced").open = Boolean(
    target?.proxyCommand || target?.jumpHost || target?.relayKeepAliveUntilReset === false,
  );
  paintSshTargetGrace();
  say(el("ssh-target-dialog-title"), () => (target
    ? t("settings.ssh.edit", "대상 편집")
    : t("settings.ssh.add", "대상 추가")));
  clearSshTargetDialogError();
  showModal(sshTargetScrim, { animated: true });
}

function closeSshTargetDialog() {
  sshTargetMutation = null;
  clearSshTargetDialogError();
  hideModal(sshTargetScrim, { animated: true });
  paintSshTargets();
}

/* On blur, because a parse that ran per keystroke would rewrite the field
 * under the person still typing into it. The port is only taken when the one
 * on screen is the default nobody chose — a 2222 somebody typed outranks a
 * `:22` pasted after it. */
function readSshTargetHostField() {
  const field = el("ssh-target-host");
  const parsed = parseSshHostInput(field.value);
  if (parsed.error !== null) {
    reportSshTargetDialogError(t("settings.ssh.errHostFormat", "이 주소를 읽지 못했습니다"));
    return;
  }
  clearSshTargetDialogError();
  field.value = parsed.host;
  if (parsed.username !== null) el("ssh-target-username").value = parsed.username;
  if (parsed.port !== null) {
    const draft = el("ssh-target-port").value.trim();
    if (draft === "" || draft === "22") el("ssh-target-port").value = String(parsed.port);
  }
}

function sshTargetDraft() {
  const kept = el("ssh-target-keep-alive").checked;
  const grace = Number(el("ssh-target-grace").value);
  return {
    id: el("ssh-target-id").value,
    label: el("ssh-target-label").value.trim(),
    host: el("ssh-target-host").value.trim(),
    configHost: "",
    port: Number(el("ssh-target-port").value) || 22,
    username: el("ssh-target-username").value.trim(),
    identityFile: el("ssh-target-identity").value.trim(),
    proxyCommand: el("ssh-target-proxy").value.trim(),
    jumpHost: el("ssh-target-jump").value.trim(),
    systemSshConnectionReuse: el("ssh-target-reuse").checked,
    relayGracePeriodSeconds: kept || !Number.isFinite(grace) ? 0 : grace,
    relayKeepAliveUntilReset: kept,
    // Everything this pane writes is a person's own row. `~/.ssh/config`
    // import is the only other writer there will be, and it must never take
    // one of these over.
    source: "manual",
  };
}

async function saveSshTarget() {
  if (sshTargetMutation !== null) return;
  const generation = ++sshTargetGeneration;
  sshTargetMutation = "saving";
  clearSshTargetDialogError();
  el("ssh-target-save").disabled = true;
  try {
    const answer = await invoke("ssh_save_target", { target: sshTargetDraft() });
    if (generation !== sshTargetGeneration) return;
    sshTargets = normalizeSshTargets(answer);
    sshTargetError = null;
    closeSshTargetDialog();
  } catch (error) {
    if (generation !== sshTargetGeneration) return;
    sshTargetMutation = null;
    reportSshTargetDialogError(sshTargetRefusal(error));
  } finally {
    if (generation === sshTargetGeneration) {
      sshTargetMutation = null;
      el("ssh-target-save").disabled = false;
    }
  }
}

async function requestRemoveSshTarget(target) {
  if (sshTargetMutation !== null) return;
  const said = await askConfirm({
    title: t("settings.ssh.removeTitle", "SSH 대상 제거"),
    body: t("settings.ssh.removeBody", "이 대상을 목록에서 제거합니다."),
    confirm: t("settings.ssh.remove", "대상 제거"),
    deny: t("app.cancel", "취소"),
    danger: true,
  });
  if (said !== true) return;
  const generation = ++sshTargetGeneration;
  sshTargetMutation = target.id;
  sshTargetError = null;
  paintSshTargets();
  try {
    const answer = await invoke("ssh_remove_target", { id: target.id });
    if (generation !== sshTargetGeneration) return;
    sshTargets = normalizeSshTargets(answer);
  } catch (error) {
    if (generation !== sshTargetGeneration) return;
    sshTargetError = sshTargetRefusal(error);
  }
  if (generation !== sshTargetGeneration) return;
  sshTargetMutation = null;
  paintSshTargets();
}

el("ssh-target-add").addEventListener("click", () => openSshTargetDialog());
el("ssh-target-import").addEventListener("click", () => void syncSshTargetsFromConfig(true));
el("ssh-target-cancel").addEventListener("click", closeSshTargetDialog);
el("ssh-target-save").addEventListener("click", () => void saveSshTarget());
el("ssh-target-host").addEventListener("blur", readSshTargetHostField);
el("ssh-target-keep-alive").addEventListener("change", paintSshTargetGrace);
sshTargetScrim.addEventListener("mousedown", (event) => {
  if (event.target === sshTargetScrim && sshTargetMutation === null) closeSshTargetDialog();
});


/* ---- Remote workspaces -------------------------------------------------
 *
 * A saved row is only a host UUID plus a server-canonical POSIX root. The
 * backend re-verifies that root through SFTP before every save, test and open;
 * this renderer never turns a local Path or an unchecked string into a
 * workspace identity. */
const remoteWorkspaceScrim = el("remote-workspace-scrim");
let remoteWorkspaceReport = { workspaces: [] };
let remoteWorkspaceLoading = false;
let remoteWorkspaceError = null;
let remoteWorkspaceGeneration = 0;
let remoteWorkspaceProbeGeneration = 0;
let remoteWorkspaceMutation = null;
const remoteWorkspaceActions = new Map();

function normalizeRemoteWorkspaceReport(answer) {
  const rows = Array.isArray(answer?.workspaces) ? answer.workspaces : [];
  return {
    workspaces: rows
      .filter((workspace) => workspace
        && typeof workspace === "object"
        && typeof workspace.id === "string")
      .map((workspace) => ({
        id: workspace.id,
        label: String(workspace.label ?? ""),
        host_id: String(workspace.hostId ?? workspace.host_id ?? ""),
        host_label: String(workspace.hostLabel ?? workspace.host_label ?? ""),
        root: String(workspace.root ?? ""),
      })),
  };
}

function remoteWorkspaceHost(workspace) {
  return sshHostReport.hosts.find((host) => host.id === workspace.host_id) ?? null;
}

function remoteWorkspaceNode(workspace) {
  const row = document.createElement("div");
  row.className = "integration-site";
  row.dataset.workspaceId = workspace.id;
  row.setAttribute("role", "listitem");

  const identity = document.createElement("button");
  identity.type = "button";
  identity.className = "integration-site-pick remote-workspace-edit";
  identity.disabled = remoteWorkspaceMutation !== null;
  say(identity, () => `${workspace.label} · ${workspace.host_label}`);
  identity.addEventListener("click", () => openRemoteWorkspaceDialog(workspace));
  row.appendChild(identity);

  const summary = document.createElement("span");
  summary.className = "integration-site-summary";
  say(summary, () => {
    const connected = remoteWorkspaceActions.get(workspace.id)?.state === "connected"
      ? t("settings.integrations.connected", "연결됨")
      : null;
    return [workspace.root, connected].filter(Boolean).join(" · ");
  });
  row.appendChild(summary);

  const actions = document.createElement("div");
  actions.className = "integration-site-actions";
  const busy = remoteWorkspaceActions.get(workspace.id)?.busy === true;
  const connectable = sshHostCanConnect(remoteWorkspaceHost(workspace) ?? {});
  actions.appendChild(integrationActionButton(
    "remote-workspace-test",
    busy
      ? t("settings.integrations.testing", "처리 중…")
      : t("settings.integrations.test", "연결 검사"),
    busy || remoteWorkspaceMutation !== null || !connectable,
    () => void testRemoteWorkspace(workspace),
  ));
  actions.appendChild(integrationActionButton(
    "remote-workspace-open",
    t("settings.remoteWorkspaces.open", "터미널 열기"),
    busy || remoteWorkspaceMutation !== null || !connectable,
    () => void openRemoteWorkspaceTerminal(workspace),
  ));
  actions.appendChild(integrationActionButton(
    "remote-workspace-files", t("sftp.title", "SFTP 파일"),
    busy || remoteWorkspaceMutation !== null,
    () => void openSftpManager({ id: workspace.host_id }, workspace.root),
  ));
  actions.appendChild(integrationActionButton(
    "btn--halt remote-workspace-remove",
    t("settings.remoteWorkspaces.remove", "삭제"),
    busy || remoteWorkspaceMutation !== null,
    () => void removeRemoteWorkspace(workspace),
  ));
  row.appendChild(actions);
  return row;
}

function paintRemoteWorkspaces() {
  const summary = el("remote-workspaces-summary");
  if (remoteWorkspaceLoading) {
    summary.dataset.state = "checking";
    say(summary, () => t("settings.integrations.checking", "확인 중…"));
  } else if (remoteWorkspaceError) {
    summary.dataset.state = "error";
    say(summary, () => t("settings.integrations.checkFailed", "확인 실패"));
  } else {
    summary.dataset.state = remoteWorkspaceReport.workspaces.length > 0
      ? "connected"
      : "disconnected";
    say(summary, () => t("settings.remoteWorkspaces.count", "{{count}}개 워크스페이스", {
      count: remoteWorkspaceReport.workspaces.length,
    }));
  }
  const error = el("remote-workspaces-error");
  error.hidden = !remoteWorkspaceError;
  if (remoteWorkspaceError) say(error, () => remoteWorkspaceError);
  el("remote-workspace-list").replaceChildren(
    ...remoteWorkspaceReport.workspaces.map(remoteWorkspaceNode),
  );
  el("remote-workspace-empty").hidden = remoteWorkspaceLoading
    || remoteWorkspaceReport.workspaces.length > 0;
  el("remote-workspace-refresh").disabled = remoteWorkspaceLoading
    || remoteWorkspaceMutation !== null;
  el("remote-workspace-add").disabled = remoteWorkspaceMutation !== null
    || sshHostReport.hosts.length === 0;
}

async function refreshRemoteWorkspaces(preservedError = null) {
  if (remoteWorkspaceMutation !== null) return false;
  const generation = ++remoteWorkspaceGeneration;
  remoteWorkspaceLoading = true;
  remoteWorkspaceError = null;
  paintRemoteWorkspaces();
  let refreshed = false;
  try {
    const answer = await invoke("remote_workspaces");
    if (generation !== remoteWorkspaceGeneration) return false;
    remoteWorkspaceReport = normalizeRemoteWorkspaceReport(answer);
    remoteWorkspaceActions.clear();
    refreshed = true;
  } catch (error) {
    if (generation !== remoteWorkspaceGeneration) return false;
    remoteWorkspaceError = preservedError || String(error);
  }
  if (generation !== remoteWorkspaceGeneration) return false;
  remoteWorkspaceLoading = false;
  if (!remoteWorkspaceError) remoteWorkspaceError = preservedError;
  paintRemoteWorkspaces();
  return refreshed;
}

function remoteWorkspaceMatchesInput(workspace, input) {
  return (input.id === null || workspace.id === input.id)
    && workspace.label === input.label
    && workspace.host_id === input.hostId
    && workspace.root === input.root;
}

function integrationRowsMeetExpectation(rows, expectation, matchesInput) {
  if (expectation.saved) {
    return rows.some((row) => matchesInput.call(null, row, expectation.saved));
  }
  return !rows.some((row) => row.id === expectation.removed);
}

async function reconcileRemoteWorkspaceMutation(error, expectation) {
  remoteWorkspaceMutation = null;
  const refreshed = await refreshRemoteWorkspaces(String(error));
  if (!refreshed
      || !integrationRowsMeetExpectation(
        remoteWorkspaceReport.workspaces,
        expectation,
        remoteWorkspaceMatchesInput,
      )) {
    return false;
  }
  remoteWorkspaceError = null;
  paintRemoteWorkspaces();
  return true;
}

function selectedRemoteWorkspace() {
  const id = el("remote-workspace-id").value;
  return remoteWorkspaceReport.workspaces.find((workspace) => workspace.id === id) ?? null;
}

function populateRemoteWorkspaceHosts(selectedId = "") {
  const picker = el("remote-workspace-host");
  picker.replaceChildren(...sshHostReport.hosts.map((host) => {
    const option = document.createElement("option");
    option.value = host.id;
    option.textContent = `${host.label} · ${sshHostEndpoint(host)}`;
    return option;
  }));
  picker.value = selectedId || sshHostReport.hosts[0]?.id || "";
}

function remoteWorkspaceDialogCanSave() {
  const verified = el("remote-workspace-verified-root").value;
  return remoteWorkspaceMutation === null
    && el("remote-workspace-label").value.trim() !== ""
    && el("remote-workspace-host").value !== ""
    && el("remote-workspace-root").value.trim() !== ""
    && verified !== ""
    && verified === el("remote-workspace-root").value.trim();
}

function paintRemoteWorkspaceDialog() {
  const verified = el("remote-workspace-verified-root").value;
  const root = el("remote-workspace-root").value.trim();
  const status = el("remote-workspace-root-status");
  if (remoteWorkspaceMutation === "probing") {
    status.dataset.state = "checking";
    say(status, () => t("settings.integrations.checking", "확인 중…"));
  } else if (verified !== "" && verified === root) {
    status.dataset.state = "connected";
    say(status, () => t("settings.remoteWorkspaces.verified", "확인됨"));
  } else {
    status.dataset.state = "unchecked";
    say(status, () => t("settings.integrations.unchecked", "확인 전"));
  }
  el("remote-workspace-probe").disabled = remoteWorkspaceMutation !== null
    || el("remote-workspace-host").value === ""
    || root === "";
  el("remote-workspace-save").disabled = !remoteWorkspaceDialogCanSave();
}

function clearRemoteWorkspaceDialogError() {
  const error = el("remote-workspace-dialog-error");
  error.hidden = true;
  error.textContent = "";
}

function reportRemoteWorkspaceDialogError(error) {
  const output = el("remote-workspace-dialog-error");
  say(output, () => String(error));
  output.hidden = false;
}

function invalidateRemoteWorkspaceRoot() {
  ++remoteWorkspaceProbeGeneration;
  if (remoteWorkspaceMutation === "probing") remoteWorkspaceMutation = null;
  el("remote-workspace-verified-root").value = "";
  clearRemoteWorkspaceDialogError();
  paintRemoteWorkspaceDialog();
}

function openRemoteWorkspaceDialog(workspace = null) {
  remoteWorkspaceMutation = null;
  el("remote-workspace-id").value = workspace?.id ?? "";
  el("remote-workspace-label").value = workspace?.label ?? "";
  populateRemoteWorkspaceHosts(workspace?.host_id ?? "");
  el("remote-workspace-root").value = workspace?.root ?? "";
  el("remote-workspace-verified-root").value = workspace?.root ?? "";
  say(el("remote-workspace-dialog-title"), () => workspace
    ? t("settings.remoteWorkspaces.editTitle", "원격 워크스페이스 편집")
    : t("settings.remoteWorkspaces.add", "워크스페이스 추가"));
  clearRemoteWorkspaceDialogError();
  paintRemoteWorkspaceDialog();
  showModal(remoteWorkspaceScrim);
}

function closeRemoteWorkspaceDialog() {
  ++remoteWorkspaceProbeGeneration;
  remoteWorkspaceMutation = null;
  clearRemoteWorkspaceDialogError();
  hideModal(remoteWorkspaceScrim);
  paintRemoteWorkspaces();
}

async function probeRemoteWorkspaceRoot() {
  const generation = ++remoteWorkspaceProbeGeneration;
  remoteWorkspaceMutation = "probing";
  clearRemoteWorkspaceDialogError();
  paintRemoteWorkspaceDialog();
  try {
    const answer = await invoke("probe_remote_workspace", {
      input: {
        hostId: el("remote-workspace-host").value,
        root: el("remote-workspace-root").value.trim(),
      },
    });
    if (generation !== remoteWorkspaceProbeGeneration) return;
    const root = String(answer?.root ?? "");
    el("remote-workspace-root").value = root;
    el("remote-workspace-verified-root").value = root;
  } catch (error) {
    if (generation !== remoteWorkspaceProbeGeneration) return;
    reportRemoteWorkspaceDialogError(error);
  } finally {
    if (generation === remoteWorkspaceProbeGeneration) {
      remoteWorkspaceMutation = null;
      paintRemoteWorkspaceDialog();
    }
  }
}

async function saveRemoteWorkspace() {
  if (!remoteWorkspaceDialogCanSave()) return;
  const generation = ++remoteWorkspaceGeneration;
  remoteWorkspaceMutation = "saving";
  clearRemoteWorkspaceDialogError();
  paintRemoteWorkspaceDialog();
  const input = {
    id: el("remote-workspace-id").value || null,
    label: el("remote-workspace-label").value.trim(),
    hostId: el("remote-workspace-host").value,
    root: el("remote-workspace-verified-root").value,
  };
  try {
    const answer = await invoke("save_remote_workspace", { input });
    if (generation !== remoteWorkspaceGeneration) return;
    remoteWorkspaceReport = normalizeRemoteWorkspaceReport(answer);
    closeRemoteWorkspaceDialog();
  } catch (error) {
    if (generation !== remoteWorkspaceGeneration) return;
    if (await reconcileRemoteWorkspaceMutation(error, { saved: input })) {
      closeRemoteWorkspaceDialog();
      return;
    }
    reportRemoteWorkspaceDialogError(error);
    paintRemoteWorkspaceDialog();
  }
}

async function removeRemoteWorkspace(workspace) {
  if (remoteWorkspaceMutation !== null) return;
  const generation = ++remoteWorkspaceGeneration;
  remoteWorkspaceMutation = workspace.id;
  remoteWorkspaceError = null;
  paintRemoteWorkspaces();
  try {
    const answer = await invoke("remove_remote_workspace", { id: workspace.id });
    if (generation !== remoteWorkspaceGeneration) return;
    remoteWorkspaceReport = normalizeRemoteWorkspaceReport(answer);
  } catch (error) {
    if (generation !== remoteWorkspaceGeneration) return;
    await reconcileRemoteWorkspaceMutation(error, { removed: workspace.id });
    return;
  }
  if (generation !== remoteWorkspaceGeneration) return;
  remoteWorkspaceMutation = null;
  paintRemoteWorkspaces();
}

async function testRemoteWorkspace(workspace) {
  if (remoteWorkspaceActions.get(workspace.id)?.busy) return;
  remoteWorkspaceActions.set(workspace.id, { busy: true });
  remoteWorkspaceError = null;
  paintRemoteWorkspaces();
  try {
    await invoke("test_remote_workspace", { id: workspace.id });
    remoteWorkspaceActions.set(workspace.id, { state: "connected" });
  } catch (error) {
    remoteWorkspaceActions.delete(workspace.id);
    remoteWorkspaceError = String(error);
  }
  paintRemoteWorkspaces();
}

async function openRemoteWorkspaceTerminal(workspace) {
  if (remoteWorkspaceActions.get(workspace.id)?.busy) return;
  remoteWorkspaceActions.set(workspace.id, { busy: true });
  remoteWorkspaceError = null;
  paintRemoteWorkspaces();
  try {
    const term = await invoke("open_remote_workspace_terminal", {
      id: workspace.id,
      rows: 24,
      cols: 96,
    });
    remoteWorkspaceActions.delete(workspace.id);
    setSettingsOpen(false);
    mountTermTab(term, {
      label: workspace.label,
      remoteHost: workspace.host_id,
      remoteWorkspace: workspace.id,
    }, { placement: "tab" });
  } catch (error) {
    remoteWorkspaceActions.delete(workspace.id);
    remoteWorkspaceError = String(error);
    paintRemoteWorkspaces();
  }
}

el("remote-workspace-refresh").addEventListener("click", () => void refreshRemoteWorkspaces());
el("remote-workspace-add").addEventListener("click", () => openRemoteWorkspaceDialog());
el("remote-workspace-cancel").addEventListener("click", closeRemoteWorkspaceDialog);
el("remote-workspace-probe").addEventListener("click", () => void probeRemoteWorkspaceRoot());
el("remote-workspace-save").addEventListener("click", () => void saveRemoteWorkspace());
remoteWorkspaceScrim.addEventListener("mousedown", (event) => {
  if (event.target === remoteWorkspaceScrim && remoteWorkspaceMutation === null) {
    closeRemoteWorkspaceDialog();
  }
});
el("remote-workspace-label").addEventListener("input", () => {
  clearRemoteWorkspaceDialogError();
  paintRemoteWorkspaceDialog();
});
for (const id of ["remote-workspace-host", "remote-workspace-root"]) {
  el(id).addEventListener(id.endsWith("host") ? "change" : "input", invalidateRemoteWorkspaceRoot);
}

/* ---- Remote session servers --------------------------------------------
 *
 * The renderer receives metadata and credential standing only. The access
 * link exists in the password input just long enough to cross native IPC;
 * the backend verifies the loopback session protocol before it stores the
 * bearer in the OS credential provider. */
const remoteServerScrim = el("remote-server-scrim");
let remoteServerReport = { servers: [] };
let remoteServerLoading = false;
let remoteServerError = null;
let remoteServerGeneration = 0;
let remoteServerMutation = null;
const remoteServerActions = new Map();

function normalizeRemoteServerReport(answer) {
  const rows = Array.isArray(answer?.servers) ? answer.servers : [];
  const standings = new Set(["available", "missing", "unreadable"]);
  return {
    servers: rows
      .filter((server) => server
        && typeof server === "object"
        && typeof server.id === "string")
      .map((server) => {
        const standing = String(server.credentialStatus ?? server.credential_status ?? "unreadable");
        return {
          id: server.id,
          name: String(server.name ?? ""),
          endpoint: String(server.endpoint ?? ""),
          credential_status: standings.has(standing) ? standing : "unreadable",
        };
      }),
  };
}

function remoteServerCanConnect(server) {
  return server?.credential_status === "available";
}

function remoteServerCredentialLabel(server) {
  if (server.credential_status === "available") {
    return t("settings.remoteServers.tokenAvailable", "토큰 저장됨");
  }
  if (server.credential_status === "missing") {
    return t("settings.remoteServers.tokenMissing", "액세스 링크 필요");
  }
  return t("settings.remoteServers.tokenUnreadable", "토큰을 읽을 수 없음");
}

function remoteServerNode(server) {
  const row = document.createElement("div");
  row.className = "integration-site";
  row.dataset.serverId = server.id;
  row.setAttribute("role", "listitem");

  const identity = document.createElement("button");
  identity.type = "button";
  identity.className = "integration-site-pick remote-server-edit";
  identity.disabled = remoteServerMutation !== null;
  say(identity, () => server.name);
  identity.addEventListener("click", () => openRemoteServerDialog(server));
  row.appendChild(identity);

  const summary = document.createElement("span");
  summary.className = "integration-site-summary";
  say(summary, () => {
    const connected = remoteServerActions.get(server.id)?.state === "connected"
      ? t("settings.integrations.connected", "연결됨")
      : null;
    return [server.endpoint, remoteServerCredentialLabel(server), connected]
      .filter(Boolean)
      .join(" · ");
  });
  row.appendChild(summary);

  const actions = document.createElement("div");
  actions.className = "integration-site-actions";
  const busy = remoteServerActions.get(server.id)?.busy === true;
  const connectable = remoteServerCanConnect(server);
  actions.appendChild(integrationActionButton(
    "remote-server-test",
    busy
      ? t("settings.integrations.testing", "처리 중…")
      : t("settings.integrations.test", "연결 검사"),
    busy || remoteServerMutation !== null || !connectable,
    () => void testRemoteServer(server),
  ));
  actions.appendChild(integrationActionButton(
    "remote-server-open",
    t("settings.remoteServers.open", "지속 세션 열기"),
    busy || remoteServerMutation !== null || !connectable,
    () => void openRemoteServerSession(server),
  ));
  actions.appendChild(integrationActionButton(
    "btn--halt remote-server-remove",
    t("settings.remoteServers.remove", "삭제"),
    busy || remoteServerMutation !== null,
    () => void removeRemoteServer(server),
  ));
  row.appendChild(actions);
  return row;
}

function paintRemoteServers() {
  const summary = el("remote-servers-summary");
  if (remoteServerLoading) {
    summary.dataset.state = "checking";
    say(summary, () => t("settings.integrations.checking", "확인 중…"));
  } else if (remoteServerError) {
    summary.dataset.state = "error";
    say(summary, () => t("settings.integrations.checkFailed", "확인 실패"));
  } else {
    summary.dataset.state = remoteServerReport.servers.length > 0 ? "connected" : "disconnected";
    say(summary, () => t("settings.remoteServers.count", "{{count}}개 서버", {
      count: remoteServerReport.servers.length,
    }));
  }
  const error = el("remote-servers-error");
  error.hidden = !remoteServerError;
  if (remoteServerError) say(error, () => remoteServerError);
  el("remote-server-list").replaceChildren(...remoteServerReport.servers.map(remoteServerNode));
  el("remote-server-empty").hidden = remoteServerLoading || remoteServerReport.servers.length > 0;
  el("remote-server-refresh").disabled = remoteServerLoading || remoteServerMutation !== null;
  el("remote-server-add").disabled = remoteServerMutation !== null;
}

async function refreshRemoteServers(preservedError = null) {
  if (remoteServerMutation !== null) return false;
  const generation = ++remoteServerGeneration;
  remoteServerLoading = true;
  remoteServerError = null;
  paintRemoteServers();
  let refreshed = false;
  try {
    const answer = await invoke("remote_servers");
    if (generation !== remoteServerGeneration) return false;
    remoteServerReport = normalizeRemoteServerReport(answer);
    remoteServerActions.clear();
    refreshed = true;
  } catch (error) {
    if (generation !== remoteServerGeneration) return false;
    remoteServerError = preservedError || String(error);
  }
  if (generation !== remoteServerGeneration) return false;
  remoteServerLoading = false;
  if (!remoteServerError) remoteServerError = preservedError;
  paintRemoteServers();
  return refreshed;
}

function remoteServerEndpointFromAccessLink(accessLink) {
  try {
    return new URL(accessLink).searchParams.get("endpoint") ?? "";
  } catch {
    return "";
  }
}

function remoteServerMatchesInput(server, input) {
  return (input.id === null || server.id === input.id)
    && server.name === input.name
    && server.endpoint === remoteServerEndpointFromAccessLink(input.accessLink);
}

async function reconcileRemoteServerMutation(error, expectation) {
  remoteServerMutation = null;
  const refreshed = await refreshRemoteServers(String(error));
  if (!refreshed
      || !integrationRowsMeetExpectation(
        remoteServerReport.servers,
        expectation,
        remoteServerMatchesInput,
      )) {
    return false;
  }
  remoteServerError = null;
  paintRemoteServers();
  return true;
}

function remoteServerDialogCanSave() {
  return remoteServerMutation === null
    && el("remote-server-name").value.trim() !== ""
    && el("remote-server-access-link").value.trim() !== "";
}

function paintRemoteServerDialog() {
  el("remote-server-save").disabled = !remoteServerDialogCanSave();
}

function clearRemoteServerDialogError() {
  const error = el("remote-server-dialog-error");
  error.hidden = true;
  error.textContent = "";
}

function reportRemoteServerDialogError(error) {
  const output = el("remote-server-dialog-error");
  say(output, () => String(error));
  output.hidden = false;
}

function openRemoteServerDialog(server = null) {
  remoteServerMutation = null;
  el("remote-server-id").value = server?.id ?? "";
  el("remote-server-name").value = server?.name ?? "";
  el("remote-server-access-link").value = "";
  say(el("remote-server-dialog-title"), () => server
    ? t("settings.remoteServers.editTitle", "원격 서버 편집")
    : t("settings.remoteServers.add", "서버 추가"));
  clearRemoteServerDialogError();
  paintRemoteServerDialog();
  showModal(remoteServerScrim);
}

function closeRemoteServerDialog() {
  remoteServerMutation = null;
  el("remote-server-access-link").value = "";
  clearRemoteServerDialogError();
  hideModal(remoteServerScrim);
  paintRemoteServers();
}

async function saveRemoteServer() {
  if (!remoteServerDialogCanSave()) return;
  const generation = ++remoteServerGeneration;
  remoteServerMutation = "saving";
  clearRemoteServerDialogError();
  paintRemoteServerDialog();
  const input = {
    id: el("remote-server-id").value || null,
    name: el("remote-server-name").value.trim(),
    accessLink: el("remote-server-access-link").value.trim(),
  };
  try {
    const answer = await invoke("save_remote_server", { input });
    if (generation !== remoteServerGeneration) return;
    remoteServerReport = normalizeRemoteServerReport(answer);
    closeRemoteServerDialog();
  } catch (error) {
    if (generation !== remoteServerGeneration) return;
    if (await reconcileRemoteServerMutation(error, { saved: input })) {
      closeRemoteServerDialog();
      return;
    }
    reportRemoteServerDialogError(error);
    paintRemoteServerDialog();
  }
}

async function removeRemoteServer(server) {
  if (remoteServerMutation !== null) return;
  const generation = ++remoteServerGeneration;
  remoteServerMutation = server.id;
  remoteServerError = null;
  paintRemoteServers();
  try {
    const answer = await invoke("remove_remote_server", { id: server.id });
    if (generation !== remoteServerGeneration) return;
    remoteServerReport = normalizeRemoteServerReport(answer);
  } catch (error) {
    if (generation !== remoteServerGeneration) return;
    await reconcileRemoteServerMutation(error, { removed: server.id });
    return;
  }
  if (generation !== remoteServerGeneration) return;
  remoteServerMutation = null;
  paintRemoteServers();
}

async function testRemoteServer(server) {
  if (!remoteServerCanConnect(server) || remoteServerActions.get(server.id)?.busy) return;
  remoteServerActions.set(server.id, { busy: true });
  remoteServerError = null;
  paintRemoteServers();
  try {
    await invoke("test_remote_server", { id: server.id });
    remoteServerActions.set(server.id, { state: "connected" });
  } catch (error) {
    remoteServerActions.delete(server.id);
    remoteServerError = String(error);
  }
  paintRemoteServers();
}

async function openRemoteServerSession(server) {
  if (!remoteServerCanConnect(server) || remoteServerActions.get(server.id)?.busy) return;
  remoteServerActions.set(server.id, { busy: true });
  remoteServerError = null;
  paintRemoteServers();
  try {
    const term = await invoke("open_remote_server_session", {
      id: server.id,
      rows: 24,
      cols: 96,
    });
    remoteServerActions.delete(server.id);
    setSettingsOpen(false);
    mountTermTab(term, {
      label: server.name,
      remoteServer: server.id,
      worktree: `zerocode://${server.endpoint}`,
    }, { placement: "tab" });
  } catch (error) {
    remoteServerActions.delete(server.id);
    remoteServerError = String(error);
    paintRemoteServers();
  }
}

el("remote-server-refresh").addEventListener("click", () => void refreshRemoteServers());
el("remote-server-add").addEventListener("click", () => openRemoteServerDialog());
el("remote-server-cancel").addEventListener("click", closeRemoteServerDialog);

/* ---- macOS developer permissions ---------------------------------------
 *
 * The native side owns every identifier, probe, fixed System Settings URL,
 * and socket decision. This map owns words and button posture only. Keeping
 * the two lists strict means a new native permission cannot silently render
 * as an unlabeled or unactionable row. */
const developerPermissionDefinitions = Object.freeze([
  {
    id: "microphone",
    title: () => t("settings.permissions.microphone", "마이크"),
    hint: () => t("settings.permissions.microphoneHint", "음성 입력과 오디오 도구가 마이크를 사용합니다."),
  },
  {
    id: "camera",
    title: () => t("settings.permissions.camera", "카메라"),
    hint: () => t("settings.permissions.cameraHint", "영상 및 캡처 도구가 카메라를 사용합니다."),
  },
  {
    id: "screen",
    title: () => t("settings.permissions.screen", "화면 및 시스템 오디오 녹화"),
    hint: () => t("settings.permissions.screenHint", "화면 검사와 캡처 도구가 화면 내용을 읽습니다."),
  },
  {
    id: "accessibility",
    title: () => t("settings.permissions.accessibility", "손쉬운 사용"),
    hint: () => t("settings.permissions.accessibilityHint", "자동화 도구가 다른 앱의 입력과 UI를 제어합니다."),
  },
  {
    id: "full-disk-access",
    title: () => t("settings.permissions.fullDisk", "전체 디스크 접근"),
    hint: () => t("settings.permissions.fullDiskHint", "개발 도구가 보호된 파일과 폴더를 읽습니다."),
  },
  {
    id: "automation",
    title: () => t("settings.permissions.automation", "자동화"),
    hint: () => t("settings.permissions.automationHint", "Apple Events를 사용하는 도구가 다른 앱을 조종합니다. 판정 대상은 System Events이며, 그 앱이 떠 있지 않으면 물어볼 상대가 없어 「알 수 없음」입니다."),
  },
  {
    id: "local-network",
    title: () => t("settings.permissions.localNetwork", "로컬 네트워크"),
    hint: () => t("settings.permissions.localNetworkHint", "LAN의 개발 서버와 기기에 연결합니다. macOS는 이 권한을 조회하는 API를 주지 않으므로, 아래 연결 테스트만이 증거입니다."),
  },
  {
    id: "usb",
    title: () => t("settings.permissions.usb", "USB 액세서리"),
    hint: () => t("settings.permissions.usbHint", "연결된 개발 및 디버깅 기기에 접근합니다. macOS는 이 권한을 조회하는 API를 주지 않고, 액세서리가 붙는 그때마다 묻습니다."),
  },
  {
    id: "bluetooth",
    title: () => t("settings.permissions.bluetooth", "Bluetooth"),
    hint: () => t("settings.permissions.bluetoothHint", "근처의 Bluetooth 개발 기기에 연결합니다."),
  },
]);
const developerPermissionIds = new Set(developerPermissionDefinitions.map((row) => row.id));
const developerPermissionStatuses = new Set([
  "granted", "denied", "unknown", "ready", "unsupported",
]);
const developerPermissionActions = new Set(["open-settings", "trigger-prompt"]);
let developerPermissionRows = [];
let developerPermissionLoading = false;
let developerPermissionError = false;
const developerPermissionBusy = new Set();
let developerPermissionGeneration = 0;
let localNetworkTesting = false;
let localNetworkTestResult = null;

function normalizeDeveloperPermissionStates(value) {
  if (!Array.isArray(value) || value.length !== developerPermissionDefinitions.length) {
    throw new Error("developer permission inventory mismatch");
  }
  const states = new Map();
  for (const row of value) {
    if (!row || !developerPermissionIds.has(row.id)
        || !developerPermissionStatuses.has(row.status)
        || !developerPermissionActions.has(row.action) || states.has(row.id)) {
      throw new Error("developer permission inventory mismatch");
    }
    states.set(row.id, { status: row.status, action: row.action });
  }
  return developerPermissionDefinitions.map((definition) => ({
    ...definition,
    ...states.get(definition.id),
  }));
}

function developerPermissionStatusCopy(status) {
  return {
    granted: ["connected", () => t("settings.permissions.granted", "허용됨")],
    denied: ["error", () => t("settings.permissions.denied", "거부됨")],
    unknown: ["unchecked", () => t("settings.permissions.unknown", "알 수 없음")],
    ready: ["available", () => t("settings.permissions.ready", "요청 가능")],
    unsupported: ["unsupported", () => t("settings.permissions.unsupported", "이 플랫폼에서 지원하지 않음")],
  }[status];
}

/* The only row with no status API of its own: what stands in for one is the
 * card's own connection test, carried here with the time it ran so a stale
 * success cannot pass for a fresh one. It is not a new status word — the row
 * still says 「알 수 없음」, and this line says what was actually tried. */
function lastLocalNetworkTestCopy(id) {
  if (id !== "local-network" || !localNetworkTestResult) return null;
  const at = new Date(localNetworkTestResult.testedAt).toLocaleString();
  return localNetworkTestResult.ok
    ? () => t("settings.permissions.lastTestOk", "최근 연결 테스트 성공 · {{at}}", { at })
    : () => t("settings.permissions.lastTestFailed", "최근 연결 테스트 실패 · {{at}}", { at });
}

function developerPermissionNode(row) {
  const item = document.createElement("div");
  item.className = "integration-site developer-permission-row";
  item.dataset.permissionId = row.id;
  item.setAttribute("role", "listitem");

  const copy = document.createElement("div");
  copy.className = "developer-permission-copy";
  const title = document.createElement("strong");
  title.className = "developer-permission-title";
  say(title, row.title);
  const hint = document.createElement("span");
  hint.className = "settings-row-desc";
  say(hint, row.hint);
  copy.append(title, hint);

  const evidence = lastLocalNetworkTestCopy(row.id);
  if (evidence) {
    const said = document.createElement("span");
    said.className = "settings-row-desc";
    said.dataset.permissionEvidence = row.id;
    say(said, evidence);
    copy.append(said);
  }

  const status = document.createElement("span");
  status.className = "settings-state";
  const [state, statusCopy] = developerPermissionStatusCopy(row.status);
  status.dataset.state = state;
  say(status, statusCopy);

  const action = document.createElement("button");
  action.className = "btn";
  action.type = "button";
  action.disabled = developerPermissionBusy.has(row.id) || row.status === "unsupported";
  say(action, row.action === "trigger-prompt"
    ? () => t("settings.permissions.triggerPrompt", "권한 요청 표시")
    : () => t("settings.permissions.openSettings", "시스템 설정 열기"));
  action.addEventListener("click", () => void actOnDeveloperPermission(row));
  item.append(copy, status, action);
  return item;
}

function paintDeveloperPermissions() {
  const summary = el("developer-permissions-summary");
  if (developerPermissionLoading) {
    summary.dataset.state = "checking";
    say(summary, () => t("settings.permissions.checking", "확인 중…"));
  } else if (developerPermissionError) {
    summary.dataset.state = "error";
    say(summary, () => t("settings.permissions.checkFailed", "상태를 확인하지 못함"));
  } else {
    const available = developerPermissionRows.filter(
      (row) => row.status === "granted" || row.status === "ready",
    ).length;
    summary.dataset.state = available > 0 ? "available" : "unchecked";
    say(summary, () => t("settings.permissions.count", "{{available}} / {{total}} 사용 가능", {
      available,
      total: developerPermissionDefinitions.length,
    }));
  }
  const error = el("developer-permissions-error");
  error.hidden = !developerPermissionError;
  if (developerPermissionError) {
    say(error, () => t("settings.permissions.checkFailedHint", "macOS 권한 상태를 읽지 못했습니다. 다시 시도하세요."));
  }
  el("developer-permission-list").replaceChildren(
    ...developerPermissionRows.map(developerPermissionNode),
  );
  el("developer-permissions-refresh").disabled = developerPermissionLoading
    || developerPermissionBusy.size > 0;
}

async function refreshDeveloperPermissions(preservedError = false) {
  if (developerPermissionBusy.size > 0) return false;
  const generation = ++developerPermissionGeneration;
  developerPermissionLoading = true;
  developerPermissionError = false;
  paintDeveloperPermissions();
  try {
    const answer = await invoke("developer_permission_statuses");
    if (generation !== developerPermissionGeneration) return false;
    developerPermissionRows = normalizeDeveloperPermissionStates(answer);
  } catch {
    if (generation !== developerPermissionGeneration) return false;
    developerPermissionError = true;
  }
  developerPermissionLoading = false;
  if (!developerPermissionError) developerPermissionError = preservedError;
  paintDeveloperPermissions();
  return !developerPermissionError;
}

async function actOnDeveloperPermission(row) {
  if (developerPermissionBusy.has(row.id)) return;
  developerPermissionBusy.add(row.id);
  developerPermissionError = false;
  paintDeveloperPermissions();
  let actionFailed = false;
  try {
    if (row.action === "trigger-prompt") {
      await invoke("request_developer_permission", { id: row.id });
    } else {
      await invoke("open_developer_permission_settings", { id: row.id });
    }
  } catch {
    actionFailed = true;
  }
  developerPermissionBusy.delete(row.id);
  await refreshDeveloperPermissions(actionFailed);
}

function localNetworkFailureCopy(failure) {
  return {
    "invalid-target": () => t("settings.permissions.invalidTarget", "호스트와 포트를 확인하세요."),
    unresolved: () => t("settings.permissions.unresolved", "호스트 이름을 찾지 못했습니다."),
    timeout: () => t("settings.permissions.timeout", "연결 시간이 초과되었습니다."),
    refused: () => t("settings.permissions.refused", "대상에서 연결을 거부했습니다."),
    unreachable: () => t("settings.permissions.unreachable", "사설 또는 로컬 주소에 연결할 수 없습니다."),
  }[failure] ?? (() => t("settings.permissions.testFailed", "연결 테스트에 실패했습니다."));
}

function paintLocalNetworkTest() {
  const output = el("local-network-test-status");
  if (localNetworkTesting) {
    say(output, () => t("settings.permissions.testing", "연결 중…"));
  } else if (localNetworkTestResult?.ok) {
    say(output, () => t("settings.permissions.testSucceeded", "{{host}}:{{port}} 연결 성공", {
      host: localNetworkTestResult.host,
      port: localNetworkTestResult.port,
    }));
  } else if (localNetworkTestResult) {
    say(output, localNetworkFailureCopy(localNetworkTestResult.failure));
  } else {
    output.textContent = "";
  }
  el("local-network-test").disabled = localNetworkTesting;
  el("local-network-host").disabled = localNetworkTesting;
  el("local-network-port").disabled = localNetworkTesting;
  paintDeveloperPermissions();
}

async function testLocalNetworkPermission() {
  if (localNetworkTesting) return;
  const host = el("local-network-host").value.trim();
  const port = Number(el("local-network-port").value);
  localNetworkTesting = true;
  localNetworkTestResult = null;
  paintLocalNetworkTest();
  try {
    localNetworkTestResult = await invoke("test_local_network_permission", { host, port });
  } catch {
    localNetworkTestResult = { ok: false, failure: "unknown", testedAt: Date.now() };
  }
  localNetworkTesting = false;
  paintLocalNetworkTest();
}

el("settings-security-group").hidden = !usesCommandModifier;
el("developer-permissions-refresh").addEventListener(
  "click",
  () => void refreshDeveloperPermissions(),
);
el("local-network-test").addEventListener("click", () => void testLocalNetworkPermission());
window.addEventListener("focus", () => {
  if (!settingsView.hidden && settingsPane === "macos-permissions") {
    void refreshDeveloperPermissions();
  }
});
el("remote-server-save").addEventListener("click", () => void saveRemoteServer());
remoteServerScrim.addEventListener("mousedown", (event) => {
  if (event.target === remoteServerScrim && remoteServerMutation === null) closeRemoteServerDialog();
});
for (const id of ["remote-server-name", "remote-server-access-link"]) {
  el(id).addEventListener("input", () => {
    clearRemoteServerDialogError();
    paintRemoteServerDialog();
  });
}

/* What the Jira panel shows, asked fresh every time the screen opens rather
 * than cached: a stale "not connected" is wrong the moment a connection is
 * made from the settings card, and this dialog has no push channel to learn
 * that on its own — `refreshScm` re-asks git for the same reason. */
let jiraStatus = {
  connected: false,
  active_site_id: null,
  selected_site_id: "all",
  credential_protection: null,
  sites: [],
};
let jiraStatusGeneration = 0;
let jiraIssuesGeneration = 0;
let jiraMutationGeneration = 0;
let jiraTestGeneration = 0;
const jiraTestRequests = new Map();
let jiraStatusLoading = false;
let jiraStatusError = null;
let jiraMutationSiteId = null;
const jiraTestResults = new Map();

/* Status changed shape when Jira became multi-site. Keep the compatibility at
 * this one boundary: current backends say `selected_site_id`; the store's
 * tagged `selected` and the old single-site report are accepted only here. */
function jiraStandingOf(value, fallback = "unchecked") {
  const held = value && typeof value === "object"
    ? value.standing ?? value.status ?? value.kind
    : value;
  return typeof held === "string"
    ? held.trim().toLowerCase().replaceAll("-", "_")
    : fallback;
}

function jiraStandingMessage(value) {
  if (!value || typeof value !== "object") return "";
  return String(value.message ?? value.error?.message ?? "");
}

function jiraStandingAvailable(standing) {
  return ["available", "connected", "healthy", "ok"].includes(standing);
}

function normalizeJiraStatus(answer) {
  const raw = answer && typeof answer === "object" ? answer : {};
  const rawSites = Array.isArray(raw.sites) ? raw.sites : [];
  let selectedSiteId = raw.selected_site_id ?? raw.selectedSiteId;
  if (selectedSiteId === undefined) {
    if (raw.selected?.kind === "all" || raw.selected === "all") selectedSiteId = "all";
    else if (raw.selected?.kind === "site") selectedSiteId = raw.selected.site_id;
    else if (typeof raw.selected === "string") selectedSiteId = raw.selected;
  }
  if (!selectedSiteId) {
    selectedSiteId = rawSites.length === 1 ? rawSites[0].id : "all";
  }
  const activeSiteId = raw.active_site_id ?? raw.activeSiteId
    ?? (selectedSiteId === "all" ? rawSites[0]?.id : selectedSiteId)
    ?? null;
  const legacyAvailable = raw.connected === true && rawSites.length === 1;
  const sites = rawSites.map((site) => {
    const credentialStanding = jiraStandingOf(site.credential, "");
    // The backend deliberately distinguishes an unreadable credential from a
    // missing one. Its broad `auth_error` standing must not erase that more
    // precise storage diagnosis before the renderer can explain it.
    const source = credentialStanding === "unreadable" ? site.credential : site.standing ?? site.credential
      ?? (raw.credential_error ? { kind: "unreadable", message: raw.credential_error } : undefined);
    const standing = credentialStanding === "unreadable"
      ? "unreadable"
      : jiraStandingOf(source, legacyAvailable ? "available" : "unchecked");
    return {
      ...site,
      active: site.active ?? site.id === activeSiteId,
      selected: selectedSiteId === "all" || site.id === selectedSiteId,
      standing,
      standing_message: jiraStandingMessage(source) || String(site.standing_message ?? ""),
    };
  });
  const selectedSites = selectedSiteId === "all"
    ? sites
    : sites.filter((site) => site.id === selectedSiteId);
  return {
    ...raw,
    connected: selectedSites.some((site) => jiraStandingAvailable(site.standing))
      || (raw.connected === true && selectedSites.length > 0),
    active_site_id: activeSiteId,
    selected_site_id: selectedSiteId,
    credential_protection: raw.credential_protection ?? raw.credentialProtection ?? null,
    sites,
  };
}

function selectedJiraSites() {
  return jiraStatus.selected_site_id === "all"
    ? jiraStatus.sites
    : jiraStatus.sites.filter((site) => site.id === jiraStatus.selected_site_id);
}

function selectedJiraSiteId() {
  return jiraStatus.selected_site_id || "all";
}

function effectiveJiraStanding(site) {
  return jiraTestResults.get(site.id)?.standing ?? site.standing;
}

function effectiveJiraStandingMessage(site) {
  return jiraTestResults.get(site.id)?.message || site.standing_message;
}

/* The rows themselves, and which status groups are folded shut.
 *
 * The fold set is keyed by status NAME rather than by index, so a refresh that
 * reorders the groups — which every refresh can, the query being sorted by
 * last-touched — does not silently unfold one group and fold another. */
let jiraIssues = [];
let jiraIssueFailures = [];
const jiraFolded = new Set();

/* The panel is in exactly one of these at a time.
 *
 * Written as a list and a single gesture rather than as a `hidden` per branch,
 * because the bug this shape prevents is the one every multi-state panel gets:
 * a branch that shows its own surface and forgets somebody else's, leaving
 * "no issues" underneath a spinning skeleton. Adding a state means adding a
 * name here — not another line in four functions. */
const JIRA_SURFACES = [
  "jira-loading",
  "jira-empty",
  "jira-credential-error",
  "jira-error",
  "jira-quiet",
  "jira-none",
  "jira-list",
];

/* The five of those the toolbar stands over: everything the query itself
 * produced. A refused JQL is one of them on purpose — the only way out of a
 * `400` is to edit the query, and a toolbar that leaves when the error arrives
 * takes the edit box with it. */
const JIRA_TOOL_SURFACES = new Set([
  "jira-loading",
  "jira-error",
  "jira-quiet",
  "jira-none",
  "jira-list",
]);

/* Show one surface and put the other six away, and — when the surface is one
 * that carries a sentence — say that sentence into it.
 *
 * Through `say` rather than a bare `textContent`, for the reason every
 * keyed element in this window goes through it: `applyLocale` redraws from the
 * key, so a sentence written behind its back lasts until the next language
 * change and then vanishes. These particular sentences come from Rust and are
 * the same in every language, which is exactly what `say` preserves. */
function showJiraOnly(id, said) {
  showOneSurface(JIRA_SURFACES, id, said);
  // 도구단은 물어본 자리의 것: 연결 카드나 읽지 못한 자격증명 위에는 서지
  // 않는다. 「기다리는 중」이 이 집합에 드는 것은 취향이 아니라 필수다 —
  // 검색 한 글자마다 도구단이 사라지면 타이핑 중인 칸이 화면에서 없어진다.
  el("jira-tools").hidden = !JIRA_TOOL_SURFACES.has(id);
  if (!JIRA_TOOL_SURFACES.has(id)) paintJiraSearchInterpretation(false);
}

function paintJiraSearchInterpretation(interpretedAsText) {
  el("jira-search-interpretation").hidden = !interpretedAsText;
}

/* The gesture both of those name. One function, because the bug it prevents is
 * one bug: a panel with N surfaces where some branch shows its own and forgets
 * somebody else's. Two copies of it would be two chances to forget the `say`,
 * which is what keeps a backend sentence alive across a language change. */
function showOneSurface(surfaces, id, said) {
  for (const name of surfaces) el(name).hidden = name !== id;
  if (said !== undefined) say(el(id), () => said);
}

async function refreshJiraStatus(preservedError = null) {
  if (jiraMutationSiteId !== null) return;
  const generation = ++jiraStatusGeneration;
  // A new status determines a new issue scope. Any answer still travelling for
  // the previous scope is stale before this request leaves the renderer.
  ++jiraIssuesGeneration;
  ++jiraTestGeneration;
  jiraTestRequests.clear();
  jiraTestResults.clear();
  jiraStatusLoading = true;
  jiraStatusError = null;
  paintJiraIntegration();
  showJiraOnly("jira-loading");
  try {
    const answer = await invoke("jira_status");
    if (generation !== jiraStatusGeneration) return;
    jiraStatus = normalizeJiraStatus(answer);
  } catch (error) {
    if (generation !== jiraStatusGeneration) return;
    jiraStatusError = preservedError || String(error);
    jiraStatusLoading = false;
    paintJiraIntegration();
    if (!taskView.hidden) showJiraOnly("jira-error", jiraStatusError);
    return;
  }
  jiraStatusLoading = false;
  jiraStatusError = preservedError;
  paintJiraIntegration();
  if (!taskView.hidden) {
    if (jiraStatusError) showJiraOnly("jira-error", jiraStatusError);
    else paintJiraPanel();
  }
}

function paintJiraPanel() {
  paintTaskContext();
  paintJiraSitePicker();
  if (jiraStatus.sites.length === 0) {
    showJiraOnly("jira-empty");
    return;
  }
  const selected = selectedJiraSites();
  if (!selected.some((site) => jiraStandingAvailable(effectiveJiraStanding(site)))) {
    const site = selected[0];
    const message = site && effectiveJiraStandingMessage(site)
      || t("task.jira.credentialUnavailable", "선택한 Jira 사이트의 자격증명을 사용할 수 없습니다.");
    showJiraOnly("jira-credential-error", message);
    return;
  }
  // At least one selected site is healthy. A failed neighbour is carried in
  // the issue report and drawn beside the healthy rows; it does not gate them.
  void loadJiraIssues();
}

/* ---- integrations settings --------------------------------------------- */

const EMPTY_GITHUB_STATUS = Object.freeze({
  availability: "available",
  connected: false,
  credential_protection: "external_cli",
  repo_host: null,
  repo_standing: "auth_error",
  active_account_id: null,
  accounts: [],
});
let githubStatus = { ...EMPTY_GITHUB_STATUS };
let githubStatusLoaded = false;
let githubIntegrationPending = null;
let githubIntegrationGeneration = 0;
let githubMutationGeneration = 0;
let githubMutationAccountId = null;
let githubTestGeneration = 0;
const githubTestRequests = new Map();
const githubTestResults = new Map();
let githubIntegrationLoading = false;
let githubIntegrationError = null;
let githubLoginOpening = false;

/* Onboarding and the setup guide need only the old three-state summary, but
 * they do not own another backend query or cache. This pure projection keeps
 * the canonical account snapshot as the single read model. */
function githubStandingOf(status) {
  if (status?.availability === "missing") return "missing";
  return status?.connected === true ? "connected" : "signin_needed";
}

function currentGithubStanding() {
  if (githubStatusLoaded) return githubStandingOf(githubStatus);
  return githubIntegrationError ? "missing" : null;
}

/* Deliberately copy only the non-secret lifecycle contract. A future `gh`
 * field or accidental backend passthrough cannot become renderer state just
 * because it appeared beside `login` in the JSON response. */
function normalizeGithubStatus(answer) {
  const accounts = Array.isArray(answer?.accounts)
    ? answer.accounts.map((account) => ({
      id: String(account?.id ?? ""),
      host: String(account?.host ?? ""),
      login: String(account?.login ?? ""),
      active: account?.active === true,
      standing: String(account?.standing ?? "auth_error"),
      selectable: account?.selectable === true,
      disconnectable: account?.disconnectable === true,
      credential_source: account?.credential_source === "gh" ? "gh" : "environment",
      env_credential: typeof account?.env_credential === "string" && account.env_credential
        ? account.env_credential
        : null,
      scope_gaps: Array.isArray(account?.scope_gaps)
        ? account.scope_gaps.filter((scope) => typeof scope === "string" && scope)
        : [],
    })).filter((account) => account.id && account.host && account.login)
    : [];
  const active = String(answer?.active_account_id ?? "");
  return {
    availability: answer?.availability === "missing" ? "missing" : "available",
    connected: answer?.connected === true,
    credential_protection: "external_cli",
    repo_host: answer?.repo_host == null ? null : String(answer.repo_host),
    repo_standing: String(answer?.repo_standing ?? "auth_error"),
    active_account_id: accounts.some((account) => account.id === active) ? active : null,
    accounts,
  };
}

function githubIntegrationCopy() {
  if (githubStatus.availability === "missing") {
    return {
      state: "unavailable",
      status: t("settings.integrations.missing", "설치되지 않음"),
      availability: t("settings.integrations.notInstalled", "CLI 없음"),
      auth: t("settings.integrations.unchecked", "확인 전"),
      live: t("settings.integrations.unavailable", "사용 불가"),
    };
  }
  const authenticated = githubStatus.accounts.some((account) => account.standing === "connected");
  const live = githubStatus.repo_standing === "connected";
  return {
    state: authenticated ? "connected" : "disconnected",
    status: authenticated
      ? t("settings.integrations.connected", "연결됨")
      : t("settings.integrations.signinNeeded", "로그인 필요"),
    availability: t("settings.integrations.installed", "CLI 설치됨"),
    auth: authenticated
      ? t("settings.integrations.authenticated", "로그인됨")
      : t("settings.integrations.signinNeeded", "로그인 필요"),
    live: live
      ? t("settings.integrations.availableNow", "사용 가능")
      : githubStatus.repo_standing === "not_repository"
        ? t("settings.integrations.noRepository", "현재 저장소 없음")
        : t("settings.integrations.unavailable", "사용 불가"),
  };
}

function githubAccountStanding(account) {
  const tested = githubTestResults.get(account.id);
  if (tested?.checking) return t("settings.integrations.testing", "검사 중…");
  if (tested?.message) return tested.message;
  const standing = tested?.standing ?? account.standing;
  return standing === "connected"
    ? t("settings.integrations.credentialAvailable", "자격증명 사용 가능")
    : t("settings.integrations.credentialMissing", "자격증명 없음");
}

function githubIntegrationAccountNode(account) {
  const row = document.createElement("div");
  row.className = "integration-site";
  row.dataset.accountId = account.id;
  row.setAttribute("role", "listitem");

  const pick = document.createElement("button");
  pick.type = "button";
  pick.className = "integration-site-pick integration-github-select";
  pick.dataset.accountId = account.id;
  pick.setAttribute("aria-pressed", String(account.active));
  pick.disabled = githubMutationAccountId !== null || account.active || !account.selectable;
  say(pick, () => `${account.active ? "●" : "○"} ${account.login}@${account.host}`);
  pick.addEventListener("click", () => void selectGithubAccount(account.id));
  row.appendChild(pick);

  const summary = document.createElement("span");
  summary.className = "integration-site-summary";
  say(summary, () => {
    const authority = account.credential_source === "environment"
      ? (account.env_credential
        ? t(
          "settings.integrations.githubEnvVarCredential",
          "{{name}} 환경 변수 자격증명 · 여기서 변경할 수 없음",
          { name: account.env_credential },
        )
        : t("settings.integrations.githubEnvironmentCredential", "환경 변수 자격증명 · 여기서 변경할 수 없음"))
      : t("settings.integrations.githubCliCredential", "GitHub CLI가 관리함");
    return `${githubAccountStanding(account)} · ${authority}`;
  });
  row.appendChild(summary);

  const actions = document.createElement("div");
  actions.className = "integration-site-actions";
  const test = document.createElement("button");
  test.type = "button";
  test.className = "btn integration-github-test";
  test.dataset.accountId = account.id;
  test.disabled = githubMutationAccountId !== null
    || !account.active
    || githubTestResults.get(account.id)?.checking === true;
  say(test, () => githubTestResults.get(account.id)?.checking
    ? t("settings.integrations.testing", "검사 중…")
    : t("settings.integrations.test", "연결 검사"));
  test.addEventListener("click", () => void testGithubAccount(account.id));
  actions.appendChild(test);

  const disconnect = document.createElement("button");
  disconnect.type = "button";
  disconnect.className = "btn btn--halt integration-github-disconnect";
  disconnect.dataset.accountId = account.id;
  disconnect.disabled = githubMutationAccountId !== null || !account.disconnectable;
  say(disconnect, () => t("settings.integrations.disconnect", "연결 해제"));
  disconnect.addEventListener("click", () => void disconnectGithubAccount(account.id));
  actions.appendChild(disconnect);
  row.appendChild(actions);
  return row;
}

function paintGithubIntegration() {
  const status = el("integration-github-status");
  const error = el("integration-github-error");
  const accounts = el("integration-github-accounts");
  const copy = githubIntegrationCopy();
  if (githubIntegrationLoading) {
    status.dataset.state = "checking";
    say(status, () => t("settings.integrations.checking", "확인 중…"));
  } else if (githubIntegrationError) {
    status.dataset.state = "error";
    say(status, () => t("settings.integrations.checkFailed", "확인 실패"));
  } else {
    status.dataset.state = copy.state;
    say(status, () => githubIntegrationCopy().status);
  }
  say(el("integration-github-availability"), () => githubIntegrationLoading
    ? t("settings.integrations.checking", "확인 중…")
    : githubIntegrationCopy().availability);
  say(el("integration-github-auth"), () => githubIntegrationLoading
    ? t("settings.integrations.checking", "확인 중…")
    : githubIntegrationCopy().auth);
  say(el("integration-github-live"), () => githubIntegrationLoading
    ? t("settings.integrations.checking", "확인 중…")
    : githubIntegrationCopy().live);

  error.hidden = !githubIntegrationError;
  if (githubIntegrationError) say(error, () => githubIntegrationError);
  accounts.replaceChildren(...githubStatus.accounts.map(githubIntegrationAccountNode));
  el("integration-github-empty").hidden = githubIntegrationLoading
    || githubStatus.accounts.length > 0;

  const action = el("integration-github-action");
  action.replaceChildren();
  const note = document.createElement("span");
  if (githubStatus.availability === "missing") {
    const install = document.createElement("button");
    install.type = "button";
    install.className = "btn";
    say(install, () => t("settings.integrations.githubInstall", "GitHub CLI 설치 안내"));
    install.addEventListener("click", () => openExternal("https://cli.github.com"));
    action.appendChild(install);
  } else {
    say(note, () => t(
      "settings.integrations.githubExternalAuthority",
      "로그인은 GitHub CLI가 관리하며 이 앱은 토큰을 저장하지 않습니다.",
    ));
    action.appendChild(note);
  }
  const busy = githubIntegrationLoading || githubMutationAccountId !== null || githubLoginOpening;
  el("integration-github-login").disabled = busy || githubStatus.availability === "missing";
  el("integration-github-recheck").disabled = busy;
  paintGithubRemedy();
}

/* 환경 변수가 gh의 키링 로그인을 가리면 refresh도 switch도 듣지 않는다 —
 * 활성 계정이 환경 변수 출신인데 서지 못할 때만, 그 사정과 손에 잡히는 두
 * 명령(찾기·해제)을 준다. 값은 절대 만지지 않고 이름만 말한다. */
function githubEnvRemedy() {
  if (githubIntegrationLoading) return null;
  const active = githubStatus.accounts.find(
    (account) => account.id === githubStatus.active_account_id,
  );
  if (!active || active.credential_source !== "environment") return null;
  if (active.standing !== "auth_error") return null;
  if (!active.env_credential) return null;
  return {
    name: active.env_credential,
    // 같은 호스트에 gh가 제 손으로 쥔 로그인이 남아 있으면, 변수를 지우는
    // 것만으로 그 로그인이 이어받는다 — 다음 할 일이 달라지는 사실이다.
    keyringWaits: githubStatus.accounts.some((account) =>
      account.host === active.host && account.credential_source === "gh"),
  };
}

/* 그 변수를 다루는 두 명령 — 이 창이 실행하지 않고, 사람의 터미널로 간다.
 * 명령의 겉모습은 OS의 것: PowerShell과 셸 rc 파일은 같은 일을 다른 손으로
 * 한다. */
function envVarCommands(name) {
  if (/win/i.test(reportedPlatform)) {
    return [
      { key: "settings.integrations.envFindWin", fallback: "어디서 설정되는지 확인 (PowerShell)",
        command: `Get-ChildItem Env:${name}` },
      { key: "settings.integrations.envUnsetWin", fallback: "해제 (PowerShell, 영구)",
        command: `Remove-Item Env:${name}; [Environment]::SetEnvironmentVariable('${name}', $null, 'User')` },
    ];
  }
  return [
    { key: "settings.integrations.envFind", fallback: "어디서 내보내는지 찾기",
      command: `grep -RIn '${name}' ~/.zshrc ~/.zshenv ~/.bashrc ~/.bash_profile ~/.profile 2>/dev/null` },
    { key: "settings.integrations.envUnset", fallback: "이 셸에서 해제",
      command: `unset ${name}` },
  ];
}

/* 로그인은 섰지만 토큰에 이 창이 쓰는 스코프가 없다 — refresh 한 번이 길이다.
 * env 간섭이 먼저다: env 토큰 위에서는 refresh 자체가 듣지 않으므로, 그
 * 사다리를 지나온 뒤에만 이 얼굴이 선다. */
function githubScopeRemedy() {
  if (githubIntegrationLoading) return null;
  const active = githubStatus.accounts.find(
    (account) => account.id === githubStatus.active_account_id,
  );
  if (!active || active.scope_gaps.length === 0) return null;
  return { scopes: active.scope_gaps, host: active.host };
}

/* Orca의 REFRESH_CMD 형태 그대로 — 요구 스코프만 이 창의 것이다. */
function ghRefreshCommand(host, scopes) {
  const flags = scopes.map((scope) => `-s ${scope}`).join(" ");
  return host && host.toLowerCase() !== "github.com"
    ? `gh auth refresh --hostname ${host} ${flags}`
    : `gh auth refresh ${flags}`;
}

function remedyAct(command, label) {
  const act = document.createElement("button");
  act.type = "button";
  act.className = "btn integration-remedy-copy";
  act.dataset.tip = command;
  say(act, () => t(label.key, label.fallback));
  act.addEventListener("click", () => void clipboardText.write(command));
  return act;
}

function paintGithubRemedy() {
  const box = el("integration-github-remedy");
  const envTrouble = githubEnvRemedy();
  const scopeTrouble = envTrouble ? null : githubScopeRemedy();
  box.hidden = !envTrouble && !scopeTrouble;
  if (box.hidden) {
    box.replaceChildren();
    return;
  }
  const head = document.createElement("p");
  head.className = "integration-remedy-head";
  const body = document.createElement("p");
  body.className = "integration-remedy-body";
  const acts = document.createElement("div");
  acts.className = "integration-remedy-acts";
  if (envTrouble) {
    say(head, () => t(
      "settings.integrations.envShadowHead",
      "{{name}} 환경 변수가 gh의 키링 로그인을 가리고 있습니다. gh auth refresh는 환경 변수 토큰을 바꾸지 못합니다.",
      { name: envTrouble.name },
    ));
    say(body, () => (envTrouble.keyringWaits
      ? t(
        "settings.integrations.envShadowKeyring",
        "변수를 지우고 앱을 다시 시작하면 키링의 gh 로그인이 이어받습니다.",
      )
      : t(
        "settings.integrations.envShadowLogin",
        "변수를 지운 뒤 gh auth login으로 로그인하고 다시 확인하세요.",
      )));
    for (const one of envVarCommands(envTrouble.name)) {
      acts.appendChild(remedyAct(one.command, one));
    }
  } else {
    say(head, () => t(
      "settings.integrations.scopeGapHead",
      "gh 토큰에 {{scopes}} 스코프가 없습니다 — 이 창의 GitHub 읽기가 그것을 씁니다.",
      { scopes: scopeTrouble.scopes.join(", ") },
    ));
    say(body, () => t(
      "settings.integrations.scopeGapBody",
      "새로고침 명령을 터미널에서 실행하면 브라우저가 열려 새 스코프를 승인합니다. 끝나면 다시 확인하세요.",
    ));
    acts.appendChild(remedyAct(
      ghRefreshCommand(scopeTrouble.host, scopeTrouble.scopes),
      { key: "settings.integrations.copyRefresh", fallback: "새로고침 명령 복사" },
    ));
  }
  box.replaceChildren(head, body, acts);
}

async function refreshGithubIntegration(preservedError = null, force = false) {
  if (githubMutationAccountId !== null) return;
  if (!force && githubIntegrationPending) {
    await githubIntegrationPending.catch(() => {});
    return;
  }
  const generation = ++githubIntegrationGeneration;
  ++githubTestGeneration;
  githubTestRequests.clear();
  githubTestResults.clear();
  githubIntegrationLoading = true;
  githubIntegrationError = null;
  paintGithubIntegration();
  const request = invoke("github_status", { force });
  githubIntegrationPending = request;
  try {
    const answer = await request;
    if (generation !== githubIntegrationGeneration) return;
    githubStatus = normalizeGithubStatus(answer);
    githubStatusLoaded = true;
  } catch (error) {
    if (generation !== githubIntegrationGeneration) return;
    githubIntegrationError = preservedError || String(error);
    githubIntegrationLoading = false;
    paintGithubIntegration();
    return;
  } finally {
    if (githubIntegrationPending === request) githubIntegrationPending = null;
  }
  githubIntegrationLoading = false;
  githubIntegrationError = preservedError;
  paintGithubIntegration();
}

function beginGithubMutation(accountId) {
  const generation = ++githubMutationGeneration;
  ++githubIntegrationGeneration;
  ++githubTestGeneration;
  githubTestRequests.clear();
  githubTestResults.clear();
  githubIntegrationLoading = false;
  githubIntegrationError = null;
  githubMutationAccountId = accountId;
  paintGithubIntegration();
  return generation;
}

function acceptGithubMutation(canonical) {
  githubMutationAccountId = null;
  if (!canonical || !Array.isArray(canonical.accounts)) return false;
  ++githubIntegrationGeneration;
  githubStatus = normalizeGithubStatus(canonical);
  githubIntegrationError = null;
  paintGithubIntegration();
  return true;
}

async function recoverGithubMutation(generation, error) {
  if (generation !== githubMutationGeneration) return;
  const failure = failureOf(error);
  githubMutationAccountId = null;
  await refreshGithubIntegration(
    failure.message || t("settings.integrations.checkFailed", "확인 실패"),
  );
}

async function selectGithubAccount(accountId) {
  if (!accountId || githubMutationAccountId !== null) return;
  const generation = beginGithubMutation(accountId);
  try {
    const canonical = await invoke("github_select_account", { accountId });
    if (generation !== githubMutationGeneration) return;
    if (!acceptGithubMutation(canonical)) await refreshGithubIntegration();
  } catch (error) {
    await recoverGithubMutation(generation, error);
  }
}

async function disconnectGithubAccount(accountId) {
  if (!accountId || githubMutationAccountId !== null) return;
  const generation = beginGithubMutation(accountId);
  try {
    const canonical = await invoke("github_disconnect", { accountId });
    if (generation !== githubMutationGeneration) return;
    if (!acceptGithubMutation(canonical)) await refreshGithubIntegration();
  } catch (error) {
    await recoverGithubMutation(generation, error);
  }
}

async function testGithubAccount(accountId) {
  const account = githubStatus.accounts.find((candidate) => candidate.id === accountId);
  if (!account?.active || githubMutationAccountId !== null) return;
  const scope = githubTestGeneration;
  const generation = (githubTestRequests.get(accountId) ?? 0) + 1;
  githubTestRequests.set(accountId, generation);
  githubTestResults.set(accountId, { checking: true });
  paintGithubIntegration();
  try {
    const answer = await invoke("github_test_connection", { accountId });
    if (scope !== githubTestGeneration || generation !== githubTestRequests.get(accountId)) return;
    githubTestResults.set(accountId, {
      checking: false,
      standing: answer?.standing === "connected" ? "connected" : "auth_error",
      message: "",
    });
  } catch (error) {
    if (scope !== githubTestGeneration || generation !== githubTestRequests.get(accountId)) return;
    githubTestResults.set(accountId, {
      checking: false,
      standing: "auth_error",
      message: failureOf(error).message || t("settings.integrations.testFailed", "연결 검사 실패"),
    });
  }
  paintGithubIntegration();
}

async function openGithubLoginTerminal() {
  if (githubLoginOpening || githubMutationAccountId !== null) return;
  githubLoginOpening = true;
  githubIntegrationError = null;
  paintGithubIntegration();
  try {
    const command = String(await invoke("github_login_intent"));
    if (!/^gh auth login(?: --hostname [a-z0-9.-]+)? --web$/.test(command)) {
      throw new Error("Invalid GitHub login intent");
    }
    setSettingsOpen(false);
    const term = await invoke("open_term_tab", { rows: 24, cols: 96, plain: true });
    mountTermTab(term);
    await invoke("term_text", { term, text: `${command}\r` });
  } catch (error) {
    githubIntegrationError = String(error);
    showError(error);
  }
  githubLoginOpening = false;
  paintGithubIntegration();
}

/* ---- GitLab, recognised on its own (1-g56a) ----
 *
 * "git lab은 orca 다 인식되던데" — and it is, because there is nothing to
 * recognise it with except the CLI the person already signed in to. The card
 * has no token field and never will: `glab` owns the credential, this window
 * owns one question (is it here, is it signed in) and two remedies.
 *
 * Deliberately thinner than the GitHub card beside it. `gh` lets an app switch
 * and disconnect accounts, so that card is a lifecycle; `glab auth` offers no
 * such thing, and a select/disconnect row here would be a control that cannot
 * act. Anything the card cannot do, the terminal does — and opening that
 * terminal with the command already typed is this window's job ("git hub
 * 오르카는 자동으로 연결되던데"): the same door the GitHub card stands. The
 * command stays visible beside it for the person who wants their own shell. */

const EMPTY_GITLAB_STATUS = Object.freeze({
  installed: false,
  authenticated: false,
});
let gitlabStatus = { ...EMPTY_GITLAB_STATUS, hosts: [] };
let gitlabIntegrationLoading = false;
let gitlabIntegrationError = null;
let gitlabIntegrationGeneration = 0;
let gitlabLoginOpening = false;

/* One spelling each. The row shows the command and the copy button carries it,
 * and two spellings of one command is how somebody pastes something the screen
 * never said. */
const GLAB_LOGIN_COMMAND = "glab auth login";
const GLAB_INSTALL_URL = "https://gitlab.com/gitlab-org/cli#installation";

/* Copy only the facts the backend answers for. A future `glab` field — or an
 * accidental passthrough — cannot become renderer state just because it
 * arrived in the same object.
 *
 * `tokenless` and `gitProtocol` are the two `glab` says out loud beside "not
 * signed in", and they are the difference between a card somebody believes and
 * one they argue with: "깃랩도 실제로 연결되는데 지금 안될로 나옴". Both halves
 * of that were true — the API has no token, and git operations are over ssh,
 * which is a key and not this token — and the card only said the first. */
function normalizeGitlabStatus(answer) {
  const protocol = answer?.git_protocol;
  return {
    installed: answer?.installed === true,
    authenticated: answer?.authenticated === true,
    hosts: Array.isArray(answer?.hosts)
      ? answer.hosts.filter((host) => typeof host === "string" && host)
      : [],
    tokenless: answer?.tokenless === true,
    // Only the two spellings the backend accepts reach here; anything else is
    // a sentence this window has no phrasing for.
    gitProtocol: protocol === "ssh" || protocol === "https" ? protocol : null,
  };
}

function gitlabIntegrationCopy() {
  if (!gitlabStatus.installed) {
    return {
      state: "unavailable",
      status: t("settings.integrations.missing", "설치되지 않음"),
      availability: t("settings.integrations.notInstalled", "CLI 없음"),
      auth: t("settings.integrations.unchecked", "확인 전"),
    };
  }
  return {
    state: gitlabStatus.authenticated ? "connected" : "disconnected",
    status: gitlabStatus.authenticated
      ? t("settings.integrations.connected", "연결됨")
      : t("settings.integrations.signinNeeded", "로그인 필요"),
    availability: t("settings.integrations.installed", "CLI 설치됨"),
    auth: gitlabStatus.authenticated
      ? t("settings.integrations.authenticated", "로그인됨")
      : t("settings.integrations.signinNeeded", "로그인 필요"),
  };
}

/* The two unhealthy states, each with the one thing that ends it. Nothing is
 * offered while the answer is still being fetched or failed to arrive: a remedy
 * under a stale reading is advice for a machine nobody looked at. */
function paintGitlabRemedy() {
  const box = el("integration-gitlab-remedy");
  const settled = !gitlabIntegrationLoading && gitlabIntegrationError === null;
  box.hidden = !settled || (gitlabStatus.installed && gitlabStatus.authenticated);
  if (box.hidden) {
    box.replaceChildren();
    return;
  }
  const head = document.createElement("p");
  head.className = "integration-remedy-head";
  const acts = document.createElement("div");
  acts.className = "integration-remedy-acts";
  if (!gitlabStatus.installed) {
    say(head, () => t(
      "settings.gitlab.install",
      "MR·이슈·파이프라인을 쓰려면 GitLab CLI를 설치하세요.",
    ));
    const install = document.createElement("button");
    install.type = "button";
    install.className = "btn integration-gitlab-install";
    say(install, () => t("settings.gitlab.installCta", "GitLab CLI 설치"));
    install.addEventListener("click", () => openExternal(GLAB_INSTALL_URL));
    acts.appendChild(install);
    box.replaceChildren(head, acts);
    return;
  }
  say(head, () => t(
    "settings.gitlab.auth",
    "GitLab CLI가 설치되어 있지만 인증되지 않았습니다. 로그인을 누르면 앱 터미널에서 glab이 안내합니다.",
  ));
  // What `glab` itself said beside that, when it said anything: where it
  // looked for a token, and what git is still able to do without one. Two
  // sentences rather than one, because they are two different credentials and
  // reading them as one is the report this answers.
  const also = [];
  if (gitlabStatus.tokenless) {
    also.push(() => t(
      "settings.gitlab.noToken",
      "토큰이 없습니다 — 설정 파일·키체인·환경변수를 모두 확인했습니다.",
    ));
  }
  if (gitlabStatus.gitProtocol) {
    also.push(() => t(
      "settings.gitlab.gitStillWorks",
      "git 작업은 {{protocol}}로 설정되어 있어 clone·commit·push는 그대로 됩니다. 이 로그인은 MR·이슈·파이프라인을 읽기 위한 것입니다.",
      { protocol: gitlabStatus.gitProtocol },
    ));
  }
  // Not `say`: this is a command, not a sentence, and a language change must
  // not put a translation of it on screen.
  const line = document.createElement("code");
  line.className = "integration-remedy-command";
  line.textContent = GLAB_LOGIN_COMMAND;
  const door = document.createElement("button");
  door.type = "button";
  door.className = "btn btn--primary integration-gitlab-login";
  say(door, () => t("settings.gitlab.loginCta", "GitLab 로그인"));
  door.addEventListener("click", () => void openGitlabLoginTerminal());
  acts.appendChild(door);
  acts.appendChild(remedyAct(GLAB_LOGIN_COMMAND, {
    key: "settings.integrations.copyLogin", fallback: "로그인 명령 복사",
  }));
  const why = document.createElement("p");
  // The quiet rung this card already has for a second line.
  why.className = "integration-remedy-body";
  why.hidden = also.length === 0;
  if (also.length > 0) say(why, () => also.map((one) => one()).join(" "));
  box.replaceChildren(head, why, line, acts);
}

/* The GitHub card's door, for glab: a plain in-app terminal with the login
 * command already typed. `glab auth login` is interactive from there on —
 * the credential never passes through this window. */
async function openGitlabLoginTerminal() {
  if (gitlabLoginOpening) return;
  gitlabLoginOpening = true;
  gitlabIntegrationError = null;
  paintGitlabIntegration();
  try {
    setSettingsOpen(false);
    const term = await invoke("open_term_tab", { rows: 24, cols: 96, plain: true });
    mountTermTab(term);
    await invoke("term_text", { term, text: `${GLAB_LOGIN_COMMAND}\r` });
  } catch (error) {
    gitlabIntegrationError = String(error);
    showError(error);
  }
  gitlabLoginOpening = false;
  paintGitlabIntegration();
}

function paintGitlabIntegration() {
  const standing = el("integration-gitlab-status");
  if (gitlabIntegrationLoading) {
    standing.dataset.state = "checking";
    say(standing, () => t("settings.integrations.checking", "확인 중…"));
  } else if (gitlabIntegrationError !== null) {
    standing.dataset.state = "error";
    say(standing, () => t("settings.integrations.checkFailed", "확인 실패"));
  } else {
    standing.dataset.state = gitlabIntegrationCopy().state;
    say(standing, () => gitlabIntegrationCopy().status);
  }
  say(el("integration-gitlab-availability"), () => gitlabIntegrationLoading
    ? t("settings.integrations.checking", "확인 중…")
    : gitlabIntegrationCopy().availability);
  say(el("integration-gitlab-auth"), () => gitlabIntegrationLoading
    ? t("settings.integrations.checking", "확인 중…")
    : gitlabIntegrationCopy().auth);

  const trouble = el("integration-gitlab-error");
  trouble.hidden = gitlabIntegrationError === null;
  if (gitlabIntegrationError !== null) say(trouble, () => gitlabIntegrationError);

  // Which instances the login actually covers. On a machine with a company
  // GitLab beside gitlab.com, "연결됨" alone does not say which one answered.
  const named = gitlabStatus.authenticated ? gitlabStatus.hosts : [];
  const where = el("integration-gitlab-hosts");
  where.hidden = gitlabIntegrationLoading
    || gitlabIntegrationError !== null
    || named.length === 0;
  say(where, () => t("settings.gitlab.hosts", "로그인된 호스트: {{hosts}}", {
    hosts: named.join(", "),
  }));

  el("integration-gitlab-recheck").disabled = gitlabIntegrationLoading;
  paintGitlabRemedy();
}

/* Re-read the machine. `force` is the explicit 다시 확인 only: it re-hydrates
 * the login shell PATH, which is what somebody who just installed `glab`
 * needs, and what an ordinary pane open must not spend. */
async function refreshGitlabIntegration(force = false) {
  const generation = ++gitlabIntegrationGeneration;
  gitlabIntegrationLoading = true;
  gitlabIntegrationError = null;
  paintGitlabIntegration();
  try {
    const answer = await invoke("gitlab_status", { force });
    if (generation !== gitlabIntegrationGeneration) return;
    gitlabStatus = normalizeGitlabStatus(answer);
    // 실행당 한 번은 성공한 답에만 해당한다: 실패한 확인을 「물어봤다」로
    // 세면 작업판이 못 읽은 기계를 「glab 없음」이라고 부른다.
    gitlabStatusLoaded = true;
  } catch (error) {
    if (generation !== gitlabIntegrationGeneration) return;
    // A reading that failed is not a machine without `glab`: the card must not
    // offer an install button because the probe timed out.
    gitlabStatus = { ...EMPTY_GITLAB_STATUS, hosts: [] };
    gitlabIntegrationError = failureOf(error).message
      || t("settings.integrations.checkFailed", "확인 실패");
    gitlabIntegrationLoading = false;
    paintGitlabIntegration();
    return;
  }
  gitlabIntegrationLoading = false;
  paintGitlabIntegration();
}

function jiraSiteName(site) {
  if (site?.site_name) return String(site.site_name);
  if (site?.site_url) {
    try {
      return new URL(site.site_url).host || String(site.site_url);
    } catch {
      return String(site.site_url);
    }
  }
  return String(site?.display_name || site?.id || "Jira");
}

function jiraStandingCopy(site) {
  const tested = jiraTestResults.get(site.id);
  const standing = tested?.standing ?? site.standing;
  const message = tested?.message || site.standing_message;
  if (tested?.checking) return t("settings.integrations.checking", "확인 중…");
  if (message) return message;
  if (jiraStandingAvailable(standing)) {
    return t("settings.integrations.credentialAvailable", "자격증명 사용 가능");
  }
  if (["missing", "auth", "auth_error", "signin_needed"].includes(standing)) {
    return t("settings.integrations.credentialMissing", "자격증명 없음");
  }
  if (standing === "unreadable") {
    return t("settings.integrations.credentialUnreadable", "자격증명을 읽을 수 없음");
  }
  if (standing === "offline") return t("settings.integrations.offline", "연결할 수 없음");
  return t("settings.integrations.unchecked", "확인 전");
}

function jiraIntegrationSiteNode(site) {
  const row = document.createElement("div");
  row.className = "integration-site";
  row.dataset.siteId = site.id;
  row.setAttribute("role", "listitem");

  const pick = document.createElement("button");
  pick.type = "button";
  pick.className = "integration-site-pick";
  pick.dataset.siteId = site.id;
  const picked = jiraStatus.selected_site_id === site.id;
  pick.setAttribute("aria-pressed", String(picked));
  say(pick, () => {
    const mark = picked ? "●" : "○";
    const active = site.id === jiraStatus.active_site_id
      ? ` · ${t("settings.integrations.active", "활성")}`
      : "";
    return [mark, jiraSiteName(site)].join(" ") + active;
  });
  pick.disabled = jiraMutationSiteId !== null;
  pick.addEventListener("click", () => void selectJiraSite(site.id));
  row.appendChild(pick);

  const summary = document.createElement("span");
  summary.className = "integration-site-summary";
  say(summary, () => {
    const identity = site.email || site.site_url || site.id;
    return [identity, jiraStandingCopy(site)].join(" · ");
  });
  row.appendChild(summary);

  const actions = document.createElement("div");
  actions.className = "integration-site-actions";
  const test = document.createElement("button");
  test.type = "button";
  test.className = "btn integration-jira-test";
  test.dataset.siteId = site.id;
  test.disabled = jiraMutationSiteId !== null || jiraTestResults.get(site.id)?.checking === true;
  say(test, () => jiraTestResults.get(site.id)?.checking
    ? t("settings.integrations.testing", "검사 중…")
    : t("settings.integrations.test", "연결 검사"));
  test.addEventListener("click", () => void testJiraSite(site.id));
  actions.appendChild(test);

  const disconnect = document.createElement("button");
  disconnect.type = "button";
  disconnect.className = "btn btn--halt integration-jira-disconnect";
  disconnect.dataset.siteId = site.id;
  disconnect.disabled = jiraMutationSiteId !== null;
  say(disconnect, () => t("settings.integrations.disconnect", "연결 해제"));
  disconnect.addEventListener("click", () => void disconnectJiraSite(site.id));
  actions.appendChild(disconnect);
  row.appendChild(actions);
  return row;
}

function paintJiraIntegration() {
  const summary = el("integration-jira-summary");
  const error = el("integration-jira-error");
  const sites = el("integration-jira-sites");
  const available = jiraStatus.sites.filter((site) =>
    jiraStandingAvailable(effectiveJiraStanding(site)));
  if (jiraStatusLoading) {
    summary.dataset.state = "checking";
    say(summary, () => t("settings.integrations.checking", "확인 중…"));
  } else if (jiraStatusError) {
    summary.dataset.state = "error";
    say(summary, () => t("settings.integrations.checkFailed", "확인 실패"));
  } else if (jiraStatus.sites.length === 0) {
    summary.dataset.state = "disconnected";
    say(summary, () => t("settings.integrations.disconnected", "연결 안 됨"));
  } else {
    summary.dataset.state = available.length > 0 ? "connected" : "auth-error";
    say(summary, () => t("settings.integrations.jiraCount", "{{available}} / {{total}} 사이트 사용 가능",
      { available: available.length, total: jiraStatus.sites.length },
    ));
  }

  error.hidden = !jiraStatusError;
  if (jiraStatusError) say(error, () => jiraStatusError);
  const active = jiraStatus.sites.find((site) => site.id === jiraStatus.active_site_id);
  say(el("integration-jira-active"), () => active
    ? jiraSiteName(active)
    : t("settings.integrations.none", "없음"));
  const credentialStorageCopy = {
    native: () => t("settings.integrations.nativeProtected", "OS 자격증명 저장소"),
    plaintext: () => t(
      "settings.integrations.plaintextProtected",
      "보안 이전 대기 중인 레거시 평문 (사용 안 함)",
    ),
    unavailable: () => t(
      "settings.integrations.credentialStoreUnavailable",
      "OS 자격증명 저장소를 사용할 수 없음",
    ),
  };
  say(el("integration-jira-storage"), () =>
    (credentialStorageCopy[jiraStatus.credential_protection]
      ?? (() => t("settings.integrations.unchecked", "확인 전")))());

  sites.replaceChildren(...jiraStatus.sites.map(jiraIntegrationSiteNode));
  el("integration-jira-empty").hidden = jiraStatus.sites.length > 0 || jiraStatusLoading;
  el("integration-jira-refresh").disabled = jiraStatusLoading || jiraMutationSiteId !== null;
  paintJiraSitePicker();
}

function jiraSitePickerElement() {
  let picker = document.getElementById("jira-site-picker");
  if (picker) return picker;
  const field = document.createElement("label");
  field.id = "jira-site-picker-field";
  field.className = "settings-field";
  const label = document.createElement("span");
  label.className = "settings-label";
  say(label, () => t("task.jira.sitePicker", "Jira 사이트"));
  picker = document.createElement("select");
  picker.id = "jira-site-picker";
  picker.className = "settings-input";
  picker.addEventListener("change", () => void selectJiraSite(picker.value));
  field.append(label, picker);
  el("task-panel-jira").insertBefore(field, el("jira-empty"));
  return picker;
}

function paintJiraSitePicker() {
  const picker = jiraSitePickerElement();
  const field = picker.parentElement;
  const sites = jiraStatus.sites;
  field.hidden = sites.length < 2;
  picker.replaceChildren();
  if (sites.length > 1) {
    const all = document.createElement("option");
    all.value = "all";
    say(all, () => t("task.jira.allSites", "모든 사이트"));
    picker.appendChild(all);
  }
  for (const site of sites) {
    const option = document.createElement("option");
    option.value = site.id;
    say(option, () => jiraSiteName(site));
    picker.appendChild(option);
  }
  const selected = jiraStatus.selected_site_id === "all" && sites.length === 1
    ? sites[0]?.id
    : jiraStatus.selected_site_id;
  if ([...picker.options].some((option) => option.value === selected)) picker.value = selected;
  picker.disabled = jiraStatusLoading || jiraMutationSiteId !== null;
}

function beginJiraMutation(siteId) {
  const generation = ++jiraMutationGeneration;
  ++jiraStatusGeneration;
  ++jiraIssuesGeneration;
  ++jiraTestGeneration;
  jiraTestRequests.clear();
  jiraTestResults.clear();
  jiraStatusLoading = false;
  jiraMutationSiteId = siteId;
  jiraStatusError = null;
  paintJiraIntegration();
  if (!taskView.hidden) showJiraOnly("jira-loading");
  return generation;
}

function acceptJiraMutation(canonical) {
  jiraMutationSiteId = null;
  if (!canonical || !Array.isArray(canonical.sites)) return false;
  // A status read could have been started while this write was in flight by a
  // settings reopen in another surface. The write's canonical answer is newer.
  ++jiraStatusGeneration;
  jiraStatus = normalizeJiraStatus(canonical);
  paintJiraIntegration();
  if (!taskView.hidden) paintJiraPanel();
  return true;
}

/* A rejected mutation and a committed mutation whose acknowledgement was
 * lost are indistinguishable at the bridge. Re-read the store in both cases,
 * then put the original failure back on screen. The mutation generation keeps
 * an older recovery from clearing or repainting a newer operation. */
async function recoverJiraMutation(generation, error) {
  if (generation !== jiraMutationGeneration) return null;
  const parsed = failureOf(error);
  const failure = {
    ...parsed,
    message: parsed.message || t("settings.integrations.checkFailed", "확인 실패"),
  };
  jiraMutationSiteId = null;
  await refreshJiraStatus(failure.message);
  if (generation !== jiraMutationGeneration) return null;
  return failure;
}

async function selectJiraSite(siteId) {
  if (!siteId || jiraMutationSiteId !== null) return;
  const generation = beginJiraMutation(siteId);
  try {
    const canonical = await invoke("jira_select_site", { siteId });
    if (generation !== jiraMutationGeneration) return;
    if (!acceptJiraMutation(canonical)) await refreshJiraStatus();
  } catch (error) {
    await recoverJiraMutation(generation, error);
  }
}

async function disconnectJiraSite(siteId) {
  if (!siteId || jiraMutationSiteId !== null) return;
  const generation = beginJiraMutation(siteId);
  try {
    const canonical = await invoke("jira_disconnect", { siteId });
    if (generation !== jiraMutationGeneration) return;
    jiraTestResults.delete(siteId);
    if (!acceptJiraMutation(canonical)) await refreshJiraStatus();
  } catch (error) {
    await recoverJiraMutation(generation, error);
  }
}

async function testJiraSite(siteId) {
  const scope = jiraTestGeneration;
  const generation = (jiraTestRequests.get(siteId) ?? 0) + 1;
  jiraTestRequests.set(siteId, generation);
  jiraTestResults.set(siteId, { checking: true });
  paintJiraIntegration();
  try {
    const answer = await invoke("jira_test_connection", { siteId });
    if (scope !== jiraTestGeneration || generation !== jiraTestRequests.get(siteId)) return;
    if (Array.isArray(answer?.sites)) jiraStatus = normalizeJiraStatus(answer);
    const source = answer?.standing ?? answer?.status ?? answer;
    const ok = answer === true || answer?.ok === true || jiraStandingAvailable(jiraStandingOf(source));
    jiraTestResults.set(siteId, {
      checking: false,
      standing: ok ? "connected" : jiraStandingOf(source, "error"),
      message: String(answer?.message ?? (answer === false
        ? t("settings.integrations.testFailed", "연결 검사 실패")
        : "")),
    });
  } catch (error) {
    if (scope !== jiraTestGeneration || generation !== jiraTestRequests.get(siteId)) return;
    const failure = failureOf(error);
    jiraTestResults.set(siteId, {
      checking: false,
      standing: failure.kind,
      message: failure.message,
    });
  }
  paintJiraIntegration();
  if (!taskView.hidden) paintJiraPanel();
}

el("integration-github-recheck").addEventListener("click", () => {
  void refreshGithubIntegration(null, true);
});
el("integration-github-login").addEventListener("click", () => {
  void openGithubLoginTerminal();
});
el("integration-gitlab-recheck").addEventListener("click", () => {
  void refreshGitlabIntegration(true);
});
el("integration-jira-refresh").addEventListener("click", () => {
  void refreshJiraStatus();
});
el("integration-jira-add").addEventListener("click", openJiraConnectDialog);

/* The two failure shapes, told apart once.
 *
 * A backend refusal arrives as `{ kind, message }` and a bridge that broke
 * before reaching it arrives as an exception. Normalising here means every
 * caller downstream sees one shape, and the `kind` is what decides between a
 * banner and a quiet line — never a word sniffed out of a sentence.
 *
 * Not Jira's alone: the composer's GitHub tab reads `gh`'s refusals through the
 * same shape (`GhFailure` in Rust is `jira::Failure`'s twin for exactly this
 * reason), and a second normaliser beside it would be a second place to forget
 * that an exception is not a refusal. */
function failureOf(error) {
  if (error && typeof error === "object" && typeof error.kind === "string") {
    return { kind: error.kind, message: String(error.message ?? "") };
  }
  return { kind: "unknown", message: String(error) };
}

/* The kinds drawn as a quiet line rather than as an error.
 *
 * Both are states of the world, not of this window: nothing was refused and
 * nothing is broken, there is simply nothing to ask right now. Drawing them
 * red would make a laptop on a plane look like a laptop with a bad token —
 * and the answer to that is to reconnect, which is exactly the wrong thing to
 * do. Rust decides which kinds these are (`FailureKind::is_quiet`); this list
 * is the same judgement spelled for the DOM. */
const JIRA_QUIET_FAILURES = new Set(["offline", "disconnected"]);

function jiraIssueSite(issue, context = {}) {
  const siteId = issue.site_id ?? context.site_id
    ?? (jiraStatus.sites.length === 1 ? jiraStatus.sites[0].id : "unknown");
  const site = jiraStatus.sites.find((held) => held.id === siteId);
  return {
    ...issue,
    site_id: String(siteId),
    site_name: String(issue.site_name ?? context.site_name ?? site?.display_name
      ?? site?.site_url ?? siteId),
  };
}

/* Multi-site reads may return either a flat successful list or a report with
 * per-site buckets. Normalise both without throwing away a healthy site's rows
 * merely because another bucket contains a failure. */
function normalizeJiraIssueReport(answer) {
  const issues = [];
  const failures = [];
  const interpretedAsText = answer?.interpreted_as_text === true
    || answer?.interpretedAsText === true;
  const addFailure = (raw, context = {}) => {
    if (!raw) return;
    const source = raw.failure ?? raw.error ?? raw;
    const failure = failureOf(source);
    const siteId = String(raw.site_id ?? context.site_id ?? "unknown");
    const site = jiraStatus.sites.find((held) => held.id === siteId);
    failures.push({
      ...failure,
      site_id: siteId,
      site_name: String(raw.site_name ?? context.site_name ?? (site ? jiraSiteName(site) : null)
        ?? raw.site_id ?? context.site_id
        ?? t("task.jira.site", "Jira 사이트")),
    });
  };
  const addBucket = (bucket) => {
    if (!bucket || typeof bucket !== "object") return;
    const context = {
      site_id: bucket.site_id,
      site_name: bucket.site_name,
    };
    if (Array.isArray(bucket.issues)) {
      for (const issue of bucket.issues) issues.push(jiraIssueSite(issue, context));
    }
    addFailure(bucket.failure ?? bucket.error, context);
  };

  if (Array.isArray(answer)) {
    if (answer.some((row) => Array.isArray(row?.issues) || row?.failure || row?.error)) {
      for (const bucket of answer) addBucket(bucket);
    } else {
      for (const issue of answer) issues.push(jiraIssueSite(issue));
    }
    return { issues, failures, interpreted_as_text: false };
  }
  if (!answer || typeof answer !== "object") {
    return { issues, failures, interpreted_as_text: false };
  }
  if (Array.isArray(answer.issues)) {
    for (const issue of answer.issues) issues.push(jiraIssueSite(issue));
  }
  const buckets = Array.isArray(answer.results) ? answer.results
    : Array.isArray(answer.sites) ? answer.sites
      : [];
  for (const bucket of buckets) addBucket(bucket);
  for (const failure of answer.failures ?? answer.errors ?? []) addFailure(failure);
  return { issues, failures, interpreted_as_text: interpretedAsText };
}

/* 무엇이 목록을 만드는가 — 프리셋 하나, 아니면 사람이 친 JQL.
 *
 * 기본은 백엔드의 기본과 같은 「담당」이다(`Preset::default`), 그래서 첫
 * 화면이 그리는 목록은 이 조각 이전과 한 줄도 다르지 않다. */
let jiraPreset = "assigned";
let jiraSearchTimer = null;

/* 300ms. Orca의 `TASK_SEARCH_DEBOUNCE_MS` 실측값이고, 이 창이 검색을 하나
 * 더 붙일 때 같이 읽으라고 상수다 — GitHub 쪽은 실측이 750이라 이 값이
 * 아니다(1-g70에서 그쪽 상수로). */
const JIRA_SEARCH_DEBOUNCE_MS = 300;

/* 지금 물어야 할 JQL, 없으면 빈 문자열. 다듬는 것은 양끝 공백뿐 — 따옴표도
 * 검사도 붙이지 않는다. 문법의 주인은 Jira이고, 여기서 거르면 Jira가 답했을
 * 질의를 이 창이 대신 거절하게 된다. */
function jiraSearchText() {
  return el("jira-search").value.trim();
}

/* 고른 칩은 검색칸이 비어 있을 때에만 켜진다(실측: `!input && chosen`).
 * 목록을 만든 것이 무엇인지 화면이 두 곳에서 말하지 않게 하는 규칙이다 —
 * 칩이 켜진 채로 검색 결과가 서 있으면 둘 다 거짓말이 된다. */
function paintJiraTools() {
  const searching = jiraSearchText() !== "";
  for (const chip of el("jira-presets").querySelectorAll("[data-jira-preset]")) {
    const active = !searching && chip.dataset.jiraPreset === jiraPreset;
    chip.classList.toggle("is-active", active);
    chip.setAttribute("aria-pressed", String(active));
  }
  el("jira-search-clear").hidden = !searching;
}

/* 다음 질의를 예약한다. 한 글자마다 Jira에 묻지 않기 위한 것이고, 커밋을
 * 서두르는 손(Enter·칩·지우기)은 예약을 먼저 취소한 뒤 스스로 부른다. */
function scheduleJiraSearch() {
  clearTimeout(jiraSearchTimer);
  jiraSearchTimer = setTimeout(() => void loadJiraIssues(), JIRA_SEARCH_DEBOUNCE_MS);
}

function loadJiraIssuesNow() {
  clearTimeout(jiraSearchTimer);
  void loadJiraIssues();
}

async function loadJiraIssues() {
  const generation = ++jiraIssuesGeneration;
  paintJiraSearchInterpretation(false);
  showJiraOnly("jira-loading");
  const jql = jiraSearchText();
  // 한 자리에서만 그려진다: 무엇을 물으러 가든 도구단은 그 질문을 말한다.
  paintJiraTools();
  let answer;
  try {
    // 두 명령이지 두 길이 아니다: 세대 표도, 실패의 갈래도, 스켈레톤을
    // 내리는 손도 아래 하나뿐이다.
    answer = jql
      ? await invoke("jira_search_issues", { jql, siteId: selectedJiraSiteId() })
      : await invoke("jira_issues", { preset: jiraPreset, siteId: selectedJiraSiteId() });
  } catch (error) {
    if (generation !== jiraIssuesGeneration) return;
    // The skeleton comes down on every road out of here — `showJiraOnly` is
    // what guarantees that, because a list that spins forever because one
    // branch forgot is the exact failure the quiet state exists to prevent.
    const failure = failureOf(error);
    const quiet = JIRA_QUIET_FAILURES.has(failure.kind);
    showJiraOnly(quiet ? "jira-quiet" : "jira-error", failure.message);
    return;
  }
  if (generation !== jiraIssuesGeneration) return;
  const report = normalizeJiraIssueReport(answer);
  jiraIssues = report.issues;
  jiraIssueFailures = report.failures;
  paintJiraSearchInterpretation(Boolean(jql) && report.interpreted_as_text);
  paintJiraIssues();
}

/* One group per status, in the order the rows arrived.
 *
 * Orca orders its groups by the project's board columns, which is a second
 * request per project (`jira:getProjectStatusOrder`) and degrades to nothing
 * when it fails — the list still draws (measured, 1-i). This window ships
 * that degraded case as its only case: the rows come back newest-first, so
 * first-seen order puts the group somebody touched most recently on top,
 * which is the same question the sort answers. */
function groupJiraIssues(rows) {
  const groups = new Map();
  for (const issue of rows) {
    const name = issue.status || "";
    if (!groups.has(name)) groups.set(name, []);
    groups.get(name).push(issue);
  }
  return groups;
}

/* Status category → the signal token that already means this in this window.
 *
 * Measured (`getJiraStatusTone`, 1-i): the tone is read off the CATEGORY and
 * never off the status name, because a site can call its statuses anything.
 * Three of them, and everything Jira does not call done or in-progress —
 * including its own `new` — takes the muted default. Which colour each one
 * wears is this window's answer and not Orca's; the stylesheet says why. */
const JIRA_STATUS_TONE = {
  done: "is-done",
  doing: "is-doing",
  todo: "",
};

function paintJiraIssues() {
  const list = el("jira-list");
  list.replaceChildren();
  if (jiraIssues.length === 0 && jiraIssueFailures.length === 0) {
    // 빈 답에는 두 가지가 있고, 사람이 할 일이 서로 다르다: 검색이 서 있으면
    // 질의를 고칠 수 있고, 프리셋이라면 고칠 것이 없다. `say`로 말하는 것은
    // 언어가 바뀔 때 열쇠의 문장이 이 판단을 덮어쓰지 않게 하기 위해서다.
    say(el("jira-none-body"), () => (jiraSearchText()
      ? t("task.jira.emptySearchHint", "다른 JQL 쿼리를 시도해 보세요.")
      : t("task.jira.emptyPresetHint", "선택한 프리셋에 맞는 이슈가 없습니다.")));
    showJiraOnly("jira-none");
    return;
  }
  showJiraOnly("jira-list");
  for (const failure of jiraIssueFailures) {
    const note = document.createElement("p");
    note.className = JIRA_QUIET_FAILURES.has(failure.kind) ? "task-quiet" : "task-error";
    note.dataset.siteId = failure.site_id;
    say(note, () => `${failure.site_name}: ${failure.message}`);
    list.appendChild(note);
  }
  if (jiraIssues.length === 0) return;
  for (const [status, rows] of groupJiraIssues(jiraIssues)) {
    list.appendChild(jiraGroupNode(status, rows));
  }
}

function jiraGroupNode(status, rows) {
  const group = document.createElement("div");
  group.className = "task-jira-group";
  const folded = jiraFolded.has(status);

  const head = document.createElement("button");
  head.type = "button";
  head.className = "task-jira-group-head";
  head.setAttribute("aria-expanded", String(!folded));
  const twist = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  twist.setAttribute("class", `icon icon--twist${folded ? "" : " is-open"}`);
  twist.setAttribute("aria-hidden", "true");
  const use = document.createElementNS("http://www.w3.org/2000/svg", "use");
  use.setAttribute("href", "#i-chevron");
  twist.appendChild(use);
  head.appendChild(twist);
  const label = document.createElement("span");
  label.className = "task-jira-group-name";
  label.textContent = status;
  head.appendChild(label);
  const count = document.createElement("span");
  count.className = "task-jira-group-count";
  count.textContent = String(rows.length);
  head.appendChild(count);
  head.addEventListener("click", () => {
    if (jiraFolded.has(status)) jiraFolded.delete(status);
    else jiraFolded.add(status);
    paintJiraIssues();
  });
  group.appendChild(head);

  const body = document.createElement("div");
  body.className = "task-jira-group-body";
  body.hidden = folded;
  for (const issue of rows) body.appendChild(jiraRowNode(issue));
  group.appendChild(body);
  return group;
}

/* ---- Jira 이슈 카드(1-g57a) ----
 *
 * 실측 JiraIssueWorkspace의 읽기 반절: 행이 아는 사실로 먼저 서고, 상세
 * (보고자·라벨·본문)와 대화가 그 위를 덮는다. 편집·전이는 다음 조각. */
const jiraItemScrim = el("jira-item-scrim");
let jiraItem = null;
let jiraItemAsking = null;
let jiraItemBusy = false;
/* 고쳐쓰기 판(1-g57b) — 폼은 카드가 아는 원문에서 심기고, 고를 것들은
 * 편집을 여는 손이 한 번 데운다. */
let jiraItemEditing = false;
let jiraItemOptions = null;
/* 모달이 키보드를 가져간 자리. 닫힌 뒤 같은 이슈 행으로 돌아가야 Escape가
 * 작업 화면의 맨 처음으로 사람을 내던지는 문이 되지 않는다. */
let jiraItemOpeningFocus = null;

function openJiraItem(issue) {
  const opener = jiraItemOpeningFocus ?? document.activeElement;
  jiraItemOpeningFocus = null;
  const returnKey = opener instanceof HTMLElement ? opener.dataset.key : null;
  jiraItem = { site: issue.site_id, key: issue.key, url: issue.url };
  const shut = el("jira-item-shut");
  // 아이콘 버튼의 같은 이름이 말풍선과 보조기술에 함께 간다.
  labelButton(shut, t("app.close", "닫기"));
  showModal(jiraItemScrim, {
    animated: true,
    opener,
    fallback: () => [...document.querySelectorAll(".task-jira-row")]
      .find((row) => row.dataset.key === returnKey),
  });
  paintJiraItemHead(issue);
  el("jira-item-meta").textContent = "";
  el("jira-item-labels").hidden = true;
  el("jira-item-labels").replaceChildren();
  el("jira-item-body").replaceChildren();
  el("jira-item-attachments-head").hidden = true;
  el("jira-item-attachments-head").textContent = "";
  el("jira-item-attachments").replaceChildren();
  el("jira-item-comments-head").textContent = "";
  el("jira-item-comments").replaceChildren();
  el("jira-item-error").hidden = true;
  jiraItemEditing = false;
  jiraItemOptions = null;
  el("jira-item-edit").hidden = true;
  el("jira-item-edit-form").hidden = true;
  const commentField = el("jira-item-comment-field");
  const commentWord = t(
    "task.jira.commentPlaceholder",
    "{{key}}에 댓글…",
    { key: issue.key },
  );
  commentField.placeholder = commentWord;
  commentField.setAttribute("aria-label", commentWord);
  void readJiraItem();
}

function closeJiraItem() {
  if (jiraItemBusy || jiraItemScrim.classList.contains("is-closing")) return;
  hideModal(jiraItemScrim, { animated: true });
  jiraItem = null;
  jiraItemAsking = null;
  el("jira-item-comment-field").value = "";
}

/* 머리는 두 번 그려진다 — 행이 아는 만큼 먼저, 상세가 온 뒤 다시(GitLab
 * 카드의 그 규칙). 상태 낱말과 알약 톤은 목록 행의 그 손 그대로다. */
function paintJiraItemHead(issue) {
  el("jira-item-key").textContent = issue.key;
  el("jira-item-title").textContent = issue.title ?? "";
  const pill = el("jira-item-state");
  pill.className =
    `task-jira-status detail-card-state ${JIRA_STATUS_TONE[issue.category] ?? ""}`.trim();
  pill.textContent = issue.status ?? "";
}

function sayJiraItemTrouble(failure, fallback) {
  const line = el("jira-item-error");
  line.hidden = false;
  say(line, () => failure?.message || fallback());
}

async function readJiraItem() {
  const at = jiraItem;
  if (!at) return;
  const asking = [at.site, at.key].join(" ");
  jiraItemAsking = asking;
  el("jira-item-loading").hidden = false;
  el("jira-item-error").hidden = true;
  // 상세와 대화는 서로를 기다리지 않는다 — 카드가 반쪽씩 채워지는 쪽이,
  // 둘 다 올 때까지 비어 있는 쪽보다 낫다.
  const detailAsk = invoke("jira_issue_detail", { siteId: at.site, key: at.key });
  const talkAsk = invoke("jira_issue_comments", { siteId: at.site, key: at.key });
  let detail = null;
  try {
    detail = await detailAsk;
  } catch (error) {
    if (jiraItemAsking !== asking) return;
    el("jira-item-loading").hidden = true;
    sayJiraItemTrouble(failureOf(error), () =>
      t("task.jira.itemFailed", "이슈를 불러오지 못했습니다"));
  }
  if (jiraItemAsking !== asking) return;
  if (detail) {
    el("jira-item-loading").hidden = true;
    paintJiraItem(detail);
  }
  let talk = null;
  try {
    talk = await talkAsk;
  } catch (error) {
    if (jiraItemAsking !== asking) return;
    sayJiraItemTrouble(failureOf(error), () =>
      t("task.jira.talkFailed", "댓글을 불러오지 못했습니다"));
    return;
  }
  if (jiraItemAsking !== asking) return;
  paintJiraTalk(Array.isArray(talk) ? talk : []);
}

function paintJiraItem(detail) {
  if (!jiraItem) return;
  jiraItem.url = detail.url || jiraItem.url;
  // 편집 폼은 상세가 말한 원문에서 시작한다(MR 카드의 그 규칙).
  jiraItem.told = detail;
  jiraItemEditing = false;
  paintJiraItemEdit();
  paintJiraItemHead(detail);
  // 보고자·담당자·우선순위·생성일 — 있는 사실만 · 로 잇는다.
  say(el("jira-item-meta"), () => [
    detail.reporter
      ? t("task.jira.reporterWord", "보고자 {{name}}", { name: detail.reporter })
      : "",
    detail.assignee
      ? t("task.jira.assigneeWord", "담당자 {{name}}", { name: detail.assignee })
      : "",
    detail.priority ?? "",
    jiraDayWord(detail.created),
  ].filter(Boolean).join(" · "));
  const labels = el("jira-item-labels");
  labels.hidden = detail.labels.length === 0;
  labels.replaceChildren(...detail.labels.map((name) => {
    const chip = document.createElement("span");
    chip.className = "gl-item-label";
    chip.textContent = name;
    return chip;
  }));
  const body = el("jira-item-body");
  body.replaceChildren();
  const blocks = Array.isArray(detail.blocks) ? detail.blocks : [];
  if (blocks.length) {
    // ADF는 백엔드가 이미 구조로 내렸다 — 여기서는 갈래마다 제 요소로만
    // 세우고, 값은 전부 textContent라 어떤 본문도 마크업이 되지 못한다.
    // 번호 목록의 번호는 그리는 이 손이 센다: 깊이별로, 목록이 끊기면 처음부터.
    let ordinals = [];
    for (const block of blocks) {
      if (block?.kind === "listItem") {
        const depth = Math.max(0, block.depth ?? 0);
        ordinals.length = depth + 1;
        ordinals[depth] = block.ordered ? (ordinals[depth] ?? 0) + 1 : 0;
        body.appendChild(jiraBodyBlockNode(block, ordinals[depth]));
        continue;
      }
      ordinals = [];
      body.appendChild(jiraBodyBlockNode(block, 0));
    }
  } else if (detail.body.trim()) {
    // 구조가 없으면(옛 백엔드·Server 평문) 글로 — 문단으로만 세운다.
    for (const line of detail.body.split("\n")) {
      const said = document.createElement("p");
      said.className = "jira-item-line";
      said.textContent = line;
      body.appendChild(said);
    }
  } else {
    const quiet = document.createElement("p");
    quiet.className = "gl-item-quiet";
    say(quiet, () => t("task.jira.noBody", "설명이 없습니다."));
    body.appendChild(quiet);
  }
  paintJiraAttachments(detail);
}

/* 본문 덩이 하나 — 백엔드 `BodyBlock`의 갈래 그대로, 모르는 갈래는 문단이다. */
function jiraBodyBlockNode(block, ordinal) {
  switch (block?.kind) {
    case "heading": {
      const head = document.createElement("p");
      head.className = "jira-item-line jira-item-heading";
      head.dataset.level = String(Math.min(Math.max(block.level ?? 1, 1), 6));
      head.textContent = block.text ?? "";
      return head;
    }
    case "listItem": {
      const row = document.createElement("p");
      row.className = "jira-item-line jira-item-bullet";
      // 깊이는 값이고 들여쓰기는 CSS의 것이다 — 계단은 토큰 간격의 배수.
      row.style.setProperty("--jira-bullet-depth", String(Math.min(block.depth ?? 0, 4)));
      const mark = document.createElement("span");
      mark.className = "jira-item-bullet-mark";
      mark.setAttribute("aria-hidden", "true");
      mark.textContent = block.ordered ? `${ordinal}.` : "•";
      row.append(mark, document.createTextNode(block.text ?? ""));
      return row;
    }
    case "code": {
      const pre = document.createElement("pre");
      pre.className = "jira-item-code";
      pre.textContent = block.text ?? "";
      return pre;
    }
    case "quote": {
      const said = document.createElement("blockquote");
      said.className = "jira-item-line jira-item-quote";
      said.textContent = block.text ?? "";
      return said;
    }
    case "rule": {
      const rule = document.createElement("hr");
      rule.className = "jira-item-rule";
      return rule;
    }
    case "media": {
      // 미디어 노드의 id는 첨부 id가 아니다(백엔드의 그 기록) — 여기서는
      // 자리 표시가 정직하고, 그림은 아래 첨부 줄의 미리보기가 맡는다.
      const seat = document.createElement("p");
      seat.className = "jira-item-line jira-item-media";
      const alt = typeof block.alt === "string" ? block.alt.trim() : "";
      if (alt) {
        say(seat, () => t("task.jira.media", "미디어: {{name}}", { name: alt }));
      } else {
        say(seat, () => t("task.jira.mediaUnnamed", "첨부 미디어"));
      }
      return seat;
    }
    case "table": {
      const wrap = document.createElement("div");
      wrap.className = "jira-item-table-wrap";
      const table = document.createElement("table");
      table.className = "jira-item-table";
      for (const cells of Array.isArray(block.rows) ? block.rows : []) {
        const row = document.createElement("tr");
        for (const cell of Array.isArray(cells) ? cells : []) {
          const seat = document.createElement("td");
          seat.textContent = typeof cell === "string" ? cell : "";
          row.appendChild(seat);
        }
        table.appendChild(row);
      }
      wrap.appendChild(table);
      if (block.truncated) {
        const cut = document.createElement("p");
        cut.className = "gl-item-quiet";
        say(cut, () => t("task.jira.tableTruncated", "표 일부 생략"));
        wrap.appendChild(cut);
      }
      return wrap;
    }
    default: {
      const said = document.createElement("p");
      said.className = "jira-item-line";
      said.textContent = block?.text ?? "";
      return said;
    }
  }
}

/* 미리보기의 세션 캐시 — 같은 카드를 다시 열어도 데이터 URI를 다시 청하지
 * 않는다(백엔드 캐시는 디스크의, 이 Map은 메모리의 재사용). 가득 차면
 * 비운다: 캐시는 재사용이지 저장소가 아니다. */
const jiraPreviewCache = new Map();
const JIRA_PREVIEW_CACHE_MAX = 64;

function jiraAttachmentPreviewOf(site, key, id) {
  const at = [site, key, id].join(" ");
  if (!jiraPreviewCache.has(at)) {
    if (jiraPreviewCache.size >= JIRA_PREVIEW_CACHE_MAX) jiraPreviewCache.clear();
    jiraPreviewCache.set(
      at,
      invoke("jira_attachment_preview", {
        siteId: site,
        key,
        attachmentId: id,
      }).catch((error) => {
        // 실패는 캐시되지 않는다 — 다음 열람이 다시 물을 수 있어야 한다.
        jiraPreviewCache.delete(at);
        throw error;
      }),
    );
  }
  return jiraPreviewCache.get(at);
}

/* 첨부 — 메타데이터 행이 먼저 서고, 그림인 것만 한정된 데이터 URI 미리보기가
 * 그 아래 따라온다. 바이트의 한도와 검문은 백엔드의 것이고, 여기는 온 것이
 * 그림 URI인지 한 번 더 볼 뿐이다. */
function paintJiraAttachments(detail) {
  const head = el("jira-item-attachments-head");
  const box = el("jira-item-attachments");
  const rows = Array.isArray(detail.attachments) ? detail.attachments : [];
  head.hidden = rows.length === 0;
  if (rows.length === 0) {
    head.textContent = "";
    box.replaceChildren();
    return;
  }
  say(head, () => t("task.jira.attachments", "첨부 {{count}}", { count: rows.length }));
  const at = jiraItem;
  box.replaceChildren(...rows.map((one) => {
    const row = document.createElement("div");
    row.className = "jira-item-attachment";
    const name = document.createElement("span");
    name.className = "jira-item-attachment-name";
    name.textContent = one.name ?? "";
    const fact = document.createElement("span");
    fact.className = "jira-item-attachment-fact";
    fact.textContent = [one.mime, resourceFormatMemory(one.size ?? 0)]
      .filter(Boolean)
      .join(" · ");
    row.append(name, fact);
    if (at && typeof one.mime === "string" && one.mime.startsWith("image/")) {
      void jiraAttachmentPreviewOf(at.site, at.key, one.id)
        .then((preview) => {
          if (jiraItem !== at || !row.isConnected) return;
          const uri = preview?.data_uri ?? "";
          if (!uri.startsWith("data:image/")) return;
          const img = document.createElement("img");
          img.className = "jira-item-attachment-img";
          img.alt = one.name ?? "";
          img.src = uri;
          row.appendChild(img);
        })
        .catch(() => {
          if (jiraItem !== at || !row.isConnected) return;
          const quiet = document.createElement("span");
          quiet.className = "gl-item-quiet jira-item-attachment-fail";
          say(quiet, () =>
            t("task.jira.attachmentPreviewFailed", "미리보기를 불러오지 못했습니다"));
          row.appendChild(quiet);
        });
    }
    return row;
  }));
}

function jiraDayWord(stamp) {
  if (!stamp) return "";
  const at = new Date(stamp);
  return Number.isNaN(at.getTime()) ? "" : at.toLocaleDateString();
}

function paintJiraTalk(voices) {
  say(el("jira-item-comments-head"), () =>
    t("task.jira.comments", "댓글 {{count}}", { count: voices.length }));
  el("jira-item-comments").replaceChildren(...voices.map((voice) =>
    itemVoiceCard("gl", () => [
      voice.author,
      jiraDayWord(voice.created),
    ].filter(Boolean).join(" · "), voice.body ?? "")));
}

/* 읽기와 고쳐쓰기 중 하나만 선다 — [편집]은 상세가 도착해야 선다. */
function paintJiraItemEdit() {
  el("jira-item-edit-form").hidden = !jiraItemEditing;
  el("jira-item-edit").hidden = jiraItemEditing || !jiraItem?.told;
  el("jira-item-body").hidden = jiraItemEditing;
  el("jira-item-labels").hidden =
    jiraItemEditing || (jiraItem?.told?.labels ?? []).length === 0;
  el("jira-item-attachments-head").hidden =
    jiraItemEditing || (jiraItem?.told?.attachments ?? []).length === 0;
  el("jira-item-attachments").hidden = jiraItemEditing;
}

/* 콤보 하나: 「그대로」가 기본이고, 온 것들이 그 아래 선다. */
function jiraEditPick(select, keepWord, rows, extra = []) {
  select.replaceChildren(...[
    (() => {
      const keep = document.createElement("option");
      keep.value = "";
      keep.textContent = keepWord;
      return keep;
    })(),
    ...extra,
    ...rows.map((row) => {
      const one = document.createElement("option");
      one.value = row.id;
      one.textContent = row.name;
      return one;
    }),
  ]);
  select.value = "";
}

function paintJiraEditPicks() {
  const held = jiraItemOptions ?? { transitions: [], priorities: [], users: [] };
  jiraEditPick(
    el("jira-item-edit-status"),
    t("task.jira.keepStatus", "상태 그대로"),
    held.transitions,
  );
  jiraEditPick(
    el("jira-item-edit-priority"),
    t("task.jira.keepPriority", "우선순위 그대로"),
    held.priorities,
  );
  const clear = document.createElement("option");
  clear.value = "__clear__";
  clear.textContent = t("task.jira.clearAssignee", "담당자 해제");
  jiraEditPick(
    el("jira-item-edit-assignee"),
    t("task.jira.keepAssignee", "담당자 그대로"),
    held.users,
    [clear],
  );
}

el("jira-item-edit").addEventListener("click", async () => {
  if (!jiraItem?.told) return;
  el("jira-item-edit-title").value = jiraItem.told.title ?? "";
  el("jira-item-edit-labels").value = (jiraItem.told.labels ?? []).join(", ");
  jiraItemEditing = true;
  paintJiraItemEdit();
  paintJiraEditPicks();
  el("jira-item-scroll").scrollTop = 0;
  el("jira-item-edit-title").focus({ preventScroll: true });
  // 고를 것들은 이 열림이 데운다 — 카드당 한 번.
  if (jiraItemOptions === null) {
    const asking = jiraItemAsking;
    let held = null;
    try {
      held = await invoke("jira_issue_options", {
        siteId: jiraItem.site,
        key: jiraItem.key,
      });
    } catch (error) {
      if (jiraItemAsking === asking) {
        sayJiraItemTrouble(failureOf(error), () =>
          t("task.jira.optionsFailed", "고를 항목을 불러오지 못했습니다"));
      }
      return;
    }
    if (jiraItemAsking !== asking) return;
    jiraItemOptions = held ?? { transitions: [], priorities: [], users: [] };
    if (jiraItemEditing) paintJiraEditPicks();
  }
});

el("jira-item-edit-cancel").addEventListener("click", () => {
  jiraItemEditing = false;
  paintJiraItemEdit();
});

el("jira-item-edit-save").addEventListener("click", async () => {
  if (!jiraItem?.told || jiraItemBusy) return;
  const title = el("jira-item-edit-title").value;
  const wanted = parseGlLabels(el("jira-item-edit-labels").value);
  const had = jiraItem.told.labels ?? [];
  const ask = {
    siteId: jiraItem.site,
    key: jiraItem.key,
    title: title !== jiraItem.told.title ? title : null,
    // 라벨은 통째로 간다(실측 fields.labels) — 차는 같음/다름만 가른다.
    labels:
      wanted.length === had.length && wanted.every((name) => had.includes(name))
        ? null
        : wanted,
    priorityId: el("jira-item-edit-priority").value || null,
    assigneeId: (() => {
      const picked = el("jira-item-edit-assignee").value;
      if (!picked) return null;
      return picked === "__clear__" ? "" : picked;
    })(),
    transitionId: el("jira-item-edit-status").value || null,
  };
  const quiet = [ask.title, ask.labels, ask.priorityId, ask.assigneeId, ask.transitionId]
    .every((field) => field == null);
  if (quiet) {
    // 바뀐 것이 없으면 보낼 것도 없다 — 폼만 접는다.
    jiraItemEditing = false;
    paintJiraItemEdit();
    return;
  }
  jiraItemBusy = true;
  const save = el("jira-item-edit-save");
  save.disabled = true;
  try {
    await invoke("jira_update_issue", ask);
    // 상태가 바뀌었으면 목록의 그 행도 다시 서야 한다.
    await readJiraItem();
    loadJiraIssuesNow();
  } catch (error) {
    sayJiraItemTrouble(failureOf(error), () =>
      t("task.jira.updateFailed", "변경을 저장하지 못했습니다"));
  } finally {
    jiraItemBusy = false;
    save.disabled = false;
  }
});

el("jira-item-shut").addEventListener("click", closeJiraItem);
jiraItemScrim.addEventListener("mousedown", (event) => {
  if (event.target === jiraItemScrim) closeJiraItem();
});

el("jira-item-open-web").addEventListener("click", () => {
  if (jiraItem) openExternal(jiraItem.url);
});

el("jira-item-open-app").addEventListener("click", () => {
  if (!jiraItem || jiraItemBusy) return;
  const at = jiraItem;
  closeJiraItem();
  // Jira 카드만 닫아서는 작업 화면이 스테이지를 계속 덮는다. 네이티브
  // 브라우저는 DOM보다 위에 놓이므로 `browserCovered()`가 숨김을 보내고,
  // 화면이 깜박인 뒤 열리지 않는 것처럼 보인다. 먼저 작업 화면을 내려
  // 스테이지를 드러낸 다음, + 팔레트와 같은 브라우저 길로 연다.
  setTaskOpen(false);
  // 자리는 + 팔레트에서 마지막으로 고른 그 자리 — 옆에 분할이면 곁 판,
  // 단일 창이면 지금 판의 탭. 탭이 하나도 없는 스테이지(프로젝트 없음)는
  // 어느 쪽이든 탭으로 선다.
  void openBrowserTab(at.url, { beside: lastBrowserPlacement() === "split" });
});

el("jira-item-start").addEventListener("click", () => {
  if (!jiraItem || jiraItemBusy) return;
  const told = jiraItem.told ?? {};
  const issue = {
    site_id: jiraItem.site,
    key: jiraItem.key,
    url: jiraItem.url,
    title: told.title ?? jiraItem.key,
    status: told.status ?? "",
    updated: told.updated ?? null,
  };
  closeJiraItem();
  startWorktreeFromJiraItem(issue);
});

el("jira-item-comment-send").addEventListener("click", async () => {
  if (!jiraItem || jiraItemBusy) return;
  const field = el("jira-item-comment-field");
  if (!field.value.trim()) return;
  jiraItemBusy = true;
  const send = el("jira-item-comment-send");
  send.disabled = true;
  try {
    await invoke("jira_comment_issue", {
      siteId: jiraItem.site,
      key: jiraItem.key,
      body: field.value,
    });
    field.value = "";
    // 방금 쓴 말이 대화에 서야 사람은 그것이 갔는지를 안다(그 집 규칙).
    await readJiraItem();
  } catch (error) {
    sayJiraItemTrouble(failureOf(error), () =>
      t("task.jira.commentFailed", "댓글을 보내지 못했습니다"));
  } finally {
    jiraItemBusy = false;
    send.disabled = false;
  }
});

function jiraRowNode(issue) {
  const row = document.createElement("div");
  row.className = "task-jira-row";
  // Jira keys are unique only inside one site. The DOM identity has to name
  // both or ABC-9 from two selected sites becomes one row to automation and
  // accessibility tooling even though two rows are visible.
  row.dataset.key = `${issue.site_id}:${issue.key}`;
  row.dataset.issueId = `${issue.site_id}:${issue.key}`;
  row.dataset.siteId = issue.site_id;

  const key = document.createElement("span");
  key.className = "task-jira-key";
  const allSites = jiraStatus.selected_site_id === "all" && jiraStatus.sites.length > 1;
  key.textContent = allSites ? `${issue.site_name} · ${issue.key}` : issue.key;
  row.appendChild(key);

  const title = document.createElement("span");
  title.className = "task-jira-title";
  title.textContent = issue.title;
  row.appendChild(title);

  const status = document.createElement("span");
  status.className = `task-jira-status ${JIRA_STATUS_TONE[issue.category] ?? ""}`.trim();
  status.textContent = issue.status;
  row.appendChild(status);

  const priority = document.createElement("span");
  priority.className = "task-jira-priority";
  priority.textContent = issue.priority ?? "—";
  row.appendChild(priority);

  const assignee = document.createElement("span");
  assignee.className = "task-jira-assignee";
  assignee.textContent = issue.assignee ?? t("task.jira.unassigned", "미할당");
  row.appendChild(assignee);

  const updated = document.createElement("span");
  updated.className = "task-jira-updated";
  const updatedAt = Date.parse(issue.updated ?? "");
  updated.textContent = Number.isNaN(updatedAt) ? "" : agoWord(updatedAt, Date.now());
  row.appendChild(updated);

  // 행은 이제 이 창의 카드를 연다(1-g57a) — 브라우저는 카드의 [Jira에서
  // 열기]가 맡는다. `actsAsButton`은 그대로: 키보드의 손이 먼저다.
  actsAsButton(row, (event) => {
    // A click that landed on the row's own action already did something.
    if (event.target.closest(".task-jira-act")) return;
    jiraItemOpeningFocus = row;
    openJiraItem(issue);
  });

  // And the action that is the point of the screen: this issue becomes a
  // checkout. The URL goes into the composer's smart field rather than a name
  // being derived here — `work_item_seed` in Rust is the one classifier, and a
  // second one on this side is how the badge and the branch start disagreeing.
  const act = document.createElement("button");
  act.type = "button";
  act.className = "btn task-jira-act";
  act.dataset.key = issue.key;
  say(act, () => t("task.jira.startWork", "작업 트리"));
  act.addEventListener("click", () => startWorktreeFromJiraItem(issue));
  row.appendChild(act);
  return row;
}

async function startWorktreeFromProviderItem(item, provider) {
  setTaskOpen(false);
  worktreeFormProject = item.project ?? activeProjectPath;
  setWorktreeForm(true);
  if (provider === "github") {
    worktreeGithubItems = [item];
    worktreeGithubAsking = "task-page";
    setWorktreeTab(provider);
    await pickWorktreeGithub(item);
    paintWorktreeGithubList();
  } else {
    worktreeGitlabItems = [item];
    worktreeGitlabAsking = "task-page";
    setWorktreeTab(provider);
    await pickWorktreeGitlab(item);
    paintWorktreeGitlabList();
  }
}

/* Jira and pasted links use the provider-neutral smart field. */
function startWorktreeFromWorkItem(url) {
  setTaskOpen(false);
  setWorktreeForm(true);
  setWorktreeTab("smart");
  wtSpec.value = url;
  fitWorktreeSpec();
  paintWorktreeName();
  void readWorktreeSeed();
}

function startWorktreeFromJiraItem(issue) {
  setTaskOpen(false);
  setWorktreeForm(true);
  setWorktreeTab("smart");
  wtSpec.value = issue.url;
  worktreeJiraDraft = {
    site_id: issue.site_id,
    key: issue.key,
    url: issue.url,
    title: issue.title ?? issue.key,
    status: issue.status ?? "",
    updated: issue.updated ?? null,
  };
  fitWorktreeSpec();
  paintWorktreeName();
  paintWorktreeJira();
  void readWorktreeSeed();
  void loadWorktreeJiraContext();
}

let jiraCreateBusy = false;
let jiraCreateProjectsGeneration = 0;

function jiraCreateProjectControl(tagName) {
  const current = el("jira-create-project");
  if (current.tagName === tagName) return current;
  const control = document.createElement(tagName.toLowerCase());
  control.className = "settings-input";
  control.id = "jira-create-project";
  control.required = true;
  if (tagName === "INPUT") {
    control.type = "text";
    control.autocomplete = "off";
    control.spellcheck = false;
    control.placeholder = "ABC";
  }
  current.replaceWith(control);
  return control;
}

function jiraProjectsOf(answer) {
  const rows = Array.isArray(answer) ? answer
    : Array.isArray(answer?.projects) ? answer.projects
      : [];
  const seen = new Set();
  return rows.flatMap((raw) => {
    const key = String(raw?.key ?? "").trim();
    const name = String(raw?.name ?? "").trim();
    if (!key || !name || seen.has(key)) return [];
    seen.add(key);
    return [{ key, name }];
  });
}

async function loadJiraCreateProjects(siteId, preferredProject = "") {
  const generation = ++jiraCreateProjectsGeneration;
  const preferred = String(preferredProject ?? "").trim();
  const picker = jiraCreateProjectControl("SELECT");
  const waiting = document.createElement("option");
  waiting.value = preferred;
  waiting.textContent = preferred
    || t("task.jira.projectsLoading", "프로젝트 불러오는 중…");
  picker.replaceChildren(waiting);
  picker.value = preferred;
  picker.setAttribute("aria-busy", "true");
  try {
    const answer = await invoke("jira_projects", { siteId });
    if (generation !== jiraCreateProjectsGeneration
        || el("jira-create-site").value !== siteId) return;
    const projects = jiraProjectsOf(answer);
    if (projects.length === 0) throw new Error("no Jira projects");
    if (preferred && !projects.some((project) => project.key === preferred)) {
      projects.unshift({ key: preferred, name: preferred });
    }
    const options = projects.map((project) => {
      const option = document.createElement("option");
      option.value = project.key;
      option.textContent = project.name === project.key
        ? project.key
        : `${project.key} — ${project.name}`;
      return option;
    });
    picker.replaceChildren(...options);
    if (preferred) picker.value = preferred;
    picker.removeAttribute("aria-busy");
  } catch (_error) {
    if (generation !== jiraCreateProjectsGeneration
        || el("jira-create-site").value !== siteId) return;
    const input = jiraCreateProjectControl("INPUT");
    input.value = preferred;
    input.removeAttribute("aria-busy");
  }
}

function closeJiraCreate() {
  if (jiraCreateBusy) return;
  ++jiraCreateProjectsGeneration;
  hideModal(el("jira-create-scrim"));
  el("jira-create-error").hidden = true;
}

function openJiraCreate() {
  const sites = selectedJiraSites().filter((site) =>
    jiraStandingAvailable(effectiveJiraStanding(site)));
  if (sites.length === 0) return;
  const picker = el("jira-create-site");
  picker.replaceChildren(...sites.map((site) => {
    const option = document.createElement("option");
    option.value = site.id;
    option.textContent = jiraSiteName(site);
    return option;
  }));
  const seedIssue = jiraIssues[0];
  if (seedIssue?.site_id && sites.some((site) => site.id === seedIssue.site_id)) {
    picker.value = seedIssue.site_id;
  }
  el("jira-create-site-field").hidden = sites.length === 1;
  el("jira-create-type").value = "Task";
  el("jira-create-summary").value = "";
  el("jira-create-description").value = "";
  el("jira-create-error").hidden = true;
  showModal(el("jira-create-scrim"));
  void loadJiraCreateProjects(picker.value, seedIssue?.project ?? "");
}

el("jira-new").addEventListener("click", openJiraCreate);
el("jira-create-site").addEventListener("change", () => {
  const siteId = el("jira-create-site").value;
  const preferred = jiraIssues.find((issue) => issue.site_id === siteId)?.project ?? "";
  void loadJiraCreateProjects(siteId, preferred);
});
el("jira-create-cancel").addEventListener("click", closeJiraCreate);
el("jira-create-scrim").addEventListener("mousedown", (event) => {
  if (event.target === el("jira-create-scrim")) closeJiraCreate();
});
el("jira-create-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  if (jiraCreateBusy) return;
  const request = {
    siteId: el("jira-create-site").value,
    projectKey: el("jira-create-project").value.trim(),
    issueType: el("jira-create-type").value.trim(),
    summary: el("jira-create-summary").value.trim(),
    description: el("jira-create-description").value.trim() || null,
  };
  if (!request.siteId || !request.projectKey || !request.issueType || !request.summary) return;
  jiraCreateBusy = true;
  el("jira-create-submit").disabled = true;
  try {
    const created = await invoke("jira_create_issue", request);
    jiraCreateBusy = false;
    closeJiraCreate();
    loadJiraIssuesNow();
    if (created?.key && created?.site_id && created?.url) {
      openJiraItem({
        site_id: created.site_id,
        key: created.key,
        url: created.url,
        title: request.summary,
        status: "",
        category: "todo",
      });
    }
  } catch (error) {
    say(el("jira-create-error"), () => String(error));
    el("jira-create-error").hidden = false;
  } finally {
    jiraCreateBusy = false;
    el("jira-create-submit").disabled = false;
  }
});

/* ---- 도구단의 손들(1-g69) ---- */

/* 프리셋을 고르는 것은 검색을 그만두는 것이다.
 *
 * Orca도 칩을 누르면 검색칸을 비운다(실측). 둘을 동시에 세워 두면 목록이
 * 무엇의 답인지 화면이 대답할 수 없고, 그다음에 오는 질문 — 「지우기를
 * 누르면 무엇이 돌아오는가」 — 에도 답이 없다. */
el("jira-presets").addEventListener("click", (event) => {
  const chip = event.target.closest("[data-jira-preset]");
  if (!chip) return;
  jiraPreset = chip.dataset.jiraPreset;
  el("jira-search").value = "";
  loadJiraIssuesNow();
});

el("jira-search").addEventListener("input", () => {
  paintJiraTools();
  scheduleJiraSearch();
});

/* Enter는 기다림을 건너뛴다 — 다만 조합 중인 글자는 아직 질의가 아니다.
 *
 * 한국어 한 음절은 여러 번의 keydown으로 만들어지고, 그 사이의 Enter는 IME가
 * 음절을 확정하는 키다. 그 키로 검색을 보내면 「프로」까지 친 사람이 「프로」를
 * 검색하게 된다. `isComposing`만으로는 부족해 이 창의 다른 문들과 같은 셋을
 * 본다(`Process`와 레거시 229). */
el("jira-search").addEventListener("keydown", (event) => {
  if (event.isComposing || event.key === "Process" || event.keyCode === 229) return;
  if (event.key !== "Enter") return;
  event.preventDefault();
  loadJiraIssuesNow();
});

/* 지우기는 빈 화면이 아니라 고른 프리셋의 목록으로 돌아온다 — 검색을 시작하기
 * 전에 보고 있던 그 목록이다. */
el("jira-search-clear").addEventListener("click", () => {
  el("jira-search").value = "";
  el("jira-search").focus();
  loadJiraIssuesNow();
});

/* ---- Jira's connect dialog ----
 *
 * Every field, toggle and state below is read from
 * `jira-connect-dialog-C8sgaCaJ.js` in full (1-i) — nothing here is guessed.
 * ZeroCode has no remote-runtime target the way Orca does, so the one branch
 * that depended on that (the storage note) is written as the single case
 * that is actually true here, not as a dead conditional. */
const jiraConnectScrim = el("jira-connect-scrim");
let jiraInstanceType = "cloud";
let jiraAuthMethod = "pat";
let jiraConnecting = false;

/* `needsIdentity = !isServer || isServerBasic`, measured verbatim: cloud
 * always pairs an email with its token; a server PAT does not, because the
 * token alone already identifies the account. */
function jiraNeedsIdentity() {
  return jiraInstanceType !== "server" || jiraAuthMethod === "basic";
}

function paintJiraConnectFields() {
  const isServer = jiraInstanceType === "server";
  el("jira-auth-method-field").hidden = !isServer;
  const needsIdentity = jiraNeedsIdentity();
  el("jira-identity-field").hidden = !needsIdentity;
  const asBasic = isServer && jiraAuthMethod === "basic";
  // Which pair of words this form wants is the instance type's answer, not
  // the language's — so it has to survive a language change, which would
  // otherwise put the cloud wording back over a server form.
  say(el("jira-connect-copy"), () =>
    asBasic
      ? t("task.jira.copyBasic", "자체 호스팅 Jira의 기본 URL과 사용자 이름·비밀번호로 이슈를 봅니다.")
      : isServer
        ? t("task.jira.copyServer", "자체 호스팅 Jira의 기본 URL과 개인 액세스 토큰으로 이슈를 봅니다.")
        : t("task.jira.copyCloud", "Jira Cloud 사이트 URL과 Atlassian 이메일, API 토큰으로 이슈를 봅니다."));
  say(el("jira-site-url-label"), () =>
    isServer ? t("task.jira.siteUrl", "Jira 사이트 URL") : t("task.jira.siteUrlCloud", "Jira Cloud 사이트 URL"));
  say(el("jira-identity-label"), () =>
    asBasic ? t("task.jira.username", "사용자 이름") : t("task.jira.email", "이메일"));
  say(el("jira-token-label"), () =>
    asBasic ? t("task.jira.password", "비밀번호") : t("task.jira.token", "API 토큰"));
  el("jira-site-url").placeholder = isServer
    ? "https://jira.example.com"
    : "https://example.atlassian.net";
  el("jira-identity").placeholder = asBasic
    ? t("task.jira.usernamePlaceholder", "사용자 이름")
    : "you@example.com";
  el("jira-token").placeholder = asBasic
    ? t("task.jira.passwordPlaceholder", "Jira 계정 비밀번호")
    : isServer
      ? t("task.jira.patPlaceholder", "Jira 개인 액세스 토큰")
      : t("task.jira.tokenPlaceholder", "Atlassian API 토큰");
  // 도움말도 같은 답을 따른다 — 셋 중 지금의 길 하나만 선다.
  el("jira-help-cloud").hidden = isServer;
  el("jira-help-server").hidden = !isServer || asBasic;
  el("jira-help-basic").hidden = !asBasic;
  paintJiraConnectSubmit();
}

function jiraCanSubmit() {
  const siteUrl = el("jira-site-url").value.trim();
  const identity = el("jira-identity").value.trim();
  const token = el("jira-token").value;
  return Boolean(siteUrl && (!jiraNeedsIdentity() || identity) && token && !jiraConnecting);
}

function paintJiraConnectSubmit() {
  const button = el("jira-connect-submit");
  button.disabled = !jiraCanSubmit();
  button.querySelector("span:not(.jira-connect-spin)").textContent = jiraConnecting
    ? t("task.jira.connecting", "확인 중…")
    : t("task.jira.connectSubmit", "연결");
  // 확인하는 동안은 다이얼로그 전체가 숨을 죽인다(Orca 실측) — 자격이 심사대
  // 위에 있는데 자격을 고치거나 문을 닫는 손이 있으면 안 된다.
  button.querySelector(".jira-connect-spin").hidden = !jiraConnecting;
  for (const field of ["jira-site-url", "jira-identity", "jira-token"]) {
    el(field).disabled = jiraConnecting;
  }
  for (const group of ["jira-instance-type", "jira-auth-method"]) {
    for (const pick of el(group).querySelectorAll(".segment-btn")) {
      pick.disabled = jiraConnecting;
    }
  }
  el("jira-connect-cancel").disabled = jiraConnecting;
}

/* 오류는 다음 손질이 지운다(Orca clearErrorOnEdit) — 고친 값 옆에 옛 오류가
 * 서 있으면 그 오류가 지금 값의 것으로 읽힌다. */
function clearJiraConnectError() {
  const said = el("jira-connect-error");
  if (said.hidden) return;
  said.hidden = true;
  el("jira-token").removeAttribute("aria-invalid");
  el("jira-token").removeAttribute("aria-describedby");
}

function selectSegment(groupId, value) {
  for (const button of el(groupId).querySelectorAll(".segment-btn")) {
    const active = button.dataset.value === value;
    button.classList.toggle("is-active", active);
    button.setAttribute("aria-selected", String(active));
  }
}

function clearJiraConnectDialogFields() {
  jiraConnecting = false;
  el("jira-site-url").value = "";
  el("jira-identity").value = "";
  el("jira-token").value = "";
  clearJiraConnectError();
  el("jira-connect-error").textContent = "";
  paintJiraConnectFields();
}

function openJiraConnectDialog() {
  jiraInstanceType = "cloud";
  jiraAuthMethod = "pat";
  selectSegment("jira-instance-type", "cloud");
  selectSegment("jira-auth-method", "pat");
  clearJiraConnectDialogFields();
  showModal(jiraConnectScrim);
}

function dismissJiraConnectDialog() {
  hideModal(jiraConnectScrim);
}

function closeJiraConnectDialog() {
  hideModal(jiraConnectScrim);
  clearJiraConnectDialogFields();
}

el("jira-connect-open").addEventListener("click", openJiraConnectDialog);
// 확인 중에는 어느 손으로도 닫히지 않는다 — 심사대 위의 자격이 주인 잃은
// 요청이 되므로(Orca handleOpenChange 실측).
el("jira-connect-cancel").addEventListener("click", () => {
  if (!jiraConnecting) closeJiraConnectDialog();
});
jiraConnectScrim.addEventListener("mousedown", (event) => {
  if (event.target === jiraConnectScrim && !jiraConnecting) closeJiraConnectDialog();
});

for (const field of ["jira-site-url", "jira-identity", "jira-token"]) {
  el(field).addEventListener("input", () => {
    clearJiraConnectError();
    paintJiraConnectSubmit();
  });
}

el("jira-instance-type").addEventListener("click", (event) => {
  const button = event.target.closest(".segment-btn");
  if (!button || jiraConnecting) return;
  jiraInstanceType = button.dataset.value;
  selectSegment("jira-instance-type", jiraInstanceType);
  // Orca's own `clearCredentialsOnModeSwitch`, measured: a credential must
  // not survive into a mode it was never checked against — nor its error.
  el("jira-identity").value = "";
  el("jira-token").value = "";
  clearJiraConnectError();
  paintJiraConnectFields();
});

el("jira-auth-method").addEventListener("click", (event) => {
  const button = event.target.closest(".segment-btn");
  if (!button || jiraConnecting) return;
  jiraAuthMethod = button.dataset.value;
  selectSegment("jira-auth-method", jiraAuthMethod);
  el("jira-identity").value = "";
  el("jira-token").value = "";
  clearJiraConnectError();
  paintJiraConnectFields();
});

el("jira-connect-submit").addEventListener("click", async () => {
  if (!jiraCanSubmit()) return;
  const generation = beginJiraMutation("connecting");
  jiraConnecting = true;
  paintJiraConnectSubmit();
  clearJiraConnectError();
  try {
    const canonical = await invoke("jira_connect", {
      siteUrl: el("jira-site-url").value.trim(),
      // `authType` chooses the backend's allowed REST family (cloud/server).
      // Inside server, its wire contract chooses Bearer PAT only when identity
      // is empty and Basic otherwise. Derive that value from the selected
      // method instead of trusting a hidden input to have stayed empty.
      email: jiraNeedsIdentity() ? el("jira-identity").value.trim() : "",
      apiToken: el("jira-token").value,
      authType: jiraInstanceType,
    });
    if (generation !== jiraMutationGeneration) return;
    closeJiraConnectDialog();
    if (!acceptJiraMutation(canonical)) await refreshJiraStatus();
  } catch (error) {
    if (generation !== jiraMutationGeneration) return;
    jiraConnecting = false;
    paintJiraConnectSubmit();
    const failure = await recoverJiraMutation(generation, error);
    if (!failure) return;
    // The measured fallback: an exception with no message reads as this
    // string, not as "undefined" or an empty box.
    el("jira-connect-error").textContent = failure.message
      || t("task.jira.connectFailed", "Connection failed");
    el("jira-connect-error").hidden = false;
    // 스크린리더에게도 같은 문장이 닿는다 — 오류는 토큰 칸의 것(Orca 실측).
    el("jira-token").setAttribute("aria-invalid", "true");
    el("jira-token").setAttribute("aria-describedby", "jira-connect-error");
    paintJiraConnectSubmit();
  }
});

/* ---- Linear: the same task-source state machine over one GraphQL account. */

const EMPTY_LINEAR_STATUS = Object.freeze({
  connected: false,
  credential: "missing",
  connection: null,
});
let linearStatus = { ...EMPTY_LINEAR_STATUS };
let linearStatusLoading = false;
let linearStatusError = null;
let linearStatusGeneration = 0;
let linearIssuesGeneration = 0;
let linearMutationGeneration = 0;
let linearIssues = [];
let linearPreset = "assigned";
let linearSearchTimer = null;
let linearConnecting = false;
let linearCreateBusy = false;
const linearFolded = new Set();

const LINEAR_SURFACES = [
  "linear-loading",
  "linear-empty",
  "linear-error",
  "linear-quiet",
  "linear-none",
  "linear-list",
];
const LINEAR_TOOL_SURFACES = new Set([
  "linear-loading",
  "linear-error",
  "linear-quiet",
  "linear-none",
  "linear-list",
]);

function normalizeLinearStatus(answer) {
  const connection = answer?.connection && typeof answer.connection === "object"
    ? {
        ...answer.connection,
        teams: Array.isArray(answer.connection.teams) ? answer.connection.teams : [],
      }
    : null;
  const credential = String(answer?.credential ?? "missing");
  return {
    connected: Boolean(answer?.connected && connection && credential === "available"),
    credential,
    connection,
  };
}

function showLinearOnly(id, said) {
  showOneSurface(LINEAR_SURFACES, id, said);
  el("linear-tools").hidden = !LINEAR_TOOL_SURFACES.has(id);
}

function paintLinearIntegration() {
  const summary = el("integration-linear-summary");
  const error = el("integration-linear-error");
  const connection = linearStatus.connection;
  if (linearStatusLoading) {
    summary.dataset.state = "checking";
    say(summary, () => t("settings.integrations.checking", "확인 중…"));
  } else if (linearStatusError) {
    summary.dataset.state = "error";
    say(summary, () => t("settings.integrations.checkFailed", "확인 실패"));
  } else if (linearStatus.connected) {
    summary.dataset.state = "connected";
    say(summary, () => t("settings.integrations.connected", "연결됨"));
  } else if (connection) {
    summary.dataset.state = "auth-error";
    say(summary, () => t("settings.integrations.credentialMissing", "자격증명 없음"));
  } else {
    summary.dataset.state = "disconnected";
    say(summary, () => t("settings.integrations.disconnected", "연결 안 됨"));
  }
  error.hidden = !linearStatusError;
  if (linearStatusError) say(error, () => linearStatusError);
  el("integration-linear-identity").textContent = connection
    ? `${connection.organization_name} · ${connection.user_name}`
    : t("settings.integrations.none", "없음");
  el("integration-linear-teams").textContent = connection?.teams?.length
    ? connection.teams.map((team) => team.name).join(", ")
    : t("settings.integrations.none", "없음");
  el("integration-linear-disconnect").hidden = !connection;
  el("integration-linear-add").hidden = Boolean(connection);
  el("integration-linear-refresh").disabled = linearStatusLoading || linearConnecting;
  paintTaskContext();
}

async function refreshLinearStatus(preservedError = null) {
  const generation = ++linearStatusGeneration;
  ++linearIssuesGeneration;
  linearStatusLoading = true;
  linearStatusError = null;
  paintLinearIntegration();
  if (taskSource === "linear" && !taskView.hidden) showLinearOnly("linear-loading");
  try {
    const answer = await invoke("linear_status");
    if (generation !== linearStatusGeneration) return;
    linearStatus = normalizeLinearStatus(answer);
  } catch (error) {
    if (generation !== linearStatusGeneration) return;
    linearStatusError = preservedError || String(error);
  }
  if (generation !== linearStatusGeneration) return;
  linearStatusLoading = false;
  paintLinearIntegration();
  if (taskSource === "linear" && !taskView.hidden) paintLinearPanel();
}

function paintLinearPanel() {
  paintTaskContext();
  if (!linearStatus.connection) {
    showLinearOnly("linear-empty");
    return;
  }
  if (!linearStatus.connected) {
    showLinearOnly(
      "linear-error",
      linearStatusError || t("task.linear.credentialUnavailable", "Linear API 키를 다시 연결하세요."),
    );
    return;
  }
  void loadLinearIssues();
}

function linearSearchText() {
  return el("linear-search").value.trim();
}

function paintLinearTools() {
  const searching = linearSearchText() !== "";
  for (const chip of el("linear-presets").querySelectorAll("[data-linear-preset]")) {
    const active = !searching && chip.dataset.linearPreset === linearPreset;
    chip.classList.toggle("is-active", active);
    chip.setAttribute("aria-pressed", String(active));
  }
  el("linear-search-clear").hidden = !searching;
}

function scheduleLinearSearch() {
  clearTimeout(linearSearchTimer);
  linearSearchTimer = setTimeout(() => void loadLinearIssues(), 300);
}

async function loadLinearIssues() {
  if (!linearStatus.connected) return;
  const generation = ++linearIssuesGeneration;
  paintLinearTools();
  showLinearOnly("linear-loading");
  try {
    const query = linearSearchText();
    const answer = query
      ? await invoke("linear_search_issues", { query })
      : await invoke("linear_issues", {
          preset: linearPreset,
          teamId: linearStatus.connection?.active_team_id ?? null,
        });
    if (generation !== linearIssuesGeneration) return;
    linearIssues = Array.isArray(answer) ? answer : [];
    paintLinearIssues();
  } catch (error) {
    if (generation !== linearIssuesGeneration) return;
    const failure = failureOf(error);
    if (["offline", "rate_limited"].includes(failure.kind)) {
      showLinearOnly("linear-quiet", failure.message || String(error));
    } else {
      showLinearOnly("linear-error", failure.message || String(error));
    }
  }
}

function paintLinearIssues() {
  paintLinearTools();
  if (linearIssues.length === 0) {
    say(el("linear-none-body"), () => linearSearchText()
      ? t("task.linear.emptySearchHint", "검색어를 바꾸거나 지워 보세요.")
      : t("task.linear.emptyPresetHint", "선택한 조건에 맞는 이슈가 없습니다."));
    showLinearOnly("linear-none");
    return;
  }
  const list = el("linear-list");
  list.replaceChildren();
  for (const [status, rows] of groupJiraIssues(linearIssues)) {
    list.appendChild(linearGroupNode(status, rows));
  }
  showLinearOnly("linear-list");
}

function linearGroupNode(status, rows) {
  const group = document.createElement("section");
  group.className = "task-jira-group";
  const head = document.createElement("button");
  head.type = "button";
  head.className = "task-jira-group-head";
  const folded = linearFolded.has(status);
  head.setAttribute("aria-expanded", String(!folded));
  const title = document.createElement("strong");
  title.textContent = status;
  const count = document.createElement("span");
  count.textContent = String(rows.length);
  head.append(title, count);
  const body = document.createElement("div");
  body.className = "task-jira-group-body";
  body.hidden = folded;
  for (const issue of rows) body.appendChild(linearRowNode(issue));
  head.addEventListener("click", () => {
    if (linearFolded.has(status)) linearFolded.delete(status);
    else linearFolded.add(status);
    const closed = linearFolded.has(status);
    body.hidden = closed;
    head.setAttribute("aria-expanded", String(!closed));
  });
  group.append(head, body);
  return group;
}

function linearRowNode(issue) {
  const row = document.createElement("div");
  row.className = "task-jira-row";
  const key = document.createElement("span");
  key.className = "task-jira-key";
  key.textContent = issue.key;
  const title = document.createElement("span");
  title.className = "task-jira-title";
  title.textContent = issue.title;
  const status = document.createElement("span");
  status.className = `task-jira-status ${JIRA_STATUS_TONE[issue.category] ?? ""}`.trim();
  status.textContent = issue.status;
  const priority = document.createElement("span");
  priority.className = "task-jira-priority";
  priority.textContent = issue.priority ?? "—";
  const assignee = document.createElement("span");
  assignee.className = "task-jira-assignee";
  assignee.textContent = issue.assignee ?? t("task.jira.unassigned", "미할당");
  const updated = document.createElement("span");
  updated.className = "task-jira-updated";
  const updatedAt = Date.parse(issue.updated ?? "");
  updated.textContent = Number.isNaN(updatedAt) ? "" : agoWord(updatedAt, Date.now());
  const act = document.createElement("button");
  act.type = "button";
  act.className = "btn task-jira-act";
  act.dataset.key = issue.key;
  say(act, () => t("task.linear.startWork", "작업 트리"));
  act.addEventListener("click", () => startWorktreeFromLinearItem(issue));
  row.append(key, title, status, priority, assignee, updated, act);
  actsAsButton(row, (event) => {
    if (event.target.closest(".task-jira-act")) return;
    openLinearItem(issue);
  });
  return row;
}

const linearItemScrim = el("linear-item-scrim");
let linearItem = null;
let linearItemAsking = null;
let linearItemBusy = false;

function openLinearItem(issue) {
  linearItem = issue;
  el("linear-item-key").textContent = issue.key;
  el("linear-item-title").textContent = issue.title;
  el("linear-item-state").textContent = issue.status;
  el("linear-item-meta").textContent = [issue.team_name, issue.assignee].filter(Boolean).join(" · ");
  el("linear-item-body").replaceChildren();
  el("linear-item-comments").replaceChildren();
  el("linear-item-error").hidden = true;
  showModal(linearItemScrim);
  void loadLinearItem();
}

async function loadLinearItem() {
  const item = linearItem;
  if (!item) return;
  const asking = Symbol("linear-item");
  linearItemAsking = asking;
  el("linear-item-loading").hidden = false;
  try {
    const [detail, comments] = await Promise.all([
      invoke("linear_issue_detail", { id: item.id }),
      invoke("linear_issue_comments", { id: item.id }),
    ]);
    if (linearItemAsking !== asking || linearItem !== item) return;
    linearItem = { ...item, ...detail, head: undefined };
    const head = detail?.head ?? detail;
    Object.assign(linearItem, head);
    paintLinearItem(detail, Array.isArray(comments) ? comments : []);
  } catch (error) {
    if (linearItemAsking !== asking) return;
    say(el("linear-item-error"), () => failureOf(error).message || String(error));
    el("linear-item-error").hidden = false;
  } finally {
    if (linearItemAsking === asking) {
      linearItemAsking = null;
      el("linear-item-loading").hidden = true;
    }
  }
}

function paintLinearItem(detail, comments) {
  const head = detail?.head ?? detail ?? {};
  el("linear-item-key").textContent = head.key ?? linearItem?.key ?? "";
  el("linear-item-title").textContent = head.title ?? linearItem?.title ?? "";
  el("linear-item-state").textContent = head.status ?? linearItem?.status ?? "";
  el("linear-item-meta").textContent = [
    head.team_name,
    head.assignee,
    detail?.created ? jiraDayWord(detail.created) : "",
  ].filter(Boolean).join(" · ");
  const labels = Array.isArray(detail?.labels) ? detail.labels : [];
  el("linear-item-labels").hidden = labels.length === 0;
  el("linear-item-labels").textContent = labels.join(" · ");
  const description = document.createElement("p");
  description.style.whiteSpace = "pre-wrap";
  description.textContent = detail?.description || t("task.linear.noDescription", "설명이 없습니다.");
  el("linear-item-body").replaceChildren(description);
  const attachments = Array.isArray(detail?.attachments) ? detail.attachments : [];
  el("linear-item-attachments-head").hidden = attachments.length === 0;
  el("linear-item-attachments").replaceChildren(...attachments.map((attachment) => {
    const link = document.createElement("button");
    link.type = "button";
    link.className = "btn btn--ghost";
    link.textContent = attachment.title;
    link.addEventListener("click", () => openExternal(attachment.url));
    return link;
  }));
  el("linear-item-comments").replaceChildren(...comments.map((comment) => {
    const row = document.createElement("article");
    row.className = "detail-card-comment";
    const by = document.createElement("strong");
    by.textContent = comment.author;
    const body = document.createElement("p");
    body.style.whiteSpace = "pre-wrap";
    body.textContent = comment.body;
    row.append(by, body);
    return row;
  }));
}

function closeLinearItem() {
  if (linearItemBusy) return;
  hideModal(linearItemScrim, { animated: true });
  linearItem = null;
  linearItemAsking = null;
}

el("linear-item-shut").addEventListener("click", closeLinearItem);
linearItemScrim.addEventListener("mousedown", (event) => {
  if (event.target === linearItemScrim) closeLinearItem();
});
el("linear-item-open-web").addEventListener("click", () => {
  if (linearItem?.url) openExternal(linearItem.url);
});
el("linear-item-start").addEventListener("click", () => {
  if (linearItem) startWorktreeFromLinearItem(linearItem);
});
el("linear-item-comment-send").addEventListener("click", async () => {
  const item = linearItem;
  const body = el("linear-item-comment-field").value.trim();
  if (!item || !body || linearItemBusy) return;
  linearItemBusy = true;
  el("linear-item-comment-send").disabled = true;
  try {
    await invoke("linear_comment_issue", { id: item.id, body });
    el("linear-item-comment-field").value = "";
    await loadLinearItem();
  } catch (error) {
    say(el("linear-item-error"), () => failureOf(error).message || String(error));
    el("linear-item-error").hidden = false;
  } finally {
    linearItemBusy = false;
    el("linear-item-comment-send").disabled = false;
  }
});

function startWorktreeFromLinearItem(issue) {
  setTaskOpen(false);
  setWorktreeForm(true);
  setWorktreeTab("smart");
  wtSpec.value = issue.url;
  worktreeLinearDraft = {
    id: issue.id,
    key: issue.key,
    url: issue.url,
    title: issue.title ?? issue.key,
    status: issue.status ?? "",
  };
  worktreeLinearContext = null;
  worktreeLinearError = null;
  fitWorktreeSpec();
  paintWorktreeName();
  paintWorktreeLinear();
  void readWorktreeSeed();
  void loadWorktreeLinearContext();
}

const linearConnectScrim = el("linear-connect-scrim");

function openLinearConnectDialog() {
  linearConnecting = false;
  el("linear-api-key").value = "";
  el("linear-connect-error").hidden = true;
  el("linear-connect-submit").disabled = true;
  showModal(linearConnectScrim);
}

function closeLinearConnectDialog() {
  if (linearConnecting) return;
  hideModal(linearConnectScrim);
  el("linear-api-key").value = "";
}

for (const id of ["linear-connect-open", "integration-linear-add"]) {
  el(id).addEventListener("click", openLinearConnectDialog);
}
el("linear-connect-cancel").addEventListener("click", closeLinearConnectDialog);
linearConnectScrim.addEventListener("mousedown", (event) => {
  if (event.target === linearConnectScrim) closeLinearConnectDialog();
});
el("linear-api-key").addEventListener("input", () => {
  el("linear-connect-error").hidden = true;
  el("linear-connect-submit").disabled = linearConnecting || !el("linear-api-key").value.trim();
});
el("linear-connect-submit").addEventListener("click", async () => {
  const apiKey = el("linear-api-key").value.trim();
  if (!apiKey || linearConnecting) return;
  const generation = ++linearMutationGeneration;
  linearConnecting = true;
  el("linear-connect-submit").disabled = true;
  el("linear-connect-cancel").disabled = true;
  try {
    const answer = await invoke("linear_connect", { apiKey });
    if (generation !== linearMutationGeneration) return;
    linearStatus = normalizeLinearStatus(answer);
    linearConnecting = false;
    el("linear-connect-cancel").disabled = false;
    closeLinearConnectDialog();
    paintLinearIntegration();
    if (taskSource === "linear" && !taskView.hidden) paintLinearPanel();
  } catch (error) {
    if (generation !== linearMutationGeneration) return;
    linearConnecting = false;
    el("linear-connect-cancel").disabled = false;
    el("linear-connect-submit").disabled = false;
    say(el("linear-connect-error"), () => failureOf(error).message || String(error));
    el("linear-connect-error").hidden = false;
  }
});

el("integration-linear-refresh").addEventListener("click", () => void refreshLinearStatus());
el("integration-linear-disconnect").addEventListener("click", async () => {
  const generation = ++linearMutationGeneration;
  try {
    const answer = await invoke("linear_disconnect");
    if (generation !== linearMutationGeneration) return;
    linearStatus = normalizeLinearStatus(answer);
    paintLinearIntegration();
    if (taskSource === "linear" && !taskView.hidden) paintLinearPanel();
  } catch (error) {
    if (generation !== linearMutationGeneration) return;
    await refreshLinearStatus(String(error));
  }
});

el("linear-presets").addEventListener("click", (event) => {
  const chip = event.target.closest("[data-linear-preset]");
  if (!chip) return;
  linearPreset = chip.dataset.linearPreset;
  el("linear-search").value = "";
  void loadLinearIssues();
});
el("linear-search").addEventListener("input", () => {
  paintLinearTools();
  scheduleLinearSearch();
});
el("linear-search").addEventListener("keydown", (event) => {
  if (event.isComposing || event.key === "Process" || event.keyCode === 229) return;
  if (event.key !== "Enter") return;
  event.preventDefault();
  clearTimeout(linearSearchTimer);
  void loadLinearIssues();
});
el("linear-search-clear").addEventListener("click", () => {
  el("linear-search").value = "";
  el("linear-search").focus();
  void loadLinearIssues();
});
el("linear-hide").addEventListener("click", () => setTaskSourceHidden("linear", true));

function closeLinearCreate() {
  if (!linearCreateBusy) hideModal(el("linear-create-scrim"));
}

function openLinearCreate() {
  const teams = linearStatus.connection?.teams ?? [];
  if (teams.length === 0) return;
  el("linear-create-team").replaceChildren(...teams.map((team) => {
    const option = document.createElement("option");
    option.value = team.id;
    option.textContent = `${team.key} — ${team.name}`;
    return option;
  }));
  el("linear-create-team").value = linearStatus.connection?.active_team_id ?? teams[0].id;
  el("linear-create-title-field").value = "";
  el("linear-create-description").value = "";
  el("linear-create-error").hidden = true;
  showModal(el("linear-create-scrim"));
}

el("linear-new").addEventListener("click", openLinearCreate);
el("linear-create-cancel").addEventListener("click", closeLinearCreate);
el("linear-create-scrim").addEventListener("mousedown", (event) => {
  if (event.target === el("linear-create-scrim")) closeLinearCreate();
});
el("linear-create-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  if (linearCreateBusy) return;
  const request = {
    team: el("linear-create-team").value,
    title: el("linear-create-title-field").value.trim(),
    desc: el("linear-create-description").value.trim() || null,
  };
  if (!request.team || !request.title) return;
  linearCreateBusy = true;
  el("linear-create-submit").disabled = true;
  try {
    const created = await invoke("linear_create_issue", request);
    linearCreateBusy = false;
    closeLinearCreate();
    void loadLinearIssues();
    if (created?.id && created?.url) openLinearItem({
      ...created,
      status: "",
      category: "todo",
      team_name: "",
    });
  } catch (error) {
    say(el("linear-create-error"), () => failureOf(error).message || String(error));
    el("linear-create-error").hidden = false;
  } finally {
    linearCreateBusy = false;
    el("linear-create-submit").disabled = false;
  }
});
