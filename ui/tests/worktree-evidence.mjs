import { openWindowTestPage } from "./window-boot.mjs";
import { workspaceBoardFixture } from "./workspace-board.mjs";

/* 워크스페이스 근거 화면 — 진짜 렌더러로.
 *
 * Rust 게이트는 이 화면의 말이 맞는지 읽을 수 있지만, 그 말이 화면에 실제로
 * 서는지는 레이아웃을 수행한 브라우저만 안다. 여기서 재는 것 넷:
 *
 * - 네 상태(읽음·없음·지원 안 함·읽지 못함)가 서로 다르게 읽히는가
 * - 묻지 않으면 묻지 않는가(닫힌 동안 조회 0, 같은 질문은 한 번)
 * - 늦게 온 답이 주인이 바뀐 화면을 덮지 않는가
 * - 다크·라이트·좁은 창·키보드에서 서는가
 * - git이 무시하는 파일이 「커밋하지 않은 내용」으로 불리지 않는가(t-34315): 폴더
 *   세 모양(무시된 파일만·진짜 변경만·둘 다)과 깨끗한 폴더가 말하는 문장,
 *   틈의 이름이 다섯 언어의 사람 말로 서는가, 사이드바 뱃지와 같은 낱말인가 */

const EVIDENCE = {
  schemaVersion: 1,
  observedAtMs: 1_700_000_000_000,
  workspaceId: "workspace-0123456789abcdef",
  repositoryId: "repo-abc",
  worktreeId: "worktree-abc",
  branch: "feature/agent-board",
  summary: {
    dirty: true,
    changedPaths: 2,
    executions: 1,
    openExecutions: 0,
    receiptsCurrent: 1,
    receiptsStale: 1,
    receiptsUnknown: 0,
    receiptsCurrentPassing: 1,
    coordinatorReports: 1,
    decisions: 1,
    unrecorded: ["tool_approvals", "ci_checks"],
  },
  snapshot: {
    state: "ok",
    freshness: "observed",
    coverage: { counted: 2, returned: 2, truncated: false },
    data: {
      headOid: "d966b814fcb28ae740c876472b5cedb389c5d73d",
      contentDigest: "e8a9597596ee3baef9dc13d8684ac6ca1e60e3b2273cd7a0105a212342a4b19a",
      detached: false,
      locked: false,
      dirty: true,
      complete: false,
      coverageGaps: ["untracked_content"],
      changes: [
        { path: "ui/shell-workspace.js", code: " M", staged: false, unstaged: true, untracked: false, conflicted: false },
        // 한 줄에 사람이 읽을 수 없는 것이 와도 화면은 글로만 넣는다.
        { path: "<img src=x onerror=alert(1)>/\u{1f4c1}/very/long/path/that/keeps/going/and/going/and/going/and/going.rs",
          code: "??", staged: false, unstaged: false, untracked: true, conflicted: true },
      ],
    },
  },
  executions: {
    state: "ok",
    freshness: "observed",
    coverage: { counted: 1, returned: 1, truncated: false },
    data: [{
      runId: "run-1", dispatchId: "d-2", workerId: "w-1", agent: "codex",
      taskId: "t-1", task: "make it work", startedMs: 12, endedMs: 30, open: false,
      workerReportedSuccess: true, retryOf: "d-1", model: "gpt-6-astra", effort: "xhigh",
    }],
  },
  reports: {
    state: "ok",
    freshness: "observed",
    coverage: { counted: 1, returned: 1, truncated: false },
    data: [{
      kind: "coordinator_report", runId: "run-1", taskId: "t-1", task: "make it work",
      verified: true, merged: false, deployed: false, written: true,
    }],
  },
  verification: {
    state: "ok",
    freshness: "observed",
    coverage: { counted: 4, returned: 2, truncated: true },
    data: [
      { source: "host_trusted", name: "gate", commandId: "just-verify", exitCode: 0,
        passed: true, currency: "current", currencyReason: "digest_matches",
        startedAtMs: 100, endedAtMs: 200, outputTruncated: false },
      { source: "manifest", name: "unit", commandId: "just-test", exitCode: 0,
        passed: true, currency: "stale", currencyReason: "content_changed",
        startedAtMs: 10, endedAtMs: 20, outputTruncated: false },
    ],
  },
  decisions: {
    state: "ok",
    freshness: "observed",
    coverage: { counted: 1, returned: 1, truncated: false },
    data: [{ kind: "workflow_review", decision: "approved", state: "approved",
      subject: "reviewer-bob", atMs: 400 }],
  },
  ci: { state: "unsupported", freshness: "unobserved", coverage: { counted: 0, returned: 0, truncated: false }, data: [] },
};

/// 저장소가 없는 창의 답 — 「없음」이 아니라 「이 창에 없는 자료」.
const WITHOUT_A_STORE = {
  ...EVIDENCE,
  summary: { ...EVIDENCE.summary, receiptsCurrent: 0, receiptsStale: 0, receiptsCurrentPassing: 0, decisions: 0 },
  verification: { state: "missing", freshness: "unobserved", coverage: { counted: 0, returned: 0, truncated: false }, data: [] },
  decisions: { state: "error", freshness: "unobserved", coverage: { counted: 0, returned: 0, truncated: false },
    error: { code: "store_unavailable", message: "the authority store could not be read", retryable: true }, data: [] },
};

/* 폴더 세 모양의 합성 답 (t-34315). 숫자와 이름은 지어낸 것이다 — 사람의
 * 폴더에서 온 것이 아니다.
 *
 * 사람이 본 화면: 모든 커밋이 main에 들어가 있고 추적하는 파일의 변경도, 새
 * 파일도 없는, git이 무시하는 파일(node_modules·빌드 결과·docs/·output/)만
 * 270 MB 남은 폴더. 사이드바는 「반영됨」인데 근거 화면은 「커밋되지 않은
 * 내용이 있습니다 · … ignored_content」였다.
 *
 * `dirty`는 옛 뜻 그대로(변경 또는 무시된 파일)다 — 새 사실 둘은 `uncommitted`
 * 와 `ignored`다. 옛 코드에 이 답을 먹이면 사람이 본 그 문장이 그려진다. */
const IGNORED_LEFT = {
  count: 5, bytes: 283_115_520, sizeText: "270 MB", complete: true,
  top: [
    { name: "node_modules", bytes: 188_743_680, sizeText: "180 MB", dir: true },
    { name: "target", bytes: 62_914_560, sizeText: "60 MB", dir: true },
    { name: "output", bytes: 20_971_520, sizeText: "20 MB", dir: true },
  ],
};
const NOTHING_IGNORED = { count: 0, bytes: 0, sizeText: "0 B", complete: true, top: [] };
const A_CHANGE = { path: "ui/shell-workspace.js", code: " M", staged: false, unstaged: true, untracked: false, conflicted: false };
const A_NEW_FILE = { path: "notes/draft.md", code: "??", staged: false, unstaged: false, untracked: true, conflicted: false };

function folderLike({ changes, ignored, gaps }) {
  const answer = structuredClone(EVIDENCE);
  answer.snapshot.data = {
    ...answer.snapshot.data,
    dirty: changes.length > 0 || ignored.count > 0,
    uncommitted: changes.length > 0,
    ignored,
    complete: gaps.length === 0,
    coverageGaps: gaps,
    changes,
  };
  answer.snapshot.coverage = { counted: changes.length, returned: changes.length, truncated: false };
  answer.summary.dirty = answer.snapshot.data.dirty;
  answer.summary.changedPaths = changes.length;
  return answer;
}

const ONLY_IGNORED = folderLike({ changes: [], ignored: IGNORED_LEFT, gaps: ["ignored_content"] });
const ONLY_CHANGES = folderLike({ changes: [A_CHANGE, A_NEW_FILE], ignored: NOTHING_IGNORED, gaps: ["untracked_content"] });
const BOTH = folderLike({ changes: [A_CHANGE, A_NEW_FILE], ignored: IGNORED_LEFT, gaps: ["ignored_content", "untracked_content"] });
const CLEAN = folderLike({ changes: [], ignored: NOTHING_IGNORED, gaps: [] });
const SIZE_IS_A_FLOOR = folderLike({ changes: [], ignored: { ...IGNORED_LEFT, complete: false }, gaps: ["ignored_content"] });

/* 백엔드가 지금 보낼 수 있는 틈의 이름 여섯. 여섯이 맞는지는 Rust 소스 계약이
 * `gap_tag`에서 읽어 지킨다 — 여기서는 각각이 말이 되는지만 본다. */
const GAP_TAGS = ["ignored_content", "untracked_content", "changed_path_limit",
  "content_byte_limit", "dirty_submodule", "index_hidden_content"];
const LOCALES = ["ko", "en", "ja", "zh", "es"];

/* 이 답을 화면에 앉히고 「변경」 절이 말하는 것을 문장 단위로 읽는다. */
const readChangesSection = (page, answer) => page.evaluate(async (held) => {
  window.__EVIDENCE_ANSWER__ = held;
  await refreshWorktreeEvidence();
  const section = document.querySelector('[data-evidence-section="changes"]');
  const said = (node, selector) => node?.querySelector(selector)?.textContent ?? null;
  const rows = [...section.querySelectorAll(".wt-evidence-rows > .wt-evidence-row")];
  const ignored = section.querySelector("[data-evidence-ignored]");
  return {
    text: section.textContent,
    head: said(rows[0], ".wt-evidence-strong"),
    warn: [...section.querySelectorAll(".wt-evidence-warn")].map((one) => one.textContent),
    changes: rows.length - 1 - (ignored ? 1 : 0),
    ignored: ignored && {
      label: said(ignored, ".wt-evidence-strong"),
      line: said(ignored, ".wt-evidence-line"),
      names: said(ignored, ".wt-evidence-path"),
      more: said(ignored, ".wt-evidence-note"),
      caveat: said(ignored, ".wt-evidence-caveat"),
      last: rows[rows.length - 1] === ignored,
    },
  };
}, answer);

function installEvidence(page, answer) {
  return page.evaluate((held) => {
    window.__EVIDENCE_CALLS__ = [];
    window.__EVIDENCE_HELD__ = [];
    window.__ANSWER__.worktree_evidence = (args) => {
      window.__EVIDENCE_CALLS__.push(args);
      if (window.__EVIDENCE_PARK__) {
        return new Promise((settle, refuse) => {
          window.__EVIDENCE_HELD__.push({ args, settle, refuse });
        });
      }
      if (window.__EVIDENCE_REFUSAL__) throw window.__EVIDENCE_REFUSAL__;
      const answer = structuredClone(window.__EVIDENCE_ANSWER__ ?? held);
      // 진짜 백엔드처럼 읽을 때마다 관측 시각이 새로 찍힌다.
      answer.observedAtMs += window.__EVIDENCE_CALLS__.length * 1000;
      return answer;
    };
    window.__EVIDENCE_ANSWER__ = structuredClone(held);
  }, answer);
}

const openFromCard = async (page, path) => {
  await page.click(`[data-worktree-path="${path}"] .workspace-board-card-menu`);
  await page.click("#sidebar-menu button:has-text('근거 보기'), #sidebar-menu button:has-text('View evidence')");
  await page.waitForSelector("#wt-evidence-scrim:not([hidden])");
};

export async function testWorktreeEvidence(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(workspaceBoardFixture);
    await page.waitForSelector(".workspace-board-card");
    await installEvidence(page, EVIDENCE);

    /* 닫혀 있는 동안은 아무것도 묻지 않는다. 보드는 몇 번을 다시 그려도
     * 근거를 한 번도 묻지 않는다 — 이 화면에는 시계가 없다. */
    const quiet = await page.evaluate(async () => {
      for (let i = 0; i < 20; i++) paintWorkspaceBoard();
      await new Promise((done) => setTimeout(done, 350));
      return window.__EVIDENCE_CALLS__.length;
    });
    ok("근거는 열기 전에는 한 번도 묻지 않는다(폴링 없음)", quiet === 0, `calls=${quiet}`);

    await openFromCard(page, "/projects/zerocode/0");
    await page.waitForSelector('[data-evidence-section="changes"]');

    const read = await page.evaluate(() => {
      const body = document.getElementById("wt-evidence-body");
      const badge = (section) => body
        .querySelector(`[data-evidence-section="${section}"] .wt-evidence-badge`)
        .dataset.evidenceState;
      return {
        calls: window.__EVIDENCE_CALLS__.length,
        asked: window.__EVIDENCE_CALLS__[0],
        sections: [...body.querySelectorAll("[data-evidence-section]")].map((one) => one.dataset.evidenceSection),
        changes: badge("changes"),
        ci: badge("ci"),
        currencies: [...body.querySelectorAll("[data-evidence-currency]")]
          .map((one) => one.dataset.evidenceCurrency),
        caveats: [...body.querySelectorAll(".wt-evidence-caveat")].map((one) => one.textContent),
        truncation: body.querySelector('[data-evidence-section="verification"] .wt-evidence-count').textContent,
        // 「검증했다고 적음」 옆에 유보 문장이 붙어 있는가.
        reportRow: body.querySelector('[data-evidence-section="reports"] .wt-evidence-row').textContent,
        // 화면에 들어온 위험한 글은 글로만 있다: 요소도 속성도 생기지 않고,
        // 글자 그대로 읽힌다.
        injected: body.querySelector("img, [onerror]") === null,
        literal: [...body.querySelectorAll(".wt-evidence-path")]
          .some((one) => one.textContent.startsWith("<img src=x onerror=alert(1)>")),
      };
    });
    ok("카드 메뉴가 그 워크스페이스의 경로로 한 번 묻는다",
      read.calls === 1 && read.asked.path === "/projects/zerocode/0" && read.asked.host === null,
      JSON.stringify(read.asked));
    ok("여섯 섹션이 순서대로 선다",
      JSON.stringify(read.sections) === JSON.stringify(["changes", "executions", "verification", "reports", "decisions", "ci"]),
      JSON.stringify(read.sections));
    ok("CI는 「지원하지 않음」이고 변경은 「읽음」이다 — 없음과 구분된다",
      read.changes === "ok" && read.ci === "unsupported", JSON.stringify(read));
    ok("영수증의 신선도가 행마다 붙는다(일치·낡음)",
      read.currencies.filter((one) => one === "current").length === 2
      && read.currencies.filter((one) => one === "stale").length === 2,
      JSON.stringify(read.currencies));
    ok("잘림은 숨기지 않고 숫자로 말한다", read.truncation === "2 / 4", read.truncation);
    ok("코디네이터 보고 행은 시험 결과가 아니라고 스스로 말한다",
      /시험 결과가 아닙니다|not a test result/.test(read.reportRow), read.reportRow);
    ok("기록되지 않은 것을 요약이 이름으로 말한다",
      read.caveats.some((one) => /도구 승인|tool approvals/.test(one)), JSON.stringify(read.caveats));
    ok("행의 글은 글로만 들어간다(마크업 주입 없음)", read.injected && read.literal, JSON.stringify(read));

    /* 같은 질문은 한 번만 난다. */
    const singleFlight = await page.evaluate(async () => {
      window.__EVIDENCE_PARK__ = true;
      const before = window.__EVIDENCE_CALLS__.length;
      const first = refreshWorktreeEvidence();
      const second = refreshWorktreeEvidence();
      const third = readWorktreeEvidence(worktreeEvidenceSubject);
      const calls = window.__EVIDENCE_CALLS__.length - before;
      for (const held of window.__EVIDENCE_HELD__.splice(0)) {
        held.settle(structuredClone(window.__EVIDENCE_ANSWER__));
      }
      await Promise.all([first, second, third]);
      window.__EVIDENCE_PARK__ = false;
      return calls;
    });
    ok("날아가 있는 조회가 있으면 같은 질문을 다시 내지 않는다", singleFlight === 1, `calls=${singleFlight}`);

    /* A를 보다 B로 옮기면, 늦게 온 A의 답은 버려진다. */
    const discarded = await page.evaluate(async () => {
      window.__EVIDENCE_PARK__ = true;
      const first = projectOfWorktree("/projects/zerocode/0");
      const a = first.worktrees.find((one) => one.path === "/projects/zerocode/0");
      const b = first.worktrees.find((one) => one.path === "/projects/zerocode/1");
      openWorktreeEvidence(a);
      const held = window.__EVIDENCE_HELD__.splice(0);
      openWorktreeEvidence(b);
      const forB = window.__EVIDENCE_HELD__.splice(0);
      // A가 뒤늦게 도착한다.
      for (const one of held) {
        one.settle({ ...structuredClone(window.__EVIDENCE_ANSWER__), branch: "STALE-A" });
      }
      await new Promise((done) => setTimeout(done, 20));
      const afterLateA = document.getElementById("wt-evidence-body").dataset.evidenceStatus;
      for (const one of forB) {
        one.settle({ ...structuredClone(window.__EVIDENCE_ANSWER__), branch: "FRESH-B" });
      }
      await new Promise((done) => setTimeout(done, 20));
      window.__EVIDENCE_PARK__ = false;
      return { afterLateA, shown: worktreeEvidenceView.evidence.branch,
        subject: document.getElementById("wt-evidence-subject").textContent };
    });
    ok("workspace를 옮긴 뒤 늦게 온 앞 답은 화면을 덮지 않는다",
      discarded.afterLateA === "reading" && discarded.shown === "FRESH-B", JSON.stringify(discarded));

    /* 바뀌지 않은 답은 DOM을 건드리지 않는다 — 포커스도 스크롤도 그대로.
     * 관측 시각만 달라진 답이 바로 그 경우다: 머리의 시각은 바뀌고 몸통은
     * 그대로 선다. */
    await page.setViewportSize({ width: 1280, height: 520 });
    const stable = await page.evaluate(async () => {
      const body = document.getElementById("wt-evidence-body");
      // 앞 사례가 남긴 답(「FRESH-B」)은 픽스처와 실제로 다르다. 한 번 읽어
      // 픽스처의 답에 앉힌 뒤부터 잰다 — 그래야 뒤의 두 번이 「같은 답」이다.
      await refreshWorktreeEvidence();
      const clock = el("wt-evidence-observed").textContent;
      body.scrollTop = Math.max(1, body.scrollHeight - body.clientHeight);
      const scrolled = body.scrollTop;
      const first = body.querySelector(".wt-evidence-row");
      el("wt-evidence-refresh").focus();
      const before = { node: first, focus: document.activeElement, children: body.children.length };
      const calls = window.__EVIDENCE_CALLS__.length;
      await refreshWorktreeEvidence();
      await refreshWorktreeEvidence();
      return {
        asked: window.__EVIDENCE_CALLS__.length - calls,
        same: before.node === body.querySelector(".wt-evidence-row"),
        focus: document.activeElement === before.focus,
        children: body.children.length === before.children,
        scrolled, scroll: body.scrollTop,
        clockMoved: el("wt-evidence-observed").textContent !== clock,
        busy: body.getAttribute("aria-busy"),
      };
    });
    ok("변화 없는 다시 읽기는 DOM·포커스·스크롤을 보존하고 관측 시각만 바꾼다",
      stable.asked === 2 && stable.same && stable.focus && stable.children
      && stable.scrolled > 0 && stable.scroll === stable.scrolled && stable.clockMoved
      && stable.busy === null, JSON.stringify(stable));
    await page.setViewportSize({ width: 1280, height: 900 });

    /* 자료가 없는 창과 읽지 못한 저장소는 서로 다르게 읽힌다. */
    const absences = await page.evaluate(async (without) => {
      window.__EVIDENCE_ANSWER__ = without;
      await refreshWorktreeEvidence();
      const body = document.getElementById("wt-evidence-body");
      const badge = (section) => body
        .querySelector(`[data-evidence-section="${section}"] .wt-evidence-badge`)
        .dataset.evidenceState;
      return {
        verification: badge("verification"),
        decisions: badge("decisions"),
        why: body.querySelector('[data-evidence-section="decisions"] .wt-evidence-why')?.dataset.evidenceCode,
        words: body.querySelector('[data-evidence-section="decisions"] .wt-evidence-why')?.textContent,
      };
    }, WITHOUT_A_STORE);
    ok("자료 없음과 읽지 못함은 다른 딱지를 단다",
      absences.verification === "missing" && absences.decisions === "error"
      && absences.why === "store_unavailable" && Boolean(absences.words),
      JSON.stringify(absences));

    /* 백엔드가 거절하면 창은 문장을 지어내지 않는다. */
    const refused = await page.evaluate(async () => {
      window.__EVIDENCE_REFUSAL__ = { code: "not_catalogued", message: "x", retryable: false };
      await refreshWorktreeEvidence();
      const body = document.getElementById("wt-evidence-body");
      window.__EVIDENCE_REFUSAL__ = null;
      return { status: body.dataset.evidenceStatus,
        code: body.querySelector(".wt-evidence-why")?.dataset.evidenceCode,
        words: body.textContent };
    });
    ok("거절은 코드와 함께 사람의 문장으로 선다",
      refused.status === "refused" && refused.code === "not_catalogued"
      && /프로젝트 목록에 없는|not in this window/.test(refused.words), JSON.stringify(refused));

    /* 원격 체크아웃은 호스트와 함께 묻는다 — 이 디스크에서 찾지 않는다. */
    const remote = await page.evaluate(async () => {
      window.__EVIDENCE_ANSWER__ = structuredClone(window.__EVIDENCE_ANSWER__);
      const project = window.__ANSWER__.project_catalog().find((one) => one.name === "acme");
      const worktree = project.worktrees[0];
      const before = window.__EVIDENCE_CALLS__.length;
      openWorktreeEvidence(worktree);
      await new Promise((done) => setTimeout(done, 20));
      return window.__EVIDENCE_CALLS__[before];
    });
    ok("원격 워크스페이스는 호스트를 실어 묻는다",
      remote.host === "acme@edge-ssh" && remote.path === "/projects/acme/0", JSON.stringify(remote));

    /* 키보드: 열면 「다시 읽기」가 키를 받고, Escape가 닫는다. */
    await page.evaluate((held) => { window.__EVIDENCE_ANSWER__ = held; }, EVIDENCE);
    await page.evaluate(() => closeWorktreeEvidence());
    await openFromCard(page, "/projects/zerocode/0");
    await page.waitForSelector('[data-evidence-section="changes"]');
    const focused = await page.evaluate(() => document.activeElement?.id);
    await page.keyboard.press("Escape");
    await page.waitForFunction(() => el("wt-evidence-scrim").hidden);
    const closed = await page.evaluate(() => ({
      hidden: el("wt-evidence-scrim").hidden,
      subject: worktreeEvidenceSubject,
    }));
    ok("열면 「다시 읽기」가 키를 받고 Escape가 닫는다",
      focused === "wt-evidence-refresh" && closed.hidden && closed.subject === null,
      JSON.stringify({ focused, ...closed }));

    /* 사이드바의 워크트리 메뉴도 같은 화면을 연다 — 그 행의 경로로. 보드는
     * 닫지 않는다: 사이드바는 보드 옆에 그대로 서 있고, 보드를 다시 열면
     * 카드가 들어오는 애니메이션이 다음 사례의 클릭을 흔든다. */
    const fromSidebar = await page.evaluate(async () => {
      const before = window.__EVIDENCE_CALLS__.length;
      const row = document.querySelector('.wt-row[data-worktree-path="/projects/zerocode/1"]');
      const box = row.getBoundingClientRect();
      row.dispatchEvent(new MouseEvent("contextmenu", {
        bubbles: true, cancelable: true, clientX: box.left + 10, clientY: box.top + 5,
      }));
      const item = [...document.querySelectorAll("#sidebar-menu button")]
        .find((one) => /근거 보기|View evidence/.test(one.textContent));
      item?.click();
      await new Promise((done) => setTimeout(done, 30));
      return { found: Boolean(item), open: !el("wt-evidence-scrim").hidden,
        asked: window.__EVIDENCE_CALLS__.slice(before) };
    });
    ok("사이드바 워크트리 메뉴가 그 행의 근거를 연다",
      fromSidebar.found && fromSidebar.open && fromSidebar.asked.length === 1
      && fromSidebar.asked[0].path === "/projects/zerocode/1", JSON.stringify(fromSidebar));
    await page.evaluate(() => closeWorktreeEvidence());

    /* 좁은 창에서 가로로 넘치지 않는다. */
    await openFromCard(page, "/projects/zerocode/0");
    await page.waitForSelector('[data-evidence-section="changes"]');
    for (const width of [1280, 900, 420]) {
      await page.setViewportSize({ width, height: 900 });
      await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
      const size = await page.evaluate(() => {
        const body = document.getElementById("wt-evidence-body");
        const dialog = document.querySelector(".qo--evidence");
        const box = dialog.getBoundingClientRect();
        const actions = [...dialog.querySelectorAll(".wt-evidence-actions .btn")]
          .map((one) => one.getBoundingClientRect());
        return { body: body.clientWidth, bodyScroll: body.scrollWidth,
          left: Math.round(box.left), right: Math.round(box.right),
          page: document.documentElement.clientWidth,
          pageScroll: document.documentElement.scrollWidth,
          // 머리의 버튼이 대화 상자 안에 서는가 — 폭만 재면 오른쪽으로 밀려난
          // 상자를 놓친다(420px에서 실제로 그랬다).
          actionsInside: actions.every((one) => one.left >= box.left - 1 && one.right <= box.right + 1) };
      });
      ok(`${width}px에서 근거 화면이 가로로 넘치지 않는다`,
        size.bodyScroll <= size.body + 1 && size.left >= 0 && size.right <= size.page
        && size.pageScroll <= size.page + 1 && size.actionsInside, JSON.stringify(size));
      await page.screenshot({ path: `output/playwright/worktree-evidence/evidence-${width}.png` });
    }

    /* ---- 무시된 파일은 「커밋하지 않은 내용」이 아니다 (t-34315) ----
     *
     * 두 사실이다: git이 변경으로 센 것(커밋하지 않은 변경)과, git이 무시하는
     * 파일이 남아 있다는 것(폴더를 지우면 함께 사라지는 것). 폴더 세 모양과
     * 깨끗한 폴더 하나가 말하는 문장을 글자 그대로 못 박는다. */
    const shown = {
      onlyIgnored: await readChangesSection(page, ONLY_IGNORED),
      onlyChanges: await readChangesSection(page, ONLY_CHANGES),
      both: await readChangesSection(page, BOTH),
      clean: await readChangesSection(page, CLEAN),
      floor: await readChangesSection(page, SIZE_IS_A_FLOOR),
    };
    const NO_TAG = (text) => !GAP_TAGS.some((tag) => text.includes(tag));
    const IGNORED_SENTENCES = {
      label: "무시된 파일 남음",
      line: "git이 무시하는 파일 5개 항목 · 270 MB",
      names: "node_modules/ (180 MB) · target/ (60 MB) · output/ (20 MB)",
      more: "외 2개",
      caveat: "폴더를 지우면 함께 사라지며, 시험 영수증과 비교하는 내용에는 들어가지 않습니다",
    };
    const sameSentences = (one, want) => Object.entries(want).every(([key, text]) => one?.[key] === text);
    ok("무시된 파일만 남은 폴더: 「커밋하지 않은 변경이 없습니다」와 무시된 파일 한 줄의 사실 — 경고 줄도 날것의 이름도 없다",
      shown.onlyIgnored.head === "커밋하지 않은 변경이 없습니다" && shown.onlyIgnored.warn.length === 0
      && shown.onlyIgnored.changes === 0 && sameSentences(shown.onlyIgnored.ignored, IGNORED_SENTENCES)
      && !shown.onlyIgnored.text.includes("커밋되지 않은") && NO_TAG(shown.onlyIgnored.text),
      JSON.stringify(shown.onlyIgnored));
    ok("진짜 변경만 있는 폴더: 「커밋하지 않은 변경이 있습니다」와 변경 두 줄 — 무시된 파일 줄은 없고, 읽지 못한 부분은 사람 말로 선다",
      shown.onlyChanges.head === "커밋하지 않은 변경이 있습니다" && shown.onlyChanges.changes === 2
      && shown.onlyChanges.ignored === null
      && JSON.stringify(shown.onlyChanges.warn) === JSON.stringify(["다 읽지는 못했습니다 — 새 파일의 내용은 읽지 않았습니다"])
      && NO_TAG(shown.onlyChanges.text),
      JSON.stringify(shown.onlyChanges));
    ok("둘 다 있는 폴더: 두 사실이 따로 선다 — 변경 줄들 뒤에 무시된 파일 줄, 그리고 틈 줄은 무시된 파일을 되풀이하지 않는다",
      shown.both.head === "커밋하지 않은 변경이 있습니다" && shown.both.changes === 2
      && sameSentences(shown.both.ignored, IGNORED_SENTENCES) && shown.both.ignored.last
      && JSON.stringify(shown.both.warn) === JSON.stringify(["다 읽지는 못했습니다 — 새 파일의 내용은 읽지 않았습니다"])
      && NO_TAG(shown.both.text),
      JSON.stringify(shown.both));
    ok("깨끗한 폴더: 「커밋하지 않은 변경이 없습니다」 한 줄뿐 — 경고도 무시된 파일 줄도 없다",
      shown.clean.head === "커밋하지 않은 변경이 없습니다" && shown.clean.warn.length === 0
      && shown.clean.changes === 0 && shown.clean.ignored === null,
      JSON.stringify(shown.clean));
    ok("무시된 파일의 크기를 끝까지 못 쟀으면 숫자를 「이상」으로 말한다 — 모자란 숫자를 정확한 것처럼 적지 않는다",
      shown.floor.ignored?.line === "git이 무시하는 파일 5개 항목 · 270 MB 이상",
      JSON.stringify(shown.floor.ignored));

    /* 틈의 이름은 다섯 언어 모두에서 사람 말이다: 날것의 이름이 화면에 닿지 않고,
     * 한국어가 다른 언어 화면으로 새지 않고, 여섯이 서로 다른 말이고, 백엔드가
     * 아직 모르는 이름도 말로 나온다. */
    const gapWords = await page.evaluate(async ({ held, tags, locales }) => {
      const out = {};
      for (const code of locales) {
        setLocale(code, { persist: false });
        out[code] = {};
        for (const tag of [...tags, "a_gap_kind_from_the_future"]) {
          const answer = structuredClone(held);
          answer.snapshot.data = {
            ...answer.snapshot.data, uncommitted: false, complete: false, coverageGaps: [tag], changes: [],
            ignored: { count: 0, bytes: 0, sizeText: "0 B", complete: true, top: [] },
          };
          window.__EVIDENCE_ANSWER__ = answer;
          await refreshWorktreeEvidence();
          out[code][tag] = [...document.querySelectorAll('[data-evidence-section="changes"] .wt-evidence-warn')]
            .map((one) => one.textContent);
        }
      }
      setLocale("ko", { persist: false });
      return out;
    }, { held: ONLY_CHANGES, tags: GAP_TAGS, locales: LOCALES });
    const everyTag = [...GAP_TAGS, "a_gap_kind_from_the_future"];
    const hangul = /[가-힣]/;
    const wordless = [];
    for (const code of LOCALES) {
      for (const tag of everyTag) {
        const lines = gapWords[code][tag];
        const line = lines?.[0] ?? "";
        if (lines?.length !== 1 || line === "") wordless.push(`${code} ${tag}: ${JSON.stringify(lines)}`);
        else if (line.includes(tag)) wordless.push(`${code} ${tag}: the raw tag reached the screen`);
        else if (code !== "ko" && hangul.test(line)) wordless.push(`${code} ${tag}: Korean leaked into this language`);
        else if (code !== "ko" && line === gapWords.ko[tag][0]) wordless.push(`${code} ${tag}: same as the Korean`);
      }
    }
    const koWords = everyTag.map((tag) => gapWords.ko[tag]?.[0]);
    ok("틈의 이름 여섯과 아직 모르는 이름은 다섯 언어 모두에서 사람 말로 선다 — 날것의 이름도, 새어 나온 한국어도 없다",
      wordless.length === 0 && new Set(koWords).size === koWords.length,
      JSON.stringify({ wordless, koWords }));

    /* 사이드바 뱃지와 근거 화면은 같은 낱말로 말한다: 뱃지의 끝말이 패널 행의
     * 이름표다 — 같은 함수가 같은 카탈로그 항목을 읽는다. */
    const sameWords = await page.evaluate(async ({ held, locales }) => {
      const out = {};
      for (const code of locales) {
        setLocale(code, { persist: false });
        const chip = worktreeLandingSay({
          state: "landed", detached: false, ahead: 0, dirty: false, ignored: true, compare_ref: "origin/main",
        }, {});
        window.__EVIDENCE_ANSWER__ = held;
        await refreshWorktreeEvidence();
        out[code] = {
          chip: chip.word,
          label: document.querySelector("[data-evidence-ignored] .wt-evidence-strong")?.textContent ?? null,
        };
      }
      setLocale("ko", { persist: false });
      return out;
    }, { held: ONLY_IGNORED, locales: LOCALES });
    ok("뱃지와 패널이 같은 말을 한다 — 다섯 언어 모두 뱃지가 「… · 〈패널 행의 이름표〉」로 끝난다",
      LOCALES.every((code) => sameWords[code].label && sameWords[code].chip.endsWith(` · ${sameWords[code].label}`)),
      JSON.stringify(sameWords));
    await page.evaluate((held) => { window.__EVIDENCE_ANSWER__ = held; return refreshWorktreeEvidence(); }, EVIDENCE);

    /* 사진: 옛 코드와 새 코드가 같은 합성 답을 어떻게 그리는지. 환경 변수가
     * 있을 때만 찍는다 — 검사가 아니라 전/후 비교용이다. */
    const capture = process.env.WORKTREE_EVIDENCE_CAPTURE ?? null;
    if (capture) {
      for (const [name, answer] of [["only-ignored", ONLY_IGNORED], ["only-changes", ONLY_CHANGES], ["both", BOTH]]) {
        await readChangesSection(page, answer);
        for (const which of ["dark", "light"]) {
          await page.evaluate((wanted) => { theme = wanted; applyTheme(); }, which);
          await page.waitForTimeout(400);
          await page.locator(".qo--evidence").screenshot({ path: `${capture}/${name}-${which}.png`, animations: "disabled" });
        }
      }
      await page.evaluate((held) => { window.__EVIDENCE_ANSWER__ = held; return refreshWorktreeEvidence(); }, EVIDENCE);
    }

    /* 라이트와 다크 — 파일 이름이 아니라 계산된 스타일로 확인한다. */
    await page.setViewportSize({ width: 1280, height: 900 });
    /* 테마를 바꾸고, 색이 실제로 가라앉을 때까지 기다린다. 버튼 글자색에는
     * 150ms 전환이 걸려 있어서 배경만 보고 찍으면 전환 한가운데가 찍힌다 —
     * 첫 라이트 사진이 흰 버튼 위 흰 글자였다. 표본이 300ms 동안 그대로여야
     * 가라앉은 것이다. */
    const settleTheme = (wanted) => page.evaluate(async (wanted) => {
      const sample = () => {
        const refresh = getComputedStyle(el("wt-evidence-refresh"));
        const badge = getComputedStyle(document.querySelector('[data-evidence-section="changes"] .wt-evidence-badge'));
        const dialog = getComputedStyle(document.querySelector(".qo--evidence"));
        return [getComputedStyle(document.body).backgroundColor, dialog.backgroundColor,
          refresh.color, refresh.borderTopColor, badge.color].join(" | ");
      };
      // `applyTheme()`는 모듈의 `theme`를 읽는다 — 인자는 무시된다.
      theme = wanted; applyTheme();
      let last = sample();
      let since = performance.now();
      const deadline = since + 5000;
      while (performance.now() < deadline) {
        await new Promise((done) => requestAnimationFrame(done));
        const now = sample();
        if (now !== last) { last = now; since = performance.now(); continue; }
        if (performance.now() - since >= 300) break;
      }
      const refresh = getComputedStyle(el("wt-evidence-refresh"));
      return { root: document.documentElement.dataset.theme ?? null,
        body: getComputedStyle(document.body).backgroundColor,
        dialog: getComputedStyle(document.querySelector(".qo--evidence")).backgroundColor,
        refresh: refresh.color, settled: last };
    }, wanted);
    const light = await settleTheme("light");
    await page.screenshot({ path: "output/playwright/worktree-evidence/evidence-light.png" });
    const dark = await settleTheme("dark");
    await page.screenshot({ path: "output/playwright/worktree-evidence/evidence-dark.png" });
    ok("라이트 사진은 실제로 라이트다(가라앉은 계산 스타일이 다크와 다르다)",
      light.root === "light" && light.body !== dark.body && light.dialog !== dark.dialog
      && light.refresh !== dark.refresh, JSON.stringify({ light, dark }));

    /* 말은 언어를 따른다. */
    await page.evaluate(() => setLocale("en"));
    const english = await page.evaluate(() => ({
      title: el("wt-evidence-title").textContent,
      section: document.querySelector('[data-evidence-section="ci"] .wt-evidence-section-title').textContent,
      badge: document.querySelector('[data-evidence-section="ci"] .wt-evidence-badge').textContent,
      caveat: document.querySelector('[data-evidence-section="reports"] .wt-evidence-caveat').textContent,
    }));
    ok("근거 화면의 말은 언어를 따른다",
      english.title === "Workspace evidence" && english.badge === "Not supported"
      && english.caveat === "A coordinator's note, not a test result", JSON.stringify(english));
    await page.evaluate(() => setLocale("ko"));

    ok("근거 화면은 브라우저 오류를 내지 않는다", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}
