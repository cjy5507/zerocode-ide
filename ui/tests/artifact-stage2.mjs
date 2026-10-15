/* The artifact page, stage 2 B (t-59665, design ④~⑧ of reports/t-55370/artifact/design.html).
 *
 * ④ A menu over the page does not blank it: before the native pane lies down under a menu or a
 *    picker, the window takes the pane's picture through its own snapshot door (`browser_snapshot`,
 *    the markup editor's road) and lays it in the pane's place; when the menu closes the pane stands
 *    again where it was and the picture goes. No navigation, no reload — the scroll is untouched.
 * ⑤ A page the person has looked at gives the gallery its thumbnail: a beat after its first paint
 *    the window asks `artifact_thumbnail` with the pane's label, so the hidden pane renders only
 *    pages nobody has seen. A Markdown card's face is an excerpt of its first characters with every
 *    fenced block (code, mermaid) left out, and asks for no picture.
 * ⑥ Reading a page, the browser's address bar folds and the header is one line; the toolbar's other
 *    children stay; a link out of the artifact brings the address bar back.
 * ⑦ A republished page swaps in behind the picture of the old one and keeps the reader's scroll; a
 *    heavy publication waits for 「최신으로」.
 * ⑧ The open is timed in visible time only and recorded once (`artifact_open_record`); past the
 *    slow mark the strip says the page is still loading, past the stalled mark the open is recorded
 *    as stalled.
 *
 * The stub backend makes no native pane: `open_browser_pane` is counted, `browser_place` is recorded
 * with its `shown`, `browser_snapshot` answers a picture, `browser_eval` answers a scroll position and
 * records what it was asked. Everything in the fixture is synthetic. Every read of the page is
 * defensive (`?.`, `?? null`): on a tree without the change the checks fail at their assertions, one
 * line each, and the suite runs to its end.
 *
 *   WINDOW_SUITES=artifact-stage2 node ui/tests/window.mjs */
import { openWindowTestPage, recordBootPhases, waitBootSettled } from "./window-boot.mjs";
import { REAL_WINDOW } from "./artifact-cards.mjs";

/* The gallery fixture's rows: pages `gal-0` and `gal-3`, documents `gal-1` and `gal-4`, claude.ai
 * rows `gal-2` and `gal-5`. */
const CATALOG = Object.freeze({ count: 0, gallery: 6 });
/* A one-pixel GIF: what the snapshot door answers in this suite. */
const PICTURE = "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";

/* The page's hands for this suite, installed once: the gallery standing, the pane doors counted and
 * recorded, and the gestures each check makes. */
function standStage2(page) {
  return page.evaluate(async ({ catalog, picture }) => {
    dropTab("artifacts");
    window.__ARTIFACTS__ = catalog;
    const held = (window.__STAGE2__ = { panes: 0, places: [], snapshots: [], evals: [], walks: [], records: [], extra: new Map() });
    window.__ANSWER__.open_browser_pane = () => {
      held.panes += 1;
      return `browser-stage2-${held.panes}`;
    };
    window.__ANSWER__.browser_zoom = () => null;
    window.__ANSWER__.close_browser_pane = () => null;
    window.__ANSWER__.browser_grab_arm = () => null;
    window.__ANSWER__.browser_grab_disarm = () => null;
    window.__ANSWER__.browser_place = (args) => {
      held.places.push({ ...args });
      return null;
    };
    window.__ANSWER__.browser_snapshot = (args) => {
      held.snapshots.push(args.label);
      return picture;
    };
    window.__ANSWER__.browser_eval = (args) => {
      held.evals.push({ label: args.label, expression: args.expression });
      return held.scroll ?? [0, 0];
    };
    window.__ANSWER__.browser_navigate = (args) => {
      held.walks.push({ label: args.label, url: args.url });
      return null;
    };
    window.__ANSWER__.artifact_open_record = (args) => {
      held.records.push(JSON.parse(JSON.stringify(args.record ?? args)));
      return null;
    };
    window.__ANSWER__.artifact_export_formats = () => [
      { format: "html", available: true }, { format: "pdf", available: true }, { format: "png", available: true },
    ];
    // The store knows its own files: a terminal link to one opens that artifact.
    window.__ANSWER__.artifact_page_at = (args) => {
      const row = [...artifactRows.values(), ...held.extra.values()].find((one) => one.path === args.path);
      return row ? { artifact: row, version: null } : null;
    };
    el("nav-artifacts").click();
    await window.__PAINTED__();
    for (let beat = 0; beat < 600 && (artifactAsking || artifactRows.size === 0); beat += 1) {
      await new Promise((next) => requestAnimationFrame(next));
    }
    await window.__PAINTED__();
    const settle = (ms = 60) => new Promise((done) => setTimeout(done, ms));
    held.settle = settle;
    held.tabsOf = (id) => tabs.filter((one) => one.kind === "browser" && one.artifact?.id === id);
    held.pageTabs = () => tabs.filter((one) => one.kind === "browser" && one.artifact);
    held.hostOf = (tab) => (tab ? docHost(tab.pane, "browser") : null);
    held.toGallery = async (id) => {
      openArtifacts();
      await settle();
      if (id) {
        selectArtifact(artifactsView(), id);
        await settle();
      }
    };
    held.openFromGallery = async (id) => {
      await held.toGallery(id);
      await artifactAction(artifactsView(), "open", id);
      await settle();
      return held.tabsOf(id)[0] ?? null;
    };
    /* The page's own word that it loaded, as the backend sends it. */
    held.navigate = async (tab, url, state = "finished") => {
      for (const speak of window.__LISTENERS__["browser:nav"] ?? []) speak({ payload: { label: tab.label, url, state } });
      await settle();
    };
    held.published = async (row) => {
      for (const speak of window.__LISTENERS__["artifacts:published"] ?? []) speak({ payload: row });
      await settle();
    };
    /* A clean stage between scenes: every page tab gone, the records kept. */
    held.fresh = async () => {
      for (const tab of tabs.filter((one) => one.kind === "browser")) dropTab(tab.id);
      renderTabs();
      await settle();
      held.places.length = 0;
      held.snapshots.length = 0;
      held.evals.length = 0;
      held.walks.length = 0;
    };
    /* A publication row of this suite's own, beside the fixture's pages. */
    held.publication = (id, more = {}) => {
      const row = {
        id, kind: "page", title: `발행물 ${id}`, path: `/data/artifacts/pages/${id}/index.html`, bytes: 2048,
        created_ms: Date.now(), modified_ms: Date.now(), version: 2, source_path: `/tmp/zerocode-window-test/project/${id}.html`,
        origin: { agent: "claude", project: "/tmp/zerocode-window-test/project" }, tags: [], preview: { kind: "none" }, source: "agent_page",
        ...more,
      };
      held.extra.set(id, row);
      artifactRows.set(id, row);
      return row;
    };
    held.shadeOf = (tab) => held.hostOf(tab)?.querySelector(".browser-body > .artifact-shade") ?? null;
    held.lastPlace = (label) => held.places.filter((one) => one.label === label).at(-1) ?? null;
    held.rect = (node) => {
      const box = node?.getBoundingClientRect();
      return box ? [Math.round(box.x), Math.round(box.y), Math.round(box.width), Math.round(box.height)] : null;
    };
  }, { catalog: CATALOG, picture: PICTURE });
}

export async function testArtifactStage2(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin, { before: recordBootPhases });
  try {
    await page.setViewportSize(REAL_WINDOW);
    await waitBootSettled(page);
    await standStage2(page);

    /* ---- ④ a menu over the page does not blank it ------------------------------------------ */
    const shade = await page.evaluate(async () => {
      const held = window.__STAGE2__;
      const seen = {};
      const pub = held.publication("p-shade");
      const tab = await held.openFromGallery(pub.id);
      setActiveTab(tab?.id);
      await held.settle();
      const host = held.hostOf(tab);
      const body = host?.querySelector(".browser-body");
      // The pane stands on the body before anything covers it.
      const stood = held.lastPlace(tab?.label);
      seen.stoodShown = stood?.shown === true;
      seen.bodyRect = held.rect(body);
      held.snapshots.length = 0;
      held.places.length = 0;
      window.__ORDER__ = [];
      // The export menu opens over the page — the strip's own menu, a DOM menu under the native pane.
      host?.querySelector(".artifact-strip-export")?.click();
      for (let beat = 0; beat < 40 && (sidebarMenu.hidden || held.lastPlace(tab?.label)?.shown !== false); beat += 1) await held.settle(30);
      seen.menuOpen = sidebarMenu.hidden === false;
      const picture = held.shadeOf(tab);
      seen.paneHidden = held.lastPlace(tab?.label)?.shown === false;
      seen.snapshotsWhileOpen = held.snapshots.length;
      seen.shadeShown = picture ? picture.hidden === false && picture.getAttribute("src") !== null && picture.getAttribute("src") !== "" : false;
      seen.shadeRect = held.rect(picture);
      // The blank rectangle: the body with nothing drawn in it while the pane lies down.
      seen.blankWhileOpen = seen.paneHidden && !seen.shadeShown ? 1 : 0;
      seen.snapshotBeforeHide = (() => {
        const order = window.__ORDER__ ?? [];
        const snap = order.indexOf("browser_snapshot");
        const hide = order.indexOf("browser_place");
        return snap >= 0 && hide >= 0 && snap < hide;
      })();
      // The menu closes: the pane stands again where it was, the picture goes, nothing navigated.
      closeSidebarMenu();
      for (let beat = 0; beat < 40 && held.lastPlace(tab?.label)?.shown !== true; beat += 1) await held.settle(30);
      await held.settle(120);
      const back = held.lastPlace(tab?.label);
      seen.paneBack = back?.shown === true;
      seen.sameSeat = back ? [Math.round(back.x), Math.round(back.y), Math.round(back.width), Math.round(back.height)].join() === [stood.x, stood.y, stood.width, stood.height].map(Math.round).join() : false;
      seen.shadeLifted = held.shadeOf(tab)?.hidden !== false;
      seen.walked = held.walks.length;
      seen.reloaded = window.__COUNTS__.browser_reload ?? 0;
      await held.fresh();
      return seen;
    });
    ok(
      "④ while the export menu covers an artifact page the pane's picture stands in its place: one snapshot, taken before the pane lies down, and no blank rectangle",
      shade.stoodShown && shade.menuOpen && shade.paneHidden && shade.snapshotsWhileOpen === 1 && shade.shadeShown
        && shade.snapshotBeforeHide && shade.blankWhileOpen === 0
        && Array.isArray(shade.shadeRect) && Array.isArray(shade.bodyRect) && shade.shadeRect.join() === shade.bodyRect.join(),
      JSON.stringify(shade),
    );
    ok(
      "④ when the menu closes the pane stands again at the same seat, the picture goes, and nothing was navigated or reloaded — the scroll is where it was",
      shade.paneBack && shade.sameSeat && shade.shadeLifted && shade.walked === 0 && shade.reloaded === 0,
      JSON.stringify(shade),
    );

    /* ---- ⑤ a page the person looked at gives the gallery its thumbnail ------------------- */
    const seen = await page.evaluate(async () => {
      const held = window.__STAGE2__;
      const out = {};
      // Two pictures: the seen pane's and the hidden pane's, told apart by their bytes.
      const SEEN = "data:image/gif;base64,R0lGODlhAQABAIAAAP///wAAACH5BAEAAAAALAAAAAABAAEAAAICRAEAOw==";
      const HIDDEN = "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";
      held.thumbAsks = [];
      window.__ANSWER__.artifact_thumbnail = async (args) => {
        held.thumbAsks.push({ id: args.id, label: args.label ?? null });
        await new Promise((done) => setTimeout(done, 4));
        return { data_url: args.label ? SEEN : HIDDEN, cached: false };
      };
      // A beat after the first paint, not half a second: the number is the table's, overridden here.
      document.documentElement.style.setProperty("--artifact-seen-thumb-ms", "40");
      artifactThumbs.delete("gal-0");
      artifactThumbFailed.delete("gal-0");
      // Opened by a terminal link while no gallery card is on screen: nobody has seen this page, and
      // no card has asked the hidden pane for it yet.
      await openArtifactPageAt(artifactRows.get("gal-0")?.path ?? "");
      await held.settle();
      const tab = held.tabsOf("gal-0")[0] ?? null;
      setActiveTab(tab?.id);
      await held.settle();
      const url = pathAsFileUrl(artifactRows.get("gal-0")?.path ?? "");
      await held.navigate(tab, url, "started");
      await held.navigate(tab, url, "finished");
      for (let beat = 0; beat < 40 && !held.thumbAsks.some((one) => one.id === "gal-0" && one.label); beat += 1) await held.settle(30);
      out.seenAsks = held.thumbAsks.filter((one) => one.id === "gal-0" && one.label === tab?.label).length;
      out.hiddenAsksWhileOpen = held.thumbAsks.filter((one) => one.id === "gal-0" && one.label === null).length;
      out.heldThumb = artifactThumbs.get("gal-0") === SEEN;
      // Back in the gallery the card wears the seen picture and asks the hidden pane for nothing.
      held.thumbAsks.length = 0;
      await held.toGallery(null);
      await held.settle(200);
      const view = artifactsView();
      view?.querySelector('[data-artifact-tab="pages"]')?.click();
      await held.settle(200);
      const card = view?.querySelector('.artifact-card[data-id="gal-0"]');
      const thumb = card?.querySelector(".artifact-card-thumb");
      out.cardShowsSeen = thumb ? thumb.hidden === false && thumb.getAttribute("src") === SEEN : null;
      out.hiddenAsksInGallery = held.thumbAsks.filter((one) => one.id === "gal-0" && one.label === null).length;
      document.documentElement.style.removeProperty("--artifact-seen-thumb-ms");
      await held.fresh();
      return out;
    });
    ok(
      "⑤ a beat after a page's first paint the window asks artifact_thumbnail with the pane's label — once — and keeps that picture",
      seen.seenAsks === 1 && seen.hiddenAsksWhileOpen === 0 && seen.heldThumb,
      JSON.stringify(seen),
    );
    ok(
      "⑤ back in the gallery the card wears the seen picture and the hidden pane is asked for nothing (hidden renders for a seen page 1 → 0)",
      seen.cardShowsSeen === true && seen.hiddenAsksInGallery === 0,
      JSON.stringify(seen),
    );

    /* ---- ⑤ a Markdown card's face is an excerpt without its fenced blocks ------------------ */
    const excerpt = await page.evaluate(async () => {
      const held = window.__STAGE2__;
      const out = {};
      const base = window.__buildArtifacts__({ count: 0, gallery: 6 });
      const now = Date.now();
      const doc = {
        id: "doc-fence", kind: "document", title: "fenced.md", path: "/tmp/zerocode-window-test/project/fenced.md", bytes: 900,
        created_ms: now, modified_ms: now, origin: { agent: "claude", project: "/tmp/zerocode-window-test/project" }, tags: [],
        preview: {
          kind: "markdown",
          text: "# 보고서 제목\n\n```mermaid\ngraph TD; A-->B\n```\n\n첫 문단은 발췌에 선다.\n\n```js\nconst fence = 1;\n```\n\n둘째 문단도 선다.\n\n~~~\ntilde fence\n~~~\n\n셋째 문단까지 선다.\n\n```\nan open fence runs to the end",
        },
        source: "agent_page",
      };
      window.__ANSWER__.artifacts_list = () => ({
        rows: [doc, ...base], total: base.length + 1, truncated: false, missing: [], missing_total: 0,
        thumb: { width: 320, height: 240, queue_max: 24 },
      });
      window.__PREVIEW_ASKS__ = [];
      held.thumbAsks = [];
      window.__ANSWER__.artifact_thumbnail = async (args) => {
        held.thumbAsks.push({ id: args.id, label: args.label ?? null });
        return { data_url: null, cached: true };
      };
      artifactAskedAt = 0;
      await refreshArtifacts();
      await held.settle(120);
      const view = artifactsView();
      view?.querySelector('[data-artifact-tab="pages"]')?.click();
      await held.settle(200);
      const card = view?.querySelector('.artifact-card[data-id="doc-fence"]');
      const face = card?.querySelector(".artifact-card-doc");
      out.faceShown = face ? face.hidden === false : null;
      out.face = face?.textContent ?? null;
      out.hasProse = ["첫 문단은 발췌에 선다", "둘째 문단도 선다", "셋째 문단까지 선다"].every((words) => (out.face ?? "").includes(words));
      out.noFences = !["graph TD", "const fence", "tilde fence", "open fence"].some((words) => (out.face ?? "").includes(words));
      out.thumbHidden = card?.querySelector(".artifact-card-thumb")?.hidden ?? null;
      out.pictureAsks = held.thumbAsks.filter((one) => one.id === "doc-fence").length
        + (window.__PREVIEW_ASKS__ ?? []).filter((id) => id === "doc-fence").length;
      delete window.__ANSWER__.artifacts_list;
      artifactAskedAt = 0;
      await refreshArtifacts();
      await held.settle(120);
      return out;
    });
    ok(
      "⑤ a Markdown card's face is the excerpt of its text with every fenced block left out — code, mermaid, tilde and an unclosed fence — and no picture is asked for it",
      excerpt.faceShown === true && excerpt.hasProse && excerpt.noFences && excerpt.thumbHidden === true && excerpt.pictureAsks === 0,
      JSON.stringify(excerpt),
    );

    ok("the artifact-stage2 window raised no errors", faults.length === 0, faults.slice(0, 3).join(" | "));
  } finally {
    await page.close();
  }
}
