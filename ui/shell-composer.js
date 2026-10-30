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
 * that simply failed.
 *
 * `road` names the chip whose command this is (`model`, `permission`), so the
 * window does not count it as the person's words (t-57712). Only the paste
 * carries it: the Enter after it adds no words of anyone's. */
async function deliverToPane(term, text, landed = null, road = null) {
  await invoke("term_paste", road ? { term, text, road } : { term, text });
  landed?.();
  await new Promise((done) => setTimeout(done, COMPOSER_SUBMIT_DELAY_MS));
  await invoke("term_key", { term, press: { key: "Enter", ctrl: false, alt: false } });
}

/* The screen a picker lives on: a pane wearing its conversation is turned back
 * to its terminal so the CLI's own dialog is in front of the person. */
function showPaneScreen(term) {
  if (typeof paneChatOn === "function" && paneChatOn(term)) setPaneChat(term, false);
}

/* The mode as a person reads it: the CLI's own word when the catalog has it
 * (Claude Code's panel says "Manual", "Edit automatically" — `AB0`, 2.1.280),
 * else the CLI's spelling opened up: `bypassPermissions` → "bypass
 * permissions", `workspace-write` → "workspace write". No table of
 * translations — the mode is the CLI's vocabulary. */
function permissionModeWords(mode, spec = null) {
  const said = permissionModeRow(spec, mode)?.label;
  if (said) return said;
  return String(mode)
    .replace(/([a-z])([A-Z])/g, "$1 $2")
    .replace(/[-_]+/g, " ")
    .toLowerCase();
}

/* The catalog's row for a mode, by its reported spelling or another the CLI
 * gives it (Claude Code 2.1.280's `--help` says `manual` for `default`). */
function permissionModeRow(spec, mode) {
  return (spec?.permission_modes ?? []).find((row) => row.mode === mode || row.aliases?.includes(mode)) ?? null;
}

/* The mode Shift+Tab — or the chip — steps to next. A CLI whose cycle the
 * catalog orders (its rows carry the CLI's words) steps in that order, the
 * extension's own (`d6`: a mode kept only while current drops out once left),
 * over the modes the session says it takes; any other steps through the
 * session's list as the session gave it. */
function nextPermissionMode(spec, current, offered) {
  const worded = (spec?.permission_modes ?? []).filter((row) => row.label);
  if (worded.length) {
    const takes = (row) => !offered.length || [row.mode, ...(row.aliases ?? [])].some((id) => offered.includes(id));
    const cycle = worded.filter((row) => (row.cycles || row.mode === current) && takes(row));
    if (!cycle.length) return null;
    const at = cycle.findIndex((row) => row.mode === current || row.aliases?.includes(current));
    return cycle[(at + 1) % cycle.length].mode;
  }
  if (!offered.length) return null;
  return offered[(offered.indexOf(current) + 1) % offered.length];
}

/* What stops a pane's turn: the key its CLI names on its own screen
 * (`interrupt_key`: Claude Code and zo say "esc to interrupt"), else the
 * terminal's own interrupt. */
function interruptPress(spec) {
  return spec?.interrupt_key
    ? { key: spec.interrupt_key, ctrl: false, alt: false }
    : { key: "c", ctrl: true, alt: false };
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
 * — a rule that colours names the reach it colours. */
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
    const voiced = () => spec ?? installedAgents().find((row) => row.id === run.agent) ?? null;
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
        const next = nextPermissionMode(voiced(), log().mode, (log().modes ?? []).map((mode) => mode.id));
        return next ? invoke("wire_set_mode", { id: run.wire, mode: next }) : Promise.resolve();
      },
      // The session's own interrupt request (`wire_interrupt`) — the stop
      // button and Esc both send it.
      interrupt: () => invoke("wire_interrupt", { id: run.wire }),
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
    interrupt: () => invoke("term_key", { term: run.term, press: interruptPress(spec) }),
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

/* How long the conversation being handed to its wire keeps the queue
 * (t-84670). The hand-over ends the hold itself when the wire stands or is
 * refused; this bounds the one that does neither (10-30, right after
 * v1.1.105's restart: a person's Enter went into the queue under an idle
 * head, again after ×, and nothing let it go). The backend's own bound is
 * longer — `wire_start` waits up to 90 s for each handshake request
 * (`HANDSHAKE_WAIT`, two on a protocol that opens a session), 10 s for each
 * of two probes and 8 s for the pane's exit — so a hand-over is not left to
 * it: the window gives it a deadline (`COMPOSER_HAND_OVER_DUE_MS`) and lets
 * the held words go to the pane at this bound. The other holds wait for their
 * own event and nothing else: a question's card stands where the person sees
 * it, and words let go under it would fall on its menu (t-81786 P0). */
const COMPOSER_HOLD_MS = 60_000;

/* By when, from the hand-over's start, its wire must have stood
 * (`wire_start`'s `deadline_ms`): well inside the hold, so the pane's exit
 * command is typed before the hold's clock could let the words go to the
 * pane. Once it is typed the window is told (`wire:exit-typed`) and the hold
 * has no clock; the backend's watch for the pane's leaving bounds it
 * (`HAND_OVER_LEAVE_LIMIT`, t-84850 r2). The backend stops a wire that stands
 * later, so none ever opens beside — or ends — a pane the held words were
 * just given to. */
const COMPOSER_HAND_OVER_DUE_MS = COMPOSER_HOLD_MS - 15_000;

/* Whether a hold that began at `since` has run past its bound. A hold with no
 * stamp is not one this bound knows. */
function composerHoldOver(since, now = Date.now()) {
  return Number.isFinite(since) && now - since >= COMPOSER_HOLD_MS;
}

/* Whether words sent now wait for a hand-over: the pane's conversation is
 * being handed to its wire (t-54304). The wire's page sends them when it
 * stands (`carryPaneChat`); a refused hand-over leaves them to the pane, and
 * one that neither stands nor is refused within its hold leaves them to the
 * pane too (t-84670). */
function composerHolding(run) {
  const held = run.helper?.id === PANE_LOG_ID ? paneChats.get(run.term) : null;
  // Once the exit command is typed the clock no longer lets them go: the
  // pane may yet read `/exit`, and the backend's own watch bounds the wait
  // (`HAND_OVER_LEAVE_LIMIT`, t-84850 r2).
  return held?.handing === true && (held.exitTyped === true || !composerHoldOver(held.handingSince));
}

/* Whether words sent now wait in the queue rather than go — the submit's test
 * and the send's face (`paintComposerSend`) read this one answer. `asking` is
 * the question standing as its card once its answer went (t-81786 r2 P2-1):
 * the words after it wait as words sent mid-turn do. A card still waiting for
 * its answer takes the words as that answer, and is asked before this. */
function composerQueues(run, asking) {
  return composerWorking(run) || composerHolding(run) || Boolean(asking);
}

/* The moment the hand-over now keeping `run`'s queue stops holding it, or
 * null when no hand-over holds it. */
function composerHoldEnds(run, now = Date.now()) {
  const held = run.helper?.id === PANE_LOG_ID ? paneChats.get(run.term) : null;
  const at = held?.handing === true && held.exitTyped !== true && Number.isFinite(held.handingSince)
    ? held.handingSince + COMPOSER_HOLD_MS
    : null;
  return at !== null && at > now ? at : null;
}

/* One timer per run for a queue a hand-over holds: when the hold runs out the
 * queue is asked again. Nothing else moves when a hand-over sticks — that is
 * the case this is for (t-84670). A queue no hand-over holds keeps no
 * timer. */
function armComposerRelease(run) {
  const at = run.queue?.length ? composerHoldEnds(run) : null;
  if (at !== null && run.queueRelease?.at === at) return;
  dropComposerRelease(run);
  if (at !== null) run.queueRelease = { at, timer: setTimeout(() => releaseComposerHold(run), at - Date.now() + 1) };
}

/* The run's release timer gone — its hold ended by its own event, or the
 * conversation it held for went. */
function dropComposerRelease(run) {
  clearTimeout(run.queueRelease?.timer);
  run.queueRelease = null;
}

function releaseComposerHold(run) {
  run.queueRelease = null;
  syncWorkerComposers(run);
  settleComposerQueue(run);
}

/* Why the queue is waiting, in one line under its count — the turn, a
 * hand-over (and by when at the latest), a question, or the turn of what it
 * sent last. */
function composerQueueWhy(run) {
  const latest = { s: Math.round(COMPOSER_HOLD_MS / 1000) };
  if (composerWorking(run)) return t("composer.queue.why.turn", "이 턴이 끝나면 하나씩 보냅니다");
  if (composerHolding(run)) {
    const held = paneChats.get(run.term);
    if (held?.exitTyped === true) {
      return t("composer.queue.why.exitTyped", "판의 CLI가 끝나기를 기다립니다 — 끝나면 대화로 보내고, 아니면 입력칸에 돌려 둡니다");
    }
    return t("composer.queue.why.handing", "대화를 실시간으로 잇는 중 — 이어지면, 늦어도 {{s}}초 뒤 보냅니다", latest);
  }
  const standing = standingAskOf(run);
  if (standing) {
    return standing.draft?.sending
      ? t("composer.queue.why.answered", "질문에 보낸 답이 닿기를 기다립니다 — 판이 넘어가면 보냅니다")
      : t("composer.queue.why.asking", "위 질문에 답하면 보냅니다");
  }
  if (run.queueArmed === false) return t("composer.queue.why.started", "앞서 보낸 글의 턴이 시작되기를 기다립니다");
  return t("composer.queue.why.going", "곧 보냅니다");
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
  const why = document.createElement("span");
  why.className = "composer-queue-why";
  why.setAttribute("role", "status");
  queue.append(count, why);
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
  // What it waits for moves without the items moving: written on its own,
  // guarded, so a quiet poll still writes nothing.
  const why = queue.querySelector(".composer-queue-why");
  if (why && held.length > 0) {
    const words = composerQueueWhy(run);
    writeTextContent(why, words);
    writeAttribute(why, "data-tip", words);
  }
  const sign = held.map(composerQueueWords).join("\u0000");
  if (queue.__queueSign === sign) return;
  queue.__queueSign = sign;
  const count = queue.querySelector(".composer-queue-count");
  writeTextContent(count, t("composer.queue.count", "대기 {{n}}", { n: held.length }));
  queue.replaceChildren(count, ...(why ? [why] : []), ...held.map((message, at) => composerQueueItemNode(run, message, at)));
}

/* 대기열의 첫 항목을 내보내는 문 — 상태가 움직이는 자리에서만 불린다.
 *
 * 한 턴에 하나. 보낸 직후에는 훅도 선도 아직 「작업 중」이라 말하지 않아서,
 * 그 짧은 창에 이 문이 다시 불리면 대기열이 통째로 쏟아진다. 그래서 보낼
 * 때 빗장(`run.queueArmed`)을 내리고, 상태가 실제로 working으로 오른 것을
 * 본 뒤에만 다시 올린다 — 시계가 아니라 상태가 기준이다. 시계는 판의 대화를
 * 선으로 넘기는 일이 끝내 끝나지 않을 때만 쓴다: 그 붙잡음은
 * `COMPOSER_HOLD_MS` 뒤 풀린다(`armComposerRelease`, t-84670). */
function settleComposerQueue(run) {
  // Held for a hand-over under way: they go with the conversation (t-54304).
  if (composerHolding(run)) {
    armComposerRelease(run);
    return;
  }
  if (composerWorking(run)) {
    run.queueArmed = true;
    return;
  }
  // Nor while a question stands as its card: pasted now, the words would fall
  // on the menu it shows (t-81786 P0). They go once the card is gone.
  if (standingAskOf(run)) return;
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

/* What waited for a hand-over that typed the pane's exit command and was
 * refused (t-84850 r2), back in the box: the queued words first, in their
 * order, then whatever was being written — one draft the person sends again.
 * Never to the pane, which may be reading `/exit` late or be a shell now. */
function returnComposerQueueToBox(run) {
  const held = run.queue?.splice(0) ?? [];
  dropComposerRelease(run);
  if (held.length > 0) {
    const pieces = [...held, run.draft ?? ""].filter((words) => words.trim() !== "");
    run.draft = pieces.join("\n\n");
    // How many messages this draft is — the person's own count, for a
    // notice that has to say them again (`sayComposerQueueLost`).
    run.returned = { draft: run.draft, count: pieces.length };
    for (const form of document.querySelectorAll(".worker-composer")) {
      if (form.__workerRun !== run) continue;
      form.querySelector(".worker-composer-box").value = run.draft;
      form.__workerFit?.();
    }
  }
  syncWorkerComposers(run);
}

/* What waited for a hand-over whose pane left as its wire was refused, or
 * left later with those words back in its box: no box and no screen are left
 * to hold them, so they are said — in a toast that stands until they are
 * copied, with the words under it — rather than dropped. */
function sayComposerQueueLost(run) {
  const draft = run.draft ?? "";
  const held = [...(run.queue?.splice(0) ?? []), draft].filter((words) => words.trim() !== "");
  // A draft the window made of returned messages counts as the messages the
  // person sent, while it still stands as returned; one they have written
  // since counts as one.
  const count = held.length + (draft.trim() !== "" && run.returned?.draft === draft ? run.returned.count - 1 : 0);
  run.draft = "";
  run.returned = null;
  dropComposerRelease(run);
  if (held.length === 0) return;
  const words = held.join("\n\n");
  const note = toast(
    t("composer.queue.lost", "판이 닫혀 기다리던 글 {{n}}개를 보내지 못했습니다", { n: count }),
    "halt",
    {
      sticky: true,
      tip: words,
      action: {
        label: t("composer.queue.copy", "글 복사"),
        run: () => void clipboardText.write(words).then((copied) => { if (copied) closing(note, () => note.remove()); }),
      },
    },
  );
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
 * chevron that says it opens. A helper's page has no composer, so no chip here
 * ever speaks for a helper: its model is in the page's head (`updateHelperPageModel`). */
function composerAgentChip(run, spec) {
  const chip = document.createElement("button");
  chip.className = "worker-composer-pill worker-composer-agent";
  const mark = agentMarkNode("worker-composer-mark", agentVoice(spec.id).glyph);
  const words = pillWordsNode(spec.name);
  const model = document.createElement("span");
  model.className = "worker-composer-model-words";
  chip.type = "button";
  chip.setAttribute("aria-haspopup", "menu");
  chip.setAttribute("aria-expanded", "false");
  chip.dataset.keyboardOwner = "true";
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

/* The words for a model id: the catalog's display name when the model list
 * knows the id, else the id as the vendor said it. One lookup for a pane's
 * chip and a helper's (`helperModelWords`), so one id is never worded two
 * ways. */
function catalogModelWords(said, models = null) {
  const known = models?.find((row) => row.id === said);
  return known ? known.display_name : said;
}

/* The model list the window holds for a run's agent: the catalog road's,
 * else what the wire said it can switch to. */
function runModelList(run) {
  return agentModelLists.get(run.agent)?.rows ?? run.wireModels ?? null;
}

/* The words for the model a run runs on — one answer for the composer's chip
 * and the page's head (t-22100), so one model is never worded two ways. */
function runModelWords(run, models = runModelList(run)) {
  return catalogModelWords(composerRoad(run).model(), models);
}

/* The words the chip shows for the pane's model: the catalog's display name
 * when the model list knows the id, else the id as the hook said it. */
function paintComposerAgentChip(chip, run, models = null) {
  const words = runModelWords(run, models);
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
  // The menu's rows take their own keys: without this the window's key sink
  // took the focus back on the keydown, before Enter or Space could press
  // the row it stood on (t-81532).
  menu.dataset.keyboardOwner = "true";
  return menu;
}

function composerMenuSection(menu, words) {
  const head = document.createElement("div");
  head.className = "composer-menu-head";
  head.textContent = words;
  menu.appendChild(head);
  return head;
}

function composerMenuItem(menu, words, sub, on, { current = false, disabled = false } = {}) {
  const option = document.createElement("button");
  option.type = "button";
  option.className = "composer-menu-item";
  option.setAttribute("role", "menuitem");
  if (current) option.setAttribute("aria-current", "true");
  // A row that cannot act now still reads, and keeps its focus for the
  // keyboard: `aria-disabled`, never `disabled`, which would drop it from
  // the tab order and leave the open menu with nothing to stand on.
  if (disabled) option.setAttribute("aria-disabled", "true");
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
    if (disabled) return;
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
  // The ticked row first — a selector list answers in document order, which
  // stood the focus on the first row whatever was ticked.
  (menu.querySelector('[aria-current="true"]') ?? menu.querySelector(".composer-menu-item"))?.focus();
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
      await deliverToPane(run.term, `${spec.model_command} ${id}`, null, "model");
    } else {
      showPaneScreen(run.term);
      await deliverToPane(run.term, spec.model_command, null, "model");
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
      await invoke("term_key", { term: run.term, press, road: "permission" });
    } else if (spec.permission_road) {
      showPaneScreen(run.term);
      await deliverToPane(run.term, spec.permission_road, null, "permission");
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

/* Whether this page is a HELPER's own — the one that reads the helper's
 * transcript — and not a pane's conversation, a wire session or a command
 * run. Those three have a model of their own to read from a pane or a wire;
 * a helper has neither, and its page rides its parent's terminal, so
 * anything read off `run.term` here is the PARENT's. */
function isHelperPage(run) {
  const id = run.helper?.id;
  return id !== undefined && id !== PANE_LOG_ID && id !== WIRE_LOG_ID && !run.wire;
}

/* The roster row a helper's page stands on: the live one while its pane still
 * lists the helper, else the last one the page saw (`run.sub`, kept by
 * `openHelperPage` and `syncHelperPagesWith`). A helper outlives its row when
 * the session turns, and its model does not change because the roster
 * forgot it. */
function composerHelperRow(run) {
  return composerSubagentsOf(run).find((one) => one.id === run.helper.id) ?? run.sub ?? null;
}

/* The model a helper ITSELF runs on, in the words the model catalog gives
 * it (the id as its vendor said it when the catalog does not know it), or
 * `""` when the vendor named none.
 *
 * The one function the helper page's pill, the sidebar's helper row and the
 * agents menu all ask (t-15625), so the three cannot word one model three
 * ways — and none of them can borrow the pane's, because the pane's model is
 * not on the row. `agent` is the vendor the helper runs under: the catalog it
 * is worded from. */
function helperModelWords(sub, agent) {
  const said = sub?.model;
  return said ? catalogModelWords(said, agentModelLists.get(agent)?.rows ?? null) : "";
}

/* The facts behind a helper's model words, for a tip: the model that ran
 * (and how hard it thinks, when the vendor said), and — when that is not the
 * model the helper was asked for (an alias the vendor resolved, a router that
 * moved it, a fallback that took over) — both. `""` when there is no model
 * to speak of. The ids are the vendor's own; nothing here knows a name. */
function helperModelTip(sub) {
  if (!sub?.model) return "";
  const ran = sub.effort
    ? t("board.desk.workerModel", "{{model}} · {{effort}}", { model: sub.model, effort: sub.effort })
    : sub.model;
  return sub.requestedModel && sub.requestedModel !== sub.model
    ? t("helper.model.moved", "요청 {{asked}} → 실행 {{ran}}", { asked: sub.requestedModel, ran })
    : ran;
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
    // The helper's own model beside its state, in the words its page and its
    // sidebar row use — the same function, so the three cannot disagree.
    const words = [helperModelWords(row, run.agent), subagentStateWords(row.state)]
      .filter(Boolean)
      .join(" · ");
    composerMenuItem(menu, row.name, words, () =>
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
  writeTextContent(chip.querySelector(".worker-composer-pill-words"), permissionModeWords(mode, spec));
  // A session that asks nothing keeps it in sight by a mark, not a colour.
  writeAttribute(chip.querySelector("use"), "href", composerReachOf(run) === "bypass" ? "#i-alert" : "#i-shield");
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

/* ---- the effort chip (t-81532) -----------------------------------------
 *
 * `🧠 high ▾` beside the model: the effort the pane's session last ran a
 * turn at, as its own record says (`pane_effort`; the summons' word before
 * any turn, 「기본」 for the CLI's default), and the rungs a press can move it
 * to. A press walks the road the step-effort seat drives — Claude Code's
 * `/effort` picker, an arrow a rung and `s` for this session only — between
 * turns only, and only after the person has read what it costs: on Anthropic
 * a changed effort rewrites the conversation's cache. A CLI whose effort no
 * typed key moves is shown, and its rungs say why they cannot be pressed.
 *
 * Asked at the turn's edges and on opening, never on a paint: a turn's dozens
 * of tool events repaint the chip from what was last asked. */

/* What the backend last said about each pane's effort, by terminal — cleared
 * with the pane, as the context reading beside it is. */
const paneEfforts = new Map();

/* Ask the backend what `term`'s effort is now; repaint its chips when the
 * answer moved. One question per pane at a time. */
async function refreshPaneEffort(term) {
  if (term === undefined || term === null) return null;
  const held = paneEfforts.get(term);
  if (held?.asking) return held.asking;
  const asking = (async () => {
    let said = null;
    try {
      said = await invoke("pane_effort", { term });
    } catch {
      said = null;
    }
    rememberPaneEffort(term, said);
    return said;
  })();
  paneEfforts.set(term, { ...(held ?? {}), asking });
  try {
    return await asking;
  } finally {
    const now = paneEfforts.get(term);
    if (now?.asking === asking) now.asking = null;
  }
}

/* Keep an answer and repaint the chips that read it, only when it moved. */
function rememberPaneEffort(term, said) {
  const held = paneEfforts.get(term) ?? {};
  const sign = JSON.stringify(said ?? null);
  paneEfforts.set(term, { ...held, said, sign });
  if (held.sign !== sign) paintComposerChipsFor(term);
}

/* Ask again for every pane whose composer wears the chip — the turn edge
 * (`hook:agent`, once the state has crossed) is where its word can move. */
function refreshComposerEffortFor(term) {
  for (const form of document.querySelectorAll(".worker-composer")) {
    if (form.__workerRun?.term === term && form.querySelector(".worker-composer-effort")) {
      void refreshPaneEffort(term);
      return;
    }
  }
}

/* The answer a chip paints from, with the moment the window sees itself: a
 * turn under way in the pane says so before the backend is asked again. */
function composerEffortOf(run) {
  const said = paneEfforts.get(run.term)?.said ?? null;
  if (!said) return null;
  const working = composerWorking(run) && !said.stuck;
  return working ? { ...said, stuck: "turn_running" } : said;
}

/* Whether the stuck word is the road's own — no wait changes it. */
function effortRoadStuck(stuck) {
  return stuck === "no_road" || stuck === "relaunch" || stuck === "on_screen";
}

/* The word on the chip: the rung a press put the session on, then the one its
 * record says, then the CLI's default. The CLI's own spelling (`xhigh`) —
 * these are its vocabulary, as the permission mode is. */
function effortWords(said) {
  return said?.pending ?? said?.current ?? t("composer.effort.default", "기본");
}

/* Where the word came from, in a sentence: the session's own record, the
 * summons, or nobody. */
function effortWitnessWords(said) {
  if (said.pending) {
    return t("composer.effort.pending", "다음 턴부터 {{effort}} — 이 세션만", { effort: said.pending });
  }
  if (said.witness === "turn") {
    return t("composer.effort.fromTurn", "세션 기록: 마지막 턴이 {{effort}}로 돌았습니다", { effort: said.current });
  }
  if (said.witness === "summons") {
    return t("composer.effort.fromSummons", "소환 요청: {{effort}} (아직 턴 기록 없음)", { effort: said.current });
  }
  return t("composer.effort.fromDefault", "CLI 기본값 — 세션이 아직 노력을 말하지 않았습니다");
}

/* Why the rungs cannot be pressed now, by the backend's code. */
function effortStuckWords(said) {
  const command = said.command ?? "";
  switch (said.stuck) {
    case "no_road": return t("composer.effort.stuck.noRoad", "이 CLI의 노력을 실행 중에 바꾸는 길을 아직 재지 않았습니다");
    case "relaunch": return t("composer.effort.stuck.relaunch", "이 CLI는 실행 중에 노력을 바꾸지 못합니다 — 다음 실행에서 정합니다");
    case "on_screen": return t("composer.effort.stuck.onScreen", "이 CLI는 제 화면의 피커에서 바꿉니다 ({{command}})", { command });
    case "unknown": return t("composer.effort.stuck.unknown", "첫 턴이 끝나 세션이 노력을 말하면 바꿀 수 있습니다");
    case "off_ladder": return t("composer.effort.stuck.offLadder", "세션이 이 목록에 없는 단계에 있습니다");
    case "turn_running": return t("composer.effort.stuck.turnRunning", "턴이 도는 중입니다 — 턴 사이에 바꿀 수 있습니다");
    case "never_heard": return t("composer.effort.stuck.neverHeard", "이 판의 턴 상태를 아직 듣지 못했습니다");
    case "held_by_person": return t("composer.effort.stuck.heldByPerson", "지난 턴을 직접 멈춘 판입니다 — 다음 턴이 끝나면 바꿀 수 있습니다");
    case "shell_in_front": return t("composer.effort.stuck.shellInFront", "에이전트 앞에 셸이 있습니다");
    case "draft": return t("composer.effort.stuck.draft", "판의 입력줄에 쓰던 글이 있습니다");
    case "seat_moving": return t("composer.effort.stuck.seatMoving", "자동 노력 조절이 이 판을 옮기는 중입니다");
    case "press_moving": return t("composer.effort.stuck.pressMoving", "앞서 누른 바꾸기가 아직 진행 중입니다 — 끝나면 다시 누를 수 있습니다");
    default: return "";
  }
}

/* What a press refused with, in words: a stuck code, or the door's own. */
function effortRefusalWords(code) {
  if (code === "withheld") return t("composer.effort.refused.withheld", "판이 피커 명령을 받지 않았습니다 — 노력은 그대로입니다");
  if (code === "picker_missing") return t("composer.effort.refused.pickerMissing", "피커가 화면에 뜨지 않았습니다 — 노력은 그대로입니다");
  return effortStuckWords({ stuck: code }) || String(code);
}

function composerEffortChip(run) {
  const chip = document.createElement("button");
  chip.type = "button";
  chip.className = "worker-composer-pill worker-composer-effort";
  chip.hidden = true;
  chip.setAttribute("aria-haspopup", "menu");
  chip.setAttribute("aria-expanded", "false");
  chip.dataset.keyboardOwner = "true";
  const chevron = document.createElement("span");
  chevron.className = "worker-composer-chevron";
  chevron.textContent = "▾";
  chip.append(iconNode("brain"), pillWordsNode(""), chevron);
  chip.addEventListener("click", () => {
    if (chip.getAttribute("aria-expanded") === "true") closeComposerMenu(chip);
    else void openComposerEffortMenu(chip, run);
  });
  paintComposerEffortChip(chip, run);
  void refreshPaneEffort(run.term);
  return chip;
}

/* Shown once the backend has answered for a pane; the tip says the word's
 * witness and, when a press cannot move it, why. Every write is guarded. */
function paintComposerEffortChip(chip, run) {
  const said = composerEffortOf(run);
  writeHidden(chip, !said);
  if (!said) return;
  writeTextContent(chip.querySelector(".worker-composer-pill-words"), effortWords(said));
  writeClass(chip, "is-stuck", effortRoadStuck(said.stuck));
  const why = said.stuck ? effortStuckWords(said) : t("composer.effort.tip", "노력 바꾸기 — 이 세션만");
  const tip = t("composer.effort.chipTip", "노력 {{effort}} · {{why}}", { effort: effortWords(said), why });
  writeAttribute(chip, "data-tip", tip);
  writeAttribute(chip, "aria-label", tip);
}

/* A line of words inside the menu, in the empty line's type. */
function composerMenuNote(menu, words, kind) {
  const note = document.createElement("p");
  note.className = `composer-menu-empty composer-effort-${kind}`;
  note.textContent = words;
  menu.appendChild(note);
  return note;
}

/* The effort menu: where the word came from, the rungs (the one the session
 * stands on ticked), why they cannot be pressed when they cannot, and — on a
 * road where a move rewrites the cache — what a move costs. Asked afresh on
 * opening, so the menu never offers a press on a stale answer. */
async function openComposerEffortMenu(chip, run) {
  closeComposerMenu();
  await refreshPaneEffort(run.term);
  const said = composerEffortOf(run);
  if (!said) return;
  const menu = composerMenuNode();
  menu.classList.add("composer-effort-menu");
  composerMenuSection(menu, t("composer.effort.menu", "노력"));
  composerMenuNote(menu, effortWitnessWords(said), "witness");
  const stands = said.pending ?? said.current;
  for (const rung of said.ladder ?? []) {
    composerMenuItem(
      menu,
      rung,
      rung === stands ? t("composer.effort.now", "지금") : "",
      () => void pressPaneEffort(chip, run, said, rung),
      { current: rung === stands, disabled: Boolean(said.stuck) || rung === stands },
    );
  }
  if (said.stuck) composerMenuNote(menu, effortStuckWords(said), "stuck");
  else if (said.rewritesCache) composerMenuNote(menu, effortCacheWords(run), "cost");
  standOpenComposerMenu(chip, menu);
}

/* What a move costs on a road that rewrites the cache, in one line — with the
 * conversation's size when the pane has said it. */
function effortCacheWords(run) {
  const reading = composerRoad(run).usage?.() ?? null;
  return reading?.tokens > 0
    ? t("composer.effort.cacheTokens", "바꾸면 이 대화 {{tokens}} 토큰을 캐시에 다시 씁니다 — 다음 턴이 느려지고 그만큼 값이 듭니다", { tokens: reading.tokens.toLocaleString() })
    : t("composer.effort.cache", "바꾸면 이 대화를 캐시에 다시 씁니다 — 다음 턴이 느려지고 그만큼 값이 듭니다");
}

/* A rung picked. On a road that rewrites the cache the menu asks once more
 * with the cost in front of the person; the move goes only on that yes. */
async function pressPaneEffort(chip, run, said, rung) {
  if (said.rewritesCache) {
    const menu = composerMenuNode();
    menu.classList.add("composer-effort-menu");
    composerMenuSection(menu, t("composer.effort.confirmHead", "노력 {{from}} → {{to}}", { from: effortWords(said), to: rung }));
    composerMenuNote(menu, effortCacheWords(run), "cost");
    composerMenuItem(menu, t("composer.effort.confirm", "바꾸기 (이 세션만)"), "", () => void movePaneEffort(run, rung));
    composerMenuItem(menu, t("composer.effort.cancel", "그대로 두기"), "", () => chip.focus());
    standOpenComposerMenu(chip, menu);
    return;
  }
  await movePaneEffort(run, rung);
}

/* The press itself: the backend drives the picker and answers the chip as it
 * stands after; a refusal is said in the person's words and changes nothing. */
async function movePaneEffort(run, rung) {
  try {
    const said = await invoke("pane_effort_press", { term: run.term, to: rung });
    rememberPaneEffort(run.term, said);
  } catch (error) {
    showError(effortRefusalWords(String(error)));
    void refreshPaneEffort(run.term);
  }
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

/* ---- the keys (t-6323 A3) ---------------------------------------------------
 *
 * The extension's prompt keys (2.1.280 webview `dB0`): Enter sends and Shift+
 * Enter or Ctrl+J breaks the line (`zC1`; an Enter inside a composition is
 * the composition's); Shift+Tab steps the permission mode (`d6`); ArrowUp at
 * the very start of the box recalls the previous prompt and ArrowDown at the
 * very end walks back toward the draft (`Uq0` — the session's prompts and
 * the queued ones, newest first, no wrap). The palette and the menus take
 * their keys first (the palette listens in the capture phase and prevents
 * what it takes), so a key they used is not one of these. */
function composerKeys(form, box, run, spec) {
  box.addEventListener("keydown", (event) => {
    if (event.defaultPrevented || event.isComposing || event.keyCode === 229) return;
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      form.requestSubmit();
      return;
    }
    if (event.key.toLowerCase() === "j" && event.ctrlKey && !event.shiftKey && !event.altKey && !event.metaKey) {
      event.preventDefault();
      box.setRangeText("\n", box.selectionStart, box.selectionEnd, "end");
      box.dispatchEvent(new Event("input", { bubbles: true }));
      return;
    }
    if (event.key === "Tab" && event.shiftKey && !event.ctrlKey && !event.altKey && !event.metaKey) {
      const road = composerRoad(run, spec);
      if (!road.canChangeMode()) return;
      event.preventDefault();
      road.cycleMode().catch((error) => showError(error));
      return;
    }
    if (event.key === "ArrowUp" || event.key === "ArrowDown") {
      if (event.shiftKey || event.altKey || event.ctrlKey || event.metaKey) return;
      if (recallComposerHistory(box, run, event.key === "ArrowUp" ? 1 : -1)) event.preventDefault();
    }
  });
  // Emptied by hand or sent: the walk starts again from the newest.
  box.addEventListener("input", () => {
    if (box.value === "" && !box.__recalling) box.__recall = null;
  });
  form.addEventListener("submit", () => {
    box.__recall = null;
  });
}

/* The prompts ArrowUp walks through, newest first: what is queued, then what
 * the person said on this page. */
function composerHistory(run) {
  const said = (run.helper?.turns ?? []).filter((turn) => turn.role === "user").map((turn) => turn.text);
  return [...(run.queue ?? [])].reverse().concat(said.reverse()).filter((text) => text.trim() !== "");
}

/* One step through the history (`by` 1 = older, -1 = newer), under the
 * extension's own rule: older only with the caret at the very start of the
 * box, newer only at the very end and only while a recall stands; past the
 * newest the draft comes back. Answers whether it moved, so the key does its
 * ordinary work when it did not. */
function recallComposerHistory(box, run, by) {
  const start = box.selectionStart === 0 && box.selectionEnd === 0;
  const end = box.selectionStart === box.value.length && box.selectionEnd === box.value.length;
  const held = box.__recall ?? { at: -1, draft: "" };
  const history = composerHistory(run);
  let at;
  if (by > 0) {
    if (!start || held.at + 1 >= history.length) return false;
    if (held.at === -1) held.draft = box.value;
    at = held.at + 1;
  } else {
    if (!end || held.at === -1) return false;
    at = held.at - 1;
  }
  held.at = at;
  box.__recall = held;
  box.__recalling = true;
  box.value = at === -1 ? held.draft : history[at];
  box.dispatchEvent(new Event("input", { bubbles: true }));
  box.__recalling = false;
  if (at === -1) box.__recall = null;
  // Back through the history the caret stays at the start, so the next
  // ArrowUp walks on; forward it goes to the end.
  const caret = by > 0 ? 0 : box.value.length;
  box.setSelectionRange(caret, caret);
  return true;
}

/* A plain Esc anywhere on a conversation page interrupts its turn — the
 * extension's own (`zU0`, listening on the whole panel): no modifier, no
 * repeat, no composition, and only while the turn is out. A popup that took
 * the key first (the palette, a menu) prevented it; a question standing as
 * its card is answered there, unless the person is typing in the box. The
 * page's run is read when the key comes: one host carries several pages. */
function interruptOnEscape(host) {
  if (host.__interruptKeys) return;
  host.__interruptKeys = true;
  host.addEventListener("keydown", (event) => {
    if (event.key !== "Escape" || event.defaultPrevented || event.repeat) return;
    if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
    if (event.isComposing || event.keyCode === 229) return;
    const run = host.__helperPage?.run;
    // A helper's page has no input, and its `composerWorking` is its PARENT's
    // turn: Esc there would end the parent's whole turn from a page that never
    // said it could (t-15683). The parent's own page is where Esc interrupts.
    if (!run || isHelperPage(run) || !composerWorking(run)) return;
    const typing = event.target?.closest?.(".worker-composer-box") !== null && event.target?.closest?.(".worker-composer-box") !== undefined;
    if (host.querySelector(".pane-chat-ask") && !typing) return;
    event.preventDefault();
    const spec = installedAgents().find((row) => row.id === run.agent) ?? null;
    void composerRoad(run, spec).interrupt().catch((error) => showError(error));
  });
}
