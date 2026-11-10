/* The artifacts' seams (2026-11-10): what a person could not see or reach.
 *
 *  - The 「새 N」 mark on the sidebar's artifact door was drawn and invisible wherever the destinations fold
 *    to marks (the conversation list's mode, the default since 2026-11-08): the rule that clips the names
 *    clipped the pill with them — measured 8×1 px. On a mark it is now a count, solid, at the corner.
 *  - The gallery's project was a name over an invisible select. It is a strip of chips: one press on a
 *    project lists that project's, with the tab's count on each chip; the rows that name no project have a
 *    chip of their own, and a switch of project in the sidebar is the newer word.
 *  - A card says it is unseen (「새」), says its project while every project is listed, and says how many
 *    pages of the second brain name it; the reader lists those pages and opens one.
 *  - 「새」 is a word about recent things: a mark older than a week is not kept, and 「모두 읽음」 takes the
 *    rest down at once.
 *
 *   WINDOW_SUITES=artifact-seams node ui/tests/window.mjs */
import { createRequire } from "node:module";
import { openWindowTestPage } from "./window-boot.mjs";

const HOME = "/tmp/zerocode-window-test";
const OTHER = "/tmp/zerocode-window-other/harbor";
const STRAY = "/elsewhere/stray-repo";
/* How long a theme's colours take to settle before axe reads them: the window eases a colour over its
 * fast beat, and a contrast read in the middle of that is neither theme's. */
const THEME_SETTLE_MS = 300;
/* What a person can read and press: the count on a mark, and a chip. */
const MARK_MIN_PX = 12;
const PRESS_MIN_PX = 28;
const TEXT_MIN_PX = 12;
/* 「새」 is kept a week (`ARTIFACT_UNSEEN_MAX_AGE_MS`); a mark a day past it is gone at the next load. */
const DAY_MS = 24 * 60 * 60 * 1000;

const axeViolations = async (page, AxeBuilder, include) => {
  const result = await new AxeBuilder({ page }).include(include).withTags(["wcag2a", "wcag2aa"]).analyze();
  return result.violations.map((one) => `${one.id}: ${one.nodes.map((node) => node.target.join(" ")).join(", ")}`);
};

/* The catalog of this suite, in the page: pages of three places and of none, a report, and the second
 * brain's citations of two of them. */
const stand = (page) => page.evaluate(async ({ home, other, stray }) => {
  const now = Date.now();
  const hour = 60 * 60 * 1000;
  projects = [
    ...projects,
    { name: "harbor", path: other, worktrees: [{ path: other, branch: "main", base: "", is_main: true, active: false, is_folder: false }] },
  ];
  const row = (id, kind, at, origin, more = {}) => ({
    id, kind, title: `${id} 제목`, path: `/data/artifacts/pages/${id}/index.html`, url: `file:///data/artifacts/pages/${id}/index.html`,
    bytes: 10, created_ms: now - at * hour, modified_ms: now - at * hour, version: kind === "page" ? 1 : undefined,
    origin, tags: [], preview: { kind: "none" }, source: "manual", ...more,
  });
  const rows = [
    row("p-home-new", "page", 1, { pane: "term-90", agent: "claude", project: home, worktree: home }),
    row("p-home-old", "page", 30, { agent: "claude", project: home }),
    row("p-harbor", "page", 2, { agent: "codex", project: other }),
    row("p-stray", "page", 3, { agent: "claude", project: stray }),
    row("p-nowhere", "page", 4, { pane: "term-91", agent: "claude" }),
    row("r-home", "report", 5, { agent: "claude", worktree: home }, { path: "/data/artifacts/run/r-home/report.md", subtype: "report" }),
  ];
  window.__SEAMS__ = { rows, opened: [] };
  window.__ANSWER__.artifacts_list = () => ({
    rows, total: rows.length, truncated: false, missing: [], missing_total: 0, missing_by_kind: {}, unlinked_by_kind: {},
    by_kind: {}, by_place: [], retention_days: 30, thumb: { width: 320, height: 240, queue_max: 2 },
  });
  window.__ANSWER__.artifact_counts = () => ({ total: rows.length, by_worker: {}, by_task: {}, by_run: {}, by_worktree: {}, by_automation: {}, report_by_worker: {} });
  window.__ANSWER__.artifact_thumbnail = () => ({ data_url: null, cached: false });
  window.__ANSWER__.artifact_preview = () => ({ kind: "none", bytes: 1, truncated: false });
  window.__ANSWER__.artifact_citations = () => ({
    "p-home-old": [
      { page: "wiki/gallery-seams.md", title: "갤러리의 세 이음매", path: "/vault/wiki/gallery-seams.md", modified_ms: 2 },
      { page: "wiki/board-draft.md", title: "상황판 시안", path: "/vault/wiki/board-draft.md", modified_ms: 1 },
    ],
    "p-gone": [{ page: "wiki/old.md", title: "옛 것", path: "/vault/wiki/old.md", modified_ms: 1 }],
  });
  // A vault page opens as a document tab — the suite only counts the asks.
  const read = window.__ANSWER__.read_text_file;
  window.__ANSWER__.read_text_file = (args) => {
    if (String(args?.path ?? "").startsWith("/vault/")) {
      window.__SEAMS__.opened.push(args.path);
      return { text: "# 갤러리의 세 이음매\n\n본문", version: 1 };
    }
    return read ? read(args) : { text: "", version: 1 };
  };
  localStorage.removeItem("zerocode.artifacts.unseen.v1");
  if (typeof clearArtifactUnseen === "function") clearArtifactUnseen();
}, { home: HOME, other: OTHER, stray: STRAY });

const publish = (page, ids) => page.evaluate((wanted) => {
  for (const id of wanted) {
    const held = window.__SEAMS__.rows.find((one) => one.id === id);
    // A worker's publication: nothing opens, the marks rise.
    const row = { ...held, origin: { ...held.origin, worker: "w-1" } };
    for (const speak of window.__LISTENERS__["artifacts:published"] ?? []) speak({ payload: row });
  }
}, ids);

const strip = (page) => page.evaluate(() => {
  const view = artifactsView();
  const chips = [...view.querySelectorAll(".artifacts-project-chip")];
  const group = view.querySelector(".artifacts-projects");
  return {
    role: group.getAttribute("role"),
    name: group.getAttribute("aria-label"),
    chips: chips.map((one) => `${one.dataset.project}=${one.querySelector(".artifacts-project-chip-name").textContent}:${one.querySelector(".artifacts-project-chip-count").hidden ? "" : one.querySelector(".artifacts-project-chip-count").textContent}`),
    checked: chips.filter((one) => one.getAttribute("aria-checked") === "true").map((one) => one.dataset.project),
    tabbable: chips.filter((one) => one.tabIndex === 0).map((one) => one.dataset.project),
    viewed: chips.filter((one) => one.classList.contains("is-viewed")).map((one) => one.dataset.project),
    viewedTip: chips.find((one) => one.classList.contains("is-viewed"))?.dataset.tip ?? "",
    heights: chips.map((one) => Math.round(one.getBoundingClientRect().height)),
    fonts: chips.map((one) => parseFloat(getComputedStyle(one).fontSize)),
    order: artifactOrder.join(","),
    tab: artifactFilter.tab,
    filter: artifactFilter.project,
    select: view.querySelector('[data-artifact-pick="project"]') !== null,
  };
});

export async function testArtifactSeams(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  const axeModule = createRequire(import.meta.url)("@axe-core/playwright");
  const AxeBuilder = axeModule.default ?? axeModule;
  try {
    await stand(page);

    /* ---- the 「새 N」 mark on a folded sidebar ---------------------------------------------------- */
    const mark = async (chatList) => {
      await page.evaluate((on) => {
        chatListSidebar = on;
        applyChatListSidebar();
      }, chatList);
      await page.evaluate(() => window.__PAINTED__());
      return page.evaluate(() => {
        const row = document.getElementById("nav-artifacts");
        const pill = row.querySelector(".nav-new");
        if (!pill) return { pill: false };
        const box = pill.getBoundingClientRect();
        const door = row.getBoundingClientRect();
        const drawn = getComputedStyle(pill, "::after");
        const name = row.querySelector(":scope > span:not(.nav-badge, .nav-new)");
        return {
          pill: true,
          words: pill.textContent,
          n: pill.dataset.n,
          box: [Math.round(box.width), Math.round(box.height)],
          insideDoor: box.left >= door.left - 1 && box.right <= door.right + 1 && box.top >= door.top - 1 && box.bottom <= door.bottom + 1,
          wordSize: parseFloat(getComputedStyle(pill).fontSize),
          countDrawn: drawn.content,
          countSize: parseFloat(drawn.fontSize),
          position: getComputedStyle(pill).position,
          nameClipped: name ? name.getBoundingClientRect().width <= 1 : null,
        };
      });
    };
    const quiet = await mark(true);
    ok("with nothing new the artifact door wears no mark", quiet.pill === false, JSON.stringify(quiet));
    await publish(page, ["p-home-new", "p-nowhere"]);
    const folded = await mark(true);
    ok(
      `on a folded sidebar the 「새 N」 mark is a count a person can see — at least ${MARK_MIN_PX}px each way, inside its door's corner, the number drawn and the word left to a screen reader`,
      folded.pill && folded.words === "새 2" && folded.n === "2" && folded.box[0] >= MARK_MIN_PX && folded.box[1] >= MARK_MIN_PX
        && folded.insideDoor && folded.position === "absolute" && folded.wordSize === 0
        && folded.countDrawn.startsWith('"2"') && folded.countSize >= 10 && folded.nameClipped === true,
      JSON.stringify(folded),
    );
    for (const theme of ["dark", "light"]) {
      await page.evaluate((next) => document.documentElement.setAttribute("data-theme", next), theme);
      await page.waitForTimeout(THEME_SETTLE_MS);
      const violations = await axeViolations(page, AxeBuilder, "#nav-artifacts").catch((error) => [String(error)]);
      ok(`the folded artifact door with its count passes axe in ${theme}`, violations.length === 0, JSON.stringify(violations));
    }
    await page.evaluate(() => document.documentElement.setAttribute("data-theme", "dark"));
    const wide = await mark(false);
    ok(
      "with the destinations named, the same mark is the words 「새 2」 beside the name and no number is drawn twice",
      wide.pill && wide.words === "새 2" && wide.wordSize >= 10 && wide.position !== "absolute" && wide.countDrawn === "none"
        && wide.nameClipped === false,
      JSON.stringify(wide),
    );

    /* ---- the door opens on what is new --------------------------------------------------------- */
    await page.evaluate(async () => {
      dropTab("artifacts");
      artifactTabPicked = false;
      Object.assign(artifactFilter, { tab: "flow", origin: null, project: null, agent: null, period: "all", showMissing: false, query: "" });
      el("nav-artifacts").click();
      await window.__PAINTED__();
      for (let tries = 0; tries < 200 && (artifactAsking || artifactRows.size === 0); tries += 1) await new Promise((done) => setTimeout(done, 20));
      await window.__PAINTED__();
    });
    const arrived = await strip(page);
    ok(
      "the door with a mark on it opens on the pages, every project listed — the new pages are in sight whatever tab and project stood before",
      arrived.tab === "pages" && arrived.filter === "*" && arrived.checked.join() === "*"
        && arrived.order === "p-home-new,p-harbor,p-stray,p-nowhere,p-home-old",
      JSON.stringify(arrived),
    );
    const cards = await page.evaluate(() => {
      const say = (id) => {
        const card = artifactCardNodeFor(id);
        const fresh = card.querySelector(".artifact-card-new");
        const cited = card.querySelector(".artifact-card-cited");
        const place = card.querySelector(".artifact-card-place");
        const when = card.querySelector(".artifact-card-when").getBoundingClientRect();
        const box = card.getBoundingClientRect();
        return {
          fresh: fresh.hidden ? "" : fresh.textContent,
          freshSize: fresh.hidden ? null : parseFloat(getComputedStyle(fresh).fontSize),
          cited: cited.hidden ? "" : cited.textContent,
          citedTip: cited.hidden ? "" : cited.dataset.tip,
          place: place.hidden ? "" : place.textContent,
          whenInside: when.right <= box.right + 0.5 && when.width > 0,
        };
      };
      const clear = artifactsView().querySelector(".artifacts-unseen-clear");
      return {
        fresh: say("p-home-new"), nowhere: say("p-nowhere"), old: say("p-home-old"), harbor: say("p-harbor"), stray: say("p-stray"),
        clear: clear.hidden ? "" : clear.textContent,
        clearHeight: Math.round(clear.getBoundingClientRect().height),
      };
    });
    ok(
      `an unseen page's card says 「새」 in ${TEXT_MIN_PX}px or more, and no other card does`,
      cards.fresh.fresh === "새" && cards.nowhere.fresh === "새" && cards.fresh.freshSize >= TEXT_MIN_PX && cards.old.fresh === "" && cards.harbor.fresh === "",
      JSON.stringify(cards),
    );
    ok(
      "with every project listed a card names its own — the window's name for a project it keeps, the folder's for one it does not, nothing for a row that names no place — and the time still stands",
      cards.fresh.place === "zerocode" && cards.harbor.place === "harbor" && cards.stray.place === "stray-repo" && cards.nowhere.place === ""
        && [cards.fresh, cards.harbor, cards.stray, cards.nowhere, cards.old].every((one) => one.whenInside),
      JSON.stringify(cards),
    );
    ok(
      "a card the second brain names says how many pages name it, with the sentence in its tooltip",
      cards.old.cited === "2" && cards.old.citedTip.includes("2") && cards.fresh.cited === "" && cards.harbor.cited === "",
      JSON.stringify(cards),
    );

    /* ---- the strip of projects ------------------------------------------------------------------ */
    ok(
      "the head's project is a strip of chips — everything, the one in view first, the others by their count, a folder the window does not know, the rows that name none — each with the tab's count, and no select behind them",
      arrived.role === "radiogroup" && arrived.name === "프로젝트" && arrived.select === false
        && arrived.chips.join("|") === `*=전체:5|${HOME}=zerocode:2|${OTHER}=harbor:1|${STRAY}=stray-repo:1|:none=프로젝트 없음:1`
        && arrived.viewed.join() === HOME && arrived.viewedTip === "지금 보는 프로젝트" && arrived.tabbable.join() === "*",
      JSON.stringify(arrived),
    );
    ok(
      `every chip is at least ${PRESS_MIN_PX}px high and its words ${TEXT_MIN_PX}px or more`,
      arrived.heights.every((px) => px >= PRESS_MIN_PX) && arrived.fonts.every((px) => px >= TEXT_MIN_PX),
      JSON.stringify({ heights: arrived.heights, fonts: arrived.fonts }),
    );
    const press = async (value) => {
      await page.evaluate(async (wanted) => {
        artifactsView().querySelector(`.artifacts-project-chip[data-project="${CSS.escape(wanted)}"]`).click();
        await window.__PAINTED__();
      }, value);
      return strip(page);
    };
    const harbor = await press(OTHER);
    const none = await press(":none");
    const home = await press(HOME);
    ok(
      "one press on a project lists that project's alone, and 「프로젝트 없음」 lists the rows that name no place",
      harbor.order === "p-harbor" && harbor.checked.join() === OTHER && harbor.tabbable.join() === OTHER
        && none.order === "p-nowhere" && none.checked.join() === ":none"
        && home.order === "p-home-new,p-home-old" && home.checked.join() === HOME,
      JSON.stringify({ harbor, none, home }),
    );
    const reports = await page.evaluate(async () => {
      artifactsView().querySelector('[data-artifact-tab="reports"]').click();
      await window.__PAINTED__();
    }).then(() => strip(page));
    ok(
      "a chip's count is its tab's: on the reports the projects with none leave the strip, the one in view stays",
      reports.chips.join("|") === `*=전체:1|${HOME}=zerocode:1` && reports.order === "r-home",
      JSON.stringify(reports),
    );
    const keys = await page.evaluate(async () => {
      const view = artifactsView();
      view.querySelector('[data-artifact-tab="pages"]').click();
      await window.__PAINTED__();
      const chip = view.querySelector('.artifacts-project-chip[aria-checked="true"]');
      chip.focus();
      const step = async (key) => {
        document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true }));
        await window.__PAINTED__();
        return `${document.activeElement?.dataset?.project ?? "?"}>${artifactOrder.join(",")}`;
      };
      return { start: chip.dataset.project, right: await step("ArrowRight"), end: await step("End"), wrap: await step("ArrowRight"), home: await step("Home") };
    });
    ok(
      "the arrows walk the strip and pick as they go, the focus riding with the pick — right, End, around the end, Home",
      keys.right === `${OTHER}>p-harbor` && keys.end === ":none>p-nowhere"
        && keys.wrap === "*>p-home-new,p-harbor,p-stray,p-nowhere,p-home-old" && keys.home === keys.wrap,
      JSON.stringify(keys),
    );
    for (const theme of ["dark", "light"]) {
      await page.evaluate((next) => document.documentElement.setAttribute("data-theme", next), theme);
      await page.waitForTimeout(THEME_SETTLE_MS);
      const violations = await axeViolations(page, AxeBuilder, ".artifacts-head").catch((error) => [String(error)]);
      ok(`the gallery's head — the strip, the unseen line, the tabs — passes axe in ${theme}`, violations.length === 0, JSON.stringify(violations));
    }
    await page.evaluate(() => document.documentElement.setAttribute("data-theme", "dark"));

    /* ---- a switch of project in the sidebar is the newer word ------------------------------------ */
    const followed = await page.evaluate(async (other) => {
      artifactsView().querySelector('.artifacts-project-chip[data-project=":none"]').click();
      await window.__PAINTED__();
      const before = artifactOrder.join(",");
      activeProjectPath = other;
      followSelectedProject();
      await window.__PAINTED__();
      return { before, filter: artifactFilter.project, order: artifactOrder.join(",") };
    }, OTHER);
    const afterSwitch = await strip(page);
    ok(
      "pressing a project in the sidebar lists that project's, whatever chip was picked before — and the strip marks it as the one in view",
      followed.before === "p-nowhere" && followed.filter === null && followed.order === "p-harbor"
        && afterSwitch.checked.join() === OTHER && afterSwitch.viewed.join() === OTHER && afterSwitch.chips[1].startsWith(`${OTHER}=harbor`),
      JSON.stringify({ followed, afterSwitch }),
    );
    await page.evaluate((home) => {
      activeProjectPath = home;
      followSelectedProject();
    }, HOME);

    /* ---- the reader lists the pages of the second brain that name the artifact -------------------- */
    const reader = await page.evaluate(async () => {
      const view = artifactsView();
      view.querySelector('.artifacts-project-chip[data-project="*"]').click();
      await window.__PAINTED__();
      selectArtifact(view, "p-home-old", { reveal: true });
      await window.__PAINTED__();
      const box = view.querySelector(".artifact-cited");
      const pages = [...box.querySelectorAll(".artifact-cited-page")];
      const said = {
        shown: !box.hidden,
        head: box.querySelector(".artifact-cited-head").textContent,
        pages: pages.map((one) => `${one.textContent}@${one.dataset.citedPath}`),
        heights: pages.map((one) => Math.round(one.getBoundingClientRect().height)),
      };
      pages[0].click();
      await new Promise((done) => setTimeout(done, 200));
      said.opened = [...window.__SEAMS__.opened];
      const back = tabs.find((held) => held.kind === "artifacts");
      if (back) setActiveTab(back.id);
      await window.__PAINTED__();
      selectArtifact(view, "p-harbor", { reveal: true });
      await window.__PAINTED__();
      said.uncited = view.querySelector(".artifact-cited").hidden;
      return said;
    });
    ok(
      "the reader of a page the second brain names lists those pages, newest first, each a button of a pressable height — and a page nobody names shows no such line",
      reader.shown && reader.head === "이 아티팩트를 말하는 지식 페이지 2"
        && reader.pages.join("|") === "갤러리의 세 이음매@/vault/wiki/gallery-seams.md|상황판 시안@/vault/wiki/board-draft.md"
        && reader.heights.every((px) => px >= PRESS_MIN_PX) && reader.uncited === true,
      JSON.stringify(reader),
    );
    ok("pressing one opens that page of the vault", reader.opened.includes("/vault/wiki/gallery-seams.md"), JSON.stringify(reader.opened));

    /* ---- 「모두 읽음」, and a mark that grew old --------------------------------------------------- */
    const cleared = await page.evaluate(async () => {
      const view = artifactsView();
      const clear = view.querySelector(".artifacts-unseen-clear");
      const before = { words: clear.hidden ? "" : clear.textContent, nav: document.querySelector("#nav-artifacts .nav-new")?.textContent ?? "" };
      clear.click();
      await window.__PAINTED__();
      return {
        before,
        clear: clear.hidden,
        nav: document.querySelector("#nav-artifacts .nav-new")?.textContent ?? "",
        cards: [...view.querySelectorAll(".artifact-card-new")].filter((one) => !one.hidden).length,
        stored: localStorage.getItem("zerocode.artifacts.unseen.v1"),
        left: artifactUnseen.size,
      };
    });
    ok(
      "「새 N · 모두 읽음」 stands while something is unseen and takes every mark down at one press — the door's, the cards', the stored ones",
      cleared.before.words === "새 2 · 모두 읽음" && cleared.before.nav === "새 2" && cleared.clear === true && cleared.nav === ""
        && cleared.cards === 0 && cleared.stored === "[]" && cleared.left === 0,
      JSON.stringify(cleared),
    );
    const aged = await page.evaluate((day) => {
      const now = Date.now();
      localStorage.setItem("zerocode.artifacts.unseen.v1", JSON.stringify([
        { id: "p-last-month", version: 1, title: "지난달", pane: null, agent: null, worker: null, at: now - 30 * day },
        { id: "p-eight-days", version: 1, title: "여드레 전", pane: null, agent: null, worker: null, at: now - 8 * day },
        { id: "p-yesterday", version: 1, title: "어제", pane: null, agent: null, worker: null, at: now - day },
      ]));
      return [...loadArtifactUnseen().keys()];
    }, DAY_MS);
    ok("a mark left unopened for more than a week is not read back — 「새」 is a word about recent things", aged.join() === "p-yesterday", JSON.stringify(aged));
    await page.evaluate(() => localStorage.removeItem("zerocode.artifacts.unseen.v1"));

    ok("the seams raised no renderer fault", faults.length === 0, faults.slice(0, 3).join(" | "));
  } finally {
    await page.close();
  }
}
