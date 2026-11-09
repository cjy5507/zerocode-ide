import { openWindowTestPage } from "./window-boot.mjs";

/* Keeping an Android phone awake (T7, 2026-11-09): the person starts it on the
 * real-phone card (설정 › Computer Use) for one of the set lengths; the status
 * bar's chip says it stands, how long is left, and stops it; a session that
 * waits on a locked phone says so; one that ended because the phone went says
 * that once. The phone, its id and its name are made up.
 *
 *   WINDOW_SUITES=phone-awake node ui/tests/window.mjs */

const DEVICE = "phone-android-synthetic";
const NAME = "Synthetic Pixel";

export async function testPhoneAwake(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const empty = await page.evaluate(async () => {
      window.__ANSWER__.phone_awake_phones = () => [];
      window.__ANSWER__.phone_awake_sessions = () => [];
      setSettingsOpen(true);
      showSettingsPane("computer-use");
      await refreshPhoneAwakeRows();
      return {
        hint: document.querySelector("#phone-awake-rows .settings-hint")?.textContent ?? "",
        chip: document.getElementById("sb-phone-awake").hidden,
      };
    });
    ok("phone-awake: with no allowed phone connected the card says where a phone is allowed, and no chip stands",
      empty.hint.includes("허락한 안드로이드 폰이 연결돼 있지 않습니다") && empty.chip, JSON.stringify(empty));

    const offered = await page.evaluate(async ({ device, name }) => {
      window.__STARTED__ = null;
      window.__STOPPED__ = null;
      window.__ANSWER__.phone_awake_phones = () => [{ device, name }];
      window.__ANSWER__.phone_awake_start = (args) => {
        window.__STARTED__ = args;
        const now = Date.now();
        return [{ device: args.device, name, started_ms: now, until_ms: now + args.minutes * 60_000, poked_ms: now, waiting: null }];
      };
      window.__ANSWER__.phone_awake_stop = (args) => {
        window.__STOPPED__ = args;
        return [];
      };
      await refreshPhoneAwakeRows();
      const row = document.querySelector(`.phone-awake-row[data-device="${device}"]`);
      return {
        name: row?.querySelector(".phone-awake-name")?.textContent,
        lengths: [...(row?.querySelectorAll("button") ?? [])].map((button) => [button.textContent, button.getAttribute("aria-label")]),
      };
    }, { device: DEVICE, name: NAME });
    ok("phone-awake: an allowed phone is offered by name with its three lengths, each a named button",
      offered.name === NAME && offered.lengths.map(([words]) => words).join("|") === "15분|30분|60분" &&
        offered.lengths.every(([, label]) => label.includes(NAME)), JSON.stringify(offered));

    const started = await page.evaluate(async (device) => {
      [...document.querySelectorAll(`.phone-awake-row[data-device="${device}"] button`)][1].click();
      await new Promise((done) => setTimeout(done, 30));
      const row = document.querySelector(`.phone-awake-row[data-device="${device}"]`);
      return {
        asked: window.__STARTED__,
        state: row?.querySelector(".phone-awake-state")?.textContent,
        stop: row?.querySelector("button")?.textContent,
        chip: !document.getElementById("sb-phone-awake").hidden,
        chipWords: document.getElementById("sb-phone-awake-words").textContent,
        chipLabel: document.getElementById("sb-phone-awake").getAttribute("aria-label"),
      };
    }, DEVICE);
    ok("phone-awake: a length starts the session — the row says the time left with a stop, and the status bar's chip stands",
      started.asked?.device === DEVICE && started.asked?.minutes === 30 && started.state === "30분 남음" &&
        started.stop === "멈추기" && started.chip && started.chipWords === "폰 깨어 있음 · 30분" &&
        started.chipLabel.includes(NAME), JSON.stringify(started));

    const waiting = await page.evaluate(({ device, name }) => {
      const now = Date.now();
      holdPhoneAwakeSessions([{ device, name, started_ms: now, until_ms: now + 600_000, poked_ms: now, waiting: "locked" }]);
      return document.querySelector(`.phone-awake-row[data-device="${device}"] .phone-awake-state`)?.textContent;
    }, { device: DEVICE, name: NAME });
    ok("phone-awake: a locked phone is waited on, and the row says so", waiting === "잠겨 있어 기다리는 중", String(waiting));

    const stopped = await page.evaluate(async () => {
      document.getElementById("sb-phone-awake").click();
      await new Promise((done) => setTimeout(done, 30));
      const item = [...document.querySelectorAll(".sidebar-menu-item")].find((one) => one.textContent.includes("멈추기"));
      const words = item?.textContent ?? "";
      item?.click();
      await new Promise((done) => setTimeout(done, 30));
      return { words, asked: window.__STOPPED__, chip: document.getElementById("sb-phone-awake").hidden };
    });
    ok("phone-awake: the chip's menu stops the session, and the chip goes",
      stopped.words.includes(NAME) && stopped.asked?.device === DEVICE && stopped.chip, JSON.stringify(stopped));

    const ended = await page.evaluate((name) => {
      sayPhoneAwakeEnded({ device: "x", name, why: "unreachable" });
      sayPhoneAwakeEnded({ device: "x", name, why: "time" });
      return [...document.querySelectorAll(".toast")].map((note) => note.textContent).filter((said) => said.includes(name));
    }, NAME);
    ok("phone-awake: a session that ended because the phone went says so once; one that ran its time says nothing",
      ended.length === 1 && ended[0].includes("폰과 연결이 끊겼습니다"), JSON.stringify(ended));
    const wireless = await page.evaluate(async () => {
      window.__PAIRED__ = null;
      window.__ANSWER__.phone_wireless_offers = () => [];
      el("phone-wireless-find").click();
      await new Promise((done) => setTimeout(done, 30));
      const none = el("phone-wireless-said").textContent;
      window.__ANSWER__.phone_wireless_offers = () => [{ addr: "192.0.2.10:37123", device: "phone-android-synthetic" }];
      el("phone-wireless-find").click();
      await new Promise((done) => setTimeout(done, 30));
      const row = document.querySelector('#phone-wireless-offers .phone-awake-row[data-addr="192.0.2.10:37123"]');
      const box = row?.querySelector(".phone-wireless-code");
      const label = box?.getAttribute("aria-label");
      window.__ANSWER__.phone_wireless_pair = () => {
        throw "wireless_pair_failed: Failed: Wrong password or connection was dropped.";
      };
      box.value = "000000";
      row.querySelector("button").click();
      await new Promise((done) => setTimeout(done, 30));
      const failed = el("phone-wireless-said").textContent;
      window.__ANSWER__.phone_wireless_pair = (args) => {
        window.__PAIRED__ = args;
        return { device: "phone-android-synthetic", name: "Synthetic Pixel", allowed: true };
      };
      box.value = "042519";
      box.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
      await new Promise((done) => setTimeout(done, 30));
      return { none, label, failed, asked: window.__PAIRED__, said: el("phone-wireless-said").textContent,
        cleared: el("phone-wireless-offers").childElementCount === 0 };
    });
    ok("phone-wireless: 찾기 lists the phones offering pairing with a named code box, words a failure, and pairs on Enter",
      wireless.none.includes("짝짓기를 연 폰이 보이지 않습니다") && wireless.label === "짝짓기 코드 — 192.0.2.10:37123" &&
        wireless.failed.includes("짝짓기가 되지 않았습니다") && wireless.failed.includes("Wrong password") &&
        wireless.asked?.addr === "192.0.2.10:37123" && wireless.asked?.code === "042519" &&
        wireless.said === "연결했습니다 — Synthetic Pixel. 이미 허락한 폰이라 바로 씁니다." && wireless.cleared,
      JSON.stringify(wireless));
    ok("phone-awake: no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}
