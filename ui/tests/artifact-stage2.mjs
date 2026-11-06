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
    // The store's versions reach the row's current number — the fixture's own answer stops at 2.
    window.__ANSWER__.artifact_versions = (args) => {
      const current = artifactRows.get(args.id)?.version ?? 2;
      return Array.from({ length: current }, (_, at) => ({
        n: at + 1, path: `/data/artifacts/versions/${args.id}/${at + 1}/page.html`, bytes: 10 + at, modified_ms: at + 1, sha256: String(at + 1).repeat(64),
      }));
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
        const ask = { id: args.id, label: args.label ?? null, answered: false };
        held.thumbAsks.push(ask);
        await new Promise((done) => setTimeout(done, 4));
        ask.answered = true;
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
      // Waited for until the seen ask is answered, not only asked: the answer comes 4 ms after the ask, and on CI's
      // loaded macOS runner the beat that saw the ask came before it (heldThumb false, then the gallery asked the
      // hidden pane at once — runs 42000454668 and 42000455539).
      for (let beat = 0; beat < 40 && !held.thumbAsks.some((one) => one.id === "gal-0" && one.label && one.answered); beat += 1) await held.settle(30);
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

    /* ---- ⑤ the gallery opened while the seen picture is still being taken ------------------ */
    // The review of t-97133 (t-97492): between the seen ask and its answer the seen key is written and no picture is
    // held; a gallery opened in that gap queued the card and asked the HIDDEN pane as well — one extra render.
    const midShot = await page.evaluate(async () => {
      const held = window.__STAGE2__;
      const SEEN = "data:image/gif;base64,R0lGODlhAQABAIAAAP///wAAACH5BAEAAAAALAAAAAABAAEAAAICRAEAOw==";
      const HIDDEN = "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";
      /* One take: the seen answer is held until `release` says what it is (a picture or nothing). */
      const take = async (seenAnswer) => {
        const out = {};
        let release = null;
        const gate = new Promise((done) => {
          release = done;
        });
        held.thumbAsks = [];
        window.__ANSWER__.artifact_thumbnail = async (args) => {
          const ask = { id: args.id, label: args.label ?? null };
          held.thumbAsks.push(ask);
          if (args.label) return (await gate) ? { data_url: SEEN, cached: false } : null;
          return { data_url: HIDDEN, cached: false };
        };
        document.documentElement.style.setProperty("--artifact-seen-thumb-ms", "40");
        artifactThumbs.delete("gal-0");
        artifactThumbFailed.delete("gal-0");
        artifactSeenShots.delete("gal-0");
        await openArtifactPageAt(artifactRows.get("gal-0")?.path ?? "");
        await held.settle();
        const tab = held.tabsOf("gal-0")[0] ?? null;
        setActiveTab(tab?.id);
        await held.settle();
        const url = pathAsFileUrl(artifactRows.get("gal-0")?.path ?? "");
        await held.navigate(tab, url, "started");
        await held.navigate(tab, url, "finished");
        // The seen ask is out and unanswered.
        for (let beat = 0; beat < 40 && !held.thumbAsks.some((one) => one.id === "gal-0" && one.label); beat += 1) await held.settle(30);
        out.seenAsked = held.thumbAsks.some((one) => one.id === "gal-0" && one.label === tab?.label);
        await held.toGallery(null);
        await held.settle(200);
        const view = artifactsView();
        view?.querySelector('[data-artifact-tab="pages"]')?.click();
        await held.settle(200);
        out.hiddenAsksWhileTaking = held.thumbAsks.filter((one) => one.id === "gal-0" && one.label === null).length;
        release(seenAnswer);
        await held.settle(200);
        const card = view?.querySelector('.artifact-card[data-id="gal-0"]');
        const thumb = card?.querySelector(".artifact-card-thumb");
        out.cardSrc = thumb && thumb.hidden === false ? (thumb.getAttribute("src") === SEEN ? "seen" : thumb.getAttribute("src") === HIDDEN ? "hidden" : "other") : "none";
        out.hiddenAsksAfter = held.thumbAsks.filter((one) => one.id === "gal-0" && one.label === null).length;
        document.documentElement.style.removeProperty("--artifact-seen-thumb-ms");
        await held.fresh();
        return out;
      };
      return { pictured: await take(true), failed: await take(false) };
    });
    ok(
      "⑤ a gallery opened while the seen picture is being taken asks the hidden pane for nothing, and the card wears the seen picture when it comes",
      midShot.pictured.seenAsked && midShot.pictured.hiddenAsksWhileTaking === 0 && midShot.pictured.hiddenAsksAfter === 0 && midShot.pictured.cardSrc === "seen",
      JSON.stringify(midShot.pictured),
    );
    ok(
      "⑤ when that seen picture fails, the card asks the hidden pane once and wears its picture",
      midShot.failed.seenAsked && midShot.failed.hiddenAsksWhileTaking === 0 && midShot.failed.hiddenAsksAfter === 1 && midShot.failed.cardSrc === "hidden",
      JSON.stringify(midShot.failed),
    );

    /* ---- ⑤ a card already queued when the seen picture starts is not drawn on the hidden pane ---- */
    // The second review of t-97519 (t-97596): the skip guarded only the ask side. A card waiting in the queue behind
    // another card's hidden render when the seen pane starts its shot was still dequeued and drawn a second time.
    const queuedShot = await page.evaluate(async () => {
      const held = window.__STAGE2__;
      const HIDDEN = "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";
      const out = {};
      let release = null;
      const gate = new Promise((done) => {
        release = done;
      });
      held.thumbAsks = [];
      // The first hidden render is held open, so every other visible card waits in the queue behind it.
      window.__ANSWER__.artifact_thumbnail = async (args) => {
        const ask = { id: args.id, label: args.label ?? null };
        held.thumbAsks.push(ask);
        if (!args.label && held.thumbAsks.filter((one) => one.label === null).length === 1) await gate;
        return { data_url: HIDDEN, cached: false };
      };
      for (const id of artifactRows.keys()) {
        artifactThumbs.delete(id);
        artifactThumbFailed.delete(id);
        artifactSeenShots.delete(id);
      }
      await held.toGallery(null);
      for (let beat = 0; beat < 40 && !held.thumbAsks.some((one) => one.label === null); beat += 1) await held.settle(30);
      const first = held.thumbAsks.find((one) => one.label === null)?.id ?? null;
      const waiting = artifactThumbQueue.find((id) => id !== first) ?? null;
      out.waiting = waiting;
      // The seen pane starts its shot of the waiting card: its key is written, no picture is held yet.
      if (waiting) artifactSeenShots.set(waiting, artifactThumbnailKey(artifactRows.get(waiting)));
      release();
      await held.settle(200);
      out.hiddenAsksWhileTaking = held.thumbAsks.filter((one) => one.id === waiting && one.label === null).length;
      // The seen shot fails: its key goes and the queue is refilled, as snapArtifactSeen does — the card's own turn.
      if (waiting) artifactSeenShots.delete(waiting);
      refillArtifactThumbQueue();
      await held.settle(200);
      out.hiddenAsksAfter = held.thumbAsks.filter((one) => one.id === waiting && one.label === null).length;
      await held.fresh();
      return out;
    });
    ok(
      "⑤ a card already queued when the seen picture starts is not drawn on the hidden pane, and gets its own turn when that picture fails",
      queuedShot.waiting !== null && queuedShot.hiddenAsksWhileTaking === 0 && queuedShot.hiddenAsksAfter === 1,
      JSON.stringify(queuedShot),
    );

    /* ---- ⑤ a seen picture taken while the row changed is not kept under the new key -------- */
    // The second review of t-97519: snapArtifactSeen wrote its answer without comparing the key after the await, so a
    // picture of the old file went into the cache while the catalog already named a newer one.
    const movedShot = await page.evaluate(async () => {
      const held = window.__STAGE2__;
      const SEEN = "data:image/gif;base64,R0lGODlhAQABAIAAAP///wAAACH5BAEAAAAALAAAAAABAAEAAAICRAEAOw==";
      const HIDDEN = "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";
      const out = {};
      let release = null;
      const gate = new Promise((done) => {
        release = done;
      });
      held.thumbAsks = [];
      window.__ANSWER__.artifact_thumbnail = async (args) => {
        const ask = { id: args.id, label: args.label ?? null };
        held.thumbAsks.push(ask);
        if (args.label) return (await gate) ? { data_url: SEEN, cached: false } : null;
        return { data_url: HIDDEN, cached: false };
      };
      document.documentElement.style.setProperty("--artifact-seen-thumb-ms", "40");
      artifactThumbs.delete("gal-0");
      artifactThumbFailed.delete("gal-0");
      artifactSeenShots.delete("gal-0");
      const before = artifactRows.get("gal-0") ?? null;
      const oldKey = artifactThumbnailKey(before);
      await openArtifactPageAt(before?.path ?? "");
      await held.settle();
      const tab = held.tabsOf("gal-0")[0] ?? null;
      setActiveTab(tab?.id);
      await held.settle();
      const url = pathAsFileUrl(before?.path ?? "");
      await held.navigate(tab, url, "started");
      await held.navigate(tab, url, "finished");
      for (let beat = 0; beat < 40 && !held.thumbAsks.some((one) => one.id === "gal-0" && one.label); beat += 1) await held.settle(30);
      out.seenAsked = held.thumbAsks.some((one) => one.id === "gal-0" && one.label === tab?.label);
      // The catalog moves on while the seen pane is still taking the old file.
      if (before) artifactRows.set("gal-0", { ...before, modified_ms: before.modified_ms + 1 });
      release(true);
      await held.settle(200);
      out.keptOldPicture = artifactThumbs.get("gal-0") === SEEN;
      out.oldKeyStays = artifactSeenShots.get("gal-0") === oldKey;
      if (before) artifactRows.set("gal-0", before);
      artifactThumbs.delete("gal-0");
      artifactSeenShots.delete("gal-0");
      document.documentElement.style.removeProperty("--artifact-seen-thumb-ms");
      await held.fresh();
      return out;
    });
    ok(
      "⑤ a seen picture whose row changed while it was taken is not cached under the new key",
      movedShot.seenAsked && movedShot.keptOldPicture === false && movedShot.oldKeyStays === false,
      JSON.stringify(movedShot),
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

    /* ---- ⑥ reading a page, the address bar folds and the header is one line --------------- */
    const reading = await page.evaluate(async () => {
      const held = window.__STAGE2__;
      const out = {};
      const widthOf = (node) => Math.round(node?.getBoundingClientRect().width ?? 0);
      const heightOf = (node) => Math.round(node?.getBoundingClientRect().height ?? 0);
      // A plain browser tab first: the body's height with the address bar standing.
      await openBrowserTab("https://example.com/plain");
      await held.settle();
      const plain = tabs.find((one) => one.kind === "browser" && one.url === "https://example.com/plain") ?? null;
      setActiveTab(plain?.id);
      await held.settle();
      const plainHost = held.hostOf(plain);
      out.plainAddressHeight = heightOf(plainHost?.querySelector(".browser-address-slot"));
      out.plainToolbarHeight = heightOf(plainHost?.querySelector(".browser-toolbar"));
      out.plainBodyHeight = heightOf(plainHost?.querySelector(".browser-body"));
      out.plainReading = plainHost?.classList.contains("is-artifact-reading") ?? null;
      // Then the artifact page, in the same group — the same host, the same height to share.
      const pub = held.publication("p-read");
      const tab = await held.openFromGallery(pub.id);
      setActiveTab(tab?.id);
      await held.settle();
      const host = held.hostOf(tab);
      const toolbar = host?.querySelector(".browser-toolbar");
      // What a later hand puts in the toolbar (the browser workspace work, C8) stays in sight.
      let planted = toolbar?.querySelector(".browser-workspace-status") ?? null;
      if (!planted && toolbar) {
        planted = document.createElement("span");
        planted.className = "browser-workspace-status";
        planted.textContent = "작업 공간";
        toolbar.appendChild(planted);
        paintBrowserView(tab);
        await held.settle();
      }
      const strip = host?.querySelector(".artifact-strip");
      out.reading = host?.classList.contains("is-artifact-reading") ?? null;
      out.addressHeight = heightOf(host?.querySelector(".browser-address-slot"));
      out.addressWidth = widthOf(host?.querySelector(".browser-address-slot"));
      out.toolbarChildrenHidden = [...(toolbar?.children ?? [])]
        .filter((one) => !one.classList.contains("browser-address-slot") && !one.hidden && widthOf(one) === 0)
        .map((one) => one.className);
      out.plantedShown = widthOf(planted) > 0 && heightOf(planted) > 0;
      out.stripHeight = heightOf(strip);
      out.headHeight = heightOf(strip?.querySelector(".artifact-strip-head"));
      out.oneLine = out.stripHeight > 0 && out.stripHeight <= Math.round(out.headHeight * 1.6);
      out.sameRow = toolbar && strip ? Math.round(toolbar.getBoundingClientRect().top) === Math.round(strip.getBoundingClientRect().top) : false;
      out.bodyHeight = heightOf(host?.querySelector(".browser-body"));
      // Before and after in one run: with the reading class taken off, the host lays the toolbar
      // and the strip out as it did before — that body is the before.
      host?.classList.remove("is-artifact-reading");
      out.stackedBodyHeight = heightOf(host?.querySelector(".browser-body"));
      out.stackedStripHeight = heightOf(strip);
      if (out.reading) host?.classList.add("is-artifact-reading");
      out.gain = out.bodyHeight - out.stackedBodyHeight;
      // A link out of the artifact: the address bar stands again.
      await held.navigate(tab, "https://example.com/elsewhere");
      out.leftReading = host?.classList.contains("is-artifact-reading") ?? null;
      out.leftAddressHeight = heightOf(host?.querySelector(".browser-address-slot"));
      // And back on its file it folds again.
      await held.navigate(tab, pathAsFileUrl(pub.path));
      out.backReading = host?.classList.contains("is-artifact-reading") ?? null;
      await held.fresh();
      return out;
    });
    ok(
      `⑥ reading an artifact page the address bar folds (height ${reading.addressHeight} px) while the toolbar's other children stay in sight — including one planted later — and the header is one line: toolbar and strip on one row`,
      reading.plainReading === false && reading.reading === true && reading.addressHeight === 0 && reading.addressWidth === 0
        && Array.isArray(reading.toolbarChildrenHidden) && reading.toolbarChildrenHidden.length === 0 && reading.plantedShown
        && reading.oneLine && reading.sameRow,
      JSON.stringify(reading),
    );
    ok(
      `⑥ the reading surface gains the folded row (body ${reading.stackedBodyHeight} → ${reading.bodyHeight} px, +${reading.gain} px; address row ${reading.plainToolbarHeight} px, strip ${reading.stackedStripHeight} → ${reading.stripHeight} px), and a link out of the artifact brings the address bar back (${reading.leftAddressHeight} px) until the page is its file again`,
      reading.gain >= reading.plainToolbarHeight - 2 && reading.leftReading === false && reading.leftAddressHeight > 0 && reading.backReading === true,
      JSON.stringify(reading),
    );

    /* ---- ⑦ a republished page swaps in behind the old picture and keeps the scroll ------- */
    const swap = await page.evaluate(async () => {
      const held = window.__STAGE2__;
      const out = {};
      const indexOf = (order, command, from = 0) => order.indexOf(command, from);
      const pub = held.publication("p-swap", { bytes: 40 * 1024 });
      const tab = await held.openFromGallery(pub.id);
      setActiveTab(tab?.id);
      await held.settle();
      await held.navigate(tab, tab?.url ?? "", "finished");
      out.shownBefore = tab?.artifact?.version ?? null;
      // The reader is 420 px down the page.
      held.scroll = [0, 420];
      held.snapshots.length = 0;
      held.walks.length = 0;
      held.evals.length = 0;
      held.places.length = 0;
      window.__ORDER__ = [];
      const next = { ...pub, version: 3, modified_ms: Date.now() + 1 };
      held.extra.set(pub.id, next);
      artifactRows.set(pub.id, next);
      await held.published(next);
      for (let beat = 0; beat < 40 && held.walks.length === 0; beat += 1) await held.settle(30);
      const order = (window.__ORDER__ ?? []).slice();
      out.walkedTo = held.walks[0]?.url ?? null;
      out.walksToV3 = held.walks.length === 1 && /\/p-swap\/3\//.test(out.walkedTo ?? "");
      out.snapshotFirst = indexOf(order, "browser_snapshot") >= 0 && indexOf(order, "browser_snapshot") < indexOf(order, "browser_navigate");
      out.scrollRead = held.evals.some((one) => one.label === tab?.label && /scrollX/.test(one.expression) && /scrollY/.test(one.expression));
      out.scrollReadFirst = indexOf(order, "browser_eval") >= 0 && indexOf(order, "browser_eval") < indexOf(order, "browser_navigate");
      out.shadeUp = held.shadeOf(tab)?.hidden === false;
      out.paneDownWhileLoading = held.lastPlace(tab?.label)?.shown === false;
      // The new page paints: the scroll goes back first, then the pane stands and the picture goes.
      held.evals.length = 0;
      window.__ORDER__ = [];
      await held.navigate(tab, out.walkedTo ?? "", "started");
      await held.navigate(tab, out.walkedTo ?? "", "finished");
      for (let beat = 0; beat < 40 && held.lastPlace(tab?.label)?.shown !== true; beat += 1) await held.settle(30);
      await held.settle(120);
      const after = (window.__ORDER__ ?? []).slice();
      out.restored = held.evals.some((one) => one.label === tab?.label && /scrollTo\(\s*0\s*,\s*420\s*\)/.test(one.expression));
      out.restoredBeforeShow = indexOf(after, "browser_eval") >= 0 && indexOf(after, "browser_eval") < after.lastIndexOf("browser_place");
      out.paneBack = held.lastPlace(tab?.label)?.shown === true;
      out.shadeLifted = held.shadeOf(tab)?.hidden !== false;
      out.shownAfter = tab?.artifact?.version ?? null;
      out.noteHidden = held.hostOf(tab)?.querySelector(".artifact-strip-note")?.hidden ?? null;
      await held.fresh();
      return out;
    });
    ok(
      "⑦ a light republication swaps in by itself: the old page's picture is laid and the scroll read before the pane walks to the new version with the pane lying down",
      swap.shownBefore === 2 && swap.walksToV3 && swap.snapshotFirst && swap.scrollRead && swap.scrollReadFirst && swap.shadeUp && swap.paneDownWhileLoading,
      JSON.stringify(swap),
    );
    ok(
      "⑦ when the new version paints the scroll is put back (420 px) before the pane stands, the picture goes, and the strip says the new version",
      swap.restored && swap.restoredBeforeShow && swap.paneBack && swap.shadeLifted && swap.shownAfter === 3 && swap.noteHidden === true,
      JSON.stringify(swap),
    );

    const heavy = await page.evaluate(async () => {
      const held = window.__STAGE2__;
      const out = {};
      const pub = held.publication("p-heavy", { bytes: 5 * 1024 * 1024 });
      const tab = await held.openFromGallery(pub.id);
      setActiveTab(tab?.id);
      await held.settle();
      await held.navigate(tab, tab?.url ?? "", "finished");
      held.walks.length = 0;
      held.snapshots.length = 0;
      const next = { ...pub, version: 3, modified_ms: Date.now() + 1 };
      held.extra.set(pub.id, next);
      artifactRows.set(pub.id, next);
      await held.published(next);
      await held.settle(150);
      out.autoWalks = held.walks.length;
      out.shownStill = tab?.artifact?.version ?? null;
      const note = held.hostOf(tab)?.querySelector(".artifact-strip-note") ?? null;
      const act = note?.querySelector(".artifact-strip-note-act") ?? null;
      out.noteShown = note ? note.hidden === false && note.getBoundingClientRect().height > 0 : null;
      out.noteWords = note?.querySelector(".artifact-strip-note-words")?.textContent ?? null;
      out.actWords = act && !act.hidden ? act.textContent.trim() : null;
      // 「최신으로」 does what the light page did by itself.
      act?.click();
      for (let beat = 0; beat < 40 && held.walks.length === 0; beat += 1) await held.settle(30);
      out.walkedOnPress = held.walks.length === 1 && /\/p-heavy\/3\//.test(held.walks[0]?.url ?? "");
      await held.navigate(tab, held.walks[0]?.url ?? "", "finished");
      await held.settle(120);
      out.shownAfter = tab?.artifact?.version ?? null;
      out.noteGone = held.hostOf(tab)?.querySelector(".artifact-strip-note")?.hidden ?? null;
      await held.fresh();
      return out;
    });
    ok(
      "⑦ a heavy republication (5 MiB) does not swap by itself: the strip offers 「최신으로」 with the new version named, and the press does the swap",
      heavy.autoWalks === 0 && heavy.shownStill === 2 && heavy.noteShown === true && String(heavy.noteWords).includes("3")
        && heavy.actWords === "최신으로" && heavy.walkedOnPress && heavy.shownAfter === 3 && heavy.noteGone === true,
      JSON.stringify(heavy),
    );

    /* ---- ⑧ the open is timed in visible time and recorded; a late page says so ------------- */
    const clock = await page.evaluate(async () => {
      const held = window.__STAGE2__;
      const out = {};
      // The marks are the table's (8 s, 30 s), pulled in here so the suite does not wait on them.
      out.slowMark = 150;
      document.documentElement.style.setProperty("--artifact-open-slow-ms", String(out.slowMark));
      document.documentElement.style.setProperty("--artifact-open-stalled-ms", "400");
      const noteOf = (tab) => held.hostOf(tab)?.querySelector(".artifact-strip-note") ?? null;
      const noteWords = (tab) => (noteOf(tab)?.hidden === false ? noteOf(tab).querySelector(".artifact-strip-note-words")?.textContent ?? "" : null);
      // A: an ordinary open paints after a while and is recorded once, with the visible time.
      held.records.length = 0;
      const pressed = performance.now();
      const tab = await held.openFromGallery("gal-0");
      setActiveTab(tab?.id);
      await held.settle(80);
      const url = pathAsFileUrl(artifactRows.get("gal-0")?.path ?? "");
      await held.navigate(tab, url, "finished");
      const wallA = performance.now() - pressed;
      const recordsA = held.records.filter((one) => one.id === "gal-0");
      out.recordsA = recordsA.length;
      out.outcomeA = recordsA[0]?.outcome ?? null;
      out.visibleA = recordsA[0]?.visible_ms ?? null;
      out.wallA = Math.round(wallA);
      out.visibleAPlausible = Number.isFinite(out.visibleA) && out.visibleA >= 60 && out.visibleA <= wallA + 5;
      out.noteA = noteWords(tab);
      await held.fresh();
      // B: only visible time counts — the clock stops while the gallery is in front of the page.
      held.records.length = 0;
      const pressedB = performance.now();
      const tabB = await held.openFromGallery("gal-3");
      setActiveTab(tabB?.id);
      await held.settle(40);
      openArtifacts();
      await held.settle(250);
      setActiveTab(tabB?.id);
      await held.settle(40);
      const wallB = performance.now() - pressedB;
      await held.navigate(tabB, pathAsFileUrl(artifactRows.get("gal-3")?.path ?? ""), "finished");
      const recordB = held.records.find((one) => one.id === "gal-3") ?? null;
      out.visibleB = recordB?.visible_ms ?? null;
      out.wallB = Math.round(wallB);
      out.awayLeftOut = Number.isFinite(out.visibleB) && out.visibleB <= wallB - 200;
      await held.fresh();
      // C: a page that does not come — the strip says so past the slow mark, and the open is recorded as stalled past the stalled mark.
      held.records.length = 0;
      const slow = held.publication("p-slow");
      const tabC = await held.openFromGallery(slow.id);
      setActiveTab(tabC?.id);
      await held.settle(60);
      // How long the page has been in sight as the early note is read: before the slow mark the strip says nothing.
      // A fixed beat cannot promise that it is still before the mark — with the page's CPU slowed 4x the beats ran
      // late and the note was read past it (job 1793773002-t97133-red-green) — so the read says when it was taken.
      const earlyClock = artifactOpenClocks.get(tabC?.label) ?? null;
      out.visibleEarly = earlyClock ? Math.round(artifactOpenVisibleMs(earlyClock)) : null;
      out.noteEarly = noteWords(tabC);
      await held.settle(200);
      out.noteSlow = noteWords(tabC);
      out.recordsSlow = held.records.filter((one) => one.id === slow.id).length;
      await held.settle(300);
      out.noteStalled = noteWords(tabC);
      const stalled = held.records.filter((one) => one.id === slow.id);
      out.recordsStalled = stalled.length;
      out.outcomeStalled = stalled[0]?.outcome ?? null;
      out.visibleStalled = stalled[0]?.visible_ms ?? null;
      out.actStalled = noteOf(tabC)?.querySelector(".artifact-strip-note-act")?.hidden === false
        ? noteOf(tabC).querySelector(".artifact-strip-note-act").textContent.trim() : null;
      // The page comes at last: the note goes.
      await held.navigate(tabC, tabC?.url ?? "", "finished");
      out.noteAfter = noteWords(tabC);
      document.documentElement.style.removeProperty("--artifact-open-slow-ms");
      document.documentElement.style.removeProperty("--artifact-open-stalled-ms");
      await held.fresh();
      return out;
    });
    ok(
      `⑧ an open is recorded once when the page paints, with the visible time (${clock.visibleA} ms of ${clock.wallA} ms wall), and the strip says nothing`,
      clock.recordsA === 1 && clock.outcomeA === "painted" && clock.visibleAPlausible && clock.noteA === null,
      JSON.stringify(clock),
    );
    ok(
      `⑧ only visible time counts: with the gallery in front for 250 ms the record says ${clock.visibleB} ms of ${clock.wallB} ms wall`,
      clock.awayLeftOut,
      JSON.stringify(clock),
    );
    ok(
      "⑧ past the slow mark the strip says the page is still loading; past the stalled mark the open is recorded as stalled and the strip says so, and the note goes when the page comes",
      Number.isFinite(clock.visibleEarly) && (clock.noteEarly === null || clock.visibleEarly >= clock.slowMark)
        && typeof clock.noteSlow === "string" && clock.noteSlow.includes("아직 읽는 중") && clock.recordsSlow === 0
        && typeof clock.noteStalled === "string" && clock.noteStalled.includes("멈춘") && clock.recordsStalled >= 1
        && clock.outcomeStalled === "stalled" && Number.isFinite(clock.visibleStalled) && clock.visibleStalled >= 380
        && clock.actStalled === "다시 읽기" && clock.noteAfter === null,
      JSON.stringify(clock),
    );

    ok("the artifact-stage2 window raised no errors", faults.length === 0, faults.slice(0, 3).join(" | "));
  } finally {
    await page.close();
  }
}
