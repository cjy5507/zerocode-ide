/* A browser profile's imported Chrome data in the real window (t-79331): the
 * import card counts what came with the cookies, the bookmarks stand on the
 * pane's bookmarks strip and in the address bar's suggestions, a page whose
 * form a saved address can fill shows the fill strip and one press fills it,
 * and the settings card says how many came. Each scene is counted before the
 * import and after it.
 *
 *   node ui/tests/browser-profile-data.mjs
 *   PROFILE_DATA_SHOTS=<folder> node ui/tests/browser-profile-data.mjs   # keeps a picture of each scene
 *
 * The backend is the harness's answer table, answering what the import of
 * `tests/fixtures/browser-profile/Bookmarks` and `web-data.sql` stores (the
 * Rust unit tests of `browser_profile_data.rs` read those files); no real
 * profile, keychain or cookie is read. The values are synthetic markers.
 */

import { mkdir } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";

const PROFILE = "a".repeat(32);
const SHOTS = process.env.PROFILE_DATA_SHOTS ? resolve(process.env.PROFILE_DATA_SHOTS) : null;

// What `browser_bookmarks` answers once the fixture's Bookmarks are imported.
const IMPORTED_BOOKMARKS = [
  { title: "[합성-북마크-가]", url: "https://bookmark-a.example.com/", root: "bar" },
  { title: "[합성-북마크-나]", url: "https://bookmark-b.example.com/start", root: "bar", folder: "[합성-폴더]" },
  { title: "[합성-북마크-다]", url: "https://bookmark-c.example.com/deep", root: "bar", folder: "[합성-폴더]" },
  { title: "[합성-북마크-라]", url: "http://bookmark-d.example.org/", root: "bar", folder: "[합성-폴더]" },
  { title: "[합성-북마크-마]", url: "https://bookmark-e.example.net/", root: "other" },
  { title: "[합성-북마크-바]", url: "https://bookmark-f.example.com/phone", root: "mobile" },
];
// What the fixture's Web Data imports to: two addresses, two past entries, and
// two cards plus three secret-looking entries held back.
const IMPORTED = { bookmarks: IMPORTED_BOOKMARKS.length, addresses: 2, autofillEntries: 2, heldBack: 5, atUnix: 1_793_200_000 };
const SUMMARY = {
  totalCookies: 4, importedCookies: 4, skippedCookies: 0, googleCookiesSkipped: 0,
  partitionSkippedCookies: 0, domains: ["example.com"],
  profileData: IMPORTED,
};
// What `browser_address_candidates` answers on the fixture's shipping form: two
// addresses, and the one past entry its unnamed field has, offered apart.
const CANDIDATES = {
  addresses: [
    { id: "00000000-0000-4000-8000-000000000002", label: "[합성-이름-나] · [합성-주소-1줄]", kinds: 6 },
    { id: "00000000-0000-4000-8000-000000000001", label: "[합성-이름-가] · [합성-이메일-가]", kinds: 2 },
  ],
  entries: [{ name: "nickname", label: "[합성-별명]" }],
};

// A picture waits out the overlays' entrance (a menu and a card fade in).
const SHOT_SETTLE_MS = 400;

async function shot(page, name) {
  if (!SHOTS) return;
  await mkdir(SHOTS, { recursive: true });
  await page.waitForTimeout(SHOT_SETTLE_MS);
  await page.screenshot({ path: resolve(SHOTS, `${name}.png`) });
}

/* One scene's numbers: what the strip shows, what the address bar offers for
 * a word a bookmark's url holds, and whether the import card counted them. */
async function scene(page, word) {
  return page.evaluate(async (typed) => {
    const strip = document.querySelector(".browser-view:not([hidden]) .browser-bookmarks");
    const address = document.querySelector(".browser-view:not([hidden]) .browser-address");
    address.focus();
    address.value = typed;
    address.dispatchEvent(new Event("input", { bubbles: true }));
    await new Promise((done) => requestAnimationFrame(() => done()));
    const rows = [...document.querySelectorAll(".browser-view:not([hidden]) .browser-suggest-row")];
    const said = {
      strip: strip && !strip.hidden ? strip.querySelectorAll(".browser-bookmarks-row .browser-bookmark").length : 0,
      folders: strip && !strip.hidden ? strip.querySelectorAll(".browser-bookmark.is-folder").length : 0,
      suggested: rows.filter((row) => row.dataset.kind === "bookmark").length,
      suggestedUrls: rows.filter((row) => row.dataset.kind === "bookmark").map((row) => row.dataset.url),
    };
    address.blur();
    return said;
  }, word);
}

export async function testBrowserProfileData(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(async ({ profile, imported, summary, candidates }) => {
      window.__BOOKMARK_ASKS__ = [];
      window.__NAVIGATIONS__ = [];
      window.__STORED_BOOKMARKS__ = [];
      window.__PROFILE_IMPORTED__ = null;
      window.__ADDRESS_ASKS__ = [];
      window.__ADDRESS_FILLS__ = [];
      window.__ADDRESS_HANDS__ = [];
      window.__ENTRY_FILLS__ = [];
      window.__ENTRY_HANDS__ = [];
      window.__ANSWER__.browser_address_candidates = (args) => {
        window.__ADDRESS_ASKS__.push(args.label);
        return window.__PROFILE_IMPORTED__ ? candidates : { addresses: [], entries: [] };
      };
      window.__ANSWER__.browser_address_fill = (args, options) => {
        window.__ADDRESS_FILLS__.push(JSON.parse(JSON.stringify(args)));
        window.__ADDRESS_HANDS__.push(options?.headers?.["zerocode-hand"] ?? "");
        return { fields: 6, entries: 0 };
      };
      window.__ANSWER__.browser_entry_fill = (args, options) => {
        window.__ENTRY_FILLS__.push(JSON.parse(JSON.stringify(args)));
        window.__ENTRY_HANDS__.push(options?.headers?.["zerocode-hand"] ?? "");
        return { fields: 0, entries: 1 };
      };
      window.__ANSWER__.browser_place = () => null;
      window.__ANSWER__.browser_navigate = (args) => {
        window.__NAVIGATIONS__.push(args.url);
        return null;
      };
      window.__ANSWER__.browser_bookmarks = (args) => {
        window.__BOOKMARK_ASKS__.push(args.profile);
        return window.__STORED_BOOKMARKS__;
      };
      window.__ANSWER__.browser_profiles = () => [{
        id: profile, name: "[합성-프로필]", staged: false, isDefault: false,
        ...(window.__PROFILE_IMPORTED__ ? { imported: window.__PROFILE_IMPORTED__ } : {}),
      }];
      window.__ANSWER__.import_browser_cookies = () => {
        window.__STORED_BOOKMARKS__ = imported;
        window.__PROFILE_IMPORTED__ = summary.profileData;
        return summary;
      };
      await refreshBrowserProfiles();
      const label = await openBrowserTab("http://127.0.0.1:54321/", { profile });
      window.__PROFILE_TAB__ = tabs.find((tab) => tab.kind === "browser" && tab.label === label);
      window.__PAGE_NAV__ = (state, url) => {
        for (const handler of window.__LISTENERS__["browser:nav"] ?? []) {
          handler({ payload: { label: window.__PROFILE_TAB__.label, url, state } });
        }
      };
    }, { profile: PROFILE, imported: IMPORTED_BOOKMARKS, summary: SUMMARY, candidates: CANDIDATES });
    await page.waitForFunction(() => window.__BOOKMARK_ASKS__.length === 1);

    // 1. before: nothing imported — no strip, no bookmark suggestion.
    const before = await scene(page, "bookmark-b");
    await shot(page, "before");
    ok("before the import the profile's pane shows no bookmarks strip and suggests no bookmark",
      before.strip === 0 && before.suggested === 0, JSON.stringify(before));
    await page.evaluate(() => {
      window.__PAGE_NAV__("started", "http://127.0.0.1:54321/order");
      window.__PAGE_NAV__("finished", "http://127.0.0.1:54321/order");
    });
    await page.evaluate(() => new Promise((done) => setTimeout(done, 50)));
    const fillBefore = await page.evaluate(() => ({
      asked: window.__ADDRESS_ASKS__.length,
      shown: !document.querySelector(".browser-view:not([hidden]) .browser-fillbar").hidden,
    }));
    ok("before the import a finished page costs no address question and shows no fill strip",
      fillBefore.asked === 0 && !fillBefore.shown, JSON.stringify(fillBefore));

    // 2. the import: one press, and the card counts the bookmarks beside the cookies.
    await page.evaluate((profile) => runCookieImport("chrome", "Default", profile, { left: 40, bottom: 40 }), PROFILE);
    await page.waitForFunction(() => window.__BOOKMARK_ASKS__.length === 2);
    await page.waitForSelector(".browser-view:not([hidden]) .browser-bookmarks:not([hidden])");
    const card = await page.evaluate(() => [...document.querySelectorAll("#cookie-import-pop p")].map((line) => line.textContent));
    await shot(page, "after-import-card");
    ok("the import card says how many bookmarks and addresses came with the cookies",
      card.some((line) => /북마크 6개 · 주소 2개 · 입력값 2개/.test(line)), JSON.stringify(card));
    ok("the import card says how many cards and secret-looking entries were held back",
      card.some((line) => /5건/.test(line) && /결제 카드/.test(line)), JSON.stringify(card));
    await page.evaluate(() => closeCookieImportPop());

    // 3. after: the strip stands in Chrome's order, a folder is one button.
    const after = await scene(page, "bookmark-b");
    await shot(page, "after-strip");
    ok("after the import the strip shows the bar's bookmark and its one first-level folder",
      after.strip === 2 && after.folders === 1, JSON.stringify(after));
    ok("the address bar suggests the bookmark whose url holds the typed words",
      before.suggested === 0 && after.suggested === 1
        && after.suggestedUrls[0] === "https://bookmark-b.example.com/start", JSON.stringify({ before, after }));
    const other = await scene(page, "bookmark-e");
    ok("a bookmark outside the bar is suggested too", other.suggested === 1, JSON.stringify(other));

    // 4. a folder opens its bookmarks as a menu; a pick navigates the pane.
    await page.click(".browser-view:not([hidden]) .browser-bookmark.is-folder");
    await page.waitForSelector("#sidebar-menu:not([hidden]) .sidebar-menu-item");
    const folder = await page.evaluate(() => [...document.querySelectorAll("#sidebar-menu .sidebar-menu-item")].map((row) => row.textContent));
    await shot(page, "after-folder-menu");
    ok("the folder's menu lists its three bookmarks, the deeper one folded in",
      folder.length === 3 && folder.some((row) => row.includes("[합성-북마크-다]")), JSON.stringify(folder));
    await page.evaluate(() => [...document.querySelectorAll("#sidebar-menu .sidebar-menu-item")]
      .find((row) => row.textContent.includes("[합성-북마크-나]")).click());
    await page.waitForFunction(() => window.__NAVIGATIONS__.length === 1);
    ok("picking a bookmark navigates the pane to it",
      await page.evaluate(() => window.__NAVIGATIONS__[0] === "https://bookmark-b.example.com/start"));

    // 5. 「모든 북마크」 reaches the other and mobile roots.
    await page.click(".browser-view:not([hidden]) .browser-bookmarks-all");
    await page.waitForSelector("#sidebar-menu:not([hidden]) .sidebar-menu-caption");
    const all = await page.evaluate(() => ({
      captions: document.querySelectorAll("#sidebar-menu .sidebar-menu-caption").length,
      rows: [...document.querySelectorAll("#sidebar-menu .sidebar-menu-item")].map((row) => row.textContent),
    }));
    ok("the all-bookmarks menu carries the bar, other bookmarks and mobile bookmarks",
      all.captions === 2 && all.rows.some((row) => row.includes("[합성-북마크-마]"))
        && all.rows.some((row) => row.includes("[합성-북마크-바]")), JSON.stringify(all));
    await page.keyboard.press("Escape");

    // 6. a visited bookmark is one row, not two.
    await page.evaluate(() => recordBrowserVisit("https://bookmark-a.example.com/", "[합성-방문-제목]"));
    const visited = await page.evaluate(async () => {
      const address = document.querySelector(".browser-view:not([hidden]) .browser-address");
      address.focus();
      address.value = "bookmark-a";
      address.dispatchEvent(new Event("input", { bubbles: true }));
      const rows = [...document.querySelectorAll(".browser-view:not([hidden]) .browser-suggest-row")]
        .filter((row) => row.dataset.url === "https://bookmark-a.example.com/");
      address.blur();
      return rows.map((row) => row.dataset.kind ?? "visit");
    });
    ok("a bookmark the history holds is one row that wears the bookmark's mark",
      visited.length === 1 && visited[0] === "bookmark", JSON.stringify(visited));

    // 6b. a page whose form a saved address can fill: one question, the strip, one press.
    await page.evaluate(() => {
      window.__PAGE_NAV__("started", "http://127.0.0.1:54321/order");
      window.__PAGE_NAV__("finished", "http://127.0.0.1:54321/order");
    });
    await page.waitForSelector(".browser-view:not([hidden]) .browser-fillbar:not([hidden])");
    const offered = await page.evaluate(() => {
      const bar = document.querySelector(".browser-view:not([hidden]) .browser-fillbar");
      return {
        asked: window.__ADDRESS_ASKS__.length,
        personOnly: bar.dataset.personOnly !== undefined,
        picks: [...bar.querySelectorAll(".browser-fillbar-act.is-primary")].map((one) => one.textContent),
        entries: [...bar.querySelectorAll(".browser-fillbar-act.is-entry")].map((one) => one.textContent),
        acts: bar.querySelectorAll(".browser-fillbar-act").length,
      };
    });
    await shot(page, "after-fill-offer");
    ok("after the import a finished page asks once and offers each saved address, the past entry apart, and a dismiss",
      fillBefore.asked === 0 && offered.asked === 1 && offered.picks.length === 2 && offered.entries.length === 1
        && offered.acts === 4 && offered.picks[0].includes("[합성-이름-나]") && offered.entries[0].includes("[합성-별명]"),
      JSON.stringify(offered));
    ok("the fill strip is the person's alone", offered.personOnly);
    await page.click(".browser-view:not([hidden]) .browser-fillbar-act.is-primary");
    await page.waitForFunction(() => window.__ADDRESS_FILLS__.length === 1);
    await page.waitForFunction(() => /6/.test(document.querySelector(".browser-view:not([hidden]) .browser-fillbar-words")?.textContent ?? ""));
    const filled = await page.evaluate(() => ({
      fill: window.__ADDRESS_FILLS__[0],
      hand: window.__ADDRESS_HANDS__[0],
      entryFills: window.__ENTRY_FILLS__.length,
      said: document.querySelector(".browser-view:not([hidden]) .browser-fillbar-words").textContent,
      acts: document.querySelectorAll(".browser-view:not([hidden]) .browser-fillbar-act").length,
    }));
    await shot(page, "after-fill-done");
    ok("the press sends only the pane's label and the address's id, with the person's hand",
      Object.keys(filled.fill).sort().join() === "id,label" && filled.fill.id === "00000000-0000-4000-8000-000000000002"
        && filled.hand === "person", JSON.stringify(filled));
    ok("the address press fills no past entry: the strip says how many fields took the address and keeps the entry's button",
      /6/.test(filled.said) && filled.entryFills === 0 && filled.acts === 2, JSON.stringify(filled));
    await page.click(".browser-view:not([hidden]) .browser-fillbar-act.is-entry");
    await page.waitForFunction(() => window.__ENTRY_FILLS__.length === 1);
    await page.waitForFunction(() => document.querySelectorAll(".browser-view:not([hidden]) .browser-fillbar-act").length === 1);
    const typed = await page.evaluate(() => ({
      fill: window.__ENTRY_FILLS__[0],
      hand: window.__ENTRY_HANDS__[0],
      said: document.querySelector(".browser-view:not([hidden]) .browser-fillbar-words").textContent,
    }));
    ok("a past entry goes only when the person presses it: the press sends the pane's label and the field's name",
      Object.keys(typed.fill).sort().join() === "label,name" && typed.fill.name === "nickname" && typed.hand === "person",
      JSON.stringify(typed));
    ok("the strip then counts the address's fields and the entry's, and offers only dismiss",
      /6/.test(typed.said) && /1/.test(typed.said), JSON.stringify(typed));
    await page.evaluate(() => window.__PAGE_NAV__("started", "http://127.0.0.1:54321/next"));
    ok("a document that starts leaving takes its fill strip with it",
      await page.evaluate(() => document.querySelector(".browser-view:not([hidden]) .browser-fillbar").hidden));
    const blank = await page.evaluate(async () => {
      const before = window.__ADDRESS_ASKS__.length;
      window.__PAGE_NAV__("finished", "about:blank");
      await new Promise((done) => setTimeout(done, 50));
      return window.__ADDRESS_ASKS__.length - before;
    });
    ok("a blank document costs no address question", blank === 0, String(blank));

    // 7. the cost: the strip's list is asked once per profile, however often the pane paints.
    const asked = await page.evaluate(() => {
      const before = window.__BOOKMARK_ASKS__.length;
      for (let i = 0; i < 50; i += 1) paintBrowserView(window.__PROFILE_TAB__);
      return window.__BOOKMARK_ASKS__.length - before;
    });
    ok("fifty paints of the pane ask the backend for bookmarks zero more times", asked === 0, String(asked));

    // 8. the settings card counts what came; a profile that never imported says nothing.
    const cards = await page.evaluate((profile) => ({
      carried: browserProfileRow({ id: profile, name: "[합성-프로필]", imported: { bookmarks: 6, addresses: 2, atUnix: 1 } }).textContent,
      never: browserProfileRow({ id: profile, name: "[합성-프로필]" }).querySelectorAll(".browser-profile-carried").length,
    }), PROFILE);
    ok("the settings card says how many bookmarks and addresses the profile carries",
      /북마크 6개 · 주소 2개/.test(cards.carried) && cards.never === 0, JSON.stringify(cards));

    // 8b. an import whose bookmarks and addresses could not be read says why — the
    // backend carries only the kind, the card speaks it.
    const skipped = await page.evaluate((profile) => {
      const said = {};
      for (const kind of ["locked", "unreadable", "unsaved"]) {
        showCookieImportSummary(profile, {
          totalCookies: 1, importedCookies: 1, skippedCookies: 0, domains: ["example.com"], profileDataSkipped: kind,
        }, { left: 8, bottom: 8 });
        said[kind] = cookieImportPop.textContent;
      }
      closeCookieImportPop();
      return said;
    }, PROFILE);
    ok("an import that could not read the bookmarks and addresses says why instead of leaving them out",
      /브라우저를 끄고/.test(skipped.locked) && /읽지 못해/.test(skipped.unreadable) && /저장하지 못해/.test(skipped.unsaved)
        && Object.values(skipped).every((text) => !/북마크 \d+개 · 주소/.test(text)), JSON.stringify(skipped));

    // 9. a pane on the built-in store carries no bookmarks and asks nothing.
    const plain = await page.evaluate(() => {
      const tab = window.__PROFILE_TAB__;
      const profile = tab.profile;
      const before = window.__BOOKMARK_ASKS__.length;
      tab.profile = null;
      paintBrowserView(tab);
      const strip = document.querySelector(".browser-view:not([hidden]) .browser-bookmarks");
      const said = { hidden: strip.hidden, buttons: strip.querySelectorAll("button").length, asked: window.__BOOKMARK_ASKS__.length - before };
      tab.profile = profile;
      paintBrowserView(tab);
      return said;
    });
    ok("a pane without a profile shows no strip and asks nothing",
      plain.hidden && plain.buttons === 0 && plain.asked === 0, JSON.stringify(plain));

    ok("the profile data scenes raised no page errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  let failed = 0;
  let passed = 0;
  try {
    await testBrowserProfileData(browser, origin, (name, pass, details = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${!pass ? `\n${details}` : ""}`);
      if (pass) passed++; else failed++;
    });
  } finally { await browser.close(); files.close(); }
  console.log(`\n${passed}/${passed + failed} passed`);
  if (failed) process.exitCode = 1;
}
