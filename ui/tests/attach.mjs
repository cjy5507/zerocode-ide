/* 입력줄 옆 「+」 — 파일·폴더·이미지 첨부 (t-2993, docs/design/composer-attachments.md §3 핀)
 *
 * 헬퍼 페이지의 입력줄(t-2973)에 첨부 모듈(ui/shell-attach.js)이 붙었는가를
 * 실제 레이아웃 엔진에서 잰다: 「+」가 도구 줄 맨 왼쪽에 이름을 달고 서고,
 * 메뉴 세 행이 조건대로 열리고 닫히는가(키보드까지), 브라우저의 답·OS 드롭·
 * ⌘V 그림이 같은 칩이 되는가, 보내기가 `term_paste`에 「글 + 빈 줄 + 첨부: +
 * 절대 경로들」을 그대로 싣고 Enter를 치는가, 없는 경로는 표시만 남고 가지
 * 않는가. `limits`는 모듈의 표에서 읽어 온다 — 숫자를 여기 한 벌 더 두지 않는다.
 *
 * 브라우저 이음매(`openPathBrowser`)는 앞의 핀에서 스텁이고, 마지막 핀이
 * 진짜 브라우저(t-2982, attach 모드)를 `browse_dir` 스텁 위에서 굴린다. */
export async function testComposerAttach(page, ok, limits) {
  // 핀 하나가 던져도 나머지는 잰다 — 모듈이 없는 날(빨강)에도 표는 열 줄이다.
  const probe = async (run, arg) => {
    try {
      return await page.evaluate(run, arg);
    } catch (error) {
      return { threw: String(error).split("\n")[0] };
    }
  };
  const stood = await page.evaluate(async () => {
    const seen = {};
    window.__ATTACH_H__ = {
      settle: (ms = 60) => new Promise((done) => setTimeout(done, ms)),
      composer: () => document.querySelector("#worker-view .worker-composer"),
      box: () => document.querySelector("#worker-view .worker-composer-box"),
      plus: () => document.querySelector("#worker-view .composer-attach-plus"),
      menu: () => document.querySelector(".composer-attach-menu"),
      rows: () =>
        [...(document.querySelector(".composer-attach-menu")?.querySelectorAll('[role="menuitem"]') ?? [])],
      chips: () => [...document.querySelectorAll("#worker-view .composer-attach-chip")],
      names: () =>
        [...document.querySelectorAll("#worker-view .composer-attach-chip")]
          .map((chip) => chip.querySelector(".composer-attach-name")?.textContent ?? null),
      glyphs: () =>
        [...document.querySelectorAll("#worker-view .composer-attach-chip")]
          .map((chip) => chip.querySelector("use")?.getAttribute("href") ?? null),
      key: (target, init) =>
        target.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init })),
      toasts: () => document.querySelectorAll(".toast").length,
      submit: () =>
        document.querySelector("#worker-view .worker-composer")
          ?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })),
      // 손잡이로 곧장: 메뉴·드롭·붙여넣기가 모두 지나는 그 문.
      addPaths: async (paths, wait = 150) => {
        const H = window.__ATTACH_H__;
        await composerAttachmentsOf(H.composer()).add(paths);
        await H.settle(wait);
      },
      // 메뉴로 한 번: 「+」 → 행 → 답이 칩이 될 때까지.
      viaMenu: async (index, wait = 150) => {
        const H = window.__ATTACH_H__;
        H.plus().click();
        await H.settle();
        H.rows()[index]?.click();
        await H.settle(wait);
      },
      // 보이는 칩을 ×로 전부 걷는다 — 숨은 것은 앞이 비면 올라온다.
      clearChips: async () => {
        const H = window.__ATTACH_H__;
        for (let tries = 0; tries < 64 && H.chips().length > 0; tries += 1) {
          H.chips()[0].querySelector("button").click();
          await H.settle(10);
        }
      },
    };
    const H = window.__ATTACH_H__;
    const term = await openTermTab({ placement: "tab" });
    const owner = tabOfTerm(term);
    const tell = (name, payload) => {
      for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
    };
    // Between turns: a send while the pane works waits in the window's queue
    // (`composer-queue`), and these pins are about what a send carries.
    tell("hook:agent", { term, state: "idle", agent: "claude", session: "s-attach" });
    tell("hook:subagent", { term, rows: [{ id: "chip", name: "@chip" }] });
    await window.__PAINTED__();
    window.__ANSWER__.subagent_log = () => ({
      found: true,
      next: 2,
      turns: [{ role: "user", text: "briefing" }, { role: "assistant", text: "ready" }],
    });
    await openHelperPage(
      { term, agent: "claude", worktree: owner.worktree, tab: owner },
      { id: "chip", name: "@chip" },
    );
    await H.settle(120);
    // 열린 파일의 절대 경로는 창이 보는 체크아웃 아래다 — 하네스에는 그 자리가
    // 비어 있을 수 있으니 하나 세우고, 끝나면 되돌린다.
    window.__ATTACH__ = {
      term,
      helperId: `helper:${term}:chip`,
      ownerId: owner.id,
      worktree: typeof owner.worktree === "string" ? owner.worktree : null,
      heldWorktreePath: activeWorktreePath,
      heldBrowser: window.openPathBrowser,
    };
    if (!activeWorktreePath) activeWorktreePath = "/tmp/zerocode-window-test";
    window.__ATTACH__.root = activeWorktreePath;
    seen.composerStands = Boolean(H.composer());
    seen.worktree = window.__ATTACH__.worktree;
    return seen;
  });

  /* ① 「+」와 메뉴 세 행, 조건대로. */
  const plusMenu = await probe(async () => {
    const H = window.__ATTACH_H__;
    const seen = {};
    const tools = H.composer()?.querySelector(".worker-composer-tools");
    const plus = H.plus();
    seen.plusFirst = Boolean(plus) && tools?.firstElementChild === plus;
    seen.wantName = t("composer.attach.add", "첨부");
    seen.named = plus?.getAttribute("aria-label") === seen.wantName && plus?.dataset.tip === seen.wantName;
    seen.noTitle = Boolean(plus) && !plus.hasAttribute("title");
    seen.popup = plus?.getAttribute("aria-haspopup") === "menu" && plus?.getAttribute("aria-expanded") === "false";
    window.__ANSWER__.clipboard_has_image = () => false;
    plus?.click();
    await H.settle();
    const menu = H.menu();
    seen.opens = Boolean(menu) && !menu.hidden && menu.getAttribute("role") === "menu" &&
      plus?.getAttribute("aria-expanded") === "true";
    // 자리는 들어오는 움직임이 끝난 뒤의 것 — 메뉴는 「+」 위에 선다.
    await H.settle(250);
    seen.above = menu && plus
      ? menu.getBoundingClientRect().bottom <= plus.getBoundingClientRect().top + 1
      : false;
    const rows = H.rows();
    seen.words = rows.map((row) => row.querySelector(".note-pop-name")?.textContent ?? null);
    seen.wantWords = [
      t("composer.attach.files", "파일 및 폴더"),
      t("composer.attach.image", "이미지 붙여넣기"),
      t("composer.attach.open", "열린 파일"),
    ];
    seen.filesOn = Boolean(rows[0]) && !rows[0].disabled;
    seen.imageOff = rows[1]?.disabled === true;
    seen.openOff = rows[2]?.disabled === true;
    seen.rowsNoTitle = rows.every((row) => !row.hasAttribute("title"));
    if (menu) H.key(menu, { key: "Escape" });
    await H.settle(250);
    seen.closed = menu?.hidden === true && plus?.getAttribute("aria-expanded") === "false";
    // 조건을 바꿔 다시 연다: 클립보드에 그림, 파일 탭 하나.
    window.__ANSWER__.clipboard_has_image = () => true;
    window.__ANSWER__.read_text_file = () => ({ text: "# plan", version: "1:1" });
    await openFile("docs/plan.md");
    await H.settle(100);
    setActiveTab(window.__ATTACH__.helperId);
    await H.settle(60);
    H.plus()?.click();
    await H.settle();
    const again = H.rows();
    seen.imageOn = Boolean(again[1]) && !again[1].disabled;
    seen.openOn = Boolean(again[2]) && !again[2].disabled;
    if (H.menu()) H.key(H.menu(), { key: "Escape" });
    await H.settle(250);
    return seen;
  });
  ok(
    "the composer's '+' stands first in its toolbar with a name and no native title, opens a menu of three rows above it, and each row is enabled by its condition: files and folders always, the image only when the clipboard holds a picture, open files only when a file tab is open",
    stood.composerStands && plusMenu.plusFirst && plusMenu.named && plusMenu.noTitle && plusMenu.popup &&
      plusMenu.opens && plusMenu.above &&
      JSON.stringify(plusMenu.words) === JSON.stringify(plusMenu.wantWords) &&
      plusMenu.filesOn && plusMenu.imageOff && plusMenu.openOff && plusMenu.rowsNoTitle &&
      plusMenu.closed && plusMenu.imageOn && plusMenu.openOn,
    JSON.stringify({ stood, plusMenu }),
  );

  /* ② 키보드: 화살표가 열린 행 사이를 돌고, Esc가 닫으며 초점을 돌려준다. */
  const keys = await probe(async () => {
    const H = window.__ATTACH_H__;
    const seen = {};
    window.__ANSWER__.clipboard_has_image = () => false;
    H.plus().click();
    await H.settle();
    const menu = H.menu();
    const rows = H.rows();
    seen.firstFocused = document.activeElement === rows[0];
    H.key(menu, { key: "ArrowDown" });
    // 그림 행은 닫혀 있으니 건너뛴다.
    seen.skipsDisabled = document.activeElement === rows[2];
    H.key(menu, { key: "ArrowDown" });
    seen.wrapsDown = document.activeElement === rows[0];
    H.key(menu, { key: "ArrowUp" });
    seen.wrapsUp = document.activeElement === rows[2];
    H.key(menu, { key: "Home" });
    seen.home = document.activeElement === rows[0];
    H.key(menu, { key: "End" });
    seen.end = document.activeElement === rows[2];
    H.key(menu, { key: "Escape" });
    await H.settle(250);
    seen.escClosed = menu.hidden;
    seen.focusBack = document.activeElement === H.plus();
    H.plus().click();
    await H.settle();
    seen.reopened = !menu.hidden;
    document.body.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
    await H.settle(250);
    seen.outsideCloses = menu.hidden;
    return seen;
  });
  ok(
    "the '+' menu is keyboard-driven: arrows move between the enabled rows and wrap, Home and End reach the ends, Escape closes it and hands the focus back to the '+', and a press outside closes it",
    keys.firstFocused && keys.skipsDisabled && keys.wrapsDown && keys.wrapsUp && keys.home && keys.end &&
      keys.escClosed && keys.focusBack && keys.reopened && keys.outsideCloses,
    JSON.stringify(keys),
  );

  /* ②′ 「파일 및 폴더」 행 → 이음매(`openPathBrowser`, 스텁) → 칩. 진짜 브라우저는 ⑨. */
  const seam = await probe(async () => {
    const H = window.__ATTACH_H__;
    const seen = {};
    const asks = [];
    window.openPathBrowser = async (opts) => {
      asks.push({ ...opts });
      return ["/tmp/seam one.md", "/tmp/seam"];
    };
    window.__ANSWER__.path_kinds = ({ paths }) => paths.map((path) => (path.endsWith("/seam") ? "folder" : "file"));
    await H.viaMenu(0);
    seen.asked = asks.length === 1 && asks[0].mode === "attach" && asks[0].multiple === true &&
      asks[0].start === window.__ATTACH__.worktree;
    seen.names = H.names();
    seen.glyphs = H.glyphs();
    await H.settle(200);
    seen.menuClosed = H.menu()?.hidden === true;
    seen.boxFocused = document.activeElement === H.box();
    // 취소(빈 답)는 칩을 만들지 않는다.
    window.openPathBrowser = async () => [];
    await H.viaMenu(0);
    seen.cancelQuiet = H.chips().length === 2;
    await H.clearChips();
    return seen;
  });
  ok(
    "the files-and-folders row asks the window's one browser seam for attach mode with the parent's checkout as the start, its answer becomes chips and the focus returns to the box; a cancelled browser adds nothing",
    seam.asked && JSON.stringify(seam.names) === JSON.stringify(["seam one.md", "seam"]) &&
      JSON.stringify(seam.glyphs) === JSON.stringify(["#i-file", "#i-folder"]) &&
      seam.menuClosed && seam.boxFocused && seam.cancelQuiet,
    JSON.stringify(seam),
  );

  /* ③ 브라우저의 답 → 칩 셋(파일·파일·폴더), ×, 같은 경로는 하나. */
  const chips = await probe(async () => {
    const H = window.__ATTACH_H__;
    const seen = {};
    const asks = [];
    window.openPathBrowser = async (opts) => {
      asks.push({ ...opts });
      return ["/tmp/a b.md", "/tmp/plan.md", "/tmp/dir"];
    };
    window.__ANSWER__.path_kinds = ({ paths }) =>
      paths.map((path) => (path.endsWith("/dir") ? "folder" : path.includes("gone") ? "missing" : "file"));
    await H.addPaths(await window.openPathBrowser());
    seen.count = H.chips().length;
    seen.names = H.names();
    seen.glyphs = H.glyphs();
    seen.tips = H.chips().map((chip) => chip.dataset.tip);
    seen.noTitle = H.chips().every((chip) => !chip.hasAttribute("title"));
    seen.rowShown = H.composer().querySelector(".composer-attach-row")?.hidden === false;
    const drop = H.chips()[1]?.querySelector("button");
    seen.dropNamed = drop?.getAttribute("aria-label") ===
      t("composer.attach.remove", "{{name}} 첨부 지우기", { name: "plan.md" });
    drop?.click();
    await H.settle();
    seen.afterDrop = H.names();
    window.openPathBrowser = async () => ["/tmp/dir"];
    await H.addPaths(await window.openPathBrowser());
    seen.dedupe = H.names();
    return seen;
  });
  ok(
    "the browser's answer becomes chips above the textarea — two files and a folder with their glyphs and the full path as the tooltip — a chip's × removes it, and the same path twice is one chip",
    chips.count === 3 &&
      JSON.stringify(chips.names) === JSON.stringify(["a b.md", "plan.md", "dir"]) &&
      JSON.stringify(chips.glyphs) === JSON.stringify(["#i-file", "#i-file", "#i-folder"]) &&
      JSON.stringify(chips.tips) === JSON.stringify(["/tmp/a b.md", "/tmp/plan.md", "/tmp/dir"]) &&
      chips.noTitle && chips.rowShown && chips.dropNamed &&
      JSON.stringify(chips.afterDrop) === JSON.stringify(["a b.md", "dir"]) &&
      JSON.stringify(chips.dedupe) === JSON.stringify(["a b.md", "dir"]),
    JSON.stringify(chips),
  );

  /* ④ 표의 상한: 넘친 경로는 토스트 하나로 거절, 칩 줄은 N개 + 「… 외 N」. */
  const cap = await probe(async (limits) => {
    const H = window.__ATTACH_H__;
    const seen = {};
    window.openPathBrowser = async () =>
      Array.from({ length: limits.chips + 1 }, (_, i) => `/tmp/many/${i}.md`);
    const toastsBefore = H.toasts();
    await H.addPaths(await window.openPathBrowser(), 200);
    seen.visible = H.chips().length;
    seen.more = H.composer().querySelector(".composer-attach-more")?.textContent ?? null;
    seen.wantMore = t("composer.attach.more", "… 외 {{count}}", { count: limits.chips - limits.shown });
    seen.toasted = H.toasts() === toastsBefore + 1;
    // 경로 하나가 표의 길이를 넘으면 그것만 거절된다.
    window.openPathBrowser = async () => [`/tmp/${"x".repeat(limits.pathChars)}.md`];
    await H.clearChips();
    const before = H.toasts();
    await H.addPaths(await window.openPathBrowser());
    seen.longRefused = H.chips().length === 0 && H.toasts() === before + 1;
    return seen;
  }, limits);
  ok(
    "past the table's cap the extra paths are refused with one toast, the row shows the first N chips and one '… 외 N' counter, and a path longer than the table's length is refused alone",
    cap.visible === limits.shown && cap.more === cap.wantMore && cap.toasted && cap.longRefused,
    JSON.stringify(cap),
  );

  /* ⑤ 보내기 형식 — 부모 판이 받는 글 그대로. */
  const send = await probe(async () => {
    const H = window.__ATTACH_H__;
    const seen = {};
    const term = window.__ATTACH__.term;
    const calls = [];
    window.__ANSWER__.term_paste = (args) => (calls.push(["paste", args.term, args.text]), null);
    window.__ANSWER__.term_key = (args) => (calls.push(["key", args.term, args.press.key]), null);
    window.__ATTACH__.calls = calls;
    window.openPathBrowser = async () => ["/tmp/a b.md", "/tmp/dir"];
    await H.addPaths(await window.openPathBrowser());
    H.box().value = "  look at these  ";
    H.submit();
    await H.settle(700);
    seen.sent = JSON.stringify(calls) === JSON.stringify([
      ["paste", term, 'look at these\n\n첨부:\n"/tmp/a b.md"\n/tmp/dir'],
      ["key", term, "Enter"],
    ]);
    seen.got = calls;
    seen.cleared = H.chips().length === 0 &&
      H.composer().querySelector(".composer-attach-row")?.hidden === true && H.box().value === "";
    // 칩만: 「첨부:」 줄만 간다.
    window.openPathBrowser = async () => ["/tmp/plan.md"];
    await H.addPaths(await window.openPathBrowser());
    H.box().value = "";
    H.submit();
    await H.settle(700);
    seen.chipsAlone = JSON.stringify(calls.slice(2)) === JSON.stringify([
      ["paste", term, "첨부:\n/tmp/plan.md"],
      ["key", term, "Enter"],
    ]);
    // 글만: 확인할 것이 없으니 첫 붙여넣기가 동기로 나간다(t-2973 핀 그대로).
    calls.length = 0;
    H.box().value = "just words";
    H.submit();
    seen.syncFirst = calls.length === 1 && calls[0][2] === "just words";
    await H.settle(700);
    seen.wordsThenEnter = calls.length === 2 && calls[1][2] === "Enter";
    // 빈 상자에 칩도 없으면 아무것도 가지 않는다.
    H.submit();
    await H.settle(700);
    seen.emptyRefused = calls.length === 2;
    return seen;
  });
  ok(
    "a send rides term_paste as the words, a blank line, 첨부: and one absolute path per line — quoted when it holds a space — then Enter; the chips leave after the send, chips alone send the 첨부: block by itself, words alone still paste synchronously, and an empty box sends nothing",
    send.sent && send.cleared && send.chipsAlone && send.syncFirst && send.wordsThenEnter && send.emptyRefused,
    JSON.stringify(send),
  );

  /* ⑥ 보내기 직전에 사라진 경로: 「없음」 표시로 남고, 가지 않는다. */
  const missing = await probe(async () => {
    const H = window.__ATTACH_H__;
    const seen = {};
    const term = window.__ATTACH__.term;
    const calls = window.__ATTACH__.calls;
    calls.length = 0;
    window.openPathBrowser = async () => ["/tmp/gone.md", "/tmp/plan.md"];
    // 들일 때는 둘 다 있었고, 보낼 때는 하나가 없다.
    let gone = false;
    window.__ANSWER__.path_kinds = ({ paths }) =>
      paths.map((path) => (gone && path.includes("gone") ? "missing" : "file"));
    await H.addPaths(await window.openPathBrowser());
    seen.bothStand = H.chips().length === 2 && H.chips().every((chip) => !chip.classList.contains("is-missing"));
    gone = true;
    H.box().value = "check";
    H.submit();
    await H.settle(700);
    seen.message = JSON.stringify(calls) === JSON.stringify([
      ["paste", term, "check\n\n첨부:\n/tmp/plan.md"],
      ["key", term, "Enter"],
    ]);
    const left = H.chips();
    seen.kept = left.length === 1 && left[0].dataset.path === "/tmp/gone.md" &&
      left[0].classList.contains("is-missing") &&
      left[0].querySelector(".composer-attach-missing")?.textContent === t("composer.attach.missing", "없음");
    // 없는 칩만 남고 글도 없으면 아무것도 가지 않는다 — Enter 홀로 부모 판에
    // 떨어지면 그 판이 들고 있던 반쪽 문장이 제출된다.
    calls.length = 0;
    H.box().value = "";
    H.submit();
    await H.settle(700);
    seen.nothing = calls.length === 0 && H.chips().length === 1;
    await H.clearChips();
    seen.gone = H.chips().length === 0;
    return seen;
  });
  ok(
    "a path that vanished before the send is marked 없음, kept as a chip, and left out of the message; with nothing else to send, nothing is sent",
    missing.bothStand && missing.message && missing.kept && missing.nothing && missing.gone,
    JSON.stringify(missing),
  );

  /* ⑦ OS 드롭: 입력줄 위에 떨어지면 칩, 판에는 붙지 않는다. */
  const drop = await probe(async () => {
    const H = window.__ATTACH_H__;
    const seen = {};
    const fire = (window.__LISTENERS__["tauri://drag-drop"] ?? [])[0];
    seen.listens = typeof fire === "function";
    if (!seen.listens) return seen;
    const pastes = [];
    window.__ANSWER__.term_paste = (args) => (pastes.push(args.text), null);
    const scale = window.devicePixelRatio || 1;
    const at = H.composer().getBoundingClientRect();
    const position = { x: (at.left + at.width / 2) * scale, y: (at.top + at.height / 2) * scale };
    await fire({ payload: { paths: ["/tmp/한 컷.png", "/tmp/설계 노트.md"], position } });
    await H.settle(150);
    seen.names = H.names();
    seen.glyphs = H.glyphs();
    seen.noPaste = pastes.length === 0;
    await fire({ payload: { paths: ["/tmp/nowhere.md"], position: { x: 1, y: 1 } } });
    await H.settle(150);
    seen.outsideSilent = pastes.length === 0 && H.chips().length === 2;
    await H.clearChips();
    return seen;
  });
  ok(
    "an OS drop over the composer stands chips instead of pasting into a pane — an image path as an image chip — and a drop outside every target stays silent",
    drop.listens && JSON.stringify(drop.names) === JSON.stringify(["한 컷.png", "설계 노트.md"]) &&
      JSON.stringify(drop.glyphs) === JSON.stringify(["#i-image", "#i-file"]) &&
      drop.noPaste && drop.outsideSilent,
    JSON.stringify(drop),
  );

  /* ⑧ ⌘V — 그림만 든 클립보드는 창의 한 그림 길로 앉아 칩이 되고, 글자는 글자다. */
  const paste = await probe(async () => {
    const H = window.__ATTACH_H__;
    const seen = {};
    let saved = 0;
    window.__ANSWER__.save_pasted_image = (args, options) => {
      saved += 1;
      seen.kind = options?.headers?.["x-image-kind"];
      seen.raw = args instanceof Uint8Array;
      return "/tmp/zerocode-paste/paste-1.png";
    };
    const box = H.box();
    const image = new DataTransfer();
    image.items.add(new File([new Uint8Array([137, 80, 78, 71])], "shot.png", { type: "image/png" }));
    const first = new ClipboardEvent("paste", { clipboardData: image, bubbles: true, cancelable: true });
    box.dispatchEvent(first);
    seen.prevented = first.defaultPrevented;
    await H.settle(150);
    seen.chip = H.chips().length === 1 && H.chips()[0].dataset.path === "/tmp/zerocode-paste/paste-1.png" &&
      H.glyphs()[0] === "#i-image";
    seen.savedOnce = saved === 1;
    const worded = new DataTransfer();
    worded.setData("text/plain", "echo hello");
    worded.items.add(new File([new Uint8Array([1])], "x.png", { type: "image/png" }));
    const second = new ClipboardEvent("paste", { clipboardData: worded, bubbles: true, cancelable: true });
    box.dispatchEvent(second);
    await H.settle(100);
    seen.wordsFree = !second.defaultPrevented && saved === 1 && H.chips().length === 1;
    delete window.__ANSWER__.save_pasted_image;
    await H.clearChips();
    return seen;
  });
  ok(
    "⌘V with a picture-only clipboard on the composer seats the picture as a temp file through the window's one image road and stands one image chip; words keep pasting as words",
    paste.prevented && paste.chip && paste.savedOnce && paste.kind === "image/png" && paste.raw && paste.wordsFree,
    JSON.stringify(paste),
  );

  /* ⑨ 진짜 브라우저(t-2982): 「파일 및 폴더」 행이 attach 모드로 열고, 담은 행이 칩이 된다. */
  const browser = await probe(async () => {
    const H = window.__ATTACH_H__;
    const seen = {};
    window.openPathBrowser = window.__ATTACH__.heldBrowser;
    const start = window.__ATTACH__.worktree ?? "/tmp/zerocode-window-test";
    window.__ANSWER__.browse_dir = ({ path }) => ({
      path,
      parent: null,
      entries: [{ name: "one.md", is_dir: false }, { name: "two.md", is_dir: false }, { name: "sub", is_dir: true }],
      total: 3,
      truncated: false,
    });
    window.__ANSWER__.browse_places = () => ({ home: "/tmp", places: [{ kind: "recent", name: "proj", path: start }] });
    window.__ANSWER__.path_kinds = ({ paths }) => paths.map((path) => (path.endsWith("/sub") ? "folder" : "file"));
    H.plus().click();
    await H.settle();
    H.rows()[0].click();
    await H.settle(150);
    seen.browserOpen = el("pb-scrim").hidden === false;
    seen.attachTitle = el("pb-title").textContent === t("project.browse.attachTitle", "파일 및 폴더 첨부");
    // 시작점이 없으면 시작 화면 — 최근 행으로 들어간다.
    if (el("pb-path").value === "") {
      el("pb-rows").firstElementChild?.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
      await H.settle(150);
    }
    seen.at = el("pb-path").value;
    const pbRows = () => [...el("pb-rows").children];
    pbRows()[0]?.querySelector(".pb-check")?.click();
    pbRows()[2]?.querySelector(".pb-check")?.click();
    seen.confirmWords = el("pb-confirm").textContent;
    seen.wantConfirm = t("project.browse.attach", "첨부 {{count}}개", { count: 2 });
    el("pb-confirm").click();
    await H.settle(200);
    seen.browserGone = el("pb-scrim").hidden === true;
    seen.names = H.names();
    seen.glyphs = H.glyphs();
    seen.paths = H.chips().map((chip) => chip.dataset.path);
    seen.wantPaths = [joinBrowsePath(seen.at, "one.md"), joinBrowsePath(seen.at, "sub")];
    delete window.__ANSWER__.browse_dir;
    delete window.__ANSWER__.browse_places;
    await H.clearChips();
    return seen;
  });
  ok(
    "the files-and-folders row opens the window's one browser in attach mode and its checked rows — a file and a folder — become chips with absolute paths",
    browser.browserOpen && browser.attachTitle && browser.confirmWords === browser.wantConfirm &&
      browser.browserGone &&
      JSON.stringify(browser.names) === JSON.stringify(["one.md", "sub"]) &&
      JSON.stringify(browser.glyphs) === JSON.stringify(["#i-file", "#i-folder"]) &&
      JSON.stringify(browser.paths) === JSON.stringify(browser.wantPaths),
    JSON.stringify(browser),
  );

  /* ⑩ 그림 행과 열린 파일 행. */
  const roads = await probe(async () => {
    const H = window.__ATTACH_H__;
    const seen = {};
    window.__ANSWER__.clipboard_has_image = () => true;
    let seated = 0;
    window.__ANSWER__.save_clipboard_image = () => (seated += 1, "/tmp/zerocode-paste/paste-2.png");
    await H.viaMenu(1);
    seen.imageChip = seated === 1 && H.chips().length === 1 &&
      H.chips()[0].dataset.path === "/tmp/zerocode-paste/paste-2.png" && H.glyphs()[0] === "#i-image";
    H.plus().click();
    await H.settle();
    H.rows()[2].click();
    await H.settle();
    seen.stillOpen = H.menu().hidden === false;
    const files = H.rows();
    const abs = joinBrowsePath(window.__ATTACH__.root, "docs/plan.md");
    seen.listed = files.map((row) => [
      row.querySelector(".note-pop-name")?.textContent,
      row.querySelector(".note-pop-where")?.textContent,
    ]);
    seen.wantListed = [["plan.md", abs]];
    seen.firstFocused = document.activeElement === files[0];
    // ← 는 첫 단으로 돌아간다.
    H.key(H.menu(), { key: "ArrowLeft" });
    seen.backToRoot = H.rows().length === 3;
    H.rows()[2].click();
    await H.settle();
    H.rows()[0].click();
    await H.settle(150);
    seen.picked = H.chips().length === 2 && H.chips()[1].dataset.path === abs && H.glyphs()[1] === "#i-file";
    await H.settle(200);
    seen.menuClosed = H.menu().hidden === true;
    await H.clearChips();
    // 되돌린다.
    delete window.__ANSWER__.save_clipboard_image;
    delete window.__ANSWER__.clipboard_has_image;
    delete window.__ANSWER__.path_kinds;
    delete window.__ANSWER__.term_paste;
    delete window.__ANSWER__.term_key;
    delete window.__ANSWER__.subagent_log;
    delete window.__ANSWER__.read_text_file;
    window.openPathBrowser = window.__ATTACH__.heldBrowser;
    activeWorktreePath = window.__ATTACH__.heldWorktreePath;
    closeTab("file:docs/plan.md");
    closeTab(window.__ATTACH__.helperId);
    closeTab(window.__ATTACH__.ownerId);
    await H.settle(120);
    return seen;
  });
  ok(
    "the image row seats the clipboard picture through save_clipboard_image, and the open-files row lists the window's file tabs with their absolute paths under the checkout — a pick stands the chip, ← returns to the first level",
    roads.imageChip && roads.stillOpen && JSON.stringify(roads.listed) === JSON.stringify(roads.wantListed) &&
      roads.firstFocused && roads.backToRoot && roads.picked && roads.menuClosed,
    JSON.stringify(roads),
  );
}
