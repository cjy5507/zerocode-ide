/* ---- secrets written as plain text in zo's settings (t-76698) ----
 *
 * A key a person typed into zo's settings file before the window kept keys
 * by reference (t-75740, t-75848) is still there as plain text. Once the
 * settings are drawn, the window asks zo which cells those are
 * (`settings_secret_literals`) and, when there are any, says so in one line
 * above the open section — the names alone, never a value, a piece of one or
 * its length. 「키체인으로 옮기기」 is the person's consent: the backend
 * (`move_settings_secrets`) puts each value in the keychain item zo reads and
 * has zo rewrite the cell as its reference, and the line says what happened.
 * A zo that does not know the door answers nothing, and nothing is shown.
 *
 * What the page keeps of a value: nothing. No value reaches it — the backend
 * answers names, zo's backup path and zo's reason — and the page writes none
 * of what it is told to storage, the console or a log. The source contract
 * (`source_contracts/settings_secrets.rs`) and the window suite
 * (`ui/tests/settings-secrets.mjs`) search for it. */

/* The page's sentence for each place a move can stop, written out whole so
 * each key is findable by its name (shell-i18n.js holds the four other
 * languages), given the variable's name. */
const SETTINGS_SECRET_STOPS = Object.freeze({
  keychain_differs: (name) => t(
    "settings.secrets.stop.keychainDiffers",
    "키체인에 같은 이름({{name}})의 항목이 이미 다른 값으로 있어 덮어쓰지 않았습니다. 그 항목을 확인한 뒤 다시 옮겨 주세요.",
    { name },
  ),
  not_resolved: (name) => t(
    "settings.secrets.stop.notResolved",
    "zo가 ${{{name}}} 참조를 풀지 못해 설정 파일을 고치지 않았습니다.",
    { name },
  ),
  move_refused: (name) => t(
    "settings.secrets.stop.moveRefused",
    "zo가 설정 파일의 {{name}} 칸을 고치지 않았습니다.",
    { name },
  ),
  cell_changed: (name) => t(
    "settings.secrets.stop.cellChanged",
    "목록을 읽은 뒤 설정 파일의 {{name}} 칸이 바뀌어 옮기지 않았습니다. 설정을 다시 열어 확인해 주세요.",
    { name },
  ),
  padded: (name) => t(
    "settings.secrets.stop.padded",
    "{{name}} 값의 앞이나 뒤에 공백이 있어 옮기지 않았습니다. 설정 파일에서 공백을 지운 뒤 다시 옮겨 주세요.",
    { name },
  ),
  keychain: (name) => t(
    "settings.secrets.stop.keychain",
    "키체인에 {{name}} 항목을 넣지 못했습니다.",
    { name },
  ),
  zo_silent: (name) => t(
    "settings.secrets.stop.zoSilent",
    "zo가 답하지 않아 옮기지 못했습니다. 잠시 뒤 다시 옮겨 주세요.",
    { name },
  ),
});

/* What the line last showed: the names zo listed, and the last move's answer
 * — or the failure of the call itself, as the backend's sentence. */
let settingsSecretNames = [];
let settingsSecretOutcome = null;
let settingsSecretFailure = null;
let settingsSecretAsking = false;
let settingsSecretMoving = false;

function paintSettingsSecrets() {
  const line = el("settings-secrets");
  if (!line) return;
  const outcome = settingsSecretOutcome;
  const moved = outcome?.moved ?? [];
  const shown = settingsSecretNames.length > 0 || moved.length > 0 || !!outcome?.stopped || !!settingsSecretFailure;
  line.hidden = !shown;
  if (!shown) return;

  const listed = settingsSecretNames.length > 0;
  say(el("settings-secrets-title"), () => listed
    ? t("settings.secrets.title", "설정 파일에 비밀 {{count}}개가 그대로 적혀 있습니다", { count: settingsSecretNames.length })
    : t("settings.secrets.allMoved", "설정 파일의 비밀을 키체인으로 옮겼습니다"));
  const names = el("settings-secrets-names");
  names.hidden = !listed;
  names.textContent = settingsSecretNames.join(", ");
  el("settings-secrets-body").hidden = !listed;
  say(el("settings-secrets-body"), () => t(
    "settings.secrets.body",
    "옮기면 값은 키체인에 들어가고 설정 파일에는 이름 참조만 남습니다. 값은 이 화면에 보이지 않으며, zo가 고치기 전의 파일을 백업해 둡니다.",
  ));

  const button = el("settings-secrets-move");
  button.hidden = !listed;
  button.disabled = settingsSecretMoving;
  say(button, () => settingsSecretMoving
    ? t("settings.secrets.moving", "옮기는 중…")
    : t("settings.secrets.move", "키체인으로 옮기기"));

  const backups = [...new Set(moved.map((one) => one.backup))];
  const done = el("settings-secrets-done");
  done.hidden = backups.length === 0;
  say(done, () => t("settings.secrets.moved", "옮김 · 백업 {{backup}}", { backup: backups.join(", ") }));

  const stopped = outcome?.stopped ?? null;
  const why = el("settings-secrets-why");
  why.hidden = !stopped && !settingsSecretFailure;
  say(why, () => {
    if (settingsSecretFailure) return t("settings.secrets.failed", "옮기지 못했습니다: {{why}}", { why: settingsSecretFailure });
    if (!stopped) return "";
    const parts = [(SETTINGS_SECRET_STOPS[stopped.reason] ?? SETTINGS_SECRET_STOPS.zo_silent)(stopped.name)];
    // zo's own sentence, said as zo's; a keychain refusal is the keychain's.
    if (stopped.why) {
      parts.push(stopped.reason === "keychain"
        ? stopped.why
        : t("settings.secrets.zoSaid", "zo: {{why}}", { why: stopped.why }));
    }
    if (stopped.keychainKept) {
      parts.push(t(
        "settings.secrets.kept",
        "키체인 항목 {{name}}은(는) 그대로 두었습니다 — 다음에 옮길 때 그대로 씁니다.",
        { name: stopped.name },
      ));
    }
    if ((outcome.left ?? []).length > 0) {
      parts.push(t("settings.secrets.left", "아직 옮기지 않은 것: {{names}}", { names: outcome.left.join(", ") }));
    }
    return parts.join(" ");
  });
}

/* zo's listing, asked once the settings are drawn. A refusal or an answer of
 * nothing hides the line; the page never says a listing failed. */
async function askSettingsSecrets() {
  if (settingsSecretAsking || settingsSecretMoving) return;
  settingsSecretAsking = true;
  try {
    const notice = await invoke("settings_secret_literals");
    settingsSecretNames = Array.isArray(notice?.names) ? notice.names.filter((name) => typeof name === "string") : [];
  } catch {
    settingsSecretNames = [];
  } finally {
    settingsSecretAsking = false;
  }
  paintSettingsSecrets();
}

async function moveSettingsSecrets(hand) {
  if (settingsSecretMoving) return;
  settingsSecretMoving = true;
  settingsSecretOutcome = null;
  settingsSecretFailure = null;
  paintSettingsSecrets();
  try {
    settingsSecretOutcome = await invoke("move_settings_secrets", {}, { hand });
  } catch (error) {
    settingsSecretFailure = typeof error === "string" ? error : String(error?.message ?? error);
  } finally {
    settingsSecretMoving = false;
  }
  paintSettingsSecrets();
  await askSettingsSecrets();
}

el("settings-secrets-move")?.addEventListener("click", () => {
  void moveSettingsSecrets(handNow());
});

/* The settings opening is the settings view losing `hidden`. The listing waits
 * for the frame after it, so the settings draw before zo is asked. */
{
  const view = el("settings-view");
  let open = view ? !view.hidden : false;
  if (view) {
    new MutationObserver(() => {
      const now = !view.hidden;
      if (now && !open) {
        settingsSecretOutcome = null;
        settingsSecretFailure = null;
        requestAnimationFrame(() => setTimeout(() => void askSettingsSecrets(), 0));
      }
      open = now;
    }).observe(view, { attributes: true, attributeFilter: ["hidden"] });
  }
}
