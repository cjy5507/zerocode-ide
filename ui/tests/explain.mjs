import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { standZo } from "./conversation-parity.mjs";
import { openWindowTestPage } from "./window-boot.mjs";

/* 「그림·페이지로 설명」 — 세 자리의 단추, 카드, 상태 줄, 열린 페이지 (t-32787).
 *
 * 창은 요청만 한다. 정리·자르기·보내기와 페이지가 준비됐다는 소식은 백엔드의 것이므로
 * 여기서는 백엔드를 가짜로 세우고(`explain_*` 명령과 `explain:state` 이벤트) 창이 그
 * 둘레에서 보이는 것을 고정한다:
 *
 *  - 단추는 처음 필요할 때 선다: 부팅 DOM에도, 손이 닿지 않은 답변 행에도 없다.
 *  - 세 자리(diff 머리, 답변 행, 인스펙터의 보고)가 같은 카드를 연다 — 사실 한 줄,
 *    이 판의 에이전트들, 따로 만들기(구독 한도 문구).
 *  - 고르면 요청이 간다(내용·판·길), 가짜 에이전트의 소식이 상태 줄을 바꾸고,
 *    발행된 아티팩트가 탭으로 열리며 두 번 열리지 않는다.
 *  - 모든 낱말은 다섯 언어 카탈로그에 있다 — 백엔드의 까닭 토큰 전부와 함께.
 *  - 이 파일은 타이머도 폴러도 짓지 않는다.
 *
 *   WINDOW_SUITES=explain node ui/tests/window.mjs */

const HERE = dirname(fileURLToPath(import.meta.url));
const LOCALES = ["ko", "en", "ja", "zh", "es"];

/* 구독 한도 문구와 요청의 첫 줄 — 다섯 언어로 못 박아, 카탈로그가 다른 말이나 번역되지
 * 않은 키로 흘러도 스스로 맞장구치지 않게 한다. */
export const QUOTA_WORDS = Object.freeze({
  ko: "구독 한도를 씁니다",
  en: "Uses your subscription quota",
  ja: "サブスクリプションの上限を使います",
  zh: "会消耗订阅额度",
  es: "Usa la cuota de tu suscripción",
});
export const HEADLINE_WORDS = Object.freeze({
  ko: "그림·페이지로 설명해 주세요",
  en: "Please explain this with a picture or page",
  ja: "図・ページで説明してください",
  zh: "请用图或页面来解释",
  es: "Explícalo con una imagen o página, por favor",
});

/* 백엔드가 말하는 까닭 토큰 전부 — Rust의 `explain::why`에서 읽는다(복사본이 아니라). */
async function readWhyTokens() {
  const source = await readFile(resolve(HERE, "..", "..", "crates/zerocode-core/src/explain.rs"), "utf8");
  return [...source.matchAll(/pub const [A-Z_]+: &str = "([a-z_]+)";/g)].map((hit) => hit[1]);
}

/* 가짜 백엔드와 가짜 에이전트: 명령은 부른 것을 적고, `__SAY__`는 에이전트가 페이지를
 * 발행했을 때 백엔드가 내는 소식을 창이 듣는 길 그대로 흘린다. */
const standBackend = () => {
  const seen = (window.__EXPLAIN__ = { starts: [], previews: [], cancels: [] });
  window.__ANSWER__.explain_roads = () => [{ agent: "claude" }, { agent: "codex" }];
  window.__ANSWER__.explain_preview = (args) => {
    seen.previews.push(args);
    return { lines: 3, masked: 2, clipped: false };
  };
  window.__ANSWER__.explain_start = (args) => {
    seen.starts.push(args.request);
    return null;
  };
  window.__ANSWER__.explain_cancel = (args) => {
    seen.cancels.push(args.id);
    return true;
  };
  window.__SAY__ = (payload) => {
    for (const held of window.__LISTENERS__["explain:state"] ?? []) held({ payload });
  };
  window.__PAGE__ = (term, id) => ({
    id, kind: "page", title: "변경 설명", path: `/data/artifacts/pages/${id}/index.html`, version: 1,
    source_path: "/tmp/explain.html", origin: { pane: `term-${term}`, agent: "claude" }, modified_ms: Date.now(),
  });
  window.__SETTLE__ = () => new Promise((done) => setTimeout(done, 250));
  // 열린 아티팩트 탭이 diff를 가리므로, diff를 다시 쓰는 케이스는 먼저 앞으로 데려온다.
  window.__DIFF_VIEW__ = async () => {
    await openDiff("src/a.rs");
    await window.__SETTLE__();
    return [...document.querySelectorAll(".file-view")].find((one) => one.querySelector(".diff-merge") && !one.hidden);
  };
};

export async function testExplain(browser, origin, ok) {
  const whyTokens = await readWhyTokens();
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const script = await (await page.request.get(`${origin}/shell-explain.js`)).text();
    await page.evaluate(standBackend);

    /* 부팅 직후: 단추도 상태 줄도 어디에도 없다 — 문은 처음 원할 때 짓는다. */
    const boot = await page.evaluate(() => ({
      buttons: document.querySelectorAll(".explain-open").length,
      status: document.getElementById("sb-explain") === null,
      staticMarkup: [...document.querySelectorAll("[id*='explain'], [class*='explain']")].length,
      script: typeof ensureExplainButton === "function" && typeof noteExplainState === "function",
    }));
    ok(
      "nothing of the explain action is in the boot DOM — no button, no status line, no static markup",
      boot.script && boot.buttons === 0 && boot.status && boot.staticMarkup === 0,
      JSON.stringify(boot),
    );

    /* ---- 1. diff 보기 ---- */
    const diff = await page.evaluate(async () => {
      const seen = {};
      // 제 탭을 가진 판 — 부팅이 먼저 연 터미널의 분할로 서면 카드의 행이 「· 분할」을 붙인다(그것도 맞다).
      const term = await openTermTab({ placement: "tab" });
      window.__AGENT_TERMS__ = [[term, "claude"]];
      paneAgents.set(term, "claude");
      setDiffSideBySide(false);
      await openDiff("src/a.rs");
      await window.__SETTLE__();
      const view = [...document.querySelectorAll(".file-view")].find((one) => one.querySelector(".diff-merge") && !one.hidden);
      const open = view.querySelector(".file-view-head .explain-open");
      seen.term = term;
      seen.worktree = tabs.find((one) => one.id === view.dataset.tab)?.worktree ?? null;
      seen.buttons = view.querySelectorAll(".explain-open").length;
      seen.tag = open?.tagName ?? "";
      seen.type = open?.type ?? "";
      seen.label = open?.getAttribute("aria-label") ?? "";
      seen.expectedLabel = t("explain.tip.diff", "이 변경을 그림·페이지로 설명");
      seen.kind = open?.dataset.explainKind ?? "";
      seen.focusable = open ? open.tabIndex >= 0 && !open.disabled : false;
      seen.beforeSend = open?.nextElementSibling?.classList.contains("diff-send") ?? false;
      // 다시 그려도 단추는 하나다.
      paintDiffView(tabs.find((one) => one.id === view.dataset.tab));
      seen.afterRepaint = view.querySelectorAll(".explain-open").length;

      open?.click();
      await window.__SETTLE__();
      const pop = document.getElementById("note-pop");
      seen.open = !pop.hidden && Boolean(pop.querySelector(".explain-card"));
      seen.labels = [...pop.querySelectorAll(".note-pop-label")].map((one) => one.textContent);
      seen.expectedLabels = [
        t("explain.title", "그림·페이지로 설명"),
        t("explain.group.pane", "이 판의 에이전트에게 보내기"),
        t("explain.group.once", "따로 만들기"),
      ];
      seen.facts = pop.querySelector(".explain-facts")?.textContent ?? "";
      seen.names = [...pop.querySelectorAll(".note-pop-row .note-pop-name")].map((one) => one.textContent);
      seen.where = pop.querySelector(".note-pop-row .note-pop-where")?.textContent ?? "";
      seen.expectedWhere = t("terminal.numbered", "터미널 {{n}}", { n: term });
      seen.quotas = [...pop.querySelectorAll(".explain-once .explain-quota")].map((one) => one.textContent);
      seen.focusedRow = document.activeElement === pop.querySelector(".note-pop-row");
      seen.dialogName = pop.getAttribute("aria-label");
      seen.preview = window.__EXPLAIN__.previews.at(-1);
      // 같은 단추를 다시 누르면 카드가 닫힌다(토글).
      open?.click();
      await window.__SETTLE__();
      seen.toggledClosed = pop.hidden;
      return seen;
    });
    ok(
      "the diff head wears one keyboard-reachable button, drawn once, before the notes' send button",
      diff.buttons === 1 && diff.afterRepaint === 1 && diff.tag === "BUTTON" && diff.type === "button" &&
        diff.kind === "diff" && diff.focusable && diff.beforeSend && diff.label === diff.expectedLabel && diff.label !== "",
      JSON.stringify(diff),
    );
    ok(
      "the card says what is sent, offers the pane's agent first and the one-shots with the quota note, and a second press closes it",
      diff.open && diff.labels.join("|") === diff.expectedLabels.join("|") &&
        diff.facts.includes("3") && diff.facts.includes("2") &&
        diff.names.join(",") === "Claude,Claude,Codex" && diff.where === diff.expectedWhere &&
        diff.quotas.length === 2 && diff.quotas.every((words) => words === QUOTA_WORDS.ko) &&
        diff.focusedRow && diff.toggledClosed && diff.dialogName === diff.expectedLabels[0],
      JSON.stringify(diff),
    );
    ok(
      "the preview asks for the diff the person is looking at, as the unified text a model reads",
      diff.preview?.kind === "diff" && diff.preview?.report === null &&
        diff.preview?.text.startsWith("diff --git a/src/a.rs b/src/a.rs\n") &&
        diff.preview?.text.includes('+    println!("hi");') && diff.preview?.text.includes(" fn main() {"),
      JSON.stringify(diff.preview),
    );

    /* 고르면: 판의 대화로 간다. 요청은 이름·종류·제목·내용·언어·첫 줄·길을 싣는다. */
    const sent = await page.evaluate(async (term) => {
      const seen = {};
      const view = await window.__DIFF_VIEW__();
      view?.querySelector(".explain-open")?.click();
      await window.__SETTLE__();
      document.querySelector("#note-pop .note-pop-row")?.click();
      await window.__SETTLE__();
      const request = window.__EXPLAIN__.starts.at(-1);
      seen.request = request;
      seen.closed = document.getElementById("note-pop").hidden;
      seen.id = request?.id ?? "";
      const status = document.getElementById("sb-explain");
      seen.sentLine = status && !status.hidden ? status.querySelector(".sb-explain-words")?.textContent ?? "" : "";
      seen.sentExpected = t("explain.state.sent", "{{agent}} · 보내는 중", { agent: "Claude" });

      window.__SAY__({ id: request?.id, state: "asked", term, agent: "claude" });
      seen.askedLine = status?.querySelector(".sb-explain-words")?.textContent ?? "";
      seen.askedExpected = t("explain.state.asked", "{{agent}} · 페이지를 만드는 중 — 발행하면 바로 열어요", { agent: "Claude" });

      const page = window.__PAGE__(term, "p-explain-diff");
      window.__SAY__({ id: request?.id, state: "ready", term, agent: "claude", artifact: page });
      await window.__SETTLE__();
      const opened = () => tabs.filter((one) => one.kind === "browser" && one.artifact?.id === "p-explain-diff").length;
      seen.opened = opened();
      seen.statusGone = document.getElementById("sb-explain")?.hidden ?? false;
      seen.toast = [...document.querySelectorAll(".toast")].map((node) => node.textContent).join(" ");
      // 같은 소식이 다시 와도 — 이 창이 이미 지운 요청이다 — 탭은 하나다.
      window.__SAY__({ id: request?.id, state: "ready", term, agent: "claude", artifact: page });
      await window.__SETTLE__();
      seen.openedAgain = opened();
      return seen;
    }, diff.term);
    ok(
      "choosing the pane sends one request carrying the chosen diff, the window's language and the pane, and the card closes",
      sent.request?.id.startsWith("explain-") && sent.request?.kind === "diff" && sent.request?.title === "src/a.rs" &&
        sent.request?.text.includes('+    println!("hi");') && sent.request?.report === null &&
        sent.request?.language === "ko" && sent.request?.headline === HEADLINE_WORDS.ko &&
        sent.request?.route?.via === "conversation" && sent.request?.route?.term === diff.term && sent.closed,
      JSON.stringify(sent.request),
    );
    ok(
      "the status line follows the agent's news, and the published page opens as an artifact tab exactly once",
      sent.sentLine === sent.sentExpected && sent.askedLine === sent.askedExpected &&
        sent.opened === 1 && sent.statusGone && sent.openedAgain === 1 && sent.toast.includes("변경 설명"),
      JSON.stringify(sent),
    );

    /* 같은 아티팩트를 두 길이 한꺼번에 열려 해도 — 발행 소식의 자동 열기와 이 단추의 열기 —
     * 탭은 하나다. */
    const race = await page.evaluate(async () => {
      const row = window.__PAGE__(1, "p-explain-race");
      await Promise.all([openArtifactBeside(row, null), openArtifactBeside(row, null)]);
      await window.__SETTLE__();
      return tabs.filter((one) => one.kind === "browser" && one.artifact?.id === "p-explain-race").length;
    });
    ok("two roads opening one published page at the same moment make one tab", race === 1, String(race));

    /* ---- 따로 만들기 + 실패의 말 ---- */
    const once = await page.evaluate(async () => {
      const seen = {};
      const view = await window.__DIFF_VIEW__();
      view?.querySelector(".explain-open")?.click();
      await window.__SETTLE__();
      [...document.querySelectorAll("#note-pop .explain-once")][1]?.click();
      await window.__SETTLE__();
      const request = window.__EXPLAIN__.starts.at(-1);
      seen.route = request?.route;
      window.__SAY__({ id: request?.id, state: "running", agent: "codex" });
      seen.runningLine = document.querySelector("#sb-explain .sb-explain-words")?.textContent ?? "";
      seen.runningExpected = t("explain.state.running", "{{agent}} · 만드는 중 — 구독 한도를 쓰고 있어요", { agent: "Codex" });
      for (const note of document.querySelectorAll(".toast")) note.remove();
      window.__SAY__({ id: request?.id, state: "failed", why: "quota_wall" });
      seen.statusGone = document.getElementById("sb-explain")?.hidden ?? false;
      const failure = [...document.querySelectorAll(".toast")].at(-1);
      seen.failure = failure?.textContent ?? "";
      seen.failureKind = failure?.dataset.kind ?? "";
      seen.failureExpected = t("explain.state.failed", "설명 페이지를 만들지 못했어요 · {{why}}", {
        why: t("explain.why.quota_wall", "구독 한도에 닿아 쉬는 중이에요"),
      });
      return seen;
    });
    ok(
      "a one-shot goes by the chosen agent's own name, says it spends the quota while it runs, and a quota wall is said in the person's words",
      once.route?.via === "one_shot" && once.route?.agent === "codex" && once.runningLine === once.runningExpected &&
        once.statusGone && once.failure === once.failureExpected && once.failureKind === "halt",
      JSON.stringify(once),
    );

    /* ---- 기다림과 취소, 가드의 거절은 창이 이미 가진 문장으로 ---- */
    const waiting = await page.evaluate(async (term) => {
      const seen = {};
      const view = await window.__DIFF_VIEW__();
      view?.querySelector(".explain-open")?.click();
      await window.__SETTLE__();
      document.querySelector("#note-pop .note-pop-row")?.click();
      await window.__SETTLE__();
      const request = window.__EXPLAIN__.starts.at(-1);
      window.__SAY__({ id: request?.id, state: "waiting", term, agent: "claude" });
      seen.waiting = document.querySelector("#sb-explain .sb-explain-words")?.textContent ?? "";
      seen.waitingExpected = t("explain.state.waiting", "{{agent}} · 끝나면 보낼게요", { agent: "Claude" });
      seen.cancelWord = document.querySelector("#sb-explain .sb-explain-act")?.textContent ?? "";
      document.querySelector("#sb-explain .sb-explain-act")?.click();
      await window.__SETTLE__();
      seen.cancelled = Boolean(request) && window.__EXPLAIN__.cancels.at(-1) === request.id;
      seen.hidden = document.getElementById("sb-explain")?.hidden ?? false;

      view?.querySelector(".explain-open")?.click();
      await window.__SETTLE__();
      document.querySelector("#note-pop .note-pop-row")?.click();
      await window.__SETTLE__();
      const refused = window.__EXPLAIN__.starts.at(-1);
      for (const note of document.querySelectorAll(".toast")) note.remove();
      window.__SAY__({ id: refused?.id, state: "failed", why: "holds_a_draft", term });
      seen.draft = [...document.querySelectorAll(".toast")].at(-1)?.textContent ?? "";
      seen.draftExpected = t("explain.state.failed", "설명 페이지를 만들지 못했어요 · {{why}}", {
        why: t("term.withheld.holdsADraft", "터미널 {{term}}에 입력 중인 글이 있어 에이전트에게 보낼 메시지를 넣지 않았습니다.", { term }),
      });
      return seen;
    }, diff.term);
    ok(
      "a request that waits for a busy pane says so and can be cancelled, and a pane refusing for the person's draft is said in the sentence the window already has",
      waiting.waiting === waiting.waitingExpected && waiting.cancelWord === "취소" && waiting.cancelled && waiting.hidden &&
        waiting.draft === waiting.draftExpected,
      JSON.stringify(waiting),
    );

    /* ---- 2. 대화의 한 턴 ---- */
    // 하네스의 에이전트 목록에는 zo가 없다 — 판의 에이전트를 이름 붙일 수 없으면 카드는 그 판의 행을 내지 않는다.
    await standZo(page);
    const turn = await page.evaluate(async () => {
      const seen = {};
      // 와이어 없는 에이전트(zo) — 판의 대화가 전사 쪽인 길이고, 같은 길을 `pane-conversation`이 쓴다.
      const term = await openTermTab({ placement: "tab" });
      window.__AGENT_TERMS__ = [[term, "zo"]];
      paneAgents.set(term, "zo");
      hookStates.set(term, "working");
      window.__ANSWER__.pane_log = () => ({
        found: true, next: 3, skipped: false, more: false, folded: false, model: "claude-opus-5",
        turns: [
          { role: "user", text: "이 화면 버그 좀 봐줘." },
          { role: "assistant", text: "원인은 스트리밍 경로입니다.\n\n그래서 한 번에 그렸습니다." },
        ],
      });
      await setPaneChat(term, true);
      await new Promise((done) => setTimeout(done, 300));
      const answer = document.querySelector(".helper-turn.is-assistant");
      const person = document.querySelector(".helper-turn.is-user");
      seen.term = term;
      seen.cwd = answer?.__run?.cwd ?? tabOfTerm(term)?.worktree ?? null;
      seen.beforeTouch = document.querySelectorAll(".helper-actions .explain-open").length;
      seen.copy = Boolean(answer?.querySelector(":scope > .helper-actions > .helper-copy"));
      // 포인터가 처음 닿으면 한 번 짓는다.
      answer.dispatchEvent(new PointerEvent("pointerover", { bubbles: true }));
      seen.afterTouch = answer.querySelectorAll(":scope > .helper-actions > .explain-open").length;
      answer.dispatchEvent(new PointerEvent("pointerover", { bubbles: true }));
      seen.afterSecond = answer.querySelectorAll(".explain-open").length;
      seen.onPerson = person?.querySelectorAll(".explain-open").length ?? -1;
      seen.label = answer.querySelector(".explain-open")?.getAttribute("aria-label") ?? "";
      seen.expectedLabel = t("explain.tip.turn", "이 대화를 그림·페이지로 설명");

      answer.querySelector(".explain-open")?.click();
      await window.__SETTLE__();
      const preview = window.__EXPLAIN__.previews.at(-1);
      seen.preview = preview;
      seen.names = [...document.querySelectorAll("#note-pop .note-pop-row .note-pop-name")].map((one) => one.textContent);
      document.querySelector("#note-pop .note-pop-row")?.click();
      await window.__SETTLE__();
      seen.request = window.__EXPLAIN__.starts.at(-1);
      // 이 길로 청한 페이지도 발행되면 탭으로 열린다 — 세 자리가 같은 소식 길을 쓴다.
      window.__SAY__({ id: seen.request?.id, state: "ready", term, agent: "zo", artifact: window.__PAGE__(term, "p-explain-turn") });
      await window.__SETTLE__();
      seen.opened = tabs.filter((one) => one.kind === "browser" && one.artifact?.id === "p-explain-turn").length;
      return seen;
    });
    ok(
      "a conversation's answer wears no button until the pointer first reaches it, then exactly one, and the person's own rows none",
      turn.beforeTouch === 0 && turn.copy && turn.afterTouch === 1 && turn.afterSecond === 1 && turn.onPerson === 0 &&
        turn.label === turn.expectedLabel,
      JSON.stringify(turn),
    );
    ok(
      "a turn goes with the person's question before it, to the pane the conversation lives in, and the page the agent publishes opens",
      turn.preview?.kind === "turn" &&
        turn.preview?.text === "Person:\n이 화면 버그 좀 봐줘.\n\nAgent:\n원인은 스트리밍 경로입니다.\n\n그래서 한 번에 그렸습니다." &&
        turn.names[0] === "ZO" &&
        turn.request?.kind === "turn" && turn.request?.title === "원인은 스트리밍 경로입니다." &&
        turn.request?.route?.via === "conversation" && turn.request?.route?.term === turn.term && turn.opened === 1,
      JSON.stringify({ preview: turn.preview, request: turn.request }),
    );

    /* 키보드: 복사 단추에 초점이 가면 다음 탭이 설명 단추다. */
    const keys = await page.evaluate(async () => {
      const answer = document.querySelector(".helper-turn.is-assistant");
      answer.querySelector(".explain-open")?.remove();
      const copy = answer.querySelector(":scope > .helper-actions > .helper-copy");
      copy.focus();
      const made = answer.querySelector(":scope > .helper-actions > .explain-open");
      return { made: Boolean(made), afterCopy: copy.nextElementSibling === made, reachable: made ? made.tabIndex >= 0 : false };
    });
    ok(
      "focusing the answer's copy button makes the explain button, right after it in the tab order",
      keys.made && keys.afterCopy && keys.reachable,
      JSON.stringify(keys),
    );

    /* ---- 3. 보드의 과업 보고 ---- */
    const report = await page.evaluate(async () => {
      const seen = {};
      const term = await openTermTab();
      window.__AGENT_TERMS__ = [[term, "codex"]];
      const actions = document.createElement("div");
      document.body.append(actions);
      const entity = { card: { pane: `term:${term}` }, place: { workerId: "w-9" }, facts: { identity: "w-9 · codex" } };
      artifactCounts = { ...(artifactCounts ?? {}), report_by_worker: {} };
      paintAgentInspectorReport(actions, entity);
      seen.noReport = actions.querySelector(".explain-open")?.hidden ?? "absent";
      artifactCounts = { ...artifactCounts, report_by_worker: { "w-9": "rep-1" } };
      paintAgentInspectorReport(actions, entity);
      const open = actions.querySelector(".explain-open");
      seen.shown = open ? !open.hidden : false;
      seen.kind = open?.dataset.explainKind ?? "";
      seen.once = actions.querySelectorAll(".explain-open").length;
      seen.next = open?.previousElementSibling?.classList.contains("agent-inspector-report") ?? false;
      open?.click();
      await window.__SETTLE__();
      seen.preview = window.__EXPLAIN__.previews.at(-1);
      seen.names = [...document.querySelectorAll("#note-pop .note-pop-row .note-pop-name")].map((one) => one.textContent);
      document.querySelector("#note-pop .note-pop-row")?.click();
      await window.__SETTLE__();
      seen.request = window.__EXPLAIN__.starts.at(-1);
      seen.term = term;
      window.__SAY__({ id: seen.request?.id, state: "ready", term, agent: "codex", artifact: window.__PAGE__(term, "p-explain-report") });
      await window.__SETTLE__();
      seen.opened = tabs.filter((one) => one.kind === "browser" && one.artifact?.id === "p-explain-report").length;
      actions.remove();
      return seen;
    });
    ok(
      "the board's report door makes no explain button while the worker left no report",
      report.noReport === "absent",
      JSON.stringify(report),
    );
    ok(
      "a report goes by its artifact id to the worker's pane, the backend reads the file, and the page the agent publishes opens",
      report.shown && report.kind === "report" && report.once === 1 && report.next &&
        report.preview?.kind === "report" && report.preview?.report === "rep-1" && report.preview?.text === null &&
        report.names[0] === "Codex" &&
        report.request?.kind === "report" && report.request?.report === "rep-1" && report.request?.text === null &&
        report.request?.title === "w-9 · codex" && report.request?.route?.term === report.term && report.opened === 1,
      JSON.stringify(report),
    );

    /* ---- 어디서 나온 요청인지: 창이 아는 것만 싣는다 ----
     * 한 번 실행이 만든 페이지는 창이 직접 발행하므로 출처를 창이 채운다. 원장이 보증하는 것만
     * 쓰이도록, 요청은 사람이 보던 판과 폴더를 모르면 비워 보낸다(추정 금지): diff는 판이 아니라
     * 체크아웃에 속하고, 보고의 판은 그 보고의 좌석이며, 대화의 한 턴은 읽던 판에 속한다. */
    ok(
      "each entry point says where its request came from — a diff its checkout and no pane, a turn its pane and folder, a report its seat and no folder",
      sent.request?.from?.term === null && sent.request?.from?.cwd === diff.worktree &&
        typeof diff.worktree === "string" && diff.worktree !== "" &&
        turn.request?.from?.term === turn.term && turn.request?.from?.cwd === turn.cwd &&
        typeof turn.cwd === "string" && turn.cwd !== "" &&
        report.request?.from?.term === report.term && report.request?.from?.cwd === null,
      JSON.stringify({
        diff: sent.request?.from, turn: turn.request?.from, report: report.request?.from,
        worktree: diff.worktree, cwd: turn.cwd,
      }),
    );

    /* ---- 다섯 언어: 모든 낱말이 카탈로그에 있다 ---- */
    const words = await page.evaluate(async ({ locales, whyTokens, source }) => {
      const seen = { missing: [], same: [], quota: {}, headline: {}, whyMissing: [] };
      const keys = [...new Set([...source.matchAll(/"(explain\.[A-Za-z0-9._]+)"/g)].map((hit) => hit[1]))];
      seen.keyCount = keys.length;
      for (const code of ["en", "ja", "zh", "es"]) {
        for (const key of keys) {
          if (typeof CATALOG[code]?.[key] !== "string" || CATALOG[code][key] === "") seen.missing.push(`${code}:${key}`);
        }
      }
      for (const token of whyTokens) {
        const guard = typeof TERM_WITHHELD !== "undefined" && Boolean(TERM_WITHHELD[token]);
        if (!guard && !keys.includes(`explain.why.${token}`)) seen.whyMissing.push(token);
      }
      for (const code of locales) {
        setLocale(code, { persist: false, refresh: false });
        seen.quota[code] = t("explain.quota", "구독 한도를 씁니다");
        seen.headline[code] = t("explain.headline", "그림·페이지로 설명해 주세요");
        for (const key of ["explain.tip.diff", "explain.title", "explain.state.failed"]) {
          if (code !== "ko" && t(key, "한국어") === "한국어") seen.same.push(`${code}:${key}`);
        }
        if (typeof explainWhyWords !== "function" || explainWhyWords("quota_wall") === "") seen.same.push(`${code}:quota_wall`);
      }
      setLocale("ko", { persist: false, refresh: false });
      return seen;
    }, { locales: LOCALES, whyTokens, source: script });
    ok(
      "every word the action says is in the four catalogs, and every reason the backend can give is worded in the window",
      words.keyCount > 20 && words.missing.length === 0 && words.whyMissing.length === 0 && words.same.length === 0,
      JSON.stringify({ keys: words.keyCount, missing: words.missing, whyMissing: words.whyMissing, same: words.same }),
    );
    ok(
      "the quota note and the first line of the request read right in all five languages",
      LOCALES.every((code) => words.quota[code] === QUOTA_WORDS[code] && words.headline[code] === HEADLINE_WORDS[code]),
      JSON.stringify({ quota: words.quota, headline: words.headline }),
    );

    /* ---- 새 타이머도 폴러도 없다 ---- */
    ok(
      "the explain script builds no timer, no interval and no frame loop — a request ends by an event, never by a clock",
      script.length > 1000 && !/\bsetInterval\(/.test(script) && !/\bsetTimeout\(/.test(script) && !/requestAnimationFrame\(/.test(script),
      `${script.length} bytes`,
    );

    ok("no page error was raised by any of it", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}

if (process.argv[1] && import.meta.url === (await import("node:url")).pathToFileURL(process.argv[1]).href) {
  const { chromium, createWindowServer } = await import("./window-boot.mjs");
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch();
  try {
    await testExplain(browser, origin, (name, pass, detail) => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name} ${detail}`);
      if (!pass) process.exitCode = 1;
    });
  } finally {
    await browser.close();
    files.close();
  }
}
