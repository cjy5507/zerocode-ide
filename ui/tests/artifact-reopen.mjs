/* An artifact opens once (t-55370, stage 1).
 *
 * A: the same artifact is one tab on every road that opens it — the gallery and its drawer, a
 *    terminal link (`openArtifactPageAt`), the notice's road beside the maker (`openArtifactBeside`)
 *    — and coming back to it asks for no new native pane.
 * B: a page the gallery opens is a glance (the tab's name leans, as a file's glance does). The next
 *    page the gallery opens takes that glance over: its native pane walks to the new address instead
 *    of a new pane being made. A glance becomes a kept tab when the person claims it — a double
 *    click on the tab, an annotation, a version picked, 「나란히」, a link followed, or opening it
 *    again on a road that is not the gallery's — and 「나란히」 still stands the other version beside.
 * C: the tab is named by the artifact before the page names itself, and the drawer's head opens a
 *    page in one press.
 *
 * The stub backend makes no native pane: `open_browser_pane` is counted (a new pane) and
 * `browser_navigate` is recorded (a pane that walks). Everything in the fixture is synthetic. Every
 * read of the page is defensive (`?.`, `?? null`): on a tree without the change the checks fail at
 * their assertions, one line each, and the suite runs to its end.
 *
 *   WINDOW_SUITES=artifact-reopen node ui/tests/window.mjs */
import { openWindowTestPage, recordBootPhases, waitBootSettled } from "./window-boot.mjs";
import { REAL_WINDOW } from "./artifact-cards.mjs";

/* The gallery fixture's rows: report rows (`art-*`) and two of each gallery kind — pages `gal-0` and
 * `gal-3`, documents `gal-1` and `gal-4`, claude.ai rows `gal-2` and `gal-5`. */
const CATALOG = Object.freeze({ count: 6, gallery: 6 });
/* What the press of a second opening of a page that already stands may still ask the backend: the
 * placement of the pane it brings forward, once or twice — never a pane, a zoom or the version list.
 * Going back to the gallery before the press is its own gesture and is not counted. */
const REOPEN_REQUESTS_MAX = 2;

/* The page's hands for this suite, installed once: the gallery standing, the pane door counted, the
 * store's answer for a path, and the gestures each check makes. */
function standReopen(page) {
  return page.evaluate(async (catalog) => {
    dropTab("artifacts");
    window.__ARTIFACTS__ = catalog;
    const held = (window.__REOPEN__ = { panes: 0, walks: [], extra: new Map() });
    window.__ANSWER__.open_browser_pane = () => {
      held.panes += 1;
      return `browser-reopen-${held.panes}`;
    };
    window.__ANSWER__.browser_zoom = () => null;
    window.__ANSWER__.close_browser_pane = () => null;
    window.__ANSWER__.browser_grab_arm = () => null;
    window.__ANSWER__.browser_grab_disarm = () => null;
    window.__ANSWER__.browser_navigate = (args) => {
      held.walks.push({ label: args.label, url: args.url });
      return null;
    };
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
    const settle = () => new Promise((done) => setTimeout(done, 60));
    const pageTabs = () => tabs.filter((one) => one.kind === "browser" && one.artifact);
    held.settle = settle;
    held.tabsOf = (id) => tabs.filter((one) => one.kind === "browser" && one.artifact?.id === id);
    held.pageTabs = pageTabs;
    /* What a person does in the gallery: back to its tab with the card selected, then the press on
     * 「열기」 — two steps, so a check can count the press alone. */
    held.toGallery = async (id) => {
      openArtifacts();
      await settle();
      selectArtifact(artifactsView(), id);
      await settle();
    };
    held.press = async (id) => {
      await artifactAction(artifactsView(), "open", id);
      await settle();
    };
    held.openFromGallery = async (id) => {
      await held.toGallery(id);
      await held.press(id);
    };
    /* The requests one gesture asked, summed. */
    held.asked = async (gesture) => {
      const before = { ...window.__COUNTS__ };
      await gesture();
      const asked = {};
      for (const [command, count] of Object.entries(window.__COUNTS__)) {
        const more = count - (before[command] ?? 0);
        if (more > 0) asked[command] = more;
      }
      return asked;
    };
    /* A clean stage between scenes: every page tab gone, the counts kept. */
    held.fresh = async () => {
      for (const tab of pageTabs()) dropTab(tab.id);
      for (const tab of tabs.filter((one) => one.kind === "browser")) dropTab(tab.id);
      renderTabs();
      await settle();
    };
    held.nameOf = (tab) => document.querySelector(`.tab[data-tab="${CSS.escape(tab?.id ?? "")}"] .tab-label`)?.textContent ?? null;
    held.leans = (tab) => document.querySelector(`.tab[data-tab="${CSS.escape(tab?.id ?? "")}"]`)?.classList.contains("is-preview") ?? null;
    held.navigate = async (tab, url) => {
      for (const speak of window.__LISTENERS__["browser:nav"] ?? []) speak({ payload: { label: tab.label, url, state: "finished" } });
      await settle();
    };
  }, CATALOG);
}

export async function testArtifactReopen(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin, { before: recordBootPhases });
  try {
    await page.setViewportSize(REAL_WINDOW);
    await waitBootSettled(page);
    await standReopen(page);

    /* ---- A + B: the gallery's two openings of one page, then another page ------------------- */
    const gallery = await page.evaluate(async () => {
      const held = window.__REOPEN__;
      await held.openFromGallery("gal-0");
      const first = held.tabsOf("gal-0")[0] ?? null;
      const seen = {
        firstPanes: held.panes,
        firstGlance: first?.preview === true,
        firstLeans: held.leans(first),
        firstName: held.nameOf(first),
        title: artifactRows.get("gal-0")?.title ?? null,
      };
      await held.toGallery("gal-0");
      const asked = await held.asked(() => held.press("gal-0"));
      seen.reopenPanes = held.panes;
      seen.reopenAsked = asked;
      seen.reopenRequests = Object.entries(asked)
        .filter(([command]) => !["artifacts_list", "artifact_counts", "artifact_tasks", "artifact_preview", "artifact_thumbnail"].includes(command))
        .reduce((sum, [, count]) => sum + count, 0);
      seen.reopenTabs = held.tabsOf("gal-0").length;
      seen.reopenFront = activeTabId === first?.id;
      seen.reopenStillGlance = first?.preview === true;
      const walksBefore = held.walks.length;
      await held.openFromGallery("gal-3");
      const glance = held.pageTabs().find((one) => one.preview) ?? null;
      seen.otherPanes = held.panes;
      seen.otherPageTabs = held.pageTabs().length;
      seen.otherSameTab = glance !== null && glance.id === first?.id;
      seen.otherWalk = held.walks.slice(walksBefore).map((one) => ({ label: one.label, url: one.url }));
      seen.otherLabel = first?.label ?? null;
      seen.otherHolds = glance?.artifact?.id ?? null;
      const strip = glance ? docHost(glance.pane, "browser").querySelector(".artifact-strip-title") : null;
      seen.otherStrip = strip?.textContent ?? null;
      seen.otherTitle = artifactRows.get("gal-3")?.title ?? null;
      seen.otherName = held.nameOf(glance);
      seen.otherFront = activeTabId === glance?.id;
      return seen;
    });
    ok(
      "a page the gallery opens is a glance: one native pane, the tab's name leans, and the tab is named by the artifact before the page names itself",
      gallery.firstPanes === 1 && gallery.firstGlance && gallery.firstLeans === true
        && gallery.firstName === gallery.title && !String(gallery.firstName).startsWith("file:"),
      JSON.stringify(gallery),
    );
    ok(
      `the same page opened again from the gallery is the same tab: no new pane, ${REOPEN_REQUESTS_MAX} requests at most, the tab in front and still a glance`,
      gallery.reopenPanes === 1 && gallery.reopenTabs === 1 && gallery.reopenRequests <= REOPEN_REQUESTS_MAX
        && gallery.reopenFront && gallery.reopenStillGlance,
      JSON.stringify({ panes: gallery.reopenPanes, tabs: gallery.reopenTabs, asked: gallery.reopenAsked, front: gallery.reopenFront }),
    );
    ok(
      "another page from the gallery takes the glance over: no new pane, its pane walks once to the new page, and the strip and the tab say the new artifact",
      gallery.otherPanes === 1 && gallery.otherPageTabs === 1 && gallery.otherSameTab
        && gallery.otherWalk.length === 1 && gallery.otherWalk[0].label === gallery.otherLabel
        && gallery.otherWalk[0].url.endsWith("/project/page-3.html")
        && gallery.otherHolds === "gal-3" && gallery.otherStrip === gallery.otherTitle
        && gallery.otherName === gallery.otherTitle && gallery.otherFront,
      JSON.stringify(gallery),
    );

    /* ---- B: a glance the person claims is kept ---------------------------------------------- */
    const kept = await page.evaluate(async () => {
      const held = window.__REOPEN__;
      const seen = {};
      // A link followed out of the glance keeps it.
      const glance = held.pageTabs().find((one) => one.preview) ?? held.pageTabs()[0] ?? null;
      await held.navigate(glance, "https://example.com/elsewhere");
      seen.linkKept = glance?.preview === false;
      seen.linkLeans = held.leans(glance);
      // The next page then opens its own glance.
      const panes = held.panes;
      await held.openFromGallery("gal-0");
      seen.afterLinkNewPane = held.panes === panes + 1;
      seen.afterLinkTabs = held.pageTabs().length;
      // A double click on the glance's tab keeps it.
      const second = held.tabsOf("gal-0")[0] ?? null;
      document.querySelector(`.tab[data-tab="${CSS.escape(second?.id ?? "")}"]`)?.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
      await held.settle();
      seen.doubleKept = second?.preview === false;
      await held.fresh();
      // An annotation started on a glance keeps it.
      await held.openFromGallery("gal-0");
      const third = held.tabsOf("gal-0")[0] ?? null;
      setActiveTab(third?.id);
      await held.settle();
      docHost(third?.pane, "browser")?.querySelector(".artifact-strip-annotate")?.click();
      await held.settle();
      seen.annotateKept = third?.preview === false;
      if (typeof stopBrowserGrab === "function") stopBrowserGrab(false);
      await held.fresh();
      // A version picked on a glance keeps it.
      await held.openFromGallery("gal-3");
      const fourth = held.tabsOf("gal-3")[0] ?? null;
      setActiveTab(fourth?.id);
      await held.settle();
      const select = docHost(fourth?.pane, "browser")?.querySelector(".artifact-strip-version");
      if (select) {
        select.value = "1";
        select.dispatchEvent(new Event("change", { bubbles: true }));
      }
      await held.settle();
      seen.versionKept = fourth?.preview === false;
      await held.fresh();
      // Opening a glance's artifact on a road that is not the gallery's is a deliberate open: it keeps the tab.
      await held.openFromGallery("gal-0");
      const fifth = held.tabsOf("gal-0")[0] ?? null;
      const panesBefore = held.panes;
      await openArtifactPageAt(artifactRows.get("gal-0")?.path ?? "");
      await held.settle();
      seen.linkOpenSameTab = held.tabsOf("gal-0").length === 1 && held.panes === panesBefore && activeTabId === fifth?.id;
      seen.linkOpenKept = fifth?.preview === false;
      await held.fresh();
      return seen;
    });
    ok(
      "a glance becomes a kept tab when a link leads out of it, and the next page the gallery opens stands in a new glance",
      kept.linkKept && kept.linkLeans === false && kept.afterLinkNewPane && kept.afterLinkTabs === 2,
      JSON.stringify(kept),
    );
    ok(
      "a double click on the glance's tab, an annotation and a picked version each keep the glance",
      kept.doubleKept && kept.annotateKept && kept.versionKept,
      JSON.stringify(kept),
    );
    ok(
      "a terminal link to a page a glance shows brings that tab forward with no new pane, and keeps it",
      kept.linkOpenSameTab && kept.linkOpenKept,
      JSON.stringify(kept),
    );

    /* ---- A: every other road to an open artifact -------------------------------------------- */
    const roads = await page.evaluate(async () => {
      const held = window.__REOPEN__;
      const seen = {};
      // The notice's road (beside the maker, or the gallery's when there is none) opens a kept tab…
      await openArtifactBeside(artifactRows.get("gal-3"), null);
      await held.settle();
      const beside = held.tabsOf("gal-3")[0] ?? null;
      seen.besideKept = beside !== null && beside.preview !== true;
      const panes = held.panes;
      // …and the same road, a terminal link and the gallery all come back to it.
      await openArtifactBeside(artifactRows.get("gal-3"), null);
      await openArtifactPageAt(artifactRows.get("gal-3")?.path ?? "");
      await held.openFromGallery("gal-3");
      seen.roadsPanes = held.panes - panes;
      seen.roadsTabs = held.tabsOf("gal-3").length;
      seen.roadsFront = activeTabId === beside?.id;
      seen.roadsStillKept = beside?.preview !== true;
      await held.fresh();
      // A claude.ai card opened twice is one tab.
      const webPanes = held.panes;
      await held.openFromGallery("gal-2");
      await held.openFromGallery("gal-2");
      const url = artifactRows.get("gal-2")?.url ?? "";
      seen.webPanes = held.panes - webPanes;
      seen.webTabs = tabs.filter((one) => one.kind === "browser" && one.url === url).length;
      await held.fresh();
      return seen;
    });
    ok(
      "the notice's road, a terminal link and the gallery all come back to the tab an artifact already has, with no new pane",
      roads.besideKept && roads.roadsPanes === 0 && roads.roadsTabs === 1 && roads.roadsFront && roads.roadsStillKept,
      JSON.stringify(roads),
    );
    ok(
      "a claude.ai card opened twice from the gallery is one tab on one pane",
      roads.webPanes === 1 && roads.webTabs === 1,
      JSON.stringify(roads),
    );

    /* ---- 「나란히」 still stands the other version beside, and keeps the glance ---------------- */
    const compare = await page.evaluate(async () => {
      const held = window.__REOPEN__;
      const pub = {
        id: "p-reopen", kind: "page", title: "합성 발행물", path: "/data/artifacts/pages/p-reopen/index.html", bytes: 2048,
        created_ms: Date.now(), modified_ms: Date.now(), version: 2, source_path: "/tmp/zerocode-window-test/project/pub.html",
        origin: { agent: "claude", project: "/tmp/zerocode-window-test/project" }, tags: [], preview: { kind: "none" }, source: "agent_page",
      };
      held.extra.set(pub.id, pub);
      artifactRows.set(pub.id, pub);
      await held.openFromGallery(pub.id);
      const glance = held.tabsOf(pub.id)[0] ?? null;
      const seen = { glance: glance?.preview === true, version: glance?.artifact?.version ?? null };
      setActiveTab(glance?.id);
      await held.settle();
      const panes = held.panes;
      docHost(glance?.pane, "browser")?.querySelector(".artifact-strip-compare")?.click();
      for (let beat = 0; beat < 40 && held.tabsOf(pub.id).length < 2; beat += 1) await held.settle();
      const pair = held.tabsOf(pub.id);
      seen.newPanes = held.panes - panes;
      seen.tabs = pair.length;
      seen.versions = pair.map((one) => one.artifact?.version ?? null).sort();
      seen.sourceKept = glance?.preview === false;
      seen.pairKept = pair.every((one) => one.preview !== true);
      await held.fresh();
      artifactRows.delete(pub.id);
      return seen;
    });
    ok(
      "「나란히」 on a glance still opens the other version beside it on a pane of its own, and both tabs are kept",
      compare.glance && compare.version === 2 && compare.newPanes === 1 && compare.tabs === 2
        && compare.versions.join() === "1,2" && compare.sourceKept && compare.pairKept,
      JSON.stringify(compare),
    );

    /* ---- C: the drawer's head opens a page in one press ------------------------------------- */
    const drawer = await page.evaluate(async () => {
      const held = window.__REOPEN__;
      openArtifacts();
      await held.settle();
      const view = artifactsView();
      const seen = {};
      const readBar = (id) => {
        selectArtifact(view, id);
        const bar = view.querySelector(".artifact-detail-bar .artifact-detail-open");
        const inFacts = view.querySelector('.artifact-actions [data-artifact-action="open"]');
        return {
          barShown: bar ? !bar.hidden && bar.getBoundingClientRect().width > 0 : null,
          barWords: bar?.textContent?.trim() ?? null,
          factsShown: inFacts ? !inFacts.hidden : null,
        };
      };
      seen.page = readBar("gal-0");
      seen.document = readBar("gal-1");
      seen.web = readBar("gal-2");
      seen.report = readBar("art-0");
      selectArtifact(view, "gal-0");
      await held.settle();
      const panes = held.panes;
      view.querySelector(".artifact-detail-bar .artifact-detail-open")?.click();
      for (let beat = 0; beat < 40 && held.tabsOf("gal-0").length === 0; beat += 1) await held.settle();
      const opened = held.tabsOf("gal-0")[0] ?? null;
      seen.onePressPanes = held.panes - panes;
      seen.onePressFront = opened !== null && activeTabId === opened.id;
      seen.onePressGlance = opened?.preview === true;
      await held.fresh();
      return seen;
    });
    ok(
      "the drawer's head says 「열기」 for a page, a document and a claude.ai card — where the detail's own 「열기」 steps aside — and not for a report the drawer already shows",
      drawer.page.barShown === true && drawer.page.barWords === "열기" && drawer.page.factsShown === false
        && drawer.document.barShown === true && drawer.web.barShown === true
        && drawer.report.barShown === false && drawer.report.factsShown === true,
      JSON.stringify(drawer),
    );
    ok(
      "one press on the drawer's 「열기」 opens the page in front, as a glance",
      drawer.onePressPanes === 1 && drawer.onePressFront && drawer.onePressGlance,
      JSON.stringify(drawer),
    );
    ok("the artifact-reopen window raised no errors", faults.length === 0, faults.slice(0, 3).join(" | "));
  } finally {
    await page.close();
  }
}
