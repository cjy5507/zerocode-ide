/* The docked session history, held to counts (t-103659): what it asks the backend, how many cards it builds, and
 * what it says while a walk looks at the disk. Times are measured beside it (`vault-dock-perf.mjs`) and judged
 * nowhere — a count is the same on any machine.
 *
 * The scene is the measuring file's: one `vault_sessions` answer of 200 made-up sessions in 24 groups. */
import { openWindowTestPage } from "./window-boot.mjs";
import { standVaultDockScene, VAULT_DOCK_SCENE } from "./vault-dock-perf.mjs";

export async function testVaultDock(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await standVaultDockScene(page);
    const seen = await page.evaluate(async (scene) => {
      const seen = {};
      const settle = () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const wait = (ms) => new Promise((done) => setTimeout(done, ms));
      const list = document.getElementById("vdock-list");
      const sub = () => document.getElementById("vdock-sub").textContent;
      const asked = () => window.__COUNTS__.vault_sessions ?? 0;
      // Every card the window builds, counted where it is built — a card that was kept and only moved to its
      // place in a new layout is not one.
      let built = 0;
      const builder = window.vaultDockCard;
      window.vaultDockCard = function countedVaultDockCard(...args) {
        built += 1;
        return builder.apply(this, args);
      };
      const builtSince = () => {
        const was = built;
        built = 0;
        return was;
      };
      locale = "ko";

      // ---- arrival: one ask, the one that looks at the disk, and only the cards near the view ----
      const before = asked();
      document.getElementById("activity-vault-tab").click();
      await wait(80);
      await settle();
      seen.arrivalAsks = asked() - before;
      seen.arrivalForce = window.__ASKED_VAULT__?.force;
      seen.cards = list.querySelectorAll(".vdock-card").length;
      seen.slots = list.querySelectorAll(".vdock-slot").length;
      seen.sessions = scene.sessions;
      seen.firstTitle = list.querySelector(".vdock-card-title")?.textContent;
      seen.slotHeight = list.style.getPropertyValue("--vdock-slot-height");
      builtSince();

      // ---- a keystroke: one ask, not forced, and no card built for an answer that says the same ----
      const typed = asked();
      const box = document.getElementById("vdock-query");
      box.value = "a";
      box.dispatchEvent(new Event("input", { bubbles: true }));
      await wait(80);
      await settle();
      seen.keystrokeAsks = asked() - typed;
      seen.keystrokeForce = window.__ASKED_VAULT__?.force;
      seen.keystrokeBuilt = builtSince();

      // ---- one conversation moved: one card is built, the rest stand ----
      const first = window.__VAULT__.groups[0].sessions[0];
      const standing = list.querySelector(".vdock-card");
      const neighbour = list.querySelectorAll(".vdock-card")[1];
      first.title = "바뀐 제목";
      await refreshVaultDock();
      await settle();
      seen.movedBuilt = builtSince();
      seen.movedTitle = list.querySelector(".vdock-card-title")?.textContent;
      seen.movedReplaced = !standing.isConnected;
      seen.neighbourKept = neighbour.isConnected;

      // ---- details open where the card stands: nothing is built again ----
      list.querySelector(".vdock-card").click();
      await settle();
      seen.openBuilt = builtSince();
      seen.openStands = Boolean(list.querySelector(".vdock-card > .vdock-detail"));
      seen.openSays = list.querySelector(".vdock-card").getAttribute("aria-expanded");
      list.querySelector(".vdock-card").click();
      await settle();
      seen.foldedAgain = !list.querySelector(".vdock-card > .vdock-detail");
      builtSince();

      // ---- the list keeps its place when an answer comes back changed ----
      list.scrollTop = Math.round(list.scrollHeight / 2);
      await wait(120);
      await settle();
      const edge = list.getBoundingClientRect().top;
      const read = [...list.querySelectorAll(".vdock-card")].find((card) => card.getBoundingClientRect().top >= edge);
      const readAt = read.getBoundingClientRect().top;
      builtSince();
      window.__VAULT__.groups[1].sessions[0].title = "멀리서 바뀐 제목";
      await refreshVaultDock();
      await settle();
      seen.placeKept = read.isConnected && Math.abs(read.getBoundingClientRect().top - readAt) <= 1;
      seen.placeWas = list.scrollTop;

      // ---- scrolled to: the cards at the end are built when they come near ----
      list.scrollTop = list.scrollHeight;
      await wait(120);
      await settle();
      const last = [...list.querySelectorAll(".vdock-group-body")].at(-1).lastElementChild;
      seen.endBuilt = last.classList.contains("vdock-card");
      seen.stillSlots = list.querySelectorAll(".vdock-slot").length;
      list.scrollTop = 0;

      // ---- a walk looking: said quietly, asked again without force, then settled ----
      window.__VAULT__.refreshing = true;
      const walking = asked();
      await refreshVaultDock(true);
      seen.refreshingSays = sub();
      seen.refreshingBusy = list.getAttribute("aria-busy");
      window.__VAULT__.refreshing = false;
      await wait(VAULT_REFRESH_RETRY_MS + 200);
      seen.askedAgain = asked() - walking;
      seen.againForce = window.__ASKED_VAULT__?.force;
      seen.settledSays = sub();
      const quiet = asked();
      await wait(VAULT_REFRESH_RETRY_MS + 200);
      seen.asksOnceSettled = asked() - quiet;

      // ---- sessions that are not a person's: one line says how many of each kind, one hand shows them ----
      seen.hidesByDefault = window.__ASKED_VAULT__?.query?.hide_empty;
      window.__VAULT__.hidden = { empty: 3, window_made: 138, measured: 0 };
      await refreshVaultDock();
      const note = document.getElementById("vdock-note");
      seen.hiddenSays = note.hidden ? "" : note.textContent;
      [...note.querySelectorAll("button")].find((one) => one.textContent === "보기")?.click();
      await wait(80);
      seen.showAsked = window.__ASKED_VAULT__?.query?.hide_empty;
      window.__VAULT__.hidden = { empty: 0, window_made: 0, measured: 0 };
      await refreshVaultDock();
      seen.nothingHiddenSays = note.hidden ? "" : note.textContent;

      // ---- a session with no name says so, never an id ----
      window.__VAULT__.groups[0].sessions[0].title = "";
      await refreshVaultDock();
      seen.untitled = list.querySelector(".vdock-card-title")?.textContent;

      // ---- the first run on a machine: reading, not "nothing found" ----
      const held = window.__VAULT__;
      window.__VAULT__ = { ...held, groups: [], shown: 0, total: 0, matched: 0, truncated: false, refreshing: true };
      await refreshVaultDock();
      seen.firstRunSays = sub();
      seen.firstRunEmpty = Boolean(list.querySelector(".vdock-empty"));
      window.__VAULT__ = { ...window.__VAULT__, refreshing: false };
      await wait(VAULT_REFRESH_RETRY_MS + 200);
      seen.nothingFoundSays = list.querySelector(".vdock-empty")?.textContent;

      // ---- every new word is in every catalog ----
      const keys = [
        "vaultDock.refreshing", "vaultDock.hidden", "vaultDock.hiddenEmpty", "vaultDock.hiddenWindow",
        "vaultDock.hiddenMeasured", "vaultDock.showHidden", "vaultDock.untitled",
      ];
      seen.catalogs = ["en", "ja", "zh", "es"].every((lang) => keys.every((key) => CATALOG[lang][key]));

      window.vaultDockCard = builder;
      window.__VAULT__ = undefined;
      vaultDockAnswer = null;
      vaultDockQuery = "";
      box.value = "";
      vaultDockHideEmpty = VAULT_HIDE_EMPTY_DEFAULT;
      vaultDockOpen.clear();
      paintVaultDock();
      document.getElementById("activity-files-tab").click();
      return seen;
    }, VAULT_DOCK_SCENE);

    ok(
      "the docked history asks once on arrival, for a look at the disk, and builds only the cards near the view",
      seen.arrivalAsks === 1 && seen.arrivalForce === true &&
        seen.cards >= 1 && seen.cards * 3 < seen.sessions && seen.cards + seen.slots === seen.sessions &&
        seen.firstTitle === "지어낸 세션 0 — 목록을 재는 제목" && /^\d+px$/.test(seen.slotHeight),
      JSON.stringify({ asks: seen.arrivalAsks, force: seen.arrivalForce, cards: seen.cards, slots: seen.slots, slotHeight: seen.slotHeight }),
    );
    ok(
      "a keystroke asks once without force, and an answer that says the same builds no card",
      seen.keystrokeAsks === 1 && seen.keystrokeForce === false && seen.keystrokeBuilt === 0,
      JSON.stringify({ asks: seen.keystrokeAsks, force: seen.keystrokeForce, built: seen.keystrokeBuilt }),
    );
    ok(
      "one conversation that moved builds one card and leaves the others standing",
      seen.movedBuilt === 1 && seen.movedTitle === "바뀐 제목" && seen.movedReplaced && seen.neighbourKept,
      JSON.stringify({ built: seen.movedBuilt, title: seen.movedTitle, replaced: seen.movedReplaced, kept: seen.neighbourKept }),
    );
    ok(
      "a card opens and folds its details where it stands, building nothing again",
      seen.openBuilt === 0 && seen.openStands && seen.openSays === "true" && seen.foldedAgain,
      JSON.stringify({ built: seen.openBuilt, stands: seen.openStands, says: seen.openSays, folded: seen.foldedAgain }),
    );
    ok(
      "the list keeps its place when a changed answer comes back, and builds the cards scrolled to",
      seen.placeKept && seen.placeWas > 0 && seen.endBuilt && seen.stillSlots > 0,
      JSON.stringify({ kept: seen.placeKept, place: seen.placeWas, endBuilt: seen.endBuilt, slots: seen.stillSlots }),
    );
    ok(
      "while a walk looks at the disk the head says so, the panel asks again without force, and stops once settled",
      seen.refreshingSays === "1000개 중 200개 표시 · 다시 읽는 중" && seen.refreshingBusy === "true" &&
        seen.askedAgain === 2 && seen.againForce === false &&
        seen.settledSays === "1000개 중 200개 표시" && seen.asksOnceSettled === 0,
      JSON.stringify({ says: seen.refreshingSays, busy: seen.refreshingBusy, again: seen.askedAgain, force: seen.againForce, settled: seen.settledSays, after: seen.asksOnceSettled }),
    );
    ok(
      "sessions that are not a person's are hidden unless asked for, one line says how many of each kind, and its hand shows them",
      seen.hidesByDefault === true &&
        seen.hiddenSays.includes("숨김: 빈 대화 3개 · 창이 만든 확인 138개보기") && !seen.hiddenSays.includes("측정 실행") &&
        seen.showAsked === false && !seen.nothingHiddenSays.includes("숨김"),
      JSON.stringify({ byDefault: seen.hidesByDefault, says: seen.hiddenSays, show: seen.showAsked, none: seen.nothingHiddenSays }),
    );
    ok(
      "a session with no name says so and the first run says it is reading, in every catalog's words",
      seen.untitled === "이름 없는 세션" && seen.firstRunSays === "세션을 읽고 있습니다…" && !seen.firstRunEmpty &&
        seen.nothingFoundSays === "agent 세션을 찾을 수 없습니다" && seen.catalogs,
      JSON.stringify({ untitled: seen.untitled, firstRun: seen.firstRunSays, empty: seen.firstRunEmpty, nothing: seen.nothingFoundSays, catalogs: seen.catalogs }),
    );
    ok("the docked history raised no error in the page", faults.length === 0, faults.join(" | "));
  } finally {
    await page.close();
  }
}
