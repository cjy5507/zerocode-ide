/* 창 안 경로 브라우저 (t-2982) — 폴더 모드와 첨부 모드, 브라우저는 하나.
 *
 * 폴더 찾아보기가 AppKit 없이 열리는가를 실제 레이아웃 엔진에서 잰다: 표의
 * 상한만큼의 행을 100 ms 안에 그리는가, 키보드가 들어가고 올라오는가, 고르고
 * 취소하고 경로를 직접 치는가, 최근 프로젝트 행이 시작 화면에 서는가, 첨부
 * 모드가 파일과 폴더를 여럿 담아 절대 경로 목록으로 답하는가, 시스템 대화상자
 * 문이 헬퍼 길(`choose_paths`)로 가고 띠가 서는가. `entryCap`은 Rust 표에서
 * 읽어 온다 — 숫자를 여기 한 벌 더 두지 않는다. */
export async function testPathBrowser(page, ok, entryCap) {
  const seen = await page.evaluate(async (cap) => {
    const seen = {};
    const tick = () => new Promise((done) => setTimeout(done, 0));
    const settle = async (until, budgetMs = 1500) => {
      const start = performance.now();
      while (!until() && performance.now() - start < budgetMs) await tick();
      return until();
    };
    const key = (target, init) =>
      target.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }));
    const original = { ...window.__ANSWER__ };
    const calls = [];
    const tree = {
      "/root": { parent: "/", entries: [
        { name: "dirA", is_dir: true }, { name: "dirB", is_dir: true },
        { name: ".git", is_dir: true }, { name: "file1.txt", is_dir: false },
      ] },
      "/root/dirA": { parent: "/root", entries: [{ name: "sub", is_dir: true }] },
      "/root/dirB": { parent: "/root", entries: [] },
      "/": { parent: null, entries: [{ name: "root", is_dir: true }] },
      "/typed/dir": { parent: "/typed", entries: [{ name: "here.md", is_dir: false }] },
    };
    window.__ANSWER__.browse_dir = ({ path }) => {
      calls.push({ command: "browse_dir", path });
      if (path === "/big") {
        const entries = [];
        for (let i = 0; i < cap; i += 1) {
          entries.push({ name: `entry-${String(i).padStart(5, "0")}`, is_dir: i % 3 === 0 });
        }
        return { path, parent: "/", entries, total: cap + 7, truncated: true };
      }
      const level = tree[path];
      if (!level) return Promise.reject(`${path}: 폴더가 아닙니다`);
      return { path, parent: level.parent, entries: level.entries, total: level.entries.length, truncated: false };
    };
    const list = el("pb-list");
    const rows = el("pb-rows");
    const scrim = el("pb-scrim");
    const rowsPainted = () => rows.children.length;
    const rowsHeld = () => window.__PATH_BROWSER_ROWS__();

    // ① Opens under budget at the table's cap, and says it was cut.
    const opens = [];
    for (let round = 0; round < 3; round += 1) {
      const started = performance.now();
      const asked = openPathBrowser({ mode: "folder", system: false, start: "/big" });
      await settle(() => rowsHeld() === cap && rowsPainted() > 0);
      opens.push(performance.now() - started);
      seen.capRows = rowsHeld();
      seen.capNodes = rowsPainted();
      seen.capCount = el("pb-count").textContent;
      seen.capOpen = !scrim.hidden;
      seen.capBoxHeight = rows.getBoundingClientRect().height;
      seen.capRowHeight = rows.firstElementChild.getBoundingClientRect().height;
      key(list, { key: "End" });
      seen.capLast = list.querySelector(".is-sel .qo-label")?.textContent;
      seen.capLastNodes = rowsPainted();
      closePathBrowser([]);
      seen.capAnswer = await asked;
    }
    seen.openMs = Math.min(...opens);
    seen.opens = opens.map((one) => Math.round(one));

    // ② Keyboard: down/up highlight, Enter enters a folder, Backspace goes up,
    //    and past the root the start view stands. Dotfiles are not rows.
    let asked = openPathBrowser({ mode: "folder", system: false, start: "/root" });
    await settle(() => rowsPainted() === 3);
    seen.rootRows = [...rows.children].map((row) => row.querySelector(".qo-label").textContent);
    seen.focus = document.activeElement?.id;
    seen.nothingHighlighted = !list.querySelector(".is-sel");
    seen.chooseHereEnabledWithNothingHighlighted = !el("pb-confirm").disabled;
    key(list, { key: "ArrowDown" });
    key(list, { key: "ArrowDown" });
    seen.highlightedAfterTwoDowns = list.querySelector(".is-sel .qo-label")?.textContent;
    seen.activeDescendant = list.getAttribute("aria-activedescendant");
    key(list, { key: "ArrowUp" });
    seen.highlightedAfterUp = list.querySelector(".is-sel .qo-label")?.textContent;
    key(list, { key: "Enter" });
    await settle(() => el("pb-path").value === "/root/dirA");
    seen.enteredDirA = el("pb-path").value;
    seen.dirARows = [...rows.children].map((row) => row.querySelector(".qo-label").textContent);
    key(list, { key: "Backspace" });
    await settle(() => el("pb-path").value === "/root");
    seen.backToRoot = el("pb-path").value;
    key(list, { key: "ArrowLeft" });
    await settle(() => el("pb-path").value === "/");
    key(list, { key: "Backspace" });
    await settle(() => el("pb-path").value === "" && rowsPainted() === 3);
    seen.placesAfterRoot = [...rows.children].map((row) => ({
      label: row.querySelector(".qo-label").textContent,
      hint: row.querySelector(".qo-hint").textContent,
    }));
    seen.chooseHereDisabledOnPlaces = el("pb-confirm").disabled;
    // ③ Cancel: Escape resolves an empty list and the modal is gone.
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    seen.cancelled = await asked;
    seen.closedAfterEscape = scrim.hidden;

    // ④ Select: the highlighted folder row wins; with none, the folder shown.
    asked = openPathBrowser({ mode: "folder", system: false, start: "/root" });
    await settle(() => rowsPainted() === 3);
    key(list, { key: "ArrowDown" });
    key(list, { key: "ArrowDown" });
    el("pb-confirm").click();
    seen.chosenRow = await asked;
    asked = openPathBrowser({ mode: "folder", system: false, start: "/root/dirB" });
    await settle(() => el("pb-path").value === "/root/dirB");
    seen.emptyShown = !el("pb-empty").hidden;
    key(list, { key: "Enter", metaKey: true });
    seen.chosenHere = await asked;

    // ⑤ Typed path: Enter in the field asks for exactly what was typed.
    asked = openPathBrowser({ mode: "folder", system: false, start: "/root" });
    await settle(() => rowsPainted() === 3);
    const before = calls.length;
    el("pb-path").value = "/typed/dir";
    key(el("pb-path"), { key: "Enter" });
    await settle(() => el("pb-path").value === "/typed/dir" && rowsPainted() === 1);
    seen.typedRows = [...rows.children].map((row) => row.querySelector(".qo-label").textContent);
    el("pb-path").value = "/nowhere";
    key(el("pb-path"), { key: "Enter" });
    await settle(() => el("pb-error").textContent.length > 0);
    seen.typedError = el("pb-error").textContent;
    seen.stillTypedDir = el("pb-path").value;
    seen.typedAsked = calls.slice(before).map((one) => one.path);
    closePathBrowser([]);
    await asked;

    // ⑥ Recent projects: the start view's first row, chosen by keyboard.
    asked = openPathBrowser({ mode: "folder", system: false });
    await settle(() => rowsPainted() === 3);
    seen.startRows = [...rows.children].map((row) => ({
      label: row.querySelector(".qo-label").textContent,
      hint: row.querySelector(".qo-hint").textContent,
      glyph: row.querySelector(".qo-row-icon use").getAttribute("href"),
    }));
    seen.pathFieldEmptyOnStart = el("pb-path").value === "";
    const second = openPathBrowser({ mode: "folder", system: false });
    seen.oneBrowser = second === asked;
    key(list, { key: "ArrowDown" });
    key(list, { key: "Enter", metaKey: true });
    seen.recentChosen = await asked;

    // ⑦ Attach mode: files and folders, several, answered as absolute paths in
    //    the order they were gathered; the check is the row's own door.
    asked = openPathBrowser({ mode: "attach", start: "/root" });
    await settle(() => rowsPainted() === 3);
    seen.attachTitle = el("pb-title").textContent;
    seen.attachConfirmDead = el("pb-confirm").disabled;
    seen.attachChecksShown = [...list.querySelectorAll(".pb-check")].every((check) => !check.hidden);
    key(list, { key: "End" });
    key(list, { key: " " });
    seen.pickedFile = list.querySelector(".is-picked .qo-label")?.textContent;
    rows.children[0].querySelector(".pb-check").click();
    seen.pickedCount = list.querySelectorAll(".is-picked").length;
    seen.attachConfirmLabel = el("pb-confirm").textContent;
    key(list, { key: "Home" });
    key(list, { key: "Enter" });
    await settle(() => el("pb-path").value === "/root/dirA");
    seen.attachEnterOpensFolder = el("pb-path").value;
    key(list, { key: "Backspace" });
    await settle(() => el("pb-path").value === "/root");
    seen.picksSurviveNavigation = list.querySelectorAll(".is-picked").length;
    el("pb-confirm").click();
    seen.attached = await asked;

    // ⑧ The system door: the helper road, with the browser standing behind
    //    it and its strip up; a dismissal leaves the browser where it was.
    let answerSystem = null;
    window.__ANSWER__.choose_paths = (args) => {
      calls.push({ command: "choose_paths", ...args });
      return new Promise((done) => { answerSystem = done; });
    };
    asked = openPathBrowser({ mode: "folder", system: false, start: "/root" });
    await settle(() => rowsPainted() === 3);
    seen.checksHiddenInFolderMode = [...list.querySelectorAll(".pb-check")].every((check) => check.hidden);
    el("pb-system").click();
    await settle(() => !el("pb-standing").hidden);
    seen.standingShown = !el("pb-standing").hidden;
    seen.systemAsked = calls.filter((one) => one.command === "choose_paths").at(-1);
    seen.systemBusy = el("pb-system").getAttribute("aria-busy");
    answerSystem([]);
    await settle(() => el("pb-standing").hidden);
    seen.browserStaysAfterDismiss = !scrim.hidden;
    el("pb-system").click();
    await settle(() => !el("pb-standing").hidden);
    answerSystem(["/sys/picked"]);
    seen.systemChosen = await asked;
    seen.closedAfterSystem = scrim.hidden;

    // The words, through the same catalog the page speaks — the pins must
    // not care which language an earlier section left the window in.
    seen.words = {
      home: t("project.browse.home", "홈"),
      attachTitle: t("project.browse.attachTitle", "파일 및 폴더 첨부"),
      attachTwo: t("project.browse.attach", "첨부 {{count}}개", { count: 2 }),
    };
    window.__ANSWER__ = original;
    return seen;
  }, entryCap);

  ok(
    `the folder browser opens with ${entryCap} rows under 100 ms (min of three) as a window of nodes over a box the height of them all, says the folder was cut, and End reaches the last row`,
    seen.openMs < 100 && seen.capRows === entryCap && seen.capNodes < 120 && seen.capOpen
      && seen.capBoxHeight > seen.capNodes * seen.capRowHeight
      && /7/.test(seen.capCount)
      && seen.capLast === `entry-${String(entryCap - 1).padStart(5, "0")}` && seen.capLastNodes < 120
      && Array.isArray(seen.capAnswer) && seen.capAnswer.length === 0,
    JSON.stringify({ opens: seen.opens, rows: seen.capRows, nodes: seen.capNodes, box: seen.capBoxHeight, last: seen.capLast, count: seen.capCount }),
  );
  ok(
    "the folder browser is driven by the keyboard: down/up, Enter enters, Backspace goes up, and past the root the start view stands; dotfiles are not rows",
    seen.focus === "pb-list"
      && JSON.stringify(seen.rootRows) === JSON.stringify(["dirA", "dirB", "file1.txt"])
      && seen.nothingHighlighted
      && seen.chooseHereEnabledWithNothingHighlighted
      && seen.highlightedAfterTwoDowns === "dirB"
      && seen.activeDescendant === "pb-row-1"
      && seen.highlightedAfterUp === "dirA"
      && seen.enteredDirA === "/root/dirA"
      && JSON.stringify(seen.dirARows) === JSON.stringify(["sub"])
      && seen.backToRoot === "/root"
      && seen.placesAfterRoot.length === 3
      && seen.chooseHereDisabledOnPlaces,
    JSON.stringify({
      focus: seen.focus, root: seen.rootRows, none: seen.nothingHighlighted, twoDowns: seen.highlightedAfterTwoDowns,
      descendant: seen.activeDescendant, up: seen.highlightedAfterUp, dirA: seen.enteredDirA, dirARows: seen.dirARows,
      back: seen.backToRoot, places: seen.placesAfterRoot, disabled: seen.chooseHereDisabledOnPlaces,
    }),
  );
  ok(
    "Escape cancels the browser with an empty answer",
    JSON.stringify(seen.cancelled) === "[]" && seen.closedAfterEscape,
    JSON.stringify({ cancelled: seen.cancelled, closed: seen.closedAfterEscape }),
  );
  ok(
    "「이 폴더 선택」 answers the highlighted folder, else the folder shown (⌘↵ too)",
    JSON.stringify(seen.chosenRow) === JSON.stringify(["/root/dirB"])
      && JSON.stringify(seen.chosenHere) === JSON.stringify(["/root/dirB"])
      && seen.emptyShown,
    JSON.stringify({ row: seen.chosenRow, here: seen.chosenHere, empty: seen.emptyShown }),
  );
  ok(
    "a typed path is asked for as typed, and a path that is not a folder is refused in place",
    JSON.stringify(seen.typedAsked) === JSON.stringify(["/typed/dir", "/nowhere"])
      && JSON.stringify(seen.typedRows) === JSON.stringify(["here.md"])
      && seen.typedError.includes("/nowhere")
      && seen.stillTypedDir === "/nowhere",
    JSON.stringify({ asked: seen.typedAsked, rows: seen.typedRows, error: seen.typedError }),
  );
  ok(
    "the start view lists recent projects first, then home, then volumes, and one browser stands at a time",
    seen.startRows.length === 3
      && seen.startRows[0].label === "zerocode" && seen.startRows[0].glyph === "#i-clock"
      && seen.startRows[1].label === seen.words.home && seen.startRows[1].hint === "/Users/tester"
      && seen.startRows[2].glyph === "#i-hard-drive"
      && seen.pathFieldEmptyOnStart
      && seen.oneBrowser
      && JSON.stringify(seen.recentChosen) === JSON.stringify(["/tmp/zerocode-window-test"]),
    JSON.stringify({ rows: seen.startRows, one: seen.oneBrowser, chosen: seen.recentChosen }),
  );
  ok(
    "attach mode gathers files and folders by check and by Space, keeps them across navigation, and answers absolute paths in gathering order",
    seen.attachTitle === seen.words.attachTitle
      && seen.attachConfirmDead
      && seen.attachChecksShown
      && seen.pickedFile === "file1.txt"
      && seen.pickedCount === 2
      && seen.attachConfirmLabel === seen.words.attachTwo
      && seen.attachEnterOpensFolder === "/root/dirA"
      && seen.picksSurviveNavigation === 2
      && JSON.stringify(seen.attached) === JSON.stringify(["/root/file1.txt", "/root/dirA"]),
    JSON.stringify({
      title: seen.attachTitle, dead: seen.attachConfirmDead, file: seen.pickedFile,
      count: seen.pickedCount, label: seen.attachConfirmLabel, attached: seen.attached,
    }),
  );
  ok(
    "the system door rides the helper road (choose_paths) with the browser and its strip standing behind it; a dismissal keeps the browser, an answer closes it",
    seen.checksHiddenInFolderMode
      && seen.standingShown
      && seen.systemAsked?.kind === "folder" && seen.systemAsked?.start === "/root"
      && seen.systemBusy === "true"
      && seen.browserStaysAfterDismiss
      && JSON.stringify(seen.systemChosen) === JSON.stringify(["/sys/picked"])
      && seen.closedAfterSystem,
    JSON.stringify({
      strip: seen.standingShown, asked: seen.systemAsked, stays: seen.browserStaysAfterDismiss,
      chosen: seen.systemChosen, closed: seen.closedAfterSystem,
    }),
  );
}
