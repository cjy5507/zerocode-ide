import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { openWindowTestPage } from "./window-boot.mjs";

/* The secret card (herdr 3, t-26596): a pane that waits for a password, a
 * passphrase or a PIN gets a password field in the popup, and the value goes
 * into that pane once.
 *
 * The backend says which panes wait (`panes_secret`) and takes the value
 * (`answer_secret`). This suite stands in for it and drives the card only
 * through what a person does (typing, Enter, the buttons), and reads the result
 * off the document. It also searches everything the page can keep — the
 * document, the storage, the console, and every backend call but the one that
 * types — for the value. The value must be found in that one call and nowhere
 * else.
 *
 *   WINDOW_SUITES=secret-prompt node ui/tests/window.mjs
 *   SECRET_SHOTS_DIR=<dir> ...        the before and after screenshots
 *   WINDOW_CPU_THROTTLE=4 ...         the low-spec profile (the numbers are printed) */

/* A value that stands in for a secret. It is searched for, never shown. */
const SENTINEL = "SENTINEL-not-a-secret-7f3a";

/* The card's words in the five languages the window speaks, pinned here in
 * words so a catalog that drifted into another language, or a key nobody
 * translated, fails instead of agreeing with itself. */
export const SECRET_WORDS = Object.freeze({
  ko: {
    title: "비밀번호를 입력해 주세요",
    send: "보내기",
    cancel: "취소",
    label: "입력",
    hint: "입력한 값은 판에 한 번만 입력되고 어디에도 저장되지 않습니다 · 에이전트는 값을 받지 않습니다",
    empty: "값을 입력해 주세요",
    busy: "다른 답이 아직 가는 중입니다 — 잠시 뒤 다시 보내 주세요",
  },
  en: {
    title: "Enter the password",
    send: "Send",
    cancel: "Cancel",
    label: "Value",
    hint: "The value is typed into the pane once and stored nowhere · the agent never receives it",
    empty: "Type the value first",
    busy: "Another answer is still on its way — send again in a moment",
  },
  ja: {
    title: "パスワードを入力してください",
    send: "送信",
    cancel: "キャンセル",
    label: "値",
    hint: "入力した値はペインに一度だけ入力され、どこにも保存されません · エージェントは値を受け取りません",
    empty: "値を入力してください",
    busy: "別の回答がまだ送信中です — しばらくしてから再度送ってください",
  },
  zh: {
    title: "请输入密码",
    send: "发送",
    cancel: "取消",
    label: "内容",
    hint: "输入的内容只会在该窗格中输入一次，不会保存在任何地方 · 智能体不会收到它",
    empty: "请先输入内容",
    busy: "另一个回答仍在发送中，请稍后再发送",
  },
  es: {
    title: "Escribe la contraseña",
    send: "Enviar",
    cancel: "Cancelar",
    label: "Valor",
    hint: "El valor se escribe una vez en el panel y no se guarda en ningún sitio · el agente no lo recibe",
    empty: "Escribe primero el valor",
    busy: "Otra respuesta sigue en camino: envía de nuevo en un momento",
  },
});

/* One question a pane waits on, as `panes_secret` answers it. `since` is the
 * pane's output time: a question that stays the same keeps its key. */
const WAIT = Object.freeze({ term: 4, kind: "password", line: "[sudo] password for dev:", since: 1000 });

/* The hands every scenario shares, put on the page once: the Korean the
 * assertions are written in, the two stubs, a record of every backend call, and
 * what the console said. */
const installSecretHands = () => {
  setLocale("ko", { persist: false, refresh: false });
  const T = {
    settle: (ms = 1200) => new Promise((done) => setTimeout(done, ms)),
    waiting: [],
    /* What the next answer returns: `null` delivers, a string is thrown the way
     * the backend throws it (`answer_door`'s words). */
    reply: null,
    look() {
      const scrim = document.getElementById("ask-scrim");
      const card = document.getElementById("ask-secret");
      const input = document.getElementById("ask-secret-input");
      const said = (id) => {
        const node = document.getElementById(id);
        return node && !node.hidden && !node.closest("[hidden]") ? node.textContent.trim() : null;
      };
      const buttons = [...(scrim?.querySelectorAll("#ask-choices button") ?? [])]
        .filter((one) => !one.closest("[hidden]"))
        .map((one) => one.textContent.trim());
      return {
        shown: !!scrim && !scrim.hidden && !!card && !card.hidden,
        title: said("ask-title"),
        mono: said("ask-body"),
        label: said("ask-secret-label"),
        hint: said("ask-secret-hint"),
        error: said("ask-secret-error"),
        buttons,
        inputType: input?.type ?? null,
        autocomplete: input?.getAttribute("autocomplete") ?? null,
        autocapitalize: input?.getAttribute("autocapitalize") ?? null,
        spellcheck: input?.getAttribute("spellcheck") ?? null,
        focused: document.activeElement === input,
        value: input?.value ?? null,
        inForm: !!input?.closest("form"),
      };
    },
    type(text) {
      const input = document.getElementById("ask-secret-input");
      if (input) input.value = text;
      return input?.value ?? null;
    },
    enter() {
      document
        .getElementById("ask-secret-input")
        ?.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    },
    press(label) {
      const button = [...(document.querySelectorAll("#ask-choices button") ?? [])].find(
        (one) => one.textContent.trim() === label,
      );
      button?.click();
      return button !== undefined;
    },
    calls(name) {
      return window.__CALLS__.filter((call) => call.name === name);
    },
  };
  window.__ST__ = T;
  window.__CALLS__ = [];
  window.__CONSOLE__ = [];
  for (const level of ["log", "info", "warn", "error", "debug"]) {
    const original = console[level].bind(console);
    console[level] = (...args) => {
      window.__CONSOLE__.push(args.map((one) => String(one)).join(" "));
      original(...args);
    };
  }
  window.__ANSWER__.panes_secret = () => T.waiting.map((one) => ({ ...one }));
  window.__ANSWER__.answer_secret = (args) => {
    window.__SECRET_SENT__ = JSON.parse(JSON.stringify(args));
    if (typeof T.reply === "string") throw new Error(T.reply);
    return null;
  };
  // Every command the page calls passes through one record, so a search can see
  // what each of them was given.
  for (const name of Object.keys(window.__ANSWER__)) {
    const original = window.__ANSWER__[name];
    window.__ANSWER__[name] = (args) => {
      window.__CALLS__.push({ name, args: JSON.stringify(args ?? null) });
      return original(args);
    };
  }
};

/* Everything the page keeps that could hold a value, as one string: the
 * document, both storages, the console, and every backend call but the one that
 * types. */
const keepsOf = () => {
  const storage = {};
  for (const store of [localStorage, sessionStorage]) {
    for (let at = 0; at < store.length; at += 1) {
      const key = store.key(at);
      storage[key] = store.getItem(key);
    }
  }
  return JSON.stringify({
    document: document.documentElement.outerHTML,
    storage,
    console: window.__CONSOLE__,
    calls: window.__CALLS__.filter((call) => call.name !== "answer_secret"),
  });
};

/* Runs one scenario on a fresh page; a throw becomes one failed check with its
 * reason, so one broken scenario does not hide the rest. */
async function scenario(browser, origin, name, ok, body) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(installSecretHands);
    await body(page, faults);
  } catch (error) {
    ok(`the ${name} scenario ran to its end`, false, String(error?.stack ?? error).split("\n").slice(0, 3).join(" | "));
  } finally {
    await page.close();
  }
}

const shot = async (page, file) => {
  const dir = process.env.SECRET_SHOTS_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, file) });
};

async function raised(browser, origin, ok) {
  await scenario(browser, origin, "password question", ok, async (page) => {
    await shot(page, "secret-before.png");
    const seen = await page.evaluate(async (wait) => {
      const T = window.__ST__;
      T.waiting = [wait];
      await T.settle();
      return T.look();
    }, WAIT);
    await shot(page, "secret-after.png");
    ok(
      "a pane that waits for a password raises a password field in the popup",
      seen.shown && seen.title === SECRET_WORDS.ko.title && seen.mono === WAIT.line,
      JSON.stringify(seen),
    );
    ok(
      "the field is a real password input that no form owns and no browser keeps",
      seen.inputType === "password" && seen.autocomplete === "new-password" &&
        seen.autocapitalize === "none" && seen.spellcheck === "false" && seen.inForm === false,
      JSON.stringify(seen),
    );
    ok("the field takes the keyboard when the question appears", seen.focused === true, JSON.stringify(seen));
    ok(
      "the card offers Send and Cancel",
      JSON.stringify(seen.buttons) === JSON.stringify([SECRET_WORDS.ko.send, SECRET_WORDS.ko.cancel]),
      JSON.stringify(seen.buttons),
    );
  });
}

async function typedOnce(browser, origin, ok) {
  await scenario(browser, origin, "typed once", ok, async (page) => {
    const sent = await page.evaluate(async ([wait, value]) => {
      const T = window.__ST__;
      T.waiting = [wait];
      await T.settle();
      T.type(value);
      T.enter();
      await T.settle(300);
      return {
        calls: T.calls("answer_secret").length,
        sent: window.__SECRET_SENT__ ?? null,
        look: T.look(),
      };
    }, [WAIT, SENTINEL]);
    ok(
      "Enter sends the value to the pane's question once, with the question's own line",
      sent.calls === 1 &&
        sent.sent?.term === WAIT.term && sent.sent?.kind === WAIT.kind && sent.sent?.line === WAIT.line &&
        sent.sent?.value === SENTINEL,
      JSON.stringify({ calls: sent.calls, sent: sent.sent }),
    );
    ok("the popup closes once the answer is delivered", sent.look.shown === false, JSON.stringify(sent.look));
    ok("the field is emptied the moment the answer is sent", sent.look.value === "", JSON.stringify(sent.look));
    const kept = await page.evaluate(keepsOf);
    ok(
      "the value is kept by no document, storage, console or other backend call",
      !kept.includes(SENTINEL),
      kept.includes(SENTINEL) ? "the value was found outside the typing call" : "searched 4 places, none held it",
    );
    // The same question, polled again with nothing new printed, is not asked twice.
    const again = await page.evaluate(async (wait) => {
      const T = window.__ST__;
      T.waiting = [wait];
      await T.settle();
      return { shown: T.look().shown, calls: T.calls("answer_secret").length };
    }, WAIT);
    ok("an answered question that has not changed is not raised again", again.shown === false && again.calls === 1, JSON.stringify(again));
  });
}

async function cancelled(browser, origin, ok) {
  await scenario(browser, origin, "cancel", ok, async (page) => {
    const result = await page.evaluate(async (wait) => {
      const T = window.__ST__;
      T.waiting = [wait];
      await T.settle();
      const pressed = T.press(T.look().buttons.at(-1));
      await T.settle(300);
      const closed = T.look().shown === false;
      await T.settle();
      const staysClosed = T.look().shown === false;
      T.waiting = [{ ...wait, since: wait.since + 1000 }];
      await T.settle();
      return { pressed, closed, staysClosed, raisedAgain: T.look().shown, calls: T.calls("answer_secret").length };
    }, WAIT);
    ok("Cancel closes the card and sends nothing to the pane", result.pressed && result.closed && result.calls === 0, JSON.stringify(result));
    ok("a cancelled question stays closed while the pane prints nothing new", result.staysClosed === true, JSON.stringify(result));
    ok("a question the pane prints again after a cancel is raised again", result.raisedAgain === true, JSON.stringify(result));
  });
}

async function refusedWhileBusy(browser, origin, ok) {
  await scenario(browser, origin, "busy refusal", ok, async (page) => {
    const result = await page.evaluate(async ([wait, value]) => {
      const T = window.__ST__;
      T.reply = "answer-in-flight";
      T.waiting = [wait];
      await T.settle();
      T.type(value);
      T.press(T.look().buttons[0]);
      await T.settle(300);
      const after = T.look();
      T.waiting = [];
      await T.settle();
      return { after, gone: T.look().shown === false };
    }, [WAIT, SENTINEL]);
    ok(
      "a busy pane keeps the popup and the typed value, and says why in the card's words",
      result.after.shown === true && result.after.value === SENTINEL && result.after.error === SECRET_WORDS.ko.busy,
      JSON.stringify(result.after),
    );
    ok("a question the pane no longer asks leaves the popup", result.gone === true, JSON.stringify(result));
  });
}

async function questionChanged(browser, origin, ok) {
  await scenario(browser, origin, "question changed", ok, async (page) => {
    const result = await page.evaluate(async ([wait, value]) => {
      const T = window.__ST__;
      T.reply = "question-changed";
      T.waiting = [wait];
      await T.settle();
      T.type(value);
      T.press(T.look().buttons[0]);
      // The pane moved on as the answer went out: it no longer asks anything.
      T.waiting = [];
      await T.settle(300);
      const left = T.look();
      T.reply = null;
      T.waiting = [{ ...wait, line: "Password:", since: wait.since + 5000 }];
      await T.settle();
      return { left, next: T.look() };
    }, [WAIT, SENTINEL]);
    ok("a refusal because the pane moved on lets the popup go and clears the field", result.left.shown === false && result.left.value === "", JSON.stringify(result.left));
    ok("the next poll raises what the pane asks now", result.next.shown === true && result.next.mono === "Password:", JSON.stringify(result.next));
  });
}

async function emptyRefused(browser, origin, ok) {
  await scenario(browser, origin, "empty send", ok, async (page) => {
    const result = await page.evaluate(async (wait) => {
      const T = window.__ST__;
      T.waiting = [wait];
      await T.settle();
      T.enter();
      await T.settle(200);
      return { look: T.look(), calls: T.calls("answer_secret").length };
    }, WAIT);
    ok(
      "Enter on an empty field sends nothing and says so",
      result.calls === 0 && result.look.shown === true && result.look.error === SECRET_WORDS.ko.empty,
      JSON.stringify(result),
    );
  });
}

async function answeredThenAskedAgain(browser, origin, ok) {
  await scenario(browser, origin, "wrong password asked again", ok, async (page) => {
    const result = await page.evaluate(async ([wait, value]) => {
      const T = window.__ST__;
      T.waiting = [wait];
      await T.settle();
      T.type(value);
      T.enter();
      await T.settle(300);
      // The pane printed "Sorry, try again." and asks once more: its output moved.
      T.waiting = [{ ...wait, since: wait.since + 2000 }];
      await T.settle();
      return { again: T.look(), calls: T.calls("answer_secret").length };
    }, [WAIT, SENTINEL]);
    ok(
      "a wrong password the pane asks for again raises the card again",
      result.again.shown === true && result.calls === 1,
      JSON.stringify(result),
    );
  });
}

async function languages(browser, origin, ok) {
  await scenario(browser, origin, "secret languages", ok, async (page) => {
    const seen = await page.evaluate(async (codes) => {
      const T = window.__ST__;
      const out = {};
      let since = 9000;
      for (const code of codes) {
        setLocale(code, { persist: false, refresh: false });
        since += 1;
        T.waiting = [{ term: 4, kind: "password", line: "[sudo] password for dev:", since }];
        await T.settle();
        const look = T.look();
        T.enter();
        await T.settle(150);
        const refused = T.look();
        out[code] = { title: look.title, label: look.label, hint: look.hint, buttons: look.buttons, empty: refused.error };
        T.press(T.look().buttons.at(-1));
        await T.settle(150);
        T.waiting = [];
        await T.settle(120);
      }
      setLocale("ko", { persist: false, refresh: false });
      return out;
    }, Object.keys(SECRET_WORDS));
    for (const [code, words] of Object.entries(SECRET_WORDS)) {
      const got = seen[code] ?? {};
      ok(
        `the card speaks ${code}`,
        got.title === words.title && got.label === words.label && got.hint === words.hint &&
          JSON.stringify(got.buttons) === JSON.stringify([words.send, words.cancel]) && got.empty === words.empty,
        JSON.stringify({ want: words, got }),
      );
    }
  });
}

async function timing(browser, origin, ok) {
  await scenario(browser, origin, "card timing", ok, async (page) => {
    const rounds = Number(process.env.SECRET_ROUNDS ?? 15) || 15;
    const numbers = await page.evaluate(async (count) => {
      const T = window.__ST__;
      const measured = [];
      for (let round = 0; round < count; round += 1) {
        const started = performance.now();
        T.waiting = [{ term: 4, kind: "password", line: "[sudo] password for dev:", since: 20000 + round }];
        while (!T.look().shown && performance.now() - started < 5000) {
          await new Promise((done) => setTimeout(done, 5));
        }
        measured.push(performance.now() - started);
        T.press(T.look().buttons.at(-1));
        T.waiting = [];
        await T.settle(120);
      }
      const sorted = [...measured].sort((a, b) => a - b);
      const at = (share) => sorted[Math.min(sorted.length - 1, Math.floor(share * sorted.length))];
      return {
        rounds: measured.length,
        medianMs: Math.round(at(0.5) * 10) / 10,
        p95Ms: Math.round(at(0.95) * 10) / 10,
        maxMs: Math.round(sorted[sorted.length - 1] * 10) / 10,
      };
    }, rounds);
    numbers.cpuThrottle = Number(process.env.WINDOW_CPU_THROTTLE ?? 1) || 1;
    console.log(`SECRET_CARD_NUMBERS ${JSON.stringify(numbers)}`);
    ok("a question shows within one poll and one paint", numbers.rounds === rounds && numbers.p95Ms < 1000, JSON.stringify(numbers));
  });
}

export async function testSecretPrompt(browser, origin, ok) {
  await raised(browser, origin, ok);
  await typedOnce(browser, origin, ok);
  await cancelled(browser, origin, ok);
  await refusedWhileBusy(browser, origin, ok);
  await questionChanged(browser, origin, ok);
  await emptyRefused(browser, origin, ok);
  await answeredThenAskedAgain(browser, origin, ok);
  await languages(browser, origin, ok);
  await timing(browser, origin, ok);
}
