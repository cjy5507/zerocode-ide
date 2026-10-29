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
const BACKUP_NOTE_KO = "백업에는 옮기기 전의 비밀 값이 그대로 들어 있습니다. 옮긴 것이 잘 되는지 확인한 뒤 백업 파일을 지워 주세요.";
const WHERE_KO = "홈 폴더나 /(루트)에서 연 zo는 이 참조를 풀지 않아 그 MCP 서버를 띄우지 않습니다 — 프로젝트 폴더에서 zo를 여세요.";
const STOPPED_TITLE_KO = "설정 파일의 비밀을 다 옮기지 못했습니다";
const UNLISTED_KO = "남은 목록을 zo에게서 다시 받지 못했습니다. 설정을 다시 열어 확인해 주세요.";
const BACKUP = "/fixture/home/.zo/settings.json.bak-20261027-fixture";

/* The stubs and a record of every call, put on the page before it boots. */
const installHands = (sentinel) => {
  // `relist`, when set, is what the listing answers after a move (`"throw"`:
  // it fails) — in place of the names the move left.
  window.__SECRETS__ = { listed: null, listThrows: false, outcome: null, moveThrows: null, relist: undefined };
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
    if (S.moved && S.relist !== undefined) {
      if (S.relist === "throw") throw new Error("fixture: the listing failed");
      return S.relist;
    }
    return S.listed;
  };
  window.__ANSWER__.move_settings_secrets = (args, options) => {
    window.__SECRET_CALLS__.push({
      name: "move_settings_secrets",
      args: JSON.stringify(args ?? null),
      hand: options?.headers?.["zerocode-hand"] ?? null,
    });
    S.moved = true;
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
    backupNote: said("settings-secrets-backup-note"),
    where: said("settings-secrets-where"),
    why: said("settings-secrets-why"),
    linked: said("settings-secrets-linked"),
    move: said("settings-secrets-move"),
    asked: window.__SECRET_CALLS__.filter((call) => call.name === "settings_secret_literals").length,
    moves: window.__SECRET_CALLS__.filter((call) => call.name === "move_settings_secrets"),
  };
};

/* Waits until the line has asked for the listing `asks` times and pressed the
 * move `moves` times, and is busy with neither: each call the stub counted has
 * come back and been painted. Nothing here waits on a clock. */
async function settled(page, { asks, moves = 0 }) {
  await page.waitForFunction(([asks, moves]) => {
    const calls = window.__SECRET_CALLS__;
    const line = document.getElementById("settings-secrets");
    return calls.filter((call) => call.name === "settings_secret_literals").length === asks &&
      calls.filter((call) => call.name === "move_settings_secrets").length === moves &&
      !!line && line.getAttribute("aria-busy") === "false";
  }, [asks, moves]);
}

/* Presses the move, and waits for it and the listing asked after it. */
async function pressMove(page) {
  await page.click("#settings-secrets-move");
  await settled(page, { asks: 2, moves: 1 });
}

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
  await settled(page, { asks: 1 });
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
    ok(
      "before the press the line already says zo's backup will hold the old values",
      seen.body?.includes("그 백업에는 옛 값이 그대로 들어 있습니다") && seen.backupNote === null && seen.where === null,
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
    await pressMove(page);
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
    ok(
      "beside the backup the line says it still holds the old values, to be removed once the move is checked",
      seen.backupNote === BACKUP_NOTE_KO && !/<button|삭제|지우기 단추/.test(seen.backupNote ?? ""),
      JSON.stringify(seen),
    );
    ok(
      "after a move the line says a zo opened in the home folder or at / does not resolve the reference",
      seen.where === WHERE_KO,
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
    await pressMove(page);
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
    await pressMove(page);
    const seen = await page.evaluate(look);
    ok(
      "a refused move says zo's own reason and that the keychain item was left for the next try",
      seen.shown && seen.why ===
        `zo가 설정 파일의 ${NAMES[0]} 칸을 고치지 않았습니다. zo: ${why} 키체인 항목 ${NAMES[0]}은(는) 그대로 두었습니다 — 다음에 옮길 때 그대로 씁니다.`,
      JSON.stringify(seen),
    );
  });
}

/* t-77216: a settings file whose own name is a link (stow, home-manager).
 * The names zo read through it are said with the file the link stands on,
 * and no move is offered for them; a file zo did not read is said with the
 * page's sentence for zo's word, and zo's own sentence for a word the page
 * does not know. Before, the line showed nothing at all. */
async function linkedSettings(browser, origin, ok) {
  const target = "/fixture/home/dotfiles/zo-settings.json";
  const file = "/fixture/home/.zo/settings.json";
  await scenario(browser, origin, "linked settings", ok, async (page) => {
    await page.evaluate(([names, target]) => {
      window.__SECRETS__.listed = { names: [], linked: [names[0]], linkedTo: target };
    }, [NAMES, target]);
    await openSettings(page);
    const seen = await page.evaluate(look);
    ok(
      "names zo read through the settings file's link are said with the file it points to, and offer no move",
      seen.shown && seen.title === "설정 파일에 비밀 1개가 그대로 적혀 있습니다" && seen.move === null &&
        seen.body === null &&
        seen.linked === `설정 파일이 링크라 창은 옮기지 않습니다: ${NAMES[0]} — 링크가 가리키는 ${target}에서 직접 고쳐 주세요.`,
      JSON.stringify(seen),
    );
  });
  await scenario(browser, origin, "skipped settings", ok, async (page) => {
    const why = "fixture: a word this page does not know";
    await page.evaluate(([file, why]) => {
      window.__SECRETS__.listed = {
        names: [],
        skipped: [
          { file, reason: "link_target_writable_by_others", why: "fixture: writable by others" },
          { file, reason: "a_word_from_a_later_zo", why },
        ],
      };
    }, [file, why]);
    await openSettings(page);
    const seen = await page.evaluate(look);
    ok(
      "a settings file zo did not read is said with the page's sentence for zo's word, and zo's own for a word it does not know",
      seen.shown && seen.title === "zo가 읽지 않은 설정 파일이 있습니다" && seen.move === null &&
        seen.linked === `${file}이(가) 가리키는 파일은 다른 사용자도 고칠 수 있어 zo가 읽지 않았습니다. 그 파일의 그룹·다른 사용자 쓰기 권한을 끄면 읽습니다. zo: ${why}`,
      JSON.stringify(seen),
    );
  });
}

/* A move that stopped, after which the listing could not be read again: the
 * names are unknown, and nothing may read as a finished move. */
async function stoppedUnlisted(browser, origin, ok) {
  for (const [label, setup] of [
    ["zo stopped answering and the listing came back empty", (S, names) => {
      S.listed = { names };
      S.outcome = { moved: [], stopped: { name: "", reason: "zo_silent", why: null, keychainKept: false }, left: [] };
      S.relist = null;
    }],
    ["the move itself failed and the listing failed too", (S, names) => {
      S.listed = { names };
      S.moveThrows = "fixture: the window could not start the move";
      S.relist = "throw";
    }],
    ["one moved, the next stopped and the listing came back empty", (S, names) => {
      S.listed = { names };
      S.outcome = {
        moved: [{ name: names[0], backup: "/fixture/home/.zo/settings.json.bak-fixture" }],
        stopped: { name: names[1], reason: "move_refused", why: null, keychainKept: true },
        left: [],
      };
      S.relist = null;
    }],
  ]) {
    await scenario(browser, origin, `stopped, unlisted: ${label}`, ok, async (page) => {
      await page.evaluate(`(${setup})(window.__SECRETS__, ${JSON.stringify(NAMES)})`);
      await openSettings(page);
      await pressMove(page);
      const seen = await page.evaluate(look);
      ok(
        `when ${label}, the title is not the finished one and the line says the list was not read again`,
        seen.shown && seen.title === STOPPED_TITLE_KO && seen.title !== "설정 파일의 비밀을 키체인으로 옮겼습니다" &&
          !!seen.why && seen.why.includes(UNLISTED_KO),
        JSON.stringify(seen),
      );
    });
  }
}

/* zo said it moved the last cell, and the file it left is not the one the
 * window read with that cell turned into its reference: the line says so in
 * each language, and nothing reads as a finished move. */
async function stoppedUnconfirmed(browser, origin, ok) {
  await scenario(browser, origin, "last cell unconfirmed", ok, async (page) => {
    await page.evaluate(([names, backup]) => {
      window.__SECRETS__.listed = { names: [names[0]] };
      window.__SECRETS__.outcome = {
        moved: [{ name: names[0], backup }],
        stopped: { name: names[0], reason: "unconfirmed", why: null, keychainKept: true },
        left: [],
      };
      window.__SECRETS__.relist = { names: [] };
    }, [NAMES, BACKUP]);
    await openSettings(page);
    await pressMove(page);
    const seen = await page.evaluate(look);
    ok(
      "a last cell zo said it moved, in a file not the one expected, is said as such and not as a finished move",
      seen.shown && seen.title === STOPPED_TITLE_KO && seen.done === `옮김 · 백업 ${BACKUP}` &&
        seen.why ===
          `zo는 ${NAMES[0]} 칸을 옮겼다고 답했지만, 남은 설정 파일이 그 칸만 참조로 바뀐 파일이 아닙니다. 설정 파일과 백업을 확인해 주세요. ` +
          `키체인 항목 ${NAMES[0]}은(는) 그대로 두었습니다 — 다음에 옮길 때 그대로 씁니다. ${UNLISTED_KO}`,
      JSON.stringify(seen),
    );
    const whys = await page.evaluate((codes) => {
      const out = {};
      for (const code of codes) {
        setLocale(code, { persist: false, refresh: false });
        out[code] = document.getElementById("settings-secrets-why").textContent.trim();
      }
      setLocale("ko", { persist: false, refresh: false });
      return out;
    }, Object.keys(SETTINGS_SECRET_WORDS));
    ok(
      "the unconfirmed move speaks each of the five languages",
      new Set(Object.values(whys).filter(Boolean)).size === 5 &&
        Object.values(whys).every((why) => why.includes(NAMES[0])),
      JSON.stringify(whys),
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
  await scenario(browser, origin, "languages after a move", ok, async (page) => {
    await page.evaluate(([names, backup]) => {
      window.__SECRETS__.listed = { names };
      window.__SECRETS__.outcome = { moved: names.map((name) => ({ name, backup })), stopped: null, left: [] };
    }, [NAMES, BACKUP]);
    await openSettings(page);
    await pressMove(page);
    const notes = await page.evaluate((codes) => {
      const out = {};
      for (const code of codes) {
        setLocale(code, { persist: false, refresh: false });
        out[code] = document.getElementById("settings-secrets-backup-note").textContent.trim();
        out[`${code}-where`] = document.getElementById("settings-secrets-where").textContent.trim();
      }
      setLocale("ko", { persist: false, refresh: false });
      return out;
    }, Object.keys(SETTINGS_SECRET_WORDS));
    ok(
      "the backup's warning and where zo resolves the reference speak each of the five languages",
      new Set(Object.values(notes).filter(Boolean)).size === 10 && notes.ko === BACKUP_NOTE_KO && notes["ko-where"] === WHERE_KO,
      JSON.stringify(notes),
    );
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
    await pressMove(page);
    await shot(page, "stopped-390.png");
    for (const code of Object.keys(SETTINGS_SECRET_WORDS)) {
      const spill = await page.evaluate(async (code) => {
        setLocale(code, { persist: false, refresh: false });
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
  await linkedSettings(browser, origin, ok);
  await stoppedUnlisted(browser, origin, ok);
  await stoppedUnconfirmed(browser, origin, ok);
  await oldZo(browser, origin, ok);
  await languages(browser, origin, ok);
  await narrow(browser, origin, ok);
}
