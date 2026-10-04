/* ---- 그림·페이지로 설명 (t-32787) -----------------------------------------
 *
 * 사람이 diff, 대화의 한 턴, 과업 보고를 읽다가 글을 더 읽는 대신 그림 한 장을
 * 청한다. 세 자리에 같은 단추 하나가 서고, 누르면 카드가 열린다: 고른 내용에서
 * 비밀 값을 지우고 보낸다는 말, 이 판의 에이전트에게 보내기, 따로 만들기.
 *
 * 창은 요청만 한다. 정리·자르기·글 짓기·보내기는 백엔드의 것이고(`explain_*`),
 * 에이전트가 페이지를 발행하는 순간 백엔드가 `explain:state`로 알린다 — 이 파일은
 * 그 소식에 맞춰 상태 줄을 바꾸고 마지막에 그 아티팩트를 열 뿐이다. 새 타이머도
 * 새 폴러도 없다: 기다림은 이벤트가 끝낸다.
 *
 * 단추는 처음 필요할 때 짓는다(정적 hidden 단추는 부팅 DOM에 센다). diff 머리와
 * 인스펙터는 그리는 길이, 대화의 답변 행은 포인터나 초점이 처음 닿을 때 짓는다. */

/* 이 단추를 내는 세 자리와 각자의 말. */
const EXPLAIN_KINDS = Object.freeze({
  diff: { tip: { key: "explain.tip.diff", word: "이 변경을 그림·페이지로 설명" }, what: { key: "explain.kind.diff", word: "변경" } },
  turn: { tip: { key: "explain.tip.turn", word: "이 대화를 그림·페이지로 설명" }, what: { key: "explain.kind.turn", word: "대화" } },
  report: { tip: { key: "explain.tip.report", word: "이 보고를 그림·페이지로 설명" }, what: { key: "explain.kind.report", word: "보고" } },
});

/* diff 줄의 종류가 모델이 읽는 통일 diff에서 다는 표. 화면의 줄 클래스
 * (`diff-line--add`)와 줄 데이터(`kind`)가 같은 낱말을 쓰므로 이 표 하나로 읽는다. */
const EXPLAIN_DIFF_MARKS = Object.freeze({ add: "+", del: "-", ctx: " " });

/* 한 턴을 자료로 만들 때 말한 쪽의 이름 — 모델에게 가는 글의 구조이지 사람이 읽는
 * 문장이 아니다. */
const EXPLAIN_TURN_SPEAKERS = Object.freeze({ user: "Person", assistant: "Agent" });

/* 제목으로 쓰는 답의 첫 줄이 이보다 길면 자른다. */
const EXPLAIN_TITLE_CHARS = 60;

/* 진행 중인 요청이 상태 줄에서 하는 말, 백엔드의 상태 낱말마다(`explain_desk::State`).
 * 끝난 둘(ready·failed)은 줄이 아니라 알림이 말한다. */
const EXPLAIN_STATES = Object.freeze({
  waiting: { key: "explain.state.waiting", word: "{{agent}} · 끝나면 보낼게요" },
  sent: { key: "explain.state.sent", word: "{{agent}} · 보내는 중" },
  asked: { key: "explain.state.asked", word: "{{agent}} · 페이지를 만드는 중 — 발행하면 바로 열어요" },
  running: { key: "explain.state.running", word: "{{agent}} · 만드는 중 — 구독 한도를 쓰고 있어요" },
});
const EXPLAIN_READY = { key: "explain.state.ready", word: "페이지를 열었어요 · {{title}}" };
const EXPLAIN_FAILED = { key: "explain.state.failed", word: "설명 페이지를 만들지 못했어요 · {{why}}" };

/* 실패의 까닭, 백엔드의 토큰마다(`explain::why`). 판의 가드가 말하는 토큰
 * (`holds_a_draft`·`parked`…)은 창이 이미 가진 문장을 쓴다 — `termWithheldWords`. */
const EXPLAIN_WHY = Object.freeze({
  no_pane: { key: "explain.why.no_pane", word: "그 판이 더는 없어요" },
  no_agent: { key: "explain.why.no_agent", word: "그 판에 에이전트가 없거나 이 일을 받을 수 없어요" },
  in_flight: { key: "explain.why.in_flight", word: "그 판에 아직 끝나지 않은 설명 요청이 있어요" },
  too_many: { key: "explain.why.too_many", word: "진행 중인 설명 요청이 너무 많아요" },
  not_ready: { key: "explain.why.not_ready", word: "에이전트가 글을 받을 준비가 되기 전에 시간이 다 됐어요" },
  no_page: { key: "explain.why.no_page", word: "에이전트가 페이지를 발행하지 않고 턴을 마쳤어요" },
  cli_missing: { key: "explain.why.cli_missing", word: "이 컴퓨터에서 그 에이전트의 명령을 찾지 못했어요" },
  quota_wall: { key: "explain.why.quota_wall", word: "구독 한도에 닿아 쉬는 중이에요" },
  login_wall: { key: "explain.why.login_wall", word: "그 에이전트에 로그인이 필요해요" },
  timed_out: { key: "explain.why.timed_out", word: "정해진 시간 안에 끝내지 못했어요" },
  not_html: { key: "explain.why.not_html", word: "에이전트의 답이 페이지가 아니었어요" },
  cli_refused: { key: "explain.why.cli_refused", word: "에이전트가 이 일을 거절하거나 실패했어요" },
  budget: { key: "explain.why.budget", word: "같은 일이 이미 돌고 있어서 새로 시작하지 않았어요" },
  not_published: { key: "explain.why.not_published", word: "만든 페이지를 아티팩트로 올리지 못했어요" },
});

/* 이 창이 청해서 아직 끝나지 않은 요청, 이름순이 아니라 청한 순서. 끝나는 순간
 * 지워지므로 서 있는 것은 진행 중인 것뿐이다. */
const explainRequests = new Map();

/* 까닭 하나를 사람의 말로. 우리 토큰이 아니면 판의 가드가 쓰는 문장 표에서 찾고,
 * 거기에도 없으면 토큰 그대로 둔다. */
function explainWhyWords(why, term = null) {
  const own = EXPLAIN_WHY[why];
  if (own) return t(own.key, own.word);
  const sentence = withheldSentenceOf(why, false) ?? withheldSentenceOf(why, true);
  return sentence ? t(sentence.key, sentence.word, { term }) : String(why ?? "");
}

/* 창의 언어를 두 글자 코드로 — 백엔드가 모델에게 「이 언어로 쓰라」고 말할 때 읽는다. */
function explainLanguage() {
  const code = locale === "system" ? systemLocale : locale;
  return String(code ?? "").slice(0, 2).toLowerCase();
}

/* 단추 하나, 한 번만 짓는다. 이미 있으면 말과 자료의 길만 새로 건다 — 다시 그리는
 * 길이 부르므로 아무것도 새로 짓지 않는 것이 비용의 전부다. `source`는 눌렀을 때의
 * 자료를 돌려주는 함수(없으면 null), `before`는 그 앞에 세울 이웃이다. */
function ensureExplainButton(host, kind, source, { before = null } = {}) {
  let button = host.querySelector(":scope > .explain-open");
  if (!button) {
    button = document.createElement("button");
    button.type = "button";
    button.className = "explain-open";
    button.dataset.explainKind = kind;
    button.appendChild(iconNode("workflow"));
    if (before) before.before(button);
    else host.appendChild(button);
  }
  const row = EXPLAIN_KINDS[kind].tip;
  const tip = t(row.key, row.word);
  if (button.dataset.tip !== tip) {
    button.dataset.tip = tip;
    button.setAttribute("aria-label", tip);
  }
  button._explainSource = source;
  return button;
}

/* ---- 자료: 세 자리가 각자 내놓는 것 ---- */

/* 긁어 둔 diff 줄 — 선택이 이 판의 줄에만 걸쳐 있을 때만. 없으면 null. */
function explainSelectedDiffLines(rows) {
  const selection = window.getSelection();
  if (!rows || !selection || selection.isCollapsed || selection.rangeCount === 0) return null;
  const range = selection.getRangeAt(0);
  const picked = [...rows.querySelectorAll(".diff-line")].filter((row) => range.intersectsNode(row));
  if (picked.length === 0) return null;
  return picked.map((row) => {
    const kind = Object.keys(EXPLAIN_DIFF_MARKS).find((one) => row.classList.contains(`diff-line--${one}`));
    return (EXPLAIN_DIFF_MARKS[kind] ?? "") + (row.querySelector(".diff-text")?.textContent ?? "");
  });
}

/* diff 보기: 긁은 줄이 있으면 그 줄, 없으면 그 파일의 diff 전체 — 모델이 파일
 * 구획을 알아보도록 통일 diff의 머리를 단다. 줄이 없는 diff(너무 커서 보류됨)는 null. */
function explainSourceOfDiff(button) {
  const view = button.closest(".file-view");
  const tab = tabs.find((one) => one.id === view?.dataset.tab);
  if (!tab || !Array.isArray(tab.lines) || tab.lines.length === 0) return null;
  const lines = explainSelectedDiffLines(view.querySelector(".diff-rows"))
    ?? tab.lines.map((line) => (EXPLAIN_DIFF_MARKS[line.kind] ?? "") + line.text);
  return {
    title: tab.path,
    text: [`diff --git a/${tab.path} b/${tab.path}`, ...lines].join("\n"),
    term: artifactDraftSeats()[0]?.term ?? null,
  };
}

/* 대화의 한 턴: 그 답과 바로 앞 사람의 말. 판은 그 대화가 들어 있는 판이다. */
function explainSourceOfTurn(run, turn) {
  if (!run || !turn) return null;
  const turns = run.helper?.turns ?? [];
  const at = turns.indexOf(turn);
  const asked = turns.slice(0, at < 0 ? turns.length : at).findLast((one) => one.role === "user")?.text ?? "";
  const answered = cleanseAssistantText(turn.text) || turn.text || "";
  if (answered === "") return null;
  const said = (role, text) => `${EXPLAIN_TURN_SPEAKERS[role]}:\n${text}`;
  const parts = asked === "" ? [said("assistant", answered)] : [said("user", asked), said("assistant", answered)];
  const head = answered.split("\n", 1)[0].slice(0, EXPLAIN_TITLE_CHARS);
  return {
    title: head || t(EXPLAIN_KINDS.turn.what.key, EXPLAIN_KINDS.turn.what.word),
    text: parts.join("\n\n"),
    term: run.term ?? null,
  };
}

/* 과업 보고: 보고서 아티팩트의 id만 보낸다 — 본문은 백엔드가 파일에서 읽는다. */
function explainSourceOfReport(entity, report, term) {
  return { title: entity.facts?.identity ?? report, report, term };
}

/* 답변 행의 단추: 행에 포인터나 초점이 처음 닿을 때 한 번 짓는다 — 긴 대화의 모든
 * 답마다 노드 셋씩을 미리 지으면 읽지도 않은 행이 값을 낸다. */
function ensureExplainTurnButton(event) {
  const row = event.target.closest?.(".helper-turn.is-assistant");
  const actions = row?.querySelector(":scope > .helper-actions");
  if (!actions || actions.querySelector(":scope > .explain-open")) return;
  ensureExplainButton(actions, "turn", () => explainSourceOfTurn(row.__run, row.__turn));
}

document.addEventListener("pointerover", ensureExplainTurnButton);
document.addEventListener("focusin", ensureExplainTurnButton);

/* ---- 카드 ---- */

/* 한 줄 사실: 무엇을 몇 줄, 비밀 값을 몇 곳 지웠는지, 잘렸는지. */
function explainFactsNode(kind, facts) {
  const line = document.createElement("div");
  line.className = "explain-facts";
  const what = EXPLAIN_KINDS[kind].what;
  const words = facts.clipped
    ? t("explain.factsClipped", "{{what}} {{lines}}줄 · 비밀 값 {{masked}}곳은 지우고, 길어서 뒤쪽은 잘라 보내요", {
      what: t(what.key, what.word), lines: facts.lines, masked: facts.masked,
    })
    : t("explain.facts", "{{what}} {{lines}}줄 · 비밀 값 {{masked}}곳은 지우고 보내요", {
      what: t(what.key, what.word), lines: facts.lines, masked: facts.masked,
    });
  line.textContent = words;
  return line;
}

/* 카드의 행 하나: 에이전트 아이콘, 이름, 그 아래 한 줄. */
function explainRowNode(spec, second, secondClass = "") {
  const pick = document.createElement("button");
  pick.className = "note-pop-row";
  pick.type = "button";
  pick.appendChild(agentIcon(spec));
  const words = document.createElement("span");
  words.className = "note-pop-words";
  const name = document.createElement("span");
  name.className = "note-pop-name";
  name.textContent = spec.name;
  const where = document.createElement("span");
  where.className = `note-pop-where ${secondClass}`.trim();
  where.textContent = second;
  words.append(name, where);
  pick.appendChild(words);
  return pick;
}

function explainNoneNode(words) {
  const none = document.createElement("div");
  none.className = "note-pop-none";
  none.textContent = words;
  return none;
}

/* 카드를 짓고 보낼 곳에 앵커한다. 보내기 메뉴의 `#note-pop`을 같이 쓴다 — 팝 요소를
 * 따로 두지 않는다. 처음 행에 초점이 간다(키보드 한 번이면 보낸다). */
function paintExplainCard(button, kind, source, facts, roads, targets) {
  const host = el("note-pop-body");
  host.replaceChildren();
  // 같이 쓰는 팝의 이름은 보내기 메뉴의 것이다 — 이 카드가 서 있는 동안은 이 카드의 이름을 쓴다.
  notePop.setAttribute("aria-label", t("explain.title", "그림·페이지로 설명"));
  const card = document.createElement("div");
  card.className = "explain-card";
  const label = (words) => {
    const line = document.createElement("div");
    line.className = "note-pop-label";
    line.textContent = words;
    card.appendChild(line);
  };
  label(t("explain.title", "그림·페이지로 설명"));
  if (facts) card.appendChild(explainFactsNode(kind, facts));
  label(t("explain.group.pane", "이 판의 에이전트에게 보내기"));
  if (targets.length === 0) card.appendChild(explainNoneNode(t("explain.none", "에이전트가 앉은 판이 없어요")));
  for (const target of targets) {
    const seat = tabLabel(target.tab);
    const where = target.term === target.tab.term ? seat : t("review.splitSeat", "{{tab}} · 분할", { tab: seat });
    const pick = explainRowNode(target.spec, where);
    pick.addEventListener("click", () => void startExplain(kind, source, { via: "conversation", term: target.term }, target.spec.name));
    card.appendChild(pick);
  }
  const rule = document.createElement("div");
  rule.className = "note-pop-rule";
  card.appendChild(rule);
  label(t("explain.group.once", "따로 만들기"));
  const once = roads.map((road) => agentRows.find((row) => row.id === road.agent)).filter(Boolean);
  if (once.length === 0) card.appendChild(explainNoneNode(t("explain.noneOnce", "따로 만들 수 있는 에이전트가 없어요")));
  for (const spec of once) {
    const pick = explainRowNode(spec, t("explain.quota", "구독 한도를 씁니다"), "explain-quota");
    pick.classList.add("explain-once");
    pick.addEventListener("click", () => void startExplain(kind, source, { via: "one_shot", agent: spec.id }, spec.name));
    card.appendChild(pick);
  }
  host.appendChild(card);
  showing(notePop);
  const at = button.getBoundingClientRect();
  const width = notePop.getBoundingClientRect().width;
  notePop.style.left = `${Math.min(Math.max(8, at.right - width), window.innerWidth - width - 8)}px`;
  notePop.style.top = `${Math.min(at.bottom + 6, window.innerHeight - 80)}px`;
  card.querySelector(".note-pop-row")?.focus();
}

/* 단추를 눌렀다: 사실 한 줄(백엔드가 지운 곳을 센다)과 보낼 곳을 모아 카드를 연다.
 * 보내기 메뉴와 같은 세대표를 쓴다 — 나중에 연 쪽이 이긴다. */
async function openExplainCard(button) {
  closeNoteSend();
  const generation = ++noteSendGeneration;
  const tabNow = activeTabId;
  const worktreeNow = activeWorktreePath;
  const source = button._explainSource?.(button) ?? null;
  if (!source) return;
  const kind = button.dataset.explainKind;
  const [facts, roads, targets] = await Promise.all([
    invoke("explain_preview", { kind, text: source.text ?? null, report: source.report ?? null }).catch(() => null),
    invoke("explain_roads").catch(() => []),
    runningAgentTargets({ maker: source.term ?? null }),
  ]);
  if (generation !== noteSendGeneration || activeTabId !== tabNow || activeWorktreePath !== worktreeNow) return;
  paintExplainCard(button, kind, source, facts, Array.isArray(roads) ? roads : [], targets);
}

document.addEventListener("click", (event) => {
  const button = event.target.closest?.(".explain-open");
  if (!button || button.disabled) return;
  if (!overlayClosed(notePop) && notePop.querySelector(".explain-card")) closeNoteSend();
  else void openExplainCard(button);
});

/* ---- 요청 ---- */

/* 고른 곳으로 청한다. 모든 결과는 `explain:state`로 돌아오므로 여기서는 이름 하나를
 * 짓고 말을 올릴 뿐이다 — 백엔드가 청 자체를 거절하는 경우(알 수 없는 종류)만 던진다. */
async function startExplain(kind, source, route, agent) {
  closeNoteSend();
  const id = `explain-${crypto.randomUUID()}`;
  explainRequests.set(id, { id, agent, state: "sent", route });
  paintExplainStatus();
  const request = {
    id,
    kind,
    title: source.title,
    text: source.text ?? null,
    report: source.report ?? null,
    language: explainLanguage(),
    headline: t("explain.headline", "그림·페이지로 설명해 주세요"),
    route,
  };
  try {
    await invoke("explain_start", { request });
  } catch (error) {
    explainRequests.delete(id);
    paintExplainStatus();
    showError(String(error));
  }
}

/* 만든 페이지를 연다. 이미 그 아티팩트의 탭이 있으면 다시 열지 않는다 — 발행 소식이
 * 먼저 만든 판 옆에 열어 두었을 수 있다. */
async function openExplainedPage(row) {
  if (tabs.some((one) => one.kind === "browser" && one.artifact?.id === row.id)) {
    markArtifactSeen(row.id);
    return;
  }
  try {
    await openArtifactBeside(row, artifactMakerTerm(row));
  } catch (error) {
    showError(String(error));
  }
}

/* 백엔드의 소식 하나. 이 창이 청한 요청만 읽는다 — 다른 창이나 지난 기동의 것은 모른다. */
function noteExplainState(payload) {
  const held = explainRequests.get(payload?.id);
  if (!held) return;
  if (payload.state === "ready") {
    explainRequests.delete(held.id);
    paintExplainStatus();
    toast(t(EXPLAIN_READY.key, EXPLAIN_READY.word, { title: payload.artifact?.title ?? "" }));
    if (payload.artifact) void openExplainedPage(payload.artifact);
    return;
  }
  if (payload.state === "failed") {
    explainRequests.delete(held.id);
    paintExplainStatus();
    const why = explainWhyWords(payload.why, payload.term ?? held.route.term ?? null);
    toast(t(EXPLAIN_FAILED.key, EXPLAIN_FAILED.word, { why }), "halt");
    return;
  }
  if (!EXPLAIN_STATES[payload.state]) return;
  held.state = payload.state;
  if (payload.agent) held.agent = agentName(payload.agent);
  paintExplainStatus();
}

listen("explain:state", (event) => noteExplainState(event?.payload ?? null));

/* ---- 상태 줄 ---- */

/* 진행 중인 가장 최근 요청의 한 줄 — 처음 필요할 때 짓는다. 열거나 실패하면 내려간다. */
function explainStatusNode() {
  let node = el("sb-explain");
  if (node) return node;
  node = document.createElement("span");
  node.className = "sb-item sb-explain";
  node.id = "sb-explain";
  node.setAttribute("role", "status");
  node.hidden = true;
  node.appendChild(iconNode("workflow"));
  const words = document.createElement("span");
  words.className = "sb-explain-words";
  const cancel = document.createElement("button");
  cancel.type = "button";
  cancel.className = "sb-button sb-explain-act";
  cancel.addEventListener("click", cancelExplain);
  node.append(words, cancel);
  document.querySelector(".statusbar .sb-spacer").before(node);
  return node;
}

function paintExplainStatus() {
  const live = [...explainRequests.values()].at(-1) ?? null;
  const node = live ? explainStatusNode() : el("sb-explain");
  if (!node) return;
  node.hidden = live === null;
  if (!live) return;
  const row = EXPLAIN_STATES[live.state];
  node.dataset.id = live.id;
  node.querySelector(".sb-explain-words").textContent = t(row.key, row.word, { agent: live.agent });
  node.querySelector(".sb-explain-act").textContent = t("explain.cancel", "취소");
}

/* 「취소」: 이 창이 더는 기다리지 않고, 백엔드가 그 요청을 놓는다. 이미 판에 들어간
 * 글이나 도는 한 번 실행은 거두지 못한다 — 놓은 것은 결과를 열지 않는 일뿐이다. */
function cancelExplain() {
  const id = el("sb-explain")?.dataset.id;
  if (!id) return;
  explainRequests.delete(id);
  paintExplainStatus();
  void invoke("explain_cancel", { id }).catch(() => {});
}
