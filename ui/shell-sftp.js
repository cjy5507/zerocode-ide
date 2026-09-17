/* SSH and SFTP share a native connection. The file manager holds paths and
 * job receipts only; file bytes never pass through the renderer transfer queue. */
const sftpConnections = new Map();
const sftpJobs = new Map();
let sftpView = null;
let sftpSources = [];
let sftpOpenGeneration = 0;
const SFTP_PAGE_SIZE = 500;
let sftpClipboard = null;
let sftpActiveSide = "remote";
let sftpEditor = null;
let sftpPromptCancel = null;
let sftpTransferCancel = null;
const SFTP_CONFLICT_CHOICES = [["ask", "ask"], ["overwrite", "overwrite"], ["skip", "skip"], ["resume", "resume"], ["newer", "newer"], ["rename", "keepBoth"]];
const sftpPanes = {
  local: { hostId: null, path: "", entries: [], selected: new Set(), generation: 0 },
  remote: { hostId: null, path: "", entries: [], selected: new Set(), generation: 0 },
};
const sftpWords = {
  title: () => t("sftp.title", "SFTP 파일"),
  local: () => t("sftp.local", "로컬 파일"), remote: () => t("sftp.remote", "원격 파일"),
  connect: () => t("sftp.connect", "연결"), reconnect: () => t("sftp.reconnect", "다시 연결"),
  disconnect: () => t("sftp.disconnect", "연결 끊기"), close: () => t("app.close", "닫기"),
  refresh: () => t("sftp.refresh", "새로 고침"), up: () => t("sftp.up", "상위 폴더"),
  mkdir: () => t("sftp.mkdir", "새 폴더"), newFile: () => t("sftp.newFile", "새 파일"),
  rename: () => t("sftp.rename", "이름 변경"), remove: () => t("sftp.remove", "삭제"),
  edit: () => t("sftp.edit", "편집"), chmod: () => t("sftp.chmod", "파일 권한"),
  copy: () => t("sftp.copy", "복사"), cut: () => t("sftp.cut", "잘라내기"), paste: () => t("sftp.paste", "붙여넣기"),
  upload: () => t("sftp.upload", "업로드 →"),
  uploadFiles: () => t("sftp.uploadFiles", "파일 선택해 업로드"), download: () => t("sftp.download", "← 다운로드"),
  path: () => t("sftp.path", "경로"), name: () => t("sftp.name", "이름"), size: () => t("sftp.size", "크기"),
  modified: () => t("sftp.modified", "수정 시간"), permissions: () => t("sftp.permissions", "권한"),
  filter: () => t("sftp.filter", "파일 이름 필터"), search: () => t("sftp.search", "하위 폴더 검색"),
  hidden: () => t("sftp.hidden", "숨김 파일"), compare: () => t("sftp.compare", "폴더 비교"),
  sync: () => t("sftp.sync", "같은 하위 폴더로 이동"), bookmark: () => t("sftp.bookmark", "폴더 저장"),
  bookmarks: () => t("sftp.bookmarks", "저장한 원격 폴더"),
  queue: () => t("sftp.queue", "전송 대기열"), clear: () => t("sftp.clear", "완료 기록 비우기"),
  conflict: () => t("sftp.conflict", "같은 이름의 파일"),
  ask: () => t("sftp.ask", "확인 필요"), overwrite: () => t("sftp.overwrite", "덮어쓰기"),
  skip: () => t("sftp.skip", "건너뛰기"), resume: () => t("sftp.resume", "이어받기"),
  newer: () => t("sftp.newer", "원본이 최신일 때"), keepBoth: () => t("sftp.keepBoth", "둘 다 보관"),
  speed: () => t("sftp.speed", "속도 제한 (KiB/s, 0=무제한)"),
  queued: () => t("sftp.queued", "대기 중"), running: () => t("sftp.running", "전송 중"),
  paused: () => t("sftp.paused", "일시 정지"), completed: () => t("sftp.completed", "완료"),
  cancelled: () => t("sftp.cancelled", "취소됨"), cancelling: () => t("sftp.cancelling", "취소 중"),
  failed: () => t("sftp.failed", "실패"), retry: () => t("sftp.retry", "재시도"),
  pause: () => t("sftp.pause", "일시 정지"), continue: () => t("sftp.continue", "계속"),
  cancel: () => t("app.cancel", "취소"), save: () => t("app.save", "저장"),
  connected: () => t("sftp.connected", "SFTP 연결됨"), connecting: () => t("sftp.connecting", "SFTP 연결 중…"),
  disconnected: () => t("sftp.disconnected", "연결 안 됨"), error: () => t("sftp.error", "SFTP 연결 실패"),
  empty: () => t("sftp.empty", "표시할 파일이 없습니다."),
  queueEmpty: () => t("sftp.queueEmpty", "파일을 선택해 업로드·다운로드하거나 다른 패널로 드래그하세요."),
  recursive: () => t("sftp.recursive", "하위 폴더와 파일에도 적용"),
  searchNeedsText: () => t("sftp.searchNeedsText", "검색어를 입력하세요."),
};

function sftpNode(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (typeof text === "function") say(node, text);
  else if (text != null) node.textContent = String(text);
  return node;
}
function sftpButton(word, action, className = "") {
  const button = sftpNode("button", `btn ${className}`, sftpWords[word]);
  button.type = "button"; button.dataset.action = word;
  button.addEventListener("click", () => void sftpRun(action));
  return button;
}
async function sftpRun(action) {
  try { sftpError(""); return await action(); }
  catch (error) { sftpError(String(error)); return null; }
}
function sftpError(message) {
  if (!sftpView) return;
  const node = sftpView.querySelector(".sftp-error");
  node.textContent = message; node.hidden = !message;
}
function sftpLocation(side, path = sftpPanes[side].path) { return { hostId: sftpPanes[side].hostId, path }; }
function sftpSelected(side) { const pane = sftpPanes[side]; return pane.entries.filter((entry) => pane.selected.has(entry.path)); }
function sftpJoin(path, name) { return `${path.replace(/\/$/, "")}/${name}`; }

function createSftpView() {
  const view = sftpNode("section", "sftp-manager");
  view.id = "sftp-manager"; view.hidden = true;
  view.setAttribute("role", "dialog"); view.setAttribute("aria-modal", "true"); view.setAttribute("aria-labelledby", "sftp-title");
  const header = sftpNode("header", "sftp-head");
  const title = sftpNode("h2", "", sftpWords.title); title.id = "sftp-title"; header.append(title);
  const host = sftpNode("select", "field"); host.id = "sftp-host"; host.setAttribute("aria-label", sftpWords.remote());
  host.addEventListener("change", () => void sftpRun(() => openSftpManager({ id: host.value })));
  header.append(host);
  const status = sftpNode("span", "sftp-connection"); status.setAttribute("role", "status"); header.append(status);
  header.append(sftpButton("reconnect", async () => {
    const id = sftpPanes.remote.hostId;
    for (const report of await invoke("sftp_connect", { hostId: id, reconnect: true })) sftpConnections.set(report.hostId, report);
    await sftpLoad("remote"); paintSftpConnection();
  }), sftpButton("disconnect", async () => {
    await invoke("sftp_disconnect", { hostId: sftpPanes.remote.hostId });
    sftpConnections.delete(sftpPanes.remote.hostId); paintSftpConnection();
  }), sftpButton("close", closeSftpManager));
  view.append(header);
  const error = sftpNode("p", "sftp-error"); error.setAttribute("role", "alert"); error.hidden = true; view.append(error);
  const options = sftpNode("div", "sftp-options");
  for (const word of ["hidden", "compare", "sync"]) {
    const label = sftpNode("label", "sftp-check");
    const input = sftpNode("input"); input.type = "checkbox"; input.id = `sftp-${word}`;
    input.addEventListener("change", () => { paintSftpPane("local"); paintSftpPane("remote"); });
    label.append(input, sftpNode("span", "", sftpWords[word])); options.append(label);
  }
  options.append(sftpButton("upload", () => sftpTransfer("local", "remote")), sftpButton("download", () => sftpTransfer("remote", "local")));
  options.append(sftpButton("uploadFiles", sftpUploadFiles));
  view.append(options);
  const panes = sftpNode("div", "sftp-panes");
  for (const side of ["local", "remote"]) panes.append(createSftpPane(side));
  view.append(panes);
  const queue = sftpNode("section", "sftp-queue");
  const queueHead = sftpNode("div", "sftp-queue-head");
  queueHead.append(sftpNode("h3", "", sftpWords.queue));
  const conflicts = sftpNode("label", "sftp-choice"); conflicts.append(sftpNode("span", "", sftpWords.conflict));
  const select = sftpNode("select", "field"); select.id = "sftp-conflict";
  for (const [value, word] of SFTP_CONFLICT_CHOICES) {
    const option = sftpNode("option", "", sftpWords[word]); option.value = value; select.append(option);
  }
  conflicts.append(select); queueHead.append(conflicts);
  const speed = sftpNode("label", "sftp-choice"); speed.append(sftpNode("span", "", sftpWords.speed));
  const input = sftpNode("input", "field"); input.type = "number"; input.min = "0"; input.value = "0"; input.id = "sftp-speed";
  speed.append(input); queueHead.append(speed, sftpButton("clear", async () => {
    const reports = await invoke("sftp_job_control", { id: "", action: "clear" });
    sftpJobs.clear(); for (const job of reports) sftpJobs.set(job.id, job); paintSftpJobs();
  }));
  queue.append(queueHead, sftpNode("div", "sftp-job-list")); view.append(queue);
  view.addEventListener("keydown", sftpKeydown);
  document.body.append(view);
  return view;
}

function createSftpPane(side) {
  const pane = sftpNode("section", "sftp-pane"); pane.dataset.side = side;
  pane.addEventListener("pointerdown", () => { sftpActiveSide = side; });
  const heading = sftpNode("div", "sftp-pane-head");
  heading.append(sftpNode("h3", "", sftpWords[side]), sftpButton("up", () => sftpPanes[side].parent && sftpLoad(side, sftpPanes[side].parent)), sftpButton("refresh", () => sftpLoad(side)));
  if (side === "remote") heading.append(sftpButton("bookmark", async () => {
    await invoke("sftp_bookmark", { location: sftpLocation("remote"), remove: false });
    sftpSources = await invoke("sftp_sources"); paintSftpBookmarks();
  }));
  pane.append(heading);
  const path = sftpNode("input", "field sftp-path"); path.id = `sftp-${side}-path`; path.setAttribute("aria-label", `${sftpWords[side]()} ${sftpWords.path()}`);
  path.addEventListener("keydown", (event) => { if (event.key === "Enter") void sftpRun(() => sftpLoad(side, path.value)); }); pane.append(path);
  const places = sftpNode("div", "sftp-places");
  if (side === "local") {
    const browse = sftpNode("button", "btn", () => t("project.browse", "폴더 찾아보기")); browse.type = "button";
    browse.addEventListener("click", () => void sftpRun(async () => {
      const paths = await openPathBrowser({ mode: "folder", start: sftpPanes.local.path, system: false });
      if (paths?.[0]) await sftpLoad("local", paths[0]);
    })); places.append(browse);
  }
  if (side === "remote") {
    const bookmarks = sftpNode("select", "field sftp-bookmarks"); bookmarks.setAttribute("aria-label", sftpWords.bookmarks());
    bookmarks.addEventListener("change", () => { if (bookmarks.value) void sftpRun(() => sftpLoad(side, bookmarks.value)); }); places.append(bookmarks);
  }
  pane.append(places);
  const tools = sftpNode("div", "sftp-file-tools");
  for (const word of ["mkdir", "newFile", "rename", "edit", "chmod", "remove", "copy", "cut", "paste"]) tools.append(sftpButton(word, () => sftpAction(side, word), word === "remove" ? "btn--halt" : ""));
  pane.append(tools);
  const search = sftpNode("div", "sftp-search");
  const filter = sftpNode("input", "field"); filter.id = `sftp-${side}-filter`; filter.placeholder = sftpWords.filter(); filter.setAttribute("aria-label", sftpWords.filter());
  filter.addEventListener("input", () => paintSftpPane(side)); search.append(filter, sftpButton("search", async () => {
    if (!filter.value.trim()) { sftpError(sftpWords.searchNeedsText()); return; }
    const state = sftpPanes[side]; const generation = ++state.generation;
    const entries = await invoke("sftp_search", { location: sftpLocation(side), query: filter.value });
    if (generation !== state.generation) return;
    state.entries = entries; state.selected.clear(); paintSftpPane(side);
  })); pane.append(search);
  const scroll = sftpNode("div", "sftp-file-scroll"); scroll.tabIndex = 0;
  const table = sftpNode("table", "sftp-files"); table.setAttribute("aria-label", sftpWords[side]());
  const head = sftpNode("thead"); const row = sftpNode("tr");
  for (const word of ["name", "size", "modified", "permissions"]) {
    const th = sftpNode("th"); th.scope = "col";
    th.append(sftpButton(word, () => {
      const state = sftpPanes[side]; state.sortAscending = state.sort === word ? !state.sortAscending : true; state.sort = word; paintSftpPane(side);
    })); row.append(th);
  }
  head.append(row); table.append(head, sftpNode("tbody")); scroll.append(table);
  scroll.addEventListener("contextmenu", (event) => {
    if (!event.target.closest("tr[data-path]")) openSftpContextMenu(side, null, event);
  });
  scroll.addEventListener("dragover", (event) => { if (sftpClipboard?.dragging) { event.preventDefault(); event.dataTransfer.dropEffect = "copy"; } });
  scroll.addEventListener("drop", (event) => {
    if (!sftpClipboard?.dragging) return;
    event.preventDefault(); event.stopPropagation(); void sftpRun(() => sftpPaste(side, { confirm: true }));
  });
  pane.append(scroll, sftpNode("div", "sftp-selection")); return pane;
}

async function openSftpManager(host, path = null) {
  try {
  const opening = ++sftpOpenGeneration;
  if (!sftpView) sftpView = createSftpView();
  showModal(sftpView, { owns: (node) => sidebarMenu.contains(node) });
  document.body.classList.add("sftp-active");
  sftpSources = await invoke("sftp_sources");
  if (opening !== sftpOpenGeneration) return;
  for (const report of await invoke("sftp_connections")) sftpConnections.set(report.hostId, report);
  if (opening !== sftpOpenGeneration) return;
  const hosts = el("sftp-host"); hosts.replaceChildren();
  for (const item of sftpSources) { const option = sftpNode("option", "", item.label); option.value = item.hostId; hosts.append(option); }
  const wanted = host?.id || sftpPanes.remote.hostId || hosts.value;
  // A door that only knows a saved host's id (a remote workspace card) opens
  // the row that absorbed that host.
  const selected = sftpSources.find((s) => s.hostId === wanted || s.nativeHostId === wanted)?.hostId || wanted;
  if (!selected) { sftpError(t("settings.sshHosts.empty", "저장된 SSH 호스트가 없습니다.")); return; }
  hosts.value = selected;
  const changed = sftpPanes.remote.hostId !== selected;
  if (changed) { sftpPanes.remote.entries = []; sftpPanes.remote.selected.clear(); sftpPanes.remote.path = ""; }
  sftpPanes.remote.hostId = selected;
  paintSftpConnection();
  await sftpRun(async () => {
    // Subscribed before the next await: a listing that fails at once (a cached
    // connection error) would otherwise reject with nobody listening yet.
    const pending = Promise.all([sftpLoad("remote", path ?? (changed ? "" : sftpPanes.remote.path)), sftpLoad("local")]);
    const reports = await invoke("sftp_jobs"); for (const job of reports) sftpJobs.set(job.id, job);
    await pending; paintSftpJobs(); paintSftpBookmarks();
  });
  if (opening === sftpOpenGeneration && !sftpView.hidden) el("sftp-remote-path").focus();
  } catch (error) { sftpError(String(error)); }
}
function closeSftpManager() { if (sftpView) hideModal(sftpView); document.body.classList.remove("sftp-active"); sftpLauncher.hidden = ![...sftpConnections.values()].some((r) => r.state === "connected"); }
function paintSftpConnection() {
  if (!sftpView) return;
  const report = sftpConnections.get(sftpPanes.remote.hostId);
  const state = report?.state || "disconnected";
  const node = sftpView.querySelector(".sftp-connection"); node.dataset.state = state;
  say(node, sftpWords[state] || sftpWords.error);
  node.dataset.tip = report?.error || report?.home || "";
  if (report?.error) sftpError(report.error);
}
function paintSftpBookmarks() {
  if (!sftpView) return;
  const select = sftpView.querySelector(".sftp-bookmarks"); select.replaceChildren();
  const hint = sftpNode("option", "", sftpWords.bookmarks); hint.value = ""; select.append(hint);
  const roots = new Set(sftpSources.find((s) => s.hostId === sftpPanes.remote.hostId)?.roots || []);
  const home = sftpConnections.get(sftpPanes.remote.hostId)?.home;
  if (home) roots.add(home);
  for (const root of roots) {
    const option = sftpNode("option", "", root); option.value = root; select.append(option);
  }
}
async function sftpLoad(side, path = sftpPanes[side].path) {
  const state = sftpPanes[side]; const generation = ++state.generation;
  const location = sftpLocation(side, path);
  const pane = sftpView.querySelector(`[data-side="${side}"]`); pane.setAttribute("aria-busy", "true");
  try {
    const answer = await invoke("sftp_list", { location });
    if (generation !== state.generation) return;
    Object.assign(state, answer); state.visibleLimit = SFTP_PAGE_SIZE; state.selected.clear();
    el(`sftp-${side}-path`).value = answer.path;
    paintSftpPane(side);
  } finally { if (generation === state.generation) pane.removeAttribute("aria-busy"); }
}
async function sftpOpenEntry(side, entry) {
  if (entry.directory || entry.symlink) {
    await sftpLoad(side, entry.path);
    if (el("sftp-sync").checked) {
      const other = side === "local" ? "remote" : "local";
      await sftpLoad(other, sftpJoin(sftpPanes[other].path, entry.name));
    }
  } else await sftpEdit(sftpLocation(side, entry.path));
}

function sftpVisibleEntries(side) {
  const state = sftpPanes[side]; const filter = el(`sftp-${side}-filter`).value.toLocaleLowerCase();
  const entries = state.entries.filter((e) => (el("sftp-hidden").checked || !e.name.startsWith(".")) && e.name.toLocaleLowerCase().includes(filter));
  const sort = state.sort || "name";
  return entries.sort((a, b) => {
    if (a.directory !== b.directory) return Number(b.directory) - Number(a.directory);
    const result = typeof a[sort] === "number" ? a[sort] - b[sort] : String(a[sort] ?? "").localeCompare(String(b[sort] ?? ""));
    return state.sortAscending === false ? -result : result;
  });
}
function paintSftpPane(side) {
  if (!sftpView) return;
  const state = sftpPanes[side]; const pane = sftpView.querySelector(`[data-side="${side}"]`); const tbody = pane.querySelector("tbody");
  tbody.replaceChildren();
  const entries = sftpVisibleEntries(side);
  if (state.parent) tbody.append(sftpParentRow(side, state.parent));
  const counterpart = new Map(sftpPanes[side === "local" ? "remote" : "local"].entries.map((e) => [e.name, e]));
  for (const entry of entries.slice(0, state.visibleLimit || SFTP_PAGE_SIZE)) {
    const row = sftpNode("tr"); row.dataset.path = entry.path; row.dataset.selected = String(state.selected.has(entry.path)); row.draggable = true;
    if (el("sftp-compare").checked) {
      const other = counterpart.get(entry.name);
      row.dataset.comparison = !other ? "missing" : entry.size !== other.size || entry.modified !== other.modified ? "different" : "same";
    }
    const name = sftpNode("td"); const label = sftpNode("button", "sftp-file-name");
    label.append(iconNode(entry.directory ? "folder" : entry.symlink ? "link" : "file"), sftpNode("span", "sftp-entry-name", entry.name));
    label.type = "button"; label.dataset.tip = entry.path; label.setAttribute("aria-pressed", String(state.selected.has(entry.path)));
    name.append(label); row.append(name, sftpNode("td", "", entry.directory ? "—" : sftpSize(entry.size)), sftpNode("td", "", entry.modified == null ? "—" : new Date(entry.modified * 1000).toLocaleString()), sftpNode("td", "", entry.permissions == null ? "—" : entry.permissions.toString(8).padStart(3, "0")));
    const selectEntry = (event) => {
      sftpActiveSide = side;
      if (event.shiftKey && state.anchor) {
        const start = entries.findIndex((e) => e.path === state.anchor), end = entries.indexOf(entry);
        if (start >= 0) for (const e of entries.slice(Math.min(start, end), Math.max(start, end) + 1)) state.selected.add(e.path);
      } else if (event.metaKey || event.ctrlKey) {
        if (state.selected.has(entry.path)) state.selected.delete(entry.path); else state.selected.add(entry.path);
      } else { state.selected.clear(); state.selected.add(entry.path); }
      state.anchor = entry.path;
      paintSftpSelection(side);
    };
    label.addEventListener("click", selectEntry);
    row.addEventListener("pointerdown", (event) => {
      if (event.button === 0 && !event.target.closest("button")) selectEntry(event);
    });
    row.addEventListener("contextmenu", (event) => openSftpContextMenu(side, entry, event));
    row.addEventListener("keydown", (event) => {
      if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) openSftpContextMenu(side, entry, event);
    });
    row.addEventListener("dblclick", () => void sftpRun(() => sftpOpenEntry(side, entry)));
    label.addEventListener("keydown", (event) => {
      if (event.key === "Enter") { event.preventDefault(); event.stopPropagation(); void sftpRun(() => sftpOpenEntry(side, entry)); }
    });
    row.addEventListener("dragstart", (event) => {
      if (!state.selected.has(entry.path)) { state.selected.clear(); state.selected.add(entry.path); paintSftpSelection(side); }
      sftpCopy(side, false); sftpClipboard.dragging = true; event.dataTransfer.setData("text/plain", entry.path); event.dataTransfer.effectAllowed = "copyMove";
    });
    row.addEventListener("dragend", () => { if (sftpClipboard) sftpClipboard.dragging = false; });
    tbody.append(row);
  }
  if (entries.length > (state.visibleLimit || SFTP_PAGE_SIZE)) {
    const row = sftpNode("tr"), cell = sftpNode("td"); cell.colSpan = 4;
    const more = sftpNode("button", "btn", () => t("sftp.more", "다음 {{count}}개 표시", { count: SFTP_PAGE_SIZE })); more.type = "button";
    more.onclick = () => { state.visibleLimit = (state.visibleLimit || SFTP_PAGE_SIZE) + SFTP_PAGE_SIZE; paintSftpPane(side); };
    cell.append(more); row.append(cell); tbody.append(row);
  }
  if (!entries.length) { const row = sftpNode("tr"); const td = sftpNode("td", "sftp-empty", sftpWords.empty); td.colSpan = 4; row.append(td); tbody.append(row); }
  paintSftpSelection(side);
}
/* The way up as an entry — every file manager's first row. It carries no
 * data-path, so selection, drag and the file tools never count it. */
function sftpParentRow(side, parent) {
  const row = sftpNode("tr", "sftp-parent"); const cell = sftpNode("td"); cell.colSpan = 4;
  const label = sftpNode("button", "sftp-file-name"); label.type = "button"; label.dataset.tip = parent; label.setAttribute("aria-label", sftpWords.up());
  label.append(iconNode("folder"), sftpNode("span", "sftp-entry-name", ".."));
  const up = () => void sftpRun(() => sftpLoad(side, parent));
  label.addEventListener("click", up);
  label.addEventListener("keydown", (event) => { if (event.key === "Enter") { event.preventDefault(); event.stopPropagation(); up(); } });
  cell.append(label); row.append(cell); return row;
}
function paintSftpSelection(side) {
  const state = sftpPanes[side]; const pane = sftpView.querySelector(`[data-side="${side}"]`);
  for (const row of pane.querySelectorAll("tr[data-path]")) { const selected = state.selected.has(row.dataset.path); row.dataset.selected = String(selected); row.querySelector("button").setAttribute("aria-pressed", String(selected)); }
  for (const button of pane.querySelectorAll(".sftp-file-tools button")) {
    const action = button.dataset.action;
    button.disabled = !state.path || (action === "paste" ? !sftpClipboard?.entries.length : ["rename", "edit"].includes(action) ? state.selected.size !== 1 || (action === "edit" && sftpSelected(side)[0]?.directory) : !["mkdir", "newFile"].includes(action) && state.selected.size === 0);
  }
  sftpView.querySelector('[data-action="upload"]').disabled = sftpPanes.local.selected.size === 0 || !sftpPanes.remote.path;
  sftpView.querySelector('[data-action="download"]').disabled = sftpPanes.remote.selected.size === 0 || !sftpPanes.local.path;
  pane.querySelector(".sftp-selection").textContent = t("sftp.selection", "{{selected}}개 선택 · {{total}}개 항목", { selected: state.selected.size, total: state.entries.length });
}
function sftpSize(bytes) { const units = ["B", "KiB", "MiB", "GiB", "TiB"]; let n = Number(bytes) || 0, i = 0; while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; } return `${n.toLocaleString(undefined, { maximumFractionDigits: i ? 1 : 0 })} ${units[i]}`; }

function openSftpContextMenu(side, entry, event) {
  event.preventDefault(); event.stopPropagation(); sftpActiveSide = side;
  const state = sftpPanes[side];
  if (entry && !state.selected.has(entry.path)) state.selected = new Set([entry.path]);
  if (!entry) state.selected.clear();
  paintSftpSelection(side);
  const selection = sftpSelected(side);
  const location = sftpLocation(side);
  const run = (action) => () => sftpRun(async () => {
    if (state.hostId !== location.hostId || state.path !== location.path) return;
    state.selected = new Set(selection.map((e) => e.path));
    await sftpAction(side, action);
  });
  const items = [];
  if (entry?.directory) items.push({ label: t("links.openFolder", "폴더 열기"), run: () => sftpRun(() => sftpLoad(side, entry.path)) });
  items.push({ label: side === "local" ? sftpWords.upload() : sftpWords.download(), disabled: selection.length === 0, run: () => sftpRun(() => sftpTransfer(side, side === "local" ? "remote" : "local")) }, { separator: true });
  for (const word of ["copy", "cut", "paste", "rename", "edit", "chmod", "remove"]) {
    items.push({ label: sftpWords[word](), danger: word === "remove", disabled: word === "paste" ? !sftpClipboard?.entries.length : ["rename", "edit"].includes(word) ? selection.length !== 1 || (word === "edit" && entry?.directory) : selection.length === 0, run: run(word) });
  }
  items.push({ label: t("artifacts.copyPath", "경로 복사"), disabled: selection.length === 0, run: () => clipboardText.write(selection.map((e) => e.path).join("\n")) }, { separator: true });
  for (const word of ["mkdir", "newFile", "refresh"]) items.push({ label: sftpWords[word](), run: word === "refresh" ? () => sftpRun(() => sftpLoad(side)) : run(word) });
  const box = event.currentTarget.getBoundingClientRect();
  openSidebarMenu(event.clientX || box.left, event.clientY || box.bottom, items, event.currentTarget.querySelector("button") || event.currentTarget);
}

function sftpCopy(side, moving) { sftpClipboard = { entries: sftpSelected(side).map((entry) => ({ name: entry.name, directory: entry.directory, location: sftpLocation(side, entry.path) })), moving }; paintSftpSelection("local"); paintSftpSelection("remote"); }
async function sftpPaste(side, { confirm = false } = {}) {
  if (!sftpClipboard) return;
  const clipboard = sftpClipboard;
  const destination = sftpLocation(side);
  const options = confirm ? await confirmSftpTransfer(clipboard.entries, destination) : sftpQueueOptions();
  if (!options) return;
  for (const entry of clipboard.entries) await sftpQueue({ ...options, operation: clipboard.moving ? "move" : "copy", source: entry.location, destination: { ...destination, path: sftpJoin(destination.path, entry.name) } });
  if (clipboard.moving && sftpClipboard === clipboard) sftpClipboard = null;
}
async function sftpTransfer(from, to) { sftpCopy(from, false); await sftpPaste(to, { confirm: true }); }
function confirmSftpTransfer(entries, destination) {
  if (!entries.length || sftpTransferCancel) return Promise.resolve(null);
  return new Promise((resolve) => {
    const scrim = document.getElementById("sftp-transfer-scrim"); const dialog = scrim.firstElementChild; dialog.replaceChildren(); scrim.id = "sftp-transfer-scrim";
    
    const title = t("sftp.confirmTransfer", "파일 전송 확인"); dialog.setAttribute("aria-label", title);
    dialog.append(sftpNode("h3", "", title));
    const host = destination.hostId ? sftpSources.find((s) => s.hostId === destination.hostId)?.label || destination.hostId : sftpWords.local();
    dialog.append(sftpNode("p", "sftp-transfer-target", `${host} · ${destination.path}`));
    const list = sftpNode("ul", "sftp-transfer-files");
    for (const entry of entries) { const item = sftpNode("li"); item.append(iconNode(entry.directory ? "folder" : "file"), sftpNode("span", "", entry.location.path)); list.append(item); }
    dialog.append(list);
    const caption = sftpNode("p", "sftp-editor-meta", () => t("sftp.transferCount", "{{count}}개 항목 · 폴더 내용도 함께 전송합니다.", { count: entries.length })); dialog.append(caption);
    const choice = sftpNode("label", "sftp-choice"); choice.append(sftpNode("span", "", sftpWords.conflict));
    const policy = sftpNode("select", "field");
    const options = sftpQueueOptions();
    for (const [value, word] of SFTP_CONFLICT_CHOICES) { const option = sftpNode("option", "", sftpWords[word]); option.value = value; policy.append(option); }
    policy.value = options.conflict; choice.append(policy); dialog.append(choice);
    const finish = (value) => { hideModal(scrim); scrim.firstElementChild.replaceChildren(); sftpTransferCancel = null; resolve(value); };
    sftpTransferCancel = () => finish(null);
    const footer = sftpNode("div", "sftp-transfer-actions");
    const submit = sftpNode("button", "btn btn--primary sftp-transfer-confirm", () => t("sftp.startTransfer", "전송 시작")); submit.type = "submit";
    footer.append(sftpButton("cancel", () => finish(null)), submit); dialog.append(footer);
    dialog.addEventListener("submit", (event) => { event.preventDefault(); finish({ ...options, conflict: policy.value }); });
    showModal(scrim, { initial: submit });
  });
}

async function sftpUploadFiles() {
  const destination = sftpLocation("remote");
  const paths = await openPathBrowser({ mode: "attach", start: sftpPanes.local.path, system: true });
  if (paths?.length) await sftpDropFiles(paths, destination);
}

async function sftpDropFiles(paths, destination) {
  const kinds = await invoke("path_kinds", { paths });
  const entries = paths.map((path, index) => ({ name: path.replace(/\\/g, "/").split("/").pop(), directory: kinds?.[index] === "folder", location: { hostId: null, path } }));
  const options = await confirmSftpTransfer(entries, destination);
  if (!options) return;
  for (const entry of entries) await sftpQueue({ ...options, operation: "copy", source: entry.location, destination: { ...destination, path: sftpJoin(destination.path, entry.name) } });
}

function sftpQueueOptions() {
  return { conflict: el("sftp-conflict").value, bytesPerSecond: Math.max(0, Number(el("sftp-speed").value) || 0) * 1024 };
}
async function sftpQueue(spec) {
  const job = await invoke("sftp_enqueue", { spec: { ...sftpQueueOptions(), ...spec } });
  if (!sftpJobs.has(job.id)) sftpJobs.set(job.id, job);
  paintSftpJobs();
}
async function sftpAction(side, action) {
  const selected = sftpSelected(side);
  const location = sftpLocation(side), options = sftpQueueOptions();
  const at = (path) => ({ ...location, path });
  if (action === "copy" || action === "cut") return sftpCopy(side, action === "cut");
  if (action === "paste") return sftpPaste(side);
  if (action === "mkdir" || action === "newFile") {
    const name = await sftpAsk(sftpWords[action](), ""); if (!name) return;
    if (/[\/\\\0]/.test(name) || name === "." || name === "..") throw new Error(t("sftp.invalidName", "파일 이름에는 경로 구분자를 사용할 수 없습니다."));
    if (action === "mkdir") await invoke("sftp_mkdir", { location, name });
    else await invoke("sftp_write_text", { location: at(sftpJoin(location.path, name)), text: "", expected: null });
    return sftpLoad(side);
  }
  if (!selected.length) return;
  if (action === "remove") {
    const confirmed = await askConfirm({ title: sftpWords.remove(), body: t("sftp.removeConfirm", "선택한 {{count}}개 항목과 하위 파일을 영구 삭제합니다.\n{{paths}}", { count: selected.length, paths: selected.map((e) => e.path).join("\n") }), confirm: sftpWords.remove(), deny: sftpWords.cancel(), danger: true });
    if (confirmed) for (const entry of selected) await sftpQueue({ ...options, operation: "remove", source: at(entry.path) });
  } else if (action === "chmod") {
    const value = await sftpAsk(sftpWords.chmod(), (selected[0].permissions ?? 0o644).toString(8), true); if (!value) return;
    if (!/^[0-7]{3,4}$/.test(value.text)) throw new Error(t("sftp.invalidPermissions", "권한은 755처럼 3~4자리 8진수로 입력하세요."));
    for (const entry of selected) await sftpQueue({ ...options, operation: "chmod", source: at(entry.path), permissions: parseInt(value.text, 8), recursive: value.recursive });
  } else if (selected.length === 1 && action === "rename") {
    const name = await sftpAsk(sftpWords.rename(), selected[0].name); if (!name || name === selected[0].name) return;
    await invoke("sftp_rename", { location: at(selected[0].path), name }); await sftpLoad(side);
  } else if (selected.length === 1 && action === "edit") await sftpEdit(at(selected[0].path));
}

function sftpAsk(title, initial, recursive = false) {
  return new Promise((resolve) => {
    const scrim = document.getElementById("sftp-input-scrim"); const form = scrim.firstElementChild; form.replaceChildren(); form.setAttribute("aria-label", title);
    form.append(sftpNode("h3", "", title)); const input = sftpNode("input", "field"); input.value = initial; input.required = true; input.setAttribute("aria-label", title); form.append(input);
    const check = sftpNode("input"); check.type = "checkbox";
    if (recursive) { const label = sftpNode("label", "sftp-check"); label.append(check, sftpNode("span", "", sftpWords.recursive)); form.append(label); }
    scrim.id = "sftp-input-scrim";
    const finish = (value) => { hideModal(scrim); scrim.firstElementChild.replaceChildren(); sftpPromptCancel = null; resolve(value); };
    sftpPromptCancel = () => finish(null);
    const save = sftpNode("button", "btn btn--primary", sftpWords.save); save.type = "submit";
    form.append(save, sftpButton("cancel", () => finish(null)));
    form.addEventListener("submit", (event) => { event.preventDefault(); finish(recursive ? { text: input.value, recursive: check.checked } : input.value); });
    scrim.addEventListener("keydown", (event) => event.stopPropagation());
    showModal(scrim, { initial: input }); input.select();
  });
}
async function sftpEdit(location) {
  if (sftpEditor) return;
  const text = await invoke("sftp_read_text", { location });
  const scrim = document.getElementById("sftp-editor-scrim"); const dialog = scrim.firstElementChild; dialog.replaceChildren(); dialog.setAttribute("aria-label", location.path);
  const header = sftpNode("header", "sftp-editor-head");
  const identity = sftpNode("div", "sftp-editor-identity");
  identity.append(sftpNode("h3", "", location.path.split("/").pop()), sftpNode("p", "", location.path));
  header.append(iconNode("file"), identity);
  dialog.append(header);
  const input = sftpNode("textarea", "field sftp-editor-body"); input.value = text; input.spellcheck = false; input.setAttribute("aria-label", location.path); dialog.append(input);
  const error = sftpNode("p", "sftp-error"); error.setAttribute("role", "alert"); error.hidden = true;
  sftpEditor = { original: text, location };
  const save = async () => { try { await invoke("sftp_write_text", { location, text: input.value, expected: sftpEditor.original }); sftpEditor.original = input.value; error.textContent = ""; error.hidden = true; } catch (e) { error.textContent = String(e); error.hidden = false; } };
  const close = async () => {
    if (input.value !== sftpEditor.original && !(await askConfirm({ title: sftpWords.close(), body: t("sftp.unsaved", "저장하지 않은 변경을 버릴까요?"), confirm: sftpWords.close(), deny: sftpWords.cancel(), danger: true }))) return;
    sftpEditor = null; hideModal(scrim); scrim.firstElementChild.replaceChildren(); await Promise.all([sftpLoad("local"), sftpLoad("remote")]);
  };
  scrim.id = "sftp-editor-scrim"; sftpEditor.close = close;
  const footer = sftpNode("footer", "sftp-editor-footer");
  footer.append(sftpNode("span", "sftp-editor-meta", `UTF-8 · ${usesCommandModifier ? "⌘" : "Ctrl+"}S`), error, sftpButton("close", close), sftpButton("save", save, "btn--primary"));
  dialog.append(footer);
  scrim.addEventListener("keydown", (event) => { event.stopPropagation(); if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") { event.preventDefault(); void save(); } });
  showModal(scrim, { initial: input });
}
function sftpKeydown(event) {
  if (event.key === "Escape") { event.stopPropagation(); closeSftpManager(); return; }
  if (event.target.matches("input, select, textarea")) return;
  const side = event.target.closest(".sftp-pane")?.dataset.side || sftpActiveSide; const key = event.key.toLowerCase();
  if (event.metaKey || event.ctrlKey) {
    const action = { c: "copy", x: "cut", v: "paste" }[key];
    if (action) { event.preventDefault(); event.stopPropagation(); void sftpRun(() => sftpAction(side, action)); }
    if (key === "a") { event.preventDefault(); event.stopPropagation(); sftpPanes[side].selected = new Set(sftpVisibleEntries(side).map((e) => e.path)); paintSftpSelection(side); }
  } else if (key === "delete" || key === "backspace" || key === "f2") { event.preventDefault(); event.stopPropagation(); void sftpRun(() => sftpAction(side, key === "f2" ? "rename" : "remove")); }
}

function paintSftpJobs() {
  if (!sftpView) return;
  const list = sftpView.querySelector(".sftp-job-list"); list.replaceChildren();
  if (!sftpJobs.size) { list.append(sftpNode("p", "sftp-empty", sftpWords.queueEmpty)); return; }
  for (const job of sftpJobs.values()) {
    const row = sftpNode("div", "sftp-job"); row.dataset.jobId = job.id; row.dataset.state = job.state;
    const copy = sftpNode("div", "sftp-job-copy"); copy.append(sftpNode("div", "sftp-job-path", `${job.spec.source.path}${job.spec.destination ? ` → ${job.spec.destination.path}` : ""}`));
    const progress = job.progress || {}; const meter = sftpNode("progress"); meter.max = progress.totalBytes || progress.totalFiles || 1; meter.value = progress.totalBytes ? progress.bytes || 0 : progress.files || 0; meter.setAttribute("aria-label", job.spec.source.path); copy.append(meter);
    copy.append(sftpNode("small", "", `${sftpWords[job.state]?.() || job.state} · ${sftpSize(progress.bytes)} / ${sftpSize(progress.totalBytes)} · ${progress.files || 0}/${progress.totalFiles || 0}`));
    if (job.error) copy.append(sftpNode("p", "sftp-error", job.error));
    row.append(copy);
    const control = async (action) => { const reports = await invoke("sftp_job_control", { id: job.id, action }); for (const r of reports) sftpJobs.set(r.id, r); paintSftpJobs(); };
    if (["queued", "running", "paused"].includes(job.state)) row.append(sftpButton(job.state === "paused" ? "continue" : "pause", () => control(job.state === "paused" ? "resume" : "pause")), sftpButton("cancel", () => control("cancel")));
    if (["failed", "cancelled"].includes(job.state)) row.append(sftpButton("retry", () => control("retry")));
    if (job.state === "failed" && job.error?.includes("destination already exists")) {
      for (const [word, conflict] of [["overwrite", "overwrite"], ["resume", "resume"], ["skip", "skip"], ["keepBoth", "rename"]]) row.append(sftpButton(word, () => sftpQueue({ ...job.spec, conflict })));
    }
    list.append(row);
  }
}

function sftpNativeDrop(paths, x, y) {
  if (!sftpView || sftpView.hidden) return false;
  const side = document.elementFromPoint(x, y)?.closest(".sftp-pane")?.dataset.side;
  if (side) {
    const destination = sftpLocation(side);
    void sftpRun(() => sftpDropFiles(paths, destination));
  }
  return true;
}
listen("sftp:connection", (event) => {
  const report = event.payload; if (!report?.hostId) return;
  sftpConnections.set(report.hostId, report); paintSftpConnection();
  sftpLauncher.hidden = ![...sftpConnections.values()].some((r) => r.state === "connected");
  if (report.state === "connected") { void refreshRemoteWorkspaces().then(paintSftpBookmarks); }
});
listen("sftp:job", (event) => {
  const job = event.payload; if (!job?.id) return; sftpJobs.set(job.id, job); paintSftpJobs();
  if (["completed", "failed", "cancelled"].includes(job.state) && sftpView && !sftpView.hidden) {
    void sftpRun(() => Promise.all([sftpLoad("local"), sftpLoad("remote")]));
  }
});
const sftpLauncher = sftpButton("title", () => openSftpManager()); sftpLauncher.classList.add("sftp-launcher"); sftpLauncher.hidden = true; document.body.append(sftpLauncher);

async function openSftpTarget(target) {
  await sftpRun(async () => {
    const sources = await invoke("sftp_sources");
    const source = sources.find((s) => s.targetId === target.id);
    if (source) await openSftpManager({ id: source.hostId });
  });
}

// Every modal the focus inventory registers must stand in the document from
// load (shell-input.js's table is compared against the DOM): the manager view
// and the three dialog scrims are created hidden here and filled on open,
// emptied on close — never created and removed per use.
sftpView = createSftpView();
for (const [id, tag, face] of [
  ["sftp-transfer-scrim", "form", "sftp-dialog sftp-transfer-dialog"],
  ["sftp-input-scrim", "form", "sftp-dialog"],
  ["sftp-editor-scrim", "section", "sftp-editor"],
]) {
  const shell = sftpNode("div", "sftp-dialog-scrim"); shell.id = id; shell.hidden = true;
  const dialog = sftpNode(tag, face); dialog.setAttribute("role", "dialog"); dialog.setAttribute("aria-modal", "true");
  shell.append(dialog); document.body.append(shell);
}
