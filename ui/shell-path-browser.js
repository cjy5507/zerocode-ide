/* ---- 창 안 경로 브라우저 — 폴더 선택과 첨부, 브라우저는 하나 (t-2982) --------
 *
 * 시스템 폴더 창을 쓸 수 없을 때의 대체 문. AppKit의 NSOpenPanel은 원격 뷰
 * 서비스(`openAndSavePanelService`)가 올라올 때까지 메인 스레드를 동기로 잡고,
 * macOS 26에서는 그 서비스가 AutoFill 실드까지 세운다 — 같은 길에서 강제
 * 종료가 셋(t-2488 09-05 02:49, 09-06 17:15, 09-07 11:38). 여기는 웹뷰 안의
 * 모달이라 AppKit을 부르지 않는다: 백엔드에 묻는 것은 디렉터리 한 층
 * (`browse_dir`)과 시작점(`browse_places`)뿐이고, 둘 다 async 런타임의 평범한
 * 읽기다. 설계: docs/design/folder-panel-off-the-main-thread.md §2.
 *
 * 브라우저는 하나, 모드는 둘(docs/design/composer-attachments.md §1):
 *   - `folder` — 폴더 하나를 고른다. 「이 폴더 선택」은 강조된 폴더 행이 있으면
 *     그것, 없으면 지금 보고 있는 폴더(Finder의 Choose와 같은 규칙).
 *   - `attach` — 파일과 폴더를 여럿 고른다. 행의 체크가 담고, Enter는 폴더에
 *     들어가거나 파일을 담는다. 답은 담은 순서의 절대 경로 목록.
 * 이음매는 `openPathBrowser({ mode, start, multiple })` 하나 — 약속은 절대 경로
 * 배열로 풀리고, 취소는 빈 배열이다. 첨부 모듈(t-2993)은 이 함수를 부를 뿐
 * 두 번째 브라우저를 만들지 않는다.
 *
 * 폴더 선택은 시스템 대화상자가 기본이고, 첨부는 이 브라우저를 쓴다: 창이 아니라
 * 헬퍼 프로세스 `zerocode-pick`이 자기 프로세스에서 패널을 열고 stdout으로
 * 답한다 — 메인 스레드는 자유라 8초 토스트가 실제로 뜨고, 이 브라우저의 띠가
 * 「앞으로 가져오기」와 「취소」를 내민다.
 *
 * 숫자는 여기 없다: 한 폴더의 항목 상한과 최근 프로젝트 행 수는
 * crates/zerocode-shell/src/project_runtime.rs 의 폴더 패널 표가 정하고,
 * 답이 잘렸으면 답 자신이 `truncated`와 `total`로 말한다. */
const pathBrowserScrim = el("pb-scrim");
const pbTitle = el("pb-title");
const pbPath = el("pb-path");
const pbList = el("pb-list");
const pbRows = el("pb-rows");
const pbEmpty = el("pb-empty");
const pbCount = el("pb-count");
const pbKeys = el("pb-keys");
const pbError = el("pb-error");
const pbStanding = el("pb-standing");
const pbConfirm = el("pb-confirm");
const pbSystem = el("pb-system");

/* The one browser while it stands. `at` is the folder shown, `null` on the
 * start view; `rows` are what the list paints, in order; `picked` is the
 * attach mode's answer in the order it was gathered. */
let pathBrowser = null;

/* Dotfiles are machinery, not something a person opens as a project or
 * attaches by browsing — Finder hides them too. The path field still takes
 * one typed in full. */
function pathBrowserShows(entry) {
  return !entry.name.startsWith(".");
}

function openPathBrowser({ mode = "folder", start = null, multiple = mode === "attach", system = mode === "folder" } = {}) {
  // One browser. A second asker waits on the answer the first is gathering
  // rather than raising a second modal over it.
  if (pathBrowser) {
    if (pathBrowser.systemAsk) void invoke("recall_folder_panel").catch(showError);
    return pathBrowser.promise;
  }
  let resolve;
  const promise = new Promise((done) => {
    resolve = done;
  });
  pathBrowser = {
    mode,
    multiple,
    resolve,
    promise,
    at: null,
    parent: null,
    rows: [],
    // Which row the keyboard is on, `-1` for none: Finder's rule — nothing is
    // highlighted until an arrow moves, so 「이 폴더 선택」 means the folder
    // shown. Kept here rather than as a class on a node, because only a
    // window of the rows has nodes at all.
    highlight: -1,
    picked: new Map(),
    // Only the newest ask may paint: a slow folder answering after the
    // person already went elsewhere must not overwrite where they are.
    generation: 0,
    systemAsk: null,
    nativeFirst: system,
    start,
    dismissRequested: false,
  };
  paintPathBrowserWords();
  paintPathBrowserRows([]);
  pbError.textContent = "";
  pbStanding.hidden = true;
  if (system) void askSystemPanel();
  else showPathBrowserContents(start);
  return promise;
}

function showPathBrowserContents(start, error = "") {
  const held = pathBrowser;
  if (!held) return;
  held.nativeFirst = false;
  showModal(pathBrowserScrim, { initial: "pb-list" });
  void (start ? enterPathBrowserFolder(start) : showPathBrowserPlaces()).then(() => {
    if (pathBrowser === held && error) pbError.textContent = error;
  });
}

/* Close, answering. `paths` empty is a cancel — an answer, not a failure. */
function closePathBrowser(paths = []) {
  const held = pathBrowser;
  if (!held) return;
  if (held.systemAsk) {
    if (held.dismissRequested) return;
    held.dismissRequested = true;
    held.pendingPaths = [...paths];
    void invoke("cancel_folder_panel").catch((error) => {
      if (pathBrowser === held) held.dismissRequested = false;
      showError(error);
    });
    return;
  }
  pathBrowser = null;
  dismissFolderPanelToast();
  hideModal(pathBrowserScrim);
  held.resolve(paths);
}

/* The words that depend on the mode are SAID, not keyed: `say` speaks them
 * now and again on a language change, and an element with a key would be
 * overwritten by that key's folder-mode words the moment the language moved. */
function paintPathBrowserWords() {
  const attach = pathBrowser?.mode === "attach";
  say(pbTitle, () =>
    attach
      ? t("project.browse.attachTitle", "파일 및 폴더 첨부")
      : t("project.browse.title", "폴더 선택"));
  say(pbKeys, () =>
    attach
      ? t("project.browse.attachKeys", "↑↓ 이동 · ↵ 열기·담기 · 스페이스 담기 · ⌫ 위로 · ⌘↵ 첨부")
      : t("project.browse.keys", "↑↓ 이동 · ↵ 열기 · ⌫ 위로 · ⌘↵ 선택"));
  paintPathBrowserConfirm();
}

function paintPathBrowserConfirm() {
  if (!pathBrowser) return;
  if (pathBrowser.mode === "attach") {
    const count = pathBrowser.picked.size;
    say(pbConfirm, () => t("project.browse.attach", "첨부 {{count}}개", { count }));
    pbConfirm.disabled = count === 0;
  } else {
    say(pbConfirm, () => t("project.browse.chooseHere", "이 폴더 선택"));
    // Nothing to choose on the start view unless a folder row is highlighted.
    pbConfirm.disabled = !pathBrowserChoice();
  }
}

/* The folder mode's answer as it stands: the highlighted folder row, else the
 * folder being looked at. */
function pathBrowserChoice() {
  const row = pathBrowserHighlighted();
  if (row?.isDir) return row.path;
  return pathBrowser?.at ?? null;
}

async function showPathBrowserPlaces() {
  const held = pathBrowser;
  if (!held) return;
  const generation = ++held.generation;
  let places;
  try {
    places = await invoke("browse_places");
  } catch (error) {
    if (pathBrowser === held && held.generation === generation) pbError.textContent = String(error);
    return;
  }
  if (pathBrowser !== held || held.generation !== generation) return;
  held.at = null;
  held.parent = null;
  held.home = places?.home ?? null;
  pbPath.value = "";
  pbError.textContent = "";
  const rows = (places?.places ?? []).map((place) => ({
    kind: place.kind,
    name: place.kind === "home" ? t("project.browse.home", "홈") : place.name,
    hint: place.path,
    path: place.path,
    isDir: true,
  }));
  paintPathBrowserRows(rows);
  say(pbCount, () => "");
}

async function enterPathBrowserFolder(path) {
  const held = pathBrowser;
  if (!held) return;
  const generation = ++held.generation;
  let answer;
  try {
    answer = await invoke("browse_dir", { path });
  } catch (error) {
    if (pathBrowser === held && held.generation === generation) {
      pbError.textContent = String(error);
      pbPath.select();
    }
    return;
  }
  if (pathBrowser !== held || held.generation !== generation) return;
  held.at = answer.path;
  held.parent = answer.parent ?? null;
  pbPath.value = answer.path;
  pbError.textContent = "";
  const shown = answer.entries.filter(pathBrowserShows);
  paintPathBrowserRows(
    shown.map((entry) => ({
      kind: entry.is_dir ? "folder" : "file",
      name: entry.name,
      hint: "",
      path: joinBrowsePath(answer.path, entry.name),
      isDir: entry.is_dir,
    })),
  );
  say(pbCount, () =>
    answer.truncated
      ? t("project.browse.truncated", "{{shown}}개 표시 · 외 {{more}}개 — 나머지는 경로를 직접 입력하세요", {
          shown: answer.entries.length,
          more: answer.total - answer.entries.length,
        })
      : t("project.browse.count", "{{count}}개", { count: shown.length }));
}

/* `/` keeps its one separator; everything else gets one. */
function joinBrowsePath(dir, name) {
  return dir.endsWith("/") || dir.endsWith("\\") ? `${dir}${name}` : `${dir}/${name}`;
}

function goUpPathBrowser() {
  if (!pathBrowser) return;
  if (pathBrowser.at === null) return;
  if (pathBrowser.parent) void enterPathBrowserFolder(pathBrowser.parent);
  else void showPathBrowserPlaces();
}

const PB_GLYPH = Object.freeze({
  recent: "clock",
  home: "folder",
  volume: "hard-drive",
  folder: "folder",
  file: "file",
});

/* The row's height, from the token the stylesheet lays rows out with — the
 * one number the windowing arithmetic needs, read rather than repeated. */
function pathBrowserRowHeight() {
  return parseFloat(getComputedStyle(pbList).getPropertyValue("--pb-row-height"));
}

/* Paint a fresh list: the model takes the rows, the scroll goes to the top,
 * and only the rows in view get nodes. A folder at the table's cap is five
 * thousand rows; the window's harness pins the open under 100 ms, and five
 * thousand nodes with a glyph each do not fit in that. */
function paintPathBrowserRows(rows) {
  const held = pathBrowser;
  if (!held) return;
  held.rows = rows;
  held.highlight = -1;
  pbList.scrollTop = 0;
  renderPathBrowserWindow();
  pbEmpty.hidden = rows.length > 0 || held.at === null;
  paintPathBrowserConfirm();
}

/* Nodes for the rows in view and one viewport's worth either side, placed by
 * transform inside a box the height of the whole list, so the scrollbar is
 * honest about how many rows there are. Cheap enough to rebuild on every
 * scroll: a viewport is a dozen rows. */
function renderPathBrowserWindow() {
  const held = pathBrowser;
  if (!held) return;
  const height = pathBrowserRowHeight();
  const total = held.rows.length;
  pbRows.style.height = `${total * height}px`;
  const viewport = pbList.clientHeight;
  const span = Math.max(1, Math.ceil(viewport / height));
  const first = Math.max(0, Math.floor(pbList.scrollTop / height) - span);
  const last = Math.min(total, first + 3 * span);
  const template = el("pb-row-template").content.firstElementChild;
  const attach = held.mode === "attach";
  const fragment = document.createDocumentFragment();
  for (let index = first; index < last; index += 1) {
    const row = held.rows[index];
    const node = template.cloneNode(true);
    node.id = `pb-row-${index}`;
    node.dataset.index = String(index);
    node.dataset.path = row.path;
    node.style.transform = `translateY(${index * height}px)`;
    node.setAttribute("aria-posinset", String(index + 1));
    node.setAttribute("aria-setsize", String(total));
    if (row.isDir) node.classList.add("is-dir");
    const highlighted = index === held.highlight;
    node.classList.toggle("is-sel", highlighted);
    node.setAttribute("aria-selected", String(highlighted));
    const picked = attach && held.picked.has(row.path);
    node.classList.toggle("is-picked", picked);
    node.querySelector(".pb-check").hidden = !attach;
    node.querySelector(".pb-check").setAttribute("aria-pressed", String(picked));
    node.querySelector(".qo-row-icon use").setAttribute("href", `#i-${PB_GLYPH[row.kind] ?? "file"}`);
    node.querySelector(".qo-label").textContent = row.name;
    node.querySelector(".qo-hint").textContent = row.hint;
    fragment.appendChild(node);
  }
  pbRows.replaceChildren(fragment);
  announcePathBrowserRow();
}

let pathBrowserScrollFrame = 0;
pbList.addEventListener("scroll", () => {
  if (pathBrowserScrollFrame) return;
  pathBrowserScrollFrame = requestAnimationFrame(() => {
    pathBrowserScrollFrame = 0;
    renderPathBrowserWindow();
  });
});

function pathBrowserSelectedNode() {
  return pbRows.querySelector(".is-sel");
}

function pathBrowserHighlighted() {
  const held = pathBrowser;
  return held && held.highlight >= 0 ? held.rows[held.highlight] ?? null : null;
}

/* Point the listbox at the row the arrows have landed on — the only thing
 * that tells a screen reader which row Enter would take, since the list and
 * not the row holds the focus. */
function announcePathBrowserRow() {
  const selected = pathBrowserSelectedNode();
  if (selected) pbList.setAttribute("aria-activedescendant", selected.id);
  else pbList.removeAttribute("aria-activedescendant");
}

function highlightPathBrowserRow(index) {
  const held = pathBrowser;
  if (!held || held.rows.length === 0) return;
  const bounded = Math.max(0, Math.min(held.rows.length - 1, index));
  held.highlight = bounded;
  // Scrolled into view before the render, so the highlighted row has a node.
  const height = pathBrowserRowHeight();
  const top = bounded * height;
  const viewport = pbList.clientHeight;
  if (top < pbList.scrollTop) pbList.scrollTop = top;
  else if (top + height > pbList.scrollTop + viewport) pbList.scrollTop = top + height - viewport;
  renderPathBrowserWindow();
  paintPathBrowserConfirm();
}

function movePathBrowser(step) {
  highlightPathBrowserRow((pathBrowser?.highlight ?? -1) + step);
}

/* Enter on a row: a folder is entered, a file (attach) is toggled. */
function activatePathBrowserRow(index) {
  const row = pathBrowser?.rows[index];
  if (!row) return;
  if (row.isDir) void enterPathBrowserFolder(row.path);
  else if (pathBrowser.mode === "attach") togglePathBrowserPick(index);
}

function togglePathBrowserPick(index) {
  const held = pathBrowser;
  const row = held?.rows[index];
  if (!row || held.mode !== "attach") return;
  if (held.picked.has(row.path)) held.picked.delete(row.path);
  else {
    if (!held.multiple) held.picked.clear();
    held.picked.set(row.path, row);
  }
  renderPathBrowserWindow();
  paintPathBrowserConfirm();
}

function confirmPathBrowser() {
  const held = pathBrowser;
  if (!held) return;
  if (held.mode === "attach") {
    if (held.picked.size === 0) return;
    closePathBrowser([...held.picked.keys()]);
    return;
  }
  const choice = pathBrowserChoice();
  if (choice) closePathBrowser([choice]);
}

/* The second door: the operating system's own panel, in the helper's process.
 * The browser stays up behind it so the strip below can bring the panel
 * forward or kill it — a panel a person cannot see is the whole history of
 * this road. */
async function askSystemPanel() {
  const held = pathBrowser;
  if (!held || held.systemAsk) return;
  pbSystem.setAttribute("aria-busy", "true");
  pbStanding.hidden = false;
  pbError.textContent = "";
  const ask = invoke("choose_paths", {
    kind: held.mode === "attach" ? "files" : "folder",
    start: held.at ?? held.start,
  });
  held.systemAsk = ask;
  if (held.nativeFirst) showFolderPanelProgress();
  let chosen = [];
  let failure = "";
  try {
    chosen = (await ask) ?? [];
  } catch (error) {
    failure = String(error);
    if (pathBrowser === held) pbError.textContent = failure;
  } finally {
    if (pathBrowser === held) {
      held.systemAsk = null;
      pbSystem.removeAttribute("aria-busy");
      pbStanding.hidden = true;
      dismissFolderPanelToast();
    }
  }
  if (pathBrowser !== held) return;
  if (held.pendingPaths) closePathBrowser(held.pendingPaths);
  else if (chosen.length > 0) closePathBrowser(chosen);
  else if (held.nativeFirst && failure) showPathBrowserContents(held.start, failure);
  else if (held.nativeFirst) closePathBrowser([]);
}

pbList.addEventListener("keydown", (event) => {
  if (event.isComposing || event.key === "Process" || event.keyCode === 229) return;
  const index = pathBrowser?.highlight ?? -1;
  if (event.key === "ArrowDown") {
    event.preventDefault();
    movePathBrowser(1);
  } else if (event.key === "ArrowUp") {
    event.preventDefault();
    movePathBrowser(-1);
  } else if (event.key === "Home") {
    event.preventDefault();
    highlightPathBrowserRow(0);
  } else if (event.key === "End") {
    event.preventDefault();
    highlightPathBrowserRow((pathBrowser?.rows.length ?? 0) - 1);
  } else if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
    event.preventDefault();
    confirmPathBrowser();
  } else if (event.key === "Enter" || event.key === "ArrowRight") {
    event.preventDefault();
    if (index >= 0) activatePathBrowserRow(index);
  } else if (event.key === "Backspace" || event.key === "ArrowLeft") {
    event.preventDefault();
    goUpPathBrowser();
  } else if (event.key === " ") {
    event.preventDefault();
    if (index >= 0) togglePathBrowserPick(index);
  }
});

pbList.addEventListener("click", (event) => {
  const node = event.target.closest(".pb-row");
  if (!node) return;
  const index = Number(node.dataset.index);
  highlightPathBrowserRow(index);
  if (event.target.closest(".pb-check")) {
    togglePathBrowserPick(index);
    return;
  }
  const row = pathBrowser?.rows[index];
  // A click on a file in attach mode is the check; a click on a folder is
  // the highlight, and a double click enters it.
  if (row && !row.isDir && pathBrowser.mode === "attach") togglePathBrowserPick(index);
});

pbList.addEventListener("dblclick", (event) => {
  const node = event.target.closest(".pb-row");
  if (!node || event.target.closest(".pb-check")) return;
  activatePathBrowserRow(Number(node.dataset.index));
});

pbPath.addEventListener("keydown", (event) => {
  if (event.isComposing || event.key === "Process" || event.keyCode === 229) return;
  if (event.key !== "Enter") return;
  event.preventDefault();
  const typed = pbPath.value.trim();
  if (typed) {
    void enterPathBrowserFolder(typed).then(() => pbList.focus({ preventScroll: true }));
  } else void showPathBrowserPlaces();
});

el("pb-up").addEventListener("click", () => {
  goUpPathBrowser();
  pbList.focus({ preventScroll: true });
});
el("pb-close").addEventListener("click", () => closePathBrowser([]));
el("pb-cancel").addEventListener("click", () => closePathBrowser([]));
pbConfirm.addEventListener("click", confirmPathBrowser);
pbSystem.addEventListener("click", () => void askSystemPanel());
el("pb-bring").addEventListener("click", () => void invoke("recall_folder_panel").catch(showError));
el("pb-abort").addEventListener("click", () => void invoke("cancel_folder_panel").catch(showError));
pathBrowserScrim.addEventListener("mousedown", (event) => {
  if (event.target === pathBrowserScrim) closePathBrowser([]);
});

/* A narrow harness seam: how many rows the list holds, which is not how many
 * nodes it painted — the difference is the whole point of the window. */
window.__PATH_BROWSER_ROWS__ = () => pathBrowser?.rows.length ?? 0;
