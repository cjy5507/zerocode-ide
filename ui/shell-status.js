/* What is running, as its own status field. Orca's bar reads as counts rather
 * than as one sentence, and these are the two this product has to count.
 * Painted with the tabs because that is drawn on every boot — `refreshChrome`
 * waits for a lane event, which an idle window never gets. */
/* ---- what is listening ----
 *
 * Orca's right column has a `ports` activity, and its icon is `sshOnly`
 * (App-BaqTRjaA.js:21510-21566) — so on a local repository the icon is not
 * there and the status bar segment IS the way in. That is what this is: the
 * segment and the popover it raises, not a fifth icon in the rail we would
 * have added by reading the activity list and stopping there.
 *
 * The scan is system-wide and that is the point. Orca runs `lsof` over every
 * listening socket and then attributes what it finds, rather than tracking
 * the processes it started — so a dev server somebody launched in a real
 * terminal an hour ago is in the list. Ports it can tie to a checkout are
 * that workspace's; the rest are `external` and get their own section. */
const STATUS_BAR_ITEM_CATALOG = [
  { id: "claude" },
  { id: "codex" },
  { id: "antigravity" },
  { id: "kimi" },
  { id: "grok" },
  { id: "opencode-go" },
  { id: "resource-usage" },
  { id: "ports" },
  /* 「새 빌드 준비됨」의 세그먼트 자리 (t-3005, docs/design/release-lane-off-
   * the-window.md §2.4). 카탈로그 한 몸에 서되 아직 몸(세그먼트)이 없으므로
   * 내놓지 않는다 — 기본 꺼짐이고, 설정에 체크박스도 없고, 백엔드의
   * `STATUS_BAR_ITEMS` 문도 이 이름을 모른다. B8 이 세그먼트를 그릴 때
   * `offered` 를 지우고 그 셋을 함께 연다. */
  { id: "update", offered: false },
];
const USAGE_PERCENTAGE_DISPLAYS = ["used", "remaining"];
const STATUS_BAR_USAGE_MODES = ["verbose", "compact"];

/* The rows a person can switch: every catalog row with a segment behind it. */
function offeredStatusBarItems() {
  return STATUS_BAR_ITEM_CATALOG.filter((item) => item.offered !== false);
}

let statusBarItems = new Set(offeredStatusBarItems().map((item) => item.id));
let usagePercentageDisplay = "used";
let statusBarUsageMode = "verbose";

function normalizedStatusBarItems(items) {
  const asked = new Set(Array.isArray(items) ? items : []);
  return new Set(
    offeredStatusBarItems()
      .map((item) => item.id)
      .filter((item) => asked.has(item)),
  );
}

function statusBarItemEnabled(item) {
  return statusBarItems.has(item);
}

function normalizedUsagePercentageDisplay(display) {
  return USAGE_PERCENTAGE_DISPLAYS.includes(display) ? display : "used";
}

function normalizedStatusBarUsageMode(mode) {
  return STATUS_BAR_USAGE_MODES.includes(mode) ? mode : "verbose";
}

function displayedUsagePercentage(usedPercent) {
  if (!Number.isFinite(usedPercent)) return 0;
  const roundedUsed = Math.round(Math.min(100, Math.max(0, usedPercent)));
  return usagePercentageDisplay === "used" ? roundedUsed : 100 - roundedUsed;
}

/* 「N% 사용」 또는 「N% 남음」 — 설정이 고른 그 낱말로. 패널의 행과 바의
 * 세그먼트가 같은 문장을 말해야 하므로 문장은 하나다. */
function spokenUsagePercentage(usedPercent) {
  const pct = displayedUsagePercentage(usedPercent);
  return usagePercentageDisplay === "used"
    ? t("usage.used", "{{pct}}% 사용", { pct })
    : t("usage.left", "{{pct}}% 남음", { pct });
}

function paintUsagePercentageChoice() {
  for (const display of USAGE_PERCENTAGE_DISPLAYS) {
    const button = el(`usage-percentage-${display}`);
    const active = display === usagePercentageDisplay;
    button.classList.toggle("is-active", active);
    button.setAttribute("aria-pressed", String(active));
  }
}

function paintStatusBarUsageModeChoice() {
  for (const mode of STATUS_BAR_USAGE_MODES) {
    const button = el(`usage-mode-${mode}`);
    const active = mode === statusBarUsageMode;
    button.classList.toggle("is-active", active);
    button.setAttribute("aria-pressed", String(active));
  }
}

function repaintUsageConsumers({ stats = false } = {}) {
  paintUsageSegments();
  paintUsagePanel();
  if (stats) paintStatsUsage();
}

function paintUsagePercentagePreference() {
  paintUsagePercentageChoice();
  repaintUsageConsumers({ stats: true });
}

function paintStatusBarUsageModePreference() {
  paintStatusBarUsageModeChoice();
  repaintUsageConsumers();
}

/* ---- Caffeinate 세그먼트 (P0-17 잔여) ----
 *
 * Orca `CaffeinateStatusSegment.tsx` 실측 그대로: 커피 글리프, 모드의
 * 낱말(On·Agent·Off), 켜져 있는 동안 차오르는 점. 서비스가 말한 상태와
 * 설정된 모드가 어긋나면 설정이 이긴다(:83-85) — 방금 바꾼 사람이 보는
 * 것은 자기 선택이어야 하므로. 팝오버의 세 줄은 General 판의 셀렉트와
 * 같은 문(`setComputerAwakeMode`) 하나로 쓴다.
 *
 * 이 세그먼트는 statusBarItems 카탈로그의 항목이 아니다: Orca는 이것만
 * 게이트 없이 상시로 그리고(`StatusBar.tsx:2393`), 설정의 체크박스
 * 목록(:2456-2571)에도 넣지 않는다. 끄는 문은 General 판의 모드
 * 선택(끔)뿐이다. */
let awakeStatus = { mode: "off", active: false };

function caffeinateModeWord(mode) {
  if (mode === "on") return t("caffeinate.on", "켬");
  if (mode === "auto") return t("caffeinate.auto", "에이전트");
  return t("caffeinate.off", "끔");
}

function reconciledAwake() {
  return awakeStatus.mode === computerAwakeMode
    ? awakeStatus
    : { mode: computerAwakeMode, active: computerAwakeMode === "on" };
}

function paintCaffeinateSegment() {
  const seat = el("sb-caffeinate");
  const { mode, active } = reconciledAwake();
  el("sb-caffeinate-mode").textContent = caffeinateModeWord(mode);
  el("sb-caffeinate-dot").classList.toggle("is-active", active);
  const spoken = `${caffeinateModeWord(mode)} · ${
    active ? t("caffeinate.active", "작동 중") : t("caffeinate.inactive", "쉬는 중")
  }`;
  el("caffeinate-standing").textContent = spoken;
  seat.dataset.tip = `Caffeinate — ${spoken}`;
  seat.setAttribute("aria-label", `Caffeinate, ${spoken}`);
  for (const option of el("caffeinate-options").querySelectorAll(".caffeinate-option")) {
    option.classList.toggle("is-active", option.dataset.awakeMode === mode);
  }
}

async function refreshAwakeStatus() {
  let said;
  try {
    said = await invoke("computer_awake_status", {});
  } catch {
    return;
  }
  if (said && typeof said.mode === "string") {
    awakeStatus = { mode: said.mode, active: said.active === true };
    paintCaffeinateSegment();
  }
}

function setCaffeinateOpen(on) {
  const pop = el("caffeinate-pop");
  if (on) showing(pop);
  else closing(pop);
  el("sb-caffeinate").setAttribute("aria-expanded", on ? "true" : "false");
  if (!on) return;
  // Somebody is about to READ the standing — the one moment a fresh ask is
  // worth its round trip.
  void refreshAwakeStatus();
  const seat = el("sb-caffeinate");
  const at = seat.getBoundingClientRect();
  pop.style.left = `${Math.max(8, at.right - pop.offsetWidth)}px`;
}

function paintStatusBarPreferences() {
  for (const item of offeredStatusBarItems()) {
    el(`status-bar-${item.id}`).checked = statusBarItemEnabled(item.id);
  }
  paintUsagePercentageChoice();
  paintStatusBarUsageModeChoice();
  repaintUsageConsumers({ stats: true });
  paintPorts();
  const resourceEnabled = statusBarItemEnabled("resource-usage");
  el("sb-resource").hidden = !resourceEnabled;
  if (!resourceEnabled && !el("resource-pop").hidden) setResourceOpen(false);
  if (!statusBarItemEnabled("ports") && !el("ports-pop").hidden) setPortsOpen(false);
  paintCaffeinateSegment();
  /* 판은 이제 공급자 하나가 아니라 모두를 보이므로, 조각 하나가 꺼졌다고 닫지
   * 않는다 — 닫는 것은 **매달릴 곳이 없어졌을 때**다. 팝오버는 막대의 조각에
   * 붙어 서므로 사용량 조각이 하나도 없으면 허공에 떠 있게 된다. */
  if (!el("usage-pop").hidden &&
      !USAGE_PROVIDERS.some((provider) => statusBarItemEnabled(provider.id))) {
    setUsageOpen(false);
  }
  usageAmbient.sync();
}

function setStatusBarItem(item, enabled) {
  const next = new Set(statusBarItems);
  if (enabled) next.add(item);
  else next.delete(item);
  statusBarItems = normalizedStatusBarItems([...next]);
  paintStatusBarPreferences();
  void commitSetting(`status_bar_items.${item}`, "set_status_bar_item", { item, enabled });
}

function setUsagePercentageDisplay(display) {
  const normalized = normalizedUsagePercentageDisplay(display);
  if (normalized === usagePercentageDisplay) return;
  usagePercentageDisplay = normalized;
  paintUsagePercentagePreference();
  void commitSetting("usage_percentage_display", "set_usage_percentage_display", {
    display: normalized,
  });
}

function setStatusBarUsageMode(mode) {
  const normalized = normalizedStatusBarUsageMode(mode);
  if (normalized === statusBarUsageMode) return;
  statusBarUsageMode = normalized;
  paintStatusBarUsageModePreference();
  void commitSetting("status_bar_usage_mode", "set_status_bar_usage_mode", {
    mode: normalized,
  });
}

for (const item of offeredStatusBarItems()) {
  el(`status-bar-${item.id}`).addEventListener("change", (event) => {
    setStatusBarItem(item.id, event.currentTarget.checked);
  });
}

/* Caffeinate 배선 — 정의부는 paintStatusBarPreferences 앞에 산다:
 * 부팅의 설정 absorb가 그 안에서 paintCaffeinateSegment를 부르므로,
 * 상태(let)가 뒤에 있으면 TDZ가 absorb를 중단한다(2026-08-18 실사고 —
 * terminal_prefs까지 우리를 못 읽어 휠 핸들러가 null을 밟았다). */
el("sb-caffeinate").addEventListener("click", () => {
  setCaffeinateOpen(el("caffeinate-pop").hidden);
});
for (const option of el("caffeinate-options").querySelectorAll(".caffeinate-option")) {
  option.addEventListener("click", () => {
    setComputerAwakeMode(option.dataset.awakeMode);
    setCaffeinateOpen(false);
  });
}

listen("computer-awake:changed", (event) => {
  const said = event.payload;
  if (!said || typeof said.mode !== "string") return;
  awakeStatus = { mode: said.mode, active: said.active === true };
  paintCaffeinateSegment();
});
void refreshAwakeStatus();

/* 잠에서 돌아온 기계 (P0-17 잔여) — Orca system-resume-broadcast.ts의
 * `system:resumed` 그대로. 자던 동안의 SSH 링크는 서 있다고 믿을 수 없다:
 * Orca의 resume 핸들러가 활성 대상을 전부 다시 거는 것처럼(ipc/ssh.ts
 * onResume), 연결로 기억된 링크를 다시 걸고 곁줄을 새로 긋는다. */
listen("system:resumed", () => {
  for (const report of sshLinkStates.values()) {
    if (report?.state === "connected") {
      void askSshLink("ssh_link_connect", { id: report.id });
    }
  }
  void refreshSidebarSshHosts();
  void refreshAwakeStatus();
});

/* 벨을 누른 손의 도착 (P0-14) — Orca `ui:activateWorktree` +
 * `ui:focusTerminal`(flashFocusedPane, native-notification-delivery.ts:90-104)
 * 의 이 창 등가. 백엔드는 "방금 벨이 울린 복귀"만 이 이벤트로 보낸다
 * (RunEvent::Reopen + RING_CLICK_WINDOW). 워크트리를 갈아타고, 그 판이
 * 이 창에 살아 있으면 탭을 앞세워 초점을 주고 한 번 번쩍인다 — 어디로
 * 불려 왔는지 눈이 알도록. 레인 벨은 판 번호가 없어 워크트리까지만 간다. */
listen("notify:activate", async (event) => {
  const said = event.payload;
  if (typeof said?.worktree !== "string") return;
  if (said.worktree !== activeWorktreePath && !(await activateWorktree(said.worktree))) {
    return;
  }
  if (typeof said.term !== "number") return;
  const tab = tabs.find((one) => one.kind === "term" && one.layout !== null
    && paneLeaves(one.layout).includes(said.term));
  if (!tab) return;
  setActiveTab(tab.id);
  setActivePane(tab, said.term);
  flashPane(said.term);
});

/* 한 번의 번쩍임 — 클래스 하나가 keyframes를 데려오고, 끝나면 스스로
 * 벗는다: 두 번째 벨이 같은 판을 다시 부를 수 있어야 하므로. */
function flashPane(term) {
  const slot = document.querySelector(`.pane-slot[data-term="${term}"]`);
  if (!slot) return;
  slot.classList.remove("is-called");
  // 강제 리플로우로 애니메이션을 처음부터 — 같은 클래스의 재부착은
  // 리플로우 없이는 아무 일도 아니다.
  void slot.offsetWidth;
  slot.classList.add("is-called");
  slot.addEventListener("animationend", () => slot.classList.remove("is-called"), {
    once: true,
  });
}

/* OpenCode Go's two fields. The cookie is a secret and goes to the keychain
 * through its own command; the workspace override is an ordinary setting.
 * Both commit on blur and on Enter rather than per keystroke — a keychain
 * write per typed character is a keychain prompt per typed character. */
let opencodeCookieConfigured = false;

function commitOpencodeCookie() {
  const field = el("opencode-cookie");
  const pasted = field.value.trim();
  // An empty box is not "clear it": the box is empty whenever a cookie is
  // already held, because a stored secret is never read back into the window.
  if (pasted === "") return;
  field.value = "";
  void commitSetting("opencode_cookie_configured", "set_opencode_cookie", { cookie: pasted })
    .then(() => refreshProviderUsage(usageProvider("opencode-go"), true));
}

function clearOpencodeCookie() {
  el("opencode-cookie").value = "";
  void commitSetting("opencode_cookie_configured", "set_opencode_cookie", { cookie: "" });
}

for (const [id, run] of [
  ["opencode-cookie", commitOpencodeCookie],
  ["opencode-workspace", () => {
    void commitSetting("opencode_workspace", "set_opencode_workspace", {
      workspace: el("opencode-workspace").value.trim(),
    });
  }],
]) {
  el(id).addEventListener("blur", run);
  el(id).addEventListener("keydown", (event) => {
    if (event.key !== "Enter") return;
    event.preventDefault();
    run();
    // Escape on the cookie box takes the stored value back — the one gesture
    // that cannot be spelled by typing, since an empty box means "unchanged".
  });
  el(id).addEventListener("keydown", (event) => {
    if (id === "opencode-cookie" && event.key === "Escape" && event.shiftKey) {
      event.preventDefault();
      clearOpencodeCookie();
    }
  });
}

for (const display of USAGE_PERCENTAGE_DISPLAYS) {
  el(`usage-percentage-${display}`).addEventListener("click", () => {
    setUsagePercentageDisplay(display);
  });
}

for (const mode of STATUS_BAR_USAGE_MODES) {
  el(`usage-mode-${mode}`).addEventListener("click", () => {
    setStatusBarUsageMode(mode);
  });
}

let portScan = null;

/* A dev server the terminal announced, waiting for the OS to agree.
 *
 * A URL alone is not enough: agents quote documentation, tests print fake
 * addresses, and a process can say "ready" before bind succeeds. A listening
 * socket alone is not enough either: databases and somebody else's project
 * listen too. Automatic opening is the intersection — a local URL printed by
 * a terminal in the active worktree and a real listener whose process cwd is
 * that exact worktree. */
const AUTO_DEV_SERVER_SCAN_LIMIT = 3;
const AUTO_DEV_SERVER_SCAN_DELAY = 350;
const autoDevServerCandidates = new Map();
const autoDevServersOpened = new Set();
let autoDevServerScanTimer = null;

function localDevServerCandidate(raw) {
  const canonical = canonicalHttpLink(raw);
  if (canonical === null) return null;
  const url = new URL(canonical);
  if (url.username || url.password) return null;
  const host = url.hostname.toLowerCase();
  const loopback =
    host === "localhost" ||
    host.endsWith(".localhost") ||
    host === "127.0.0.1" ||
    host === "[::1]";
  const wildcard = host === "0.0.0.0" || host === "[::]";
  if (!loopback && !wildcard) return null;
  if (wildcard) url.hostname = "localhost";
  return {
    url: url.href,
    origin: url.origin,
    port: Number(url.port || (url.protocol === "https:" ? 443 : 80)),
  };
}

function terminalAddressWorktree(address) {
  if (address?.kind === "lane") {
    return lanes.get(address.id)?.lane.worktree_id ?? null;
  }
  if (address?.kind !== "term") return null;
  return tabs.find(
    (tab) =>
      tab.kind === "term" &&
      tab.layout !== null &&
      paneLeaves(tab.layout).includes(address.term),
  )?.worktree ?? null;
}

function autoDevServerKey(worktree, origin) {
  return `${worktree}\0${origin}`;
}

function browserTabHasServer(worktree, origin) {
  return tabs.some((tab) => {
    if (tab.kind !== "browser" || tab.worktree !== worktree) return false;
    const canonical = canonicalHttpLink(tab.url);
    return canonical !== null && new URL(canonical).origin === origin;
  });
}

function scheduleAutoDevServerScan() {
  if (autoDevServerScanTimer !== null || autoDevServerCandidates.size === 0) return;
  autoDevServerScanTimer = window.setTimeout(() => {
    autoDevServerScanTimer = null;
    if (autoDevServerCandidates.size > 0) void refreshPorts();
  }, AUTO_DEV_SERVER_SCAN_DELAY);
}

function notePotentialDevServerLink(raw, address) {
  const worktree = terminalAddressWorktree(address);
  if (!worktree || worktree !== activeWorktreePath) return;
  const candidate = localDevServerCandidate(raw);
  if (candidate === null) return;
  const key = autoDevServerKey(worktree, candidate.origin);
  if (autoDevServersOpened.has(key) || browserTabHasServer(worktree, candidate.origin)) {
    autoDevServersOpened.add(key);
    return;
  }
  if (!autoDevServerCandidates.has(key)) {
    autoDevServerCandidates.set(key, { ...candidate, worktree, scans: 0 });
  }
  scheduleAutoDevServerScan();
}

function confirmAutoDevServers() {
  const listeners = portScan?.ports ?? [];
  for (const [key, candidate] of autoDevServerCandidates) {
    candidate.scans += 1;
    const listening = listeners.some(
      (port) => port.port === candidate.port && port.owner_path === candidate.worktree,
    );
    if (!listening) {
      if (candidate.scans >= AUTO_DEV_SERVER_SCAN_LIMIT) autoDevServerCandidates.delete(key);
      continue;
    }
    autoDevServerCandidates.delete(key);
    // The scan is asynchronous. A project switch during it must not open a
    // page in the project that replaced the one which printed the URL.
    if (candidate.worktree !== activeWorktreePath) continue;
    if (autoDevServersOpened.has(key) || browserTabHasServer(candidate.worktree, candidate.origin)) {
      autoDevServersOpened.add(key);
      continue;
    }
    // Reserve before the native pane is created: two scans can finish in the
    // same frame, and both must not mint a tab for the same server.
    autoDevServersOpened.add(key);
    void openBrowserTab(candidate.url, {
      beside: true,
      expectedWorktree: candidate.worktree,
    }).then((opened) => {
      if (opened !== true) autoDevServersOpened.delete(key);
    });
  }
  scheduleAutoDevServerScan();
}

async function refreshPorts() {
  try {
    portScan = await invoke("listening_ports");
  } catch {
    // A scan that could not run says so in the popover rather than here.
    portScan = null;
  }
  paintPorts();
  confirmAutoDevServers();
}

function ownedPorts() {
  return (portScan?.ports ?? []).filter((port) => port.owner !== null);
}

function externalPorts() {
  return (portScan?.ports ?? []).filter((port) => port.owner === null);
}

/* The number on the bar is THIS project's ports, not the machine's.
 *
 * Orca's segment wears `workspacePortCount` and keeps the external tally for
 * the tooltip and the popover's own header. A bar that counted every socket
 * on a developer's machine would read as noise — the number is meant to
 * answer "is my thing up", and forty of somebody else's daemons do not. */
function paintPorts() {
  const owned = ownedPorts().length;
  const outside = externalPorts().length;
  const segment = el("sb-ports");
  // Nothing running and nothing to say: no scan, or a scan that found
  // nothing at all. A zero on the bar is a fact nobody asked for.
  segment.hidden =
    !statusBarItemEnabled("ports") || portScan === null || (owned === 0 && outside === 0);
  el("sb-ports-count").textContent = String(owned);
  segment.dataset.tip = t("ports.tally", "포트 — 이 프로젝트 {{owned}}개 · 그 밖 {{outside}}개", {
    owned,
    outside,
  });
  segment.setAttribute("aria-label", segment.dataset.tip);
  el("ports-tally").textContent = t("ports.counts", "{{owned}} 워크스페이스 · {{outside}} 외부", {
    owned,
    outside,
  });
  paintWorktreePortPins();
  if (!el("ports-pop").hidden) paintPortRows();
}

/* The plug on each sidebar row, standing only while that checkout OWNS
 * listening ports — Orca's `WorktreeCardPortsTrigger` (WorktreeCard-
 * B91w6t_B.js): `if (ports.length === 0) return null`, a 14px Plug at 70%
 * ink, aria "{{n}} live port(s)" (fed49903c9 — the unit words are English
 * in every locale, Orca's own quirk). Painted in place on every scan: the
 * rows outlive a scan and a rebuilt sidebar would cost more than the fact
 * changed. */
function paintWorktreePortPins() {
  for (const pin of document.querySelectorAll(".wt-row .wt-ports")) {
    const path = pin.closest(".wt-row")?.dataset.worktreePath ?? "";
    const count = ownedPorts().filter((port) => port.owner_path === path).length;
    pin.hidden = count === 0;
    if (count === 0) continue;
    const said = t("ports.livePin", "{{count}} 실시간 {{unit}}", {
      count,
      unit: count === 1 ? "port" : "ports",
    });
    pin.dataset.tip = said;
    pin.setAttribute("aria-label", said);
  }
}

function portRow(port) {
  const row = document.createElement("div");
  row.className = "port-row";
  const address = `${port.connect_host}:${port.port}`;
  row.innerHTML =
    '<span class="port-num"></span><span class="port-proc"></span>' +
    '<span class="port-addr"></span>' +
    '<button class="port-copy" type="button"></button>';
  // Orca's row leads with the port, colon-prefixed, then names the process
  // beside it and puts the address a person would open on the line below.
  row.querySelector(".port-num").textContent = `:${port.port}`;
  row.querySelector(".port-proc").textContent =
    port.process || t("ports.unknownProcess", "PID {{pid}}", { pid: port.pid });
  row.querySelector(".port-addr").textContent = address;
  // 행 전체가 문이다(Orca WorktreeCardPorts.handleOpen) — 어느 브라우저로
  // 갈지는 링크 라우팅 정책 그대로: 기본은 설정이, 수식키가 뒤집는다.
  actsAsButton(row, (event) => routeHttpLink(`http://${address}`, event));
  const copy = row.querySelector(".port-copy");
  copy.textContent = t("ports.copy", "주소 복사");
  copy.addEventListener("click", (event) => {
    event.stopPropagation();
    void clipboardText.write(address);
  });
  // 내 워크스페이스의 프로세스만 멈출 수 있다(Orca canStopWorkspacePort) —
  // 남의 리스너에 이 손을 대는 것은 이 창의 일이 아니다.
  if (port.owner_path) {
    const stop = document.createElement("button");
    stop.type = "button";
    stop.className = "port-stop";
    stop.textContent = t("ports.stop", "프로세스 중지");
    stop.addEventListener("click", async (event) => {
      event.stopPropagation();
      stop.disabled = true;
      try {
        await invoke("stop_workspace_port", { pid: port.pid, port: port.port });
        await refreshPorts();
      } catch (error) {
        showError(error);
        stop.disabled = false;
      }
    });
    row.appendChild(stop);
  }
  return row;
}

function paintPortRows() {
  const body = el("ports-body");
  body.replaceChildren();
  const note = el("ports-note");
  const why = portScan?.unavailable ?? null;
  // Why there is nothing, when the answer is not "nothing is listening".
  // Orca says which platform and what about it, and a popover that said
  // "no ports" about a machine it never scanned would be lying quietly.
  note.hidden = why === null;
  note.textContent = why ?? "";
  if (why !== null) return;

  // Narrowed to the row's own checkout when a row raised it. The head says
  // so the way Orca's does — "라이브 포트 (15)" (`WorktreeCardPorts`
  // 3240f320d7, count from the screenshot's own header) — because a list
  // that silently dropped every other workspace would read as the machine
  // having gone quiet.
  const owned = portsScope === null
    ? ownedPorts()
    : ownedPorts().filter((port) => port.owner_path === portsScope);
  if (portsScope !== null) {
    el("ports-tally").textContent = t("ports.liveCount", "라이브 포트 ({{count}})", {
      count: owned.length,
    });
  }
  if (owned.length === 0) {
    const empty = document.createElement("p");
    empty.className = "ports-empty";
    empty.textContent = t("ports.noneHere", "이 프로젝트의 포트가 없습니다");
    body.appendChild(empty);
  } else {
    for (const port of owned) body.appendChild(portRow(port));
  }

  // A workspace-scoped list is that workspace's alone: Orca's row popover
  // carries no "그 밖" section — those are the bar's to show.
  if (portsScope !== null) return;
  const outside = externalPorts();
  if (outside.length === 0) return;
  // Its own section under a rule, the way Orca separates them: these are
  // somebody else's processes and the list must not read as if they were
  // this project's.
  const section = document.createElement("section");
  section.className = "ports-external";
  const head = document.createElement("p");
  head.className = "ports-section-head";
  head.textContent = t("ports.external", "그 밖");
  section.appendChild(head);
  for (const port of outside) section.appendChild(portRow(port));
  body.appendChild(section);
}

/* Which checkout the popover is ABOUT, when it was raised from a worktree
 * row — Orca's row trigger opens the same list the bar owns, narrowed to
 * that workspace's ports (`WorktreeCardPortsTrigger` beside
 * `getWorkspacePortsByWorktreeId`, WorktreeCard-B91w6t_B.js). One popover,
 * one set of hands (1-g65); a second owner would disagree with the first. */
let portsScope = null;

function setPortsOpen(on, scope = null, anchor = null) {
  const pop = el("ports-pop");
  portsScope = on ? scope : null;
  if (on) showing(pop);
  else closing(pop);
  el("sb-ports").setAttribute(
    "aria-expanded",
    on && portsScope === null ? "true" : "false",
  );
  if (!on) return;
  // Somebody is about to READ this list, which is the one moment the scan is
  // worth what it costs.
  paintMachineFigures(true);
  // `offsetWidth`, not the client rect: the popover is mid-entrance here and
  // that animation carries a `scale(0.95)`, which a client rect reports and a
  // layout box does not. Measuring the transformed box put the panel 5% of
  // its own width off the segment that opened it.
  const width = pop.offsetWidth;
  if (anchor instanceof HTMLElement) {
    // Raised from a sidebar row: beside the row, kept inside the window —
    // the stylesheet's bar-corner right/bottom must not fight the inline
    // left/top, so both are taken over for this showing.
    const at = anchor.getBoundingClientRect();
    pop.style.right = "auto";
    pop.style.bottom = "auto";
    pop.style.left = `${Math.max(8, Math.min(at.right + 8, window.innerWidth - width - 8))}px`;
    pop.style.top = `${Math.max(8, Math.min(at.top, window.innerHeight - pop.offsetHeight - 8))}px`;
  } else {
    // Anchored to the control that raised it rather than to the window's
    // corner: the segment is not the last thing on the bar, and a panel that
    // opens somewhere other than what was clicked reads as a different
    // panel. Kept inside the window, because the segment can sit near
    // either edge. The row-anchor overrides are cleared so the stylesheet's
    // bottom placement holds again.
    pop.style.left = "";
    pop.style.top = "";
    pop.style.bottom = "";
    const trigger = el("sb-ports").getBoundingClientRect();
    const right = Math.max(8, window.innerWidth - trigger.right);
    pop.style.right = `${Math.min(right, window.innerWidth - width - 8)}px`;
  }
  // The rows already known, so the popover is never blank while the scan
  // runs. `paintMachineFigures(true)` above is what refreshes them — asking
  // twice here was a second pair of `lsof` calls for one gesture.
  paintPortRows();
}

el("sb-ports").addEventListener("click", () => setPortsOpen(el("ports-pop").hidden));

/* ---- the plan segment ----
 *
 * Orca leads its bar with provider usage (`DEFAULT_STATUS_BAR_ITEMS`), each
 * segment the TIGHTEST window — the one closest to its limit
 * (`getTightestUsageSection`) — as a 28×5 bar and an 11px figure, amber from
 * 60 and red from 80 (`barColor`). The numbers come from the backend's
 * hidden-terminal scan; between scans, a live rate-limit frame from a
 * running session updates the session window the way Orca's live ingest
 * does. The window polls at Orca's own ambient cadence and the backend
 * refuses to scan more often than its floor, so mashing refresh costs one
 * scan, not five. */

let claudeUsage = null;
let claudeFetching = false;
let codexUsage = null;
let codexFetching = false;
let antigravityUsage = null;
let antigravityFetching = false;
let kimiUsage = null;
let kimiFetching = false;
let grokUsage = null;
let grokFetching = false;
let opencodeUsage = null;
let opencodeFetching = false;
/* One follow-up timer per provider. Two scans run against two different CLIs
 * and settle at their own speeds; a single timer would let whichever answered
 * first cancel the other's follow-up and leave its segment saying ··· forever. */
const usageAskTimers = new Map();
/* Which provider the panel is showing. The bar has two segments and one panel,
 * so the panel has to remember whose it is — otherwise the refresh button
 * re-reads Claude while the person is looking at Codex. */
let usagePanelProvider = "claude";
let usagePanelOpen = false;

/* 한 번의 정착 타이머, 이름 하나에 하나.
 *
 * 같은 이름으로 다시 부르면 앞의 것을 물리고 다시 센다 — 뜻은 "이 사건이 그친
 * 뒤에 한 번"이다. 이 창에 그 뜻이 필요한 자리가 둘이고(사이드바의 활동 정렬,
 * 상태 바의 사용량), 둘 다 같은 사건 — 에이전트가 움직였다 — 을 듣는다. 타이머
 * 장부를 각자 들면 같은 버그를 두 번 고치게 된다. */
const settleTimers = new Map();

function settle(name, ms, run) {
  clearTimeout(settleTimers.get(name));
  settleTimers.set(
    name,
    setTimeout(() => {
      settleTimers.delete(name);
      run();
    }, ms),
  );
}

/* 에이전트가 조용해진 뒤 사용량을 다시 묻기까지. 3초는 한 턴이 끝났다는 뜻으로
 * 충분히 길고, 사람이 상태 바를 쳐다보기에는 충분히 짧다. */
const USAGE_SETTLE_MS = 3_000;

const USAGE_AMBIENT_MS = 15 * 60 * 1000;
const USAGE_STALE_MS = 30 * 60 * 1000;

/* 「이 CLI 에 로그인이 없다」고 스캔이 말할 때의 상태어.
 *
 * 백엔드가 이것을 `unavailable` 과 **다른 낱말**로 만든 이유를 그쪽 주석이 적어
 * 뒀다: 「계정 목록이 이것을 읽어 행을 표시하기 때문이고, 「읽을 수 없었다」는
 * 아무도 표시해선 안 된다」(zerocode-shell `usage.rs:602-604`, 1-g15). 그런데
 * 그 낱말을 읽는 곳이 정말로 계정 목록 **하나뿐**이었다 — 항상 보이는 표면은
 * 배운 적이 없어서, 로그인이 풀린 공급자의 세그먼트가 아이콘만 남은 빈칸으로
 * 서 있었다. 낱말을 여기 한 번 적어 두는 것은 두 쪽이 서로 다른 철자로
 * 갈라지는 것을 막기 위해서다. */
const USAGE_SIGNED_OUT = "signed_out";

function usageSections(usage) {
  if (!usage) return [];
  return [
    usage.session && { label: t("usage.session", "세션"), window: usage.session },
    usage.weekly && { label: t("usage.weekly", "주간"), window: usage.weekly },
    usage.fable_weekly && { label: "Fable", window: usage.fable_weekly },
    // A 30-day window, which only OpenCode Go and unified-billing Grok have.
    // Its own row rather than a borrowed one: "Fable" is a model's name, and
    // a month wearing it reads as a model to whoever is looking.
    usage.monthly && { label: t("usage.monthly", "월간"), window: usage.monthly },
  ].filter(Boolean);
}

/* The bar's own list, which is NOT the panel's.
 *
 * Two rules the segment holds and the popover does not, both measured off
 * `VerboseProviderUsage` (StatusBar.tsx:1173-1248) and pinned by Orca's own
 * `provider-segment-monthly-window.test.tsx`:
 *
 *  1. A provider whose quota is answered per MODEL is named by those models
 *     — `Pro 80% 사용 · Flash 25% 사용`, the NAME first — rather than by the
 *     derived session row the popover borrows. Three names are shown; the
 *     rest of a long roster belongs to the panel, not to a status bar.
 *  2. A monthly window rides the bar only for a provider that has nothing
 *     shorter. With a session and a week already there, a third figure makes
 *     the segment longer than the bar it sits in, and the panel is where the
 *     original puts it.
 */
const STATUS_BAR_BUCKET_NAMES = new Set(["Flash", "Pro", "1.5 Pro"]);

function segmentUsageSections(usage) {
  if (!usage) return [];
  const named = (usage.buckets ?? []).filter((bucket) =>
    STATUS_BAR_BUCKET_NAMES.has(bucket.name),
  );
  if (named.length > 0) {
    return named.map((bucket) => ({ label: bucket.name, window: bucket, named: true }));
  }
  return usageSections(usage).filter(
    (section) =>
      section.window !== usage.monthly || (!usage.session && !usage.weekly),
  );
}

function usageTone(pct) {
  return pct >= 80 ? "hot" : pct >= 60 ? "warn" : "";
}

/* Orca's `formatResetDuration`: minutes under an hour, `Nh Mm` under a day,
 * `Nd Nh` beyond. The compact roster uses the bare duration while detailed rows
 * add the localized reset sentence, so the arithmetic has one owner.
 *
 * The WORDS come from the catalogs, and from the same entries the stats head's
 * own span uses (`statsWorkedWords`) — this used to build `1h 8m` out of letters
 * written here and drop it into a translated sentence, so a Korean window said
 * 재설정까지 1h 8m and a Japanese one リセットまで 1h 8m. A number is the same in
 * every language; its unit is not. */
function usageDuration(ms) {
  if (ms <= 0) return null;
  const mins = Math.floor(ms / 60000);
  if (mins < 60) return t("stats.minutes", "{{n}}분", { n: mins });
  const hours = Math.floor(mins / 60);
  if (hours >= 24) {
    const days = Math.floor(hours / 24);
    const rem = hours % 24;
    return rem > 0
      ? t("stats.daysHours", "{{d}}일 {{h}}시간", { d: days, h: rem })
      : t("session.days", "{{n}}일", { n: days });
  }
  const rem = mins % 60;
  return rem > 0
    ? t("stats.hoursMinutes", "{{h}}시간 {{m}}분", { h: hours, m: rem })
    : t("stats.hours", "{{n}}시간", { n: hours });
}

/* The status bar is a glance, not a sentence. Orca keeps reset tails in the
 * compact `4h 28m` / `6d 21h` vocabulary even when the surrounding chrome is
 * translated; the full localized wording remains in the panel below. */
function statusUsageDuration(ms) {
  if (ms <= 0) return null;
  const mins = Math.floor(ms / 60000);
  if (mins < 60) return `${mins}m`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) {
    const rem = mins % 60;
    return rem > 0 ? `${hours}h ${rem}m` : `${hours}h`;
  }
  const days = Math.floor(hours / 24);
  const rem = hours % 24;
  return rem > 0 ? `${days}d ${rem}h` : `${days}d`;
}

function usageCountdown(ms) {
  const dur = usageDuration(ms);
  if (dur === null) return t("usage.resetsNow", "지금 재설정");
  return t("usage.resets", "재설정까지 {{when}}", { when: dur });
}

/* Which window a row is, when the provider did not name it (`shortLabel`).
 *
 * A length in the catalogs' words, with one exception: the weekly quota is a
 * NAME rather than a length — the original says `wk`, and 7일 here would read
 * as seven days remaining instead of the week's own bucket. The four cases the
 * original spells separately (10080, 300, 60, the rest) are three here: 300 and
 * 60 are what the hours branch already answers. */
function usageWindowLabel(window) {
  const mins = window.window_minutes;
  if (mins === 10080) return t("usage.windowWeek", "주");
  if (mins % 1440 === 0) return t("session.days", "{{n}}일", { n: mins / 1440 });
  if (mins % 60 === 0) return t("stats.hours", "{{n}}시간", { n: mins / 60 });
  return t("stats.minutes", "{{n}}분", { n: mins });
}

function compactUsageWindowLabel(section) {
  // A model-named bucket is told apart by its NAME; only an unnamed window is
  // told apart by which window it is (`shortLabel`, UsageRosterPanel.tsx:39-46).
  if (section.named || section.label === "Fable") return section.label;
  if (section.window.resets_at == null) return usageWindowLabel(section.window);
  return usageDuration(section.window.resets_at - Date.now()) ?? t("usage.now", "지금");
}

function statusUsageWindowLabel(section) {
  if (section.named || section.label === "Fable") return section.label;
  if (section.window.resets_at == null) {
    const mins = section.window.window_minutes;
    if (mins === 10080) return "wk";
    if (mins % 1440 === 0) return `${mins / 1440}d`;
    if (mins % 60 === 0) return `${mins / 60}h`;
    return `${mins}m`;
  }
  return statusUsageDuration(section.window.resets_at - Date.now()) ?? "now";
}

/* Every window a provider reports, the way the popover and the tightest pick
 * both read it (`getWindowSections`, tooltip.tsx:141-163).
 *
 * A provider answering per MODEL is listed by those models AND still by its
 * week: the buckets replace the derived session row — the session IS the
 * tightest bucket, so drawing both prints one figure twice — but the weekly
 * window is a different fact and stays. */
function providerSections(usage) {
  if (!usage) return [];
  const named = (usage.buckets ?? []).map((bucket) => ({
    label: bucket.name,
    window: bucket,
    named: true,
  }));
  if (named.length === 0) return usageSections(usage);
  return usage.weekly
    ? [...named, { label: t("usage.weekly", "주간"), window: usage.weekly }]
    : named;
}

/* The one window a compact bar shows: the most spent, whatever kind it is.
 *
 * Urgency is read off consumption even when the person displays "% 남음", so
 * the comparison is always the used figure and never the displayed one. */
function tightestUsage(usage = claudeUsage) {
  const sections = providerSections(usage);
  if (sections.length === 0) return null;
  return sections.reduce((held, one) =>
    one.window.used_percent > held.window.used_percent ? one : held);
}

/* The providers this bar can carry, in Orca's own order — `claude`, `codex`
 * (`DEFAULT_STATUS_BAR_ITEMS`, I18nProvider-4EBrmTGg.js:21360).
 *
 * A table rather than a second copy of the segment code: the two segments are
 * the same anatomy against different screens, and the moment they were written
 * twice is the moment one of them stops getting the next fix. What differs is
 * named here — the command, the state, the element prefix — and everything
 * else is shared by construction. */
const USAGE_PROVIDERS = [
  {
    id: "claude",
    name: "Claude",
    prefix: "sb-claude",
    command: "claude_usage",
    read: () => ({ usage: claudeUsage, fetching: claudeFetching }),
    write: (usage, fetching) => {
      claudeUsage = usage;
      claudeFetching = fetching;
    },
    /* 로그인이 풀렸을 때 갈 곳. 원본은 로스터 행마다 이 손을 받고, 손이 **없는**
     * 공급자에는 단추를 세우지 않는다(`canSignIn`, UsageRosterPanel.tsx:212,
     * `:288`) — 눌러도 아무 일이 없는 단추는 없는 단추보다 나쁘다. 우리에게 그
     * 길이 있는 것은 계정 기구를 가진 둘뿐이고(1-g103 의 「다시 로그인」이 사는
     * 곳), 그 사실을 다른 곳의 목록이 아니라 **공급자 기록 자신**이 든다. */
    signIn: () => showProviderAccounts(),
    // Claude's segment is always there: this window's own default agent is
    // claude, so an empty one still says "not read yet" about something the
    // person has.
    alwaysShown: true,
  },
  {
    id: "codex",
    name: "Codex",
    prefix: "sb-codex",
    command: "codex_usage",
    read: () => ({ usage: codexUsage, fetching: codexFetching }),
    write: (usage, fetching) => {
      codexUsage = usage;
      codexFetching = fetching;
    },
    signIn: () => showProviderAccounts(),
    alwaysShown: false,
  },
  {
    id: "antigravity",
    name: "Antigravity",
    prefix: "sb-antigravity",
    command: "antigravity_usage",
    read: () => ({ usage: antigravityUsage, fetching: antigravityFetching }),
    write: (usage, fetching) => {
      antigravityUsage = usage;
      antigravityFetching = fetching;
    },
    // 이제 갈 길이 있다: 이 로그인을 만드는 카드가 그 화면에 있다. 없던
    // 동안 세그먼트는 「Antigravity 로그인 필요」라고만 말하고 사람을
    // 터미널의 `zo login google` 로 보냈다.
    signIn: () => showProviderAccounts(),
    alwaysShown: false,
  },
  // Kimi reads a credentials file the CLI owns and never writes it back
  // (`kimi-fetcher.ts:298-301`), so there is no opt-in to wait for and no
  // account to scope to — the segment simply stays hidden on a machine with
  // no Kimi on it.
  {
    id: "kimi",
    name: "Kimi",
    prefix: "sb-kimi",
    command: "kimi_usage",
    read: () => ({ usage: kimiUsage, fetching: kimiFetching }),
    write: (usage, fetching) => {
      kimiUsage = usage;
      kimiFetching = fetching;
    },
    alwaysShown: false,
  },
  // Grok reads the session file its CLI keeps, and the reading carries
  // whichever account that file held — so a session swapped underneath us
  // drops the old figure rather than showing it under the new name.
  {
    id: "grok",
    name: "Grok",
    prefix: "sb-grok",
    command: "grok_usage",
    read: () => ({ usage: grokUsage, fetching: grokFetching }),
    write: (usage, fetching) => {
      grokUsage = usage;
      grokFetching = fetching;
    },
    alwaysShown: false,
  },
  // OpenCode Go has no usage API — the figures are scraped off the page the
  // site renders for a signed-in session, which is why this one needs a
  // pasted cookie rather than a file the CLI already wrote.
  {
    id: "opencode-go",
    name: "OpenCode Go",
    prefix: "sb-opencode-go",
    command: "opencode_usage",
    read: () => ({ usage: opencodeUsage, fetching: opencodeFetching }),
    write: (usage, fetching) => {
      opencodeUsage = usage;
      opencodeFetching = fetching;
    },
    alwaysShown: false,
  },
];

function usageProvider(id) {
  return USAGE_PROVIDERS.find((one) => one.id === id) ?? USAGE_PROVIDERS[0];
}

/* Which providers have a sign-in road THIS window can walk, learned from the
 * backend's CLI login table (`cli_login_list`, painted on the accounts pane)
 * rather than written per record above: a provider that gains a row there
 * gains its button here, with no second list to forget. Claude, Codex and
 * Antigravity keep the `signIn` hand on their own records — their cards are
 * hand-made. */
const cliSignIns = new Set();

function noteCliLoginRows(rows) {
  cliSignIns.clear();
  for (const row of rows) cliSignIns.add(row.agent);
  paintUsagePanel();
}

/* Where a signed-out gauge sends a person, or `null` when this window has no
 * road for that provider — and then no button stands, because a button that
 * does nothing is worse than none. */
function usageSignIn(provider) {
  if (provider.signIn) return provider.signIn;
  return cliSignIns.has(provider.id) ? showProviderAccounts : null;
}

function usageProviderDomain(provider) {
  return agentRows.find((one) => one.id === provider.id)?.favicon_domain ?? "";
}

function usageProviderIcon(provider, boxed = false) {
  const mark = agentIcon({
    id: provider.id,
    name: provider.name,
    favicon_domain: usageProviderDomain(provider),
  });
  if (!boxed) return mark;
  const box = document.createElement("span");
  box.className = "usage-roster-icon";
  box.setAttribute("aria-hidden", "true");
  box.appendChild(mark);
  return box;
}

/* The bar's Claude glyph, drawn once it can be drawn properly.
 *
 * Built lazily and kept, but NOT kept when it was built without a domain: this
 * segment paints on the first usage frame, which can arrive before the agent
 * registry does, and the old form cached whatever it managed to make on that
 * first pass — a letter tile that never became the mark. `dataset.domain` is
 * the record of which version is on screen, so the swap happens exactly once
 * and only when there is something better to swap to. */
function paintProviderGlyph(provider) {
  const ico = el(`${provider.prefix}-ico`);
  const domain = usageProviderDomain(provider);
  if (ico.dataset.domain === domain) return;
  ico.dataset.domain = domain;
  ico.replaceChildren(usageProviderIcon(provider));
}

function paintProviderSegment(provider) {
  paintProviderGlyph(provider);
  const { usage, fetching } = provider.read();
  const tight = tightestUsage(usage);
  const signedOut = usage?.status === USAGE_SIGNED_OUT;
  const segment = el(provider.prefix);
  segment.dataset.usageMode = statusBarUsageMode;
  // A segment for an agent this machine does not have would be a permanently
  // empty figure nobody can act on. Shown once there is either a reading or a
  // reason — which is also what makes the FIRST scan visible. `unavailable`
  // is a provider the person never set up
  // — Orca hides those rather than drawing a bar that reads "--" forever
  // (`isProviderConfigured`, status-bar-provider-visibility.ts:49-60), while
  // `error` stays visible: a CONFIGURED provider failing transiently should
  // wear its triangle, not flap off the bar.
  segment.hidden =
    !statusBarItemEnabled(provider.id) ||
    (!provider.alwaysShown && ((!usage && !fetching) || usage?.status === "unavailable"));
  el(`${provider.prefix}-wait`).hidden = !(fetching && !tight);
  el(`${provider.prefix}-bar`).hidden = !tight || statusBarUsageMode === "compact";
  // 숫자가 없어도 낱말은 있다: 로그인이 풀린 공급자는 읽을 수치가 없지만 사람이
  // 할 일이 있으므로, 수치가 서는 그 자리에 낱말이 선다.
  el(`${provider.prefix}-pct`).hidden = !tight && !signedOut;
  // Stale is a fact about time, error a fact about the last scan — either
  // one earns the triangle, with the numbers still shown when they exist.
  // `unavailable` is neither: not signed in is a state, not a failure, and a
  // warning triangle on it sends somebody looking for a bug that is not there.
  //
  // `signed_out` 은 그 면제에 들지 않는다 — 그리고 이 구분이 백엔드가 두 낱말을
  // 따로 만든 이유다. `unavailable` 은 애초에 세운 적 없는 공급자라 **고칠 것이
  // 없고**, `signed_out` 은 있던 로그인이 풀린 것이라 고칠 수 있다. 삼각형은
  // 「없는 버그를 찾아 나서게 만드는 표시」가 아니라 「손이 필요하다」는 표시이고,
  // 여기서는 정말로 손이 필요하다(설정의 그 행이 「다시 로그인」을 들고 있다).
  const stale = usage && Date.now() - usage.updated_at > USAGE_STALE_MS && !fetching;
  el(`${provider.prefix}-warn`).hidden = !(usage?.status === "error" || stale || signedOut);
  if (!tight) {
    if (signedOut) {
      const said = el(`${provider.prefix}-pct`);
      // 공급자의 이름을 든 낱말: 「로그인 필요」가 둘 나란히 서면 어느 것이
      // 어느 공급자인지는 12px 아이콘 하나가 말해야 했다(실보고 2026-09-02
      // 「가독성이 떨어짐」). 수치가 있을 때는 아이콘과 막대가 그 일을 한다.
      said.textContent = t("usage.signedOutOf", "{{provider}} 로그인 필요", {
        provider: provider.name,
      });
      // 색조는 사용량의 것이다 — 여기엔 사용량이 없으므로 앞 스캔이 남긴 색을
      // 들고 있어서는 안 된다.
      delete said.dataset.tone;
      // 왜인지는 호버가 말한다. 스캔이 읽은 화면의 문장을 그대로 옮긴다.
      segment.dataset.tip = usage.error ?? t("usage.signedOut", "로그인 필요");
    }
    return;
  }
  const usedPct = tight.window.used_percent;
  const displayedPct = displayedUsagePercentage(usedPct);
  const fill = el(`${provider.prefix}-fill`);
  fill.style.width = `${displayedPct}%`;
  // Urgency belongs in the detailed panel. The always-visible status rail is
  // deliberately neutral so a healthy quota does not turn the whole footer
  // into an alert surface as it approaches a reset.
  delete fill.dataset.tone;
  const figure = el(`${provider.prefix}-pct`);
  // 신판 실측(Image #81): verbose의 창 하나는 「N% 사용 <리셋 카운트다운>」이고
  // (Fable 창만 제 이름), 창들은 ·로 나란하다 — 창 라벨(wk·5h)이 아니라
  // 카운트다운이 꼬리인 것이 실측이 우리와 갈리던 자리다.
  // A named bucket wears its NAME first and no window tail — `Pro 80% 사용`
  // — because the name is what distinguishes it, where an unnamed window is
  // told apart by which window it is.
  figure.textContent = statusBarUsageMode === "compact"
    ? `${displayedPct}% ` + statusUsageWindowLabel(tight)
    : segmentUsageSections(usage)
      .map((section) => {
        const spent = spokenUsagePercentage(section.window.used_percent);
        if (section.named) return `${section.label} ${spent}`;
        const tail = statusUsageWindowLabel(section);
        return `${spent} ${tail}`;
      })
      .join(" · ");
  delete figure.dataset.tone;
  // 로그인이 돌아오면 호버도 돌아온다: 위 갈래가 세그먼트의 말을 바꿔 두므로,
  // 수치가 서는 이 갈래가 그것을 되돌리지 않으면 고쳐진 뒤에도 옛 이유가 남는다.
  segment.dataset.tip = t("usage.title", "플랜 사용량");
}

/* Every segment the bar carries. Named for what it does now: it painted only
 * Claude's before there was a second provider. */
function paintUsageSegments() {
  for (const provider of USAGE_PROVIDERS) paintProviderSegment(provider);
  paintUsageRefresh();
}

/* The bar's own refresh, after the last usage segment.
 *
 * Orca keeps one on the BAR, not only inside the panel — measured whole
 * (StatusBar-DNeqEWup.js): shown while `anyVisible && !isEmptyUsageState`,
 * `RefreshCw size={11}` spinning while a scan runs and disabled meanwhile,
 * aria "Refresh rate limits" with a "Refresh usage data" tooltip — and its
 * absence was reported by name ("밑에 새로고침도 바깥에 있고"). Clicking asks
 * every visible provider with force, the same road the panel's button takes. */
function paintUsageRefresh() {
  const busy = USAGE_PROVIDERS.some((provider) => provider.read().fetching);
  const seen = USAGE_PROVIDERS.some(
    (provider) => !el(provider.prefix).hidden && provider.read().usage,
  );
  const button = el("sb-usage-refresh");
  button.hidden = !seen;
  button.disabled = busy;
  button.classList.toggle("is-refreshing", busy);
  button.dataset.tip = t("usage.refreshData", "사용량 데이터 새로 고침");
  button.setAttribute("aria-label", t("usage.refreshLimits", "rate-limit 새로고침"));
}

el("sb-usage-refresh").addEventListener("click", () => {
  for (const provider of USAGE_PROVIDERS) {
    if (!el(provider.prefix).hidden) void refreshProviderUsage(provider, true);
  }
});

function paintUsageRows(host, usage, fetching, mode = "verbose") {
  host.replaceChildren();
  host.dataset.usageMode = mode;
  const sections = providerSections(usage);
  if (sections.length === 0) {
    const none = document.createElement("p");
    none.className = "usage-none";
    none.textContent = fetching
      ? t("usage.loading", "확인 중…")
      : (usage?.error ?? t("usage.none", "사용량 정보가 없습니다"));
    host.appendChild(none);
  }
  const visibleSections = mode === "compact" ? [tightestUsage(usage)].filter(Boolean) : sections;
  for (const section of visibleSections) {
    const box = document.createElement("div");
    box.className = "usage-row";
    const label = document.createElement("div");
    label.className = "usage-row-label";
    label.textContent = section.label;
    const displayedPct = displayedUsagePercentage(section.window.used_percent);
    if (mode === "compact") {
      box.classList.add("usage-row--compact");
      label.textContent = compactUsageWindowLabel(section);
      const figure = document.createElement("span");
      figure.className = "usage-row-compact-figure";
      figure.textContent = `${displayedPct}%`;
      figure.dataset.tone = usageTone(section.window.used_percent);
      box.append(label, figure);
      host.appendChild(box);
      continue;
    }
    const bar = document.createElement("div");
    bar.className = "usage-row-bar";
    const fill = document.createElement("div");
    fill.className = "usage-row-fill";
    fill.style.width = `${displayedPct}%`;
    fill.dataset.tone = usageTone(section.window.used_percent);
    bar.appendChild(fill);
    const foot = document.createElement("div");
    foot.className = "usage-row-foot";
    const spent = document.createElement("span");
    spent.textContent = spokenUsagePercentage(section.window.used_percent);
    const reset = document.createElement("span");
    if (section.window.resets_at != null) {
      reset.textContent = usageCountdown(section.window.resets_at - Date.now());
    } else if (section.window.reset_description) {
      reset.textContent = section.window.reset_description;
    }
    foot.append(spent, reset);
    box.append(label, bar, foot);
    host.appendChild(box);
  }
}

function paintUsageUpdated(node, usage) {
  if (!usage) {
    node.hidden = true;
    return;
  }
  const mins = Math.floor((Date.now() - usage.updated_at) / 60000);
  node.textContent = mins < 1
    ? t("usage.justNow", "방금 확인")
    : t("usage.updated", "{{mins}}분 전 확인", { mins });
  node.hidden = false;
}

/* ── 사용량 팝오버: 한 공급자가 아니라 **모두** ──────────────────────────
 *
 * 원본의 팝오버는 로스터다(`UsageRosterPanel.tsx`): 제목 「Usage」와 그 옆의
 * 「all agents」, 새로 고침, 밀도 선택, 그리고 **공급자 하나에 한 행**. 우리
 * 것은 막대의 어느 조각을 눌렀느냐로 한 공급자만 보여 주고 있었다 — 그런데
 * 「지금 어디가 빡빡한가」는 하나를 묻는 질문이 아니라 **비교**하는 질문이고,
 * 비교하려면 판을 다섯 번 열었다 닫아야 했다.
 *
 * 행의 상태는 여섯 갈래이고 원본의 것을 그대로 옮긴다
 * (`usage-roster-row-state.ts:33-77`). 다만 **「로그인이 풀렸다」를 아는 방법이
 * 다르다**: 원본은 오류 문장에 일곱 개의 정규식을 대 본다(`:10-31`) — 그리고 그
 * 주석이 위험까지 적어 뒀다: 토큰 갱신 실패나 네트워크 실패도 인증을 말하는데
 * 그때 세션은 살아 있으므로 **명시적인 로그아웃 문구만** 로그인 단추를 얻는다.
 * 우리 백엔드는 그것을 사실로 준다(`usage.rs` 의 `signed_out`) — 문장을 훑을
 * 필요가 없고, 원본이 관리하던 그 위험(살아 있는 세션에 로그인을 권하는 일)이
 * 우리에게는 **구조적으로 없다**. */
const USAGE_ROSTER_UNAVAILABLE = "unavailable";
const USAGE_ROSTER_ERROR = "error";

function usageRosterState(usage, fetching, sections) {
  if (sections.length > 0) return { kind: "usage", says: null };
  // 아직 아무 것도 읽지 않은 것과 읽는 중인 것은 사람에게 같은 일이다.
  if (fetching || usage == null) return { kind: "loading", says: t("usage.loading", "확인 중…") };
  if (usage.status === USAGE_SIGNED_OUT) {
    return { kind: "sign-in", says: t("usage.notSignedIn", "로그인되어 있지 않습니다") };
  }
  // 오류는 **마지막 조회에 대한 사실**이라 그 문장을 그대로 보여 준다.
  if (usage.status === USAGE_ROSTER_ERROR || usage.error) {
    return { kind: "error", says: usage.error ?? t("usage.none", "사용량 정보가 없습니다") };
  }
  /* 세운 적 없는 공급자. 막대에서는 숨고(`paintProviderSegment` — 영원히 「--」인
   * 막대는 아무도 손댈 수 없다) 여기서는 보인다. 원본의 상태 기계에 이 갈래가
   * 있는 이유가 그것이다: 목록은 「없다」를 말할 수 있는 유일한 자리다. */
  if (usage.status === USAGE_ROSTER_UNAVAILABLE) {
    return { kind: "unavailable", says: t("usage.unavailable", "사용량을 읽을 수 없습니다") };
  }
  return { kind: "empty", says: t("usage.none", "사용량 정보가 없습니다") };
}

/* `max_20x` → `Max 20x`, `chatgpt_plus` → `ChatGPT Plus`.
 *
 * 원본의 `formatPlanLabel`(usage-roster-formatting.ts:6-20)을 그대로 옮긴다:
 * 공백·밑줄·붙임표로 자르고 낱말마다 첫 글자를 올리며 `chatgpt` 하나만
 * 손으로 적는다. 이 값은 여태 페이로드에 실려 오고도(`ProviderUsage.plan_type`)
 * 창이 한 번도 읽지 않았다 — 로스터가 그것을 처음 읽는 자리다. */
function usagePlanLabel(planType) {
  const said = planType?.trim();
  if (!said) return null;
  return said
    .split(/[\s_-]+/)
    .map((word) => {
      const low = word.toLowerCase();
      return low === "chatgpt" ? "ChatGPT" : low.charAt(0).toUpperCase() + low.slice(1);
    })
    .join(" ");
}

/* 로스터에 설 공급자들, **빡빡한 것부터**.
 *
 * 정렬이 원본의 것이다(`UsageRosterPanel.tsx:227-229`): 한계에 가장 가까운
 * 에이전트가 맨 위에 앉아 목록의 첫 줄이 곧 답이 되도록. 목록에 드는 조건은
 * 막대의 것과 **다르다**: 막대는 사람이 끈 조각과 세운 적 없는 공급자를 숨기지만
 * 이 목록은 왜 숫자가 없는지 말할 수 있는 유일한 자리다. */
function usageRosterProviders() {
  const shown = USAGE_PROVIDERS.filter((provider) => {
    const { usage, fetching } = provider.read();
    return provider.alwaysShown || usage != null || fetching;
  });
  return shown.sort((a, b) => usageRosterWorst(b) - usageRosterWorst(a));
}

function usageRosterWorst(provider) {
  const sections = providerSections(provider.read().usage);
  return sections.reduce((worst, one) => Math.max(worst, one.window.used_percent ?? 0), 0);
}

/* 가장 먼저 풀리는 창의 리셋 문구 — 상세 모드 머리의 오른쪽 끝
 * (`soonestResetLabel`, UsageRosterPanel.tsx:129). 가장 나중이 아니라 가장
 * 먼저인 이유는 사람이 기다리는 것이 **다음** 리셋이라서다. 시각을 아는 창이
 * 하나도 없으면 창이 스스로 적어 보낸 설명을 쓰고, 그것도 없으면 아무 말도
 * 하지 않는다 — 여기서 지어낸 말은 틀린 말이 된다. */
function usageRosterReset(sections) {
  const timed = sections.filter((one) => one.window.resets_at != null);
  if (timed.length > 0) {
    const soonest = timed.reduce((first, one) =>
      one.window.resets_at < first.window.resets_at ? one : first);
    return usageCountdown(soonest.window.resets_at - Date.now());
  }
  return sections.find((one) => one.window.reset_description)?.window.reset_description ?? null;
}

function paintUsageRosterMetrics(host, sections) {
  host.replaceChildren();
  for (const section of sections) {
    const metric = document.createElement("span");
    metric.className = "usage-roster-metric";
    const label = document.createElement("span");
    label.className = "usage-roster-metric-label";
    label.textContent = section.label;
    const bar = document.createElement("span");
    bar.className = "usage-roster-metric-bar";
    const fill = document.createElement("span");
    fill.className = "usage-roster-metric-fill";
    const displayed = displayedUsagePercentage(section.window.used_percent);
    fill.style.width = `${displayed}%`;
    fill.dataset.tone = usageTone(section.window.used_percent);
    bar.appendChild(fill);
    const figure = document.createElement("span");
    figure.className = "usage-roster-metric-figure";
    figure.textContent = `${displayed}%`;
    figure.dataset.tone = usageTone(section.window.used_percent);
    metric.append(label, bar, figure);
    host.appendChild(metric);
  }
}

/* 한 공급자의 행: 이름 줄 하나 — 그리고 상세 모드에서 수치가 있으면 그 아래 한 줄. */
function usageRosterRow(provider) {
  const { usage, fetching } = provider.read();
  const sections = providerSections(usage);
  const state = usageRosterState(usage, fetching, sections);
  const row = document.createElement("div");
  row.className = "usage-roster-row";
  row.dataset.provider = provider.id;
  row.dataset.state = state.kind;

  const head = document.createElement("button");
  head.className = "usage-roster-head";
  head.type = "button";
  const name = document.createElement("span");
  name.className = "usage-roster-name";
  name.textContent = provider.name;
  const plan = usagePlanLabel(usage?.plan_type);
  if (plan) {
    // 원본은 이름 뒤에 가운뎃점으로 잇고 플랜만 흐리게 둔다(`:141-142`) — 하나는
    // 무엇인가이고 하나는 어느 요금제인가라서 같은 무게로 읽히면 안 된다.
    const said = document.createElement("span");
    said.className = "usage-roster-plan";
    said.textContent = ` · ${plan}`;
    name.appendChild(said);
  }
  head.append(usageProviderIcon(provider, true), name);

  if (state.kind === "usage") {
    /* 머리의 오른쪽 끝은 **모드마다 다른 것**을 말한다. 원본에서 `tightest` 는
     * compact 일 때만 계산되고(`UsageRow`, `:132`), verbose 일 때 그 자리에
     * 오는 것은 **가장 이른 리셋**이다(`:164`) — 아래 줄에 창마다의 수치가 다
     * 있으니 머리에서 하나를 골라 되풀이할 이유가 없고, 대신 그 판에서 아직
     * 없는 사실(언제 풀리나)을 말한다. 첫 판에서 두 모드 모두 수치를 세웠는데,
     * 그건 상세 모드의 머리를 아래 줄의 요약으로 만드는 것이었다. */
    if (statusBarUsageMode === "compact") {
      const tight = tightestUsage(usage);
      if (tight) {
        // 라벨과 수치가 따로인 이유는 원본의 `UsageMetric` 이 그렇기 때문이고
        // (`:88-109`), 어느 창인지 없는 백분율은 답이 아니기 때문이다.
        const label = document.createElement("span");
        label.className = "usage-roster-label";
        label.textContent = compactUsageWindowLabel(tight);
        const figure = document.createElement("span");
        figure.className = "usage-roster-figure";
        figure.textContent = `${displayedUsagePercentage(tight.window.used_percent)}%`;
        figure.dataset.tone = usageTone(tight.window.used_percent);
        head.append(label, figure);
      }
    } else {
      const soon = usageRosterReset(sections);
      if (soon) {
        const reset = document.createElement("span");
        reset.className = "usage-roster-reset";
        reset.textContent = soon;
        head.appendChild(reset);
      }
    }
  } else {
    const says = document.createElement("span");
    says.className = "usage-roster-says";
    says.textContent = state.says;
    head.appendChild(says);
  }
  // 행 전체가 그 공급자의 상세로 가는 문이다 — 원본도 행을 누르면 설정으로
  // 보낸다(`onOpenProvider`, `:317-324`).
  head.addEventListener("click", () => showUsageDetails(provider.id));
  row.appendChild(head);

  // 로그인 단추는 **확인된 로그아웃**에만, 그리고 갈 길이 있을 때만 선다 —
  // 기록 자신의 손이거나, 백엔드의 CLI 로그인 표가 이 공급자를 알 때.
  const signIn = usageSignIn(provider);
  if (state.kind === "sign-in" && signIn) {
    const enter = document.createElement("button");
    enter.className = "usage-roster-signin";
    enter.type = "button";
    enter.textContent = t("usage.signIn", "로그인");
    enter.addEventListener("click", () => signIn());
    row.appendChild(enter);
  }

  // 상세 모드의 두 번째 줄은 좁은 로스터용 지표다. 전체 폭의 설정 막대를 그대로
  // 축소하면 행마다 세로 공간이 급격히 늘어나므로, Orca처럼 label + 28×5 bar +
  // percentage를 한 줄에 놓고 상태 색은 이 상세 면에서만 쓴다.
  if (state.kind === "usage" && statusBarUsageMode === "verbose") {
    const host = document.createElement("div");
    host.className = "usage-roster-body";
    paintUsageRosterMetrics(host, sections);
    row.appendChild(host);
  }
  return row;
}

function paintUsageRoster(host) {
  host.replaceChildren();
  host.dataset.usageMode = statusBarUsageMode;
  const providers = usageRosterProviders();
  if (providers.length === 0) {
    const none = document.createElement("p");
    none.className = "usage-none";
    none.textContent = t("usage.none", "사용량 정보가 없습니다");
    host.appendChild(none);
    return;
  }
  for (const provider of providers) host.appendChild(usageRosterRow(provider));
}

/* 팝오버의 발 밑 두 줄이 가는 곳. 원본과 같은 두 곳이다(`:326-343`): 숫자의
 * 역사와, 그 숫자를 내는 로그인들. 판을 먼저 닫는 이유는 설정이 그 위를 덮기
 * 때문이다 — 뒤에 남은 팝오버는 돌아왔을 때 낡은 것을 말한다. */
function showUsageDetails(provider = null) {
  setUsageOpen(false);
  if (provider) settingsUsageProvider = provider;
  setSettingsOpen(true);
  showSettingsPane("stats");
  paintStatsUsage();
}

function showProviderAccounts() {
  setUsageOpen(false);
  setSettingsOpen(true);
  showSettingsPane("provider-accounts");
}

/* 발 밑의 「방금 확인」은 이제 판 하나가 아니라 **판 전체**에 대한 말이므로 여러
 * 읽기 중 가장 새것을 말한다. 가장 오래된 것을 말하면 한 공급자가 조용히
 * 실패하는 동안 판 전체가 낡아 보인다. */
function newestUsageReading() {
  let newest = null;
  for (const provider of USAGE_PROVIDERS) {
    const { usage } = provider.read();
    if (usage?.updated_at == null) continue;
    if (newest == null || usage.updated_at > newest.updated_at) newest = usage;
  }
  return newest;
}

el("usage-details").addEventListener("click", () => showUsageDetails());
el("usage-accounts").addEventListener("click", () => showProviderAccounts());

function paintUsagePanel() {
  if (el("usage-pop").hidden) return;
  el("usage-name").textContent = t("usage.title", "플랜 사용량");
  el("usage-scope").textContent = t("usage.allAgents", "모든 에이전트");
  paintUsageRoster(el("usage-body"));
  paintUsageUpdated(el("usage-foot"), newestUsageReading());
}

let settingsUsageProvider = USAGE_PROVIDERS[0].id;

function paintStatsUsage() {
  const select = el("settings-usage-provider");
  const selected = usageProvider(settingsUsageProvider);
  if (select.options.length !== USAGE_PROVIDERS.length) {
    select.replaceChildren(...USAGE_PROVIDERS.map((provider) => {
      const option = document.createElement("option");
      option.value = provider.id;
      option.textContent = provider.name;
      return option;
    }));
  }
  select.value = selected.id;
  const { usage, fetching } = selected.read();
  paintUsageRows(el("settings-usage-body"), usage, fetching);
  paintUsageUpdated(el("settings-usage-foot"), usage);
  el("settings-usage-refresh").disabled = fetching;
}

/* ── The token ledger ──────────────────────────────────────────────────────
 *
 * A different question from the card above it. The plan-limit rows say how
 * much of a subscription window is spent; these say what the tokens actually
 * were, counted off the transcripts Claude Code writes to disk. The two are
 * never mixed: a percentage drawn under a token label is the one mistake in
 * this pane that a person cannot detect by looking.
 *
 * Orca draws the same anatomy — `StatsPane` → `ClaudeUsagePane` → eight stat
 * cards, a stacked day chart, by-model and by-project lists, a recent-session
 * table. The backend answers for one scope and one range at a time so the
 * five panels can never disagree about which days they are describing.
 */

const USAGE_STATS_SCOPES = [
  { value: "zerocode", label: () => t("stats.scopeOurs", "이 앱의 워크트리만") },
  { value: "all", label: () => t("stats.scopeAll", "이 컴퓨터의 모든 Claude 사용량") },
];

const USAGE_STATS_RANGES = [
  { value: "7d", label: () => t("stats.range7d", "최근 7일") },
  { value: "30d", label: () => t("stats.range30d", "최근 30일") },
  { value: "90d", label: () => t("stats.range90d", "최근 90일") },
  { value: "all", label: () => t("stats.rangeAll", "전체 기간") },
];

/* The two ledgers this pane can draw.
 *
 * A table rather than two copies of the pane: the scope and range controls,
 * the chart, the two breakdown lists and the session table are the same
 * anatomy against different counters, and the moment they are written twice is
 * the moment one of them stops getting the next fix. What differs is named
 * here — the command, and the cards each provider's numbers make sense as. */
const USAGE_STATS_PROVIDERS = [
  // The overview has no command of its own: it is DERIVED from the others,
  // the same way the original builds it in the renderer rather than asking
  // the backend a third question (`buildUsageOverview`).
  { id: "overview", name: () => t("stats.overview", "전체"), command: null },
  {
    id: "claude",
    name: () => "Claude",
    command: "claude_usage_stats",
    // What the original puts in a breakdown row's token column
    // (`ClaudeUsageDetails.tsx:39`): input plus output, cache NOT counted —
    // the cache figures have their own cards, and adding them here would
    // make one model's row dwarf every other line on the pane.
    rowTokens: (row) => row.input_tokens + row.output_tokens,
    rowActivity: (row) => t("stats.turnsCount", "턴 {{n}}회", { n: row.turns }),
    face: () => FOUR_COUNTER_FACE,
    costLabel: () => t("stats.estCost", "API 환산 비용"),
    // The share card's tile is 136px wide, which the pane's full label does
    // not fit — the original shortens it there for the same reason
    // (`ShareUsageCard.tsx`: "Est. cost" beside the pane's own longer word).
    costShort: () => t("stats.estCostShort", "환산 비용"),
    note: () => t(
      "stats.reuseAbout",
      "캐시 재사용률은 캐시 읽기 ÷ (입력 + 캐시 읽기)입니다. 구독제에서는 요금이 청구되지 않으며, 비용은 API 기준 환산값입니다.",
    ),
    // Four independent counters: the cache is read plus write and the input
    // is all new, because nothing here is a share of anything else.
    overview: {
      activityLabel: "turns",
      activity: (summary) => summary.turns ?? 0,
      total: (summary) =>
        summary.input_tokens + summary.output_tokens + summary.cache_read_tokens + summary.cache_write_tokens,
      newInput: (summary) => summary.input_tokens,
      cache: (summary) => summary.cache_read_tokens + summary.cache_write_tokens,
      reasoning: () => 0,
      dayTotal: (day) =>
        day.input_tokens + day.output_tokens + day.cache_read_tokens + day.cache_write_tokens,
    },
  },
  {
    id: "codex",
    name: () => "Codex",
    command: "codex_usage_stats",
    // Codex reports its own total and the original uses it
    // (`CodexUsageDetails.tsx:39`) — that figure carries the reasoning
    // output, which input plus output does not.
    rowTokens: (row) => row.total_tokens,
    rowActivity: (row) => t("stats.eventsCount", "이벤트 {{n}}건", { n: row.events }),
    face: () => FIVE_COUNTER_FACE,
    costLabel: () => t("stats.estCost", "API 환산 비용"),
    costShort: () => t("stats.estCostShort", "환산 비용"),
    note: () => t(
      "stats.codexAbout",
      "캐시된 입력은 입력의 일부이며 캐시 요금으로 한 번만 계산됩니다. 추론 출력은 출력에 이미 포함되어 있어 따로 청구되지 않고, 여기서는 참고용으로만 보입니다.",
    ),
    // Cached input is a SUBSET of input here, so the new input is the
    // remainder — adding them the other way counts the same tokens twice and
    // makes Codex look twice the size it is.
    overview: {
      activityLabel: "events",
      activity: (summary) => summary.events ?? 0,
      total: (summary) => summary.total_tokens,
      newInput: (summary) => Math.max(summary.input_tokens - summary.cached_input_tokens, 0),
      cache: (summary) => summary.cached_input_tokens,
      reasoning: (summary) => summary.reasoning_output_tokens,
      dayTotal: (day) => day.total_tokens,
    },
  },
  {
    id: "opencode",
    name: () => "OpenCode",
    command: "opencode_usage_stats",
    rowTokens: (row) => row.total_tokens,
    rowActivity: (row) => t("stats.eventsCount", "이벤트 {{n}}건", { n: row.events }),
    face: () => FIVE_COUNTER_FACE,
    // OpenCode reports what each turn cost, so this figure is the vendor's own
    // rather than a price table's guess — and on a subscription it reports
    // nothing, which the card shows as "해당 없음" rather than as free.
    costLabel: () => t("stats.reportedCost", "보고된 비용"),
    costShort: () => t("stats.reportedCost", "보고된 비용"),
    note: () => t(
      "stats.opencodeAbout",
      "캐시 읽기는 입력과 별개의 카운터이며, OpenCode가 보고한 합계에 이미 포함되어 있습니다. 비용은 OpenCode가 턴마다 기록한 값이며, 구독제에서는 비어 있습니다.",
    ),
    // The five-counter shape again, but this vendor's cache is its own
    // counter BESIDE the input rather than a share of it: every one of the
    // 5224 rows on the machine this was read from satisfies
    // `total = input + output + reasoning + cache read`. So the new input is
    // the whole input, and subtracting the cache from it — which is what the
    // Codex normalisation above does — would report less input than was sent.
    overview: {
      activityLabel: "events",
      activity: (summary) => summary.events ?? 0,
      total: (summary) => summary.total_tokens,
      newInput: (summary) => summary.input_tokens,
      cache: (summary) => summary.cached_input_tokens,
      reasoning: (summary) => summary.reasoning_output_tokens,
      dayTotal: (day) => day.total_tokens,
    },
  },
];

/* The providers with a ledger of their own, which is every row but the
 * overview — the one the overview is built FROM. */
const USAGE_STATS_LEDGERS = USAGE_STATS_PROVIDERS.filter((one) => one.command !== null);

/* ---- the two counter shapes these panes draw -----------------------------
 *
 * Claude reports four independent counters (input, output, cache read, cache
 * write) and a reuse rate. Codex and OpenCode report five (input, its cached
 * part, output, its reasoning part, and a total the vendor computed itself).
 * Everything that differs between the two panes — which cards, which bands in
 * the bar, which columns in the table — is one of those two facts, so it is
 * written twice rather than once per provider. A third five-counter vendor is
 * a row in the table above and nothing here.
 *
 * What each vendor says ABOUT its counters is its own, though, so the note and
 * the cost label stay on the provider's row: Codex's cached input is a share
 * of its input and OpenCode's is a counter beside it, and one sentence cannot
 * be true of both. */
const FIVE_COUNTER_FACE = {
  skeletonCards: 6,
  dailyAbout: () => t("stats.dailyAboutCodex", "날짜별 입력·캐시된 입력·출력·추론 합계."),
  // The original stacks the FULL input and the cached part beside it, which
  // overlaps for Codex — the heading says these are the four totals reported
  // by day, not a partition of a whole. Subtracting to make the bar add up
  // would draw a sky band a fraction of the original's.
  segments: () => [
    { key: "input", label: t("stats.input", "입력"), of: (day) => day.input_tokens },
    { key: "output", label: t("stats.output", "출력"), of: (day) => day.output_tokens },
    { key: "cache-read", label: t("stats.cachedInput", "캐시된 입력"), of: (day) => day.cached_input_tokens },
    { key: "cache-write", label: t("stats.reasoning", "추론"), of: (day) => day.reasoning_output_tokens },
  ],
  legendReversed: false,
  cards: (summary, provider) => [
    usageStatsCard(t("stats.inputTokens", "입력 토큰"), formatTokens(summary.input_tokens), "sparkles"),
    usageStatsCard(t("stats.outputTokens", "출력 토큰"), formatTokens(summary.output_tokens), "activity"),
    usageStatsCard(t("stats.cachedInput", "캐시된 입력"), formatTokens(summary.cached_input_tokens), "database-zap"),
    usageStatsCard(t("stats.reasoningOutput", "추론 출력"), formatTokens(summary.reasoning_output_tokens), "brain"),
    usageStatsCard(
      t("stats.sessionsEvents", "세션 / 이벤트"),
      `${summary.sessions.toLocaleString()} / ${summary.events.toLocaleString()}`,
      "folder-kanban",
    ),
    usageStatsCard(provider.costLabel(), formatCost(summary.estimated_cost_usd), "coins"),
  ],
  // No reuse rate to report — the cached count is beside or inside the input
  // depending on the vendor, and neither is a read-versus-write ratio. The
  // line says the share it does have rather than an empty figure under a
  // borrowed label.
  sessionAbout: (summary) => {
    const share = summary.input_tokens > 0
      ? `${Math.round((summary.cached_input_tokens / summary.input_tokens) * 100)}%`
      : t("stats.na", "해당 없음");
    return t("stats.cachedShareIs", "입력 중 캐시된 비율: {{rate}}", { rate: share });
  },
  // The table ends with the vendor's own total, which is what both of these
  // panes end with upstream (`CodexUsageDetails.tsx:74`,
  // `OpenCodeUsageDetails.tsx:74`). Not the cached share: for one of these two
  // that count is inside the input and for the other it is beside it, so the
  // same column would mean two different things.
  sessionColumns: () => [
    { label: t("stats.events", "이벤트"), of: (row) => String(row.events) },
    { label: t("stats.input", "입력"), of: (row) => formatTokens(row.input_tokens) },
    { label: t("stats.output", "출력"), of: (row) => formatTokens(row.output_tokens) },
    { label: t("stats.total", "합계"), of: (row) => formatTokens(row.total_tokens) },
  ],
};

const FOUR_COUNTER_FACE = {
  skeletonCards: 8,
  dailyAbout: () => t("stats.dailyAbout", "날짜별 입력·출력·캐시 읽기·캐시 쓰기 합계."),
  segments: () => [
    { key: "cache-write", label: t("stats.cacheWrite", "캐시 쓰기"), of: (day) => day.cache_write_tokens },
    { key: "cache-read", label: t("stats.cacheRead", "캐시 읽기"), of: (day) => day.cache_read_tokens },
    { key: "output", label: t("stats.output", "출력"), of: (day) => day.output_tokens },
    { key: "input", label: t("stats.input", "입력"), of: (day) => day.input_tokens },
  ],
  legendReversed: true,
  cards: (summary, provider) => {
    const turnShare = summary.turns > 0
      ? `${Math.round((summary.zero_cache_read_turns / summary.turns) * 100)}%`
      : t("stats.na", "해당 없음");
    return [
      usageStatsCard(t("stats.inputTokens", "입력 토큰"), formatTokens(summary.input_tokens), "sparkles"),
      usageStatsCard(t("stats.outputTokens", "출력 토큰"), formatTokens(summary.output_tokens), "activity"),
      usageStatsCard(t("stats.cacheRead", "캐시 읽기"), formatTokens(summary.cache_read_tokens), "database-zap"),
      usageStatsCard(t("stats.cacheWrite", "캐시 쓰기"), formatTokens(summary.cache_write_tokens), "waypoints"),
      usageStatsCard(t("stats.cacheReuse", "캐시 재사용률"), formatPercent(summary.cache_reuse_rate), "gauge"),
      usageStatsCard(t("stats.coldTurns", "캐시 없이 시작한 턴"), turnShare, "database-zap"),
      usageStatsCard(
        t("stats.sessionsTurns", "세션 / 턴"),
        `${summary.sessions.toLocaleString()} / ${summary.turns.toLocaleString()}`,
        "folder-kanban",
      ),
      usageStatsCard(provider.costLabel(), formatCost(summary.estimated_cost_usd), "coins"),
    ];
  },
  sessionAbout: (summary) =>
    t("stats.reuseIs", "캐시 재사용률: {{rate}}", { rate: formatPercent(summary.cache_reuse_rate) }),
  sessionColumns: () => [
    { label: t("stats.turns", "턴"), of: (row) => String(row.turns) },
    { label: t("stats.input", "입력"), of: (row) => formatTokens(row.input_tokens) },
    { label: t("stats.output", "출력"), of: (row) => formatTokens(row.output_tokens) },
    {
      label: t("stats.cache", "캐시"),
      of: (row) => formatTokens(row.cache_read_tokens + row.cache_write_tokens),
    },
  ],
};

/* How many days the intensity grid shows — the original's own forty-two, six
 * weeks, which is what makes the grid read as weeks rather than as a strip. */
const USAGE_OVERVIEW_DAYS = 42;

let usageStatsProvider = "overview";
let usageStatsScope = "all";
let usageStatsRange = "30d";
/* Held per provider rather than as one slot: the overview needs ALL of them at
 * once, and a single slot would make it redraw whichever answer arrived last.
 * Keyed off the table, so a ledger added there is held here. */
const usageStatsReports = Object.fromEntries(USAGE_STATS_LEDGERS.map((one) => [one.id, null]));
let usageStatsScanning = false;
/* Providers whose ledger this window does not read. The OFF list, mirroring
 * the setting: reading is the default, so an empty set is "everything on". */
const usageAnalyticsOff = new Set();
let usageStatsMeta = null;
let usageStatsTimer = null;

function usageStatsProviderOf(id) {
  return USAGE_STATS_PROVIDERS.find((one) => one.id === id) ?? USAGE_STATS_PROVIDERS[0];
}

/* Orca's `formatTokens`, with one step ADDED on purpose.
 *
 * The original stops at `M`, which was written before prompt caching made a
 * cache-read column routinely reach ten figures: this machine's own ledger
 * renders as `14504.6M` there, and a four-digit mantissa in front of a unit
 * reads as a typo rather than as fourteen billion. The step to `B` is the
 * same rule the other two already follow — one decimal, next unit at a
 * thousand of the last — so nothing here is a new convention, only the one
 * the original would have needed had it been written after caching. */
function formatTokens(value) {
  const count = Number(value) || 0;
  if (count >= 1_000_000_000) return `${(count / 1_000_000_000).toFixed(1)}B`;
  if (count >= 1_000_000) return `${(count / 1_000_000).toFixed(1)}M`;
  if (count >= 1_000) return `${(count / 1_000).toFixed(1)}k`;
  return count.toLocaleString();
}

/* Orca's `formatCost`: four decimals below a cent, so a cheap day reads as a
 * number rather than as `$0.00`. */
function formatCost(value) {
  if (value === null || value === undefined) return t("stats.na", "해당 없음");
  return value < 0.01 ? `$${value.toFixed(4)}` : `$${value.toFixed(2)}`;
}

function formatPercent(rate) {
  if (rate === null || rate === undefined) return t("stats.na", "해당 없음");
  return `${Math.round(rate * 100)}%`;
}

/* The minutes the window is east of UTC, which is what the backend needs to
 * decide which midnight a turn falls on. `getTimezoneOffset` counts the other
 * way, so the sign is flipped here rather than in four places there. */
function windowUtcOffsetMinutes() {
  return -new Date().getTimezoneOffset();
}

/* Turns one provider's ledger on or off.
 *
 * One function for the two hands that do it — the header switch and the
 * overview row's way back — because they are the same act and a second copy
 * would be the one that forgets to start the read again. */
function setUsageAnalyticsEnabled(id, enabled) {
  if (enabled) usageAnalyticsOff.delete(id);
  else usageAnalyticsOff.add(id);
  usageStatsMeta = null;
  clearTimeout(usageStatsTimer);
  paintUsageStats();
  // Turning it back on starts the read the switch was holding back.
  if (enabled) void refreshUsageStats(false);
  void commitSetting("usage_analytics_off", "set_usage_analytics_enabled", {
    provider: id,
    enabled,
  });
}

/* The pane a switched-off provider shows.
 *
 * `UsageTrackingPaneShell` with `enabled={false}`: the title stays, and in
 * place of everything else there is one sentence saying what turning it on
 * would read. The switch itself is in the header, where it is when on — a
 * control that moves when you use it is a control you have to find twice. */
function usageStatsDisabled(provider) {
  const box = document.createElement("div");
  box.className = "usage-empty usage-off";
  box.textContent = t(
    "stats.offAbout",
    "{{provider}}의 로컬 사용 기록을 읽어 토큰·모델·세션 통계를 보여 줍니다.",
    { provider: provider.name() },
  );
  return box;
}

/* ---- the three counted figures at the head of the pane ------------------
 *
 * `StatsPane.tsx:111-150`. Counted rather than derived, so they come from
 * their own door and are held here between openings — the pane is opened far
 * more often than these numbers change. */
/* Scans finish out of band; two seconds keeps both usage surfaces responsive
 * without turning a running backend scan into a busy poll. */
const USAGE_STATS_RETRY_MS = 2000;
let statsCounters = null;

async function refreshStatsHead() {
  try {
    statsCounters = await invoke("stats_summary");
  } catch (error) {
    showError(error);
    return;
  }
  paintStatsHead();
}

/* How long agents worked, in the original's own words (`formatDuration`):
 * hours and minutes once there is an hour, minutes under that, and seconds
 * only while there is not yet a minute — a figure that reads "0분" for a
 * morning's work says the counter is broken. */
function statsWorkedWords(ms) {
  const seconds = Math.floor(Math.max(ms, 0) / 1000);
  if (seconds < 60) return t("stats.seconds", "{{n}}초", { n: seconds });
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return t("stats.minutes", "{{n}}분", { n: minutes });
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest === 0
    ? t("stats.hours", "{{n}}시간", { n: hours })
    : t("stats.hoursMinutes", "{{h}}시간 {{m}}분", { h: hours, m: rest });
}

function paintStatsHead() {
  const host = el("stats-head");
  host.replaceChildren();
  if (!statsCounters) return;
  // The original's own emptiness test: what a PERSON did, not elapsed time.
  if (statsCounters.agents_spawned === 0 && statsCounters.prs_created === 0) {
    host.appendChild(
      usageStatsEmpty(t("stats.headEmpty", "첫 에이전트를 시작하면 여기서부터 셉니다")),
    );
    return;
  }
  const cards = document.createElement("div");
  cards.className = "usage-stat-cards stats-head-cards";
  cards.append(
    usageStatsCard(
      t("stats.agentsSpawned", "시작한 에이전트"),
      statsCounters.agents_spawned.toLocaleString(),
      "bot",
    ),
    usageStatsCard(
      t("stats.agentTime", "에이전트가 일한 시간"),
      statsWorkedWords(statsCounters.agent_time_ms),
      "clock",
    ),
    usageStatsCard(
      t("stats.prsCreated", "만든 PR"),
      statsCounters.prs_created.toLocaleString(),
      "pr",
    ),
  );
  host.appendChild(cards);
  if (statsCounters.first_event_at_ms) {
    const since = document.createElement("p");
    since.className = "usage-stats-note stats-head-since";
    const when = new Date(statsCounters.first_event_at_ms).toLocaleDateString(undefined, {
      year: "numeric",
      month: "short",
      day: "numeric",
    });
    since.textContent = t("stats.trackingSince", "{{when}}부터 세고 있습니다", { when });
    host.appendChild(since);
  }
}

/* Nothing to show, said as a shape rather than a sentence in the margin.
 *
 * The original's own empty (`UsageTrackingPaneShell.tsx:152-154`): a dashed
 * box where the cards would be. A dashed rule is the grammar for "this frame
 * is real, its content is not yet", which a bare line of grey text is not. */
function usageStatsEmpty(words) {
  const box = document.createElement("div");
  box.className = "usage-empty";
  box.textContent = words;
  return box;
}

/* The shape the answer will take, while it is being read.
 *
 * `ClaudeUsageLoadingState`: the same card grid and the same ten-column chart,
 * drawn as pulsing blanks. The counts and the class names are the real ones,
 * so nothing on this pane moves when the numbers land. */
function usageStatsSkeleton(cards) {
  const host = document.createElement("div");
  host.className = "usage-skeleton";
  host.setAttribute("aria-hidden", "true");

  const grid = document.createElement("div");
  grid.className = "usage-stat-cards";
  for (let at = 0; at < cards; at += 1) {
    const card = document.createElement("div");
    card.className = "usage-stat-card";
    const tile = document.createElement("span");
    tile.className = "usage-stat-ico usage-blank";
    const block = document.createElement("div");
    block.className = "usage-stat-block";
    const figure = document.createElement("span");
    figure.className = "usage-blank usage-blank-figure";
    const label = document.createElement("span");
    label.className = "usage-blank usage-blank-label";
    block.append(figure, label);
    card.append(tile, block);
    grid.appendChild(card);
  }

  const chart = document.createElement("div");
  chart.className = "usage-chart";
  for (let at = 0; at < 10; at += 1) {
    const column = document.createElement("div");
    column.className = "usage-chart-column";
    const sum = document.createElement("span");
    sum.className = "usage-blank usage-blank-total";
    const bed = document.createElement("div");
    bed.className = "usage-chart-bed";
    const stack = document.createElement("div");
    stack.className = "usage-chart-stack usage-blank";
    // Uneven heights, the original's own stagger — ten equal blanks read as a
    // chart OF nothing rather than as a chart still being read.
    stack.style.height = `${35 + ((at % 5) + 1) * 10}%`;
    bed.appendChild(stack);
    const when = document.createElement("span");
    when.className = "usage-blank usage-blank-day";
    column.append(sum, bed, when);
    chart.appendChild(column);
  }
  host.append(grid, chart);
  return host;
}

/* One figure, as the original's `StatCard` builds it: a square tile holding
 * the figure's own face, then the number over the word it answers
 * (`StatCard.tsx` — size-9 muted tile, `text-lg font-semibold` value, `text-xs
 * muted` label). The face is not decoration: eight cards of the same size read
 * as a wall of numbers, and the glyph is what lets a person find the one they
 * came for without reading all eight. */
function usageStatsCard(label, value, glyph, hint) {
  const card = document.createElement("div");
  card.className = "usage-stat-card";
  const tile = document.createElement("span");
  tile.className = "usage-stat-ico";
  tile.setAttribute("aria-hidden", "true");
  tile.innerHTML = icon(glyph);
  const block = document.createElement("div");
  block.className = "usage-stat-block";
  const figure = document.createElement("p");
  figure.className = "usage-stat-figure";
  figure.textContent = value;
  const name = document.createElement("p");
  name.className = "usage-stat-label";
  name.textContent = label;
  block.append(figure, name);
  card.append(tile, block);
  if (hint) card.dataset.tip = hint;
  return card;
}

/* The stacked day chart. Ten bars, newest last, each split into the four
 * counters — Orca's `ClaudeUsageDailyChart`, whose height is a share of the
 * heaviest day rather than of its own total, so the bars are comparable. */
function usageStatsChart(standing, daily) {
  const face = standing.face();
  const section = document.createElement("section");
  section.className = "usage-stats-section";
  const head = document.createElement("div");
  head.className = "usage-stats-section-head";
  const title = document.createElement("h4");
  title.textContent = t("stats.daily", "일별 사용량");
  const about = document.createElement("p");
  about.textContent = face.dailyAbout();
  head.append(title, about);
  section.appendChild(head);

  const shown = daily.slice(-10);
  // The vendor's own counters, in the original's stacking order
  // (`ClaudeUsageDailyChart.tsx:53-81`, `CodexUsageDailyChart.tsx:47-76`) —
  // the first entry renders at the top of the bar.
  const segments = face.segments();
  const total = (day) => segments.reduce((sum, segment) => sum + segment.of(day), 0);
  let heaviest = 1;
  for (const day of shown) heaviest = Math.max(heaviest, total(day));

  const chart = document.createElement("div");
  chart.className = "usage-chart";
  for (const day of shown) {
    const column = document.createElement("div");
    column.className = "usage-chart-column";
    const sum = document.createElement("span");
    sum.className = "usage-chart-total";
    sum.textContent = formatTokens(total(day));
    // The bar sits in a centring bed rather than filling the column: the
    // original caps it at 48px (`max-w-12`), so a ten-day chart draws bars
    // rather than ten slabs touching each other.
    const bed = document.createElement("div");
    bed.className = "usage-chart-bed";
    const stack = document.createElement("div");
    stack.className = "usage-chart-stack";
    for (const segment of segments) {
      const value = segment.of(day);
      if (value <= 0) continue;
      const piece = document.createElement("div");
      piece.className = "usage-chart-piece";
      piece.dataset.segment = segment.key;
      piece.style.height = `${(value / heaviest) * 100}%`;
      const spoken = value.toLocaleString();
      piece.dataset.tip = `${day.day} · ${segment.label}: ${spoken}`;
      stack.appendChild(piece);
    }
    bed.appendChild(stack);
    const when = document.createElement("span");
    when.className = "usage-chart-day";
    when.textContent = day.day.slice(5);
    column.append(sum, bed, when);
    chart.appendChild(column);
  }
  section.appendChild(chart);

  const legend = document.createElement("div");
  legend.className = "usage-chart-legend";
  const listed = face.legendReversed ? [...segments].reverse() : segments;
  for (const segment of listed) {
    const item = document.createElement("span");
    item.className = "usage-chart-legend-item";
    const dot = document.createElement("span");
    dot.className = "usage-chart-dot";
    dot.dataset.segment = segment.key;
    item.append(dot, document.createTextNode(segment.label));
    legend.appendChild(item);
  }
  section.appendChild(legend);
  return section;
}

/* One of the two lists under the chart — the five heaviest rows, each with
 * its share written the way the original writes it. */
/* One breakdown section.
 *
 * WHAT a row's token figure counts belongs to the provider, not to this
 * function: the two ledgers count differently and the original does the same
 * mapping at each of its call sites. The section draws rows; the provider
 * says how to read one. */
function usageStatsBreakdown(standing, title, topLabel, topValue, rows) {
  const section = document.createElement("section");
  section.className = "usage-stats-section";
  const head = document.createElement("div");
  head.className = "usage-stats-section-head";
  const heading = document.createElement("h4");
  heading.textContent = title;
  const top = document.createElement("p");
  const none = t("stats.na", "해당 없음");
  top.textContent = `${topLabel} ${topValue ?? none}`;
  head.append(heading, top);
  section.appendChild(head);

  const list = document.createElement("div");
  list.className = "usage-breakdown";
  for (const row of rows.slice(0, 5)) {
    const line = document.createElement("div");
    line.className = "usage-breakdown-row";
    const top = document.createElement("div");
    top.className = "usage-breakdown-top";
    const label = document.createElement("span");
    label.className = "usage-breakdown-label";
    label.textContent = row.label;
    const tokens = document.createElement("span");
    tokens.className = "usage-breakdown-tokens";
    tokens.textContent = formatTokens(standing.rowTokens(row));
    top.append(label, tokens);
    const foot = document.createElement("div");
    foot.className = "usage-breakdown-foot";
    const sessions = t("stats.sessionsCount", "세션 {{n}}개", { n: row.sessions });
    const turns = standing.rowActivity(row);
    const priced = row.estimated_cost_usd === null || row.estimated_cost_usd === undefined
      ? ""
      : ` · ${formatCost(row.estimated_cost_usd)}`;
    foot.textContent = `${sessions} · ${turns}${priced}`;
    line.append(top, foot);
    list.appendChild(line);
  }
  section.appendChild(list);
  return section;
}

/* The recent-session table. Orca shows twelve, and so does the backend. */
function usageStatsSessions(standing, rows, summary) {
  const face = standing.face();
  const columns = face.sessionColumns();
  const section = document.createElement("section");
  section.className = "usage-stats-section";
  const head = document.createElement("div");
  head.className = "usage-stats-section-head";
  const heading = document.createElement("h4");
  heading.textContent = t("stats.recent", "최근 세션");
  const about = document.createElement("p");
  about.textContent = face.sessionAbout(summary);
  head.append(heading, about);
  section.appendChild(head);

  const scroll = document.createElement("div");
  scroll.className = "usage-table-scroll";
  const table = document.createElement("table");
  table.className = "usage-table";
  const headings = [
    t("stats.lastActive", "마지막 활동"),
    t("stats.project", "프로젝트"),
    t("stats.model", "모델"),
    ...columns.map((column) => column.label),
  ];
  const thead = document.createElement("thead");
  const headRow = document.createElement("tr");
  for (const heading of headings) {
    const cell = document.createElement("th");
    cell.textContent = heading;
    headRow.appendChild(cell);
  }
  thead.appendChild(headRow);
  const body = document.createElement("tbody");
  for (const row of rows) {
    const line = document.createElement("tr");
    const when = new Date(row.last_active_at);
    const spokenWhen = Number.isNaN(when.getTime())
      ? row.last_active_at
      : when.toLocaleString(undefined, {
          month: "short",
          day: "numeric",
          hour: "numeric",
          minute: "2-digit",
        });
    const unknown = t("stats.unknownModel", "알 수 없음");
    for (const value of [
      spokenWhen,
      row.project_label,
      row.model ?? unknown,
      ...columns.map((column) => column.of(row)),
    ]) {
      const cell = document.createElement("td");
      cell.textContent = value;
      line.appendChild(cell);
    }
    if (row.branch) line.dataset.tip = row.branch;
    body.appendChild(line);
  }
  table.append(thead, body);
  scroll.appendChild(table);
  section.appendChild(scroll);
  return section;
}

/* ── the overview ──────────────────────────────────────────────────────────
 *
 * Derived, never asked for: the two ledgers already answered, and a third
 * backend question would be a third chance to describe a different set of
 * days. Ported from Orca's `buildUsageOverview` + `usage-provider-
 * normalization` + `usage-overview-daily-series`.
 *
 * The normalisation is the part that matters, and it belongs to the PROVIDER
 * rather than to this function: Claude reports four independent counters,
 * Codex reports cached input as a subset of its input, OpenCode reports the
 * cache as its own counter beside the input. Each of those rules lives on its
 * own row of the table above, so a fourth ledger is a row rather than another
 * branch here — and no row can be normalised by the wrong vendor's rule.
 */
function overviewProviderOf(id, report) {
  const provider = usageStatsProviderOf(id);
  const shape = provider.overview;
  const summary = report?.summary ?? null;
  const figure = (read) => (summary ? read(summary) || 0 : 0);
  return {
    id,
    label: provider.name(),
    enabled: !usageAnalyticsOff.has(id),
    hasData: Boolean(summary?.has_any_data),
    sessions: summary?.sessions ?? 0,
    activityLabel: shape.activityLabel,
    activityCount: figure(shape.activity),
    totalTokens: figure(shape.total),
    newInputTokens: figure(shape.newInput),
    outputTokens: summary?.output_tokens ?? 0,
    cacheTokens: figure(shape.cache),
    reasoningTokens: figure(shape.reasoning),
    estimatedCostUsd: summary?.estimated_cost_usd ?? null,
    topModel: summary?.top_model ?? null,
    topProject: summary?.top_project ?? null,
    // The total the VENDOR reported, never the counters added up: where a
    // counter is a share of another one, adding them counts the same tokens
    // twice (`buildDailyOverview` uses `entry.totalTokens` for this reason).
    dailyTotals: (report?.daily ?? []).map((day) => ({
      day: day.day,
      total: shape.dayTotal(day) || 0,
    })),
  };
}

/* Five buckets against the heaviest day, so a quiet day is visible as one
 * step rather than as nothing. */
function overviewIntensity(total, heaviest) {
  if (total <= 0 || heaviest <= 0) return 0;
  const ratio = total / heaviest;
  if (ratio <= 0.25) return 1;
  if (ratio <= 0.5) return 2;
  if (ratio <= 0.75) return 3;
  return 4;
}

function buildUsageOverview(providers) {
  const byDay = new Map();
  for (const provider of providers) {
    for (const entry of provider.dailyTotals) {
      const held = byDay.get(entry.day) ?? { day: entry.day, total: 0, byProvider: {} };
      held.total += entry.total;
      held.byProvider[provider.id] = (held.byProvider[provider.id] ?? 0) + entry.total;
      byDay.set(entry.day, held);
    }
  }
  let heaviest = 0;
  for (const day of byDay.values()) heaviest = Math.max(heaviest, day.total);
  const daily = [...byDay.values()]
    .sort((left, right) => left.day.localeCompare(right.day))
    .map((day) => ({ ...day, intensity: overviewIntensity(day.total, heaviest) }));

  const sum = (pick) => providers.reduce((total, one) => total + pick(one), 0);
  const newInputTokens = sum((one) => one.newInputTokens);
  const cacheTokens = sum((one) => one.cacheTokens);
  const knownCost = providers
    .filter((one) => one.estimatedCostUsd !== null)
    .reduce((total, one) => total + one.estimatedCostUsd, 0);
  return {
    providers,
    dataProviderCount: providers.filter((one) => one.hasData).length,
    hasAnyData: providers.some((one) => one.hasData),
    totalTokens: sum((one) => one.totalTokens),
    newInputTokens,
    outputTokens: sum((one) => one.outputTokens),
    cacheTokens,
    reasoningTokens: sum((one) => one.reasoningTokens),
    sessions: sum((one) => one.sessions),
    // A day counts as active for the WHOLE overview when anything ran on it.
    activeDays: daily.filter((day) => day.total > 0).length,
    estimatedCostUsd: providers.some((one) => one.estimatedCostUsd !== null) ? knownCost : null,
    // True when a provider has data but no price — the cost shown is then a
    // floor rather than a total, and the pane says so.
    hasPartialCost: providers.some((one) => one.hasData && one.estimatedCostUsd === null),
    cacheShare: newInputTokens + cacheTokens > 0 ? cacheTokens / (newInputTokens + cacheTokens) : null,
    daily,
    bestDay: daily.reduce((best, day) => (!best || day.total > best.total ? day : best), null),
  };
}

/* The last N calendar days ending today, including the ones nothing ran on —
 * a grid that skips empty days is a grid whose columns are not dates. */
function overviewRecentDays(daily, count) {
  const held = new Map(daily.map((day) => [day.day, day]));
  const end = new Date();
  end.setHours(0, 0, 0, 0);
  const days = [];
  for (let back = count - 1; back >= 0; back--) {
    const at = new Date(end);
    at.setDate(end.getDate() - back);
    const key = `${at.getFullYear()}-${String(at.getMonth() + 1).padStart(2, "0")}-${String(at.getDate()).padStart(2, "0")}`;
    days.push(held.get(key) ?? { day: key, total: 0, byProvider: {}, intensity: 0 });
  }
  return days;
}

function overviewDayLabel(day) {
  const parsed = new Date(`${day}T12:00:00`);
  if (Number.isNaN(parsed.getTime())) return day;
  return parsed.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

function overviewIntensityGrid(days, bestDay) {
  const section = document.createElement("section");
  section.className = "usage-stats-section";
  const head = document.createElement("div");
  head.className = "usage-stats-section-head";
  const heading = document.createElement("h4");
  heading.textContent = t("stats.intensity", "일별 밀도");
  const about = document.createElement("p");
  about.textContent = t("stats.intensityAbout", "최근 6주 동안 두 에이전트가 함께 쓴 토큰.");
  head.append(heading, about);
  if (bestDay && bestDay.total > 0) {
    const best = document.createElement("span");
    best.className = "usage-overview-best";
    best.textContent = t("stats.bestDay", "최다 {{when}}", { when: overviewDayLabel(bestDay.day) });
    head.appendChild(best);
  }
  section.appendChild(head);

  const grid = document.createElement("div");
  grid.className = "usage-heatmap";
  grid.setAttribute("role", "img");
  grid.setAttribute("aria-label", t("stats.intensityAria", "최근 토큰 활동 히트맵"));
  for (const day of days) {
    const cell = document.createElement("span");
    cell.className = "usage-heatmap-cell";
    cell.dataset.intensity = String(day.intensity);
    cell.dataset.tip = `${day.day} · ${day.total.toLocaleString()}`;
    grid.appendChild(cell);
  }
  section.appendChild(grid);

  const foot = document.createElement("div");
  foot.className = "usage-heatmap-foot";
  const first = document.createElement("span");
  first.textContent = overviewDayLabel(days[0]?.day ?? "");
  const less = document.createElement("span");
  less.textContent = t("stats.less", "적음");
  const scale = document.createElement("span");
  scale.className = "usage-heatmap-scale";
  for (const step of [0, 1, 2, 3, 4]) {
    const mark = document.createElement("span");
    mark.className = "usage-heatmap-cell";
    mark.dataset.intensity = String(step);
    scale.appendChild(mark);
  }
  const more = document.createElement("span");
  more.textContent = t("stats.more", "많음");
  const last = document.createElement("span");
  last.textContent = overviewDayLabel(days.at(-1)?.day ?? "");
  foot.append(first, less, scale, more, last);
  section.appendChild(foot);
  return section;
}

function overviewTokenMix(overview) {
  const section = document.createElement("section");
  section.className = "usage-stats-section";
  const head = document.createElement("div");
  head.className = "usage-stats-section-head";
  const heading = document.createElement("h4");
  heading.textContent = t("stats.tokenMix", "토큰 구성");
  const about = document.createElement("p");
  about.textContent = t("stats.tokenMixAbout", "두 에이전트의 입력·출력·캐시를 합친 비율.");
  head.append(heading, about);
  if (overview.reasoningTokens > 0) {
    const badge = document.createElement("span");
    badge.className = "usage-overview-best";
    const spoken = formatTokens(overview.reasoningTokens);
    badge.textContent = t("stats.reasoningIs", "추론 {{tokens}}", { tokens: spoken });
    head.appendChild(badge);
  }
  section.appendChild(head);

  const segments = [
    { key: "new-input", label: t("stats.newInput", "새 입력"), value: overview.newInputTokens },
    { key: "output", label: t("stats.output", "출력"), value: overview.outputTokens },
    { key: "cache", label: t("stats.cache", "캐시"), value: overview.cacheTokens },
  ];
  const total = segments.reduce((sum, one) => sum + one.value, 0);
  const bar = document.createElement("div");
  bar.className = "usage-mix-bar";
  for (const segment of segments) {
    if (segment.value <= 0) continue;
    const piece = document.createElement("span");
    piece.className = "usage-mix-piece";
    piece.dataset.mix = segment.key;
    piece.style.width = `${(segment.value / total) * 100}%`;
    piece.dataset.tip = `${segment.label} · ${segment.value.toLocaleString()}`;
    bar.appendChild(piece);
  }
  section.appendChild(bar);

  const legend = document.createElement("div");
  legend.className = "usage-mix-legend";
  for (const segment of segments) {
    const item = document.createElement("span");
    item.className = "usage-chart-legend-item";
    const dot = document.createElement("span");
    dot.className = "usage-mix-dot";
    dot.dataset.mix = segment.key;
    item.append(dot, document.createTextNode(`${segment.label}: ${formatTokens(segment.value)}`));
    legend.appendChild(item);
  }
  section.appendChild(legend);
  return section;
}

/* One provider's line in the overview (`ProviderUsageRow`).
 *
 * Name and standing on top, what it last read underneath, then the three
 * figures and the share bar. The standing matters here more than anywhere
 * else on this pane: the overview is the ONLY place a switched-off provider
 * is still visible, so it is where saying so — and offering the way back —
 * belongs. */
function overviewProviderRow(provider, totalTokens) {
  const row = document.createElement("div");
  row.className = "usage-provider-row";

  const top = document.createElement("div");
  top.className = "usage-provider-top";
  const name = document.createElement("h5");
  name.className = "usage-provider-name";
  name.textContent = provider.label;
  const badge = document.createElement("span");
  badge.className = "usage-provider-badge";
  badge.dataset.standing = provider.enabled ? "on" : "off";
  badge.textContent = provider.enabled
    ? t("stats.standingOn", "켜짐")
    : t("stats.standingOff", "꺼짐");
  top.append(name, badge);
  if (!provider.enabled) {
    // The way back, where the person is looking at what they are missing.
    const turnOn = document.createElement("button");
    turnOn.type = "button";
    turnOn.className = "btn usage-provider-enable";
    turnOn.textContent = t("stats.turnOn", "켜기");
    turnOn.addEventListener("click", () => setUsageAnalyticsEnabled(provider.id, true));
    top.appendChild(turnOn);
  }
  row.appendChild(top);

  const said = document.createElement("p");
  said.className = "usage-provider-said";
  const model = provider.topModel ?? t("stats.noModelYet", "아직 모델 없음");
  said.textContent = provider.topProject ? `${model} · ${provider.topProject}` : model;
  row.appendChild(said);

  const foot = document.createElement("div");
  foot.className = "usage-provider-figures";
  const activity = provider.activityLabel === "turns"
    ? t("stats.turnsCount", "턴 {{n}}회", { n: provider.activityCount })
    : t("stats.eventsCount", "이벤트 {{n}}건", { n: provider.activityCount });
  for (const figure of [
    t("stats.tokensAre", "토큰 {{tokens}}", { tokens: formatTokens(provider.totalTokens) }),
    `${t("stats.sessionsCount", "세션 {{n}}개", { n: provider.sessions })} · ${activity}`,
    formatCost(provider.estimatedCostUsd),
  ]) {
    const cell = document.createElement("span");
    cell.textContent = figure;
    foot.appendChild(cell);
  }
  row.appendChild(foot);

  const share = document.createElement("div");
  share.className = "usage-row-bar";
  const fill = document.createElement("div");
  fill.className = "usage-row-fill";
  const portion = totalTokens > 0 ? (provider.totalTokens / totalTokens) * 100 : 0;
  // A provider that read something never draws an empty bar: the original
  // floors the fill at 2% so "almost nothing" still reads as "something".
  fill.style.width = `${provider.totalTokens > 0 ? Math.max(portion, 2) : 0}%`;
  share.appendChild(fill);
  row.appendChild(share);
  return row;
}

function overviewProviderRows(overview) {
  const section = document.createElement("section");
  section.className = "usage-stats-section";
  const head = document.createElement("div");
  head.className = "usage-stats-section-head";
  const heading = document.createElement("h4");
  heading.textContent = t("stats.providers", "제공자");
  const about = document.createElement("p");
  about.textContent = t("stats.providersWithData", "{{n}}곳에서 기록을 읽었습니다", {
    n: overview.dataProviderCount,
  });
  head.append(heading, about);
  section.appendChild(head);

  const list = document.createElement("div");
  list.className = "usage-provider-rows";
  for (const provider of overview.providers) {
    list.appendChild(overviewProviderRow(provider, overview.totalTokens));
  }
  section.appendChild(list);
  return section;
}

function paintUsageOverview(host) {
  const providers = USAGE_STATS_LEDGERS.map((one) =>
    overviewProviderOf(one.id, usageStatsReports[one.id]),
  );
  const overview = buildUsageOverview(providers);

  const cards = document.createElement("div");
  cards.className = "usage-stat-cards";
  cards.append(
    usageStatsCard(t("stats.totalTokens", "전체 토큰"), formatTokens(overview.totalTokens), "sparkles"),
    usageStatsCard(t("stats.estCost", "API 환산 비용"), formatCost(overview.estimatedCostUsd), "coins"),
    usageStatsCard(t("stats.activeDays", "활동한 날"), overview.activeDays.toLocaleString(), "calendar-days"),
    usageStatsCard(t("stats.cacheShare", "캐시 비중"), formatPercent(overview.cacheShare), "database-zap"),
  );
  host.appendChild(cards);

  if (overview.hasPartialCost) {
    const note = document.createElement("p");
    note.className = "usage-stats-note";
    note.textContent = t(
      "stats.partialCost",
      "가격표에 없는 모델이 섞여 있어, 비용은 알려진 모델만 더한 하한값입니다.",
    );
    host.appendChild(note);
  }

  if (!overview.hasAnyData) {
    const none = document.createElement("p");
    none.className = "usage-none";
    none.textContent = t("stats.noneForScope", "이 범위와 기간에는 기록된 사용량이 없습니다");
    host.appendChild(none);
    return;
  }

  const pair = document.createElement("div");
  pair.className = "usage-stats-pair";
  pair.append(
    overviewIntensityGrid(overviewRecentDays(overview.daily, USAGE_OVERVIEW_DAYS), overview.bestDay),
    overviewTokenMix(overview),
  );
  host.appendChild(pair);
  host.appendChild(overviewProviderRows(overview));
}

/* Scope and range live behind the filters button, as two radio groups — the
 * original's `UsageFilterRadioGroup` pair. They are set once and looked past,
 * so they do not earn a permanent seat in a header the provider needs. */
function openUsageStatsFilters(x, y, opener) {
  openSidebarMenu(x, y, [
    { caption: t("stats.scope", "범위") },
    {
      segment: USAGE_STATS_SCOPES.map((one) => ({ value: one.value, label: one.label() })),
      value: usageStatsScope,
      pick: (value) => {
        usageStatsScope = value;
        paintUsageStats();
        return refreshUsageStats(false);
      },
    },
    { caption: t("stats.range", "기간") },
    {
      segment: USAGE_STATS_RANGES.map((one) => ({ value: one.value, label: one.label() })),
      value: usageStatsRange,
      pick: (value) => {
        usageStatsRange = value;
        paintUsageStats();
        return refreshUsageStats(false);
      },
    },
  ], opener);
}

/* The face on the provider trigger, and on nothing else.
 *
 * Overview has no agent behind it, so it wears the original's own chart glyph
 * (`UsageAnalyticsOptionIcon`: BarChart3 for overview, the agent's mark
 * otherwise). Kept by what it was built from, because the registry can arrive
 * after the first paint and a letter tile that never became the mark is the
 * bug this shape exists to avoid. */
function paintUsageStatsFace(id, name) {
  const face = el("usage-stats-provider-face");
  const domain = id === "overview"
    ? ""
    : agentRows.find((one) => one.id === id)?.favicon_domain ?? "";
  const built = `${id}:${domain}`;
  if (face.dataset.face === built) return;
  face.dataset.face = built;
  if (id === "overview") {
    face.innerHTML = icon("chart");
    return;
  }
  face.replaceChildren(agentIcon({ id, name, favicon_domain: domain }));
}

/* The provider menu (`StatsPane.tsx:181-194`): the three options with the
 * standing one ticked, aligned to the trigger's end and as wide as the
 * original's `w-44`. It closes on a pick — this is a choice, not a filter. */
function openUsageStatsProviders(x, y, opener) {
  openSidebarMenu(x, y, USAGE_STATS_PROVIDERS.map((one) => ({
    label: one.id === usageStatsProvider ? `✓ ${one.name()}` : one.name(),
    run: () => pickUsageStatsProvider(one.id),
  })), opener);
}

function pickUsageStatsProvider(id) {
  const chosen = usageStatsProviderOf(id);
  if (chosen.id === usageStatsProvider) return;
  usageStatsProvider = chosen.id;
  // The held answers survive the switch — they are filed per provider, so
  // nothing can be redrawn under another one's labels — but the freshness
  // line belongs to the ask that produced it and is cleared until the next.
  usageStatsMeta = null;
  clearTimeout(usageStatsTimer);
  paintUsageStats();
  void refreshUsageStats(false);
}

function paintUsageStats() {
  const standing = usageStatsProviderOf(usageStatsProvider);
  const word = standing.name();
  el("usage-stats-provider-name").textContent = word;
  paintUsageStatsFace(standing.id, word);
  // The trigger says WHICH question it answers, not just its answer — the
  // word alone ("전체") is not a control's name.
  el("usage-stats-provider").setAttribute(
    "aria-label",
    `${t("stats.provider", "제공자")}: ${word}`,
  );
  // The switch belongs to a provider, so the overview — which is derived from
  // the others rather than read from a disk — does not carry one.
  const switching = el("usage-stats-switch");
  const single = standing.command !== null;
  const off = single && usageAnalyticsOff.has(standing.id);
  switching.hidden = !single;
  switching.setAttribute("aria-checked", off ? "false" : "true");
  switching.dataset.tip = t("stats.trackingSwitch", "{{provider}} 사용량 분석", {
    provider: standing.name(),
  });
  switching.setAttribute("aria-label", switching.dataset.tip);
  // A pane that is off has nothing to filter and nothing to re-read.
  el("usage-stats-filters").hidden = off;
  el("usage-stats-refresh").hidden = off;
  // And a card can only be shared once there is a figure on it. The overview
  // never carries one: it is derived from several reads, and a card claiming
  // to be one of them would be a fourth answer nobody asked for.
  el("usage-stats-share").hidden = shareUsageStanding() === null;
  el("usage-stats-refresh").disabled = usageStatsScanning;
  // What the filters are set to, spelled out under the header — otherwise a
  // menu that is shut is a question with no visible answer.
  const scopeWord = USAGE_STATS_SCOPES.find((one) => one.value === usageStatsScope)?.label() ?? "";
  const rangeWord = USAGE_STATS_RANGES.find((one) => one.value === usageStatsRange)?.label() ?? "";
  el("usage-stats-selection").textContent = `${scopeWord} · ${rangeWord}`;

  const status = el("usage-stats-status");
  if (usageStatsScanning) {
    status.textContent = t("stats.scanning", "전사 기록을 읽는 중…");
  } else if (usageStatsMeta?.error) {
    status.textContent = t("stats.readFailed", "읽지 못했습니다 — {{why}}", {
      why: usageStatsMeta.error,
    });
  } else if (usageStatsMeta?.scanned_at) {
    const when = new Date(usageStatsMeta.scanned_at).toLocaleString();
    const files = usageStatsMeta.files ?? 0;
    status.textContent = t("stats.scannedAt", "{{when}} · 전사 {{files}}개", { when, files });
    if (usageStatsMeta.capped) {
      status.textContent += ` · ${t("stats.capped", "상한에 걸려 일부만 읽었습니다")}`;
    }
  } else {
    status.textContent = t("stats.neverScanned", "아직 읽지 않았습니다");
  }

  const host = el("usage-stats-body");
  host.replaceChildren();
  if (off) {
    // No filters, no freshness: both are claims about a read that is not
    // happening. "아직 읽지 않았습니다" under a switched-off pane reads as a
    // scan that failed.
    el("usage-stats-selection").textContent = "";
    status.textContent = "";
    host.appendChild(usageStatsDisabled(standing));
    return;
  }
  if (usageStatsProvider === "overview") {
    if (!USAGE_STATS_LEDGERS.some((one) => usageStatsReports[one.id])) {
      host.appendChild(
        usageStatsScanning
          ? usageStatsSkeleton(4)
          : usageStatsEmpty(t("stats.noData", "읽은 사용량이 없습니다")),
      );
      return;
    }
    paintUsageOverview(host);
    return;
  }
  const report = usageStatsReports[usageStatsProvider];
  if (!report) {
    // A first scan reads every transcript on the disk, and a one-line "확인
    // 중…" under an empty card says nothing about how much is coming. The
    // original draws the shape it is about to fill (`ClaudeUsageLoadingState`)
    // so the pane does not jump when the answer lands.
    host.appendChild(
      usageStatsScanning
        ? usageStatsSkeleton(standing.face().skeletonCards)
        : usageStatsEmpty(t("stats.noData", "읽은 사용량이 없습니다")),
    );
    return;
  }
  const summary = report.summary;
  if (!summary.has_any_data) {
    host.appendChild(
      usageStatsEmpty(t("stats.noneForScope", "이 범위와 기간에는 기록된 사용량이 없습니다")),
    );
    return;
  }

  const cards = document.createElement("div");
  cards.className = "usage-stat-cards";
  cards.append(...standing.face().cards(summary, standing));
  const note = document.createElement("p");
  note.className = "usage-stats-note";
  note.textContent = standing.note();
  host.appendChild(cards);
  host.appendChild(note);

  host.appendChild(usageStatsChart(standing, report.daily));
  const pair = document.createElement("div");
  pair.className = "usage-stats-pair";
  pair.append(
    usageStatsBreakdown(
      standing,
      t("stats.byModel", "모델별"),
      t("stats.topModel", "가장 많이 쓴 모델:"),
      summary.top_model,
      report.model_breakdown,
    ),
    usageStatsBreakdown(
      standing,
      t("stats.byProject", "프로젝트별"),
      t("stats.topProject", "가장 많이 쓴 프로젝트:"),
      summary.top_project,
      report.project_breakdown,
    ),
  );
  host.appendChild(pair);
  host.appendChild(usageStatsSessions(standing, report.recent_sessions, summary));
}

/* Asks the backend, and keeps asking while its scan runs.
 *
 * The first scan reads every transcript on the disk, so the pane draws the
 * previous answer meanwhile and follows up rather than blocking. One timer,
 * cleared before it is set, because two asks racing would leave the slower
 * one's follow-up running forever. */
async function refreshUsageStats(force) {
  // The provider is read BEFORE the await and compared after it: switching
  // while a scan is in flight would otherwise land Codex's numbers under
  // Claude's cards, which is exactly the mistake this pane must not make.
  const asked = usageStatsProvider;
  // The overview is derived from both ledgers, so it asks both — and asks
  // them together rather than one after the other, because two round trips in
  // sequence would draw a half-built overview in between.
  const wanted = asked === "overview" ? USAGE_STATS_LEDGERS.map((one) => one.id) : [asked];
  const answers = await Promise.all(
    wanted.map(async (id) => {
      try {
        return [id, await invoke(usageStatsProviderOf(id).command, {
          scope: usageStatsScope,
          range: usageStatsRange,
          offsetMinutes: windowUtcOffsetMinutes(),
          force: Boolean(force),
        })];
      } catch (error) {
        showError(error);
        return [id, null];
      }
    }),
  );
  if (asked !== usageStatsProvider) return;
  let scanning = false;
  let scannedAt = null;
  let files = 0;
  let capped = false;
  let failed = null;
  for (const [id, answer] of answers) {
    usageStatsReports[id] = answer?.report ?? null;
    scanning = scanning || Boolean(answer?.scanning);
    files += answer?.files ?? 0;
    capped = capped || Boolean(answer?.capped);
    // A ledger this window could not open must SAY so: a database somebody
    // else has locked, reported as a clean zero, is a wrong answer nobody
    // looking can tell from a right one.
    failed = failed ?? answer?.error ?? null;
    // The OLDER of the two stamps, so the overview never claims to be fresher
    // than its stalest half.
    const stamp = answer?.scanned_at ?? null;
    if (stamp !== null) scannedAt = scannedAt === null ? stamp : Math.min(scannedAt, stamp);
  }
  usageStatsScanning = scanning;
  usageStatsMeta = { scanned_at: scannedAt, files, capped, error: failed };
  paintUsageStats();
  clearTimeout(usageStatsTimer);
  if (usageStatsScanning) {
    usageStatsTimer = setTimeout(() => refreshUsageStats(false), USAGE_STATS_RETRY_MS);
  }
}

el("usage-stats-switch").addEventListener("click", () => {
  const standing = usageStatsProviderOf(usageStatsProvider);
  if (standing.command === null) return;
  setUsageAnalyticsEnabled(standing.id, usageAnalyticsOff.has(standing.id));
});

el("usage-stats-provider").addEventListener("click", (event) => {
  const bounds = event.currentTarget.getBoundingClientRect();
  openUsageStatsProviders(bounds.right - 176, bounds.bottom + 4, event.currentTarget);
});

el("scm-overflow").addEventListener("click", (event) => {
  const bounds = event.currentTarget.getBoundingClientRect();
  openScmOverflow(bounds.right - 180, bounds.bottom + 4, event.currentTarget);
});

el("usage-stats-filters").addEventListener("click", (event) => {
  const bounds = event.currentTarget.getBoundingClientRect();
  openUsageStatsFilters(bounds.left, bounds.bottom + 4, event.currentTarget);
});

el("usage-stats-refresh").addEventListener("click", () => {
  void refreshUsageStats(true);
});

/* ── The share card ───────────────────────────────────────────────────────
 *
 * Orca's `ShareUsageCard` is a DOM node its button screenshots with
 * `html-to-image`, which serializes the node into an SVG `foreignObject` and
 * draws that into a canvas. WebKit refuses that path — a canvas with a
 * foreignObject image drawn into it is tainted, and `toDataURL` on it throws —
 * so on the engine this window actually ships, the original's approach copies
 * nothing at all.
 *
 * So the card is PAINTED, on a canvas, once. That is not a second renderer for
 * the pane's numbers: it is the only renderer for this card, and the dialog
 * shows the very canvas that gets copied, which is a stronger promise than the
 * original's (there, what you see is a DOM node and what you copy is a
 * rasterization of it, and the two differ wherever the rasterizer disagrees
 * with the browser).
 *
 * Everything the card says comes from the provider's own row in the table
 * above — its counters, its segment order, its labels, its cost word — so a
 * fourth ledger gets a share card by existing, and no vendor's numbers can be
 * drawn under another's headings.
 *
 * The geometry is the original's own (`ShareUsageCard.tsx`: 480 wide, 28px
 * gutters, a 120px chart of the last ten days, 52px stat tiles). What is NOT
 * carried over is its identity — its logo, its handle, its repository — which
 * belongs to them; this card wears this app's name and nothing else.
 */
const SHARE_CARD = {
  width: 480,
  pad: 28,
  head: 34,
  dates: 26,
  tiles: 52,
  gap: 20,
  chartHead: 24,
  dayTotals: 14,
  chart: 120,
  dayLabels: 16,
  legend: 24,
  foot: 44,
};

/* The card's height, as the sum of the blocks it draws. Written as the sum so
 * that a block that grows cannot draw past the bottom edge. */
function shareCardHeight() {
  const { pad, head, dates, tiles, gap, chartHead, dayTotals, chart, dayLabels, legend, foot } =
    SHARE_CARD;
  return pad + head + dates + tiles + gap + chartHead + dayTotals + chart + dayLabels + legend + foot;
}

/* The chart's colours, read from the same tokens the pane's own chart uses —
 * so the card cannot drift into a second palette. */
function shareCardInk() {
  const held = getComputedStyle(document.documentElement);
  const read = (name, fallback) => held.getPropertyValue(name).trim() || fallback;
  return {
    input: read("--usage-input", "#38bdf8"),
    output: read("--usage-output", "#34d399"),
    "cache-read": read("--usage-cache-read", "#fbbf24"),
    "cache-write": read("--usage-cache-write", "#d946ef"),
  };
}

/* One line, cut on the ellipsis rather than spilling out of its tile. */
function shareCardFit(ctx, text, room) {
  if (ctx.measureText(text).width <= room) return text;
  let cut = text;
  while (cut.length > 1 && ctx.measureText(`${cut}…`).width > room) cut = cut.slice(0, -1);
  return `${cut}…`;
}

/* The range, as the card's own pill says it. */
function shareCardRangeWord() {
  return USAGE_STATS_RANGES.find((one) => one.value === usageStatsRange)?.label() ?? usageStatsRange;
}

function drawShareCard(canvas, standing, report, scale) {
  const box = SHARE_CARD;
  const height = shareCardHeight();
  canvas.width = Math.round(box.width * scale);
  canvas.height = Math.round(height * scale);
  canvas.style.width = `${box.width}px`;
  canvas.style.height = `${height}px`;
  const ctx = canvas.getContext("2d");
  ctx.setTransform(scale, 0, 0, scale, 0, 0);
  const family = getComputedStyle(document.body).fontFamily;
  const font = (size, weight = 400) => {
    ctx.font = `${weight} ${size}px ${family}`;
  };

  // ---- the ground, and the two glows over it ----
  const ground = ctx.createLinearGradient(0, 0, box.width, height);
  ground.addColorStop(0, "#111111");
  ground.addColorStop(0.5, "#0a0a0a");
  ground.addColorStop(1, "#0d0d1a");
  ctx.beginPath();
  ctx.roundRect(0, 0, box.width, height, 16);
  ctx.fillStyle = ground;
  ctx.fill();
  // The two glows, inside the card's own corners.
  ctx.save();
  ctx.beginPath();
  ctx.roundRect(0, 0, box.width, height, 16);
  ctx.clip();
  const glow = (x, y, radius, colour) => {
    const shine = ctx.createRadialGradient(x, y, 0, x, y, radius);
    shine.addColorStop(0, colour);
    shine.addColorStop(1, "rgba(0, 0, 0, 0)");
    ctx.fillStyle = shine;
    ctx.fillRect(0, 0, box.width, height);
  };
  glow(box.width * 0.9, -40, 300, "rgba(20, 71, 230, 0.16)");
  glow(box.width * 0.05, height + 20, 250, "rgba(139, 92, 246, 0.12)");
  ctx.restore();
  ctx.strokeStyle = "rgba(255, 255, 255, 0.08)";
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.roundRect(0.5, 0.5, box.width - 1, height - 1, 16);
  ctx.stroke();

  const left = box.pad;
  const right = box.width - box.pad;
  let y = box.pad;

  // ---- who this is, and for which range ----
  ctx.textBaseline = "alphabetic";
  ctx.textAlign = "left";
  font(14, 600);
  ctx.fillStyle = "#fafafa";
  ctx.fillText(SHARE_CARD_NAME, left, y + 12);
  font(10);
  ctx.fillStyle = "#666666";
  ctx.fillText(
    t("stats.shareCardSaid", "{{provider}} 사용량", { provider: standing.name() }),
    left,
    y + 27,
  );
  const pill = shareCardRangeWord();
  font(11, 500);
  const pillWidth = ctx.measureText(pill).width + 16;
  ctx.beginPath();
  ctx.roundRect(right - pillWidth, y + 2, pillWidth, 20, 6);
  ctx.fillStyle = "rgba(255, 255, 255, 0.06)";
  ctx.fill();
  ctx.fillStyle = "#a1a1a1";
  ctx.textAlign = "center";
  ctx.fillText(pill, right - pillWidth / 2, y + 16);
  ctx.textAlign = "left";
  y += box.head;

  const summary = report.summary;
  const shape = standing.overview;
  font(11);
  ctx.fillStyle = "#555555";
  ctx.fillText(shareCardDates(report), left, y + 8);
  y += box.dates;

  // ---- three tiles: what it cost, how much it was, which model ----
  const tileWidth = (box.width - box.pad * 2 - 16) / 3;
  const tiles = [
    {
      value: formatCost(summary.estimated_cost_usd),
      label: standing.costShort(),
      ground: "rgba(20, 71, 230, 0.1)",
      edge: "rgba(20, 71, 230, 0.2)",
      ink: "#93b4ff",
      size: 16,
    },
    {
      value: formatTokens(shape.total(summary) || 0),
      label: t("stats.totalTokens", "전체 토큰"),
      ground: "rgba(255, 255, 255, 0.04)",
      edge: "rgba(255, 255, 255, 0.06)",
      ink: "#fafafa",
      size: 16,
    },
    {
      value: summary.top_model ?? t("stats.unknownModel", "알 수 없음"),
      label: t("stats.topModelShort", "가장 많이 쓴 모델"),
      ground: "rgba(255, 255, 255, 0.04)",
      edge: "rgba(255, 255, 255, 0.06)",
      ink: "#fafafa",
      size: 14,
    },
  ];
  tiles.forEach((tile, at) => {
    const x = left + at * (tileWidth + 8);
    ctx.beginPath();
    ctx.roundRect(x, y, tileWidth, box.tiles, 10);
    ctx.fillStyle = tile.ground;
    ctx.fill();
    ctx.strokeStyle = tile.edge;
    ctx.beginPath();
    ctx.roundRect(x + 0.5, y + 0.5, tileWidth - 1, box.tiles - 1, 10);
    ctx.stroke();
    font(tile.size, 600);
    ctx.fillStyle = tile.ink;
    ctx.fillText(shareCardFit(ctx, tile.value, tileWidth - 24), x + 12, y + 24);
    font(10);
    ctx.fillStyle = "#666666";
    ctx.fillText(shareCardFit(ctx, tile.label, tileWidth - 24), x + 12, y + 40);
  });
  y += box.tiles + box.gap;

  // ---- the ten days, in the pane's own stacking order ----
  font(11, 500);
  ctx.fillStyle = "#555555";
  ctx.fillText(t("stats.daily", "일별 사용량").toUpperCase(), left, y + 10);
  font(10);
  ctx.fillStyle = "#444444";
  ctx.textAlign = "right";
  // The summary already carries whichever count this vendor keeps — turns for
  // one shape, events for the other — and the provider's own row knows which
  // word goes with it.
  ctx.fillText(
    `${t("stats.sessionsCount", "세션 {{n}}개", { n: summary.sessions })} · ${standing.rowActivity(summary)}`,
    right,
    y + 10,
  );
  ctx.textAlign = "left";
  y += box.chartHead;

  const days = (report.daily ?? []).slice(-10);
  const segments = standing.face().segments();
  const ink = shareCardInk();
  const column = (box.width - box.pad * 2) / Math.max(days.length, 1);
  let heaviest = 1;
  for (const day of days) {
    heaviest = Math.max(heaviest, segments.reduce((sum, one) => sum + (one.of(day) || 0), 0));
  }
  font(8);
  ctx.textAlign = "center";
  days.forEach((day, at) => {
    ctx.fillStyle = "#444444";
    ctx.fillText(formatTokens(shape.dayTotal(day) || 0), left + column * (at + 0.5), y + 8);
  });
  const floor = y + box.dayTotals + box.chart;
  days.forEach((day, at) => {
    let bottom = floor;
    for (const segment of [...segments].reverse()) {
      const value = segment.of(day) || 0;
      if (value <= 0) continue;
      const tall = Math.max(1, Math.round((value / heaviest) * box.chart));
      ctx.fillStyle = ink[segment.key] ?? "#666666";
      ctx.fillRect(left + column * at + column * 0.2, bottom - tall, column * 0.6, tall);
      bottom -= tall;
    }
  });
  y = floor;
  font(9);
  days.forEach((day, at) => {
    ctx.fillStyle = "#555555";
    ctx.fillText(day.day.slice(5), left + column * (at + 0.5), y + 11);
  });
  ctx.textAlign = "left";
  y += box.dayLabels;

  font(9);
  let pen = left;
  for (const segment of segments) {
    ctx.beginPath();
    ctx.arc(pen + 3, y + 8, 3, 0, Math.PI * 2);
    ctx.fillStyle = ink[segment.key] ?? "#666666";
    ctx.fill();
    ctx.fillStyle = "#555555";
    ctx.fillText(segment.label, pen + 11, y + 11);
    pen += 11 + ctx.measureText(segment.label).width + 12;
  }
  y += box.legend;

  // ---- the two figures every one of these vendors reports, and whose card
  // ---- this is
  ctx.strokeStyle = "rgba(255, 255, 255, 0.05)";
  ctx.beginPath();
  ctx.moveTo(left, y + 0.5);
  ctx.lineTo(right, y + 0.5);
  ctx.stroke();
  y += 12;
  const pair = [
    [formatTokens(summary.input_tokens), t("stats.input", "입력")],
    [formatTokens(summary.output_tokens), t("stats.output", "출력")],
  ];
  let cursor = left;
  for (const [figure, word] of pair) {
    font(12, 600);
    ctx.fillStyle = "#cccccc";
    ctx.fillText(figure, cursor, y + 12);
    cursor += ctx.measureText(figure).width + 4;
    font(12);
    ctx.fillStyle = "#888888";
    ctx.fillText(word, cursor, y + 12);
    cursor += ctx.measureText(word).width + 16;
  }
  font(11);
  ctx.fillStyle = "#888888";
  ctx.textAlign = "right";
  ctx.fillText(SHARE_CARD_NAME, right, y + 12);
  ctx.textAlign = "left";
}

/* The days this card is about, spelled the way the original's own line is
 * (`share-card-utils.tsx:36-50`): a range reads as its two ends, and an
 * all-time card reads as "through today" because it has no other end. */
function shareCardDates(report) {
  const end = new Date();
  const spoken = (when) => when.toLocaleDateString(undefined, { month: "short", day: "numeric" });
  const days = Number.parseInt(usageStatsRange, 10);
  if (!Number.isFinite(days)) {
    return t("stats.shareThrough", "{{day}}까지", { day: spoken(end) });
  }
  const start = new Date(end.getTime() - days * 86_400_000);
  return `${spoken(start)} – ${spoken(end)}`;
}

/* The product's own name, which is the only identity this card carries. */
const SHARE_CARD_NAME = "ZeroCode";

/* The card is drawn at twice its size, which is the original's own
 * `pixelRatio: 2` — a card pasted into a chat is looked at on a display that
 * has more pixels than it has points. */
const SHARE_CARD_SCALE = 2;

let sharingUsage = false;

function shareUsageStanding() {
  const standing = usageStatsProviderOf(usageStatsProvider);
  if (standing.command === null) return null;
  const report = usageStatsReports[standing.id];
  return report?.summary?.has_any_data ? { standing, report } : null;
}

function openShareUsage() {
  const held = shareUsageStanding();
  if (!held) return;
  say(el("share-usage-copy-said"), () => t("stats.copyImage", "이미지 복사"));
  drawShareCard(el("share-usage-canvas"), held.standing, held.report, SHARE_CARD_SCALE);
  showModal(el("share-usage-scrim"));
}

function closeShareUsage() {
  hideModal(el("share-usage-scrim"));
}

/* The image, onto the clipboard, through the door that already exists for the
 * browser's markup: raw pixels and their measured size, checked against the
 * claim on the other side. */
async function copyShareUsage() {
  if (sharingUsage) return;
  const canvas = el("share-usage-canvas");
  sharingUsage = true;
  try {
    const pixels = canvas.getContext("2d").getImageData(0, 0, canvas.width, canvas.height);
    await invoke("set_clipboard_image", {
      rgba: markupBase64(new Uint8Array(pixels.data.buffer)),
      width: canvas.width,
      height: canvas.height,
    });
    say(el("share-usage-copy-said"), () => t("stats.copied", "복사했습니다"));
  } catch (error) {
    showError(error);
  } finally {
    sharingUsage = false;
  }
}

/* The post, as text. No handle and no address: the original signs its card
 * with its own project's, and inventing ours would put a link in somebody's
 * timeline that nobody here can answer for. */
function postShareUsage() {
  const held = shareUsageStanding();
  if (!held) return;
  const { standing, report } = held;
  const lines = [
    t("stats.sharePost", "{{range}} {{provider}} 사용량 — {{tokens}} 토큰 · {{cost}}", {
      range: shareCardRangeWord(),
      provider: standing.name(),
      tokens: formatTokens(standing.overview.total(report.summary) || 0),
      cost: formatCost(report.summary.estimated_cost_usd),
    }),
  ];
  openExternal(`https://x.com/intent/post?text=${encodeURIComponent(lines.join("\n"))}`);
}

el("usage-stats-share").addEventListener("click", openShareUsage);
el("share-usage-close").addEventListener("click", closeShareUsage);
el("share-usage-copy").addEventListener("click", () => void copyShareUsage());
el("share-usage-post").addEventListener("click", postShareUsage);

async function refreshProviderUsage(provider, force) {
  let report;
  try {
    report = await invoke(provider.command, { force });
  } catch (error) {
    showError(error);
    return;
  }
  const fetching = Boolean(report?.fetching);
  provider.write(report?.usage ?? null, fetching);
  paintProviderSegment(provider);
  paintUsageRefresh();
  paintUsagePanel();
  paintStatsUsage();
  // While the backend scans, keep asking — its answer changes once, and the
  // backend's own floor makes the asking free. One timer per provider: two
  // scans run against two different CLIs, and a shared timer would let the
  // faster one cancel the slower one's follow-up.
  clearTimeout(usageAskTimers.get(provider.id));
  if (fetching) {
    usageAskTimers.set(
      provider.id,
      setTimeout(() => refreshProviderUsage(provider, false), USAGE_STATS_RETRY_MS),
    );
  }
}

function refreshClaudeUsage(force) {
  return refreshProviderUsage(usageProvider("claude"), force);
}

function refreshCodexUsage(force) {
  return refreshProviderUsage(usageProvider("codex"), force);
}

function setUsageOpen(on, id = usagePanelProvider) {
  const pop = el("usage-pop");
  usagePanelProvider = id;
  usagePanelOpen = on;
  if (on) showing(pop);
  else closing(pop);
  usageAmbient.sync();
  el("sb-usage-roster").setAttribute("aria-expanded", String(on));
  if (!on) return;
  // This trigger owns the whole roster. Opening it renews every provider that
  // already has a place on screen rather than whichever icon happened to sit
  // under the pointer.
  for (const provider of usageRosterProviders()) refreshProviderUsage(provider, false);
  paintUsagePanel();
  const at = el("sb-usage-roster").getBoundingClientRect();
  pop.style.left = `${Math.max(8, at.left)}px`;
}

el("sb-usage-roster").addEventListener("click", () =>
  setUsageOpen(el("usage-pop").hidden),
);
/* 판이 모두를 보이므로 판의 새로 고침도 모두에게 묻는다 — 하나만 다시 읽으면
 * 나머지 행들이 옆에서 낡아 가고, 그 판은 「지금」을 말한다고 주장한다. 막대의
 * 새로 고침이 이미 이 길이다(`sb-usage-refresh`). */
el("usage-refresh").addEventListener("click", () => {
  for (const provider of usageRosterProviders()) void refreshProviderUsage(provider, true);
});
el("settings-usage-provider").addEventListener("change", (event) => {
  settingsUsageProvider = event.target.value;
  paintStatsUsage();
});
el("settings-usage-refresh").addEventListener("click", () =>
  refreshProviderUsage(usageProvider(settingsUsageProvider), true),
);
paintStatsUsage();

/* ---- when the gauges are asked -----------------------------------------
 *
 * A quota figure moves for exactly one reason: an agent spent something. So
 * the gauges are asked when that could have happened, and not on a blind
 * clock that is equally wrong on a busy machine and an idle one.
 *
 * Asking is nearly free and the backend is why: `usage_scan_holds` refuses
 * any unforced scan inside `usage::MIN_REFETCH` (5 minutes) and answers from
 * its cache instead, so an extra ask costs one IPC round trip and no network
 * at all. That floor is the rate limiter; this side only has to name the
 * moments worth asking at.
 *
 * Three moments, in the order they matter:
 *
 * 1. **The agents went quiet.** The dot beat already runs on every turn of
 *    every agent; three seconds of quiet after it is a turn that ended, which
 *    is the instant the number actually changed.
 * 2. **You looked at the window.** Staleness only matters to someone reading
 *    it, and that is the moment they are.
 * 3. **Time passed and nothing else happened.** The old blind interval, kept
 *    unchanged as the net under the other two — an idle machine costs exactly
 *    what it costs today.
 *
 * A live `rate_limit` frame beats all three when one arrives (`laneSignal`),
 * but that road is Claude's alone and only while a lane runs under `zo`. */
function askEveryProviderUsage() {
  for (const provider of usageAmbientTargets()) refreshProviderUsage(provider, false);
}

function usageAmbientTargets() {
  return USAGE_PROVIDERS.filter(
    (provider) => usagePanelOpen || statusBarItemEnabled(provider.id),
  );
}

const usageAmbient = idlePoller({
  wanted: () => usageAmbientTargets().length > 0,
  every: USAGE_AMBIENT_MS,
  tick: askEveryProviderUsage,
});

/* ---- 「새 빌드 준비됨」 ----------------------------------------------------
 *
 * The release lane runs outside this window (docs/design/release-lane-off-
 * the-window.md §2.3–2.4) and leaves two files behind; `release_status`
 * reads them on this bar's own fifteen-minute clock and judges them into
 * one notice (`update_runtime::update_notice`). This is the toast half:
 * sticky, once per installed pair for the life of this window — window
 * memory rather than storage, because a restart is exactly what makes it
 * moot — with 「다시 시작」 on the one restart road when the app changed, and
 * no button at all when only zo did (a new zo pane already runs the new
 * one). The other half is the settings notice (`paintUpdateNotice`,
 * shell-settings.js), painted from the same source. Nothing here restarts
 * anything (PRODUCT §10); the folder-panel toast is the shape — a toast
 * that stands for a condition and leaves with it. */
let releaseNotice = null;
/* How many workers are at work in this window's panes, by the ledger, as
 * `release_status` last answered (t-3058). A restart cuts every one of them
 * — they sleep and are seated again, but their turn is cut short — so the
 * app notice says so beside its button and recommends restarting after they
 * land. Zero says nothing. */
let releaseWorkers = 0;
const noticedBuilds = new Set();
let updateToast = null;
let updateToastKey = null;

/* The suffix the app notice carries while workers are at work — one t() in
 * five catalogs, shared by the toast and the settings notice. */
function updateWorkersWords() {
  if (!(releaseWorkers > 0)) return "";
  return t("update.workersBusy", "워커 {{n}}개 진행 중 — 착지 뒤 재시작 권장", { n: releaseWorkers });
}

/* The app sentence (t-3237): the change the backend named picks it. A
 * version the lane wrote beside the installed sha that is not the running
 * one is 「새 버전 {{version}}」, with the sha as a dim auxiliary after the
 * words; the same version again — or a lane that did not say — is 「새
 * 빌드({{sha}})」, carrying the sha itself. One builder for the toast and the
 * settings notice, through t() in five catalogs. */
function updateReadyAppWords(app) {
  if (app.change === "version") {
    return {
      sentence: t(
        "update.readyAppVersion",
        "새 버전 {{version}}이(가) 설치되었습니다 · 다시 시작하면 적용됩니다",
        { version: app.installed_version },
      ),
      aux: shortSha(app.installed),
    };
  }
  return {
    sentence: t(
      "update.readyAppBuild",
      "새 빌드({{sha}})가 설치되었습니다 · 다시 시작하면 적용됩니다",
      { sha: shortSha(app.installed) },
    ),
    aux: "",
  };
}

/* The zo sentence by the same rule — a pane still running another version
 * makes it 「zo {{version}} 설치됨」 — and no button belongs to it: a new zo
 * pane already runs the installed one. */
function updateReadyZoWords(zo) {
  if (zo.change === "version") {
    return {
      sentence: t(
        "update.readyZoVersion",
        "zo {{version}} 설치됨 · 새 zo 판부터 새 버전입니다",
        { version: zo.installed_version },
      ),
      aux: shortSha(zo.installed),
    };
  }
  return {
    sentence: t(
      "update.readyZoBuild",
      "새 zo 빌드({{sha}})가 설치되었습니다 · 새 zo 판부터 새 버전입니다",
      { sha: shortSha(zo.installed) },
    ),
    aux: "",
  };
}

/* The toast's app words: the sentence with the worker suffix (t-3058) after
 * it, and the dim sha to follow both. */
function updateToastAppWords(app) {
  const { sentence, aux } = updateReadyAppWords(app);
  const busy = updateWorkersWords();
  return { text: busy ? `${sentence} ${busy}` : sentence, aux };
}

/* Words with a dim auxiliary after them — the sha beside 「새 버전
 * {{version}}」 (t-3237), read second. `className` is the surface's own
 * (`toast-aux`, `settings-notice-aux`); no auxiliary is the words alone. */
function speakWithAux(node, words, aux, className) {
  node.textContent = words;
  if (!aux) return;
  const dim = document.createElement("span");
  dim.className = className;
  dim.textContent = aux;
  node.append(" ", dim);
}

function shortSha(sha) {
  return String(sha ?? "").slice(0, 7);
}

/* One key per installed pair: an app swap after a zo-only notice is a new
 * thing to say, and the same pair on the next poll is not. */
function updateNoticeKey(notice) {
  return `${notice.app?.installed ?? "-"}/${notice.zo?.installed ?? "-"}`;
}

function dismissUpdateToast() {
  const note = updateToast;
  updateToast = null;
  updateToastKey = null;
  if (note?.isConnected) closing(note, () => note.remove());
}

function raiseUpdateToast() {
  if (!releaseNotice) {
    dismissUpdateToast();
    return;
  }
  const key = updateNoticeKey(releaseNotice);
  // One stands at a time, and it stands for the pair on file now: a toast
  // for another pair leaves whether or not this one gets its own.
  if (updateToast && updateToastKey !== key) dismissUpdateToast();
  if (noticedBuilds.has(key)) return;
  noticedBuilds.add(key);
  updateToastKey = key;
  const { app, zo } = releaseNotice;
  if (app) {
    const { text, aux } = updateToastAppWords(app);
    updateToast = toast(text, "", {
      sticky: true,
      aux,
      action: {
        label: t("update.restart", "다시 시작"),
        run: () => void invoke("relaunch_window").catch(showError),
      },
    });
  } else {
    const { sentence, aux } = updateReadyZoWords(zo);
    updateToast = toast(sentence, "", { sticky: true, aux });
  }
  if (updateToast) updateToast.dataset.notice = "update";
}

function absorbReleaseStatus(answer) {
  releaseNotice = answer?.notice ?? null;
  releaseWorkers = Number.isInteger(answer?.workers) ? answer.workers : 0;
  paintUpdateNotice();
  raiseUpdateToast();
  // A toast already standing for this pair keeps saying the truth: the
  // worker count moves while the sha does not, and the sentence follows it.
  const app = releaseNotice?.app ?? null;
  if (app && updateToast?.isConnected && updateToastKey === updateNoticeKey(releaseNotice)) {
    const node = updateToast.querySelector(".toast-text");
    if (node) {
      const { text, aux } = updateToastAppWords(app);
      speakWithAux(node, text, aux, "toast-aux");
    }
  }
}

/* A refused or absent answer is silence: the lane is another process, its
 * files are its own, and a window that cannot read them has nothing to
 * say. Not settled and not on focus — a release is a minutes-scale event. */
function askReleaseStatus() {
  return invoke("release_status")
    .then(absorbReleaseStatus)
    .catch(() => {});
}

const releaseAmbient = idlePoller({
  wanted: () => true,
  every: USAGE_AMBIENT_MS,
  tick: askReleaseStatus,
  onResume: askReleaseStatus,
});
releaseAmbient.sync();

/* The agents stirred. Settled rather than immediate: a streaming turn beats
 * the dots many times a second, and one ask per beat would be one IPC round
 * trip per beat for an answer that cannot have moved yet. */
function noteUsageMayHaveMoved() {
  settle("usage", USAGE_SETTLE_MS, askEveryProviderUsage);
}

/* ---- source control, when something changed it -------------------------
 *
 * The panel was re-read only when THIS window saved a file, when it was
 * reopened, or when somebody pressed something in it. Everything else that
 * moves git — an agent editing files in a terminal, a `git` command run by
 * hand, an editor outside this app — reached it never, and the way to see
 * your own changes was to leave the panel and come back.
 *
 * Two moments fix that, and both are moments this window already knows:
 *
 * - **The agents stirred.** The dot beat runs on every turn, which is when an
 *   agent is writing. Settled, because a working agent beats it continuously
 *   and `git status` is a process, not a lookup.
 * - **You looked at the window.** Covers every editor and terminal outside
 *   this app, which no beat in here can hear.
 *
 * A recursive watcher over the worktree would hear all of it directly and is
 * what the original runs (`@parcel/watcher` with an ignore list, plus a 2 s
 * poll of the git common directory). Ours watches only the paths of open tabs
 * (`file_watch::WatchSet` stats an explicit list), and widening that to a
 * repository would be a stat storm — so the beat above stands in until a real
 * watcher exists. Recorded as the deviation it is.
 *
 * Never run for a panel nobody is looking at: `git status` on a large
 * repository is real work, and the panel re-reads itself when it opens. */
const SCM_SETTLE_MS = 1_500;

function refreshScmIfShowing() {
  if (el("activity-scm").hidden) return;
  refreshScm().catch(() => {});
}

function noteScmMayHaveChanged() {
  settle("scm", SCM_SETTLE_MS, refreshScmIfShowing);
}

/* Coming back to the window is the moment to catch up on everything that
 * changed while it was not being read — staleness only matters to somebody
 * reading, and this is when they are. Not settled: the answer wanted is the
 * one on screen now, and each of these already decides for itself whether
 * asking costs anything. */
function noteWindowFocused() {
  askEveryProviderUsage();
  refreshScmIfShowing();
}

window.addEventListener("focus", noteWindowFocused);

usageAmbient.sync();

/* ---- one dismisser for every popover ----
 *
 * Four popovers grew four copies of "close on an outside press", and the
 * next one would have made five. One registry, one document listener for
 * each gesture: a popover names the surfaces a press may land on without
 * dismissing it, and Escape closes whichever is open — top of the paint
 * order last, though in practice one is open at a time. */
const DISMISSABLE = [];

/* `keeps` are elements or selectors — a press inside one leaves the popover
 * up (its own trigger, a sibling submenu, the buttons that own it). */
function dismissable(pop, close, ...keeps) {
  DISMISSABLE.push({ pop, close, keeps });
}

document.addEventListener("pointerdown", (event) => {
  for (const { pop, close, keeps } of DISMISSABLE) {
    if (overlayClosed(pop)) continue;
    if (pop.contains(event.target)) continue;
    const kept = keeps.some((held) => {
      // 함수 keep은 판정 자체를 판이 정한다 — ＋ 팔레트가 "버튼 곁 몇 px의
      // 빗나간 재누름"을 바깥으로 치지 않으려고 쓴다(여섯 번째 실종).
      if (typeof held === "function") return Boolean(held(event));
      if (typeof held === "string") return Boolean(event.target.closest?.(held));
      return held.contains(event.target);
    });
    if (kept) continue;
    // 판정의 근거를 닫는 쪽에 건넨다 — 어디를 눌러서 닫혔는지는 그 판의
    // 일지가 적을 몫이고(＋ 팔레트가 그렇다), 관심 없는 닫기는 그냥 버린다.
    close(event);
  }
});

document.addEventListener("keydown", (event) => {
  if (event.key !== "Escape") return;
  for (const { pop, close } of DISMISSABLE) {
    if (overlayClosed(pop)) continue;
    close();
    event.stopPropagation();
  }
}, true);

dismissable(el("usage-pop"), () => setUsageOpen(false), el("sb-usage-roster"));

// The row pins are dynamic nodes, so they are kept by selector — a pointer
// landing on any of them must not dismiss the popover it is about to move.
dismissable(el("ports-pop"), () => setPortsOpen(false), el("sb-ports"), ".wt-ports");
dismissable(el("caffeinate-pop"), () => setCaffeinateOpen(false), el("sb-caffeinate"));

/* ---- 읽는 판에서 긁은 글자 (1-g41) ----
 *
 * 읽는 판의 선택은 우클릭 한 번으로 복사된다 — Orca의 `SelectedTextCopyMenu`.
 *
 * 메뉴를 새로 세우지는 않는다. 좌표를 받아 창 안으로 죄고, 고르면 스스로 닫히고,
 * 바깥을 누르거나 Escape를 치면 물러나는 판이 이미 하나 있다(`openSidebarMenu`).
 * 사이드바 바로가기 줄이 이미 그 판으로 한 줄짜리 메뉴를 띄운다. 두 번째 메뉴는
 * 두 벌의 닫힘 규칙과 두 벌의 그림자를 뜻하고, 그 둘은 반드시 어긋난다.
 *
 * 폭을 상수로 박지 않는 것도 같은 이유다. Orca의 144×36은 한 언어짜리 라벨을 잰
 * 값이고 이 창의 `복사`는 언어마다 길이가 다르다 — 실제로 그려진 상자를 재어
 * 죄는 쪽이 어느 언어에서도 창 밖으로 넘어가지 않는다. */
function selectionTextInside(host) {
  const selection = window.getSelection();
  if (!selection || selection.rangeCount === 0) return "";
  // 두 끝이 모두 이 판 안일 때만 이 판의 선택이다. 한쪽 끝이 밖이면 사람이 긁지
  // 않은 글자까지 따라오고, 복사한 사람은 그것을 볼 방법이 없다.
  if (!host.contains(selection.anchorNode) || !host.contains(selection.focusNode)) return "";
  return selection.toString().trim();
}

function armSelectionCopyMenu(host, extraItemsOf = null) {
  // 캡처 단계로 단다: 판 안의 줄도 저마다 우클릭 메뉴를 가질 수 있고, 긁어 둔
  // 글자가 있는 우클릭에서는 이 메뉴가 이긴다. 선택이 없으면 아무것도 하지 않고
  // 흘려보내므로 줄의 메뉴도, 플랫폼의 기본 메뉴도 그대로 남는다.
  host.addEventListener(
    "contextmenu",
    (event) => {
      const text = selectionTextInside(host);
      if (!text) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      // 판은 제 몫의 줄을 더 얹을 수 있다 — 프리뷰의 `AI 메모 추가`가 그것이다
      // (1-g43). 줄을 **지금** 만드는 것이 중요하다: 메뉴의 줄을 누르는 순간이면
      // 선택은 이미 걷혀 있고, 그때 재려던 것은 아무것도 남아 있지 않다.
      const rows = [{ label: t("selection.copy", "복사"), run: () => clipboardText.write(text) }];
      rows.push(...(extraItemsOf?.(text) ?? []));
      openSidebarMenu(event.clientX, event.clientY, rows, host);
    },
    true,
  );
}

// 읽으라고 둔 판들: 포트 목록, 검사 패널, 자동화의 최근 실행. 터미널과 브라우저
// 판은 여기 없다 — 저마다 제 선택과 제 우클릭 메뉴를 이미 들고 있고, 한 제스처가
// 두 가지 일을 하는 것이 이 기능이 아니다.
for (const host of [el("ports-pop"), el("activity-checks"), el("auto-runs")]) {
  armSelectionCopyMenu(host);
}

/* ---- leaving ----
 *
 * `hidden` takes the box away before any animation on it can run, which is
 * why every overlay in this window arrived with a 150ms fade and left between
 * two frames. Orca closes through `data-[state=closed]:animate-out` — the
 * entrance played backwards.
 *
 * ONE helper rather than a timer per dialog: mark the surface closing, let
 * the stylesheet own what "closing" looks like, and hide it when the motion
 * is spent. The class is what the CSS keys off; this only decides when.
 *
 * `prefers-reduced-motion` is respected by the same global rule that already
 * disables every other animation here — with `animation: none !important` the
 * `animationend` never comes, so the timer is the fallback that does the
 * hiding, and the surface closes instantly instead of hanging. */
/* Which turn of open/close a surface is on. A close that is still in flight
 * when the same surface is reopened must not hide the NEW one — which is the
 * bug a bare `setTimeout(() => node.hidden = true)` ships with, and it shows
 * up as a context menu that opens and then disappears 150ms later when it is
 * summoned twice quickly. */
const OVERLAY_TURN = new WeakMap();

function closing(node, done) {
  if (node.hidden) return;
  const turn = (OVERLAY_TURN.get(node) ?? 0) + 1;
  OVERLAY_TURN.set(node, turn);
  node.classList.add("is-closing");
  const motion = Number.parseFloat(
    getComputedStyle(document.documentElement).getPropertyValue("--motion-fast"),
  );
  setTimeout(
    () => {
      if (OVERLAY_TURN.get(node) !== turn) return;
      node.classList.remove("is-closing");
      node.hidden = true;
      if (node.matches?.(":popover-open")) {
        try {
          node.hidePopover();
        } catch {
          // 이미 내려가 있으면 그것으로 충분하다.
        }
      }
      done?.();
    },
    Number.isFinite(motion) && motion > 0 ? motion : 150,
  );
}

/* Showing cancels whatever close is in flight and clears the mark, so a
 * surface reopened mid-exit arrives through its entrance rather than
 * finishing somebody else's exit. Every opener goes through here for that
 * reason — setting `hidden = false` directly is what leaves the stale timer
 * armed. */
function showing(node) {
  OVERLAY_TURN.set(node, (OVERLAY_TURN.get(node) ?? 0) + 1);
  node.classList.remove("is-closing");
  node.hidden = false;
  // 합성 깨우기 (분할 스테이지 실측 2026-08-25). ＋팔레트가 owner=self ·
  // opacity=1 · rect 정상으로 서 있는데 화면에는 안 올랐다 — DOM은 다 그렸고
  // 웹뷰가 그 프레임을 화면에 올리지 않은 것이다. hidden 토글(display 재배치)
  // 조차 프레임을 못 올렸으므로, 레이어 자체를 새로 세운다: translateZ(0)가
  // 이 노드를 제 GPU 레이어로 승격시키고, 다음 프레임에 걷는다. rAF가 함께
  // 멎어 있는 병든 창에서는 걷히지 못한 translateZ(0)가 남는데, fixed 팝에는
  // 시각적 무해(항등 변환)라 그대로 두어도 된다.
  node.style.transform = "translateZ(0)";
  requestAnimationFrame(() => {
    if (!node.hidden) node.style.transform = "";
  });
  // popover를 단 표면은 top layer로도 올린다 — 이미 열려 있으면 그대로.
  if (node.matches?.("[popover]") && !node.matches(":popover-open")) {
    try {
      node.showPopover();
    } catch {
      // top layer가 없는 웹뷰에서는 fixed+z-index가 하던 일을 그대로 한다.
    }
  }
}

/* Whether an overlay is closed AS A PERSON SEES IT — which includes the
 * 150ms it spends leaving. `hidden` drops only when the exit's timer lands,
 * so a toggle asking `hidden` inside that window answers "open" and closes
 * the surface the person just asked FOR: pressing ＋ there made the palette
 * only ever blink, and pressing again landed in the exit of the press
 * before, forever (live report 2026-08-24, "깜박깜박만 거리고 열리지
 * 않아"). Mid-exit IS closed — `showing` was built to catch exactly that
 * reopen and cancel the stale timer; the toggles just had to ask the right
 * question. */
function overlayClosed(pop) {
  return pop.hidden || pop.classList.contains("is-closing");
}

/* A popover under the control that opened it, clamped inside the window.
 *
 * Clamped against the MEASURED box rather than a guessed width: the rows of a
 * menu are words, and words are a different length in every language — the one
 * that fits in English is the one that hangs off the edge in German. Called
 * after `showing`, because a hidden node measures zero. */
function placeUnder(pop, anchor) {
  const at = anchor.getBoundingClientRect();
  const box = pop.getBoundingClientRect();
  pop.style.left = `${Math.max(8, Math.min(at.left, window.innerWidth - box.width - 8))}px`;
  pop.style.top = `${Math.max(8, Math.min(at.bottom + 4, window.innerHeight - box.height - 8))}px`;
}

/* ---- the tooltip ----
 *
 * Ninety-two controls in this window explained themselves with the native
 * `title` attribute, and the operating system drew every one of them: a
 * yellow-grey box in the platform's font, at the platform's delay, in a
 * corner the platform chose. It was the most frequently seen surface in the
 * product and the only one that was not ours — hover any icon button and the
 * illusion ended there.
 *
 * Orca draws its own: `rounded-md px-3 py-1.5 text-xs` with the foreground
 * and background INVERTED (`bg-foreground text-background`,
 * `tooltip-DBWPZtc-.js:518`), entering on `fade-in-0 zoom-in-95
 * slide-in-from-*-2`. The inversion is the whole idea — a tooltip is the one
 * thing on screen that is not part of the layout, so it is the one thing
 * drawn in the opposite ink.
 *
 * ONE listener per gesture, at the document, reading `data-tip`. The
 * alternative — a component wrapped around each of the ninety-two — is how
 * ninety-two copies of a fade get written, and why the attribute this
 * replaces was attractive in the first place. Adding a tooltip here means
 * writing `data-tip="…"` in the markup and nothing else, which is exactly
 * the ergonomics of `title` with the drawing taken back.
 *
 * 700ms before it opens: Radix's own `delayDuration` default, which is what
 * Orca's tooltip is built on. Shorter and the window flickers as the pointer
 * crosses a toolbar; longer and it never arrives. */
const TIP_DELAY_MS = 700;
/* The gap between the anchor and the pill — the 8px the pill also travels as
 * it arrives, so the motion reads as the tooltip stepping away from the
 * control rather than sliding across it. */
const TIP_GAP = 8;

const tipNode = document.createElement("div");
// `tooltip`, not `tip`: the one-time tips modal already wears `.tip`
// (`<section class="permission tip">`), and taking that name would restyle a
// dialog this has nothing to do with.
tipNode.className = "tooltip";
tipNode.setAttribute("role", "tooltip");
tipNode.hidden = true;
document.body.append(tipNode);

let tipTimer = null;
let tipAnchor = null;

function hideTip() {
  if (tipTimer !== null) {
    clearTimeout(tipTimer);
    tipTimer = null;
  }
  tipAnchor = null;
  tipNode.hidden = true;
}

/* Placed after it is measured, never before: the pill's width depends on the
 * words in it, and a position computed from a stale box is the jump that
 * makes a tooltip look like it is chasing the pointer.
 *
 * Below the control by default, above it when there is no room — and the
 * side it came FROM is what `data-side` says, because the 8px slide has to
 * travel toward the anchor rather than away from whichever edge it flipped
 * off. */
function placeTip(anchor) {
  const box = anchor.getBoundingClientRect();
  // The pill's LAYOUT box. Its entrance animation carries a scale and a
  // translate, both of which a client rect reports — measuring that would
  // place the tooltip from a box that is still moving.
  const pillWidth = tipNode.offsetWidth;
  const pillHeight = tipNode.offsetHeight;
  const below = box.bottom + TIP_GAP;
  const room = window.innerHeight - below >= pillHeight;
  const top = room ? below : box.top - TIP_GAP - pillHeight;
  tipNode.dataset.side = room ? "bottom" : "top";
  // Centred on the anchor, then pulled back inside the window. A tooltip that
  // hangs off the edge is a tooltip with its words cut in half.
  const wanted = box.left + box.width / 2 - pillWidth / 2;
  const left = Math.max(TIP_GAP, Math.min(wanted, window.innerWidth - pillWidth - TIP_GAP));
  tipNode.style.left = `${Math.round(left)}px`;
  tipNode.style.top = `${Math.round(top)}px`;
}

function showTip(anchor) {
  const words = anchor.dataset.tip;
  // An empty `data-tip` is how a control says "nothing to add" — the same way
  // an empty `title` used to. Drawing an empty pill would be worse than the
  // attribute being absent.
  if (!words || !anchor.isConnected) return hideTip();
  tipAnchor = anchor;
  tipNode.textContent = words;
  // Measured off-screen first: `hidden` is `display: none`, which has no box
  // to measure, and the entrance animation restarts from the same toggle.
  tipNode.style.left = "0";
  tipNode.style.top = "-9999px";
  tipNode.hidden = false;
  placeTip(anchor);
}

/* `pointerover` rather than `mouseover`: it fires for pen and touch too, and
 * it bubbles, which is what makes one listener enough. */
document.addEventListener("pointerover", (event) => {
  const anchor = event.target.closest?.("[data-tip]");
  if (anchor === tipAnchor && !tipNode.hidden) return;
  hideTip();
  if (!anchor || !anchor.dataset.tip) return;
  tipTimer = setTimeout(() => {
    tipTimer = null;
    showTip(anchor);
  }, TIP_DELAY_MS);
});

document.addEventListener("pointerout", (event) => {
  const anchor = event.target.closest?.("[data-tip]");
  if (!anchor) return;
  // Moving between two children of the same control is not leaving it.
  if (anchor.contains(event.relatedTarget)) return;
  hideTip();
});

/* Keyboard reach. A control focused by tab explains itself immediately —
 * the delay exists to stop a travelling POINTER from opening things, and a
 * person who tabbed to a control has already chosen it. */
document.addEventListener("focusin", (event) => {
  const anchor = event.target.closest?.("[data-tip]");
  hideTip();
  if (anchor) showTip(anchor);
});

document.addEventListener("focusout", hideTip);

/* ---- the toast layer ----
 *
 * THE INLINE RULE STANDS. A result belongs beside the thing it happened to,
 * and every inline summary in this window stays exactly where it is — the
 * cleanup dialog still says what it deleted in the dialog, the push refusal
 * still gets its own line under the commit box. Those are the right shape and
 * this does not replace them.
 *
 * What was missing is the layer for the case the inline rule cannot answer: a
 * result whose origin is no longer on screen. `showError` had one — with any
 * tab open it wrote the failure to `console.error` and the person was told
 * nothing at all. That is the seam Orca puts a toast in, and it is the only
 * caller here; the rest is a layer and an API for the surfaces that adopt it
 * next.
 *
 * Sonner's measured defaults: the app radius, `min(26rem, 100vw-2rem)`,
 * `14px 16px`, 13px, lifted `2.5rem` off the bottom so it clears the status
 * bar rather than sitting on it. */
const TOAST_MS = 6000;

const toastLayer = document.createElement("div");
toastLayer.className = "toasts";
// A live region, so a failure a person cannot see is still a failure a screen
// reader says. `polite`: it is a report, not an interruption.
toastLayer.setAttribute("role", "status");
toastLayer.setAttribute("aria-live", "polite");
document.body.append(toastLayer);

/* Says one thing, then takes itself away.
 *
 * `kind` is the role the message plays — "halt" for a failure, absent for a
 * plain report — and it selects a token rather than a colour. */
function toast(words, kind = "", { action = null, sticky = false, aux = "" } = {}) {
  const said = String(words ?? "").trim();
  if (said === "") return null;
  const note = document.createElement("div");
  note.className = "toast";
  if (kind) note.dataset.kind = kind;
  // `aux` is a dim word after the sentence — the sha beside 「새 버전
  // {{version}}」 (t-3237) — inside the words' node, so the button is still
  // its own thing and the sentence is still read first.
  const dim = String(aux ?? "").trim();
  if (action) {
    // A toast that offers a way out carries one button beside its words —
    // Sonner's `action` — and the words get their own node so the button is
    // not part of the sentence a screen reader hears.
    note.classList.add("toast--acting");
    const text = document.createElement("span");
    text.className = "toast-text";
    speakWithAux(text, said, dim, "toast-aux");
    const button = document.createElement("button");
    button.type = "button";
    button.className = "btn toast-action";
    button.textContent = action.label;
    button.addEventListener("click", () => action.run());
    note.append(text, button);
  } else {
    speakWithAux(note, said, dim, "toast-aux");
  }
  toastLayer.append(note);
  // Leaves the way the overlays do, through the same exit the stylesheet
  // already owns — one motion vocabulary rather than a second one for this
  // layer alone. A sticky toast is the caller's to take away: it stands for
  // a condition, and goes when the condition does.
  if (!sticky) setTimeout(() => closing(note, () => note.remove()), TOAST_MS);
  return note;
}

/* PR conditions share the sticky toast surface. Dismissal is local UI state;
 * only a fetched resolving fact closes the durable notification. */
const scmNotices = new Map();
const scmDismissed = new Map();

function paintScmNotices(rows) {
  const active = new Set();
  for (const row of rows ?? []) {
    if (!row || typeof row.id !== "string") continue;
    if (row.resolved_at != null) {
      scmNotices.get(row.id)?.remove();
      scmNotices.delete(row.id);
      scmDismissed.delete(row.id);
      continue;
    }
    active.add(row.id);
    if (scmNotices.has(row.id) || scmDismissed.get(row.id) === row.opened_at) continue;
    const words = row.kind === "mergeable"
      ? t("checks.observe.conflict", "PR #{{number}} 병합 충돌 · {{root}}", row)
      : t("checks.observe.ci", "PR #{{number}} CI 실패 · {{root}}", row);
    const notice = toast(words, "halt", {
      sticky: true,
      action: {
        label: t("checks.observe.dismiss", "닫기", {}),
        run: () => {
          scmDismissed.set(row.id, row.opened_at);
          scmNotices.get(row.id)?.remove();
          scmNotices.delete(row.id);
        },
      },
    });
    if (notice) scmNotices.set(row.id, notice);
  }
  for (const [id, notice] of scmNotices) {
    if (active.has(id)) continue;
    notice.remove();
    scmNotices.delete(id);
  }
  for (const id of scmDismissed.keys()) {
    if (!active.has(id)) scmDismissed.delete(id);
  }
}

listen("scm:notices", (event) => paintScmNotices(event.payload));

/* Codex's PTY route is the safety net, not a state to hide. The first event
 * is said immediately and this term-keyed mark remains reviewable until every
 * worker using the fallback has ended. */
const codexPtyFallbacks = new Map();

function codexPtyFallbackMessage(reason) {
  return t(
    "codexRoute.degraded",
    "Codex 네이티브 라우트를 쓸 수 없어 PTY로 전환했습니다: {{reason}}",
    { reason },
  );
}

function paintCodexPtyFallbacks() {
  const mark = el("sb-codex-route");
  mark.hidden = codexPtyFallbacks.size === 0;
  if (mark.hidden) {
    delete mark.dataset.tip;
    mark.removeAttribute("aria-label");
    return;
  }
  const reason = [...codexPtyFallbacks.values()].at(-1);
  const tip = t(
    "codexRoute.status",
    "Codex 워커 {{count}}개의 포인터 전달이 PTY 폴백을 사용 중입니다. 클릭해 이유를 확인하세요. {{reason}}",
    { count: codexPtyFallbacks.size, reason },
  );
  mark.dataset.tip = tip;
  mark.setAttribute("aria-label", tip);
}

function forgetCodexPtyFallback(term) {
  if (!codexPtyFallbacks.delete(term)) return;
  paintCodexPtyFallbacks();
}

listen("codex-route:degraded", (event) => {
  const { term, reason } = event.payload ?? {};
  if (typeof term !== "number" || typeof reason !== "string" || reason.trim() === "") return;
  codexPtyFallbacks.set(term, reason);
  paintCodexPtyFallbacks();
  showError(codexPtyFallbackMessage(reason));
});

el("sb-codex-route").addEventListener("click", () => {
  const reason = [...codexPtyFallbacks.values()].at(-1);
  if (reason !== undefined) toast(codexPtyFallbackMessage(reason), "halt");
});
// Anything that moves the window under the pill takes it away: a scrolled
// list, a press, a keystroke. Capture, because a scroller that stops the
// event still moved the anchor.
document.addEventListener("scroll", hideTip, true);
document.addEventListener("pointerdown", hideTip, true);
document.addEventListener("keydown", hideTip, true);

/* What this process is holding, the way Orca's bar reports it. Asked rarely:
 * it is a number to glance at, not to watch. */
function paintMemory() {
  return invoke("process_memory")
    .then((bytes) => {
      // Keep the closed status segment on the same unit scale as the detailed
      // process tree that replaces this figure while the manager is open.
      const measured = resourceManager.open
        ? resourceManager.snapshot?.total_memory ?? bytes
        : bytes;
      el("sb-memory").textContent = measured > 0 ? resourceFormatMemory(measured) : "";
    })
    .catch(() => {});
}

/* ---- the two figures on the bar that cost a subprocess ----
 *
 * These were asked from `renderTabs`, which is a DOM render — it runs on every
 * tab opened or closed, every pane split, every lane event, every stage
 * update, every workspace switch. And a port scan is not a read: on macOS it
 * is `lsof` TWICE, once for the listening sockets and once for those pids'
 * working directories, and on a busy machine that pair is hundreds of
 * milliseconds. So clicking a project shelled out to `lsof` half a dozen
 * times, and the click had a beat in it.
 *
 * The counts beside them are free — they are already in this window's memory —
 * so the two halves are separated and only the free one is on the render path.
 * Bounded rather than timed: still asked by the things that happen anyway,
 * but never twice inside one beat and never twice at once. */
const MACHINE_BEAT = 4000;
let machineAskedAt = -Infinity;
let machineInFlight = false;

function paintMachineFigures(now = false) {
  // `now` is for the one caller that is a person LOOKING at the answer —
  // opening the ports popover. Everything else takes what the last beat got.
  if (machineInFlight) return;
  const at = performance.now();
  if (!now && at - machineAskedAt < MACHINE_BEAT) return;
  machineAskedAt = at;
  machineInFlight = true;
  Promise.allSettled([paintMemory(), refreshPorts()]).then(() => {
    machineInFlight = false;
  });
}

function paintRunningCount() {
  // A number beside a terminal glyph, no words — Orca's resource segment
  // (`ResourceUsageStatusSegment`: glyph, 11px tabular figure, a quiet
  // interpunct between pairs). The words this used to print said the same
  // thing wider in four languages, and they were the only words on the
  // right half of the bar.
  //
  // Shells, not tabs: a split tab is running two of them, and the number a
  // person glances at to ask "what is this machine holding" is the number
  // of processes. Lanes are counted in with them — a lane IS a shell here,
  // and Orca's own figure is every session its daemon holds.
  const figure = String(
    order.length +
      detachedAgents.size +
      Number(floatTermRunning) +
      tabs
        .filter((tab) => tab.kind === "term")
        .reduce((total, tab) => total + paneLeaves(tab.layout).length, 0),
  );
  // Assigning the text it already shows replaces the text node all the same
  // — a mutation and a layout of the bar once a frame while a shell renames
  // itself (the freeze probe's `sb-count childList×59`).
  const count = el("sb-count");
  if (count.textContent !== figure) count.textContent = figure;
}

function renderTabs() {
  // The pop-out is a board-only surface. Canonical settings can repaint
  // shared chrome during boot, but it must never manufacture the main
  // window's tab strip or stage in this webview.
  if (isPopout) return;
  paintRunningCount();
  // The board door on the toolbar wears whether the board is the tab on
  // stage — Orca's trigger carries `aria-pressed` (SidebarToolbar.tsx:94).
  // Painted here because every road a tab takes on or off the stage runs
  // through this render, so the door cannot say yes to a board that left.
  const door = el("nav-agents");
  const pressed = String(activeTabId === "board");
  if (door.getAttribute("aria-pressed") !== pressed) door.setAttribute("aria-pressed", pressed);
  // The Jev door beside it wears the same fact about its own tab.
  const jevDoor = el("nav-jev");
  const jevPressed = String(activeTabId === JEV_TAB.id);
  if (jevDoor.getAttribute("aria-pressed") !== jevPressed) jevDoor.setAttribute("aria-pressed", jevPressed);
  // The machine figures ride along, bounded — a render is a fine moment to
  // notice they are stale, and a terrible one to spawn `lsof`.
  paintMachineFigures();
  renderStage();
  for (const group of stageGroups()) renderPane(groupOf(group), group);
}

/* What about a tab is BUILT rather than set.
 *
 * Everything else a tab wears — active, unsaved, preview, the lane's state,
 * what its agent last said, its colour, its label — is an attribute that can
 * be written onto a node that already exists. These two are not: the glyph is
 * markup, and a pinned tab wears a pin where the close control would be. So a
 * node whose shape is unchanged is reused, and only a tab that was pinned or
 * changed kind is built again. */
function tabNodeShape(tab) {
  return `${TAB_GLYPH[tab.kind] ?? "terminal"}|${tab.pinned ? "pin" : "close"}`;
}

/* The parts of a tab node that never change while it exists — its markup, its
 * glyph, and its listeners.
 *
 * The listeners look their tab up by id at the moment they fire rather than
 * closing over the object they were built beside. A node outlives any single
 * render now, and `openTab` can replace what an id holds; a handler holding
 * the object it first saw would act on a tab that is no longer the tab. */
function buildTabNode(tab, group) {
  const id = tab.id;
  const owner = () => tabs.find((held) => held.id === id);
  const node = document.createElement("div");
  // A tab stop with Enter, but keeping `role="tab"` rather than borrowing
  // the button role — this is a tab, and saying otherwise to a screen
  // reader would be worse than saying nothing.
  node.setAttribute("role", "tab");
  node.tabIndex = 0;
  node.dataset.tab = id;
  // A pinned tab wears the pin where the close would be — Orca's strip
  // hides the close button entirely while the pin holds
  // (rename-file-Bs2nv9pa.js:6325, :6397).
  node.innerHTML =
    '<span class="tab-dot"></span><span class="tab-kind"></span>' +
    '<span class="tab-label"></span>' +
    (tab.pinned
      ? '<span class="tab-pin" aria-hidden="true"></span>'
      : '<button class="tab-close">×</button>');
  if (tab.pinned) node.querySelector(".tab-pin").innerHTML = icon("pin");
  // What kind of thing this tab holds. Orca's tabs carry one (measured on
  // 1.4.164: its `Terminal 1` tab wears a terminal glyph), and a strip of
  // bare words is the thing that reads as a wireframe — the same finding
  // that put glyphs on the sidebar rows.
  // A file tab wears the file's own glyph, the way Orca's editor tabs do
  // (EditorPanel calls the same getFileTypeIcon the explorer uses).
  node.querySelector(".tab-kind").innerHTML = icon(
    tab.kind === "file" && tab.path ? fileTypeIcon(tab.path) : (TAB_GLYPH[tab.kind] ?? "terminal"),
  );
  node.addEventListener("click", () => setActiveTab(id));
  // Double-clicking the tab keeps it: Orca's `onMakePermanent`, on the
  // label's own double-click (EditorPanel-C3Tfktqz.js:610).
  node.addEventListener("dblclick", () => {
    const held = owner();
    if (!held?.preview) return;
    held.preview = false;
    renderTabs();
    // A kept tab is kept across the restart too.
    persistStageLayouts();
  });
  // Middle-click closes it, the way Orca's strip does and the way every
  // strip a person has used does (`onAuxClick` checking `event.button === 1`,
  // unsaved-close-queue-DB-LbON-.js:2540-2605). It goes through `closeTab`,
  // so a document with unsaved typing still gets asked about. A pinned tab
  // refuses it outright — the middle click is the accidental close, and
  // the pin exists against exactly that (rename-file-Bs2nv9pa.js:6305).
  node.addEventListener("auxclick", (event) => {
    if (event.button !== 1) return;
    event.preventDefault();
    if (owner()?.pinned) return;
    closeTab(id);
  });
  node.addEventListener("pointerdown", (event) => beginTabDrag(event, id));
  // Right-click raises the tab's own menu. It stages the tab first: every
  // row on that menu is about "this tab", and answering one about a tab
  // the window is not showing is how a person closes the wrong thing.
  node.addEventListener("contextmenu", (event) => {
    event.preventDefault();
    setActiveTab(id);
    const held = owner();
    if (held) tabMenuAt(held, event.clientX, event.clientY, node);
  });
  node.addEventListener("keydown", (event) => {
    if (event.key !== "Enter" && event.key !== " ") return;
    event.preventDefault();
    setActiveTab(id);
  });
  // The keyboard's way to the same menu. Orca's rows answer the menu key
  // too, and a menu only the mouse can raise is the gap the worktree list
  // already closed once.
  node.addEventListener("keydown", (event) => {
    if (event.key !== "ContextMenu" && !(event.key === "F10" && event.shiftKey)) return;
    event.preventDefault();
    setActiveTab(id);
    const held = owner();
    if (!held) return;
    const box = node.getBoundingClientRect();
    tabMenuAt(held, box.left, box.bottom, node);
  });
  node.querySelector(".tab-close")?.addEventListener("click", (event) => {
    event.stopPropagation();
    closeTab(id);
  });
  return node;
}

/* Which agent a terminal tab IS, when it is one.
 *
 * Orca resolves the tab's agent from its panes (`resolvePaneAgentOwner`) —
 * the owner is whichever pane holds the session. The first pane that names
 * one is this side's honest equivalent: the session ledger once a hook has
 * spoken, the launch ledger from the moment of the spawn. */
function tabAgentSlug(tab) {
  if (tab.kind !== "term") return null;
  for (const term of paneLeaves(tab.layout)) {
    const named = paneSessions.get(term)?.agent ?? paneAgents.get(term);
    if (named) return named;
  }
  return null;
}

/* Everything about a tab that a render can have changed, onto a node that
 * already exists. */
function paintTabNode(node, tab) {
  const lane = tab.kind === "lane" ? lanes.get(focusedId)?.lane : null;
  // What the agent inside this terminal last reported. Same sentence as a
  // lane's state and drawn with the same marks, because to a person looking
  // at the strip it IS the same sentence — one arrives over a socket and the
  // other through a hook.
  const said = hookStateOfTab(tab);
  const classes = [
    "tab",
    tab.kind === "lane" ? `is-lane ${slotClass(focusedId)}` : "",
    tab.id === activeTabId ? "is-active" : "",
    // Unsaved edits, on the tab rather than only in the header — the header
    // belongs to the file you are looking at, and the one you have to be
    // told about is the one you are not.
    isDirty(tab) ? "is-unsaved" : "",
    // A glance, not a keep — the label goes italic, Orca's own mark
    // (EditorPanel-C3Tfktqz.js:607).
    tab.preview ? "is-preview" : "",
    tab.pinned ? "is-pinned" : "",
    lane ? `is-${STATE_CLASS[lane.state] ?? "idle"}` : "",
    // A shell in this tab rang the bell while nobody was looking. Same dot as
    // the rest of the strip's "this one wants you" marks, because to a person
    // reading the strip it is the same sentence.
    tab.kind === "term" && paneLeaves(tab.layout).some((term) => bellRang.has(term))
      ? "is-bell"
      : "",
    // A pane whose ring the notify seat took away is not marked as waiting
    // either (t-6043): the mark is the ring's twin on the strip.
    said === "needs-attention" && !tabHushed(tab) ? "is-waiting" : "",
    said === "working" ? "is-streaming" : "",
    tab.color ? "has-color" : "",
  ]
    .filter(Boolean)
    .join(" ");
  if (node.className !== classes) node.className = classes;
  // A repository's declared tab colour, on the identity dot the lane tint
  // already uses. Set as the custom property those rules read — never
  // interpolated into a style string — and the value itself came through
  // Rust's hex validator. Removed when the colour goes: a node that outlives
  // one render would otherwise keep a tint the tab no longer declares.
  if (tab.color) node.style.setProperty("--lane", tab.color);
  else node.style.removeProperty("--lane");
  // Each of these is written only when it differs: this runs for every tab
  // on the strip once a frame while any shell renames itself, and an
  // attribute or a text node set to what it already holds is still a
  // mutation record — and, for the label, a layout of the strip (the freeze
  // probe's `tab-label childList×118`, `tab-close aria-label×118`).
  const selected = tab.id === activeTabId ? "true" : "false";
  if (node.getAttribute("aria-selected") !== selected) node.setAttribute("aria-selected", selected);
  // Written per render rather than at build: this is words, and words change
  // when the language does.
  const close = node.querySelector(".tab-close");
  const closing = t("tab.close", "탭 닫기");
  if (close && close.getAttribute("aria-label") !== closing) close.setAttribute("aria-label", closing);
  const label = node.querySelector(".tab-label");
  const wording = tabLabel(tab);
  if (label.textContent !== wording) label.textContent = wording;
  // A tab holding an agent leads with the agent's identity, not the shell
  // glyph — Orca's `TerminalTabLeadingIcon` (rename-file-BAQu9znK.js): bell,
  // then state, then `TerminalTabAgentIdentityIcon` stamping
  // `data-agent-icon` and drawing `AgentIcon size={12}`, and only a tab that
  // is none of those keeps `ShellIcon`. The bell and the state already live
  // on `.tab-dot`; the identity mark is painted here, in the render pass,
  // because an agent appears AFTER the node is built — a launch or a hook,
  // never a rebuild. Swapped only when the answer changes, so a render is
  // not a refetch and the node under the pointer holds still.
  const wearing = node.querySelector(".tab-kind");
  const slug = tabAgentSlug(tab);
  // Keyed by the agent AND the domain its mark comes from. A pane resumed at
  // boot is painted before the catalog has answered, so the first face is the
  // letter tile with no domain to fetch — and a key of the slug alone would
  // hold that letter for the life of the tab (라이브 보고 2026-09-15 "claude
  // 아이콘도 사라진거같고": the tab read `C`). When the catalog lands the key
  // changes and the mark is fetched, the way `paintUsageStatsFace` already
  // keys its face.
  const reg = slug ? agentRows.find((one) => one.id === slug) : undefined;
  const face = slug ? `${slug}:${reg?.favicon_domain ?? ""}` : null;
  if ((wearing.dataset.agentIcon ?? null) !== face) {
    if (slug) {
      wearing.dataset.agentIcon = face;
      wearing.replaceChildren(
        agentIcon(reg ?? { id: slug, name: agentName(slug), favicon_domain: "" }),
      );
    } else {
      delete wearing.dataset.agentIcon;
      wearing.innerHTML = icon(TAB_GLYPH[tab.kind] ?? "terminal");
    }
  }
  // `?? ""` because `dataset` STRINGIFIES: a tab with no path — a terminal,
  // the board, a browser — handed `undefined` here wears data-tip="undefined",
  // and the tooltip layer faithfully shows the word to anyone who hovers
  // (라이브 보고 "언디파인드 뭐하는지 모르겠네"). An empty tip is falsy at the
  // reader and shows nothing, which is what no-path means.
  const tip =
    tab.kind === "lane"
      ? (lane?.session_id ?? "")
      : tab.kind === "term" && tab.resumed
        ? t("session.resumedTab", "이어서 — {{agent}}", {
            agent: tab.agent || agentName(tabAgentSlug(tab) || ""),
          })
        : (tab.path ?? "");
  if (node.dataset.tip !== tip) node.dataset.tip = tip;
}

/* ＋ 옆의 조용한 숫자 — 지금 **판 없이 도는** 에이전트가 몇인가.
 *
 * 떼어 두기가 생기면서 "닫았는데 아직 돈다"가 화면에서 사라진다. 상태
 * 표시줄의 총계는 그 사실을 말해 주지 못한다(거기서는 셸 하나가 는 것과
 * 구별되지 않는다). 그래서 새 판을 여는 바로 그 자리 옆에서, **안 보이는
 * 것들**만 센다 — 없으면 서지 않고, 누르면 그 목록이 열려 하나를 고르면
 * 그 자리에서 다시 붙는다. 사용자 요청: "＋ 버튼 옆에 터미널 표시가 되야 함".
 *
 * 자기 워크트리의 것만 센다: 이 줄은 이 체크아웃의 무대이고, 다른 프로젝트에서
 * 도는 에이전트는 그 프로젝트의 카드가 말한다. */
function detachedTabButton(pane, group) {
  if (pane.detachedTab) return pane.detachedTab;
  const pill = document.createElement("button");
  pill.type = "button";
  pill.className = "tab-detached";
  const glyph = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  glyph.setAttribute("class", "icon");
  glyph.setAttribute("aria-hidden", "true");
  const use = document.createElementNS("http://www.w3.org/2000/svg", "use");
  use.setAttribute("href", "#i-terminal");
  glyph.appendChild(use);
  const figure = document.createElement("span");
  figure.className = "tab-detached-count";
  pill.append(glyph, figure);
  pill.addEventListener("click", (event) => {
    focusedPane = group;
    const box = event.currentTarget.getBoundingClientRect();
    const waiting = detachedHere();
    if (waiting.length === 0) return;
    openSidebarMenu(box.left, box.bottom + 4, [
      {
        label: t("terminal.detachedTitle", "판 없이 도는 에이전트"),
        disabled: true,
        run: () => {},
      },
      ...waiting.map(([term, worktree]) => ({
        label: detachedRowLabel(term),
        run: () => void focusAgentPane(worktree, null, term, paneAgents.get(term) ?? null),
      })),
    ]);
  });
  pane.detachedTab = pill;
  return pill;
}

/* 이 체크아웃에서 판 없이 도는 것들, 열린 순서대로. 떼어 둔 뒤에 끝난
 * 것은 세지 않는다 — 완전한 삭제는 term:exited의 일이고, 이 목록의 일은
 * 지금 참인 문장이다. */
function detachedHere() {
  return [...detachedAgents].filter(
    ([term, worktree]) => worktree === activeWorktreePath && detachedStillWorking(term),
  );
}

/* 그 줄이 뭐라고 불리는가 — 터미널이 말한 제목, 없으면 에이전트 이름,
 * 그것도 없으면 셸 번호. 카드의 행이 쓰는 사다리와 같은 순서다. */
function detachedRowLabel(term) {
  const spoken = (termTitles.get(term) ?? "").trim();
  if (spoken) return spoken;
  const agent = paneSessions.get(term)?.agent ?? paneAgents.get(term);
  if (agent) return agentName(agent);
  return t("terminal.shellNumber", "셸 {{n}}", { n: term });
}

/* The one control that is not a tab. Built with its strip and kept with it,
 * for the same reason the tabs are — its words are rewritten per render. */
function addTabButton(pane, group) {
  if (pane.addTab) return pane.addTab;
  // Orca keeps this beside its tabs — and its `+` never opens a terminal
  // outright: it drops the searchable create palette (`TabBarCreateEntry`),
  // measured and carried in 1-fs ("+ 버튼 위에 터미널이 열리는게 아니고
  // 이런식으로 열림 확인해"). `⌘T` stays the straight door to a terminal for
  // people who already know the chord.
  const add = document.createElement("button");
  add.type = "button";
  add.className = "tab-new";
  add.textContent = "＋";
  const summon = (road) => {
    // It opens into the leaf it was clicked in, which is what clicking the
    // control in *that* strip means.
    focusedPane = group;
    if (overlayClosed(tabCreatePop)) return openTabCreate(add, road);
    // 방금 연 판의 재누름은 재시도다(TC_RETRY_GRACE 곁의 일지) — 닫지 말고
    // 입장을 다시 민다: 재소환은 showing()의 합성 킥과 자리잡기를 한 번 더
    // 지나므로, 프레임을 삼킨 창에서는 이 재입장이 곧 약이다.
    if (Date.now() - tcOpenedAt < TC_RETRY_GRACE) {
      return openTabCreate(add, `reopen(${road})`);
    }
    closeTabCreate("toggle");
  };
  // The custom titlebar hands a drag to macOS on mousedown. The OS may keep
  // the matching mouseup/click, which left this control waiting for an event
  // that never came. Own a primary pointer at its first reversible edge;
  // openTabCreate focuses the field itself, so preventing the button's native
  // focus costs nothing. The click road below remains for keyboard and AT.
  add.addEventListener("pointerdown", (event) => {
    if (event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    summon("pointerdown");
  });
  add.addEventListener("click", (event) => {
    // A real pointer was already handled above. Synthetic/keyboard activation
    // has detail 0 and still deserves the same one owner.
    if (event.detail > 0) return;
    summon("keyboard");
  });
  pane.addTab = add;
  return add;
}

/* The strip, reconciled rather than rebuilt.
 *
 * This threw every node away and made them again on each render — and a
 * render is what a tab CLICK does, so switching tabs rebuilt nine nodes, nine
 * innerHTML parses, nine icon substitutions and some seventy listeners to
 * change one class. Worse than the cost: the node under the pointer was
 * replaced mid-gesture, which is a tooltip that will not settle and a hover
 * that blinks.
 *
 * Nodes are kept by tab id and reordered in place. Only a tab whose SHAPE
 * changed — pinned, or a different kind — is built again. */
function renderPane(pane, group) {
  const strip = pane.strip;
  const held = (pane.tabNodes ??= new Map());
  const wanted = paneTabs(group);
  const seen = new Set();
  for (const [at, tab] of wanted.entries()) {
    seen.add(tab.id);
    const shape = tabNodeShape(tab);
    let entry = held.get(tab.id);
    if (entry !== undefined && entry.shape !== shape) {
      entry.node.remove();
      entry = undefined;
    }
    if (entry === undefined) {
      entry = { node: buildTabNode(tab, group), shape };
      held.set(tab.id, entry);
    }
    paintTabNode(entry.node, tab);
    if (strip.children[at] !== entry.node) {
      strip.insertBefore(entry.node, strip.children[at] ?? null);
    }
  }
  for (const [id, entry] of held) {
    if (seen.has(id)) continue;
    entry.node.remove();
    held.delete(id);
  }
  const waiting = detachedHere();
  const pill = detachedTabButton(pane, group);
  if (pill.hidden !== (waiting.length === 0)) pill.hidden = waiting.length === 0;
  if (waiting.length > 0) {
    pill.querySelector(".tab-detached-count").textContent = String(waiting.length);
    pill.dataset.tip = t("terminal.detachedTip", "판 없이 도는 에이전트 {{n}}", {
      n: waiting.length,
    });
    pill.setAttribute("aria-label", pill.dataset.tip);
  }
  const add = addTabButton(pane, group);
  const addChord = optionalShortcutLabel("terminal.newTab");
  const addTip = addChord === null
    ? t("terminal.new", "새 터미널")
    : t("terminal.newChord", "새 터미널 ({{chord}})", { chord: addChord });
  if (add.dataset.tip !== addTip) add.dataset.tip = addTip;
  const addLabel = t("terminal.newTab", "새 터미널");
  if (add.getAttribute("aria-label") !== addLabel) add.setAttribute("aria-label", addLabel);
  // Both tail pieces settle in one move, and only when either is out of
  // place. Appending each on its own — "the pill last, then the add last" —
  // had them evict each other on every render: two nodes moved, four
  // childList mutations and a relayout of the strip on every agent-paint
  // beat, with nothing on the strip having changed (the freeze probe's
  // `#tabstrip childList×236` over sixty frames).
  if (strip.lastElementChild !== add || add.previousElementSibling !== pill) strip.append(pill, add);
  // Every strip in the tree stays visible: a group with no tabs has already
  // folded out of the tree (`collapseStageGroup`), so the only empty strip
  // left is the last group standing — the stage itself, which carries the
  // way to open a terminal exactly when there is nothing open.
  strip.hidden = false;
}

/* Dragging a tab to reorder it, measured from Orca rather than guessed.
 *
 * The inventory carried this as "reported but not verified" and said to
 * re-measure before building it, so it was read out of the bundle:
 * `TAB_DRAG_ACTIVATION_DISTANCE_PX = 12` and `resolveDropZone` insetting a
 * drop target by 10% before it counts as the strip rather than an edge
 * (`rename-file-*.js`). Twelve pixels is the part that matters here — below
 * it a drag is a click, and a tab that slides on a one-pixel tremor makes the
 * strip feel broken to anyone who taps rather than clicks.
 *
 * Reorder only. Orca's edge zones split the pane, and a split is a different
 * model with its own tree (1-e); the thresholds are recorded so the next
 * stage does not have to re-measure them. */
const TAB_DRAG_ACTIVATION_PX = 12;

let tabDrag = null;

function beginTabDrag(event, id) {
  // Left button only, and never from the close control — a drag that starts
  // on `×` would fight the click that closes the tab.
  if (event.button !== 0 || event.target.closest(".tab-close")) return;
  tabDrag = { id, startX: event.clientX, moved: false };
}

/* Which leaf the pointer is over, and where in it. */
function dropTargetAt(x, y) {
  for (const group of stageGroups()) {
    const rect = groupOf(group).el.getBoundingClientRect();
    if (x < rect.left || x > rect.right || y < rect.top || y > rect.bottom) continue;
    return { group, zone: dropZone(rect, x, y) };
  }
  return null;
}

function showDropZone(target) {
  for (const held of groups.values()) {
    held.el.classList.remove("is-drop-left", "is-drop-right", "is-drop-up", "is-drop-down");
  }
  if (target === null || target.zone === "center") return;
  groupOf(target.group).el.classList.add(`is-drop-${target.zone}`);
}

function tabDragOver(event) {
  if (tabDrag === null) return;
  if (!tabDrag.moved) {
    if (Math.abs(event.clientX - tabDrag.startX) < TAB_DRAG_ACTIVATION_PX) return;
    tabDrag.moved = true;
    tabstrip.classList.add("is-dragging");
  }
  // An edge is a split, so it is shown while the pointer is there and acted on
  // when the pointer is let go — a split that happened mid-drag would fire
  // every time somebody dragged across the stage.
  tabDrag.target = dropTargetAt(event.clientX, event.clientY);
  showDropZone(tabDrag.target);
  if (tabDrag.target !== null && tabDrag.target.zone !== "center") return;
  const from = tabs.findIndex((tab) => tab.id === tabDrag.id);
  if (from < 0) return;
  // Reordering happens inside the tab's own strip; crossing into another
  // group is a drop, decided when the pointer is let go.
  if (tabDrag.target !== null && tabDrag.target.group !== tabs[from].pane) return;
  // Where the pointer sits among the tabs as they are drawn now: past a
  // neighbour's midpoint means it belongs on the other side of it. The strip
  // is the tab's own group's — the global list holds every group's tabs, so
  // the neighbour's place is looked up by who it is, not by where it sits.
  const strip = groupOf(tabs[from].pane).strip;
  let to = tabs.length;
  for (const node of strip.querySelectorAll(".tab")) {
    const box = node.getBoundingClientRect();
    if (event.clientX < box.left + box.width / 2) {
      to = tabs.findIndex((tab) => tab.id === node.dataset.tab);
      break;
    }
  }
  if (to === from || to < 0) return;
  tabs.splice(to > from ? to - 1 : to, 0, ...tabs.splice(from, 1));
  renderTabs();
}

function endTabDrag() {
  if (tabDrag === null) return;
  const { id, target } = tabDrag;
  tabDrag = null;
  tabstrip.classList.remove("is-dragging");
  showDropZone(null);
  if (!target) return;
  const tab = tabs.find((held) => held.id === id);
  if (!tab) return;
  const source = tab.pane;
  // A worker's own tab, dragged by the person: the placement seat's label
  // reads the move (t-5806). A tab holding a split is the coordinator's.
  const draggedTerm =
    tab.kind === "term" && paneLeaves(tab.layout).length === 1 ? paneLeaves(tab.layout)[0] : null;
  // Dropped in another group's middle: the tab joins that group — Orca's
  // `moveUnifiedTabToGroup` — and the group it left folds if it emptied.
  if (target.zone === "center") {
    if (target.group === source) return;
    tab.pane = target.group;
    collapseStageGroup(source);
    setActiveTab(tab.id);
    renderTabs();
    updateStage();
    persistStageLayouts();
    if (draggedTerm !== null) noteWorkerRoomChange(draggedTerm, "tab");
    return;
  }
  // Dropped on an edge: the leaf under the pointer becomes a split, the tab
  // opens the new group on the side it was dropped on, and the zone picks
  // the axis — the measured mapping (buildSplitNode's callers, :57706).
  if (splitDropIsNoOp(source, paneTabs(source).length, target.group, target.zone)) return;
  const added = nextGroupId;
  nextGroupId += 1;
  setStageTree(
    splitStageLeaf(
      stageTree(),
      target.group,
      added,
      SPLIT_DIRECTION[target.zone],
      SPLIT_POSITION[target.zone],
    ),
  );
  tab.pane = added;
  collapseStageGroup(source);
  setActiveTab(tab.id);
  renderTabs();
  updateStage();
  persistStageLayouts();
  if (draggedTerm !== null) noteWorkerRoomChange(draggedTerm, "tab");
}

window.addEventListener("pointermove", tabDragOver);
window.addEventListener("pointerup", endTabDrag);
window.addEventListener("pointercancel", endTabDrag);

/* Which tab kinds share the one preview slot. Orca's editor family — editor,
 * diff, conflict-review, check-details — replace each other's preview and
 * nothing else's (`canReplacePreviewContentType`, I18nProvider:57448); ours is
 * the document family the stage draws. */
function previewReplaceable(kind) {
  return kind === "file" || kind === "diff" || kind === "image";
}

/* `focus: false` puts a tab on the strip without taking the stage from what
 * is on it. Only one road asks for that today — a schedule firing at 9am,
 * which must be findable afterwards and must not seize the screen of somebody
 * mid-sentence — and it is an argument rather than a second builder because
 * everything else about being born is identical. */
function openTab(tab, { focus = true } = {}) {
  const owned = { worktree: activeWorktreePath, ...tab };
  const held = tabs.find((existing) => existing.id === owned.id);
  // A tab opened without a leaf named lands in the one being looked at, which
  // is what "open this next to what I am reading" means.
  if (held) {
    // Global document doors can reopen in another checkout. Their former
    // leaf belongs to the old layout, just as for a newly born foreign tab.
    if (held.worktree !== owned.worktree && owned.pane === undefined) {
      owned.pane = owned.worktree === activeWorktreePath ? focusedPane : 0;
    }
    // Opening the same document DELIBERATELY pins it; glancing at it again
    // keeps it a glance. Orca's activate does exactly this split
    // (`preservePreview: isPreview`, I18nProvider:71750, :57782).
    const staying = owned.preview ? held.preview : false;
    Object.assign(held, owned);
    held.preview = staying;
  } else {
    // One glance at a time: a new preview takes the seat the old one held,
    // rather than a row of tabs nobody asked to keep (I18nProvider:57610).
    // Safe to drop silently — a preview with typing in it stopped being a
    // preview at the first keystroke, so what goes has nothing to lose.
    if (owned.preview && previewReplaceable(owned.kind)) {
      const glanced = tabs.find(
        (existing) =>
          existing.preview &&
          existing.worktree === owned.worktree &&
          previewReplaceable(existing.kind),
      );
      if (glanced) dropTab(glanced.id);
    }
    // The leaf a tab is born into is a leaf of the tree IN FRONT — and the
    // tree in front belongs to the active checkout. A tab opened for another
    // checkout (an orchestration worker seated by `term:worker` while the
    // person was looking elsewhere) must not inherit that leaf: the other
    // checkout's tree may never have had it, and a tab whose group no tree
    // holds is listed by the sidebar (which reads `tabs`) and drawn by
    // nothing (the strip and the stage read the group). Every tree has
    // group 0 — it is the chassis that never folds — so that is home.
    const pane = owned.worktree === activeWorktreePath ? focusedPane : 0;
    tabs.push({ pane, ...owned });
  }
  if (focus) {
    setActiveTab(owned.id);
  } else {
    // The strip learns about it and the stage keeps whoever had it. Not
    // `setActiveTab` with the old id put back: that would acknowledge the new
    // tab, move it up the recency order and hand the keyboard around — three
    // things a tab nobody has looked at has no business claiming.
    renderTabs();
    updateStage();
  }
  syncWatchedFiles();
  // A terminal tab being born is a layout change too — `renderPanes` ran
  // before this tab was on the strip, so the persist it fired missed it.
  // It is also the fact the sidebar dot is folded from: a live shell makes a
  // workspace `active` whether or not an agent ever speaks in it, so the row
  // has to hear about the shell rather than about the first hook.
  if (owned.kind === "term") {
    persistPaneLayouts(owned.worktree);
    scheduleAgentPaint(["cards"]);
  }
  // And a document being born is a stage change.
  if (STAGE_STORED_KINDS.has(owned.kind)) persistStageLayouts();
  // The tab that is actually on the strip.
  //
  // Neither road above seats the caller's object: a new tab is pushed as a
  // COPY (`{ pane: focusedPane, ...owned }`) and an existing one is assigned
  // INTO. So a caller that keeps what it passed in is holding a ghost — the
  // two objects share an id and nothing else, and every later mutation lands
  // on the one nobody can see. That cost a restored conversation its wake:
  // the eager wake ran in full, resumed a real agent, and cleared `asleep` on
  // an object the strip had never heard of.
  return tabs.find((one) => one.id === owned.id) ?? owned;
}

/* Tell the backend which files to watch for outside writes: every open file
 * tab, as one whole set. Sent whole rather than as add/remove deltas — the
 * set is derived state, and a replacement cannot drift from what is open.
 * Both lifecycle doors (`openTab`, `dropTab`) just call this, so nothing has
 * to reason about which door it was. Skipped when the set did not change:
 * most tabs are terminals, and opening one should not cost the backend a
 * watch-list rebuild. */
let watchedFiles = "";
function syncWatchedFiles() {
  const paths = [...new Set(tabs.filter((tab) => tab.kind === "file").map((tab) => tab.path))].sort();
  const key = paths.join("\n");
  if (key === watchedFiles) return;
  watchedFiles = key;
  invoke("watch_files", { paths }).catch(() => {});
}

function setActiveTab(id) {
  activeTabId = id;
  const owner = tabs.find((tab) => tab.id === id);
  if (owner) {
    focusedPane = owner.pane;
    activeTabByWorktree.set(owner.worktree, owner.id);
    // Picking a restored tab whose shells refused to start is the gesture
    // that asks again — the stage stopped asking on its own so that a repaint
    // could not turn one failure into an error per frame.
    delete owner.wakeRefused;
    // Not awaited: bringing a tab forward must not wait on the disk. The
    // answer repaints when it arrives.
    checkFileMoved(owner);
    // Looking at a tab is what "seen" means — the board's unseen dot
    // clears for every pane this tab holds (Orca acknowledges by paneKey).
    acknowledgeTab(owner);
    // And the sidebar wears both halves of that look: the card's unread
    // weight drops, and the row of the pane now on stage takes the focused
    // fill. Signature-guarded, so a switch that changes neither paints
    // nothing.
    scheduleAgentPaint(["cards"]);
    // And looking at it moves it up the group's recency order — the order
    // that decides where the eye lands when a tab closes.
    if (owner.pane !== undefined) pushRecentTab(owner.pane, owner.id);
  }
  // The document surfaces are painted by `updateStage`, per leaf, rather than
  // here for the one tab that was clicked \u2014 see the note there.
  updateStage();
  renderTabs();
  paintViewToggle();
  const active = currentTab();
  if ((active?.kind === "lane" || active?.kind === "term") && termFloat.hidden) keySink.focus();
  // Which shell holds the keyboard just changed, and a program that asked to
  // be told is owed both halves — the one losing it and the one taking it.
  noteTermFocus();
  workerClock.sync();
  helperClock.sync();
  browserMenuPoll.sync();
  resourcePoll.sync();
  jevPoll.sync();
}

/* A tree back from the file may hold leaves whose tabs did not come back
 * with it — a browser is never respawned, and the stage record keeps no
 * terminals — and an empty leaf nobody closes stood as a dead half-screen
 * (live report 2026-08-14 #65). So a finished restore walks close's own
 * door over every group, and repaints only if something folded. */
function settleRestoredStage() {
  let folded = false;
  for (const group of stageGroups()) {
    if (collapseStageGroup(group)) folded = true;
  }
  if (folded) {
    renderTabs();
    updateStage();
  }
}

/* The workspaces this window has already put back.
 *
 * The early return below asks "has this workspace been restored yet", and it
 * used to answer with "does any tab belong to it" — one fact standing in for
 * another. The two came apart the moment something OTHER than a restore could
 * seat a tab in a checkout nobody had opened, and the ledger's restart
 * restoration does exactly that: a sleeping worker is reseated into the
 * checkout it was cut for (`seatLedgerManagedTerm`) long before anybody clicks
 * that card. The workspace then owned a tab, so opening it read no file at all
 * — and the first save from it, which stores every tab the ledger does NOT
 * own, wrote the person's own stored conversation away as an empty set. That
 * is the whole of "재시작하면 그 워크트리의 클로드 판이 죽은 채 돌아오지
 * 않았다": nothing died, nobody read.
 *
 * Asked as itself, the two facts stop being able to answer for each other. */
const restoredWorkspaces = new Set();

async function restoreActiveWorktreeTab({ firstTerminal = true } = {}) {
  // Tabs from THIS worktree may have left while another one was on stage —
  // their groups could not fold then, because folding reads the visible
  // tree. Now that this tree is the visible one, fold what emptied.
  for (const group of stageGroups()) collapseStageGroup(group);
  const worktree = activeWorktreePath;
  const remembered = activeTabByWorktree.get(activeWorktreePath);
  const owned = tabs.filter((tab) => tab.worktree === activeWorktreePath);
  const target = owned.find((tab) => tab.id === remembered) ?? owned[owned.length - 1];
  if (target && restoredWorkspaces.has(worktree)) {
    setActiveTab(target.id);
    return;
  }
  // Read once per workspace and no more. Everything below OPENS what the file
  // holds, and a second reading of the same file opens it a second time.
  restoredWorkspaces.add(worktree);

  // Hide the previous workspace immediately. Its PTYs keep running in the
  // registry, but none may remain visible while the new cwd is being opened.
  activeTabId = null;
  focusedPane = stageGroups()[0];
  updateStage();
  renderTabs();
  // What this worktree looked like when it was last looked at, before the
  // plain first terminal: the documents in their groups under their tree,
  // then the terminal tabs with fresh shells in them.
  const docsRestored = await restoreStageLayout(activeWorktreePath);
  // …except that a stored set of nothing but PLAIN SHELLS gives way to the
  // chosen default agent, which is what opening a workspace is supposed to
  // start (Orca's initial terminal is the default agent —
  // `pickQuickWorkspaceAgent(settings.defaultTuiAgent)`, and its restored tabs
  // are the running programs themselves reattached, so nobody there ever
  // lands on a bare shell where an agent belonged). Ours re-spawns from a
  // record, so a record of bare shells restored one and the default agent was
  // never reached at all. The asymmetry is the whole rule: a stored
  // CONVERSATION is a thing somebody said and it always wins; a stored plain
  // shell is a shape, and a shape yields. With 자동 nothing is chosen to yield
  // to, and the stored shells come back exactly as before.
  const stored = await storedPaneLayouts(activeWorktreePath);
  const yieldsToDefaultAgent =
    stored.length > 0 && !storedLayoutsHoldProgram(stored) && defaultAgentChosen();
  if (!yieldsToDefaultAgent) {
    if (await restoreWorktreeLayouts(activeWorktreePath, stored)) {
      settleRestoredStage();
      return;
    }
    if (docsRestored) {
      settleRestoredStage();
      return;
    }
  }
  // Tabs something ELSE seated here are this workspace too — a worker the
  // ledger reseated into the checkout it was cut for is the case that exists.
  // Nothing of this workspace's own was stored, so there is nothing left to
  // put back, and neither the repository's opening layout nor a plain
  // terminal belongs on top of a pane that is already working.
  if (target) {
    setActiveTab(target.id);
    settleRestoredStage();
    return;
  }
  // The repository's own opening layout, when it declares one and this
  // workspace has not had it yet. Falls through to the plain terminal
  // otherwise, which is what every checkout without a `defaultTabs:` gets.
  if (await openDefaultTabs(activeWorktreePath)) {
    settleRestoredStage();
    return;
  }
  // …unless the caller is about to seat this workspace's first terminal
  // itself — the creation flow's agent tab or lane. Orca's activation carries
  // that seat as part of the same call for the same reason (`activateAndReveal-
  // Worktree(…)` returns the `primaryTabId` it seated): with two doors, two
  // agents stand up in one fresh checkout.
  // The ledger's word first: a checkout it cut for a worker stored nothing,
  // and the default agent is a guess where the ledger has the answer.
  if (firstTerminal && !(await openLedgerSeatedAgent())) await openTermTab();
  settleRestoredStage();
}

/* The tabs a repository says a new workspace opens with.
 *
 * `defaultTabs:` in the project file — a list of `{title, color, command}` —
 * measured from Orca's `applyDefaultTerminalTabs`
 * (worktree-activation-3qRw45tK.js:1724). This window has parsed the key since
 * the project file was built and never acted on it, so a repository could
 * declare its opening layout and nothing happened.
 *
 * Three rules come with it, and each one is load-bearing:
 *
 *   1. **Once per workspace.** Orca keys a flag by worktree id
 *      (`defaultTerminalTabsAppliedByWorktreeId`) so a second visit does not
 *      stack a second set of tabs. Ours is a list of paths in the state
 *      directory, asked before and written after.
 *   2. **The commands run only if the person's policy says so.** A repository
 *      declaring `defaultTabs` with commands in them is a file in a checkout
 *      asking this machine to run things, which is the question the setup
 *      policy already answers. Orca ties them to the same policy
 *      (`getDefaultTabsLaunch`, out/main/index.js:69371) and so does this — a
 *      second door with its own default would let a repository route around
 *      the answer somebody already gave.
 *   3. **Typed, never executed.** The command is put into the shell the way a
 *      person would type it, and the Enter is theirs. This window learned that
 *      rule the expensive way on the usage scanner, and a command that came out
 *      of a file in a checkout is the last place to unlearn it.
 */
/* Worktrees whose declared tabs are being opened RIGHT NOW. The applied
 * ledger on disk answers "ever", and its two IPC halves — read, then mark —
 * leave a gap a double-click on a not-yet-active sidebar row can race
 * through: both activations read `applied: false` before either has marked,
 * and both open the whole set. One synchronous membership test in the one
 * webview that calls this closes the gap the file cannot. (Found by review.) */
const defaultTabsOpening = new Set();

async function openDefaultTabs(worktree) {
  if (defaultTabsOpening.has(worktree)) return false;
  defaultTabsOpening.add(worktree);
  try {
    return await openDefaultTabsOnce(worktree);
  } finally {
    defaultTabsOpening.delete(worktree);
  }
}

async function openDefaultTabsOnce(worktree) {
  let report;
  try {
    report = await invoke("default_tabs", { worktree });
  } catch {
    // A project file this window cannot read is not a reason to leave somebody
    // without a terminal. The scripts panel is where that failure is reported.
    return false;
  }
  const tabs = report?.tabs ?? [];
  if (tabs.length === 0 || report.applied) return false;
  // Marked before the first spawn, not after: opening several shells is slow
  // enough that a second activation can arrive mid-way, and two runs of this
  // would each open the whole set.
  try {
    await invoke("mark_default_tabs_applied", { worktree });
  } catch {
    // Unrecorded means it would run again next visit — better than not running
    // at all, and the person can close what they do not want.
  }
  // A declared command is code out of somebody else's checkout, and typing it
  // into a shell is how it gets in front of the Enter key. Asked once, before
  // the first one, and only when there is a command to ask about — a repository
  // that declares titles and colours is declaring a layout, not code. A refusal
  // still opens every tab: the layout was never the dangerous half.
  const declares = tabs.some((template) => template.command?.trim());
  const trusted = declares ? await askRepoTrust() : true;
  let first = null;
  for (const template of tabs) {
    // Tabs, said out loud: the file declared a LIST OF TABS, and a list of
    // three that opened as one tab split three ways is not what it asked for.
    const term = await openTermTab({ placement: "tab", door: "command" });
    if (term === null || term === undefined) break;
    const tab = tabOfTerm(term);
    // The title goes on the PANE, which is where this window keeps a name
    // somebody gave a shell — the strip reads it back for a one-pane tab.
    if (tab && template.title) setPaneTitle(tab, term, template.title);
    // The declared colour rides the tab and paints the strip's identity dot.
    // Verbatim from the report: the value was validated in Rust (`tab_color`,
    // #rgb/#rrggbb only) and this side adds nothing to it.
    if (tab && template.color) tab.color = template.color;
    if (first === null) first = tab?.id ?? null;
    const command = template.command?.trim();
    // `runs_commands` is three-valued: true runs it, false opens the tab with
    // the command typed and waiting, and absent means the policy is to ask —
    // which this does not do behind somebody's back either. An unrun command
    // is typed WITHOUT its newline, so the shell shows it ready and the Enter
    // is the person's.
    if (command && trusted) {
      const said = report.runs_commands === true ? `${command}\n` : command;
      await invoke("term_text", { term, text: said });
    }
  }
  renderTabs();
  if (first !== null) setActiveTab(first);
  return true;
}

/* Take a tab off the strip without touching what it was showing. Closing is
 * [`closeTab`]; this is what a lane disappearing on its own looks like.
 *
 * `closed` says whether a person asked for this — their close, the agent's
 * `zerocode-browser close`, which walks the same door on their behalf. Every
 * other caller is BOOKKEEPING: a checkout swept off the disk, a project
 * removed, an emulator tab giving its seat to the shell that replaces it. The
 * two are told apart for one reason — the browser's restore record. A tab the
 * person closed is one they do not want back; a tab this window took away is
 * one they never let go of, and rewriting the record for it is how a window
 * comes back on the next start with nothing in it (t-5453). */
function dropTab(id, { closed = false } = {}) {
  const at = tabs.findIndex((tab) => tab.id === id);
  const removed = tabs[at];
  const leaf = removed?.pane ?? focusedPane;
  if (at < 0) return;
  cancelAutoSave(removed);
  if (removed.kind === "skills") releaseSkillsView(removed);
  // The pane host exists because this tab does, so it goes when the tab
  // goes — whichever path took the tab off the strip. Left to the callers,
  // this was missed by every route that drops a tab as bookkeeping rather
  // than as a close, and each one left a host on the stage with shells inside
  // it that still believed they were being looked at. Ending the processes is
  // a different question and stays with `closeTab`; this only takes away the
  // element the tab was drawn into.
  dropPaneHost(id);
  tabs.splice(at, 1);
  // A question about a tab that has left is a question nobody can answer, so
  // it leaves too. (The answered one arrives here one shift later and finds
  // nothing, which is what makes this safe to call on every close.)
  withdrawPinnedAsk(removed);
  if (activeTabByWorktree.get(removed.worktree) === id) {
    activeTabByWorktree.delete(removed.worktree);
  }
  // What the eye had seen of it goes with it, before any successor is
  // chosen off the same order. (The scroll cache entry deliberately stays:
  // closing and reopening within one sitting comes back to the same line,
  // and the cache is LRU-bounded either way.)
  dropRecentTab(leaf, id);
  // The group folds with its last tab, and the eye moves to its layout
  // sibling — which is where the replacement tab is looked for too.
  const folded = removed.worktree === activeWorktreePath && collapseStageGroup(leaf);
  if (activeTabId === id) {
    // Only a sibling owned by the workspace now on screen can replace it.
    // Choosing a tab from another workspace is the stale-terminal bug.
    // WHICH sibling is the recency order's answer, not the strip's —
    // Orca's `pickNextActiveTab`: the tab seen most recently, else the
    // neighbour (next first, then previous).
    const group = folded ? focusedPane : leaf;
    const siblings = paneTabs(group);
    setActiveTab(pickNextActiveTab(siblings, group, id));
  } else if (folded) {
    renderTabs();
    updateStage();
  }
  syncWatchedFiles();
  // The native pane goes when the tab does, from THIS door — every road off
  // the strip passes here, and a webview surviving its tab would float over
  // whatever the leaf shows next. Ending it is idempotent on the backend, so
  // a close that already said it costs one refused knock.
  // 프리뷰가 가면 그 판에 걸려 있던 찾기의 Range도 간다 — 등록부에 남은 죽은
  // Range는 아무것도 칠하지 않으면서 판만 붙들고 있다(1-g38).
  if (removed?.kind === "mdview" && docSearchRanges.delete(removed.id)) {
    paintDocSearch();
  }
  // 닫힌 결합 뷰의 로더와 감시자 — 대기열이 남으면 다음 답이 고아 절을 찾고,
  // 관찰자는 떼어낸 DOM을 붙들고 있는다.
  if (removed?.kind === "changes") {
    changesLoads.delete(removed.id);
    changesWatches.get(removed.id)?.disconnect();
    changesWatches.delete(removed.id);
    // 그리고 그 절들의 편집면. `destroy()` 없이 버린 merge 뷰는 자기 DOM
    // 리스너와 측정 루프를 계속 든다 — 판 하나에 절이 이백 개일 수 있다.
    for (const key of [...changesMerges.keys()]) {
      if (key.startsWith(`${removed.id}\n`)) dropChangesMerge(key);
    }
    for (const key of [...changesEditing]) {
      if (key.startsWith(`${removed.id}\n`)) changesEditing.delete(key);
    }
  }
  if (removed?.kind === "emulator") {
    removed.emulatorEpoch = (removed.emulatorEpoch ?? 0) + 1;
    // 닫힌 탭의 자막 시계는 여기서 멈춘다 — 1초 타이머는 탭이 사라져도
    // 혼자 돌고, 그 틱이 남의 판에 글을 쓴다.
    dropEmulatorBootCaption(removed);
    // 아직 아무 스트림에도 붙지 못한 탭이 닫힐 수 있다 — 판을 먼저 세우므로
    // 붙기 전의 판이 존재하고, 그 판을 닫는 것은 정상적인 일이다.
    void teardownEmulatorStream(removed.stream);
    dropEmulatorBinaryDoor(removed);
    // objectURL을 놓는 것만으로는 그림이 사라지지 않는다 — 판의 호스트는
    // 그룹마다 캐시되어 탭보다 오래 살고, `<img>`는 마지막 그림을, 캔버스는
    // 제 백스토어(1080×2400이면 10MB 남짓)를 계속 붙들고 있는다.
    //
    // 호스트가 이 탭의 것일 때만 걷는다: 같은 그룹의 다른 에뮬레이터 탭이
    // 그 자리를 이어받았다면 그 탭의 그림을 지우는 일이 된다.
    const emptied = groupOf(removed.pane)?.emulatorView;
    if (emptied && emptied._emulatorTab === removed) {
      const shown = emptied.querySelector(".emulator-frame");
      shown.removeAttribute("src");
      shown.hidden = true;
      const drawn = emptied.querySelector(".emulator-canvas");
      drawn.width = 0;
      drawn.height = 0;
      drawn.hidden = true;
      emptied._emulatorTab = null;
    }
  }
  if (removed?.kind === "browser") {
    // A crosshair armed on the pane that is leaving has nothing left to grab.
    if (browserGrab !== null && browserGrab.label === removed.label) stopBrowserGrab(false);
    invoke("close_browser_pane", { label: removed.label }).catch(() => {});
    cancelBrowserRetry(removed);
    browserSaid.delete(removed.label);
    browserEarly.delete(removed.label);
    browserAnnotations.delete(removed.label);
    // The record only hears the person. The native pane above has to go
    // whatever brought us here — a webview outliving its tab floats over
    // whatever the leaf shows next — but the LIST of what to put back on the
    // next start is the person's, and only their close edits it. Everything
    // else that takes a browser tab away (a checkout gone from the disk, a
    // project removed) leaves that list standing, so the address comes back.
    if (closed) rememberBrowserOpenTabs();
  }
  // A closed terminal tab must stay closed on the next visit, and an empty
  // set is how the file hears that. The dot goes back down with it — the last
  // shell leaving a workspace is what `inactive` means.
  if (removed?.kind === "term") {
    persistPaneLayouts(removed.worktree);
    scheduleAgentPaint(["cards"]);
  }
  // A closed document the same — and a group that folded moved the tree.
  if (STAGE_STORED_KINDS.has(removed?.kind)) persistStageLayouts();
}

/* ---- 도는 프로세스의 터미널은 닫기 전에 묻는다 (1-ft) ----
 *
 * Orca's `CloseTerminalDialog`: every terminal close first asks the PTY
 * whether a foreground job owns it. The hook state is a fallback only when a
 * platform/transport cannot answer that process-group question. The person's
 * "don't ask again" answer is a canonical setting shared by every window and
 * the next boot; this old localStorage key is read only for one-way migration. */
const CLOSE_RUNNING_KEY = "zerocode.confirm-close-running.v1";
let confirmCloseRunning = true;
let closeRunningLegacyMigrationStarted = false;
let legacySkipCloseRunningConfirm = false;
try {
  legacySkipCloseRunningConfirm = localStorage.getItem(CLOSE_RUNNING_KEY) === "off";
} catch {
  // Refusing legacy browser storage cannot weaken the canonical default.
}

function setConfirmCloseRunning(on) {
  confirmCloseRunning = on;
}

function forgetLegacyCloseRunningPreference() {
  try {
    localStorage.removeItem(CLOSE_RUNNING_KEY);
  } catch {
    // The canonical setting already owns the answer; stale browser storage is inert.
  }
}

const CLOSE_RUNNING_PROBE_TIMEOUT_MS = 4_000;

async function terminalCloseNeedsConfirmation(terms) {
  const ids = [...new Set(terms)].filter((term) => Number.isInteger(term));
  if (ids.length === 0) return false;
  const probes = Promise.allSettled(
    ids.map((term) => invoke("term_has_running_process", { term })),
  );
  const settled = await Promise.race([
    probes,
    new Promise((resolve) => setTimeout(() => resolve(null), CLOSE_RUNNING_PROBE_TIMEOUT_MS)),
  ]);
  if (Array.isArray(settled)) {
    if (settled.some((answer) => answer.status === "fulfilled" && answer.value === true)) {
      return true;
    }
    const allKnownIdle = settled.every(
      (answer) => answer.status === "fulfilled" && answer.value === false,
    );
    if (allKnownIdle) return false;
  }
  return ids.some((term) => hookStates.get(term) === "working");
}

async function askAboutRunning(tab, terms) {
  const agent = terms.some((term) => hookStates.get(term) === "working") || !!tab.agent;
  const name = tab.agent || t("board.agent", "에이전트");
  const answer = await askConfirm({
    title: agent
      ? t("terminal.stopAgentTitle", "이 에이전트를 중지할까요?")
      : t("terminal.stopProcessTitle", "실행 중인 프로세스를 중지할까요?"),
    body: agent
      ? t("terminal.stopAgentBody", "이 터미널을 닫으면 {{name}}이(가) 하던 작업이 중지됩니다.", {
        name,
      })
      : t("terminal.stopProcessBody", "이 터미널을 닫으면 실행 중인 프로세스가 중지됩니다."),
    confirm: agent
      ? t("terminal.stopAgent", "에이전트 중지")
      : t("terminal.closeRunning", "터미널 닫기"),
    deny: t("app.cancel", "취소"),
    danger: true,
    remember: t("terminal.stopAskAgain", "도는 터미널에 대해 다시 묻지 않기"),
  });
  // The line is read only on the answer that closes: "don't ask again" ticked
  // and then cancelled is a person deciding NOT to decide yet.
  if (answer === true && askRemembered()) {
    setConfirmCloseRunning(false);
    void commitSetting(
      "skip_close_terminal_with_running_process_confirm",
      "set_skip_close_terminal_with_running_process_confirm",
      { skip: true },
    ).then((snapshot) => {
      if (snapshot?.skip_close_terminal_with_running_process_confirm === true) {
        forgetLegacyCloseRunningPreference();
      }
    });
  }
  return answer === true;
}

/* One-shot passes for a close the person already confirmed — consumed by the
 * re-entry, so the next close of the same tab asks again. */
const closeConfirmed = new Set();
const closeChecks = new Set();

function closeTab(id) {
  const tab = tabs.find((held) => held.id === id);
  if (!tab) return;
  // A pinned tab closes only on purpose. Whether that purpose has to be
  // SAID is the switch (`guardPinnedTabClose` reading `confirmClosePinnedTab
  // ?? true`): on, the direct roads ask first and the answer routes back
  // through this same door with the pin lifted; off, the aim is taken as the
  // answer and the same door is re-entered straight away. Either way the pin
  // comes off here and not in the branches below, so what closes a pinned tab
  // is always the ordinary close — a document still asks about its unsaved
  // typing, and a terminal still ends its processes.
  if (tab.pinned) {
    if (!confirmClosePinnedTab) {
      tab.pinned = false;
      closeTab(id);
      return;
    }
    void askAboutPinned(tab).then((proceed) => {
      if (!proceed) return;
      tab.pinned = false;
      closeTab(id);
    });
    return;
  }
  // The lane tab exists because the lane does, so closing it closes the lane
  // — the same thing closing a terminal tab means anywhere else. The
  // `lane:closed` event takes the tab off the strip.
  if (tab.kind === "lane") {
    if (focusedId) closeLane(focusedId);
    return;
  }
  // A terminal tab IS its shell, so closing the tab ends the process — the
  // same thing closing a terminal means anywhere else. It does not go on the
  // reopen stack: ⌘⇧T would bring back a tab with a dead process behind it.
  if (tab.kind === "term") {
    // Measure every pane before killing any of them. Re-entry carries one
    // consumed pass so the query and dialog cannot duplicate themselves.
    if (confirmCloseRunning && !closeConfirmed.has(id)) {
      if (closeChecks.has(id)) return;
      const terms = paneLeaves(tab.layout);
      closeChecks.add(id);
      void terminalCloseNeedsConfirmation(terms).then(async (needsConfirmation) => {
        closeChecks.delete(id);
        if (!tabs.some((held) => held.id === id)) return;
        if (needsConfirmation && !(await askAboutRunning(tab, terms))) return;
        closeConfirmed.add(id);
        closeTab(id);
      });
      return;
    }
    closeChecks.delete(id);
    closeConfirmed.delete(id);
    // Every pane in it. A tab that was split holds several shells, and
    // leaving the ones that were not the tab's first behind would leak a
    // process with nothing on screen addressing it.
    for (const term of paneLeaves(tab.layout)) {
      invoke("close_term", { term }).catch(() => {});
      dropTermView(term);
    }
    dropTab(id);
    if (termFloat.hidden) keySink.focus();
    return;
  }
  // A browser tab has nothing unsaved to ask about, and nothing behind it to
  // reopen — the page dies with the pane (`dropTab` ends the webview), and a
  // ⌘⇧T that brought back the URL with none of the page's state would be a
  // different page wearing the same address.
  if (tab.kind === "browser") {
    // The one door a browser tab leaves by on purpose — the strip's ×, ⌘W,
    // 「탭 닫기」, and the agent's `zerocode-browser close`, which the window
    // routes here so that it walks exactly this road. Said out loud
    // (`closed`), because it is what tells the restore record to forget this
    // address; no other road may.
    dropTab(id, { closed: true });
    if (termFloat.hidden) keySink.focus();
    return;
  }
  // Documents go through the one door that asks about unsaved typing, and
  // onto the reopen stack only if they actually went.
  letGoOf(tab).then((released) => {
    if (!released) return;
    // A worker page closed by hand stays closed: the same run must not
    // reopen it (Orca's popup manners), and there is no path to re-read —
    // so it records its dismissal instead of joining the reopen stack.
    if (tab.kind === "worker") {
      // A wire session's page IS the session: closing it ends the child.
      if (tab.worker.wire) invoke("wire_stop", { id: tab.worker.wire }).catch(() => {});
      workerDismissed.add(tab.worker.id);
      if (termFloat.hidden) keySink.focus();
      return;
    }
    // Remembered so ⌘⇧T can bring it back. Only documents go on the stack:
    // the lane's tab exists because the lane does, and reopening it without
    // one would be a tab with nothing behind it. A reclaimed untitled file
    // has no behind either — the close deleted it.
    if (!tab.reclaimed) closedTabs.push({ kind: tab.kind, path: tab.path });
    if (termFloat.hidden) keySink.focus();
  });
}

/* Whether this tab is a document — something read from a path, which is the
 * thing that stops meaning what it meant when the checkout under it changes. */
function isDocument(tab) {
  return tab.kind === "file" || tab.kind === "diff" || tab.kind === "image";
}

/* The kinds Escape may take off the strip: the ones that are READ, and whose
 * whole state is the path they were read from.
 *
 * Escape used to ask `keyboardTarget() === null` instead — "is anything
 * listening" — which is true of every kind but a terminal and a lane. So the
 * key closed the browser pane somebody was signed into, the board and the
 * graph, and each browser close rewrote the restore record on the way out:
 * three panes and two pages gone in seven seconds, and nothing to come back
 * to on the next start (t-5453, 2026-09-20 16:13:03–09; the record went 3 → 2
 * → 1 → 0 as the panes went). The comment above the road always named the
 * right family — a file, a diff, a notebook — so it is asked as that family
 * rather than as the absence of a listener. A live surface leaves by its own
 * close door and no other.
 *
 * This is the same shape as the bug before it, one kind wider: the test then
 * was `kind !== "lane"`, which swept in terminals, and the answer was to name
 * what may go rather than what may not. */
const ESCAPE_CLOSES = new Set(
  ["file", "diff", "imagediff", "image", "mdview", "csv", "ipynb"],
);

/* The question in flight, and the promise waiting on its answer. */
let closingAsk = null;

/* Ask about a document with unsaved typing. Resolves `save`, `drop` or
 * `cancel` — a promise rather than a callback because the callers are loops:
 * closing one tab, and letting go of every document at once when the checkout
 * under them is about to change. */
function askAboutLosing(tab) {
  return new Promise((resolve) => {
    closingAsk = { tab, resolve };
    el("save-name").textContent = basename(tab.path);
    showModal(el("save-scrim"));
  });
}

function answerLosing(answer) {
  const asked = closingAsk;
  closingAsk = null;
  hideModal(el("save-scrim"));
  asked?.resolve(answer);
}

/* Take a document off the strip, asking first if it holds work that is only
 * in this window. Answers whether it actually went.
 *
 * The one door: closing a tab and changing the checkout under every tab are
 * the same question asked twice, and two copies of it would drift into one
 * that asks and one that does not. */
async function letGoOf(tab) {
  if (isDirty(tab)) {
    const answer = await askAboutLosing(tab);
    if (answer === "cancel") return false;
    // Kept only if the write landed. A save that failed and let go anyway
    // would lose the work while reporting that it had been kept.
    if (answer === "save" && !(await saveFile(tab))) return false;
    // Dropped first, so nothing downstream asks about it a second time.
    tab.draft = tab.text;
  }
  dropTab(tab.id);
  // Orca's `deleteUntouchedOnClose` (store-BgJxB0hr.js:40965): an untitled
  // markdown closed with nothing SAVED into it was never wanted, and trying
  // the door must cost no litter. The backend checks again that the file is
  // still empty and that it minted this path itself (1-fw). Its answer —
  // removed or kept — is WAITED for, because ⌘⇧T's stack is decided by it:
  // a path this close just deleted must not be offered back, while one the
  // backend kept (bytes arrived from elsewhere) still may be.
  if (tab.untitledFresh && (tab.text ?? "") === "") {
    tab.reclaimed = await invoke("delete_untitled_markdown", { path: tab.path }).catch(() => false);
  }
  return true;
}

/* Let go of every document, because what they were read from is about to
 * stop being what this window is looking at. Answers false if the person
 * said no to one of them — and then nothing has moved, because this runs
 * before the checkout does. */
async function letGoOfDocuments() {
  for (const tab of tabs.filter(isDocument)) {
    if (!(await letGoOf(tab))) return false;
  }
  return true;
}

el("save-keep").addEventListener("click", () => answerLosing("save"));
el("save-drop").addEventListener("click", () => answerLosing("drop"));
el("save-cancel").addEventListener("click", () => answerLosing("cancel"));

el("save-scrim").addEventListener("mousedown", (event) => {
  if (event.target === el("save-scrim")) answerLosing("cancel");
});

el("pin-close").addEventListener("click", () => answerPinned(true));
el("pin-cancel").addEventListener("click", () => answerPinned(false));

el("pin-scrim").addEventListener("mousedown", (event) => {
  if (event.target === el("pin-scrim")) answerPinned(false);
});

/* Tabs that were closed, oldest first. */
const closedTabs = [];

/* Reopen the last one, by asking for its contents again rather than restoring
 * what it held. A file closed ten minutes ago and reopened now should be the
 * file as it is, not the copy that happened to be in memory. */
function reopenClosedTab() {
  const tab = closedTabs.pop();
  if (!tab) return;
  // A commit's diff reopens AT its commit. `openDiff` would answer with the
  // working tree — the right file, the wrong moment.
  if (tab.kind === "diff" && tab.commit) void openCommitFileDiff(tab.commit, tab.path, tab.origin ?? null);
  else if (tab.kind === "diff") openDiff(tab.path);
  // `openPath` re-reads and re-decides the viewer, so a file that has become
  // something else since it was closed comes back as what it is now.
  else openPath(tab.path);
}

/* The document surfaces belong to a leaf, not to the window.
 *
 * There was one file view and one diff view because there was one leaf. With
 * two, both can be showing a file at once, so each leaf gets its own — cloned
 * from the first rather than written twice, which is also what keeps their
 * markup from drifting apart. */
/* The view templates, resolved once while the markup still holds them.
 * Group 0 can fold out of the tree like any other, and `el()` cannot find an
 * id inside a detached element — so the sources are taken by the hand here
 * rather than looked up when a clone is first needed. */
const docTemplates = new Map();

function docHost(group, kind) {
  const pane = groupOf(group);
  const key = `${kind}View`;
  if (pane[key]) return pane[key];
  let source = docTemplates.get(kind);
  if (!source) {
    source = el(`${kind}-view`);
    docTemplates.set(kind, source);
  }
  const host = group === 0 ? source : source.cloneNode(true);
  if (group !== 0) {
    // Ids are the window's, and a copy must not answer to them.
    for (const node of [host, ...host.querySelectorAll("[id]")]) node.removeAttribute("id");
    // The template is the FIRST leaf's live surface, and by the time a second
    // leaf is made that surface usually has a CodeMirror editor inside it. A
    // deep clone copies those nodes as dead markup — a second gutter and a
    // second document that no EditorView owns, sitting under the real one.
    // The mount point is emptied so `paintEditor` builds into a bare host.
    host.querySelector(".file-edit")?.replaceChildren();
    // The same trap one surface over: by the time a second leaf is made, the
    // first leaf's diff view usually has a live merge view in it, and a deep
    // clone copies those two editors as dead markup under the real one.
    host.querySelector(".diff-merge")?.replaceChildren();
    pane.el.appendChild(host);
  }
  pane[key] = host;
  // A template that was built rather than found in the markup has no parent
  // yet — the first leaf's copy lands here, exactly where a found template
  // already stood.
  if (!host.parentElement) pane.el.appendChild(host);
  DOC_VIEWS[kind]?.wire?.(host);
  // A view this window BUILDS is made during load, before a stored language
  // has been applied, so its words are the Korean fallback until something
  // re-reads their keys. `applyLocale` is that something, and this is the one
  // place every built surface passes through — the markup-declared views get
  // the same treatment at boot from the document-wide sweep.
  applyLocale(host);
  return host;
}

/* Draw a tab's contents into the leaf that holds it. Terminals and lanes are
 * not here: those surfaces are painted by the frames arriving from their own
 * processes, and a repaint from this side would fight them. */
function paintDoc(tab) {
  // A leaf that has moved on from its diff drops the two editors behind it.
  // The file editor is held per leaf on purpose — a tab coming back wants its
  // undo stack — but a merge view is rebuilt from the documents either way, so
  // keeping one alive under a `hidden` attribute buys a measure loop and
  // nothing else.
  if (tab.kind !== "diff") dropDiffView(tab.pane);
  // Through the registry, not a chain of names beside it: a painter listed
  // here and a kind listed in `DOC_VIEWS` were two lists that had to agree,
  // and a surface the stage paints but never hides — or hides but never
  // shows — is exactly what their disagreement looks like.
  //
  // The return value is dropped on purpose. The board's painter is async —
  // it asks the backend what every agent is doing — and a stage change must
  // not wait on that; it fills in when the answer lands.
  void DOC_VIEWS[tab.kind]?.paint?.(tab);
}

/* Where a tab's content is drawn \u2014 the leaf that holds the tab. */
function hostOf(tab) {
  if (tab.kind === "lane") return stageView.host;
  // The tab's pane host, not a shell's screen: a terminal tab can be holding
  // several shells, and what the stage shows or hides is all of them at once.
  if (tab.kind === "term") return paneHosts.get(tab.id) ?? null;
  return docHost(tab.pane, tab.kind);
}

function activeTabIn(index) {
  const held = paneTabs(index);
  return held.find((tab) => tab.id === activeTabId) ?? held[held.length - 1] ?? null;
}

/* What went wrong with nothing on the stage to put it on, or `null`.
 *
 * Declared beside the two functions that clear and read it rather than beside
 * `paintStagePlaceholder` far below, so nothing can reach it before the
 * binding exists. */
let stageError = null;

function updateStage() {
  // The containers FIRST, because the tail of this function measures them.
  //
  // A shell is told how much room it has from the loop below, and the number
  // it reads is a box the split tree has to already be on screen for. Every
  // road that folds or grows a leaf changes the tree and then calls this —
  // and `setActiveTab` called it BEFORE `renderTabs`, which is where the
  // stage is drawn. So a resize measured the geometry the stage was leaving,
  // not the one it was arriving at: opening a mirror beside a shell told the
  // shell nothing (its old full width still matched what it had been told,
  // and the dedupe swallowed it), and closing that mirror told it the HALF
  // width it no longer had — which the dedupe then remembered, so no later
  // telling of the honest width was ever sent. The shell stayed cut at half
  // its pane until the window itself was dragged (live report 2026-08-25,
  // "ios를 켯다가 닫으면 반이 짤려잇음").
  //
  // Rendered from here rather than fixed at that one call site because there
  // are twenty roads into this function and each of them measures. The render
  // is identity-guarded on the tree, so a call that moves no leaf costs one
  // comparison.
  renderStage();
  const wasHidden = stageView.host.hidden;
  // Everything the stage can draw, hidden unless a leaf is showing it.
  const shown = new Map();
  for (const group of stageGroups()) {
    const active = activeTabIn(group);
    if (!active) continue;
    // A tab restored from the file has no shells until it is LOOKED at, and
    // this is the moment it is: Orca connects a pane when the pane renders,
    // and nothing before that starts a process. The wake paints when its
    // shells arrive — one round trip, not a frame — and until then the leaf
    // is empty rather than showing a screen with nothing behind it.
    if (active.asleep) {
      void wakeStoredTab(active);
      continue;
    }
    if (active.kind === "lane" && focusedId === null) continue;
    const host = hostOf(active);
    if (host) shown.set(host, { group, tab: active });
  }

  // Every surface this stage can draw, whether or not a tab still claims it.
  //
  // This used to walk `tabs`, and that was the bug behind two reports: a
  // surface whose tab had been taken off the strip was never told to hide, so
  // it stayed on screen with nothing addressing it — a terminal nobody could
  // type into, a file left covering the terminal that replaced it. A tab is
  // how you reach a surface; it is not what makes the surface exist.
  const surfaces = new Set([stageView.host]);
  // The pane hosts, not the shells' screens. A screen lives inside the host
  // of the tab that holds it, so hiding the host hides every pane in it — and
  // a screen hidden on its own would leave its slot on screen as a hole.
  for (const host of paneHosts.values()) surfaces.add(host);
  // The document surfaces a leaf has built, by the key `docHost` files them
  // under. Derived from the tab kinds rather than listed by hand: a hardcoded
  // list is a list that a new surface is added to LAST, and the symptom is a
  // view that never gets told to hide — which is what a board tab did until
  // this stopped being three names in a row.
  for (const pane of groups.values()) {
    for (const kind of DOC_KINDS) {
      const held = pane[`${kind}View`];
      if (held) surfaces.add(held);
    }
  }

  for (const host of surfaces) {
    const on = shown.get(host);
    host.hidden = on === undefined;
    // A surface follows its tab into the leaf that holds it. Moving the node
    // rather than drawing a second one is what keeps a terminal's scrollback
    // and a file's scroll position across a split.
    if (on && host.parentElement !== groupOf(on.group).el) {
      groupOf(on.group).el.appendChild(host);
    }
  }

  // And then down one level, into the shells a terminal tab holds.
  //
  // A view paints only while its own host is visible, which is what keeps
  // background tabs off the thread delivering the next keystroke. Hiding the
  // tab's pane host alone does not clear that flag on the screens inside it —
  // it takes them off the screen and leaves them believing they are on it.
  // The first cut of the split did the same thing in reverse: every screen
  // stayed `hidden` from birth, so the panes laid out correctly and then drew
  // nothing at all.
  //
  // Driven from the pane hosts and their slots rather than from `tabs`, for
  // the reason the pass above exists at all: a host whose tab has gone is
  // still a host, and it has to be able to quiet the shells inside it without
  // a tab left to ask.
  for (const host of paneHosts.values()) {
    const painting = !host.hidden;
    for (const slot of host.querySelectorAll(".pane-slot")) {
      const term = Number(slot.dataset.term);
      // The slot still names this shell, but the peek has borrowed its screen
      // out of it — and the borrowed screen is the one being read. Hiding it
      // from here would blank a dialog somebody is looking at, on nothing more
      // than a stage repaint behind it.
      if (term === peekTerm && !el("peek-scrim").hidden) continue;
      const view = termViews.get(term);
      // The pane's conversation, when the person turned the screen over to it:
      // it stands in the same slot and the screen hides — and a hidden screen,
      // like a hidden page, paints nothing (`placePaneChat`).
      const chat = painting && paneChatOn(term);
      if (view) view.host.hidden = !painting || chat;
      placePaneChat(term, slot, chat);
    }
  }

  // One terminal tab on screen per leaf. The others keep their views and their
  // processes — a hidden shell is still running, and hiding is what makes
  // coming back to it instant rather than a fresh repaint.
  for (const [term, view] of termViews) {
    if (!termShown(view)) continue;
    view.measure();
    // It tracked its screen while hidden without touching the DOM. This is
    // where that screen comes back, and it costs nothing for a view that was
    // visible all along.
    view.repaint();
    resizeTermTab(term);
  }

  // Every leaf draws what it is showing, not only the tab that was clicked.
  // Painting just the active one left the leaf a tab had *departed* still
  // showing it: dropping a second file on the lower edge gave two leaves
  // drawing the same document, because the one left behind was never told
  // that what it holds had changed.
  for (const { tab } of shown.values()) paintDoc(tab);

  /* Empty belongs to the workspace on stage, not to the window's global tab
   * registry.
   *
   * Tabs from every checkout stay alive in `tabs`; `paneTabs` is the one
   * filter that says which of them this workspace can draw. Reading the
   * global length here hid the placeholder whenever some OTHER checkout had
   * a tab, leaving this checkout as the literal black stage (`watch main:
   * []`). This function has twenty-two callers, and every stage transition
   * already passes through it, so the correction belongs at this one sink. */
  const hasWorkspaceTab = stageGroups().some((group) => paneTabs(group).length > 0);
  // A tab opened over it, so a failure left on the empty stage is no longer
  // what this element is for — and the element is redrawn NOW, not left
  // holding the stale error for the next reveal to show.
  if (hasWorkspaceTab && stageError !== null) {
    stageError = null;
    paintStagePlaceholder();
  }
  // The empty sentence can name a detached agent from THIS checkout. Repaint
  // while empty so moving between two empty workspaces cannot keep the first
  // one's agent and action on the second one's stage. No terminal frame takes
  // this road: the first workspace tab hides the element again.
  if (!hasWorkspaceTab) paintStagePlaceholder();
  placeholder.hidden = hasWorkspaceTab;
  // Metrics measured while the host was display:none came back zero, so the
  // first reveal is the first honest measurement.
  if (!stageView.host.hidden && wasHidden) stageView.measure();
  // 무엇이 보이는지가 방금 정해졌으므로, 무엇을 읽는지도 방금 정해졌다. 탭
  // 전환·판 나누기·판 닫기·워크스페이스 전환이 전부 이 문을 지나므로 선언은
  // 여기 한 곳이면 되고, 아무것도 안 움직였으면 아무것도 보내지 않는다.
  void syncWatchedTerms();
  // And the native browser panes follow what the stage just decided — same
  // door, same reason, same nothing-moved-nothing-sent economy.
  syncBrowserPanes();
  syncEmulatorStreamVisibility();
}

/* Leave the app. The backend decides whether a URL is one a browser may be
 * handed — this side does not, deliberately: the strings that reach here come
 * from a repository's CI, and the check has to sit where it cannot be skipped
 * by a caller who forgot. */
function openExternal(url) {
  invoke("open_url", { url }).catch((error) => showError(String(error)));
}

/* Which clicks the terminal owns — Orca's three predicates, verbatim
 * (terminal-link-activation.ts). A direct opening keeps shift (⇧⌘ is the
 * table's alternate) but never alt; the bare-click popover tolerates no
 * modifier at all; anything else — mac Ctrl+click, a lone Alt or Shift,
 * ⌘Alt — is not the terminal's gesture and is yielded to the system whole,
 * preventDefault included (terminal-web-link-click.ts:30-32). */
function terminalLinkDirectActivation(event) {
  return event.button === 0 && !event.altKey && hasPrimaryModifier(event); // :12-19
}

function terminalLinkActionActivation(event) {
  return event.button === 0 &&
    !event.altKey && !event.shiftKey && !event.metaKey && !event.ctrlKey; // :21-30
}

function terminalOwnsLinkGesture(event) {
  return terminalLinkDirectActivation(event) || terminalLinkActionActivation(event); // :32-34
}

/* The bare-click popover's gatekeeper — Orca's pointer gesture, verbatim
 * (terminal-link-pointer-gesture.ts; only the action road asks it,
 * terminal-link-action-request.ts:50-56 — a ⌘ direct opening never does).
 * Armed at mousedown when the press is an owned gesture, spoiled by a drag
 * past four pixels or by a selection that stood when the press landed (that
 * click is the selection's ending, not a request), cancelled by blur.
 * mouseup clears a microtask later so the same tick's click can still read
 * it. One set for the window, not one per view: the selection it reads is
 * the window's own, so per-view copies would only duplicate state. */
/* A selection standing in THIS screen, which is the only one whose ending a
 * link click could be.
 *
 * The original asks the terminal the gesture was installed on —
 * `terminal.hasSelection()` (terminal-link-pointer-gesture.ts:33,58) — which
 * is per-screen by construction, because xterm owns its own selection. So
 * does this window's now: each screen holds a selection model of its own
 * (`terminalSelectionStandsIn`, ui/shell-term.js), so the question is asked
 * of the screen the press landed on and of nothing else.
 *
 * Asking the WINDOW instead, which is what this once did, handed one
 * selection anywhere — a filename in the tree, a line left highlighted in a
 * diff, a word in the rail — the power to swallow every link click in every
 * terminal until somebody cleared it. Reported live, with a URL an agent had
 * printed: "클릭해도 모달이 뜨지 않아". Read at the press in the CAPTURE phase,
 * before the screen's own handler starts the new selection the press begins. */
function selectionStandsInside(node) {
  return terminalSelectionStandsIn(node);
}

const LINK_DRAG_THRESHOLD_PX = 4; // terminal-link-pointer-gesture.ts:4
let termLinkGesture = null;
function clearTermLinkGesture() {
  termLinkGesture = null;
}
document.addEventListener("mousedown", (event) => {
  if (!terminalOwnsLinkGesture(event)) {
    clearTermLinkGesture();
    return;
  }
  termLinkGesture = {
    x: event.clientX,
    y: event.clientY,
    hadSelection: selectionStandsInside(event.target?.closest?.(".terminal")),
    moved: false,
    // 이 누름이 낳는 클릭 하나만 그것을 쓴다.
    spent: false,
    // The address the press landed on, kept because the SPAN will not be
    // there to ask at click time. A TUI redraws on its own clock — a spinner,
    // a new line, its answer to this very press — and a repaint replaces the
    // run nodes under the pointer; the click then arrives with a target that
    // carries no `data-href` and a URL somebody aimed at does nothing.
    // Reported live: clicking a localhost URL an agent had printed opened no
    // chooser at all. Orca cannot have this bug because its links are never
    // DOM — a provider answers with a CELL RANGE and xterm hit-tests the
    // pointer against it (`provideLinks` → `range`,
    // terminal-handle-links.ts:162-200). Holding the press's own answer is
    // that contract in this window's spelling, and it is the more honest one
    // besides: what a person pressed is what they meant, even if the screen
    // moved underneath before they let go.
    href: event.target?.closest?.("[data-href]")?.dataset.href ?? "",
  };
}, true);
document.addEventListener("mousemove", (event) => {
  if (termLinkGesture !== null &&
    Math.hypot(event.clientX - termLinkGesture.x, event.clientY - termLinkGesture.y) >
      LINK_DRAG_THRESHOLD_PX) {
    termLinkGesture.moved = true;
  }
});
/* 누름은 다음 누름까지 살고, 클릭 하나가 그것을 쓴다.
 *
 * 여기 있던 코드는 원본 그대로였다 — `queueMicrotask(clear)` on mouseup
 * (terminal-link-pointer-gesture.ts, `handleMouseUp`). 값은 맞게 옮겼는데
 * **묻는 층이 달랐다**: 원본에서 이 질문을 하는 곳은 xterm 의 `_handleMouseUp`
 * 이고(그 안에서 `link.activate(e, …)` 를 부른다) 우리 것은 `click` 이다. 실제
 * 입력에서 mouseup 과 click 은 **다른 태스크**라 그 사이에 마이크로태스크가 돌고,
 * 그래서 플레인 클릭의 팝오버는 이 창에서 **한 번도** 열릴 수 없었다. 원본은
 * 마이크로태스크가 큐에 들어가기도 전에 이미 물었으므로 같은 코드가 거기서는
 * 옳다. 실측(실제 입력): mousedown → mouseup → microtask → click. 합성 이벤트
 * 셋을 한 태스크에 넣으면 순서가 뒤집혀(… → click → microtask) 이 문은 시험에
 * 보이지 않는다 — 그래서 이 조각의 단면만은 `page.mouse` 를 쓴다.
 *
 * 「지운다」를 「썼다」로 바꾼 이유: 수명을 다음 누름까지 늘리기만 하면 한 번의
 * 누름이 여러 클릭을 열 수 있다. 원본의 뜻은 「mouseup 이 낳는 그 활성화 하나」
 * 이므로, 클릭이 제스처를 읽고 나면 그것을 쓴 것으로 표시한다. */
document.addEventListener("click", () => {
  if (termLinkGesture !== null) termLinkGesture.spent = true;
});
window.addEventListener("blur", clearTermLinkGesture);

/* What the press was on, for a click whose node has since been repainted
   away. Read only while the gesture stands — mouseup clears it a microtask
   later, so the same tick's click still has it and the next press starts
   from nothing. */
function pressedLinkHref() {
  return termLinkGesture?.href ?? "";
}

function canRequestTermLinkAction(host) {
  return termLinkGesture !== null && !termLinkGesture.moved && !termLinkGesture.spent &&
    !termLinkGesture.hadSelection && !selectionStandsInside(host);
}

/* One owner for every user-content http(s) link.
 *
 * The terminal, Markdown renderer and editor differ only in how they discover
 * a URL. They do not get to make three routing decisions: Orca stores one
 * default and one modifier rule for all three surfaces. The default shortcut
 * is intentionally asymmetric — Shift+Cmd/Ctrl always means the system
 * browser until the explicit "modifier inverts" preference is enabled. */
function canonicalHttpLink(raw) {
  try {
    const url = new URL(String(raw ?? "").trim());
    return url.protocol === "http:" || url.protocol === "https:" ? url.href : null;
  } catch {
    return null;
  }
}

function browserLinkUsesBuiltIn(event) {
  const configured = browserPrefs.open_links_in_app === true;
  const shiftedPrimary = event?.shiftKey === true && hasPrimaryModifier(event);
  // 실측 terminalHttpLinkActionDestinationsFor: ⇧는 표의 alternate — 언제나
  // **반대 목적지**다. 이 표는 터미널의 것이고, 문서 표면(Markdown·에디터·
  // 포트 행)은 아래 documentLinkUsesBuiltIn의 수식키 규칙을 탄다.
  return shiftedPrimary ? !configured : configured;
}

/* 실측 resolveModifierRouting(http-link-routing.ts): 문서 표면의 ⇧⌘는 기본
 * (inverts=false)에서 **항상 시스템 브라우저**로 가는 한 방향 탈출구다 —
 * `open_links_in_app_modifier_inverts`를 켠 사람에게만 반대 목적지가 된다. */
function documentLinkUsesBuiltIn(event) {
  const configured = browserPrefs.open_links_in_app === true;
  const shiftedPrimary = event?.shiftKey === true && hasPrimaryModifier(event);
  if (!shiftedPrimary) return configured;
  const inverts = browserPrefs.open_links_in_app_modifier_inverts === true;
  return inverts ? !configured : false;
}

/* Which machine a terminal link speaks from — Orca's sourceOwner folded to
 * this window's two kinds (resolveTerminalHttpLinkSourceOwner,
 * terminal-http-link-source-owner.ts:11-45; our only remote is SSH, so
 * local/ssh — the fold is recorded in docs/plans/link-source-owner.md).
 * `remoteHost` is the mark both SSH roads already set on the tab — an SSH
 * terminal and a remote workspace's terminal — the same authority the
 * board's host axis reads (its comment names both roads). */
function termLinkSpokenOverSsh(term) {
  return term !== undefined && Boolean(tabOfTerm(term)?.remoteHost);
}

/* 실측 handleTerminalHttpLink의 그대로: ⌘클릭은 표의 primary — Link Routing
 * 설정이 정한 목적지 — 로 **직행**하고, ⇧⌘는 alternate — 반대 목적지 — 다.
 * 묻는 것은 일반 클릭(chooser)의 액션 팝오버뿐: 두 목적지가 행으로 선다
 * (실측 TerminalLinkActionPopover). 첫-클릭 선호 다이얼로그는 표가 없는
 * 보조 표면(대시보드 미리보기 터미널)의 것이라 이 창에는 발원이 없다 —
 * `open_links_in_app_prompted`는 설정 토글이 기록하는 백엔드 자산으로만
 * 남는다(실측 BrowserLinkRoutingSetting: 토글이 prompted를 함께 적는다). */
/* `beside` is off by default because that is the original's answer: an
   in-app link open is `createBrowserTab(worktreeId, url, { activate: true })`
   — a TAB in the workspace already in front (http-link-routing.ts:160-176),
   never a pane laid over what stands beside it. Ours stood one beside, and
   `standBesideFocused` REUSES an existing right-hand seat when there is one:
   clicking a URL took over the terminal next door, which is how it was
   reported. The callers that genuinely mean a second surface — the + palette,
   a dev server the scanner found, the browser's own context menu — still say
   so. */
function routeHttpLink(raw, event, { beside = false, chooser = false, table = false, sshSource = false } = {}) {
  const url = canonicalHttpLink(raw);
  if (url === null) return false;
  // 실측 terminalHttpLinkActionDestinationsFor(:45-52): 이 기계가 아닌 곳에서
  // 말해진 링크에는 in-app 문이 서지 않는다 — 표는 {primary:'system'} 단독
  // (⇧⌘의 alternate 자체가 없다), 라우터도 설정과 무관하게 시스템으로 간다
  // (openHttpLink의 sourceIsLocal 게이트, http-link-routing.ts:129,161).
  if (sshSource) {
    if (chooser && event) {
      if (browserPrefs.terminal_link_action_popover === false) return false;
      openLinkMenu(url, event.clientX ?? 0, event.clientY ?? 0, beside, { systemOnly: true });
      return true;
    }
    // ⌘도 ⇧⌘도 같은 한 문이다 — 반대 목적지가 표에 없으니 뒤집을 것도 없다.
    openExternal(url);
    return true;
  }
  const shifted = event?.shiftKey === true && hasPrimaryModifier(event);
  if (!shifted && chooser && event) {
    // 실측 getLinkActionContext: 팝오버 설정이 꺼져 있으면 컨텍스트가 null이
    // 되어 일반 클릭은 아무 문도 열지 않는다 — ⌘클릭만 남는다.
    if (browserPrefs.terminal_link_action_popover === false) return false;
    openLinkMenu(url, event.clientX ?? 0, event.clientY ?? 0, beside);
    return true;
  }
  const builtIn = table ? browserLinkUsesBuiltIn(event) : documentLinkUsesBuiltIn(event);
  if (builtIn) {
    void openBrowserTab(url, { beside });
  } else {
    openExternal(url);
  }
  return true;
}

/* 링크 선택 팝오버(1-g62): URL 전문이 머리에 서고, 행 둘이 문 둘을 —
 * 시스템과 이 창의 브라우저 — 수식키 지름길 표기와 함께 세운다. */
function closeLinkMenu() {
  closing(el("link-menu"));
}

/* 일반 클릭의 액션 팝오버 — 실측 TerminalLinkActionPopover: 설정이 정한
 * 목적지가 첫 행(primary)로, 반대 목적지가 그 아래(alternate)로 선다.
 * 목적지를 press로 말했으니 chord 표기는 없다(실측에도 없다). */
function openLinkMenu(url, x, y, beside, { systemOnly = false } = {}) {
  const pop = el("link-menu");
  const head = el("link-menu-url");
  head.textContent = url;
  head.dataset.tip = url;
  const host = el("link-menu-body");
  host.replaceChildren();
  const builtin = {
    said: t("links.builtin", "ZeroCode 브라우저"),
    glyph: "globe",
    close: closeLinkMenu,
    run: () => void openBrowserTab(url, { beside }),
  };
  const system = {
    said: t("links.system", "시스템 브라우저"),
    glyph: "external",
    close: closeLinkMenu,
    run: () => openExternal(url),
  };
  // 실측 ActionRow: 각 행의 오른쪽에 그 행을 곧장 여는 수식키가 선다 —
  // primary가 ⌘ Click, alternate가 ⇧⌘ Click.
  const first = browserPrefs.open_links_in_app === true;
  const primaryChord = usesCommandModifier ? "⌘ Click" : "Ctrl+Click";
  const alternateChord = usesCommandModifier ? "⇧⌘ Click" : "Shift+Ctrl+Click";
  if (systemOnly) {
    // 이 기계가 아닌 곳의 링크는 문 하나다 — {primary:'system'} 단독
    // (terminalHttpLinkActionDestinationsFor :47-49). ⌘ 캡은 그대로:
    // 직행도 같은 문으로 간다.
    menuRow(host, { ...system, chord: primaryChord });
  } else {
    menuRow(host, { ...(first ? builtin : system), chord: primaryChord });
    menuRow(host, { ...(first ? system : builtin), chord: alternateChord });
  }
  showing(pop);
  placeLinkMenu(pop, x, y);
}

/* 한 자리잡기 — URL 팝과 파일 팝이 같은 판을 쓰니 같은 손이 앉힌다. */
function placeLinkMenu(pop, x, y) {
  const wide = pop.offsetWidth;
  const tall = pop.offsetHeight;
  pop.style.left = `${Math.max(8, Math.min(x, window.innerWidth - wide - 8))}px`;
  pop.style.top = `${Math.max(8, Math.min(y, window.innerHeight - tall - 8))}px`;
}

/* 경로 링크의 무수식 팝오버 — 실측 handleTerminalFileLink(:41-96): 알려진
 * 워크트리 루트면 primary가 "워크스페이스 전환"이고 alternate는 파일
 * 관리자(맥은 Finder 낱말), 보통 파일이면 primary가 편집기·alternate가
 * 시스템 기본 앱이다. Orca는 이 낱말들을 en에만 두고 세 로케일을 영어
 * 폴백으로 내보낸다 — 우리는 네 카탈로그를 다 채운다(우위). */
function openFileLinkMenu(href, x, y) {
  const pop = el("link-menu");
  const named = href.match(/^(.*?)(?::(\d+))?(?::(\d+))?$/);
  const path = named ? named[1] : href;
  const line = named?.[2] ? Number(named[2]) : undefined;
  const head = el("link-menu-url");
  head.textContent = href;
  head.dataset.tip = href;
  const host = el("link-menu-body");
  host.replaceChildren();
  const absolute = path.startsWith("/") ? path : `${activeWorktreePath}/${path}`;
  const worktree = worktreeAt(absolute);
  const primaryChord = usesCommandModifier ? "⌘ Click" : "Ctrl+Click";
  const alternateChord = usesCommandModifier ? "⇧⌘ Click" : "Shift+Ctrl+Click";
  if (worktree) {
    menuRow(host, {
      said: t("links.switchWorkspace", "워크스페이스 전환"),
      glyph: "folder",
      close: closeLinkMenu,
      run: () => void activateWorktree(worktree),
      chord: primaryChord,
    });
    menuRow(host, {
      said: usesCommandModifier
        ? t("links.openInFinder", "Finder에서 열기")
        : t("links.openFolder", "폴더 열기"),
      glyph: "external",
      close: closeLinkMenu,
      run: () => void invoke("fs_reveal", { path: absolute }).catch((error) => showError(String(error))),
      chord: alternateChord,
    });
  } else {
    menuRow(host, {
      said: t("links.openFile", "파일 열기"),
      glyph: "file",
      close: closeLinkMenu,
      run: () => void openTermLink(path, line),
      chord: primaryChord,
    });
    menuRow(host, {
      said: t("links.openWithDefaultApp", "기본 앱으로 열기"),
      glyph: "external",
      close: closeLinkMenu,
      run: () => void invoke("fs_open_default", { path: absolute }).catch((error) => showError(String(error))),
      chord: alternateChord,
    });
  }
  showing(pop);
  placeLinkMenu(pop, x, y);
}

dismissable(el("link-menu"), closeLinkMenu);
el("link-menu-copy").innerHTML = icon("copy");
el("link-menu-settings").innerHTML = icon("gear");
el("link-menu-copy").addEventListener("click", async () => {
  // 실측 copyDestination: 복사는 팝오버를 닫지 않고 결과만 말한다.
  if (await clipboardText.write(el("link-menu-url").textContent)) {
    toast(t("links.copied", "링크를 복사했습니다."), "done");
  }
});
el("link-menu-settings").addEventListener("click", () => {
  closeLinkMenu();
  setSettingsOpen(true, "browser-terminal-link-actions");
});


function showError(error) {
  if (tabs.length === 0) {
    // Held rather than written: `paintStagePlaceholder` is the one thing that
    // draws this element and a language change runs it, so a failure written
    // behind its back is erased the moment somebody switches language.
    stageError = String(error);
    paintStagePlaceholder();
    updateStage();
  } else {
    // With a tab open there is no placeholder to write on, and this used to
    // stop at the console — a failure the person was never told about. The
    // origin of the call may be anywhere or already gone, which is precisely
    // the case the toast layer exists for.
    console.error(error);
    toast(String(error), "halt");
  }
}
