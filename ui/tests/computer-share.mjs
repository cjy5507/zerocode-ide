import { openWindowTestPage } from "./window-boot.mjs";

/* One window shared with an agent, as the person meets it (t-73906).
 *
 * The backend raises a card the first time an agent acts in an app
 * (`computer:share`) and tells the band what is shared
 * (`computer:share-state`). This suite drives both only through what the
 * backend sends and what a person presses, and reads the result off the
 * document:
 *
 *  - the card says which app, in the person's language, and its two answers
 *    go back by id through `computer_confirm_answer` — the refusal is the
 *    safe default;
 *  - the band names the shared app and offers 「제어권 가져오기」 at once,
 *    whatever the agent last did; pressing it calls `computer_take_control`;
 *  - after the person took control the band offers 「다시 맡기기」 alone,
 *    which calls `computer_give_back`;
 *  - both band buttons are the person's (`data-person-only`);
 *  - with sharing required, the card's clock says the agent waits rather
 *    than goes on unshared (t-74826), and an app left waiting is named on
 *    the band with 「맡기기」, the person's, which calls
 *    `computer_share_hand_over` with that app.
 *
 *   WINDOW_SUITES=computer-share node ui/tests/window.mjs */

/* The words in the five languages the window speaks, pinned here so a
 * catalog that drifted, or a key nobody translated, fails. */
export const SHARE_WORDS = Object.freeze({
  ko: { title: "이 창을 맡길까요?", allow: "맡기기", deny: "거절", take: "제어권 가져오기", give: "다시 맡기기" },
  en: { title: "Hand this window over?", allow: "Hand over", deny: "Decline", take: "Take control", give: "Hand back" },
  ja: { title: "このウィンドウを任せますか？", allow: "任せる", deny: "断る", take: "操作を取り戻す", give: "再び任せる" },
  zh: { title: "要把这个窗口交给它吗？", allow: "交给它", deny: "拒绝", take: "收回控制权", give: "重新交给它" },
  es: { title: "¿Ceder esta ventana?", allow: "Ceder", deny: "Rechazar", take: "Tomar el control", give: "Volver a ceder" },
});

/* What the card's clock says in each mode, one fragment a language: the
 * seconds count down, so the fragment is the part that does not move. */
export const SHARE_CLOCK_WORDS = Object.freeze({
  ko: { unshared: "공유 없이 진행", waits: "에이전트가 기다립니다" },
  en: { unshared: "goes on without sharing", waits: "keeps the agent waiting" },
  ja: { unshared: "共有せずに続けます", waits: "エージェントは待ちます" },
  zh: { unshared: "将不共享继续", waits: "智能体会等待" },
  es: { unshared: "sigue sin compartir", waits: "el agente espera" },
});

const installHands = () => {
  const T = {
    settle: (ms = 120) => new Promise((done) => setTimeout(done, ms)),
    fire(name, payload) {
      for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
    },
    answered: [],
    taken: 0,
    handed: [],
    panelLabels: [],
    given: 0,
    said(id) {
      const node = document.getElementById(id);
      return node && !node.hidden && !node.closest("[hidden]") ? node.textContent.trim() : null;
    },
    popup() {
      const scrim = document.getElementById("ask-scrim");
      const section = scrim?.querySelector("[data-kind]");
      return {
        shown: !!scrim && !scrim.hidden,
        kind: section?.dataset.kind ?? null,
        title: T.said("ask-title"),
        clock: T.said("ask-clock"),
        why: T.said("ask-why"),
        buttons: [...(scrim?.querySelectorAll("#ask-choices button") ?? [])].map((one) => one.textContent.trim()),
      };
    },
    band() {
      const band = document.getElementById("computer-band");
      const take = document.getElementById("computer-band-take");
      const give = document.getElementById("computer-band-give");
      const hand = document.getElementById("computer-band-hand");
      return {
        wait: T.said("computer-band-wait"),
        hand: hand && !hand.hidden ? hand.textContent.trim() : null,
        handPersonOnly: hand?.hasAttribute("data-person-only") ?? false,
        shown: !!band && !band.hidden,
        share: T.said("computer-band-share"),
        take: take && !take.hidden ? take.textContent.trim() : null,
        give: give && !give.hidden ? give.textContent.trim() : null,
        takePersonOnly: take?.hasAttribute("data-person-only") ?? false,
        givePersonOnly: give?.hasAttribute("data-person-only") ?? false,
      };
    },
  };
  window.__ANSWER__.computer_confirm_answer = (args) => {
    T.answered.push(JSON.parse(JSON.stringify(args)));
    return true;
  };
  window.__ANSWER__.computer_take_control = () => {
    T.taken += 1;
    return null;
  };
  window.__ANSWER__.computer_share_hand_over = (args) => {
    T.handed.push(JSON.parse(JSON.stringify(args)));
    return true;
  };
  window.__ANSWER__.computer_give_back = () => {
    T.given += 1;
    return null;
  };
  window.__ANSWER__.computer_share_state = (args) => {
    T.panelLabels.push(args.panelLabel);
    return { target: null, takenBack: false };
  };
  window.__SHARE_T__ = T;
};

async function scenario(browser, origin, name, ok, body) {
  const { page } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(installHands);
    await body(page);
  } catch (error) {
    ok(`the ${name} scenario ran to its end`, false, String(error?.stack ?? error).split("\n").slice(0, 3).join(" | "));
  } finally {
    await page.close();
  }
}

export async function testComputerShare(browser, origin, ok) {
  const words = SHARE_WORDS.ko;

  await scenario(browser, origin, "native panel language", ok, async (page) => {
    for (const [locale, words] of Object.entries(SHARE_WORDS)) {
      const shown = await page.evaluate(async (locale) => {
        setLocale(locale, { refresh: false, persist: false });
        await __SHARE_T__.settle();
        return __SHARE_T__.panelLabels.at(-1);
      }, locale);
      ok(`the native take-control panel uses the band's ${locale} words`, shown === words.take, String(shown));
    }
  });

  await scenario(browser, origin, "late share snapshot", ok, async (page) => {
    const band = await page.evaluate(() => {
      takeComputerShare({ target: "Sample", label: "Sample", takenBack: true, revision: 2 });
      takeComputerShare({ target: "Sample", label: "Sample", takenBack: false, revision: 1 });
      return __SHARE_T__.band();
    });
    ok("a late read cannot repaint a taken-back share as acting", band.take === null && band.give !== null, JSON.stringify(band));
  });

  await scenario(browser, origin, "required sharing setting", ok, async (page) => {
    const expected = {
      ko: "공유를 의무로", en: "Require window sharing", ja: "ウィンドウの共有を必須にする",
      zh: "必须共享窗口", es: "Exigir compartir la ventana",
    };
    for (const [locale, label] of Object.entries(expected)) {
      const shown = await page.evaluate(async (locale) => {
        await setLocale(locale, { refresh: false, persist: false });
        const control = document.getElementById("computer-share-required");
        return {
          label: control?.closest("label")?.textContent.trim(),
          personOnly: !!control?.closest("[data-person-only]"),
          setting: !!control?.closest(".settings-pane"),
          about: document.getElementById(control?.getAttribute("aria-describedby"))?.textContent.trim(),
        };
      }, locale);
      ok(`${locale}: the required sharing setting names its purpose`, shown.label === label && !!shown.about, JSON.stringify(shown));
      ok(`${locale}: the real setting is marked for the person`, shown.personOnly && shown.setting, JSON.stringify(shown));
    }
  });


  await scenario(browser, origin, "card", ok, async (page) => {
    const said = await page.evaluate(async () => {
      const T = window.__SHARE_T__;
      T.fire("computer:share", { id: "share-1", app: "Safari", timeoutMs: 30000 });
      await T.settle();
      const before = T.popup();
      const allow = [...document.querySelectorAll("#ask-choices button")].find((one) => one.dataset.allow === "true")
        ?? [...document.querySelectorAll("#ask-choices button")][1];
      allow?.click();
      await T.settle();
      return { before, answered: T.answered, after: T.popup() };
    });
    ok("the card stands in the question popup as its own kind", said.before.shown && said.before.kind === "computer-share", JSON.stringify(said.before));
    ok("the card asks in the person's language", said.before.title === words.title, String(said.before.title));
    ok("the card names the app", (said.before.why ?? "").includes("Safari"), String(said.before.why));
    ok("the card offers the refusal first, then the hand-over", JSON.stringify(said.before.buttons) === JSON.stringify([words.deny, words.allow]), JSON.stringify(said.before.buttons));
    ok("a hand-over answers by id through the confirm road",
      said.answered.length === 1 && said.answered[0].id === "share-1" && said.answered[0].allow === true, JSON.stringify(said.answered));
  });

  await scenario(browser, origin, "band", ok, async (page) => {
    const said = await page.evaluate(async () => {
      const T = window.__SHARE_T__;
      T.fire("computer:share-state", { target: "Safari", takenBack: false });
      await T.settle();
      const sharing = T.band();
      document.getElementById("computer-band-take")?.click();
      await T.settle();
      T.fire("computer:share-state", { target: "Safari", takenBack: true });
      await T.settle();
      const taken = T.band();
      document.getElementById("computer-band-give")?.click();
      await T.settle();
      T.fire("computer:share-state", { target: null, takenBack: false });
      await T.settle();
      return { sharing, taken, gone: T.band(), takes: T.taken, gives: T.given };
    });
    ok("the band stands while a window is shared", said.sharing.shown, JSON.stringify(said.sharing));
    ok("the band names the shared app", (said.sharing.share ?? "").includes("Safari"), String(said.sharing.share));
    ok("the band offers to take control", said.sharing.take === words.take && said.sharing.give === null, JSON.stringify(said.sharing));
    ok("both band buttons are the person's", said.sharing.takePersonOnly && said.sharing.givePersonOnly, JSON.stringify(said.sharing));
    ok("taking control asks the backend once", said.takes === 1, String(said.takes));
    ok("after taking control the band offers to hand back alone", said.taken.give === words.give && said.taken.take === null, JSON.stringify(said.taken));
    ok("handing back asks the backend once", said.gives === 1, String(said.gives));
    ok("with nothing shared the band offers neither", said.gone.take === null && said.gone.give === null && said.gone.share === null, JSON.stringify(said.gone));
  });

  await scenario(browser, origin, "card clock by mode", ok, async (page) => {
    for (const [locale, clock] of Object.entries(SHARE_CLOCK_WORDS)) {
      const shown = await page.evaluate(async (locale) => {
        const T = window.__SHARE_T__;
        await setLocale(locale, { refresh: false, persist: false });
        const seen = {};
        for (const [mode, required] of [["off", false], ["on", true]]) {
          const id = `share-${locale}-${mode}`;
          T.fire("computer:share", { id, app: "Notes", timeoutMs: 30000, required });
          await T.settle();
          seen[mode] = T.popup().clock;
          T.fire("computer:share-closed", { id });
          await T.settle();
        }
        return seen;
      }, locale);
      ok(`${locale}: unrequired, the clock says the run goes on unshared`,
        (shown.off ?? "").includes(clock.unshared) && !(shown.off ?? "").includes(clock.waits), JSON.stringify(shown));
      ok(`${locale}: required, the clock says the agent waits`,
        (shown.on ?? "").includes(clock.waits) && !(shown.on ?? "").includes(clock.unshared), JSON.stringify(shown));
    }
  });

  await scenario(browser, origin, "waiting app", ok, async (page) => {
    const said = await page.evaluate(async () => {
      const T = window.__SHARE_T__;
      T.fire("computer:share-state", { target: null, takenBack: false, waiting: "notes", waitingLabel: "Notes" });
      await T.settle();
      const waiting = T.band();
      document.getElementById("computer-band-hand")?.click();
      await T.settle();
      T.fire("computer:share-state", { target: "safari", label: "Safari", takenBack: false, waiting: "notes", waitingLabel: "Notes" });
      await T.settle();
      const beside = T.band();
      T.fire("computer:share-state", { target: "notes", label: "Notes", takenBack: false, waiting: null });
      await T.settle();
      return { waiting, beside, shared: T.band(), handed: T.handed };
    });
    ok("the band stands while an app waits for the person", said.waiting.shown, JSON.stringify(said.waiting));
    ok("the band names the app that waits", (said.waiting.wait ?? "").includes("Notes"), JSON.stringify(said.waiting));
    ok("the band offers to hand the waiting app over", said.waiting.hand === words.allow && said.waiting.take === null, JSON.stringify(said.waiting));
    ok("the hand-over is the person's", said.waiting.handPersonOnly, JSON.stringify(said.waiting));
    ok("the hand-over names the waiting app to the backend once",
      said.handed.length === 1 && said.handed[0].app === "notes", JSON.stringify(said.handed));
    ok("beside a share, both the shared app and the waiting one are named",
      (said.beside.share ?? "").includes("Safari") && (said.beside.wait ?? "").includes("Notes") && said.beside.take === words.take,
      JSON.stringify(said.beside));
    ok("once handed over nothing waits", said.shared.wait === null && said.shared.hand === null, JSON.stringify(said.shared));
  });
}
