/* ---- 입력줄 옆 「+」 — 파일·폴더·이미지 첨부 (t-2993) ----------------------
 *
 * 설계: docs/design/composer-attachments.md. 헬퍼 페이지의 입력줄(t-2973의
 * textarea, shell.js `workerComposerNode`)이 이 모듈을 부른다 — textarea 위의
 * 칩 줄, 칩이 되는 길들(브라우저의 답·OS 드롭·⌘V 그림), 보내기 직전의 경로
 * 확인, 부모 판으로 가는 글의 형식이 한 책임이다. 입력줄 하나에 매이지 않아
 * 뒤의 입력줄(자동화·아티팩트)도 같은 것을 쓴다: `composerAttachments(form,
 * box, tools)` 하나가 손잡이를 돌려주고, 입력줄은 보낼 때 `outgoing(text)`를
 * 물을 뿐이다.
 *
 * 이 창에 이미 있는 길만 쓴다(§1): 파일·폴더는 t-2982의 창 안 브라우저
 * (`openPathBrowser`, attach 모드 — 두 번째 브라우저는 없다), 이미지는 창의
 * 임시 PNG 길(`save_clipboard_image` / shell-input.js `savePastedImage`),
 * 드롭은 shell-term.js 의 `tauri://drag-drop` 리스너가 입력줄을 과녁으로 하나
 * 더 알 뿐이고, 보내기는 입력줄의 기존 `term_paste` + Enter다. 경로가 무엇인지
 * (파일·폴더·없음)는 백엔드의 `path_kinds`가 말한다 — 창은 파일을 읽지 않는다.
 *
 * 낱말은 `composer.attach.*`, 색·간격은 `--chat-*`와 기존 토큰, 숫자는 아래
 * 표 하나. */

/* 표 하나 — 첨부의 상한. 창 하네스가 이 표를 읽어 핀을 잰다. */
const ATTACH_LIMITS = Object.freeze({
  /* 한 번에 드는 첨부 수. 넘치면 토스트 한 장, 넘친 것은 들지 않는다. */
  chips: 20,
  /* 경로 한 줄의 글자 수. 브라우저가 답할 리 없는 길이 — 잘못 떨어진 덩어리다. */
  pathChars: 1024,
  /* 칩 줄에 서는 칩 수. 넘치면 「… 외 N」 한 칩이 나머지를 센다. */
  shown: 6,
  /* 「열린 파일」 목록의 행 수. */
  menuRows: 12,
});

/* 부모 판이 받는 글의 형식(§2.3): 사람의 글, 빈 줄, 이 머리말, 절대 경로 한
 * 줄에 하나 — 공백이나 따옴표가 있으면 큰따옴표로 감싼다. 에이전트가 읽는
 * 줄이라 로케일을 타지 않는다: Claude Code·Codex·zo 어느 부모든 절대 경로를
 * 그대로 읽고, zo 부모에는 이미지 경로가 그대로 첨부가 된다. */
// A wire word, not a label: the parent agent reads this line (design §3),
// so it is the same in every UI language — the harness pins it under `en`.
// The source contract names this exact line in its allowed list.
const ATTACH_HEADER = "첨부:";

// No regex literal with a quote inside it here: the shell's source contracts
// strip string literals without knowing regex syntax, and a `"` inside a
// regex opened a string that swallowed every definition after this line.
function attachLine(path) {
  const quoted = path.includes('"') || /\s/.test(path);
  return quoted ? `"${path.split('"').join('\\"')}"` : path;
}

function attachMessage(text, paths) {
  const block = [ATTACH_HEADER, ...paths.map(attachLine)].join("\n");
  return text ? `${text}\n\n${block}` : block;
}

/* 칩의 종류와 그 아이콘. 폴더는 백엔드의 답으로, 이미지는 판 드롭이 이미 아는
 * 확장자 표(shell-term.js `isImageDropPath`)로 가른다 — 표를 두 벌 두지 않는다. */
const ATTACH_GLYPH = Object.freeze({ file: "file", folder: "folder", image: "image" });

function attachKindOf(path, probed) {
  if (probed === "folder") return "folder";
  return isImageDropPath(path) ? "image" : "file";
}

/* 칩에 서는 이름 — 마지막 조각. 폴더의 끝 구분자는 이름이 아니다. */
function attachName(path) {
  const trimmed = path.replace(/[\\/]+$/, "");
  const cut = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return cut === -1 ? trimmed : trimmed.slice(cut + 1);
}

/* 경로가 무엇인지 백엔드에 묻는다. 답이 없으면(옛 백엔드, 거부) 모른다고
 * 답한다 — 모르는 경로는 확장자로 그리고, 보내기에서는 있는 것으로 친다:
 * 못 물었다고 사람의 첨부를 버리지 않는다. */
async function probeAttachKinds(paths) {
  let kinds = null;
  try {
    kinds = await invoke("path_kinds", { paths });
  } catch {
    kinds = null;
  }
  return paths.map((_, i) => (Array.isArray(kinds) ? kinds[i] ?? null : null));
}

/* 입력줄 → 그 첨부 손잡이. 드롭 리스너가 점 아래의 입력줄에서 손잡이를
 * 찾는 등록부이고, 하네스의 이음매다. */
const ATTACH_OF = new WeakMap();

function composerAttachmentsOf(form) {
  return ATTACH_OF.get(form) ?? null;
}

/* 점 아래의 입력줄 첨부 — `terminalDropTargetAt`와 같은 규칙: 겹쳐 있는 토스트는
 * 드롭을 먹지 않고, 대화상자 뒤의 입력줄은 과녁이 아니다. */
function composerAttachTargetAt(x, y) {
  for (const node of document.elementsFromPoint(x, y)) {
    if (node.closest?.('dialog, [role="dialog"]')) return null;
    const form = node.closest?.(".worker-composer");
    if (form) return ATTACH_OF.get(form) ?? null;
  }
  return null;
}

/* 입력줄 하나에 첨부를 붙인다. `form`은 제출하는 form, `box`는 textarea, `tools`는
 * 그 아래 도구 줄. 칩 줄은 textarea 위에 선다. `start`는 브라우저의 시작
 * 폴더 — 부모 판의 체크아웃. */
function composerAttachments(form, box, tools, { start = null } = {}) {
  const row = document.createElement("div");
  row.className = "composer-attach-row";
  row.hidden = true;
  row.setAttribute("role", "list");
  row.setAttribute("aria-label", t("composer.attach.list", "첨부 목록"));
  // 상자보다 먼저 — 입력줄이 상자를 아직 안 붙였어도, 붙였어도 칩 줄이 위다.
  form.prepend(row);
  // 「+」 — 도구 줄 맨 왼쪽(Codex 자리). 이름은 aria-label 과 창의 툴팁이 말한다.
  const plus = document.createElement("button");
  plus.type = "button";
  plus.className = "composer-attach-plus";
  plus.setAttribute("aria-label", t("composer.attach.add", "첨부"));
  plus.dataset.tip = plus.getAttribute("aria-label");
  plus.setAttribute("aria-haspopup", "menu");
  plus.setAttribute("aria-expanded", "false");
  // 초점이 여기 있는 동안 Enter·Space 는 이 단추의 것 — 창의 키 싱크 되찾기
  // (shell-input.js `rearmKeySink`)가 Esc 로 돌아온 초점을 다시 가져가지 않게.
  plus.dataset.keyboardOwner = "true";
  plus.appendChild(iconNode("plus"));
  tools.insertBefore(plus, tools.firstChild);
  /* 경로 → { path, kind, missing }, 담은 순서. */
  const chips = new Map();

  const paint = () => {
    row.replaceChildren();
    const all = [...chips.values()];
    row.hidden = all.length === 0;
    for (const chip of all.slice(0, ATTACH_LIMITS.shown)) row.appendChild(chipNode(chip));
    if (all.length > ATTACH_LIMITS.shown) {
      const more = document.createElement("span");
      more.className = "composer-attach-more";
      say(more, () =>
        t("composer.attach.more", "… 외 {{count}}", { count: all.length - ATTACH_LIMITS.shown }));
      row.appendChild(more);
    }
  };

  const chipNode = (chip) => {
    const node = document.createElement("span");
    node.className = "composer-attach-chip";
    node.dataset.kind = chip.kind;
    node.dataset.path = chip.path;
    // 전체 경로는 창의 툴팁이 말한다(data-tip — 네이티브 title 은 없다).
    node.dataset.tip = chip.path;
    node.classList.toggle("is-missing", chip.missing);
    node.setAttribute("role", "listitem");
    node.appendChild(iconNode(ATTACH_GLYPH[chip.kind] ?? "file"));
    const name = document.createElement("span");
    name.className = "composer-attach-name";
    name.textContent = attachName(chip.path);
    node.appendChild(name);
    if (chip.missing) {
      const word = document.createElement("span");
      word.className = "composer-attach-missing";
      say(word, () => t("composer.attach.missing", "없음"));
      node.appendChild(word);
    }
    const drop = document.createElement("button");
    drop.type = "button";
    drop.className = "composer-attach-drop";
    drop.setAttribute(
      "aria-label",
      t("composer.attach.remove", "{{name}} 첨부 지우기", { name: name.textContent }),
    );
    drop.appendChild(iconNode("x"));
    drop.addEventListener("click", () => {
      chips.delete(chip.path);
      paint();
      box.focus();
    });
    node.appendChild(drop);
    return node;
  };

  const handle = {
    form,
    box,
    plus,
    start,
    count: () => chips.size,
    paths: () => [...chips.keys()],
    /* 경로들을 칩으로. 같은 경로는 하나, 표를 넘는 것은 토스트 한 장으로
     * 거절. 종류는 백엔드에 한 번 묻는다. */
    async add(paths) {
      const fresh = [];
      let tooLong = false;
      for (const path of paths) {
        if (typeof path !== "string" || path.length === 0 || chips.has(path) || fresh.includes(path)) {
          continue;
        }
        if (path.length > ATTACH_LIMITS.pathChars) {
          tooLong = true;
          continue;
        }
        fresh.push(path);
      }
      if (tooLong) {
        showError(t("composer.attach.tooLong", "경로가 너무 깁니다 ({{cap}}자 초과)", {
          cap: ATTACH_LIMITS.pathChars,
        }));
      }
      const room = ATTACH_LIMITS.chips - chips.size;
      if (fresh.length > room) {
        showError(t("composer.attach.tooMany", "첨부는 {{cap}}개까지입니다", {
          cap: ATTACH_LIMITS.chips,
        }));
        fresh.length = Math.max(0, room);
      }
      if (fresh.length === 0) return;
      const probed = await probeAttachKinds(fresh);
      fresh.forEach((path, i) => {
        if (chips.has(path)) return;
        chips.set(path, { path, kind: attachKindOf(path, probed[i]), missing: probed[i] === "missing" });
      });
      paint();
    },
    /* 보내기 직전(§2.4): 경로를 다시 확인해 없는 것은 「없음」 칩으로 남기고,
     * 있는 것만 형식에 실어 돌려준다 — 실린 칩은 걷힌다. 보낼 것이 없으면
     * null: 글도 없고 있는 칩도 없다. */
    async outgoing(text) {
      const held = [...chips.keys()];
      const probed = await probeAttachKinds(held);
      const sending = [];
      held.forEach((path, i) => {
        const chip = chips.get(path);
        chip.missing = probed[i] === "missing";
        if (!chip.missing) sending.push(path);
      });
      for (const path of sending) chips.delete(path);
      paint();
      if (sending.length === 0) return text || null;
      return attachMessage(text, sending);
    },
  };
  plus.addEventListener("click", () => openAttachMenu(handle));
  // ⌘V 가 상자에 떨어질 때: 글자가 있으면 글자의 길 — 상자가 제 기본 동작으로
  // 받는다. 글자 없이 그림만 왔으면 창의 한 그림 길(shell-input.js
  // `savePastedImage`)로 임시 파일에 앉히고 그 경로가 칩이 된다 — 판이 하는
  // 것과 같은 계약, 자리만 다르다.
  box.addEventListener("paste", (event) => {
    const data = event.clipboardData;
    if (!data || data.getData("text")) return;
    const image = [...(data.items ?? [])]
      .find((item) => item.kind === "file" && item.type.startsWith("image/"));
    const file = image?.getAsFile();
    if (!file) return;
    event.preventDefault();
    void savePastedImage(file).then((path) => (path ? handle.add([path]) : undefined));
  });
  ATTACH_OF.set(form, handle);
  return handle;
}

/* ---- 「+」 메뉴 「추가」 (§2.1) ---------------------------------------------
 *
 * 창에 하나뿐인 메뉴 노드 — 어느 입력줄이 열었는지는 `attachMenuFor`가 안다.
 * Codex 데스크톱의 「+」 자리이고, 행은 이 창이 지킬 수 있는 셋뿐이다:
 *   - 파일 및 폴더 — t-2982의 브라우저(attach 모드, 다중 선택).
 *   - 이미지 붙여넣기 — 클립보드에 그림이 있을 때만. 있는지는 백엔드가 파일을
 *     쓰지 않고 답한다(`clipboard_has_image`); 고르면 창의 임시 PNG 길.
 *   - 열린 파일 — 이 창의 에디터 탭에 열린 파일이 있을 때만; 두 번째 단에서
 *     하나를 고른다.
 * 플러그인·목표·계획 모드 줄은 없다 — 그 문들은 이 창에 다른 자리가 있다.
 *
 * 행의 해부는 터미널 메뉴의 것(`menuRow`, `.note-pop`), 나가는 길은 오버레이
 * 공통의 `closing`, 바깥 누름과 Esc 는 창의 한 dismisser(`dismissable`)다.
 * 키보드: ↑↓ 가 열린 행 사이를 돌고(감김), Home·End, Esc 는 닫고 초점을
 * 「+」로 돌려주며, 둘째 단에서 ←·⌫ 는 첫 단으로. */
let attachMenu = null;
let attachMenuFor = null;
/* 열 때마다 하나씩 — 늦게 온 클립보드의 답이 다음에 연 메뉴의 행을 열지 않게. */
let attachMenuTurn = 0;

function attachMenuNode() {
  if (attachMenu) return attachMenu;
  attachMenu = document.createElement("div");
  attachMenu.className = "note-pop term-menu composer-attach-menu";
  attachMenu.setAttribute("role", "menu");
  attachMenu.setAttribute("aria-label", t("composer.attach.menu", "추가"));
  // 창은 편집 중이 아닌 키를 스테이지의 키 싱크로 되돌린다(shell-input.js
  // `rearmKeySink`) — 이 메뉴가 열려 있는 동안 화살표는 메뉴의 것이다.
  attachMenu.dataset.keyboardOwner = "true";
  attachMenu.hidden = true;
  attachMenu.addEventListener("keydown", attachMenuKeys);
  document.body.appendChild(attachMenu);
  // 나가는 길은 창의 한 dismisser: 바깥 누름(초점은 그대로)과 Esc(초점은
  // 「+」로) — 「+」 자체는 바깥이 아니다, 토글이니까.
  dismissable(
    attachMenu,
    (event) => closeAttachMenu({ refocus: !event }),
    (event) => Boolean(attachMenuFor?.plus.contains(event.target)),
  );
  return attachMenu;
}

function attachMenuItems() {
  return [...attachMenu.querySelectorAll('[role="menuitem"]:not(:disabled)')];
}

function focusAttachMenuItem(index) {
  const items = attachMenuItems();
  if (items.length === 0) return;
  items[((index % items.length) + items.length) % items.length].focus();
}

function attachMenuKeys(event) {
  if (event.isComposing || event.key === "Process" || event.keyCode === 229) return;
  const items = attachMenuItems();
  const at = items.indexOf(document.activeElement);
  if (event.key === "ArrowDown") {
    event.preventDefault();
    focusAttachMenuItem(at + 1);
  } else if (event.key === "ArrowUp") {
    event.preventDefault();
    focusAttachMenuItem(at - 1);
  } else if (event.key === "Home") {
    event.preventDefault();
    focusAttachMenuItem(0);
  } else if (event.key === "End") {
    event.preventDefault();
    focusAttachMenuItem(-1);
  } else if (event.key === "Tab") {
    closeAttachMenu({ refocus: false });
  } else if ((event.key === "ArrowLeft" || event.key === "Backspace") && attachMenu.dataset.level === "files") {
    event.preventDefault();
    if (attachMenuFor) paintAttachMenuRoot(attachMenuFor);
  }
}

function openAttachMenu(handle) {
  const menu = attachMenuNode();
  if (!menu.hidden && attachMenuFor === handle) {
    closeAttachMenu();
    return;
  }
  attachMenuFor = handle;
  showing(menu);
  handle.plus.setAttribute("aria-expanded", "true");
  paintAttachMenuRoot(handle);
}

function closeAttachMenu({ refocus = true } = {}) {
  if (!attachMenu || attachMenu.hidden) return;
  const plus = attachMenuFor?.plus ?? null;
  attachMenuFor = null;
  attachMenuTurn += 1;
  closing(attachMenu);
  plus?.setAttribute("aria-expanded", "false");
  if (refocus) plus?.focus();
}

/* 「+」 위에 선다 — 입력줄은 바닥에 있으니 메뉴는 위로 뜬다. 창의 가장자리
 * 안으로 들이는 여백은 간격 토큰 하나다. */
function placeAttachMenu(anchor) {
  const gap = parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--space-2")) || 0;
  const at = anchor.getBoundingClientRect();
  const box = attachMenu.getBoundingClientRect();
  const left = Math.max(gap, Math.min(at.left, window.innerWidth - box.width - gap));
  const top = Math.max(gap, at.top - box.height - gap);
  attachMenu.style.left = `${left}px`;
  attachMenu.style.top = `${top}px`;
}

function attachMenuRow(host, spec) {
  const row = menuRow(host, { ...spec, close: spec.close ?? closeAttachMenu });
  row.setAttribute("role", "menuitem");
  return row;
}

function paintAttachMenuRoot(handle) {
  const host = attachMenu;
  host.replaceChildren();
  host.dataset.level = "root";
  attachMenuRow(host, {
    said: t("composer.attach.files", "파일 및 폴더"),
    glyph: "folder-plus",
    // 이음매가 없는 창(하네스의 옛 부팅)에서는 행이 닫힌다 — 흉내 내지 않는다.
    disabled: typeof openPathBrowser !== "function",
    run: () => void pickAttachPaths(handle),
  });
  const image = attachMenuRow(host, {
    said: t("composer.attach.image", "이미지 붙여넣기"),
    glyph: "image",
    disabled: true,
    run: () => void seatClipboardImage(handle),
  });
  const opened = openFileTabs();
  attachMenuRow(host, {
    said: t("composer.attach.open", "열린 파일"),
    glyph: "file",
    disabled: opened.length === 0,
    badge: "▸",
    close: () => {},
    run: () => paintAttachMenuOpenFiles(handle, opened),
  });
  // 클립보드에 그림이 있는지는 백엔드가 안다 — 답이 오면 행이 열린다. 파일은
  // 쓰지 않는다; 늦은 답은 그때 열린 메뉴의 것이 아니면 버린다.
  const turn = ++attachMenuTurn;
  invoke("clipboard_has_image")
    .then((has) => {
      if (turn === attachMenuTurn && has === true) image.disabled = false;
    })
    .catch(() => {});
  placeAttachMenu(handle.plus);
  focusAttachMenuItem(0);
}

/* 둘째 단: 열린 파일 하나를 고른다. 이름 옆에 절대 경로가 흐리게 선다. */
function paintAttachMenuOpenFiles(handle, opened) {
  const host = attachMenu;
  host.replaceChildren();
  host.dataset.level = "files";
  const label = document.createElement("div");
  label.className = "note-pop-label";
  label.textContent = t("composer.attach.open", "열린 파일");
  host.appendChild(label);
  for (const file of opened.slice(0, ATTACH_LIMITS.menuRows)) {
    const row = attachMenuRow(host, {
      said: attachName(file.path),
      glyph: "file",
      run: () => void handle.add([file.path]).then(() => handle.box.focus()),
    });
    const where = document.createElement("span");
    where.className = "note-pop-where";
    where.textContent = file.path;
    row.appendChild(where);
  }
  if (opened.length > ATTACH_LIMITS.menuRows) {
    const rest = document.createElement("div");
    rest.className = "note-pop-none";
    rest.textContent = t("composer.attach.more", "… 외 {{count}}", {
      count: opened.length - ATTACH_LIMITS.menuRows,
    });
    host.appendChild(rest);
  }
  placeAttachMenu(handle.plus);
  focusAttachMenuItem(0);
}

/* 이 창의 에디터 탭에 열린 파일들, 절대 경로로. 탭의 경로는 체크아웃 상대
 * 경로일 수 있으니 창이 보는 체크아웃 아래로 붙인다 — 그 자리를 모르면
 * 이름을 지어낼 수 없으므로 뺀다(보내기 직전의 확인이 어차피 잡는다). */
function openFileTabs() {
  const root = activeWorktreePath ?? activeProjectPath ?? null;
  const out = [];
  for (const tab of tabs) {
    if (tab.kind !== "file" || typeof tab.path !== "string" || tab.path.length === 0) continue;
    const absolute = /^(\/|[A-Za-z]:[\\/]|\\\\)/.test(tab.path)
      ? tab.path
      : root
        ? joinBrowsePath(root, tab.path)
        : null;
    if (absolute) out.push({ path: absolute });
  }
  return out;
}

/* 파일 및 폴더 — 창에 하나뿐인 브라우저의 attach 모드. 답은 담은 순서의 절대
 * 경로, 취소는 빈 배열. */
async function pickAttachPaths(handle) {
  const paths = await openPathBrowser({ mode: "attach", start: handle.start, multiple: true });
  if (paths.length > 0) await handle.add(paths);
  handle.box.focus();
}

/* 이미지 붙여넣기 — 클립보드의 그림을 창의 임시 PNG 길로 앉히고 그 경로가 칩이 된다. */
async function seatClipboardImage(handle) {
  let path = null;
  try {
    path = await invoke("save_clipboard_image");
  } catch (error) {
    showError(error);
    return;
  }
  if (typeof path === "string" && path.length > 0) await handle.add([path]);
  handle.box.focus();
}
