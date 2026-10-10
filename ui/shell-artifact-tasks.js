/* 아티팩트 — 작업별 탭 (t-36910 2단계).
 *
 * 갤러리는 창고 셋(페이지 · 보고서 · 증거)이었고, 한 작업이 무엇을 남겼는지 알려면 셋을 따로
 * 뒤져야 했다. 이 탭은 같은 행을 작업 하나로 읽는다: 왼쪽에 작업마다 한 줄, 고르면 오른쪽
 * 읽는 칸(서랍 그대로)에 최종 보고와 그 작업의 다른 시도 · 전후 그림 · 증거와 수 · 페이지가 선다.
 *
 * 묶는 열쇠는 행의 `origin.task`(와 Computer Use 기록의 `task:` 꼬리표)이고 런타임이 센다
 * (`artifact_tasks`, `artifact_bundle`) — 창은 폴더 모양이나 파일 이름으로 묶지 않는다. 줄이
 * 말하는 수는 런타임이 한 번에 센 값이고 부분의 합이 늘 전체다.
 *
 * 그리기는 갤러리와 같은 규칙이다: 보이는 줄만 서고(풀), 바뀐 것만 쓰고, 묻는 일은 탭을 열 때 ·
 * 거르개가 바뀔 때 · 카탈로그가 움직였다는 말이 올 때뿐이다. */

/* 줄이 말하는 부분 — 순서가 곧 줄의 순서다. 수가 0인 부분은 서지 않는다. */
const ARTIFACT_TASK_PARTS = Object.freeze([
  { part: "reports", key: "artifacts.tasks.reports", word: "보고서 {{n}}" },
  { part: "pages", key: "artifacts.tasks.pages", word: "페이지 {{n}}" },
]);
/* 파일의 속 — 「파일 52 (그림 4 · 로그 23 · 그 밖 25)」: 괄호 안의 합이 늘 앞의 수다. */
const ARTIFACT_TASK_FILE_PARTS = Object.freeze([
  { part: "pictures", key: "artifacts.tasks.pictures", word: "그림 {{n}}" },
  { part: "logs", key: "artifacts.tasks.logs", word: "로그 {{n}}" },
  { part: "other", key: "artifacts.tasks.other", word: "그 밖 {{n}}" },
]);
/* 연결 안 된 것의 세 줄 — 누르면 그 탭이 「작업에 연결 안 된 것」으로 걸러져 열린다. */
const ARTIFACT_LOOSE_LINES = Object.freeze([
  { tab: "reports", part: "reports", key: "artifacts.loose.reports", word: "보고서 {{n}}" },
  { tab: "pages", part: "pages", key: "artifacts.loose.pages", word: "페이지·문서 {{n}}" },
  { tab: "evidence", part: "files", key: "artifacts.loose.files", word: "증거 {{n}}" },
]);
/* 전후 비교의 두 방식 — 자리(`mode`)가 곧 데이터 속성이다. */
const ARTIFACT_COMPARE_MODES = Object.freeze([
  { mode: "slide", key: "artifacts.compare.slide", word: "겹쳐 밀기" },
  { mode: "side", key: "artifacts.compare.side", word: "나란히" },
]);
/* 경계의 처음 자리(%)와 화살표 한 번의 걸음(%) — 막대와 손잡이가 같은 값을 읽는다. */
const ARTIFACT_COMPARE_START = 50;
const ARTIFACT_COMPARE_STEP = 2;
/* 한 번에 묻는 커밋의 수(런타임의 `COMMITS_ASKED_MAX`보다 작게: 보이는 줄의 것만 묻는다)와,
 * 물었던 커밋을 기억하는 수 — 넘으면 가장 오래전에 본 것부터 잊는다. */
const ARTIFACT_COMMITS_PER_ASK = 64;
const ARTIFACT_COMMITS_REMEMBERED = 512;

/* 작업 줄 — 런타임이 마지막으로 답한 것, 새 작업부터. */
let artifactTaskLines = [];
let artifactTaskListing = { total: 0, truncated: false, unlinked: null };
/* 읽는 칸에 연 작업. 줄이 거르개에서 빠지면 닫힌다. */
let artifactTaskOpen = null;
/* 작업마다의 묶음과 그 물음 — 카탈로그가 움직이면 버린다. */
const artifactBundles = new Map();
const artifactBundleAsks = new Map();
/* 카탈로그가 움직일 때마다 오르는 수 — 그 전에 떠난 물음의 답은 쓰지 않는다. */
let artifactBundleGeneration = 0;
/* 커밋마다의 반영 상태(워크트리가 없어진 작업의 것)와, 이미 물은 커밋. */
const artifactCommitLandings = new Map();
const artifactCommitsAsked = new Set();
let artifactCommitAskTimer = 0;
/* 작업 줄의 물음에 매기는 번호 — 늦게 온 옛 답이 새 답을 덮지 않게 — 와, 마지막으로 물은
 * 거르개. 같은 거르개는 다시 묻지 않는다(탭만 옮기거나 작업 칩으로 거를 때). */
let artifactTaskAskSerial = 0;
let artifactTaskScopeAsked = null;
/* 판마다의 줄 풀 — 카드 풀과 같은 규칙: 전환이 노드를 만들지 않는다. */
const artifactTaskPools = new WeakMap();
/* 하네스가 세는 수 — 줄 노드가 몇 번 만들어졌는가. */
let artifactTaskRowCreations = 0;

/* ---- 묻기 ---------------------------------------------------------------- */

/* 작업 줄에게 묻는 거르개: 갤러리의 것 그대로(프로젝트 · 에이전트 · 기간 · 출처)이고, 검색어는
 * 줄에게 묻는다. */
function artifactTaskFilter() {
  return { ...artifactScopeQuery(), query: artifactFilter.query.trim() };
}

/* 작업 줄을 묻는다. 이 명령을 모르는 런타임(옛 답)은 작업이 없다고 읽는다. */
async function askArtifactTasks() {
  const filter = artifactTaskFilter();
  artifactTaskScopeAsked = JSON.stringify(filter);
  try {
    return await invoke("artifact_tasks", { filter });
  } catch {
    return null;
  }
}

/* 답을 받아 적는다. 열어 둔 작업이 답에 없으면(거르개가 그 줄을 뺐다) 읽는 칸을 닫는다. */
function noteArtifactTasks(answer) {
  artifactTaskLines = Array.isArray(answer?.tasks) ? answer.tasks : [];
  artifactTaskListing = {
    total: answer?.total ?? artifactTaskLines.length,
    truncated: answer?.truncated === true,
    unlinked: answer?.unlinked ?? null,
  };
  if (artifactTaskOpen && !artifactTaskLines.some((line) => line.task === artifactTaskOpen)) closeArtifactTask();
}

/* 거르개가 바뀌었다: 줄에게 묻는 부분이 달라졌으면 다시 묻고, 서 있는 판을 다시 그린다. 빨리
 * 치는 동안 답이 순서를 바꿔 오면 마지막 물음의 답만 쓴다. */
async function refreshArtifactTasks(view) {
  if (JSON.stringify(artifactTaskFilter()) === artifactTaskScopeAsked) return;
  artifactTaskAskSerial += 1;
  const asked = artifactTaskAskSerial;
  const answer = await askArtifactTasks();
  if (asked !== artifactTaskAskSerial) return;
  noteArtifactTasks(answer);
  applyArtifactFilter();
  paintArtifactHead(view);
  paintArtifactTasks(view);
  paintArtifactDrawer(view);
}

function artifactTaskLine(task) {
  return artifactTaskLines.find((line) => line.task === task) ?? null;
}

/* 한 작업의 묶음 — 든 것이 있으면 그것을, 없으면 한 번 묻는다. 묶음의 행은 창의 카탈로그에
 * 합친다(사라진 파일의 행도): 읽는 칸이 그 행을 고를 수 있어야 한다. */
async function askArtifactBundle(task) {
  if (artifactBundles.has(task)) return artifactBundles.get(task);
  if (!artifactBundleAsks.has(task)) {
    const generation = artifactBundleGeneration;
    artifactBundleAsks.set(task, invoke("artifact_bundle", { task })
      .catch(() => null)
      .then((bundle) => {
        if (generation !== artifactBundleGeneration) return null;
        for (const row of bundle?.rows ?? []) if (!artifactRows.has(row.id)) artifactRows.set(row.id, row);
        for (const id of bundle?.missing ?? []) artifactMissing.add(id);
        if (bundle) indexArtifactTasks();
        // 답이 없었다는 것도 적어 둔다 — 그리기가 같은 작업을 되풀이해 묻지 않게.
        artifactBundles.set(task, bundle ?? null);
        artifactBundleAsks.delete(task);
        return bundle ?? null;
      }));
  }
  return artifactBundleAsks.get(task);
}

/* 카탈로그가 움직였다 — 묶음은 다음에 열 때 다시 묻는다. */
function forgetArtifactBundles() {
  artifactBundleGeneration += 1;
  artifactBundles.clear();
  artifactBundleAsks.clear();
}

/* ---- 반영 상태 — 워크트리가 없어진 작업은 커밋으로 ----------------------- */

/* 커밋으로 물어야 하는 것의 커밋: 창이 그 작업 폴더를 들고 있지 않을 때만. 들고 있으면 사이드바와
 * 같은 값(`worktreeLandings`)이 답한다. `held`는 작업 줄이거나 행의 출처다. */
function artifactLandingCommit(held) {
  const commit = held?.commit ?? "";
  if (commit === "") return "";
  return held.worktree && worktreeLandings.has(held.worktree) ? "" : commit;
}

/* 커밋의 반영 상태를 말로. 판정은 사이드바의 것과 같은 함수이고, 그 커밋을 아는 저장소가 없을
 * 때만 이 화면의 낱말로 말한다 — 그때 「확인 필요」는 사람에게 일을 시키는 말이라 틀리다. */
function artifactCommitLandingSay(commit) {
  const landing = artifactCommitLandings.get(commit);
  if (!landing) return null;
  if (landing.state === "unknown") {
    const word = artifactVerdictWords("unknown");
    return { word, head: word, tone: "none", tip: t("artifacts.landing.unknown", "이 커밋을 가진 저장소가 없어 반영 여부를 확인하지 못했습니다"), cleanable: false };
  }
  return worktreeLandingSay(landing);
}

/* 작업 줄이나 행의 출처(`held`)의 반영 상태: 작업 폴더가 있으면 그것, 없으면 넘긴 커밋. 둘 다
 * 없으면 말하지 않는다. 커밋의 답을 아직 모르면 묻는 줄에 세운다 — 그려진 것만 묻게 된다. */
function artifactLandingSayOf(held) {
  if (held?.worktree && worktreeLandings.has(held.worktree)) return worktreeLandingSayFor(held.worktree);
  const commit = artifactLandingCommit(held);
  if (commit === "") return null;
  wantArtifactCommitLanding(commit);
  return artifactCommitLandingSay(commit);
}

/* 읽는 칸 머리의 반영 상태: 작업별 탭에서 작업을 열었으면 그 작업의 것(줄이 든 커밋까지),
 * 아니면 고른 행의 것. */
function artifactReaderLandingSay(row) {
  const line = artifactFilter.tab === ARTIFACT_TASKS_TAB && artifactTaskOpen ? artifactTaskLine(artifactTaskOpen) : null;
  return artifactLandingSayOf(line ?? row.origin);
}

/* 아직 묻지 않은 커밋을 모아 한 번에 묻는다 — 그리기가 줄마다 묻지 않게. */
function wantArtifactCommitLanding(commit) {
  if (artifactCommitsAsked.has(commit)) return;
  artifactCommitsAsked.add(commit);
  if (artifactCommitsAsked.size > ARTIFACT_COMMITS_REMEMBERED) {
    const oldest = artifactCommitsAsked.values().next().value;
    artifactCommitsAsked.delete(oldest);
    artifactCommitLandings.delete(oldest);
  }
  if (artifactCommitAskTimer !== 0) return;
  artifactCommitAskTimer = setTimeout(() => {
    artifactCommitAskTimer = 0;
    void refreshArtifactCommitLandings();
  }, 0);
}

/* 물은 커밋들의 답을 읽는다 — 새로 물을 것이 생겼을 때와, 런타임이 답이 움직였다고 말할 때
 * (`worktree:commit-landing`). 반영된 것은 다시 묻지 않는다(반영된 작업은 반영된 채다). 답이
 * 실제로 달라졌을 때만 칩을 다시 입힌다. */
async function refreshArtifactCommitLandings() {
  const commits = [...artifactCommitsAsked]
    .filter((commit) => artifactCommitLandings.get(commit)?.state !== "landed")
    .slice(-ARTIFACT_COMMITS_PER_ASK);
  if (commits.length === 0) return;
  let answer = null;
  try {
    answer = await invoke("commit_landings", { commits });
  } catch {
    return;
  }
  let moved = false;
  for (const [commit, landing] of Object.entries(answer ?? {})) {
    const before = artifactCommitLandings.get(commit);
    if (before && landingShape(before) === landingShape(landing)) continue;
    artifactCommitLandings.set(commit, landing);
    moved = true;
  }
  if (moved) noteArtifactLandingMoved();
}

/* ---- 판 짓기 -------------------------------------------------------------- */

/* 작업 목록 — 목록 상자 하나와, 그 아래 바닥의 「작업에 연결 안 된 것」. */
function buildArtifactTaskList() {
  const list = document.createElement("div");
  list.className = "artifacts-tasks";
  list.tabIndex = 0;
  list.hidden = true;
  list.setAttribute("role", "listbox");
  list.dataset.i18nAria = "artifacts.tasks.list";
  list.setAttribute("aria-label", t("artifacts.tasks.list", "작업 목록"));
  const spacer = document.createElement("div");
  spacer.className = "artifacts-tasks-spacer";
  const rows = document.createElement("div");
  rows.className = "artifacts-task-rows";
  const empty = document.createElement("p");
  empty.className = "artifacts-tasks-empty";
  empty.hidden = true;
  list.append(spacer, rows, empty);

  const loose = document.createElement("section");
  loose.className = "artifacts-loose";
  loose.hidden = true;
  const looseName = document.createElement("h3");
  looseName.className = "artifacts-loose-name";
  looseName.dataset.i18n = ARTIFACT_TASK_UNLINKED_WORD.key;
  looseName.textContent = t(ARTIFACT_TASK_UNLINKED_WORD.key, ARTIFACT_TASK_UNLINKED_WORD.word);
  const looseLines = document.createElement("ul");
  looseLines.className = "artifacts-loose-lines";
  for (const row of ARTIFACT_LOOSE_LINES) {
    const item = document.createElement("li");
    const show = document.createElement("button");
    show.type = "button";
    show.className = "artifacts-loose-show";
    show.dataset.looseTab = row.tab;
    const note = document.createElement("span");
    note.className = "artifacts-loose-note";
    item.append(show, note);
    looseLines.appendChild(item);
  }
  loose.append(looseName, looseLines);
  return [list, loose];
}

/* 읽는 칸의 묶음 구역 — 서랍의 제목 아래에 선다: 빠진 것의 한 줄, 증거, 전후 비교, 페이지. */
function buildArtifactBundle() {
  const box = document.createElement("section");
  box.className = "artifact-bundle";
  box.hidden = true;

  const banner = document.createElement("div");
  banner.className = "artifact-bundle-banner";
  banner.hidden = true;
  const bannerWord = document.createElement("p");
  bannerWord.className = "artifact-bundle-banner-word";
  const why = document.createElement("details");
  why.className = "artifact-bundle-why";
  const whyAsk = document.createElement("summary");
  whyAsk.dataset.i18n = "artifacts.bundle.whyAsk";
  whyAsk.textContent = t("artifacts.bundle.whyAsk", "왜?");
  const whyWord = document.createElement("p");
  whyWord.className = "artifact-bundle-why-word";
  whyWord.dataset.i18n = "artifacts.bundle.why";
  whyWord.textContent = t("artifacts.bundle.why", "이 파일들은 작업 폴더 안에만 있었습니다. 폴더를 지우면 함께 지워지고, 카탈로그에는 이름과 시각만 남습니다. 보존 기간이 지나면 이 줄도 사라집니다.");
  why.append(whyAsk, whyWord);
  banner.append(bannerWord, why);

  const evidence = document.createElement("div");
  evidence.className = "artifact-bundle-evidence";
  evidence.hidden = true;
  const evidenceHead = document.createElement("p");
  evidenceHead.className = "artifact-bundle-head";
  const cards = document.createElement("ul");
  cards.className = "artifact-bundle-cards";
  evidence.append(evidenceHead, cards);

  const pages = document.createElement("div");
  pages.className = "artifact-bundle-pages";
  pages.hidden = true;
  const pagesHead = document.createElement("p");
  pagesHead.className = "artifact-bundle-head";
  const pageList = document.createElement("ul");
  pageList.className = "artifact-bundle-page-list";
  pages.append(pagesHead, pageList);

  const none = document.createElement("p");
  none.className = "artifact-bundle-none";
  none.dataset.i18n = "artifacts.bundle.noReport";
  none.textContent = t("artifacts.bundle.noReport", "이 작업의 보고서가 없습니다.");
  none.hidden = true;

  box.append(banner, evidence, buildArtifactCompare(), pages, none);
  return box;
}

/* 전후 비교 — 읽는 칸의 폭 그대로. 경계의 손잡이는 끌고, 아래 막대는 키보드로 옮긴다. */
function buildArtifactCompare() {
  const compare = document.createElement("div");
  compare.className = "artifact-compare";
  compare.hidden = true;
  const head = document.createElement("header");
  head.className = "artifact-compare-head";
  const name = document.createElement("span");
  name.className = "artifact-compare-name";
  const modes = document.createElement("div");
  modes.className = "artifact-compare-modes";
  modes.setAttribute("role", "group");
  modes.dataset.i18nAria = "artifacts.compare.modes";
  modes.setAttribute("aria-label", t("artifacts.compare.modes", "비교 방식"));
  for (const row of ARTIFACT_COMPARE_MODES) {
    const one = document.createElement("button");
    one.type = "button";
    one.className = "artifact-compare-mode";
    one.dataset.compareMode = row.mode;
    one.dataset.i18n = row.key;
    one.textContent = t(row.key, row.word);
    modes.appendChild(one);
  }
  const close = document.createElement("button");
  close.type = "button";
  close.className = "btn artifact-compare-close";
  close.innerHTML = icon("x");
  close.dataset.i18nAria = "artifacts.compare.close";
  close.setAttribute("aria-label", t("artifacts.compare.close", "비교 닫기"));
  head.append(name, modes, close);

  const stage = document.createElement("div");
  stage.className = "artifact-compare-stage";
  for (const side of ["after", "before"]) {
    const figure = document.createElement("figure");
    figure.className = `artifact-compare-side is-${side}`;
    const picture = document.createElement("img");
    picture.alt = "";
    // 그림은 끌려 나가지 않는다: 그림 위의 끌기는 경계를 옮기는 손이고, 그림을 집어 드는 끌기가
    // 시작되면 포인터가 취소되어 경계가 첫 걸음에서 멈춘다.
    picture.draggable = false;
    const caption = document.createElement("figcaption");
    caption.dataset.i18n = side === "before" ? "artifacts.compare.before" : "artifacts.compare.after";
    caption.textContent = side === "before" ? t("artifacts.compare.before", "전") : t("artifacts.compare.after", "후");
    figure.append(picture, caption);
    stage.appendChild(figure);
  }
  // 손잡이는 끄는 것이다 — 키보드의 길은 아래 막대라, 탭 순서와 읽는 기계에서는 뺀다.
  const handle = document.createElement("span");
  handle.className = "artifact-compare-handle";
  handle.setAttribute("aria-hidden", "true");
  stage.appendChild(handle);

  const range = document.createElement("input");
  range.type = "range";
  range.className = "artifact-compare-range";
  range.min = "0";
  range.max = "100";
  range.step = String(ARTIFACT_COMPARE_STEP);
  range.value = String(ARTIFACT_COMPARE_START);
  range.dataset.i18nAria = "artifacts.compare.range";
  range.setAttribute("aria-label", t("artifacts.compare.range", "전과 후의 경계"));
  const trouble = document.createElement("p");
  trouble.className = "artifact-compare-trouble";
  trouble.hidden = true;
  compare.append(head, stage, range, trouble);
  return compare;
}

/* ---- 손 매기 (판마다 한 번) ----------------------------------------------- */

function wireArtifactTasks(view) {
  const list = view.querySelector(".artifacts-tasks");
  list.addEventListener("scroll", () => paintArtifactTaskRows(view), { passive: true });
  list.addEventListener("click", (event) => {
    const row = event.target.closest(".artifact-task");
    if (row?.dataset.task) openArtifactTask(view, row.dataset.task);
  });
  list.addEventListener("keydown", (event) => {
    const step = { ArrowDown: 1, ArrowUp: -1 }[event.key];
    if (step === undefined || artifactTaskLines.length === 0) return;
    event.preventDefault();
    const at = artifactTaskLines.findIndex((line) => line.task === artifactTaskOpen);
    const next = at < 0 ? 0 : Math.min(artifactTaskLines.length - 1, Math.max(0, at + step));
    openArtifactTask(view, artifactTaskLines[next].task, { reveal: true });
  });
  view.querySelector(".artifacts-loose").addEventListener("click", (event) => {
    const show = event.target.closest("[data-loose-tab]");
    if (!show) return;
    artifactTabPicked = true;
    artifactFilter.tab = show.dataset.looseTab;
    filterArtifactsByTask(view, ARTIFACT_TASK_UNLINKED);
  });
  const bundle = view.querySelector(".artifact-bundle");
  bundle.querySelector(".artifact-bundle-cards").addEventListener("click", (event) => {
    const card = event.target.closest("button.artifact-evidence");
    if (!card) return;
    if (card.dataset.before) void openArtifactCompare(view, card);
    else if (card.dataset.id) selectArtifact(view, card.dataset.id);
  });
  bundle.querySelector(".artifact-bundle-page-list").addEventListener("click", (event) => {
    const page = event.target.closest("button[data-id]");
    if (page) selectArtifact(view, page.dataset.id);
  });
  wireArtifactCompare(view);
}

function wireArtifactCompare(view) {
  const compare = view.querySelector(".artifact-compare");
  const stage = compare.querySelector(".artifact-compare-stage");
  const range = compare.querySelector(".artifact-compare-range");
  compare.querySelector(".artifact-compare-close").addEventListener("click", () => closeArtifactCompare(view));
  compare.querySelector(".artifact-compare-modes").addEventListener("click", (event) => {
    const one = event.target.closest("[data-compare-mode]");
    if (one) setArtifactCompareMode(compare, one.dataset.compareMode);
  });
  range.addEventListener("input", () => setArtifactCompareAt(compare, Number(range.value)));
  // 끌기: 누른 동안만 움직임을 듣고, 프레임에 한 번만 쓴다.
  let frame = 0;
  let wanted = ARTIFACT_COMPARE_START;
  const follow = (event) => {
    const box = stage.getBoundingClientRect();
    wanted = box.width > 0 ? ((event.clientX - box.left) / box.width) * 100 : wanted;
    if (frame !== 0) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      setArtifactCompareAt(compare, wanted);
    });
  };
  stage.addEventListener("pointerdown", (event) => {
    if (stage.dataset.compareMode !== "slide") return;
    // 포인터를 쥐어 두면 그림 밖으로 나가도 끌기가 이어진다. 이미 떠난 포인터는 쥘 수 없다 —
    // 그때는 그림 안에서만 따라간다.
    try {
      stage.setPointerCapture(event.pointerId);
    } catch {
      /* 쥐지 못해도 끌기는 된다 */
    }
    stage.addEventListener("pointermove", follow);
    follow(event);
  });
  const release = (event) => {
    stage.removeEventListener("pointermove", follow);
    if (stage.hasPointerCapture(event.pointerId)) stage.releasePointerCapture(event.pointerId);
  };
  stage.addEventListener("pointerup", release);
  stage.addEventListener("pointercancel", release);
}

/* ---- 작업 목록 그리기 ------------------------------------------------------ */

/* 줄의 수를 한 줄로: 「보고서 2 · 시도 2 · 페이지 1 · 파일 52 (그림 4 · 로그 23 · 그 밖 25) ·
 * 삭제됨 1」. 0인 부분은 서지 않지만, 괄호 안의 합은 늘 「파일」의 수다. */
function artifactTaskPartsWords(line) {
  const parts = line.parts ?? {};
  const count = (row) => t(row.key, row.word, { n: artifactCountWord(parts[row.part] ?? 0) });
  const said = ARTIFACT_TASK_PARTS.filter((row) => (parts[row.part] ?? 0) > 0).map(count);
  if ((line.attempts ?? 0) > 1) said.splice(1, 0, t("artifacts.tasks.attempts", "시도 {{n}}", { n: artifactCountWord(line.attempts) }));
  if ((parts.files ?? 0) > 0) {
    const inside = ARTIFACT_TASK_FILE_PARTS.filter((row) => (parts[row.part] ?? 0) > 0).map(count).join(" · ");
    said.push(`${t("artifacts.tasks.files", "파일 {{n}}", { n: artifactCountWord(parts.files) })} (${inside})`);
  }
  if ((line.sessions ?? 0) > 0) said.push(t("artifacts.tasks.sessions", "Computer Use 기록 {{n}}", { n: artifactCountWord(line.sessions) }));
  if ((parts.missing ?? 0) > 0) said.push(t("artifacts.tasks.missing", "삭제됨 {{n}}", { n: artifactCountWord(parts.missing) }));
  return said.join(" · ");
}

function makeArtifactTaskNode() {
  artifactTaskRowCreations += 1;
  const node = document.createElement("div");
  node.className = "artifact-task";
  node.setAttribute("role", "option");
  const head = document.createElement("span");
  head.className = "artifact-task-head";
  const id = document.createElement("span");
  id.className = "artifact-task-id";
  const landing = document.createElement("span");
  landing.className = "artifact-task-landing";
  landing.hidden = true;
  const when = document.createElement("span");
  when.className = "artifact-task-when";
  head.append(id, landing, when);
  const title = document.createElement("span");
  title.className = "artifact-task-title";
  const parts = document.createElement("span");
  parts.className = "artifact-task-parts";
  node.append(head, title, parts);
  return node;
}

/* 줄 하나의 옷 — 줄이 바뀐 때에만 글을 쓰고, 반영 칩은 그 답이 움직인 때에만 다시 입는다. */
function dressArtifactTaskRow(node, line, selected) {
  if (node._line !== line) {
    node._line = line;
    node.dataset.task = line.task;
    node.querySelector(".artifact-task-id").textContent = line.task;
    const title = node.querySelector(".artifact-task-title");
    // 제목은 그 작업을 읽을 때 열리는 행의 제목이다; 없으면 작업의 이름.
    title.textContent = line.title || line.work || line.task;
    title.dataset.tip = line.work ?? "";
    const when = node.querySelector(".artifact-task-when");
    when.textContent = artifactClockWords(line.modified_ms);
    when.dataset.tip = t("artifacts.tasks.when", "마지막 산출물 {{time}}", { time: new Date(line.modified_ms).toLocaleString() });
    node.querySelector(".artifact-task-parts").textContent = artifactTaskPartsWords(line);
  }
  if (node._landingLine !== line || node._landingGeneration !== artifactLandingGeneration) {
    node._landingLine = line;
    node._landingGeneration = artifactLandingGeneration;
    dressArtifactLanding(node.querySelector(".artifact-task-landing"), artifactLandingSayOf(line));
  }
  node.classList.toggle("is-selected", selected);
  writeAttribute(node, "aria-selected", selected ? "true" : "false");
}

/* 보이는 줄(+overscan)만 선다 — 줄의 높이는 토큰 하나라 자리는 곱셈이다. 스크롤마다 도는
 * 그리기라, 바뀐 것만 쓴다: 제자리의 줄에 같은 자리와 같은 값을 다시 쓰지 않는다. */
function paintArtifactTaskRows(view) {
  const list = view.querySelector(".artifacts-tasks");
  if (list.hidden) return;
  const tuning = artifactTuning(view);
  const rows = list.querySelector(".artifacts-task-rows");
  let pool = artifactTaskPools.get(view);
  if (!pool) {
    pool = { nodes: [], byTask: new Map() };
    artifactTaskPools.set(view, pool);
  }
  const height = tuning.taskRow;
  writeStyleValue(list.querySelector(".artifacts-tasks-spacer"), "height", `${artifactTaskLines.length * height}px`);
  const empty = list.querySelector(".artifacts-tasks-empty");
  const hideEmpty = artifactTaskLines.length > 0 || artifactError !== null;
  if (empty.hidden !== hideEmpty) empty.hidden = hideEmpty;
  // 거르개가 비운 목록과, 작업이 아직 하나도 없는 카탈로그는 다른 말을 한다.
  if (!empty.hidden) {
    const narrowed = Object.values(artifactTaskFilter()).some((held) => held !== "");
    writeTextContent(empty, narrowed ? t("artifacts.tasks.empty", "이 거르개에 맞는 작업이 없습니다.") : t("artifacts.tasks.none", "아직 작업에 연결된 아티팩트가 없습니다."));
  }
  const first = Math.max(0, Math.floor(list.scrollTop / height) - tuning.overscan);
  const last = Math.min(artifactTaskLines.length, Math.ceil((list.scrollTop + list.clientHeight) / height) + tuning.overscan);
  const wanted = new Set();
  for (let at = first; at < last; at += 1) wanted.add(artifactTaskLines[at].task);
  // 제 작업을 아직 보이고 있는 줄은 그대로 두고, 나머지는 풀로 돌아간다.
  const free = [];
  for (const node of pool.nodes) {
    const held = pool.byTask.get(node.dataset.task) === node;
    if (held && wanted.has(node.dataset.task)) continue;
    if (held) pool.byTask.delete(node.dataset.task);
    free.push(node);
  }
  for (let at = first; at < last; at += 1) {
    const line = artifactTaskLines[at];
    let node = pool.byTask.get(line.task);
    if (!node) {
      node = free.pop();
      if (!node) {
        node = makeArtifactTaskNode();
        pool.nodes.push(node);
      }
      pool.byTask.set(line.task, node);
    }
    dressArtifactTaskRow(node, line, line.task === artifactTaskOpen);
    const place = `${at * height},${height}`;
    if (node._place !== place) {
      node._place = place;
      node.style.transform = `translateY(${at * height}px)`;
      node.style.height = `${height}px`;
    }
    if (node.parentElement !== rows) rows.appendChild(node);
  }
  for (const node of free) if (node.parentElement) node.remove();
}

/* 「작업에 연결 안 된 것」의 세 줄 — 보고서에는 출처가 빈 것이 몇인지, 증거에는 Computer Use
 * 기록이 파일 몇 개 · 세션 몇 개인지 함께 적는다(파일의 수와 세션의 수는 다른 수다). */
function paintArtifactLoose(view) {
  const box = view.querySelector(".artifacts-loose");
  const unlinked = artifactTaskListing.unlinked;
  const parts = unlinked?.parts ?? {};
  const any = ARTIFACT_LOOSE_LINES.some((row) => (parts[row.part] ?? 0) > 0);
  box.hidden = !any || artifactFilter.tab !== ARTIFACT_TASKS_TAB;
  if (box.hidden) return;
  const notes = {
    reports: (unlinked.reports_without_origin ?? 0) > 0
      ? t("artifacts.loose.noOrigin", "출처가 빈 것 {{n}} 포함", { n: artifactCountWord(unlinked.reports_without_origin) })
      : "",
    files: (unlinked.session_files ?? 0) > 0
      ? t("artifacts.loose.sessions", "Computer Use 기록 파일 {{files}}개 · 세션 {{sessions}}개", { files: artifactCountWord(unlinked.session_files), sessions: artifactCountWord(unlinked.sessions ?? 0) })
      : "",
  };
  for (const row of ARTIFACT_LOOSE_LINES) {
    const show = box.querySelector(`[data-loose-tab="${row.tab}"]`);
    const n = parts[row.part] ?? 0;
    show.parentElement.hidden = n === 0;
    writeTextContent(show, t(row.key, row.word, { n: artifactCountWord(n) }));
    writeTextContent(show.nextElementSibling, notes[row.part] ?? "");
  }
}

/* 작업별 탭의 그림 전부: 목록 · 연결 안 된 것. 다른 탭에서는 목록이 숨고 카드가 선다. */
function paintArtifactTasks(view) {
  const tasks = artifactFilter.tab === ARTIFACT_TASKS_TAB;
  view.classList.toggle("is-tab-tasks", tasks);
  view.classList.toggle("is-task-open", tasks && artifactTaskOpen !== null);
  view.querySelector(".artifacts-tasks").hidden = !tasks;
  view.querySelector(".artifacts-grid").hidden = tasks;
  paintArtifactLoose(view);
  paintArtifactTaskRows(view);
}

/* ---- 읽는 칸 — 작업 하나 ---------------------------------------------------- */

/* 작업을 연다: 그 작업의 첫 행(최종 보고, 없으면 페이지)을 읽는 칸에 세운다. 묶음은 읽는 칸이
 * 그려질 때 묻는다(`paintArtifactBundle`). */
function openArtifactTask(view, task, { reveal = false } = {}) {
  const line = artifactTaskLine(task);
  if (!line) return;
  artifactTaskOpen = task;
  artifactSelectedId = line.lead ?? null;
  if (reveal) {
    const list = view.querySelector(".artifacts-tasks");
    const height = artifactTuning(view).taskRow;
    const top = artifactTaskLines.indexOf(line) * height;
    if (top < list.scrollTop) list.scrollTop = top;
    else if (top + height > list.scrollTop + list.clientHeight) list.scrollTop = top + height - list.clientHeight;
  }
  closeArtifactCompare(view);
  paintArtifactTasks(view);
  paintArtifactDrawer(view);
}

/* 읽는 칸을 닫는다 — 줄이 거르개에서 빠졌거나 탭을 떠났다. */
function closeArtifactTask() {
  artifactTaskOpen = null;
  artifactSelectedId = null;
}

/* 증거 한 줄이 말하는 것과 그 결말(`verdict` — CSS가 글리프의 색을 고른다). */
function artifactEvidenceSaid(line, row, unread) {
  if (line.tests) return { verdict: line.tests.verdict, words: artifactTestsWords(line.tests) };
  if (line.steps) {
    const words = t("artifacts.bundle.steps", "Computer Use 기록 · 이 작업의 단계 {{n}} / 전체 {{total}}", { n: artifactCountWord(line.steps.of_task ?? 0), total: artifactCountWord(line.steps.total ?? 0) });
    return { verdict: "", words };
  }
  const size = artifactBytesWord(row.bytes ?? 0);
  return { verdict: "", words: unread ? `${size} · ${t("artifacts.bundle.unread", "읽지 않음")}` : size };
}

function artifactEvidenceCard(className, name, said) {
  const item = document.createElement("li");
  const card = document.createElement("button");
  card.type = "button";
  card.className = `artifact-evidence ${className}`.trim();
  card.append(
    artifactLine("span", "artifact-evidence-name", name),
    artifactLine("span", "artifact-evidence-said", said),
  );
  item.appendChild(card);
  return card;
}

/* 한 행의 이름 — 증거는 제 파일 이름으로 가린다(작업의 이름이 아니라). */
function artifactFileName(row) {
  return row?.path ? basename(row.path) : row?.title ?? "";
}

/* 묶음을 읽는 칸에 그린다 — 작업별 탭에서, 작업을 열었을 때만. 목록의 줄은 묶음이 바뀐 때에만
 * 다시 짓는다(같은 묶음에서 다른 행을 고르면 고른 표시만 옮긴다). */
function paintArtifactBundle(view) {
  const box = view.querySelector(".artifact-bundle");
  const task = artifactFilter.tab === ARTIFACT_TASKS_TAB ? artifactTaskOpen : null;
  // 아직 묻지 않은 묶음(작업을 방금 열었거나 카탈로그가 움직였다)은 한 번 묻고, 답이 오면 읽는 칸을
  // 다시 그린다 — 그 작업의 첫 행이 창에 없었다면 그 행도 그때 선다.
  if (task && !artifactBundles.has(task)) {
    void askArtifactBundle(task).then(() => {
      if (artifactTaskOpen === task && !view.hidden) paintArtifactDrawer(view);
    });
  }
  const bundle = task ? artifactBundles.get(task) ?? null : null;
  box.hidden = bundle === null;
  if (bundle === null) {
    box._bundle = null;
    return;
  }
  if (box._bundle !== bundle) {
    box._bundle = bundle;
    paintArtifactBundleLists(view, box, bundle);
  }
  for (const card of box.querySelectorAll("[data-id]")) {
    const current = card.dataset.id === artifactSelectedId;
    card.classList.toggle("is-selected", current);
    if (current) card.setAttribute("aria-current", "true");
    else card.removeAttribute("aria-current");
  }
}

function paintArtifactBundleLists(view, box, bundle) {
  const rows = new Map((bundle.rows ?? []).map((row) => [row.id, row]));
  const missing = (bundle.missing ?? []).map((id) => rows.get(id)).filter(Boolean);
  const line = bundle.line ?? {};
  const parts = line.parts ?? {};

  // 빠진 것의 한 줄 — 제품의 말로, 무엇이 왜 없고 무엇이 남았는지.
  const banner = box.querySelector(".artifact-bundle-banner");
  banner.hidden = missing.length === 0;
  if (missing.length > 0) {
    const gone = line.worktree && !worktreeAt(line.worktree);
    const lost = gone
      ? t("artifacts.bundle.lostWithFolder", "증거 {{n}}개가 없습니다 — 작업 폴더를 정리할 때 함께 삭제됐습니다.", { n: artifactCountWord(missing.length) })
      : t("artifacts.bundle.lost", "증거 {{n}}개가 없습니다 — 파일이 삭제됐습니다.", { n: artifactCountWord(missing.length) });
    const kept = (parts.reports ?? 0) > 0 ? ` ${t("artifacts.bundle.reportsKept", "보고서는 남아 있습니다.")}` : "";
    box.querySelector(".artifact-bundle-banner-word").textContent = lost + kept;
    box.querySelector(".artifact-bundle-why").open = false;
  }

  // 증거: 그림(짝은 한 장의 카드), 파일마다 한 줄, 그리고 빠진 것(줄을 긋고 「삭제됨」).
  const cards = [];
  for (const picture of bundle.pictures ?? []) {
    if (picture.before && picture.after) {
      const pair = t("artifacts.bundle.pair", "전·후 그림 · {{name}}", { name: picture.name || artifactFileName(rows.get(picture.after)) });
      const card = artifactEvidenceCard("is-pair", pair, t("artifacts.bundle.pairSaid", "눌러서 비교"));
      card.dataset.before = picture.before;
      card.dataset.after = picture.after;
      card.dataset.name = picture.name ?? "";
      cards.push(card);
    } else if (picture.single && rows.has(picture.single)) {
      const row = rows.get(picture.single);
      const card = artifactEvidenceCard("is-picture", artifactFileName(row), t(ARTIFACT_KIND_WORDS.screenshot.key, ARTIFACT_KIND_WORDS.screenshot.word));
      card.dataset.id = row.id;
      cards.push(card);
    }
  }
  const evidence = bundle.evidence ?? [];
  evidence.forEach((held, at) => {
    const row = rows.get(held.id);
    if (!row) return;
    const said = artifactEvidenceSaid(held, row, at >= evidence.length - (bundle.unread ?? 0));
    const card = artifactEvidenceCard("", artifactFileName(row), said.words);
    card.dataset.id = row.id;
    if (said.verdict) card.dataset.verdict = said.verdict;
    cards.push(card);
  });
  for (const row of missing) {
    const card = artifactEvidenceCard("is-lost", artifactFileName(row), t("artifacts.bundle.deleted", "삭제됨"));
    card.dataset.id = row.id;
    cards.push(card);
  }
  const list = box.querySelector(".artifact-bundle-cards");
  list.replaceChildren(...cards.map((card) => card.parentElement));
  const block = box.querySelector(".artifact-bundle-evidence");
  block.hidden = cards.length === 0;
  // 수는 그 자리에서 더한 값이고(묶음의 `tally`), 부분은 바로 아래 줄마다 서 있다.
  const tally = bundle.tally ?? {};
  const head = [t("artifacts.bundle.evidence", "이 작업의 증거 {{n}}", { n: artifactCountWord((parts.files ?? 0) + missing.length) })];
  if ((tally.runs ?? 0) > 0) {
    head.push(t("artifacts.bundle.tally", "시험 통과 {{passed}} · 실패 {{failed}}", { passed: artifactCountWord(tally.passed ?? 0), failed: artifactCountWord(tally.failed ?? 0) }));
    if ((tally.ignored ?? 0) > 0) head.push(t("artifacts.tests.ignored", "무시됨 {{n}}", { n: artifactCountWord(tally.ignored) }));
    if ((tally.intended ?? 0) > 0) head.push(t("artifacts.bundle.intended", "의도한 실패 {{n}}건", { n: artifactCountWord(tally.intended) }));
  }
  block.querySelector(".artifact-bundle-head").textContent = head.join(" · ");

  // 이 작업의 페이지·문서.
  const pages = (bundle.pages ?? []).map((id) => rows.get(id)).filter(Boolean);
  const pageBlock = box.querySelector(".artifact-bundle-pages");
  pageBlock.hidden = pages.length === 0;
  pageBlock.querySelector(".artifact-bundle-head").textContent = t("artifacts.bundle.pages", "이 작업의 페이지·문서 {{n}}", { n: artifactCountWord(pages.length) });
  pageBlock.querySelector(".artifact-bundle-page-list").replaceChildren(...pages.map((row) => {
    const item = document.createElement("li");
    const page = document.createElement("button");
    page.type = "button";
    page.className = "artifact-bundle-page";
    page.dataset.id = row.id;
    page.append(
      artifactLine("span", "artifact-bundle-page-title", row.title),
      artifactLine("span", "artifact-bundle-page-said", `${artifactKindWord(row)} · ${artifactClockWords(row.modified_ms)}`),
    );
    item.appendChild(page);
    return item;
  }));
  box.querySelector(".artifact-bundle-none").hidden = (parts.reports ?? 0) > 0;
  closeArtifactCompare(view);
}

/* ---- 전후 비교 -------------------------------------------------------------- */

function setArtifactCompareMode(compare, mode) {
  compare.querySelector(".artifact-compare-stage").dataset.compareMode = mode;
  // 눌린 표시는 단추의 것이다 — 그림 자리도 같은 이름의 속성으로 제 방식을 적으니 단추만 고른다.
  for (const one of compare.querySelectorAll(".artifact-compare-mode")) {
    const on = one.dataset.compareMode === mode;
    one.setAttribute("aria-pressed", on ? "true" : "false");
  }
  // 나란히 볼 때는 경계가 없다 — 막대도 선다 할 것이 없다.
  compare.querySelector(".artifact-compare-range").hidden = mode !== "slide";
}

/* 경계의 자리(0~100) — 손잡이와 막대와 가려진 폭이 한 값을 읽는다. */
function setArtifactCompareAt(compare, at) {
  const held = Math.min(100, Math.max(0, Math.round(at)));
  compare.querySelector(".artifact-compare-stage").style.setProperty("--artifact-compare-at", `${held}%`);
  const range = compare.querySelector(".artifact-compare-range");
  if (Number(range.value) !== held) range.value = String(held);
}

/* 짝의 카드를 눌렀다: 두 그림을 불러 비교를 연다. 그림을 읽지 못하면(표 상한보다 크다) 그렇게
 * 말한다 — 빈 상자를 보이지 않는다. */
async function openArtifactCompare(view, card) {
  const compare = view.querySelector(".artifact-compare");
  const asked = `${card.dataset.before}:${card.dataset.after}`;
  compare.dataset.pair = asked;
  compare.hidden = false;
  compare.querySelector(".artifact-compare-name").textContent = t("artifacts.compare.name", "전·후 비교 · {{name}}", { name: card.dataset.name || card.querySelector(".artifact-evidence-name").textContent });
  setArtifactCompareMode(compare, compare.querySelector(".artifact-compare-stage").dataset.compareMode || ARTIFACT_COMPARE_MODES[0].mode);
  setArtifactCompareAt(compare, ARTIFACT_COMPARE_START);
  const [before, after] = await Promise.all([askArtifactPreview(card.dataset.before), askArtifactPreview(card.dataset.after)]);
  if (compare.dataset.pair !== asked) return;
  const shown = Boolean(before?.data_url && after?.data_url);
  compare.querySelector(".is-before img").src = before?.data_url ?? "";
  compare.querySelector(".is-after img").src = after?.data_url ?? "";
  compare.querySelector(".artifact-compare-stage").hidden = !shown;
  const trouble = compare.querySelector(".artifact-compare-trouble");
  trouble.hidden = shown;
  trouble.textContent = shown ? "" : t("artifacts.compare.trouble", "그림을 읽지 못했습니다 — 카드에서 하나씩 열어 보세요.");
  compare.querySelector(".artifact-compare-close").focus({ preventScroll: true });
}

function closeArtifactCompare(view) {
  const compare = view.querySelector(".artifact-compare");
  if (compare.hidden) return;
  compare.hidden = true;
  compare.dataset.pair = "";
  for (const picture of compare.querySelectorAll("img")) picture.removeAttribute("src");
}

/* 런타임의 말: 커밋의 반영 상태가 움직였다 — 물은 것들을 다시 읽는다. */
listen("worktree:commit-landing", () => {
  void refreshArtifactCommitLandings();
});
