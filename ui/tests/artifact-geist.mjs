/* The artifact tab on a wide screen, in Geist (2026-11-06) — what the person's 27-inch window shows: a
 * 2480×1332 harness window puts the tab at the 1,849px the person's 2560pt window measured.
 *
 *   WINDOW_SUITES=artifact-geist node ui/tests/window.mjs
 *
 * The reader takes half the tab and reads at 16px with the prose held to a reading measure, a table's
 * hashes and paths never break inside a word, the clear sits beside the picker it clears, a zero count
 * says nothing; the grip moves the reader by drag and by key and remembers, wide reading folds the list
 * and keeps the outline open beside the text; the flow's state and the task list's time stand beside
 * their words; an open task's strip widens with the tab; a screenshot card wears its small picture; the
 * 「기타」 tab's cards are word-faced; a search that empties the tab says so and clears. Synthetic rows
 * only — paths use the /Users/dev placeholder. */
import { openWindowTestPage } from "./window-boot.mjs";
import { installHarnessWaits } from "./harness-waits.mjs";
import { standCatalog } from "./artifact-cards.mjs";
import { standTasks } from "./artifact-tasks.mjs";
import { standFlow, openFlow } from "./artifact-flow.mjs";

const WIDE_WINDOW = Object.freeze({ width: 2480, height: 1332 });
/* The reader's share of a wide tab and its floor (tokens.css `--artifact-drawer-share`, `--artifact-drawer-width`). */
const DRAWER_SHARE = 0.5;
const DRAWER_FLOOR = 560;
const WIDTH_SLACK = 16;
/* The reading measure (`--artifact-prose-max`, 48rem) and the wide reader's type. */
const PROSE_MAX = 768;
const WIDE_READ_PX = 16;
/* How far beside its words a row's state may stand: the 48rem column, the state's own gap and a margin. */
const ROW_STATE_MAX = 900;
/* The grip's keyboard step (`--artifact-grip-step`). */
const GRIP_STEP = 24;
const READER_KEY = "zerocode.artifacts.reader.v1";

/* A review report with the shape the person's screenshot had: two tables, hashes and paths in code. */
const REPORT = [
  "# QUEUE-FIX10-VERIFY: 마지막 검증 결과", "",
  "## 1. 결론", "", "막는 지적은 0건이다. 동작 결함, 높음, 중간이 하나도 없다. 낮음은 4건이다.", "",
  "| 저장소 | T9 | T10 | T9 → T10 |", "|---|---|---|---|",
  "| 서버 `/Users/dev/acme/order-service` | `558aa96f` | `249d061c` (커밋 `214ebfb1`) | 27개 파일 |",
  "| 웹 `/Users/dev/acme/order-web` | `82542e06` | `31179429` (커밋 `d51451dd`) | 16개 파일 |", "",
  ...Array.from({ length: 7 }, (_, at) => [`## ${at + 2}. 절 ${at + 2}`, "", "닫을 때 큐에 남은 일을 비우고, 비운 수를 돌려준다. ".repeat(48), ""]).flat(),
].join("\n");

export async function testArtifactGeist(browser, origin, ok) {
  /* ---- the reader, the head, the grip, wide reading ------------------------------------------------ */
  {
    const { page, faults } = await openWindowTestPage(browser, origin);
    try {
      await page.setViewportSize(WIDE_WINDOW);
      await installHarnessWaits(page);
      const seen = await page.evaluate(async ({ report, key }) => {
        try { localStorage.removeItem(key); } catch { /* the store may be closed */ }
        artifactReader.width = null;
        artifactReader.wide = false;
        const now = Date.now();
        const team = "/Users/dev/zerocode/workspaces/work/team-fdd122387e65be20ceb27b4e";
        const row = (id, title, at, text) => ({
          id, kind: "report", subtype: "review", title, path: `/Users/dev/zerocode/reports/t-501/${id}.md`,
          bytes: 4096, created_ms: now - at * 3600e3, modified_ms: now - at * 3600e3,
          origin: { run: "run-9", task: "t-501", worker: `w-${id}`, agent: "claude", model: "claude-opus-5-5", worktree: team },
          tags: [], preview: { kind: "markdown", text }, source: "worker_report",
        });
        const rows = [row("r-1", "QUEUE-FIX10-VERIFY: 마지막 검증 결과", 30, report.slice(0, 1200))];
        window.__ANSWER__.artifacts_list = () => ({
          rows, total: rows.length, truncated: false, missing: [], missing_total: 0, missing_by_kind: {}, unlinked_by_kind: {},
          by_kind: { report: 1 }, retention_days: 30, thumb: { width: 320, height: 240, queue_max: 2 },
        });
        window.__ANSWER__.artifact_preview = () => ({ kind: "markdown", text: report, bytes: report.length, truncated: false });
        const reveals = [];
        window.__ANSWER__.artifact_reveal = (args) => { reveals.push(args); return null; };
        dropTab("artifacts");
        artifactTabPicked = false;
        Object.assign(artifactFilter, { tab: "reports", origin: { field: "task", value: "t-501", label: "t-501" }, query: "", project: ARTIFACT_ALL_PROJECTS, agent: null, period: "all" });
        openArtifacts({ fresh: true });
        await window.__PAINTED__();
        await window.__UNTIL__(() => !artifactAsking && artifactRows.has("r-1"), "the report row");
        const view = artifactsView();
        selectArtifact(view, "r-1", { reveal: true });
        await window.__UNTIL__(() => view.querySelector(".artifact-preview-md .md-table") !== null, "the report's body");
        await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
        const shown = (node) => Boolean(node) && !node.closest("[hidden]") && node.getClientRects().length > 0;
        const box = (node) => node.getBoundingClientRect();
        const body = view.querySelector(".artifacts-body");
        const drawer = view.querySelector(".artifacts-drawer");
        const out = { body: Math.round(body.clientWidth), drawer: Math.round(box(drawer).width) };
        const para = view.querySelector(".artifact-preview-md .md-para");
        out.readPx = parseFloat(getComputedStyle(para).fontSize);
        out.prose = Math.round(box(para).width);
        // A code token is broken where its line changes anywhere but right after a slash.
        const range = document.createRange();
        const midBreaks = (code) => {
          let last = null;
          let before = "";
          let bad = 0;
          const walk = document.createTreeWalker(code, NodeFilter.SHOW_TEXT);
          for (let node = walk.nextNode(); node; node = walk.nextNode()) {
            for (let at = 0; at < node.textContent.length; at += 1) {
              range.setStart(node, at);
              range.setEnd(node, at + 1);
              const top = Math.round(range.getBoundingClientRect().top);
              if (last !== null && top > last + 2 && before !== "/") bad += 1;
              last = top;
              before = node.textContent[at];
            }
          }
          return bad;
        };
        const codes = [...view.querySelectorAll(".artifact-preview-md .md-table .md-code")];
        out.codes = codes.length;
        out.broken = codes.filter((code) => midBreaks(code) > 0).length;
        out.copied = codes.map((code) => code.textContent).join(" ");
        // The head: the clear beside its picker, the search held at its right end, a zero count silent.
        const pick = view.querySelector('[data-artifact-pick="task"]').closest(".artifacts-pick");
        const clear = view.querySelector(".artifacts-origin-clear");
        out.clearGap = Math.round(box(clear).left - box(pick).right);
        out.searchWidth = Math.round(box(view.querySelector(".artifacts-query")).width);
        out.zeroCounts = [...view.querySelectorAll(".artifacts-tab:not([hidden]) .artifacts-tab-count")].filter((count) => count.textContent === "0" && shown(count)).length;
        // The card: its conclusion, its model, no folder name and no emoji in its time.
        const card = view.querySelector('.artifact-card[data-id="r-1"]');
        out.lead = card.querySelector(".artifact-card-lead").textContent;
        out.maker = card.querySelector(".artifact-card-maker").textContent;
        out.when = card.querySelector(".artifact-card-when").textContent;
        // The head's quiet actions act on the row: Finder.
        view.querySelector(".artifact-detail-reveal").click();
        await new Promise((done) => setTimeout(done, 30));
        out.revealed = reveals.length;
        // The grip: a separator that says the drawer's width; a key widens it by a step and is kept;
        // a drag moves it by the drag; a double-click goes back to half the tab.
        const grip = view.querySelector(".artifacts-grip");
        out.gripShown = shown(grip);
        out.gripRole = grip.getAttribute("role");
        out.gripNow = Number(grip.getAttribute("aria-valuenow"));
        const before = box(drawer).width;
        grip.focus();
        grip.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true }));
        await window.__PAINTED__();
        out.keyStep = Math.round(box(drawer).width - before);
        out.keptWidth = JSON.parse(localStorage.getItem(key) ?? "{}").width ?? null;
        const at = box(grip);
        const pointer = (type, x) => grip.dispatchEvent(new PointerEvent(type, { bubbles: true, pointerId: 7, button: 0, clientX: x, clientY: at.top + 20 }));
        const dragFrom = box(drawer).width;
        grip.setPointerCapture = () => {};
        pointer("pointerdown", at.left);
        pointer("pointermove", at.left - 100);
        pointer("pointerup", at.left - 100);
        await window.__PAINTED__();
        out.dragged = Math.round(box(drawer).width - dragFrom);
        grip.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
        await window.__PAINTED__();
        out.reset = Math.round(box(drawer).width);
        out.keptAfterReset = JSON.parse(localStorage.getItem(key) ?? "{}").width ?? null;
        // Wide reading folds the list, keeps the outline open beside the text, and Esc comes back.
        view.querySelector(".artifact-detail-wide").click();
        await window.__PAINTED__();
        out.wide = view.classList.contains("is-reading-wide");
        out.listFolded = getComputedStyle(view.querySelector(".artifacts-main")).display === "none";
        out.wideDrawer = Math.round(box(drawer).width);
        const rail = view.querySelector(".artifact-outline-list");
        out.railOpen = shown(rail) && rail.querySelectorAll("button.is-now").length === 1;
        out.railBeside = shown(rail) && box(rail).left > box(para).right;
        out.keptWide = JSON.parse(localStorage.getItem(key) ?? "{}").wide === true;
        drawer.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
        await window.__PAINTED__();
        out.backFromWide = !view.classList.contains("is-reading-wide") && getComputedStyle(view.querySelector(".artifacts-main")).display !== "none";
        out.keptNarrow = JSON.parse(localStorage.getItem(key) ?? "{}").wide === false;
        return out;
      }, { report: REPORT, key: READER_KEY });
      const share = Math.max(DRAWER_FLOOR, seen.body * DRAWER_SHARE);
      ok(
        "on a 27-inch tab the reader takes half the tab (560px at least) and reads at 16px with the prose held to the 48rem measure",
        Math.abs(seen.drawer - share) <= WIDTH_SLACK && seen.readPx === WIDE_READ_PX && seen.prose <= PROSE_MAX + 1,
        JSON.stringify({ body: seen.body, drawer: seen.drawer, expected: share, readPx: seen.readPx, prose: seen.prose }),
      );
      ok(
        "a table's hashes and paths never break inside a word — a path breaks only after a slash — and copying them gives the words unchanged",
        seen.codes >= 6 && seen.broken === 0 && seen.copied.includes("/Users/dev/acme/order-service") && seen.copied.includes("558aa96f"),
        JSON.stringify({ codes: seen.codes, broken: seen.broken }),
      );
      ok(
        "the task filter's clear stands beside the picker it clears, the search is held to its width at the end, and a zero count says nothing",
        seen.clearGap >= 0 && seen.clearGap <= 16 && seen.searchWidth <= 360 && seen.zeroCounts === 0,
        JSON.stringify({ clearGap: seen.clearGap, search: seen.searchWidth, zeroCounts: seen.zeroCounts }),
      );
      ok(
        "a report card says its conclusion, its model rather than a folder's name, and its time without an emoji mark",
        seen.lead.startsWith("막는 지적은 0건이다") && seen.maker.includes("opus-5-5") && !seen.maker.includes("team-") && !/[🌐🔒]/u.test(seen.when),
        JSON.stringify({ lead: seen.lead, maker: seen.maker, when: seen.when }),
      );
      ok("the reader's head shows Finder without opening its details, and it acts on the row", seen.revealed === 1, JSON.stringify({ revealed: seen.revealed }));
      ok(
        "the grip is a separator that says the reader's width; a key widens it by one step and the width is kept, a drag moves it by the drag, and a double-click goes back to half the tab",
        seen.gripShown && seen.gripRole === "separator" && Math.abs(seen.gripNow - seen.drawer) <= 2
          && Math.abs(seen.keyStep - GRIP_STEP) <= 2 && seen.keptWidth !== null
          && Math.abs(seen.dragged - 100) <= 4
          && Math.abs(seen.reset - share) <= WIDTH_SLACK && seen.keptAfterReset === null,
        JSON.stringify({ shown: seen.gripShown, role: seen.gripRole, now: seen.gripNow, step: seen.keyStep, kept: seen.keptWidth, dragged: seen.dragged, reset: seen.reset, keptAfterReset: seen.keptAfterReset }),
      );
      ok(
        "wide reading folds the list, gives the reader the whole tab with the outline open beside the text, is remembered, and Esc brings the list back",
        seen.wide && seen.listFolded && Math.abs(seen.wideDrawer - seen.body) <= 2 && seen.railOpen && seen.railBeside && seen.keptWide
          && seen.backFromWide && seen.keptNarrow,
        JSON.stringify({ wide: seen.wide, folded: seen.listFolded, drawer: seen.wideDrawer, body: seen.body, railOpen: seen.railOpen, beside: seen.railBeside, kept: seen.keptWide, back: seen.backFromWide, keptNarrow: seen.keptNarrow }),
      );
      ok("the wide reader raised no renderer fault", faults.length === 0, faults.slice(0, 2).join(" | "));
    } finally {
      await page.close();
    }
  }

  /* ---- the other tabs: flow, tasks, evidence, 기타, an emptied search --------------------------------- */
  {
    const { page, faults } = await openWindowTestPage(browser, origin);
    try {
      await page.setViewportSize(WIDE_WINDOW);
      await installHarnessWaits(page);
      await standCatalog(page);
      await standTasks(page);
      await standFlow(page);
      await page.evaluate(() => {
        // A transcript row for the 「기타」 tab, and a picture for the screenshot card.
        const list = window.__ANSWER__.artifacts_list;
        const now = Date.now();
        window.__ANSWER__.artifacts_list = (args) => {
          const answer = list(args);
          const talk = {
            id: "x-talk", kind: "transcript", title: "설계 대화", path: "/Users/dev/zerocode/transcripts/x-talk.jsonl", bytes: 900,
            created_ms: now - 26 * 3600e3, modified_ms: now - 26 * 3600e3, origin: { agent: "claude", model: "claude-opus-5-5" },
            tags: [], preview: { kind: "text", text: "{\"role\":\"user\"}" }, source: "transcript",
          };
          return { ...answer, rows: [...answer.rows, talk], total: answer.total + 1, by_kind: { ...answer.by_kind, transcript: 1 } };
        };
        const PICTURE = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
        window.__ANSWER__.artifact_thumbnail = (args) => ({ data_url: artifactRows.get(args.id)?.kind === "screenshot" ? PICTURE : null, cached: true });
      });
      await openFlow(page);
      const flow = await page.evaluate(() => {
        const view = artifactsView();
        const row = [...view.querySelectorAll(".artifact-flow-item")].find((one) => one.querySelector(".artifact-flow-state-word")?.textContent);
        const said = row.querySelector(".artifact-flow-said").getBoundingClientRect();
        const state = row.querySelector(".artifact-flow-state-word").getBoundingClientRect();
        const id = row.querySelector(".artifact-flow-id").getBoundingClientRect();
        return { tab: Math.round(view.querySelector(".artifacts-body").clientWidth), state: Math.round(state.left - said.left), id: Math.round(id.left - said.left) };
      });
      ok(
        "on a 27-inch tab a flow row's state and task id stand beside its sentence, not at the tab's far edge",
        flow.tab > 1700 && flow.state > 0 && flow.state <= ROW_STATE_MAX && flow.id > 0 && flow.id <= ROW_STATE_MAX,
        JSON.stringify(flow),
      );
      const tasks = await page.evaluate(async () => {
        const view = artifactsView();
        closeArtifactFlowReading();
        artifactSelectedId = null;
        view.querySelector('[data-artifact-tab="tasks"]').click();
        await window.__PAINTED__();
        await window.__UNTIL__(() => view.querySelectorAll(".artifact-task").length > 0, "the task rows");
        const first = view.querySelector(".artifact-task");
        const title = first.querySelector(".artifact-task-title").getBoundingClientRect();
        const when = first.querySelector(".artifact-task-when").getBoundingClientRect();
        const out = { tab: Math.round(view.querySelector(".artifacts-body").clientWidth), when: Math.round(when.left - title.left) };
        openArtifactTask(view, "t-501", { reveal: true });
        await window.__UNTIL__(() => view.querySelector(".artifact-preview-md .md-para") !== null, "the task's report");
        await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
        out.strip = Math.round(view.querySelector(".artifacts-main").getBoundingClientRect().width);
        out.cut = [...view.querySelectorAll(".artifact-task-title")].filter((one) => one.getClientRects().length && one.scrollWidth > one.clientWidth + 1).length;
        // `cut` reads this machine's Korean face only. Windows draws Hangul in Malgun Gothic, wider than macOS's
        // Apple SD Gothic Neo (CI 2026-11-06: cut 1 on Windows, 0 on macOS). Geist draws a title's spaces, dashes and
        // digits on every system and no Korean face draws a syllable wider than its em, so a title whose Geist part plus
        // one em per syllable fits its column is whole in any face; `wideCut` counts the titles that would not be.
        const ruler = document.createElement("span");
        ruler.style.cssText = "position: absolute; visibility: hidden; white-space: pre";
        document.body.append(ruler);
        out.wideCut = [...view.querySelectorAll(".artifact-task-title")].filter((one) => {
          if (!one.getClientRects().length) return false;
          const look = getComputedStyle(one);
          for (const key of ["fontFamily", "fontSize", "fontWeight", "fontStyle", "fontStretch", "letterSpacing", "fontFeatureSettings"]) ruler.style[key] = look[key];
          const syllables = (one.textContent.match(/[가-힣]/g) ?? []).length;
          ruler.textContent = one.textContent.replace(/[가-힣]/g, "");
          const em = parseFloat(look.fontSize) + (parseFloat(look.letterSpacing) || 0);
          return ruler.getBoundingClientRect().width + syllables * em > one.clientWidth + 1;
        }).length;
        ruler.remove();
        const para = view.querySelector(".artifact-preview-md .md-para");
        out.prose = Math.round(para.getBoundingClientRect().width);
        out.readPx = parseFloat(getComputedStyle(para).fontSize);
        const rail = view.querySelector(".artifact-bundle-evidence");
        out.railGap = rail && rail.getClientRects().length ? Math.round(rail.getBoundingClientRect().left - view.querySelector(".artifact-preview-md").getBoundingClientRect().right) : null;
        closeArtifactTask();
        return out;
      });
      // The strip's width (`--artifact-task-strip-width`, clamp(200px, 20%, 360px)).
      const strip = Math.min(360, Math.max(200, tasks.tab * 0.2));
      ok(
        "on a 27-inch tab a task's time stands beside its title; an open task's strip widens with the tab and cuts no title, even with Hangul a full em wide as Windows draws it, and its report reads at 16px within the measure with the evidence beside the text",
        tasks.when > 0 && tasks.when <= ROW_STATE_MAX && Math.abs(tasks.strip - strip) <= 4 && tasks.cut === 0 && tasks.wideCut === 0
          && tasks.prose <= PROSE_MAX + 1 && tasks.readPx === WIDE_READ_PX && tasks.railGap !== null && tasks.railGap >= 0 && tasks.railGap <= 48,
        JSON.stringify({ ...tasks, expectedStrip: strip }),
      );
      const cards = await page.evaluate(async () => {
        const view = artifactsView();
        artifactSelectedId = null;
        // The flow stood on the project on screen (`openFlow`); these cards come from several projects and from none,
        // and what is measured here is their faces, so 「전체」 is picked as the other artifact suites do.
        artifactFilter.project = ARTIFACT_ALL_PROJECTS;
        view.querySelector('[data-artifact-tab="evidence"]').click();
        await window.__PAINTED__();
        await window.__UNTIL__(() => !artifactThumbBusy && artifactThumbQueue.length === 0, "the screenshot pictures");
        await window.__PAINTED__();
        const shot = view.querySelector('.artifact-card[data-kind="screenshot"]');
        const out = {
          shotThumb: Boolean(shot?.classList.contains("has-thumb")) && shot.querySelector(".artifact-card-thumb").hidden === false,
          maker: view.querySelector('.artifact-card[data-id="e-steps"] .artifact-card-maker')?.textContent ?? "",
        };
        view.querySelector('[data-artifact-tab="other"]').click();
        await window.__PAINTED__();
        const talk = view.querySelector('.artifact-card[data-id="x-talk"]');
        out.otherPlain = Boolean(talk?.classList.contains("is-plain"));
        out.otherLeadHidden = talk?.querySelector(".artifact-card-lead").hidden === true;
        view.querySelector('[data-artifact-tab="pages"]').click();
        await window.__PAINTED__();
        const query = view.querySelector(".artifacts-query");
        query.value = "zzzz";
        query.dispatchEvent(new Event("input", { bubbles: true }));
        await new Promise((done) => setTimeout(done, 400));
        const empty = view.querySelector(".artifacts-empty");
        out.emptyWord = empty.querySelector(".artifacts-empty-word").textContent;
        out.emptyAct = empty.querySelector(".artifacts-empty-act").textContent;
        // Enter searched the text too and found nothing either (v1.1.129 A2).
        out.titleHint = empty.querySelector(".artifacts-empty-hint").textContent;
        const askedBefore = window.__ANSWER__.artifact_search;
        window.__ANSWER__.artifact_search = () => ({ rows: [], missing: [] });
        await searchArtifactBodies(view);
        await window.__PAINTED__();
        out.bodyHint = empty.querySelector(".artifacts-empty-hint").textContent;
        // A body search still on its way when the query changes is dropped: the
        // new query was searched by title only (review t-98866 P3-3).
        let release = () => {};
        window.__ANSWER__.artifact_search = () => new Promise((done) => { release = () => done({ rows: [], missing: [] }); });
        const late = searchArtifactBodies(view);
        query.value = "zzzzq";
        query.dispatchEvent(new Event("input", { bubbles: true }));
        await new Promise((done) => setTimeout(done, 400));
        release();
        await late;
        await window.__PAINTED__();
        out.lateHint = empty.querySelector(".artifacts-empty-hint").textContent;
        window.__ANSWER__.artifact_search = askedBefore;
        empty.querySelector(".artifacts-empty-act").click();
        await new Promise((done) => setTimeout(done, 400));
        out.cleared = query.value === "" && artifactOrder.length > 0 && empty.hidden;
        return out;
      });
      ok(
        "a screenshot card wears the small picture the runtime made, and a Computer Use record names its maker",
        cards.shotThumb && cards.maker.includes("Computer Use"),
        JSON.stringify({ shotThumb: cards.shotThumb, maker: cards.maker }),
      );
      ok("the 「기타」 tab's cards are word-faced and carry no conclusion line from a raw transcript", cards.otherPlain && cards.otherLeadHidden, JSON.stringify(cards));
      ok(
        "a search that empties the tab says what was searched and offers to clear it, and clearing brings the cards back",
        cards.emptyWord.includes("zzzz") && cards.emptyAct === "검색 지우기" && cards.cleared,
        JSON.stringify({ word: cards.emptyWord, act: cards.emptyAct, cleared: cards.cleared }),
      );
      ok(
        "after Enter searched the text too and found nothing, the empty screen no longer asks for Enter (v1.1.129 A2)",
        cards.titleHint.includes("Enter") && cards.bodyHint !== "" && !cards.bodyHint.includes("Enter"),
        JSON.stringify({ title: cards.titleHint, body: cards.bodyHint }),
      );
      ok(
        "a body search answered after the query changed does not tell the new query its text was searched (review t-98866 P3-3)",
        cards.lateHint === cards.titleHint,
        JSON.stringify({ late: cards.lateHint, title: cards.titleHint }),
      );
      ok("the other tabs raised no renderer fault", faults.length === 0, faults.slice(0, 2).join(" | "));
    } finally {
      await page.close();
    }
  }
}
