/* ---- the composer's chips and its `/` palette ------------------------------
 *
 * The Claude Code extension's composer, taken apart (docs/design/
 * agent-conversation-claude-code-grammar-20260915.md §4): under the box, on
 * the left, the agent chip — the CLI's own mark, its name, the model it runs —
 * that opens the model menu; beside it the permission-mode chip; on the right
 * the `/` and the send. Typing `/` in the box opens the palette the CLI itself
 * would show: the commands THIS installation takes in THIS checkout, read from
 * the CLI (`slash_commands`, slash_catalog.rs), with the source named on the
 * palette's head so a documented list is never passed off as the binary's.
 *
 * Every word a person acts on travels one road: `deliverToPane` — paste, a
 * breath, Enter — the same road the composer's own send walks, so a model
 * picked from the menu and a command typed by hand reach the CLI the same way.
 * A CLI whose picker lives on its own screen (Codex `/model`) gets the screen
 * back so the picker is where the person is looking. */

/* Orca's measured submit interval — the words land, the TUI draws them, then
 * Enter (`NATIVE_CHAT_SUBMIT_DELAY_MS`; the Rust answer road's
 * `SUBMIT_DELAY_MS` is the same measurement). */
const COMPOSER_SUBMIT_DELAY_MS = 500;

/* How long a palette answer stands before the CLI is asked again. */
const SLASH_CATALOG_TTL_MS = 5 * 60 * 1000;

/* Send words to a pane the measured way: paste, one breath, Enter.
 *
 * `landed` is called the moment the words are in the pane's own box, before
 * the breath: an Enter that fails AFTER that leaves the words standing over
 * there, and the caller has to wear that difference (`run.sendUncertain` —
 * 「전달됐지만 전송 완료를 확인하지 못했습니다」) rather than report a send
 * that simply failed. */
async function deliverToPane(term, text, landed = null) {
  await invoke("term_paste", { term, text });
  landed?.();
  await new Promise((done) => setTimeout(done, COMPOSER_SUBMIT_DELAY_MS));
  await invoke("term_key", { term, press: { key: "Enter", ctrl: false, alt: false } });
}

/* The screen a picker lives on: a pane wearing its conversation is turned back
 * to its terminal so the CLI's own dialog is in front of the person. */
function showPaneScreen(term) {
  if (typeof paneChatOn === "function" && paneChatOn(term)) setPaneChat(term, false);
}

/* The CLI's spelling as words a person reads: `bypassPermissions` →
 * "bypass permissions", `workspace-write` → "workspace write". No table of
 * translations — the mode is the CLI's vocabulary, only its casing is opened. */
function permissionModeWords(mode) {
  return String(mode)
    .replace(/([a-z])([A-Z])/g, "$1 $2")
    .replace(/[-_]+/g, " ")
    .toLowerCase();
}

/* How far the run's permission mode reaches, off the catalog's console table
 * (`permission_modes`, `list_agents`): `edits`, `plan` or `bypass` — or
 * `null` for a mode that asks, and for no mode at all, which wears nothing.
 * The send, the focus ring and the spinner's mark wear it (the extension's
 * `[data-permission-mode]`), and the words are each CLI's own. */
function composerReachOf(run) {
  const spec = installedAgents().find((row) => row.id === run.agent) ?? null;
  if (!spec) return null;
  const mode = composerRoad(run, spec).mode();
  const reach = spec.permission_modes?.find((row) => row.mode === mode)?.reach ?? "ask";
  return reach === "ask" ? null : reach;
}

/* `data-permission-reach` on a node, or nothing at all for the asking mode
 * — a rule that matches the attribute matches only a reach that colours. */
function wearReach(node, reach) {
  if (reach) node.setAttribute("data-permission-reach", reach);
  else node.removeAttribute("data-permission-reach");
}

/* ---- the road ----------------------------------------------------------- */

/* The road a composer speaks down. A pane's composer pastes into the pty and
 * reads the hook's word for its model and mode; a wire's composer talks to
 * the backend session (`wire_*`) and reads the wire's own state (`wire_log`,
 * kept on the run). The chips, the palette and the send ask the road and
 * never the run — one composer, two roads, no branches in the painters. */
function composerRoad(run, spec = null) {
  if (run.wire) {
    const log = () => run.wireLog ?? {};
    return {
      model: () => log().model ?? "",
      models: async () => {
        if (!run.wireModels) {
          try {
            run.wireModels = (await invoke("wire_models", { id: run.wire })) ?? [];
          } catch {
            run.wireModels = [];
          }
        }
        // A wire that names no models of its own (Claude Code's) lists the
        // catalog's for its agent, as a pane of that agent does.
        return run.wireModels.length ? run.wireModels : agentModelsFor(run.agent);
      },
      setModel: (id) => invoke("wire_set_model", { id: run.wire, model: id }),
      mode: () => log().mode ?? "",
      canChangeMode: () => (log().modes?.length ?? 0) > 1,
      cycleMode: () => {
        const modes = log().modes ?? [];
        const at = modes.findIndex((mode) => mode.id === log().mode);
        const next = modes[(at + 1) % modes.length];
        return next ? invoke("wire_set_mode", { id: run.wire, mode: next.id }) : Promise.resolve();
      },
      // The commands the wire itself announced (ACP's available commands,
      // Claude Code's `slash_commands`); the window's own ride along in the
      // palette as everywhere. A wire that announces names alone borrows
      // the words the window's catalog has for them.
      catalog: async (cwd, refresh) => {
        const announced = log().commands ?? [];
        const worded = announced.some((command) => !command.about)
          ? await slashCatalogFor(run.agent, cwd ?? run.cwd, refresh)
          : null;
        const known = new Map((worded?.commands ?? []).map((command) => [command.name, command]));
        return {
          agent: run.agent,
          source: "session",
          version: log().version ?? null,
          commands: announced.map((command) => {
            if (command.about) return command;
            const word = known.get(command.name);
            return word ? { ...word, ...command, about: word.about } : command;
          }),
        };
      },
      send: (text) => invoke("wire_send", { id: run.wire, text }),
      // 이 세션이 마지막으로 말한 컨텍스트(`wire_log`의 `usage`).
      usage: () => run.wireLog?.usage ?? null,
    };
  }
  return {
    model: () => paneModels.get(run.term) ?? "",
    models: () => agentModelsFor(spec.id),
    setModel: (id) => switchPaneModel(run, spec, id),
    mode: () => panePermissionModes.get(run.term) ?? "",
    canChangeMode: () => Boolean(spec?.permission_road),
    cycleMode: () => cyclePanePermission(run, spec),
    catalog: (cwd, refresh) => slashCatalogFor(spec.id, cwd, refresh),
    send: (text) => deliverToPane(run.term, text),
    // 판에게는 출처가 둘이다. 제 전사가 말한 것이 먼저고(모든 CLI가
    // 남긴다), 그것이 없으면 zo 채널이 세션에 대해 말한 것. 셋째는 없다 —
    // 훅은 토큰을 나르지 않는다.
    usage: () => paneUsage.get(run.term)
      ?? sessionUsage.get(paneSessions.get(run.term)?.session?.id)
      ?? null,
  };
}

/* ---- 실행 중에 친 글: 창이 들고 있다가 턴 끝에 보낸다 ---------------------
 *
 * 확장의 입력 자리말 「Queue another message…」와 그 `heldPrompts`: 턴
 * 한가운데서 친 글은 창 안에 서 있다가 턴이 끝나면 나간다. 지금까지 이
 * 입력줄은 실행 중에도 즉시 보냈고, 그 글이 어디에 떨어지는지는 길마다
 * 달랐다 — 판의 TUI는 제 대기열에 넣고(그 대기열은 이 창이 그릴 수도 지울
 * 수도 없다), 선은 턴 한가운데에 user 메시지를 끼워 넣는다. 창이 들고
 * 있으면 모든 에이전트가 같은 그림을 얻는다: 보이는 항목, 사람이 정한
 * 순서, 지우는 ×. 새로고침에 사라지던 글(확장 changelog 2.1.268/272)도
 * 여기서는 실행이 살아 있는 동안 산다. */

/* 이 실행이 지금 턴을 돌리고 있는가 — 보내기가 중지로 바뀌는 그 판정
 * 하나다(`paintWorkerComposerState`가 이 함수를 부른다; 식이 두 벌 있으면
 * 언젠가 둘이 갈라진다). 판은 훅이 말한 낱말, 선은 제 상태. */
function composerWorking(run) {
  // A wire's page has no pane: its own status is the turn. A page that speaks
  // to a PANE — the pane's own conversation, or a helper's page whose words
  // go to the parent — reads that pane's hook word and nothing else: a
  // helper's `running` is the helper's, and gating the parent's composer on
  // it queued words to a parent that was sitting idle (2026-09-21, the
  // t-110 pins and the attach suite caught it on the merge).
  if (run.wire) return run.status === "running";
  const term = run.term;
  return term !== undefined && term !== null
    && (hookStates.get(term) === "working" || isMidTurn(hookStates.get(term)));
}

/* 글 하나가 실행에 닿는 길 — 선이면 한 요청, 판이면 붙여넣기·숨·Enter.
 * 제출과 대기열이 **같은 이 문**을 지나므로, 대기열에서 나간 글은 사람이
 * 직접 친 글과 한 글자도 다르지 않게 도착한다. */
function composerDeliver(run, message, landed = null) {
  return run.wire ? composerRoad(run).send(message) : deliverToPane(run.term, message, landed);
}

/* 대기열에 한 글. 상한이 없다 — 이것은 사람이 친 글이고, 어느 것을 버릴지
 * 창이 고를 수 없다. 실행 하나의 수명과 같이 살고 그 실행이 사라질 때
 * (탭 drop, `forgetPaneChat`) 같이 사라진다. */
function queueComposerMessage(run, message) {
  if (!run.queue) run.queue = [];
  run.queue.push(message);
}

/* 대기 중인 글의 한 줄 — 첫 줄(빈 줄은 건너뛴다)만. 길이는 CSS가 자른다
 * (`text-overflow`), 여기서 자르는 것은 줄바꿈뿐이다. */
function composerQueueWords(message) {
  return (String(message).split("\n").find((line) => line.trim() !== "") ?? "").trim();
}

/* 대기열 줄 하나 — 상자 위, 첨부 칩 줄과 같은 층위. 세우기만 한다. */
function composerQueueNode() {
  const queue = document.createElement("div");
  queue.className = "composer-queue";
  queue.hidden = true;
  const count = document.createElement("span");
  count.className = "composer-queue-count";
  queue.appendChild(count);
  return queue;
}

/* 항목 하나 — 글 한 줄과 그것을 빼는 ×. 지우기는 대기열을 고치고 이 실행의
 * 입력줄 전부를 다시 그린다(같은 실행을 두 자리에서 보고 있을 수 있다). */
function composerQueueItemNode(run, message, at) {
  const item = document.createElement("div");
  item.className = "composer-queue-item";
  const words = document.createElement("span");
  words.className = "composer-queue-words";
  words.textContent = composerQueueWords(message);
  const drop = document.createElement("button");
  drop.type = "button";
  drop.className = "composer-queue-drop";
  labelButton(drop, t("composer.queue.drop", "대기열에서 지우기"));
  drop.appendChild(iconNode("x"));
  drop.addEventListener("click", () => {
    run.queue?.splice(at, 1);
    syncWorkerComposers(run);
  });
  item.append(words, drop);
  return item;
}

/* 대기열의 그림. 항목들의 서명이 그대로면 DOM을 건드리지 않는다 — 조용한
 * 폴은 mutation 0이고, 다시 세우는 값은 항목이 실제로 바뀐 폴에서만 치른다.
 * 그리기만 한다: 보내는 문은 `settleComposerQueue`다(SRP). */
function paintComposerQueue(form, run) {
  const queue = form.querySelector(".composer-queue");
  if (!queue) return;
  const held = run.queue ?? [];
  writeHidden(queue, held.length === 0);
  const sign = held.map(composerQueueWords).join("\u0000");
  if (queue.__queueSign === sign) return;
  queue.__queueSign = sign;
  const count = queue.querySelector(".composer-queue-count");
  writeTextContent(count, t("composer.queue.count", "대기 {{n}}", { n: held.length }));
  queue.replaceChildren(count, ...held.map((message, at) => composerQueueItemNode(run, message, at)));
}

/* 대기열의 첫 항목을 내보내는 문 — 상태가 움직이는 자리에서만 불린다.
 *
 * 한 턴에 하나. 보낸 직후에는 훅도 선도 아직 「작업 중」이라 말하지 않아서,
 * 그 짧은 창에 이 문이 다시 불리면 대기열이 통째로 쏟아진다. 그래서 보낼
 * 때 빗장(`run.queueArmed`)을 내리고, 상태가 실제로 working으로 오른 것을
 * 본 뒤에만 다시 올린다 — 시계가 아니라 상태가 기준이다. */
function settleComposerQueue(run) {
  if (composerWorking(run)) {
    run.queueArmed = true;
    return;
  }
  if (run.queueArmed === false) return;
  if (!run.queue?.length || run.sending || run.sendUncertain) return;
  const message = run.queue.shift();
  run.queueArmed = false;
  run.sending = true;
  syncWorkerComposers(run);
  let pasted = false;
  void composerDeliver(run, message, () => { pasted = true; })
    .catch((error) => {
      if (pasted) run.sendUncertain = true;
      showError(error);
    })
    .finally(() => {
      run.sending = false;
      syncWorkerComposers(run);
    });
}

/* 그 판에 말하는 실행들의 대기열 — 판의 상태가 움직인 자리(`hook:agent`)에서
 * 한 번씩. 그림은 `paintComposerChipsFor`의 몫이고 이 문은 보내기만 한다. */
function settleComposerQueuesFor(term) {
  const settled = new Set();
  for (const form of document.querySelectorAll(".worker-composer")) {
    const run = form.__workerRun;
    if (run?.term !== term || settled.has(run)) continue;
    settled.add(run);
    settleComposerQueue(run);
  }
}

/* ---- the agent·model chip ------------------------------------------------ */

/* `✻ Claude · Fable 5.1 ▾` — the catalog's mark and name, the pane's model
 * (the hook's word, shown as the model catalog names it when it can), and the
 * chevron that says it opens. */
function composerAgentChip(run, spec) {
  const chip = document.createElement("button");
  chip.type = "button";
  chip.className = "worker-composer-pill worker-composer-agent";
  chip.setAttribute("aria-haspopup", "menu");
  chip.setAttribute("aria-expanded", "false");
  chip.dataset.keyboardOwner = "true";
  const mark = agentMarkNode("worker-composer-mark", agentVoice(spec.id).glyph);
  const words = pillWordsNode(spec.name);
  const model = document.createElement("span");
  model.className = "worker-composer-model-words";
  const chevron = document.createElement("span");
  chevron.className = "worker-composer-chevron";
  chevron.textContent = "▾";
  chip.append(mark, words, model, chevron);
  chip.dataset.tip = t("composer.agentChip", "모델·CLI 고르기");
  chip.setAttribute("aria-label", chip.dataset.tip);
  chip.addEventListener("click", () => {
    if (chip.getAttribute("aria-expanded") === "true") closeComposerMenu(chip);
    else void openComposerMenu(chip, run, spec);
  });
  return chip;
}

/* The words the chip shows for the pane's model: the catalog's display name
 * when the model list knows the id, else the id as the hook said it. */
function paintComposerAgentChip(chip, run, models = null) {
  const said = composerRoad(run).model();
  const known = models?.find((row) => row.id === said);
  const words = known ? known.display_name : said;
  writeTextContent(chip.querySelector(".worker-composer-model-words"), words ? ` · ${words}` : "");
}

const agentModelLists = new Map();

/* The models the agent can switch to, from the window's catalog road
 * (`agent_models`), asked once a minute per agent at most. */
async function agentModelsFor(agent) {
  const held = agentModelLists.get(agent);
  if (held && Date.now() - held.at < 60_000) return held.rows;
  let rows = [];
  try {
    rows = (await invoke("agent_models", { agent })) ?? [];
  } catch {
    rows = [];
  }
  agentModelLists.set(agent, { at: Date.now(), rows });
  return rows;
}

let openMenu = null;

/* The air a standing menu keeps: from the chip it hangs on, and from the
 * window's own edges. */
const MENU_CHIP_GAP = 4;
const MENU_EDGE_KEEP = 8;

/* Stand the `/` palette over the box it reads.
 *
 * Unlike the chip's menu this one spans the composer, so only its vertical
 * side is ever in question: above the box where there is room, below it where
 * there is not. `bottom: 100%` alone put it 131px off the top of the window in
 * a pane pinned to the top edge — a split pane's share of the screen, which is
 * the ordinary case rather than a corner one. Absolute inside the form, so the
 * measured window numbers are turned back into the form's own. */
function standComposerSlash(pane, anchor) {
  pane.style.maxHeight = "";
  pane.style.top = "";
  pane.style.bottom = "";
  const cap = parseFloat(getComputedStyle(pane).maxHeight) || Infinity;
  const wants = pane.getBoundingClientRect().height;
  const box = anchor.getBoundingClientRect();
  const above = box.top - MENU_CHIP_GAP - MENU_EDGE_KEEP;
  const below = window.innerHeight - box.bottom - MENU_CHIP_GAP - MENU_EDGE_KEEP;
  const overhead = above >= wants || above >= below;
  pane.style.maxHeight = `${Math.min(cap, Math.max(0, overhead ? above : below))}px`;
  const tall = pane.getBoundingClientRect().height;
  const home = pane.offsetParent?.getBoundingClientRect().top ?? 0;
  // Both anchors set would stretch a box already held to `left: 0; right: 0`.
  pane.style.bottom = "auto";
  pane.style.top = `${(overhead ? box.top - MENU_CHIP_GAP - tall : box.bottom + MENU_CHIP_GAP) - home}px`;
}

/* Stand the open menu on the chip that opened it.
 *
 * The menu is `fixed`, so every number here is the window's own, and the
 * chip's box is read fresh on each call — that is what makes the menu follow
 * when the window is resized or the composer grows under it. Its home is
 * above the chip, because the composer sits at the foot of the conversation;
 * it moves under the chip only when its home cannot hold it, and it is pulled
 * back inside the window when the chip stands too near an edge to hold the
 * whole list. Before this, the menu hung off the composer's own far corner:
 * `right: 0` against the sticky form put it at the top right while the chip
 * was at the bottom left, and `bottom: 100%` cleared the whole box. */
function standComposerMenu(menu, chip) {
  // Measured at its own size, under the painter's cap, before being placed.
  menu.style.maxHeight = "";
  const cap = parseFloat(getComputedStyle(menu).maxHeight) || Infinity;
  const wants = menu.getBoundingClientRect().height;
  const box = chip.getBoundingClientRect();
  const above = box.top - MENU_CHIP_GAP - MENU_EDGE_KEEP;
  const below = window.innerHeight - box.bottom - MENU_CHIP_GAP - MENU_EDGE_KEEP;
  // Home unless home is both too small and the smaller side.
  const overhead = above >= wants || above >= below;
  const room = Math.max(0, overhead ? above : below);
  menu.style.maxHeight = `${Math.min(cap, room)}px`;
  const tall = menu.getBoundingClientRect().height;
  menu.style.top = `${overhead ? box.top - MENU_CHIP_GAP - tall : box.bottom + MENU_CHIP_GAP}px`;
  const wide = menu.getBoundingClientRect().width;
  const rightmost = window.innerWidth - wide - MENU_EDGE_KEEP;
  menu.style.left = `${Math.max(MENU_EDGE_KEEP, Math.min(box.left, rightmost))}px`;
}

function closeComposerMenu(chip = null) {
  if (!openMenu) return;
  if (chip && openMenu.chip !== chip) return;
  openMenu.node.remove();
  openMenu.chip.setAttribute("aria-expanded", "false");
  document.removeEventListener("pointerdown", openMenu.away, true);
  document.removeEventListener("keydown", openMenu.keys, true);
  window.removeEventListener("resize", openMenu.stand);
  document.removeEventListener("scroll", openMenu.stand, true);
  openMenu.watch.disconnect();
  openMenu = null;
}

/* ---- the bones every composer menu wears -------------------------------- */

/* The panel, its heads and its rows — written once here so the model menu and
 * the agents pill's menu cannot drift into two different-looking lists. */
function composerMenuNode() {
  const menu = document.createElement("div");
  menu.className = "composer-menu";
  menu.setAttribute("role", "menu");
  return menu;
}

function composerMenuSection(menu, words) {
  const head = document.createElement("div");
  head.className = "composer-menu-head";
  head.textContent = words;
  menu.appendChild(head);
  return head;
}

function composerMenuItem(menu, words, sub, on, { current = false } = {}) {
  const option = document.createElement("button");
  option.type = "button";
  option.className = "composer-menu-item";
  option.setAttribute("role", "menuitem");
  if (current) option.setAttribute("aria-current", "true");
  const name = document.createElement("span");
  name.className = "composer-menu-name";
  name.textContent = words;
  option.appendChild(name);
  if (sub) {
    const dim = document.createElement("span");
    dim.className = "composer-menu-sub";
    dim.textContent = sub;
    option.appendChild(dim);
  }
  option.addEventListener("click", () => {
    closeComposerMenu();
    on();
  });
  menu.appendChild(option);
  return option;
}

/* Stand a built menu on the chip that opened it and keep it there: placed,
 * closed by a press outside or by Escape, and placed again whenever the
 * window or the composer moves under it. Every composer menu opens here. */
function standOpenComposerMenu(chip, menu) {
  chip.parentElement.appendChild(menu);
  standComposerMenu(menu, chip);
  chip.setAttribute("aria-expanded", "true");
  const away = (event) => {
    if (!menu.contains(event.target) && event.target !== chip) closeComposerMenu();
  };
  const keys = (event) => {
    if (event.key === "Escape") {
      event.stopPropagation();
      closeComposerMenu();
      chip.focus();
    }
  };
  const stand = () => standComposerMenu(menu, chip);
  document.addEventListener("pointerdown", away, true);
  document.addEventListener("keydown", keys, true);
  window.addEventListener("resize", stand);
  // Anything scrolling under the menu carries the chip with it, and the
  // composer grows as the draft wraps — both move what the menu stands on.
  document.addEventListener("scroll", stand, true);
  const watch = new ResizeObserver(stand);
  watch.observe(chip.closest(".worker-composer") ?? chip);
  openMenu = { node: menu, chip, away, keys, stand, watch };
  menu.querySelector('[aria-current="true"], .composer-menu-item')?.focus();
}

/* The chip's menu: the models this CLI drives, the current one ticked, then
 * the other installed CLIs — each opening a fresh pane in the same checkout,
 * because a running pty runs one program. */
async function openComposerMenu(chip, run, spec) {
  closeComposerMenu();
  const menu = composerMenuNode();
  const road = composerRoad(run, spec);
  const models = await road.models();
  paintComposerAgentChip(chip, run, models);
  const current = road.model();
  const section = (words) => composerMenuSection(menu, words);
  const item = (words, sub, on, options = {}) => composerMenuItem(menu, words, sub, on, options);
  if (spec.model_command || run.wire) {
    section(t("composer.menu.models", "모델"));
    if (models.length === 0) {
      const none = document.createElement("p");
      none.className = "composer-menu-empty";
      none.textContent = t("composer.menu.noModels", "모델 목록을 읽지 못했습니다");
      menu.appendChild(none);
    }
    for (const row of models) {
      item(row.display_name, row.id, () => void road.setModel(row.id).catch((error) => showError(error)), {
        current: row.id === current,
      });
    }
  }
  const others = installedAgents().filter((row) => row.id !== spec.id);
  if (others.length) {
    section(t("composer.menu.otherClis", "다른 CLI에서 열기"));
    for (const other of others) {
      item(other.name, agentVoice(other.id).glyph, () => void openPaneWith(run, other));
    }
  }
  // The CLIs this machine can drive without a screen — Codex `app-server`,
  // Gemini CLI `--acp` — each opening a wire session in the same checkout.
  const wired = installedAgents().filter((row) => row.wire);
  if (wired.length) {
    section(t("composer.menu.wire", "화면 없이 선으로 열기 (GUI 세션)"));
    for (const row of wired) {
      item(row.name, row.wire, () => void openWirePage(row.id, helperBase(run)));
    }
  }
  standOpenComposerMenu(chip, menu);
}

/* A pick walks the CLI's own road: `/model <id>` where the CLI takes the id,
 * the bare command — and the screen — where it only opens its picker. */
async function switchPaneModel(run, spec, id) {
  try {
    if (spec.model_command_takes_id) {
      await deliverToPane(run.term, `${spec.model_command} ${id}`);
    } else {
      showPaneScreen(run.term);
      await deliverToPane(run.term, spec.model_command);
    }
  } catch (error) {
    showError(error);
  }
}

/* The permission chip's road: the CLI's own key that cycles the mode, or the
 * slash command that opens its picker (and the screen the picker is on). */
async function cyclePanePermission(run, spec) {
  try {
    if (spec.permission_road === "shift-tab") {
      const press = { key: "Tab", ctrl: false, alt: false, shift: true };
      await invoke("term_key", { term: run.term, press });
    } else if (spec.permission_road) {
      showPaneScreen(run.term);
      await deliverToPane(run.term, spec.permission_road);
    }
  } catch (error) {
    showError(error);
  }
}

/* A fresh pane of another CLI, in the checkout this one runs in — the same
 * launch every other road takes (`launchAgentTab`). */
async function openPaneWith(run, other) {
  try {
    const owner = tabOfTerm(run.term);
    const term = await launchAgentTab({
      agent: other.id,
      prompt: "",
      ...spawnGrid({ worktree: owner?.worktree ?? run.cwd ?? undefined }),
    });
    mountTermTab(term, { agent: other.name });
  } catch (error) {
    showError(error);
  }
}

/* ---- the agents pill ----------------------------------------------------- */

/* `N agents` under the box (the extension's footer pill, 2.1.269): how many
 * helpers are running inside this pane right now, opening the list of them.
 * The roster is the backend's (`paneSubagents`, kept by `hook:subagent` and
 * cleared with the pane), so the pill counts and never remembers. */
function composerAgentsChip(run) {
  const chip = document.createElement("button");
  chip.type = "button";
  chip.className = "worker-composer-pill worker-composer-agents";
  chip.hidden = true;
  chip.setAttribute("aria-haspopup", "menu");
  chip.setAttribute("aria-expanded", "false");
  chip.dataset.keyboardOwner = "true";
  chip.append(iconNode("bot"), pillWordsNode(""));
  // The tip never changes, so it is said once rather than on every paint.
  labelButton(chip, t("composer.agentsTip", "하위 에이전트 보기"));
  chip.addEventListener("click", () => {
    if (chip.getAttribute("aria-expanded") === "true") closeComposerMenu(chip);
    else openComposerAgentsMenu(chip, run);
  });
  paintComposerAgentsChip(chip, run);
  return chip;
}

/* The helpers this page's pane is running. A wire session has no pane, so it
 * has no helpers to show — the roster is keyed by terminal. */
function composerSubagentsOf(run) {
  if (run.wire || run.term === undefined || run.term === null) return [];
  return paneSubagents.get(run.term) ?? [];
}

/* The window's word for one helper's state. The board's own words, so a row
 * in this menu and the same row in the sidebar never disagree. */
function subagentStateWords(state) {
  if (state === "done") return bucketWord("done");
  if (state === "failed") return t("worker.endedFailed", "실패");
  return bucketWord("working");
}

/* Shown only while helpers are out, and wearing the failure ink when one of
 * them failed — the tool row's own failed colour, not the halt this product
 * keeps for destructive doors. Every write is guarded. */
function paintComposerAgentsChip(chip, run) {
  const rows = composerSubagentsOf(run);
  const out = rows.filter((row) => row.state !== "done").length;
  writeHidden(chip, out === 0);
  writeClass(chip, "is-failed", rows.some((row) => row.state === "failed"));
  if (out === 0) return;
  writeTextContent(
    chip.querySelector(".worker-composer-pill-words"),
    t("composer.agents", "에이전트 {{n}}", { n: out }),
  );
}

/* Every helper of this pane, running ones and finished ones alike, each row
 * opening its own transcript page — the same door the sidebar's row opens
 * (`openHelperPage`), because a helper has no pane of its own. */
function openComposerAgentsMenu(chip, run) {
  closeComposerMenu();
  const rows = composerSubagentsOf(run);
  if (rows.length === 0) return;
  const menu = composerMenuNode();
  const owner = tabOfTerm(run.term);
  composerMenuSection(menu, t("composer.agentsTip", "하위 에이전트 보기"));
  for (const row of rows) {
    composerMenuItem(menu, row.name, subagentStateWords(row.state), () =>
      void openHelperPage(
        { term: run.term, agent: run.agent, worktree: owner?.worktree, tab: owner },
        row,
      ));
  }
  standOpenComposerMenu(chip, menu);
}

/* ---- the permission-mode chip ------------------------------------------- */

function composerModeChip(run, spec) {
  const chip = document.createElement("button");
  chip.type = "button";
  chip.className = "worker-composer-pill worker-composer-mode";
  chip.hidden = true;
  chip.appendChild(iconNode("shield"));
  chip.appendChild(pillWordsNode(""));
  chip.addEventListener("click", () => {
    composerRoad(run, spec).cycleMode().catch((error) => showError(error));
  });
  paintComposerModeChip(chip, run, spec);
  return chip;
}

/* Shown only while the pane has said its mode; the tip says what a press does. */
function paintComposerModeChip(chip, run, spec) {
  const road = composerRoad(run, spec);
  const mode = road.mode();
  const shown = Boolean(mode);
  writeHidden(chip, !shown);
  if (!shown) return;
  writeTextContent(chip.querySelector(".worker-composer-pill-words"), permissionModeWords(mode));
  const tip = run.wire
    ? road.canChangeMode()
      ? t("composer.mode.wireTip", "권한 모드 바꾸기 (선의 다음 모드)")
      : t("composer.mode.readOnlyTip", "이 CLI가 보고한 권한 모드")
    : spec.permission_road === "shift-tab"
      ? t("composer.mode.cycleTip", "권한 모드 바꾸기 (Shift+Tab)")
      : spec.permission_road
        ? t("composer.mode.pickTip", "권한 모드 고르기 ({{command}})", { command: spec.permission_road })
        : t("composer.mode.readOnlyTip", "이 CLI가 보고한 권한 모드");
  writeAttribute(chip, "data-tip", tip);
  writeAttribute(chip, "aria-label", tip);
  chip.disabled = !road.canChangeMode();
}

/* ---- 컨텍스트 미터와 압축 문 --------------------------------------------
 *
 * X의 오래된 말(「상태 줄이 가장 저평가된 기능 — 모델·effort·정확한 컨텍스트
 * 사용량」)과 확장 푸터의 파이. 확장은 창 크기에서 제 출력 상한과 제 상수를
 * 빼고 나서 그리는데, 그 둘은 확장의 산수와 확장의 리터럴이라 옮기지 않는다.
 * 여기서는 CLI가 준 두 수만 쓴다 — 쓴 만큼과 담을 수 있는 만큼.
 *
 * 그리고 그 알약이 곧 압축 단추다. 채워 가는 고리를 보여 주면서 비울 길을
 * 주지 않는 것은 계기판만 있고 손잡이가 없는 것과 같다. */

/* 고리의 둘레를 100으로 잡는다 — 백분율이 그대로 `stroke-dasharray`의
 * 길이가 되어, 비율을 그리는 데 제2의 산수가 없다. r = 100 / 2π. */
const METER_CIRCUMFERENCE = 100;
const METER_RADIUS = 15.915_5;

/* 트랙과 채움, 원 둘. 세우기만 한다. */
function composerMeterNode() {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("class", "worker-composer-meter");
  svg.setAttribute("viewBox", "0 0 32 32");
  svg.setAttribute("aria-hidden", "true");
  for (const role of ["is-track", "is-fill"]) {
    const ring = document.createElementNS("http://www.w3.org/2000/svg", "circle");
    ring.setAttribute("class", role);
    ring.setAttribute("cx", "16");
    ring.setAttribute("cy", "16");
    ring.setAttribute("r", String(METER_RADIUS));
    svg.appendChild(ring);
  }
  return svg;
}

/* 이 실행에서 압축을 부를 수 있는가 — 카탈로그가 그 명령을 알고, 이 길이
 * 그것을 나를 수 있을 때의 그 명령, 아니면 `null`.
 *
 * 판은 언제나 나를 수 있다: 사람이 화면에 직접 치는 것과 같은 길이다. 선은
 * 세션이 제 명령 목록에 그 이름을 올렸을 때만 — 올리지 않은 세션에 슬래시
 * 한 줄을 보내면 그것은 명령이 아니라 모델에게 하는 말이 되고, 사람은
 * 컨텍스트가 줄기를 기다리며 줄지 않는 고리를 본다. */
function composerCompactCommand(run, spec) {
  const command = spec?.compact_command ?? null;
  if (!command) return null;
  if (!run.wire) return command;
  return (run.wireLog?.commands ?? []).some((row) => row.name === command) ? command : null;
}

/* 창 크기를 말로 — 모르면 「?」. 지어낸 분모는 툴팁에서도 쓰지 않는다. */
function composerContextWindowWords(reading) {
  return reading.window > 0
    ? reading.window.toLocaleString()
    : t("composer.context.unknownWindow", "?");
}

function composerContextChip(run, spec) {
  const chip = document.createElement("button");
  chip.type = "button";
  chip.className = "worker-composer-pill worker-composer-context";
  chip.hidden = true;
  chip.append(composerMeterNode(), pillWordsNode(""));
  labelButton(chip, t("composer.context.tipPlain", "컨텍스트 {{tokens}} / {{window}}", {
    tokens: "0",
    window: t("composer.context.unknownWindow", "?"),
  }));
  chip.addEventListener("click", () => {
    const command = composerCompactCommand(run, spec);
    if (!command) return;
    void composerDeliver(run, command).catch((error) => showError(error));
  });
  paintComposerContextChip(chip, run, spec);
  return chip;
}

/* 읽은 값이 있을 때만 선다. 창을 아는 읽기는 고리와 백분율, 모르는 읽기는
 * 상태바와 **같은 낱말**(`usageCtxWord`)과 고리 없음 — 어휘를 두 벌 만들지
 * 않는다. 쓰기는 전부 값이 바뀔 때만(조용한 폴은 mutation 0). */
function paintComposerContextChip(chip, run, spec) {
  const reading = composerRoad(run, spec).usage();
  const shown = Boolean(reading) && reading.tokens > 0;
  writeHidden(chip, !shown);
  if (!shown) return;
  const ratio = reading.window > 0 ? Math.min(1, reading.tokens / reading.window) : 0;
  const meter = chip.querySelector(".worker-composer-meter");
  writeHidden(meter, reading.window <= 0);
  if (reading.window > 0) {
    writeAttribute(
      meter.querySelector(".is-fill"),
      "stroke-dasharray",
      `${(ratio * METER_CIRCUMFERENCE).toFixed(1)} ${METER_CIRCUMFERENCE}`,
    );
  }
  writeTextContent(
    chip.querySelector(".worker-composer-pill-words"),
    reading.window > 0 ? `${Math.round(ratio * 100)}%` : usageCtxWord(reading),
  );
  const command = composerCompactCommand(run, spec);
  const words = { tokens: reading.tokens.toLocaleString(), window: composerContextWindowWords(reading) };
  const tip = command
    ? t("composer.context.tip", "컨텍스트 {{tokens}} / {{window}} — 눌러서 압축 ({{command}})", { ...words, command })
    : t("composer.context.tipPlain", "컨텍스트 {{tokens}} / {{window}}", words);
  writeAttribute(chip, "data-tip", tip);
  writeAttribute(chip, "aria-label", tip);
  chip.disabled = command === null;
}

/* ---- the `/` palette ----------------------------------------------------- */

const slashCatalogs = new Map();

/* The commands `agent` takes in `cwd`, asked of the window's catalog road and
 * held for five minutes; the window's own commands ride along. */
async function slashCatalogFor(agent, cwd, refresh = false) {
  const key = `${agent}
${cwd}`;
  const held = slashCatalogs.get(key);
  if (!refresh && held && Date.now() - held.at < SLASH_CATALOG_TTL_MS) return held.catalog;
  let catalog = { agent, source: "none", version: null, commands: [] };
  try {
    catalog = (await invoke("slash_commands", { agent, cwd, refresh })) ?? catalog;
  } catch {
    // The road failing is not a reason for a blank palette: the window's own
    // commands still stand below.
  }
  slashCatalogs.set(key, { at: Date.now(), catalog });
  return catalog;
}

/* The commands the WINDOW answers itself, before the words reach the CLI.
 *
 * `run` is the page the palette hangs on: a command whose door is shut on
 * that page is not listed at all, because a row that does nothing when it is
 * pressed is the dead control this window does not draw. */
function windowSlashCommands(run = null) {
  const said = lastAnswerOf(run);
  const commands = [
    {
      name: "/artifact",
      about: t("composer.slash.artifact", "HTML 아티팩트 초안을 입력줄에 채운다"),
      args: "",
      kind: "window",
      // The draft goes under whatever was already written — the words a
      // person typed before reaching for the command are the artifact's brief.
      fill: (rest) => {
        const draft = t(
          "artifacts.newDraft",
          "HTML 한 페이지 아티팩트를 만들어 zerocode-artifact publish --file-path <절대 경로.html>로 이 창의 갤러리에 발행하고 id와 버전을 알려 줘. 만들 것: ",
        );
        return rest.trim() ? `${rest.trimEnd()}\n\n${draft}` : draft;
      },
    },
  ];
  if (said) {
    commands.push({
      name: "/copy",
      about: t("composer.slash.copy", "마지막 답을 클립보드에 복사한다"),
      args: "",
      kind: "window",
      // Acts here and now: no words go to the CLI, so the box is emptied and
      // the palette closes without a submit.
      act: (page) => copyLastAnswer(page),
    });
  }
  return commands;
}

/* Claude Code's own matching rule (docs/en/commands → "How the command menu
 * matches"): the letters after `/` match a command from the start of its
 * name or an alias, or from the start of a word within it, ignoring the `:`,
 * `_` and `-` separators. */
function slashMatches(command, typed) {
  if (typed === "") return true;
  const needle = typed.toLowerCase();
  const names = [command.name, ...(command.aliases ?? [])].map((name) => name.replace(/^\//, "").toLowerCase());
  return names.some((name) =>
    name.startsWith(needle) || name.split(/[:_-]/).some((word) => word.startsWith(needle)));
}

/* What the source word on the palette's head is. */
function slashSourceWords(catalog) {
  const words = {
    session: t("composer.slash.sourceSession", "이 세션"),
    binary: t("composer.slash.sourceBinary", "설치본"),
    docs: t("composer.slash.sourceDocs", "문서 기준"),
    none: t("composer.slash.sourceNone", "목록 없음"),
  };
  return words[catalog.source] ?? catalog.source;
}

/* Attach the palette to one composer: `form` holds it, `box` is the textarea
 * it reads, `run` names the pane and its agent. Returns the `/` button for the
 * tool row. */
function composerSlash(form, box, run, spec, cwd) {
  const pane = document.createElement("div");
  pane.className = "composer-slash";
  pane.hidden = true;
  pane.setAttribute("role", "listbox");
  pane.setAttribute("aria-label", t("composer.slash.list", "슬래시 명령"));
  const head = document.createElement("div");
  head.className = "composer-slash-head";
  const list = document.createElement("div");
  list.className = "composer-slash-rows";
  pane.append(head, list);
  form.prepend(pane);
  let catalog = null;
  let rows = [];
  let hot = -1;
  // The `/` button opens the palette over whatever the box holds; typing
  // opens it only while the box is one bare `/word`.
  let forced = false;
  const typedNow = () => {
    const text = box.value;
    const m = /^\/(\S*)$/.exec(text);
    return m ? m[1] : null;
  };
  const close = () => {
    if (!pane.hidden) pane.hidden = true;
    hot = -1;
    forced = false;
  };
  const paint = () => {
    const typed = typedNow() ?? (forced ? "" : null);
    if (typed === null || !catalog) {
      close();
      return;
    }
    const all = [...catalog.commands, ...windowSlashCommands(run)];
    rows = all.filter((command) => slashMatches(command, typed));
    // The head: the CLI, its installed version, and where the list came
    // from — with the page's version beside a documented list that was read
    // against another release.
    const version = catalog.version ? ` ${catalog.version}` : "";
    const behind = catalog.snapshot_version ? ` (${catalog.snapshot_version})` : "";
    writeTextContent(head, `${spec.name}${version} · ${slashSourceWords(catalog)}${behind}`);
    list.replaceChildren();
    if (rows.length === 0) {
      const none = document.createElement("p");
      none.className = "composer-slash-none";
      none.textContent = t("composer.slash.none", "「/{{typed}}」에 맞는 명령이 없습니다", { typed });
      list.appendChild(none);
      hot = -1;
    } else {
      // The top row is highlighted only when the letters matched — a typo
      // leaves the close matches listed and nothing chosen (Claude Code's rule).
      hot = typed === "" || rows.some((row) => slashMatches(row, typed)) ? 0 : -1;
      rows.forEach((command, index) => {
        const option = document.createElement("button");
        option.type = "button";
        option.className = "composer-slash-row";
        option.setAttribute("role", "option");
        option.dataset.kind = command.kind;
        if (index === hot) option.classList.add("is-hot");
        const name = document.createElement("span");
        name.className = "composer-slash-name";
        name.textContent = command.name;
        option.appendChild(name);
        if (command.args) {
          const args = document.createElement("span");
          args.className = "composer-slash-args";
          args.textContent = command.args;
          option.appendChild(args);
        }
        const about = document.createElement("span");
        about.className = "composer-slash-about";
        about.textContent = command.about;
        option.appendChild(about);
        option.addEventListener("pointerdown", (event) => event.preventDefault());
        option.addEventListener("click", () => run_(command));
        list.appendChild(option);
      });
    }
    pane.hidden = false;
    standComposerSlash(pane, box);
  };
  const moveHot = (step) => {
    if (rows.length === 0) return;
    hot = (hot + step + rows.length) % rows.length;
    [...list.children].forEach((node, index) => node.classList.toggle("is-hot", index === hot));
    list.children[hot]?.scrollIntoView({ block: "nearest" });
  };
  // The words already in the box, less the `/word` being typed, ride along:
  // as the command's argument (`/compact 이 부분만`), or under a window
  // command's own draft.
  const writeBox = (value) => {
    box.value = value;
    run.draft = box.value;
    box.dispatchEvent(new Event("input", { bubbles: true }));
    box.focus();
    box.setSelectionRange(box.value.length, box.value.length);
  };
  const fill = (command) => {
    const rest = box.value.replace(/^\/\S*\s?/, "");
    writeBox(command.fill ? command.fill(rest) : `${command.name} ${rest}`);
  };
  // Enter on a command runs it — the CLI's own behaviour — unless it wants an
  // argument, when the words are filled and the person finishes them. A
  // window command only fills.
  const run_ = (command) => {
    // A window command that ACTS is answered here: the words that summoned
    // it leave the box and nothing is sent anywhere (`/copy`).
    if (command.act) {
      writeBox("");
      close();
      command.act(run);
      return;
    }
    fill(command);
    close();
    if (!command.fill && !command.args) form.requestSubmit();
  };
  const open = async (refresh = false) => {
    catalog = await composerRoad(run, spec).catalog(cwd, refresh);
    paint();
  };
  box.addEventListener("input", () => {
    forced = false;
    if (typedNow() === null) {
      close();
      return;
    }
    if (catalog) paint();
    else void open();
  });
  box.addEventListener("keydown", (event) => {
    if (pane.hidden) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      moveHot(event.key === "ArrowDown" ? 1 : -1);
    } else if (event.key === "Tab" && hot >= 0) {
      event.preventDefault();
      fill(rows[hot]);
      close();
    } else if (event.key === "Enter" && !event.shiftKey && hot >= 0) {
      event.preventDefault();
      event.stopImmediatePropagation();
      run_(rows[hot]);
    } else if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      close();
    }
  }, true);
  box.addEventListener("blur", () => setTimeout(close, 120));
  const slash = document.createElement("button");
  slash.type = "button";
  slash.className = "worker-composer-slash";
  slash.textContent = "/";
  slash.dataset.tip = t("composer.slash.button", "슬래시 명령");
  slash.setAttribute("aria-label", slash.dataset.tip);
  slash.addEventListener("click", () => {
    if (!pane.hidden) {
      close();
      return;
    }
    forced = true;
    box.focus();
    void open();
  });
  return { slash, refresh: () => open(true), close };
}
