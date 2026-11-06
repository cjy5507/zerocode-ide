import { writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { createWindowServer, launchWindowBrowser, openWindowTestPage } from "./window-boot.mjs";

/* 만들기·열기 창 (2026-11-06) — the person: 「폴더 생성 막고있는 부분 개선 … 생성하는 디자인도 개선」,
 * then 「intellij처럼 편하게 … 막히는 구간 없이」, then 「기능 테스트까지 완벽하게」.
 *
 * Every scene presses the drawn window over a synthetic backend (paths under /Users/dev, a home of
 * /Users/tester as the harness names it). What is held:
 *
 *   ① the composer says where the worktree will be made and from what before the press — the backend's own
 *      plan (`plan_worktree`), asked again when the place is typed, and back to the setting with 기본값으로;
 *   ② 「다음에도 이 위치 사용」 makes the place the setting before the create reads it;
 *   ③ a repository with no commit: the note offers the empty first commit, ticked; unticked the button is
 *      dead; the create carries `firstCommit`;
 *   ④ a folder project keeps its ＋, the composer lists it as 「git 아님」, and 「git으로 시작하기」 makes it
 *      git — the button comes alive once the backend says so;
 *   ⑤ a create refused for want of a commit says the whole reason, and its retry makes the commit;
 *   ⑥ the projects window opens on its search: recent rows (home written `~`), a filter, a typed path that is a
 *      folder opens it, a typed path that is not there starts a new project there;
 *   ⑦ with no project yet it is IntelliJ's first run: three cards;
 *   ⑧ 새 프로젝트 shows where it will be made (the backend's plan), says a refused name under the name and a
 *      taken folder with the door to open it, and carries the Git box;
 *   ⑨ 복제 fills the whole folder from the address until the person edits it, and clones into what was edited;
 *   ⑩ the folder browser makes a folder (button, ⇧⌘N, a typed path that is missing) and stands in it; Escape
 *      folds the name row before it closes the browser; rows stand at the control height.
 *
 *   node ui/tests/create-flow.mjs [--json <file>]
 */

const PROJECT = "/tmp/zerocode-window-test";
const FOLDER = "/Users/dev/notes";
const HOME = "/Users/tester";

const helpers = () => {
  window.__T__ = {
    tick: () => new Promise((done) => setTimeout(done, 0)),
    wait: (ms) => new Promise((done) => setTimeout(done, ms)),
    async settle(until, budget = 2000) {
      const start = performance.now();
      while (!until() && performance.now() - start < budget) await new Promise((done) => setTimeout(done, 10));
      return until();
    },
    input(node, value) {
      node.value = value;
      node.dispatchEvent(new Event("input", { bubbles: true }));
    },
    key(node, init) {
      node.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }));
    },
  };
};

async function scene(browser, origin, ok, name, run, arg) {
  const faults = [];
  const { page } = await openWindowTestPage(browser, origin, { faults });
  await page.waitForFunction(() => typeof projectsRead !== "undefined" && projectsRead === true);
  await page.evaluate(helpers);
  let seen;
  try {
    seen = await page.evaluate(run, arg);
  } finally {
    await page.close();
  }
  if (faults.length) ok(`${name}: the page raised nothing`, false, faults.slice(0, 2).join(" | "));
  return seen;
}

// ①–③ — the composer over a git project.
const composer = async ({ project }) => {
  const { settle, input, wait } = window.__T__;
  const seen = { plans: [], created: [], prefs: [] };
  window.__PROJECT__ = {
    file: "/tmp/zerocode/zerocode.yaml", exists: false, unreadable: false,
    runs_setup: true, source: "shared-only", setup_run_policy: "run-by-default",
  };
  let unborn = false;
  window.__ANSWER__.plan_worktree = (args) => {
    seen.plans.push(args);
    return unborn
      ? { git: true, commits: false, path: "~/acme/wt/first", branch: "wt/first", base: null, new_folders: 1 }
      : { git: true, commits: true, path: "~/acme/wt/fix-login", branch: "wt/fix-login", base: "main", new_folders: 2 };
  };
  window.__ANSWER__.create_worktree = (args) => {
    seen.created.push(args);
    return {
      path: `${project}/.worktrees/made-${seen.created.length}`, branch: "wt/x", is_main: false,
      is_folder: false, prunable: false, active: false, setup: { kind: "nothing" },
    };
  };
  window.__ANSWER__.patch_workspace_creation_prefs = (args) => {
    seen.prefs.push({ args, created: seen.created.length });
    return null;
  };
  const words = {
    badge: t("worktree.planNewFolders", "새 폴더 {{count}}", { count: 2 }),
    fromMain: t("worktree.planFrom", " · {{base}}에서", { base: "main" }),
    fromFirst: t("worktree.planFromFirst", " · 빈 첫 커밋에서"),
  };

  // ① what will be made, before the press.
  openWorktreeFormForProject(project);
  await settle(() => el("wt-plan-path").textContent.includes("fix-login"));
  seen.path = el("wt-plan-path").textContent;
  seen.bold = el("wt-plan-path").querySelector("code b")?.textContent ?? null;
  seen.badge = el("wt-plan-path").querySelector(".wt-plan-new")?.textContent ?? null;
  seen.branch = el("wt-plan-branch").textContent;
  seen.noteAtOpen = el("wt-project-note").hidden;
  seen.resetAtOpen = el("wt-directory-reset").hidden;
  seen.rememberAtOpen = el("wt-directory-remember-row").hidden;
  seen.setting = workspaceCreationPrefs.directory;
  seen.fieldAtOpen = el("wt-directory").value;
  // One control height for every field (Geist's 32 px), and 더 만들기's switch is
  // drawn — it stood 0×0 under the dialog's own input rule before.
  seen.heights = {
    project: el("wt-project").closest(".wt-combo")?.offsetHeight ?? null,
    base: el("wt-base").closest(".wt-combo")?.offsetHeight ?? null,
    agent: el("wt-agent").closest(".wt-combo")?.offsetHeight ?? null,
    directory: el("wt-directory").closest(".wt-box")?.offsetHeight ?? null,
  };
  seen.control = parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--control-height-md"));
  seen.switchBox = { width: el("wt-more").offsetWidth, height: el("wt-more").offsetHeight };
  const field = el("wt-directory");
  field.focus();
  input(field, "~/acme/elsewhere");
  await settle(() => seen.plans.at(-1)?.directory === "~/acme/elsewhere");
  seen.askedTyped = seen.plans.at(-1)?.directory ?? null;
  seen.resetShown = !el("wt-directory-reset").hidden;
  seen.rememberShown = !el("wt-directory-remember-row").hidden;
  seen.typedKept = field.value;
  // A repaint asks nothing: the ask follows what would be made, not the paint.
  const asked = seen.plans.length;
  paintWorktreeName();
  paintWorktreeName();
  await wait(200);
  seen.repaintAsked = seen.plans.length - asked;
  // Typing the work asks again — the folder follows the name.
  input(el("wt-spec"), "fix login");
  await settle(() => seen.plans.at(-1)?.spec === "fix login");
  seen.askedSpec = seen.plans.at(-1)?.spec ?? null;
  field.blur();
  const beforeReset = seen.plans.length;
  el("wt-directory-reset").click();
  await settle(() => seen.plans.length > beforeReset);
  seen.afterReset = {
    value: field.value,
    reset: el("wt-directory-reset").hidden,
    remember: el("wt-directory-remember-row").hidden,
    asked: seen.plans.at(-1)?.directory ?? null,
  };

  // ② remembered: the setting moves before the create reads it.
  input(field, "~/acme/kept");
  await settle(() => !el("wt-directory-remember-row").hidden);
  el("wt-directory-remember").checked = true;
  await settle(() => !el("wt-create").disabled);
  el("wt-create").click();
  await settle(() => seen.created.length === 1);
  seen.rememberedFirst = seen.prefs.find((one) => one.args?.patch?.kind === "directory") ?? null;
  seen.settingAfter = workspaceCreationPrefs.directory;
  seen.createdDirectory = seen.created[0]?.directory ?? null;
  seen.createdFirstCommit = seen.created[0]?.firstCommit ?? null;
  await settle(() => wtNewScrim.hidden);

  // ③ a repository with no commit.
  unborn = true;
  openWorktreeFormForProject(project);
  await settle(() => !el("wt-project-note").hidden);
  const note = el("wt-project-note");
  const box = el("wt-first-commit");
  seen.unborn = {
    words: note.textContent,
    ticked: box?.checked ?? null,
    branch: el("wt-plan-branch").textContent,
  };
  input(el("wt-spec"), "first");
  await settle(() => !el("wt-create").disabled);
  seen.unborn.aliveTicked = !el("wt-create").disabled;
  box.checked = false;
  box.dispatchEvent(new Event("change"));
  seen.unborn.deadUnticked = el("wt-create").disabled;
  box.checked = true;
  box.dispatchEvent(new Event("change"));
  seen.unborn.aliveAgain = !el("wt-create").disabled;
  el("wt-create").click();
  await settle(() => seen.created.length === 2);
  seen.unborn.firstCommit = seen.created[1]?.firstCommit ?? null;
  await settle(() => wtNewScrim.hidden);

  // One preview out at a time: changes behind a slow answer make ONE more ask,
  // with the last of them — and the base the first answer found rides it.
  const before = seen.plans.length;
  let release = null;
  let out = 0;
  let outMax = 0;
  window.__ANSWER__.plan_worktree = (args) => {
    seen.plans.push(args);
    out += 1;
    outMax = Math.max(outMax, out);
    return new Promise((done) => {
      release = () => {
        out -= 1;
        release = null;
        done({ git: true, commits: true, path: "~/acme/wt/slow", branch: "wt/slow", base: "main", new_folders: 1 });
      };
    });
  };
  openWorktreeFormForProject(project);
  await settle(() => release !== null);
  for (const typed of ["a", "ab", "abc"]) {
    input(el("wt-spec"), typed);
    await wait(180);
  }
  seen.single = { askedWhileOut: seen.plans.length - before };
  release();
  await settle(() => release !== null);
  seen.single.trailing = seen.plans.at(-1)?.spec ?? null;
  seen.single.base = seen.plans.at(-1)?.base ?? null;
  seen.single.firstBase = seen.plans[before]?.base ?? null;
  seen.single.asked = seen.plans.length - before;
  release();
  await wait(60);
  seen.single.outMax = outMax;
  seen.single.painted = el("wt-plan-path").textContent.includes("~/acme/wt/slow");
  setWorktreeForm(false);
  seen.words = words;
  return seen;
};

// ④ — a folder project.
const folderProject = async ({ project, folder }) => {
  const { settle, input } = window.__T__;
  const seen = { plans: [], started: [] };
  window.__PROJECT__ = {
    file: "/tmp/zerocode/zerocode.yaml", exists: false, unreadable: false,
    runs_setup: true, source: "shared-only", setup_run_policy: "run-by-default",
  };
  const standing = structuredClone(projects);
  let isGit = false;
  const folderEntry = () => ({
    name: "notes", path: folder, slug: null,
    worktrees: [{
      path: folder, branch: isGit ? "main" : null, is_main: true, active: false, is_folder: !isGit,
      ownership: "zerocode-managed", external_hidden: false,
    }],
    external: { visibility: "show", legacy: false, authoritative: false, shown: 0, hidden: [], prompt: false, inbox: [] },
  });
  window.__ANSWER__.project_catalog = () => [...standing, folderEntry()];
  window.__ANSWER__.plan_worktree = (args) => {
    seen.plans.push(args);
    if (args.project !== folder) {
      return { git: true, commits: true, path: "~/acme/wt/x", branch: "wt/x", base: "main", new_folders: 1 };
    }
    return isGit
      ? { git: true, commits: true, path: "~/notes-wt/idea", branch: "wt/idea", base: null, new_folders: 1 }
      : { git: false, commits: false, path: null, branch: null, base: null, new_folders: 0 };
  };
  window.__ANSWER__.start_git = (args) => {
    seen.started.push(args);
    isGit = true;
    return folder;
  };
  await refreshWorktrees();
  await settle(() => Boolean(document.querySelector(`[data-project-path="${CSS.escape(folder)}"]`)));
  const header = document.querySelector(`[data-project-path="${CSS.escape(folder)}"]`);
  const plus = header?.querySelector(".project-worktree-new");
  seen.plusShown = Boolean(plus) && !plus.hidden;
  plus?.click();
  await settle(() => !wtNewScrim.hidden && !el("wt-project-note").hidden);
  seen.picked = el("wt-project").value;
  seen.listed = [...el("wt-project").options].map((one) => one.value);
  seen.tag = el("wt-project-tag").textContent.trim();
  seen.note = el("wt-project-note").textContent;
  input(el("wt-spec"), "idea");
  await settle(() => seen.plans.at(-1)?.spec === "idea");
  seen.deadWhileFolder = el("wt-create").disabled;
  const start = el("wt-start-git");
  seen.startShown = Boolean(start);
  start?.click();
  await settle(() => seen.started.length === 1 && el("wt-project-note").hidden && !el("wt-create").disabled);
  seen.startedWith = seen.started[0] ?? null;
  seen.noteGone = el("wt-project-note").hidden;
  seen.aliveAfter = !el("wt-create").disabled;
  seen.tagAfter = el("wt-project-tag").textContent.trim();
  seen.words = {
    notGit: t("worktree.projectNotGit", "git 아님"),
    startGit: t("worktree.startGit", "git으로 시작하기"),
  };
  setWorktreeForm(false);
  return seen;
};

// ④b — 「git으로 시작하기」 whose `git init` lands and whose first commit is refused
// (no user name on the machine): the folder is a repository now, so the form reads the
// project and its plan again instead of offering a second start the backend refuses
// (v1.1.128 review t-97103 P2-2).
const folderStartHalfFails = async ({ project, folder }) => {
  const { settle, input } = window.__T__;
  const seen = { plans: [], started: [] };
  window.__PROJECT__ = {
    file: "/tmp/zerocode/zerocode.yaml", exists: false, unreadable: false,
    runs_setup: true, source: "shared-only", setup_run_policy: "run-by-default",
  };
  const standing = structuredClone(projects);
  let isGit = false;
  const folderEntry = () => ({
    name: "notes", path: folder, slug: null,
    worktrees: [{
      path: folder, branch: isGit ? "main" : null, is_main: true, active: false, is_folder: !isGit,
      ownership: "zerocode-managed", external_hidden: false,
    }],
    external: { visibility: "show", legacy: false, authoritative: false, shown: 0, hidden: [], prompt: false, inbox: [] },
  });
  window.__ANSWER__.project_catalog = () => [...standing, folderEntry()];
  window.__ANSWER__.plan_worktree = (args) => {
    seen.plans.push(args);
    if (args.project !== folder) {
      return { git: true, commits: true, path: "~/acme/wt/x", branch: "wt/x", base: "main", new_folders: 1 };
    }
    return isGit
      ? { git: true, commits: false, path: "~/notes-wt/idea", branch: "wt/idea", base: null, new_folders: 1 }
      : { git: false, commits: false, path: null, branch: null, base: null, new_folders: 0 };
  };
  window.__ANSWER__.start_git = (args) => {
    seen.started.push(args);
    isGit = true;
    throw new Error("첫 커밋을 만들지 못했습니다: Author identity unknown");
  };
  await refreshWorktrees();
  await settle(() => Boolean(document.querySelector(`[data-project-path="${CSS.escape(folder)}"]`)));
  document.querySelector(`[data-project-path="${CSS.escape(folder)}"]`)?.querySelector(".project-worktree-new")?.click();
  await settle(() => !wtNewScrim.hidden && !el("wt-project-note").hidden);
  input(el("wt-spec"), "idea");
  await settle(() => seen.plans.at(-1)?.spec === "idea");
  const plansBefore = seen.plans.length;
  el("wt-start-git")?.click();
  await settle(() => seen.started.length === 1 && seen.plans.length > plansBefore && Boolean(document.getElementById("wt-first-commit")));
  seen.error = el("wt-new-error").textContent;
  seen.replanned = seen.plans.length > plansBefore;
  // The note now says the repository has no commit and offers the empty first
  // commit — not the start the backend would refuse.
  seen.firstCommitOffered = Boolean(document.getElementById("wt-first-commit"));
  seen.startStill = Boolean(document.getElementById("wt-start-git"));
  seen.tagAfter = el("wt-project-tag").textContent.trim();
  seen.notGit = t("worktree.projectNotGit", "git 아님");
  setWorktreeForm(false);
  return seen;
};

// ④c — the clone step's 「…」 opens the picker on the place the folder goes into, not
// on the folder itself, which does not exist yet (v1.1.128 review t-97103 P2-3).
const clonePickerStart = async () => {
  const seen = {};
  const real = openPathBrowser;
  openPathBrowser = async (options) => {
    seen.start = options?.start ?? null;
    return [];
  };
  try {
    el("addproj-parent").value = "~/code/new-repo";
    await pickAddProjectParent(el("addproj-parent"));
  } finally {
    openPathBrowser = real;
  }
  return seen;
};

// ⑤ — a create the repository refused.
const failedCreate = async ({ project }) => {
  const { settle } = window.__T__;
  const seen = { created: [] };
  const refusal = "이 저장소에는 아직 커밋이 없어 작업 트리를 만들 수 없습니다. 빈 첫 커밋을 만든 뒤 다시 시도하세요.";
  let refuse = true;
  window.__ANSWER__.plan_worktree = () =>
    ({ git: true, commits: !refuse, path: "~/acme/wt/drain", branch: "wt/drain", base: null, new_folders: 1 });
  window.__ANSWER__.create_worktree = (args) => {
    seen.created.push(args);
    if (refuse) return Promise.reject(refusal);
    return {
      path: `${project}/.worktrees/drain`, branch: "wt/drain", is_main: false, is_folder: false,
      prunable: false, active: false, setup: { kind: "nothing" },
    };
  };
  const row = () => document.querySelector("#worktrees .wt-pending");
  startWorktreeCreation({
    spec: "drain gate", project, runSetup: null, base: null, branch: null,
    reuseBranch: false, agent: null, focus: false,
  });
  const retryWords = t("worktree.retryWithFirstCommit", "첫 커밋 만들고 다시 시도");
  await settle(() => row()?.querySelector(".wt-pending-retry")?.textContent === retryWords);
  const phase = row()?.querySelector(".wt-pending-phase");
  seen.failed = row()?.classList.contains("is-failed") ?? false;
  seen.reason = phase?.textContent ?? null;
  seen.wraps = phase ? getComputedStyle(phase).whiteSpace : null;
  seen.retry = row()?.querySelector(".wt-pending-retry")?.textContent ?? null;
  refuse = false;
  row().querySelector(".wt-pending-retry").click();
  await settle(() => seen.created.length === 2 && !row());
  seen.retried = seen.created[1] ?? null;
  seen.rowGone = !row();
  seen.retryWords = retryWords;
  return seen;
};

// ⑥–⑦ — the projects window.
const hub = async ({ project, home }) => {
  const { settle, input, key, wait } = window.__T__;
  const seen = { kinds: [], opened: [] };
  const site = `${home}/code/site`;
  const standing = structuredClone(projects);
  window.__ANSWER__.project_catalog = () => [
    ...standing,
    {
      name: "site", path: site, slug: null,
      worktrees: [{ path: site, branch: "trunk", is_main: true, active: false, is_folder: false,
        ownership: "zerocode-managed", external_hidden: false }],
      external: { visibility: "show", legacy: false, authoritative: false, shown: 0, hidden: [], prompt: false, inbox: [] },
    },
  ];
  window.__ANSWER__.path_kinds = ({ paths }) => {
    seen.kinds.push(paths);
    return paths.map((one) => (one === `${home}/code/site` ? "folder" : "missing"));
  };
  const opening = window.openProjectAt;
  window.openProjectAt = (path) => {
    seen.opened.push(path);
    return Promise.resolve(path);
  };
  try {
    await refreshWorktrees();
    el("project-open").click();
    await settle(() => !el("addproj-scrim").hidden && el("addproj-list").querySelectorAll(".addproj-recent").length >= 2);
    await wait(60);
    const rows = () => [...el("addproj-list").querySelectorAll(".addproj-recent")];
    seen.focus = document.activeElement?.id ?? null;
    seen.recent = rows().map((one) => ({
      path: one.dataset.path,
      shown: one.querySelector(".addproj-recent-path")?.textContent ?? null,
      meta: one.querySelector(".addproj-recent-meta")?.textContent ?? null,
    }));
    seen.firstSelected = rows()[0]?.classList.contains("is-sel") ?? false;
    // A filter by name.
    input(el("addproj-q"), "sit");
    seen.filtered = rows().map((one) => one.dataset.path);
    // A path that is a folder: one row, and Enter opens it by its real path.
    input(el("addproj-q"), "~/code/site");
    await settle(() => rows().length === 1 && rows()[0].dataset.act === "open");
    seen.pathRow = rows().map((one) => ({ act: one.dataset.act, path: one.dataset.path }));
    // The disk is asked about the path as the backend reads it — `~` is home.
    seen.askedSite = seen.kinds.some((paths) => paths[0] === site);
    key(el("addproj-q"), { key: "Enter" });
    await settle(() => seen.opened.length === 1);
    seen.openedPath = seen.opened[0] ?? null;
    seen.closedOnOpen = el("addproj-scrim").hidden;
    // A path that is not there: Enter starts a new project there.
    el("project-open").click();
    await settle(() => !el("addproj-scrim").hidden);
    input(el("addproj-q"), "~/code/new-app");
    await settle(() => rows().length === 1 && rows()[0].dataset.act === "make");
    seen.askedNew = seen.kinds.some((paths) => paths[0] === `${home}/code/new-app`);
    seen.makeRow = rows().map((one) => ({ act: one.dataset.act, path: one.dataset.path }));
    key(el("addproj-q"), { key: "Enter" });
    await settle(() => !el("addproj-create-step").hidden);
    seen.made = {
      step: !el("addproj-create-step").hidden,
      parent: el("addproj-new-parent").value,
      name: el("addproj-new-name").value,
      back: !el("addproj-back").hidden,
    };
    el("addproj-back").click();
    seen.backToStart = !el("addproj-start").hidden;
    setAddProject(false);
    // ⑦ no project yet.
    const held = projects;
    projects = [];
    setAddProject(true);
    seen.welcome = {
      shown: !el("addproj-welcome").hidden,
      listHidden: el("addproj-list").hidden,
      cards: el("addproj-welcome").querySelectorAll(".addproj-card").length,
    };
    el("addproj-welcome").querySelector('[data-addproj="clone"]').click();
    seen.welcome.cardOpensClone = !el("addproj-clone-step").hidden;
    setAddProject(false);
    projects = held;
  } finally {
    window.openProjectAt = opening;
  }
  seen.words = { folder: t("project.metaFolder", "폴더") };
  return seen;
};

// ⑧–⑨ — the two forms.
const forms = async () => {
  const { settle, input, wait } = window.__T__;
  const seen = { planned: [], made: [], cloned: [], opened: [], errors: [] };
  window.__ANSWER__.plan_project_folder = (args) => {
    seen.planned.push(args);
    if (args.name === "taken") {
      return { target: "~/acme/taken", new_folders: 0, problem: "~/acme/taken: 이미 있습니다", name_problem: false,
        exists: "/Users/dev/acme/taken" };
    }
    if (args.name === "-bad") {
      return { target: null, new_folders: 0, problem: "-bad: 폴더 이름으로 쓸 수 없습니다", name_problem: true, exists: null };
    }
    return { target: `~/acme/${args.name}`, new_folders: 2, problem: null, name_problem: false, exists: null };
  };
  window.__ANSWER__.create_project = (args) => {
    seen.made.push(args);
    return { path: `/Users/dev/acme/${args.name}`, first_commit_refused: "Author identity unknown" };
  };
  window.__ANSWER__.clone_repository = (args) => {
    seen.cloned.push(args);
    return `${args.parent}/${args.name}`;
  };
  const opening = window.openProjectAt;
  const erring = window.showError;
  window.openProjectAt = (path) => {
    seen.opened.push(path);
    return Promise.resolve(path);
  };
  window.showError = (error) => seen.errors.push(String(error));
  try {
    // ⑧ 새 프로젝트.
    setAddProject(true, "create");
    await settle(() => el("addproj-new-parent").value !== "");
    seen.parent = el("addproj-new-parent").value;
    const name = el("addproj-new-name");
    input(name, "site");
    seen.typedLine = el("addproj-new-target").textContent;
    await settle(() => el("addproj-new-target").textContent.includes("~/acme/site"));
    seen.planLine = el("addproj-new-target").textContent;
    seen.planBadge = el("addproj-new-target").querySelector(".wt-plan-new")?.textContent ?? null;
    seen.aliveNamed = !el("addproj-create-go").disabled;
    input(name, "-bad");
    await settle(() => el("addproj-new-name-error").textContent !== "");
    seen.bad = {
      said: el("addproj-new-name-error").textContent,
      wrong: el("addproj-new-name-box").classList.contains("is-wrong"),
      dead: el("addproj-create-go").disabled,
      line: el("addproj-new-target").textContent,
    };
    input(name, "taken");
    await settle(() => !el("addproj-new-taken").hidden);
    seen.taken = {
      said: el("addproj-new-taken").textContent,
      dead: el("addproj-create-go").disabled,
      door: Boolean(el("addproj-new-taken").querySelector(".wt-note-btn")),
      nameClear: el("addproj-new-name-error").textContent === "",
    };
    input(name, "site");
    await settle(() => el("addproj-new-taken").hidden && !el("addproj-create-go").disabled);
    const git = el("addproj-new-git");
    seen.gitAtOpen = git.checked;
    git.checked = false;
    git.dispatchEvent(new Event("change"));
    seen.hintOff = el("addproj-new-git-hint").textContent;
    el("addproj-create-go").click();
    await settle(() => seen.opened.length === 1);
    seen.madeWith = seen.made[0] ?? null;
    seen.openedMade = seen.opened[0] ?? null;
    seen.refusedSaid = seen.errors.find((one) => one.includes("Author identity unknown")) ?? null;
    // The taken folder's door opens it.
    setAddProject(true, "create");
    input(el("addproj-new-name"), "taken");
    await settle(() => !el("addproj-new-taken").hidden);
    el("addproj-new-taken").querySelector(".wt-note-btn").click();
    await settle(() => seen.opened.length === 2);
    seen.openedTaken = seen.opened[1] ?? null;

    // ⑨ 복제.
    setAddProject(true, "clone");
    await settle(() => el("addproj-parent").value !== "");
    const url = el("addproj-url");
    const folder = el("addproj-parent");
    seen.cloneAtOpen = folder.value;
    input(url, "https://example.com/acme/site.git");
    await settle(() => folder.value.endsWith("/site"));
    seen.followed = folder.value;
    input(folder, "/Users/dev/elsewhere/my-site");
    input(url, "https://example.com/acme/other.git");
    await wait(300);
    seen.keptEdited = folder.value;
    await settle(() => !el("addproj-clone-go").disabled);
    el("addproj-clone-go").click();
    await settle(() => seen.cloned.length === 1 && seen.opened.length === 3);
    seen.clonedWith = seen.cloned[0] ?? null;
    seen.openedClone = seen.opened[2] ?? null;
    // A folder already standing: said, and the button is dead.
    setAddProject(true, "clone");
    input(el("addproj-parent"), "/Users/dev/acme/taken");
    await settle(() => el("addproj-error").textContent !== "");
    seen.cloneTaken = { said: el("addproj-error").textContent, dead: el("addproj-clone-go").disabled };
    setAddProject(false);
  } finally {
    window.openProjectAt = opening;
    window.showError = erring;
  }
  seen.words = {
    badge: t("worktree.planNewFolders", "새 폴더 {{count}}", { count: 2 }),
    hintOff: t("project.gitHintOff", "폴더만 만듭니다. 작업 트리는 나중에 「git으로 시작하기」로 만들 수 있습니다"),
  };
  return seen;
};

// ⑩ — the folder browser.
const browser = async ({ home }) => {
  const { settle, input, key, wait } = window.__T__;
  const seen = { browsed: [], made: [] };
  const tree = {
    "/Users/dev/acme": { parent: "/Users/dev", entries: [{ name: "app", is_dir: true }, { name: "notes.md", is_dir: false }] },
    "/Users/dev": { parent: "/Users", entries: [{ name: "acme", is_dir: true }] },
  };
  tree[home] = { parent: "/Users", entries: [] };
  window.__ANSWER__.browse_dir = ({ path }) => {
    seen.browsed.push(path);
    const at = path === "~" ? home : path;
    const level = tree[at];
    if (!level) return Promise.reject(`${path}: 폴더가 아닙니다`);
    return { path: at, parent: level.parent, entries: level.entries, total: level.entries.length, truncated: false };
  };
  window.__ANSWER__.make_folder = ({ path }) => {
    seen.made.push(path);
    tree[path] = { parent: path.replace(/\/[^/]+$/, ""), entries: [] };
    return path;
  };
  window.__ANSWER__.path_kinds = ({ paths }) => paths.map((one) => (tree[one] ? "folder" : "missing"));
  const list = el("pb-list");
  // The button.
  let asked = openPathBrowser({ mode: "folder", system: false, start: "/Users/dev/acme" });
  await settle(() => el("pb-path").value === "/Users/dev/acme");
  // The layout height: the dialog opens through a scale, and a rect read
  // during it is the row times that scale.
  seen.rowHeight = list.querySelector(".pb-row")?.offsetHeight ?? null;
  seen.toolLive = !el("pb-new-folder").disabled && !el("pb-new-folder").hidden;
  el("pb-new-folder").click();
  seen.rowOpen = !el("pb-new").hidden;
  seen.nameFocused = document.activeElement?.id ?? null;
  input(el("pb-new-name"), "fresh");
  key(el("pb-new-name"), { key: "Enter" });
  await settle(() => el("pb-path").value === "/Users/dev/acme/fresh");
  seen.madeAt = seen.made[0] ?? null;
  seen.standsInMade = el("pb-path").value;
  seen.rowFolded = el("pb-new").hidden;
  key(list, { key: "Enter", metaKey: true, ctrlKey: true });
  seen.chosenMade = await asked;

  // Escape folds the name row first; ⇧⌘N raises it and only it.
  asked = openPathBrowser({ mode: "folder", system: false, start: "/Users/dev/acme" });
  await settle(() => el("pb-path").value === "/Users/dev/acme");
  list.focus();
  const composerBefore = wtNewScrim.hidden;
  key(list, { key: "N", code: "KeyN", shiftKey: true, metaKey: true, ctrlKey: true });
  seen.chordOpens = !el("pb-new").hidden;
  seen.chordLeftComposer = wtNewScrim.hidden === composerBefore;
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
  seen.escapeFolds = el("pb-new").hidden;
  seen.browserStays = !el("pb-scrim").hidden;
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
  seen.secondEscape = await asked;
  seen.closed = el("pb-scrim").hidden;

  // A typed path that is not there: the offer, and one press makes it and stands in it.
  asked = openPathBrowser({ mode: "folder", system: false, start: "/Users/dev/acme" });
  await settle(() => el("pb-path").value === "/Users/dev/acme");
  el("pb-path").value = "/Users/dev/acme/brand-new";
  key(el("pb-path"), { key: "Enter" });
  await settle(() => !el("pb-missing").hidden);
  seen.offer = el("pb-missing-words").textContent;
  el("pb-missing-make").click();
  await settle(() => el("pb-path").value === "/Users/dev/acme/brand-new");
  seen.madeMissing = seen.made[1] ?? null;
  seen.offerGone = el("pb-missing").hidden;
  // Home.
  el("pb-home").click();
  await settle(() => el("pb-path").value === home);
  seen.askedHome = seen.browsed.includes("~");
  // The attach mode makes nothing.
  closePathBrowser([]);
  await asked;
  asked = openPathBrowser({ mode: "attach", start: "/Users/dev/acme" });
  await settle(() => el("pb-path").value === "/Users/dev/acme");
  seen.attachHidesNewFolder = el("pb-new-folder").hidden;
  closePathBrowser([]);
  await asked;
  await wait(10);
  return seen;
};

export async function testCreateFlow({ browser: chromium, origin, ok }) {
  const at = { project: PROJECT, folder: FOLDER, home: HOME };

  const one = await scene(chromium, origin, ok, "composer", composer, at);
  ok(
    "① the composer says where the worktree will be made and from what, from the backend's plan",
    one.path.includes("~/acme/wt/fix-login") && one.bold === "fix-login" && one.badge === one.words.badge &&
      one.branch.includes("wt/fix-login") && one.branch.includes(one.words.fromMain) &&
      one.noteAtOpen && one.resetAtOpen && one.rememberAtOpen && one.fieldAtOpen === one.setting,
    JSON.stringify({ path: one.path, bold: one.bold, badge: one.badge, branch: one.branch }),
  );
  ok(
    "① a typed place is asked about as typed and kept under the typing; a repaint asks nothing; 기본값으로 goes back to the setting",
    one.askedTyped === "~/acme/elsewhere" && one.resetShown && one.rememberShown && one.typedKept === "~/acme/elsewhere" &&
      one.repaintAsked === 0 && one.askedSpec === "fix login" &&
      one.afterReset.value === one.setting && one.afterReset.reset && one.afterReset.remember && one.afterReset.asked === null,
    JSON.stringify({ typed: one.askedTyped, repaint: one.repaintAsked, spec: one.askedSpec, reset: one.afterReset }),
  );
  ok(
    "① every field of the composer stands at the control height, and 더 만들기's switch is drawn",
    Object.values(one.heights).every((height) => height === one.control) && one.control === 32 &&
      one.switchBox.width > one.switchBox.height && one.switchBox.height > 0,
    JSON.stringify({ heights: one.heights, control: one.control, switch: one.switchBox }),
  );
  ok(
    "② 「다음에도 이 위치 사용」 makes the place the setting before the create, and the create stands there",
    one.rememberedFirst?.args?.patch?.value === "~/acme/kept" && one.rememberedFirst?.created === 0 &&
      one.settingAfter === "~/acme/kept" && one.createdDirectory === "~/acme/kept" && one.createdFirstCommit === null,
    JSON.stringify({ remembered: one.rememberedFirst, setting: one.settingAfter, directory: one.createdDirectory }),
  );
  ok(
    "③ a repository with no commit offers the empty first commit ticked, refuses the press unticked, and the create carries it",
    one.unborn.words.length > 0 && one.unborn.ticked === true && one.unborn.branch.includes(one.words.fromFirst) &&
      one.unborn.aliveTicked && one.unborn.deadUnticked && one.unborn.aliveAgain && one.unborn.firstCommit === true,
    JSON.stringify(one.unborn),
  );

  ok(
    "① one preview is out at a time: changes behind a slow answer make one more ask, with the last of them and the base the first answer found",
    one.single.outMax === 1 && one.single.askedWhileOut === 1 && one.single.asked === 2 &&
      one.single.trailing === "abc" && one.single.firstBase === null && one.single.base === "main" && one.single.painted,
    JSON.stringify(one.single),
  );

  const two = await scene(chromium, origin, ok, "folder project", folderProject, at);
  ok(
    "④ a folder project keeps its ＋, is listed as 「git 아님」, and 「git으로 시작하기」 makes it git before the create comes alive",
    two.plusShown && two.picked === FOLDER && two.listed.includes(FOLDER) && two.listed.includes(PROJECT) &&
      two.tag === two.words.notGit && two.note.includes(two.words.startGit) && two.deadWhileFolder && two.startShown &&
      two.startedWith?.path === FOLDER && two.noteGone && two.aliveAfter && two.tagAfter !== two.words.notGit,
    JSON.stringify({ plus: two.plusShown, picked: two.picked, tag: two.tag, started: two.startedWith, alive: two.aliveAfter, after: two.tagAfter }),
  );

  const half = await scene(chromium, origin, ok, "folder start half fails", folderStartHalfFails, at);
  ok(
    "④b a start whose git init lands and whose first commit is refused says why, and the form reads the now-git folder again instead of offering a second start",
    half.started.length === 1 && half.error.includes("첫 커밋") && half.replanned && half.firstCommitOffered && !half.startStill &&
      half.tagAfter !== half.notGit,
    JSON.stringify({ started: half.started.length, error: half.error, replanned: half.replanned, firstCommit: half.firstCommitOffered, start: half.startStill, tag: half.tagAfter }),
  );

  const picker = await scene(chromium, origin, ok, "clone picker start", clonePickerStart, at);
  ok(
    "④c the clone step's 「…」 opens the picker on the folder the clone goes into, not on the clone's own folder that is not there yet",
    picker.start === "~/code",
    JSON.stringify(picker),
  );

  const three = await scene(chromium, origin, ok, "failed create", failedCreate, at);
  ok(
    "⑤ a create refused for want of a commit says the whole reason, and its retry makes the first commit",
    three.failed && three.reason.includes("커밋") && three.wraps === "normal" && three.retry === three.retryWords &&
      three.retried?.firstCommit === true && three.rowGone,
    JSON.stringify({ reason: three.reason, wraps: three.wraps, retry: three.retry, retried: three.retried?.firstCommit }),
  );

  const four = await scene(chromium, origin, ok, "projects window", hub, at);
  const siteRow = four.recent.find((one) => one.path === `${HOME}/code/site`);
  ok(
    "⑥ the projects window opens on its search with the recent projects, home written ~, the first row ready for ↵",
    four.focus === "addproj-q" && four.recent.some((one) => one.path === PROJECT) &&
      siteRow?.shown === "~/code/site" && (siteRow?.meta ?? "").includes("trunk") && four.firstSelected &&
      JSON.stringify(four.filtered) === JSON.stringify([`${HOME}/code/site`]),
    JSON.stringify({ focus: four.focus, recent: four.recent, filtered: four.filtered }),
  );
  ok(
    "⑥ a typed path that is a folder opens it by its real path; one that is not there starts a new project there",
    four.askedSite && four.askedNew &&
      four.pathRow[0]?.act === "open" && four.pathRow[0]?.path === `${HOME}/code/site` &&
      four.openedPath === `${HOME}/code/site` && four.closedOnOpen &&
      four.makeRow[0]?.act === "make" && four.makeRow[0]?.path === "~/code/new-app" &&
      four.made.step && four.made.parent === "~/code" && four.made.name === "new-app" && four.made.back && four.backToStart,
    JSON.stringify({ row: four.pathRow, make: four.makeRow, opened: four.openedPath, made: four.made }),
  );
  ok(
    "⑦ with no project yet the window is the first run: three cards, and a card is its way",
    four.welcome.shown && four.welcome.listHidden && four.welcome.cards === 3 && four.welcome.cardOpensClone,
    JSON.stringify(four.welcome),
  );

  const five = await scene(chromium, origin, ok, "forms", forms, at);
  ok(
    "⑧ 새 프로젝트 shows the folder at once and then the backend's plan, with the folders it makes",
    five.parent === "/tmp/zerocode-window-test-parent" && five.typedLine.includes("/tmp/zerocode-window-test-parent/site") &&
      five.planLine.includes("~/acme/site") && five.planBadge === five.words.badge && five.aliveNamed &&
      five.planned.some((one) => one.parent === "/tmp/zerocode-window-test-parent" && one.name === "site"),
    JSON.stringify({ parent: five.parent, typed: five.typedLine, plan: five.planLine, badge: five.planBadge }),
  );
  ok(
    "⑧ a refused name is said under the name, a taken folder offers to open it, and both refuse the press",
    five.bad.said.includes("-bad") && five.bad.wrong && five.bad.dead && five.bad.line === "" &&
      five.taken.said.includes("~/acme/taken") && five.taken.dead && five.taken.door && five.taken.nameClear &&
      five.openedTaken === "/Users/dev/acme/taken",
    JSON.stringify({ bad: five.bad, taken: five.taken, opened: five.openedTaken }),
  );
  ok(
    "⑧ the Git box rides the create, its hint follows it, and a first commit git refused is said once",
    five.gitAtOpen === true && five.hintOff === five.words.hintOff &&
      five.madeWith?.git === false && five.madeWith?.name === "site" && five.openedMade === "/Users/dev/acme/site" &&
      five.refusedSaid !== null,
    JSON.stringify({ made: five.madeWith, opened: five.openedMade, said: five.refusedSaid }),
  );
  ok(
    "⑨ 복제 fills the whole folder from the address until it is edited, and clones into what was edited",
    five.cloneAtOpen === "/tmp/zerocode-window-test-parent" && five.followed === "/tmp/zerocode-window-test-parent/site" &&
      five.keptEdited === "/Users/dev/elsewhere/my-site" &&
      five.clonedWith?.parent === "/Users/dev/elsewhere" && five.clonedWith?.name === "my-site" &&
      five.openedClone === "/Users/dev/elsewhere/my-site" && five.cloneTaken.said.length > 0 && five.cloneTaken.dead,
    JSON.stringify({ open: five.cloneAtOpen, followed: five.followed, kept: five.keptEdited, cloned: five.clonedWith, taken: five.cloneTaken }),
  );

  const six = await scene(chromium, origin, ok, "folder browser", browser, at);
  ok(
    "⑩ 새 폴더 makes a folder where the browser stands, stands in it, and 「이 폴더 선택」 answers it",
    six.toolLive && six.rowOpen && six.nameFocused === "pb-new-name" && six.madeAt === "/Users/dev/acme/fresh" &&
      six.standsInMade === "/Users/dev/acme/fresh" && six.rowFolded &&
      JSON.stringify(six.chosenMade) === JSON.stringify(["/Users/dev/acme/fresh"]),
    JSON.stringify({ made: six.madeAt, stands: six.standsInMade, chosen: six.chosenMade }),
  );
  ok(
    "⑩ ⇧⌘N raises the name row and nothing else; Escape folds it before it closes the browser",
    six.chordOpens && six.chordLeftComposer && six.escapeFolds && six.browserStays &&
      JSON.stringify(six.secondEscape) === "[]" && six.closed,
    JSON.stringify({ chord: six.chordOpens, composer: six.chordLeftComposer, folds: six.escapeFolds, stays: six.browserStays }),
  );
  ok(
    "⑩ a typed path that is not there is offered, made in one press and stood in; home is one press; attach makes nothing",
    six.offer.includes("/Users/dev/acme/brand-new") && six.madeMissing === "/Users/dev/acme/brand-new" && six.offerGone &&
      six.askedHome && six.attachHidesNewFolder,
    JSON.stringify({ offer: six.offer, made: six.madeMissing, home: six.askedHome, attach: six.attachHidesNewFolder }),
  );
  ok(
    "⑩ the browser's rows stand at the control height (32 px), twice the names in the same list",
    six.rowHeight === 32,
    JSON.stringify({ rowHeight: six.rowHeight }),
  );
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const argv = process.argv.slice(2);
  const at = argv.indexOf("--json");
  const out = at >= 0 && at + 1 < argv.length ? argv[at + 1] : null;
  const { files, origin } = await createWindowServer();
  const chromium = await launchWindowBrowser();
  const results = [];
  const ok = (name, pass, detail = "") => {
    results.push({ name, pass: !!pass, detail });
    console.log(`${pass ? "PASS" : "FAIL"}  ${name}${detail && !pass ? `  — ${detail}` : ""}`);
  };
  try {
    await testCreateFlow({ browser: chromium, origin, ok });
  } finally {
    await chromium.close();
    files.close();
  }
  const failed = results.filter((one) => !one.pass).length;
  console.log("");
  console.log(`${results.length - failed}/${results.length} passed`);
  if (out) writeFileSync(out, `${JSON.stringify({ results }, null, 2)}\n`);
  process.exitCode = failed ? 1 : 0;
}
