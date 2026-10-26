import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { openWindowTestPage } from "./window-boot.mjs";

/* The plain-text secrets line (t-76698): once the settings are drawn, the
 * window asks zo which cells of its settings file hold a secret as plain text
 * and names them in one line, with one press that moves them into the
 * keychain.
 *
 * The backend answers `settings_secret_literals` (names, or null) and
 * `move_settings_secrets` (what moved, where it stopped). This suite stands
 * in for both, drives the line only through what a person does (opening the
 * settings, pressing the button) and reads the result off the document. A
 * value never reaches the page; the suite still searches the document, both
 * storages, the console and every backend call for the sentinel the backend
 * stub was built around, and checks the move sends the page nothing.
 *
 *   WINDOW_SUITES=settings-secrets node ui/tests/window.mjs
 *   SETTINGS_SECRETS_SHOTS_DIR=<dir> ...   the line at 1280px and at 390px */

/* The value the backend is pretending to hold. Never shown, never sent. */
const SENTINEL = "SENTINEL-not-a-secret-0123456789abcdefgh";

/* The line's words in the five languages the window speaks, pinned here so a
 * catalog that drifted, or a key nobody translated, fails. */
export const SETTINGS_SECRET_WORDS = Object.freeze({
  ko: { title: "설정 파일에 비밀 2개가 그대로 적혀 있습니다", move: "키체인으로 옮기기" },
  en: { title: "Secrets written as plain text in the settings file: 2", move: "Move to keychain" },
  ja: { title: "設定ファイルにシークレットが2個そのまま書かれています", move: "キーチェーンに移す" },
  zh: { title: "设置文件中有 2 个密钥以明文写着", move: "移到钥匙串" },
  es: { title: "Secretos escritos como texto plano en el archivo de ajustes: 2", move: "Mover al llavero" },
});

const NAMES = ["FIXTURE_TOKEN", "OTHER_FIXTURE_KEY"];
const BACKUP = "/fixture/home/.zo/settings.json.bak-20261027-fixture";

/* The stubs and a record of every call, put on the page before it boots. */
const installHands = (sentinel) => {
  window.__SECRETS__ = { listed: null, listThrows: false, outcome: null, moveThrows: null };
  window.__SECRET_CALLS__ = [];
  window.__SECRET_CONSOLE__ = [];
  for (const level of ["log", "info", "warn", "error", "debug"]) {
    const original = console[level].bind(console);
    console[level] = (...args) => {
      window.__SECRET_CONSOLE__.push(args.map((one) => String(one)).join(" "));
      original(...args);
    };
  }
  window.__SECRET_SENTINEL__ = sentinel;
};

const stubBackend = () => {
  const S = window.__SECRETS__;
  window.__ANSWER__.settings_secret_literals = (args) => {
    window.__SECRET_CALLS__.push({ name: "settings_secret_literals", args: JSON.stringify(args ?? null) });
    if (S.listThrows) throw new Error("zo: unknown argument 'settings'");
    return S.listed;
  };
  window.__ANSWER__.move_settings_secrets = (args, options) => {
    window.__SECRET_CALLS__.push({
      name: "move_settings_secrets",
      args: JSON.stringify(args ?? null),
      hand: options?.headers?.["zerocode-hand"] ?? null,
    });
    if (S.moveThrows) throw S.moveThrows;
    const outcome = S.outcome;
    // What the backend lists after a move: what is left.
    S.listed = outcome?.stopped ? { names: [outcome.stopped.name, ...(outcome.left ?? [])] } : null;
    return outcome;
  };
};

const look = () => {
  const said = (id) => {
    const node = document.getElementById(id);
    return node && !node.hidden && !node.closest("[hidden]") ? node.textContent.trim() : null;
  };
  const line = document.getElementById("settings-secrets");
  return {
    shown: !!line && !line.hidden && !line.closest("[hidden]"),
    title: said("settings-secrets-title"),
    names: said("settings-secrets-names"),
    body: said("settings-secrets-body"),
    done: said("settings-secrets-done"),
    why: said("settings-secrets-why"),
    move: said("settings-secrets-move"),
    asked: window.__SECRET_CALLS__.filter((call) => call.name === "settings_secret_literals").length,
    moves: window.__SECRET_CALLS__.filter((call) => call.name === "move_settings_secrets"),
  };
};

const settle = (page, ms = 250) => page.waitForTimeout(ms);

const shot = async (page, file) => {
  const dir = process.env.SETTINGS_SECRETS_SHOTS_DIR;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.locator("#settings-secrets").screenshot({ path: join(dir, file) });
};

async function scenario(browser, origin, name, ok, body, { width = 1280 } = {}) {
  const { page } = await openWindowTestPage(browser, origin, {
    before: (held) => held.addInitScript(installHands, SENTINEL),
  });
  try {
    if (width !== 1280) await page.setViewportSize({ width, height: 860 });
    await page.evaluate(() => setLocale("ko", { persist: false, refresh: false }));
    await page.evaluate(stubBackend);
    await body(page);
  } catch (error) {
    ok(`the ${name} scenario ran to its end`, false, String(error?.stack ?? error).split("\n").slice(0, 3).join(" | "));
  } finally {
    await page.close();
  }
}

/* Opens the settings the way the gear does, and waits for the line's ask. */
async function openSettings(page) {
  await page.evaluate(() => setSettingsOpen(true));
  await page.waitForFunction(() => !document.getElementById("settings-view").hidden);
  await settle(page);
}

const keepsOf = () => {
  const storage = {};
  for (const store of [localStorage, sessionStorage]) {
    for (let at = 0; at < store.length; at += 1) {
      const key = store.key(at);
      storage[key] = store.getItem(key);
    }
  }
  const all = JSON.stringify({
    document: document.documentElement.outerHTML,
    storage,
    console: window.__SECRET_CONSOLE__,
    calls: window.__SECRET_CALLS__,
  });
  return { holds: all.includes(window.__SECRET_SENTINEL__), searched: all.length };
};

async function named(browser, origin, ok) {
  await scenario(browser, origin, "named", ok, async (page) => {
    await page.evaluate((names) => { window.__SECRETS__.listed = { names }; }, NAMES);
    const before = await page.evaluate(look);
    ok("the window does not ask zo before the settings open", before.asked === 0 && !before.shown, JSON.stringify(before));
    await openSettings(page);
    const seen = await page.evaluate(look);
    await shot(page, "named-1280.png");
    ok(
      "opening the settings asks once and names each secret, its count in the title",
      seen.asked === 1 && seen.shown && seen.title === SETTINGS_SECRET_WORDS.ko.title &&
        seen.names === NAMES.join(", ") && seen.move === SETTINGS_SECRET_WORDS.ko.move && seen.done === null && seen.why === null,
      JSON.stringify(seen),
    );
    const kept = await page.evaluate(keepsOf);
    ok("no value is in the document, storage, console or any backend call", !kept.holds, `searched ${kept.searched} characters`);
  });
}

async function moved(browser, origin, ok) {
  await scenario(browser, origin, "moved", ok, async (page) => {
    await page.evaluate(([names, backup]) => {
      window.__SECRETS__.listed = { names };
      window.__SECRETS__.outcome = { moved: names.map((name) => ({ name, backup })), stopped: null, left: [] };
    }, [NAMES, BACKUP]);
    await openSettings(page);
    await page.click("#settings-secrets-move");
    await settle(page);
    const seen = await page.evaluate(look);
    await shot(page, "moved-1280.png");
    ok(
      "the press moves once, sends nothing but the person's hand, and asks the listing again",
      seen.moves.length === 1 && seen.moves[0].args === "{}" && seen.moves[0].hand === "person" && seen.asked === 2,
      JSON.stringify(seen.moves),
    );
    ok(
      "a full move says it with zo's backup, and the button goes",
      seen.shown && seen.title === "설정 파일의 비밀을 키체인으로 옮겼습니다" &&
        seen.done === `옮김 · 백업 ${BACKUP}` && seen.move === null && seen.names === null && seen.why === null,
      JSON.stringify(seen),
    );
    const kept = await page.evaluate(keepsOf);
    ok("after the move no value is anywhere the page keeps", !kept.holds, `searched ${kept.searched} characters`);
  });
}

async function stoppedDiffers(browser, origin, ok) {
  await scenario(browser, origin, "keychain differs", ok, async (page) => {
    await page.evaluate((names) => {
      window.__SECRETS__.listed = { names };
      window.__SECRETS__.outcome = {
        moved: [],
        stopped: { name: names[0], reason: "keychain_differs", why: null, keychainKept: false },
        left: [names[1]],
      };
    }, NAMES);
    await openSettings(page);
    await page.click("#settings-secrets-move");
    await settle(page);
    const seen = await page.evaluate(look);
    ok(
      "a different value under the name stops the move and says so, with what was not tried",
      seen.shown && seen.why?.startsWith(`키체인에 같은 이름(${NAMES[0]})의 항목이 이미 다른 값으로 있어 덮어쓰지 않았습니다.`) &&
        seen.why.includes(`아직 옮기지 않은 것: ${NAMES[1]}`) && !seen.why.includes("그대로 두었습니다") &&
        seen.names === NAMES.join(", ") && seen.move === SETTINGS_SECRET_WORDS.ko.move && seen.done === null,
      JSON.stringify(seen),
    );
  });
}

async function stoppedByZo(browser, origin, ok) {
  await scenario(browser, origin, "move refused", ok, async (page) => {
    const why = "fixture: the settings file changed after it was read";
    await page.evaluate(([names, why]) => {
      window.__SECRETS__.listed = { names: [names[0]] };
      window.__SECRETS__.outcome = {
        moved: [],
        stopped: { name: names[0], reason: "move_refused", why, keychainKept: true },
        left: [],
      };
    }, [NAMES, why]);
    await openSettings(page);
    await page.click("#settings-secrets-move");
    await settle(page);
    const seen = await page.evaluate(look);
    ok(
      "a refused move says zo's own reason and that the keychain item was left for the next try",
      seen.shown && seen.why ===
        `zo가 설정 파일의 ${NAMES[0]} 칸을 고치지 않았습니다. zo: ${why} 키체인 항목 ${NAMES[0]}은(는) 그대로 두었습니다 — 다음에 옮길 때 그대로 씁니다.`,
      JSON.stringify(seen),
    );
  });
}

async function oldZo(browser, origin, ok) {
  for (const [label, setup] of [
    ["answers nothing", (S) => { S.listed = null; }],
    ["refuses the door", (S) => { S.listThrows = true; }],
  ]) {
    await scenario(browser, origin, `a zo that ${label}`, ok, async (page) => {
      await page.evaluate(`(${setup})(window.__SECRETS__)`);
      await openSettings(page);
      const seen = await page.evaluate(look);
      ok(`a zo that ${label} shows no line and no error`, seen.asked === 1 && !seen.shown, JSON.stringify(seen));
    });
  }
}

async function languages(browser, origin, ok) {
  await scenario(browser, origin, "languages", ok, async (page) => {
    await page.evaluate((names) => { window.__SECRETS__.listed = { names }; }, NAMES);
    await openSettings(page);
    const seen = await page.evaluate(async (codes) => {
      const out = {};
      for (const code of codes) {
        setLocale(code, { persist: false, refresh: false });
        await new Promise((done) => setTimeout(done, 30));
        out[code] = {
          title: document.getElementById("settings-secrets-title").textContent.trim(),
          move: document.getElementById("settings-secrets-move").textContent.trim(),
          body: document.getElementById("settings-secrets-body").textContent.trim(),
        };
      }
      setLocale("ko", { persist: false, refresh: false });
      return out;
    }, Object.keys(SETTINGS_SECRET_WORDS));
    const bodies = new Set(Object.values(seen).map((one) => one.body));
    for (const [code, words] of Object.entries(SETTINGS_SECRET_WORDS)) {
      const got = seen[code] ?? {};
      ok(`the line speaks ${code}`, got.title === words.title && got.move === words.move, JSON.stringify({ want: words, got }));
    }
    ok("each language has its own explanation", bodies.size === 5, JSON.stringify([...bodies]));
  });
}

async function narrow(browser, origin, ok) {
  await scenario(browser, origin, "390px", ok, async (page) => {
    const long = ["A_VERY_LONG_FIXTURE_SECRET_NAME_THAT_HAS_NO_BREAK_IN_IT_AT_ALL", ...NAMES];
    await page.evaluate(([names, backup]) => {
      window.__SECRETS__.listed = { names };
      window.__SECRETS__.outcome = {
        moved: [{ name: names[1], backup: `${backup}/${"deep-folder-name/".repeat(4)}settings.json.bak` }],
        stopped: { name: names[0], reason: "move_refused", why: "fixture: a reason long enough to wrap across the line twice over at this width", keychainKept: true },
        left: [names[2]],
      };
    }, [long, BACKUP]);
    await openSettings(page);
    await page.click("#settings-secrets-move");
    await settle(page);
    await shot(page, "stopped-390.png");
    for (const code of Object.keys(SETTINGS_SECRET_WORDS)) {
      const spill = await page.evaluate(async (code) => {
        setLocale(code, { persist: false, refresh: false });
        await new Promise((done) => setTimeout(done, 30));
        const line = document.getElementById("settings-secrets");
        const box = line.getBoundingClientRect();
        const over = [line, ...line.querySelectorAll("*")]
          .filter((node) => node.getClientRects().length > 0)
          .filter((node) => node.scrollWidth > node.clientWidth + 1 || node.getBoundingClientRect().right > window.innerWidth + 0.5)
          .map((node) => node.id || node.className);
        return { shown: !line.hidden, width: window.innerWidth, left: box.left, right: box.right, over };
      }, code);
      ok(`at 390px in ${code} nothing in the line spills past its box or the window`, spill.shown && spill.width === 390 && spill.over.length === 0, JSON.stringify(spill));
    }
  }, { width: 390 });
}

export async function testSettingsSecrets(browser, origin, ok) {
  await named(browser, origin, ok);
  await moved(browser, origin, ok);
  await stoppedDiffers(browser, origin, ok);
  await stoppedByZo(browser, origin, ok);
  await oldZo(browser, origin, ok);
  await languages(browser, origin, ok);
  await narrow(browser, origin, ok);
}
