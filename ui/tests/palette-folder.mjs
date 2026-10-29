// 찾기 창 「어디로 갈까요」 — 폴더 프로젝트와 실패한 목록 (t-82495).
//
// 사람 10-29 11:4x(v1.1.103): 「work」 프로젝트에서 사이드바 위 「검색」을 누르면 오류 토스트가 떴다.
// 하네스에서 git이 아닌 폴더 프로젝트를 열고 누르면 찾기 창이 아예 열리지 않았다 — 「작업」 줄의 git 전용
// 막음(`nav-tasks`의 disabled)을 단추와 ⌘J가 함께 묻고 있었고, 그 막음은 찾기 창에 최근·이동이 생기기
// 전(t-67485)의 「폴더에선 찾을 것이 없다」에 서 있었다. 목록 명령이 실패하는 갈래(git 프로젝트에서
// `git worktree list`가 실패)에서는 토스트 글이 백엔드의 말 그대로였고 최근·이동까지 사라졌다.
//
// 여기서 보는 것:
//   1. 폴더 프로젝트에서 검색 단추와 ⌘J가 찾기 창을 「최근 + 이동 + 폴더 하나」로 연다. 오류 토스트는 0개.
//   2. 목록이 실패하면 최근·이동은 남고, 토스트는 사람의 문장 하나이며 백엔드의 말은 마우스를 올리면 보인다.
//      글자를 더 쳐도 토스트는 쌓이지 않는다.
//   3. 파일 찾기가 실패해도 토스트는 사람의 문장이다 — 「[object Object]」도 내부 이름도 아니다.
//   4. 프로젝트가 하나도 없는 첫 화면에서는 찾기 창이 열리지 않는다(그 폴더는 프로젝트가 아니다).
//
// 후속 t-82875(검수 r1의 P2·P3):
//   5. 카탈로그를 못 읽은 창(읽기 실패)에서는 백엔드가 서 있는 폴더의 행이 없다 — 누르면 거절 토스트였다.
//      찾기 창은 그 대신 「프로젝트 목록을 아직 못 읽었다」는 사람 말 한 줄을 보인다.
//   6. 카탈로그를 읽은 창에서도 워크트리 묶음은 카탈로그가 아는 경로만 세운다.
//   7. 객체로 거절된 실패의 hover는 그 객체의 말(message·code)이다 — 「[object Object]」가 아니다.
//   8. 토스트가 선 동안 다른 실패 문장은 새 토스트로 보인다(앞 것은 내려감). 같은 문장은 쌓이지 않는다.
// 합성 데이터만 쓴다(/Users/dev).
import { PRIMARY_EVENT } from "./test-platform.mjs";
import { openWindowTestPage } from "./window-boot.mjs";

const FOLDER = "/Users/dev/work";
const FOLDER_ENTRY = Object.freeze({
  path: FOLDER, branch: null, is_main: true, is_folder: true, prunable: false, active: true,
  ownership: "zerocode-managed", external_hidden: false, last_activity_ms: 0,
});
// What the backend's catalog says about a folder: one folder workspace, and an external-worktree answer
// that is not authoritative because git listed nothing.
const FOLDER_PROJECT = Object.freeze({
  name: "work", path: FOLDER, slug: null, worktrees: [FOLDER_ENTRY],
  external: { visibility: "show", legacy: false, authoritative: false, shown: 0, hidden: [], prompt: false, inbox: [] },
});
const RECENT = Object.freeze({ kind: "artifact", id: "a-2051", label: "결제 화면 시안 B", path: "/Users/dev/work/.zerocode/artifacts/b.html" });
// The folder the backend stands in when no project is active: no project of the catalog's.
const STRAY_ENTRY = Object.freeze({ ...FOLDER_ENTRY, path: "/Users/dev" });
// `OrchestratorError::Git`'s own wording (crates/zerocode-orchestrator/src/lib.rs), the one Err `list_worktrees` returns.
const GIT_LIST_FAILED = "`git worktree list` failed (exit status: 128): fatal: not a git repository (or any of the parent directories): .git";
// Words of the backend or the page that must never be the toast's sentence.
const RAW = ["git worktree", "exit status", "fatal:", "[object Object]", "Error:", "file search worker"];

const palette = (page) => page.evaluate(() => ({
  open: !document.getElementById("qo-scrim").hidden,
  groups: [...document.querySelectorAll("#qo-results .qo-group")].map((node) => node.textContent),
  rows: [...document.querySelectorAll("#qo-results .qo-row")].map((node) => ({
    label: node.querySelector(".qo-label")?.textContent ?? "",
    hint: node.querySelector(".qo-hint")?.textContent ?? null,
    icon: node.querySelector(".qo-row-icon use")?.getAttribute("href") ?? null,
  })),
  // The finder's one line about what it could not list, when it stands.
  note: (() => {
    const line = document.getElementById("qo-note");
    return line && !line.hidden ? line.textContent : null;
  })(),
}));
// The toasts on screen now, beside every toast made: one that was replaced is made but no longer stands.
const standing = (page) => page.evaluate(() => [...document.querySelectorAll(".toasts .toast")]
  .filter((node) => !node.hidden).map((node) => node.textContent.trim()));
// Every toast made since the scene was opened, read when it was made: a toast leaves on its own after a few
// seconds, so the ones standing at the moment of asking would undercount a pile on a slow machine.
const toasts = (page) => page.evaluate(() => window.__TOASTS_MADE__);
const raw = (text) => RAW.filter((word) => text.includes(word));
// The finder answers after its query resolves; a fresh `paletteRows` is the page's own word that it has.
const answered = (page) => page.waitForFunction(() => paletteMode !== null && paletteRows !== window.__ROWS_BEFORE__ && document.querySelectorAll("#qo-results .qo-row").length === paletteRows.length);
const markRows = (page) => page.evaluate(() => { window.__ROWS_BEFORE__ = paletteRows; });

async function openOn(browser, origin, scene, ready = () => projectsRead === true) {
  const faults = [];
  const { page } = await openWindowTestPage(browser, origin, {
    faults,
    before: (surface) => surface.addInitScript(scene, { folder: FOLDER_PROJECT, entry: FOLDER_ENTRY, failed: GIT_LIST_FAILED, stray: STRAY_ENTRY }),
  });
  // The first catalog read is what decides which project the window stands on.
  await page.waitForFunction(ready);
  await page.evaluate(() => {
    window.__TOASTS_MADE__ = [];
    new MutationObserver((changes) => {
      for (const change of changes) {
        for (const node of change.addedNodes) {
          if (!node.classList?.contains("toast")) continue;
          window.__TOASTS_MADE__.push({ text: node.textContent.trim(), tip: node.dataset.tip ?? "", kind: node.dataset.kind ?? "" });
        }
      }
    }).observe(document.body, { childList: true, subtree: true });
  });
  return { page, faults };
}

export async function testPaletteFolder(browser, origin, ok) {
  // 1. A folder project: the search button and ⌘J open the finder on its three groups.
  {
    const { page, faults } = await openOn(browser, origin, ({ folder, entry }) => {
      window.__ANSWER__.project_catalog = () => [folder];
      window.__ANSWER__.list_worktrees = () => [entry];
    });
    await page.evaluate((place) => rememberRecentPlace(place), RECENT);
    const standing = await page.evaluate(() => ({ active: activeProjectPath, tasksRefused: el("nav-tasks").disabled }));
    ok("the folder scene stands on the folder project, where 작업 stays refused",
      standing.active === FOLDER && standing.tasksRefused === true, JSON.stringify(standing));

    await markRows(page);
    await page.click("#sidebar-search");
    await answered(page).catch(() => null);
    const clicked = await palette(page);
    ok("the sidebar's search opens the finder in a folder project", clicked.open === true, JSON.stringify(clicked));
    ok("the folder project's finder lists recent places, the doors and the one folder",
      clicked.groups.join("|") === "최근|이동|워크트리"
        && clicked.rows[0]?.label === RECENT.label
        && clicked.rows.filter((row) => row.label === "work").length === 1
        && clicked.rows.at(-1)?.label === "work", JSON.stringify(clicked));
    ok("the folder's row says its name once, with no second copy as a hint, under a folder rather than a branch",
      clicked.rows.at(-1)?.hint === null && clicked.rows.at(-1)?.icon === "#i-folder", JSON.stringify(clicked.rows.at(-1)));
    ok("a finder whose catalog was read says no line about an unread list", clicked.note === null, JSON.stringify(clicked.note));
    const quiet = await toasts(page);
    ok("opening the finder in a folder project raises no toast", quiet.length === 0, JSON.stringify(quiet));

    await page.evaluate(() => closePalette());
    await markRows(page);
    await page.evaluate((primary) => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "j", ...primary, bubbles: true, cancelable: true }));
    }, PRIMARY_EVENT);
    await answered(page).catch(() => null);
    const chord = await palette(page);
    ok("⌘J opens the same finder in a folder project",
      chord.open === true && chord.groups.join("|") === "최근|이동|워크트리", JSON.stringify(chord));
    ok("the folder scene raised no page error", faults.length === 0, faults.join(" | "));
    await page.close();
  }

  // 2. A git project whose list will not read: the doors and the recent places stay, and the toast is a sentence.
  {
    const { page, faults } = await openOn(browser, origin, ({ failed }) => {
      window.__ANSWER__.list_worktrees = () => Promise.reject(failed);
    });
    await page.evaluate((place) => rememberRecentPlace(place), RECENT);
    await markRows(page);
    await page.click("#sidebar-search");
    await answered(page).catch(() => null);
    const shown = await palette(page);
    ok("a workspace list that will not read still leaves the recent places and the doors",
      shown.open === true && shown.groups.join("|") === "최근|이동" && shown.rows[0]?.label === RECENT.label,
      JSON.stringify(shown));
    const said = await toasts(page);
    ok("the failed list is said once, in a sentence of the person's, with git's words on the hover",
      said.length === 1 && raw(said[0].text).length === 0 && said[0].text.length > 0
        && said[0].tip.includes("exit status: 128"), JSON.stringify(said));

    for (const word of ["결", "결제"]) {
      await markRows(page);
      await page.evaluate((word) => {
        const input = document.getElementById("qo-input");
        input.value = word;
        input.dispatchEvent(new Event("input", { bubbles: true }));
      }, word);
      await answered(page).catch(() => null);
    }
    const typed = await toasts(page);
    const narrowed = await palette(page);
    ok("typing while the list will not read narrows the finder and piles no second toast",
      typed.length === 1 && narrowed.rows.map((row) => row.label).join("|") === RECENT.label, JSON.stringify({ typed, narrowed }));
    ok("the failing-list scene raised no page error", faults.length === 0, faults.join(" | "));
    await page.close();
  }

  // 3. The file finder's failure: neither 「[object Object]」 nor the backend's wording is the toast.
  {
    const { page, faults } = await openOn(browser, origin, () => {
      window.__ANSWER__.search_files = () => Promise.reject({ code: "search.failed" });
    });
    await page.evaluate(() => {
      togglePalette("file");
      const input = document.getElementById("qo-input");
      input.value = "pay";
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await page.waitForFunction(() => window.__TOASTS_MADE__.length > 0).catch(() => null);
    const said = await toasts(page);
    ok("a failed file search is said in a sentence of the person's, not as 「[object Object]」",
      said.length === 1 && raw(said[0].text).length === 0 && said[0].text.length > 0, JSON.stringify(said));
    ok("an object refusal's hover is its own word, not 「[object Object]」",
      said[0]?.tip === "search.failed", JSON.stringify(said));
    ok("the failing-search scene raised no page error", faults.length === 0, faults.join(" | "));
    await page.close();
  }

  // 4. No project at all: the backend's folder is no project, so the finder stays shut.
  {
    const { page, faults } = await openOn(browser, origin, ({ entry }) => {
      window.__ANSWER__.project_catalog = () => [];
      window.__ANSWER__.list_worktrees = () => [entry];
    });
    await page.evaluate((primary) => {
      el("sidebar-search").click();
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "j", ...primary, bubbles: true, cancelable: true }));
    }, PRIMARY_EVENT);
    await page.evaluate(() => window.__PAINTED__());
    const landing = await page.evaluate(() => ({ open: !document.getElementById("qo-scrim").hidden, asked: window.__COUNTS__.list_worktrees ?? 0 }));
    ok("with no project the search button and ⌘J open no finder and ask for no list",
      landing.open === false && landing.asked === 0, JSON.stringify(landing));
    ok("the landing scene raised no page error", faults.length === 0, faults.join(" | "));
    await page.close();
  }
  // 5. A catalog that will not read: the backend's own folder is no row, and the finder says why the group is missing.
  {
    const { page, faults } = await openOn(browser, origin, ({ stray }) => {
      window.__ANSWER__.project_catalog = () => {
        window.__CATALOG_REFUSED__ = true;
        return Promise.reject("projects file unreadable");
      };
      window.__ANSWER__.list_worktrees = () => [stray];
    }, () => window.__CATALOG_REFUSED__ === true && projectsRead === false);
    await page.evaluate(() => window.__PAINTED__());
    await page.evaluate((place) => rememberRecentPlace(place), RECENT);
    await markRows(page);
    await page.click("#sidebar-search");
    await answered(page).catch(() => null);
    const shown = await palette(page);
    ok("a window whose catalog will not read opens the finder on the recent places and the doors, with no folder row",
      shown.open === true && shown.groups.join("|") === "최근|이동" && shown.rows[0]?.label === RECENT.label
        && !shown.rows.some((row) => row.icon === "#i-folder" || row.icon === "#i-branch"), JSON.stringify(shown));
    ok("the finder says in one sentence of the person's that the project list is not read yet",
      typeof shown.note === "string" && shown.note.length > 0 && raw(shown.note).length === 0, JSON.stringify(shown.note));

    // The line follows the language: the same scene in English says it in English.
    await page.evaluate(() => {
      closePalette();
      locale = "en";
      applyLocale();
    });
    await markRows(page);
    await page.click("#sidebar-search");
    await answered(page).catch(() => null);
    const english = await palette(page);
    ok("the unread-list line is said in the window's language",
      typeof english.note === "string" && english.note !== shown.note && !/[가-힣]/u.test(english.note), JSON.stringify(english.note));
    ok("the unread-catalog scene raised no page error", faults.length === 0, faults.join(" | "));
    await page.close();
  }

  // 6. A read catalog: the workspace group stands only on the paths the catalog knows.
  {
    const { page, faults } = await openOn(browser, origin, ({ folder, entry, stray }) => {
      window.__ANSWER__.project_catalog = () => [folder];
      window.__ANSWER__.list_worktrees = () => [entry, stray];
    });
    await markRows(page);
    await page.click("#sidebar-search");
    await answered(page).catch(() => null);
    const shown = await palette(page);
    const trees = shown.rows.filter((row) => row.icon === "#i-folder" || row.icon === "#i-branch").map((row) => row.label);
    ok("the workspace group lists the catalog's folder and not a path the catalog does not know",
      trees.join("|") === "work" && shown.note === null, JSON.stringify(shown));
    ok("the stray-path scene raised no page error", faults.length === 0, faults.join(" | "));
    await page.close();
  }

  // 7·8. Two finders failing in turn: a different sentence is a new toast, the same one is not a second.
  {
    const { page, faults } = await openOn(browser, origin, ({ failed }) => {
      window.__ANSWER__.list_worktrees = () => Promise.reject(failed);
      window.__ANSWER__.search_files = () => Promise.reject({ code: "search.failed", message: "the file index is being rebuilt" });
    });
    await markRows(page);
    await page.click("#sidebar-search");
    await answered(page).catch(() => null);
    const first = await toasts(page);
    await page.evaluate(() => {
      togglePalette("file");
      const input = document.getElementById("qo-input");
      input.value = "pay";
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await page.waitForFunction(() => window.__TOASTS_MADE__.length > 1).catch(() => null);
    const both = await toasts(page);
    const up = await standing(page);
    ok("a different failure sentence while a toast stands is a new toast, which takes the earlier one's place",
      first.length === 1 && both.length === 2 && both[1].text !== both[0].text
        && up.length === 1 && up[0] === both[1].text, JSON.stringify({ both, up }));
    ok("an object refusal with a message carries that message on the hover",
      both[1]?.tip === "the file index is being rebuilt", JSON.stringify(both[1]));

    await page.evaluate(() => {
      const input = document.getElementById("qo-input");
      input.value = "paym";
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await page.waitForFunction(() => window.__COUNTS__.search_files >= 2).catch(() => null);
    await page.evaluate(() => window.__PAINTED__());
    const again = await toasts(page);
    ok("the same failure sentence again piles no further toast", again.length === 2, JSON.stringify(again));
    ok("the two-failures scene raised no page error", faults.length === 0, faults.join(" | "));
    await page.close();
  }
}
