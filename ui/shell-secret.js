/* ---- the secret a pane waits for (herdr 3, t-26596) ----
 *
 * A program that asks for a password, a passphrase or a PIN draws its question
 * on the line its cursor stands on, and then waits. The backend reads that line
 * (`zerocode_core::secret_prompt`) and names the panes that wait by
 * `panes_secret`; this file raises each one as a question of the popup, with a
 * password field that types the value into its pane once (`answer_secret`).
 *
 * What the page keeps of the value: nothing but the field. The field is the
 * person's own input. It is read once, when they send, and emptied the moment
 * the answer lands; a question that goes away takes whatever was typed with it.
 * The value is never a draft, a history entry, a log line, a record of the
 * ledger, or a second copy in a variable this file keeps. The source contract
 * (`source_contracts/secret_prompt.rs`) and the window suite
 * (`ui/tests/secret-prompt.mjs`) search for it in all of those places. */

/* How often the window asks which panes wait for a secret. The backend answers
 * from a quiet pane's last line, so one poll costs one lock per pane. */
const SECRET_POLL_MS = 500;

/* The words the backend answers a refused send with (`answer_door`). A refusal
 * the page knows is painted in its own sentence; anything else is a failure. */
const SECRET_QUESTION_CHANGED = "question-changed";
const SECRET_ANSWER_IN_FLIGHT = "answer-in-flight";
const SECRET_INVALID = "secret-invalid";

/* The catalog key of each kind's title, written out whole so each one is
 * findable by its name (shell-i18n.js holds the four other languages). */
const SECRET_TITLE_KEYS = Object.freeze({
  password: "secret.title.password",
  passphrase: "secret.title.passphrase",
  pin: "secret.title.pin",
});

/* The title of each kind, in Korean — the source language. */
const SECRET_TITLES = Object.freeze({
  password: "비밀번호를 입력해 주세요",
  passphrase: "암호 문구를 입력해 주세요",
  pin: "PIN을 입력해 주세요",
});

/* Questions the person has answered or cancelled, by key. A key carries the
 * output time the question was read at, so the same question coming back after
 * the pane printed again (a wrong password, asked a second time) raises again. */
const secretSettled = new Set();

/* The field's owner: the ask whose value it holds. Another ask finds it empty. */
let secretOwner = null;
/* Why the last send was refused, by a key of the catalog; null when nothing is. */
let secretRefusal = null;

const secretInput = el("ask-secret-input");

function secretKeyOf(item) {
  return `secret:${item.term}:${item.since}:${item.line}`;
}

function secretAskOf(item) {
  return {
    kind: "secret",
    key: secretKeyOf(item),
    term: item.term,
    secretKind: item.kind,
    line: item.line,
  };
}

function wipeSecret() {
  secretInput.value = "";
  secretRefusal = null;
  secretOwner = null;
}

function paintSecretRefusal() {
  const error = el("ask-secret-error");
  error.hidden = secretRefusal === null;
  secretInput.setAttribute("aria-invalid", secretRefusal === null ? "false" : "true");
  if (secretRefusal === null) {
    error.textContent = "";
    return;
  }
  const words = {
    empty: t("secret.empty", "값을 입력해 주세요"),
    busy: t("secret.busy", "다른 답이 아직 가는 중입니다 — 잠시 뒤 다시 보내 주세요"),
    invalid: t("secret.invalid", "한 줄짜리 값만 보낼 수 있습니다"),
  };
  error.textContent = words[secretRefusal];
}

function refuseSecret(reason) {
  secretRefusal = reason;
  paintSecretRefusal();
}

registerAskKind("secret", {
  tone: "secret",
  view: (ask) => ({
    agent: null,
    title: t(SECRET_TITLE_KEYS[ask.secretKind] ?? SECRET_TITLE_KEYS.password, SECRET_TITLES[ask.secretKind] ?? SECRET_TITLES.password),
    // The pane's own question, as the pane printed it — never what was typed.
    mono: ask.line,
    why: null,
    choices: [
      { label: t("secret.send", "보내기"), tone: "primary", decision: "send" },
      { label: t("app.cancel", "취소"), tone: "halt-quiet", decision: "cancel" },
    ],
    initial: 0,
    focus: "ask-secret-input",
    safe: 1,
  }),
  paint(ask) {
    el("ask-secret").hidden = false;
    // The same ask repainted (a language change) keeps what the hand has typed;
    // a new ask starts with an empty field.
    if (secretOwner !== ask) wipeSecret();
    secretOwner = ask;
    secretInput.readOnly = ask.sending === true;
    say(el("ask-secret-label"), () => t("secret.label", "입력"));
    say(el("ask-secret-hint"), () =>
      t(
        "secret.hint",
        "입력한 값은 판에 한 번만 입력되고 어디에도 저장되지 않습니다 · 에이전트는 값을 받지 않습니다",
      ),
    );
    paintSecretRefusal();
  },
  clear() {
    // `clear` runs before every repaint, when `activeAsk` is already the ask
    // that comes next: a field that belongs to any other ask is emptied here.
    el("ask-secret").hidden = true;
    if (secretOwner !== activeAsk) wipeSecret();
  },
  withdrawn(ask) {
    if (secretOwner === ask) wipeSecret();
  },
  async deliver(ask, choice) {
    if (choice.decision === "cancel") {
      secretSettled.add(ask.key);
      wipeSecret();
      return;
    }
    const value = secretInput.value;
    if (value === "") {
      refuseSecret("empty");
      throw Object.assign(new Error("secret-empty"), { askHeld: true });
    }
    try {
      await invoke("answer_secret", { term: ask.term, kind: ask.secretKind, line: ask.line, value });
    } catch (error) {
      // The backend's refusal is its word; a thrown Error carries the same word in its message.
      const word = typeof error === "string" ? error : String(error?.message ?? error);
      // The question is no longer on the pane: the popup lets it go, and the
      // next poll says what the pane asks now.
      if (word === SECRET_QUESTION_CHANGED) {
        wipeSecret();
        withdrawAsks((queued) => queued === ask);
        return;
      }
      // A refusal keeps the field, so the person can send again without typing.
      if (word === SECRET_ANSWER_IN_FLIGHT) refuseSecret("busy");
      else if (word === SECRET_INVALID) refuseSecret("invalid");
      else throw error;
      throw Object.assign(new Error(word), { askHeld: true });
    }
    // Delivered: the value leaves the page with the call, and the question is
    // settled by its key until the pane prints again.
    wipeSecret();
    secretSettled.add(ask.key);
  },
});

/* Enter sends, as a button press would. The IME's Enter commits a syllable and
 * sends nothing; a held Enter sends once. */
secretInput.addEventListener("keydown", (event) => {
  if (event.key !== "Enter" || event.isComposing || event.repeat) return;
  event.preventDefault();
  event.stopPropagation();
  if (activeAsk?.kind === "secret") void answerAsk({ decision: "send" });
});

let secretPolling = false;

/* Ask the backend which panes wait for a secret, and make the popup agree:
 * a question that left the pane leaves the popup, a new one is raised. A poll
 * that fails changes nothing on the screen; the next poll tries again. */
async function pollSecrets() {
  if (secretPolling) return;
  secretPolling = true;
  try {
    const waiting = await invoke("panes_secret");
    const keys = new Set(waiting.map(secretKeyOf));
    withdrawAsks((ask) => ask.kind === "secret" && !keys.has(ask.key));
    for (const item of waiting) {
      if (secretSettled.has(secretKeyOf(item))) continue;
      raiseAsk(secretAskOf(item));
    }
    for (const key of [...secretSettled]) if (!keys.has(key)) secretSettled.delete(key);
  } catch {
    // Nothing to show this time.
  } finally {
    secretPolling = false;
  }
}

/* An idle poller (shell-boot.js): it asks only while the window is visible and
 * a terminal is open, so an empty window pays no timer round trip. Terminal
 * views call `sync()` when one opens or closes (shell-term.js). */
const secretPoll = idlePoller({
  wanted: () => termViews.size > 0,
  every: SECRET_POLL_MS,
  tick: () => void pollSecrets(),
  onResume: () => void pollSecrets(),
});
secretPoll.sync();
