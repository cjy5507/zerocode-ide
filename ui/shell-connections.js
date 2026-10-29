/* Settings › 모델 and 모델 연결, as the person approved them (t-75762 v2,
 * 2026-10-26): two choices — the coding model and the judgment model — and
 * one page of where each model is reached. Every name a list shows comes
 * from zo's own model list (`zo_model_list`, the catalog the model menu
 * already reads) or the saved connections (`model_connections`); every
 * state a row wears comes from a real answer — zo's reports, a check, the
 * managed server's snapshot — and says 「아직 확인 안 함」 until one came.
 * A saved connection is shared; choosing a role never copies its credential. */

/* The plain words a state is said in, and the badge each wears — one table
 * for every row and every check on both pages (v2 대응표 8). */
const MODEL_STATES = Object.freeze({
  ok: { key: "settings.models.state.ok", word: "연결됨", badge: "connected" },
  working: { key: "settings.models.state.working", word: "동작 중", badge: "connected" },
  running: { key: "settings.models.state.running", word: "실행 중", badge: "connected" },
  usable: { key: "settings.models.state.usable", word: "쓸 수 있음", badge: "connected" },
  nokey: { key: "settings.models.state.nokey", word: "키를 찾지 못했습니다", badge: "error", todo: "settings.models.todo.nokey", todoWord: "모델 연결에서 이 연결의 키를 다시 붙여 넣으세요." },
  unreadable: { key: "settings.models.state.nokey", word: "키를 찾지 못했습니다", badge: "error", todo: "settings.models.todo.unreadable", todoWord: "이 연결의 키는 창이 읽지 못하는 환경 변수({{env}})에 있습니다. 모델 연결의 「설정」에서 키를 붙여 넣으세요." },
  badkey: { key: "settings.models.state.badkey", word: "키가 맞지 않습니다", badge: "error", todo: "settings.models.todo.badkey", todoWord: "키를 다시 붙여 넣으세요. 키 앞뒤에 빈칸이 들어갔는지도 보세요." },
  down: { key: "settings.models.state.down", word: "서버가 응답하지 않습니다", badge: "error", todo: "settings.models.todo.down", todoWord: "서버를 켜거나 주소를 확인하세요. 켜지면 다시 고를 수 있습니다." },
  schema: { key: "settings.models.state.schema", word: "판단 답을 정해진 모양으로 내지 못합니다", badge: "error", todo: "settings.models.todo.schema", todoWord: "다른 모델을 고르세요. 작은 모델일수록 자주 그렇습니다." },
  failed: { key: "settings.models.state.failed", word: "확인하지 못했습니다", badge: "error", todo: "settings.models.todo.failed", todoWord: "잠시 뒤 다시 확인하세요. 자세한 내용은 아래에 있습니다." },
  nologin: { key: "settings.models.state.nologin", word: "로그인 안 됨", badge: "checking", todo: "settings.models.todo.nologin", todoWord: "터미널에서 그 CLI에 로그인하면 여기에 저절로 나타납니다." },
  stale: { key: "settings.models.state.stale", word: "목록은 이전 확인 것", badge: "checking" },
  elsewhere: { key: "settings.models.state.elsewhere", word: "다른 환경의 조회", badge: "checking", todo: "settings.models.todo.elsewhere", todoWord: "zo 창이 쓰는 로그인과 다른 곳에서 읽은 목록입니다." },
  unknown: { key: "settings.models.state.unknown", word: "아직 확인 안 함", badge: "unchecked" },
  checking: { key: "settings.models.state.checking", word: "확인하는 중…", badge: "checking" },
  later: { key: "settings.models.state.later", word: "실행 뒤 확인합니다", badge: "unchecked" },
  restarting: { key: "settings.models.state.restarting", word: "다시 띄우는 중", badge: "checking" },
  stopped: { key: "settings.models.state.stopped", word: "멈춤", badge: "error" },
  busy: { key: "settings.models.state.busy", word: "다른 작업 중", badge: "checking" },
  changed: { key: "settings.models.state.changed", word: "설정이 방금 바뀜", badge: "checking" },
  none: { key: "settings.models.state.none", word: "없음", badge: "unchecked" },
});

/* What a failure a command answered is, in the table's states: a door's
 * token (`no_key`, `unauthorized`, `transport`, `schema_…`, `http_503`…) or
 * the router probe's own reading of its request (api_routers.rs
 * `ProbeFailure`: a kind and the answer's status, t-77116). Never a word of
 * what a server said: a 503 whose body quotes `HTTP 401` as an example is a
 * server that is down, not a wrong key. */
function probeFailureState(failure) {
  const status = Number.isInteger(failure.status) ? failure.status : null;
  if (failure.kind === "http") {
    if (status === 401 || status === 403) return "badkey";
    return status !== null && status >= 500 ? "down" : "failed";
  }
  if (failure.kind === "transport" || failure.kind === "timeout") return "down";
  if (failure.kind === "schema") return "schema";
  // Sent nowhere: the backend's own token or sentence says why.
  return modelFailureState(failure.message);
}

function modelFailureState(error) {
  if (error && typeof error === "object" && typeof error.kind === "string") return probeFailureState(error);
  const token = String(typeof error === "string" ? error : error?.message ?? error ?? "");
  if (token === "no_key") return "nokey";
  if (token === "unauthorized") return "badkey";
  if (token === "schema" || token.startsWith("schema_")) return "schema";
  if (["transport", "timeout", "provider_request", "overloaded"].includes(token) || /^http_5\d\d$/.test(token)) return "down";
  return "failed";
}

/* What a failure said, without a value the page sent with it: a server
 * that says the pasted key back must not put it on the screen (t-76028 F1;
 * the probe masks it before IPC too, with the same mark). */
const MASKED_VALUE = "[redacted]";
function withoutValue(text, value) {
  return value ? text.split(value).join(MASKED_VALUE) : text;
}

function modelStateWords(state, vars) {
  const row = MODEL_STATES[state] ?? MODEL_STATES.failed;
  return { word: t(row.key, row.word), todo: row.todo ? t(row.todo, row.todoWord, vars) : "", badge: row.badge };
}

/* zo names a login's report by its provider's company and the login's model
 * rows by the CLI's word; one spelling joins them. */
const ZO_REPORT_PROVIDER = Object.freeze({ anthropic: "claude" });
/* Each login zo reads, in the order they are drawn, by the CLI word its
 * model rows carry; a provider zo adds later follows them under its word. */
const MODEL_LOGINS = Object.freeze({
  claude: () => t("settings.models.login.claude", "Claude 로그인"),
  openai: () => t("settings.models.login.openai", "ChatGPT 로그인"),
  google: () => t("settings.models.login.google", "Gemini 로그인"),
  xai: () => t("settings.models.login.xai", "Grok 로그인"),
  kimi: () => t("settings.models.login.kimi", "Kimi 로그인"),
});
const MODEL_LOGIN_ORDER = Object.freeze(Object.keys(MODEL_LOGINS));
/* Why a login's check failed, by the kind word the window keeps in place of
 * the provider's answer (t-76873): the answer itself never reaches here. */
const MODEL_FAILURE_KINDS = Object.freeze({
  credentialUnusable: () => t("settings.models.failure.credentialUnusable", "로그인은 있으나 쓸 수 없음 — 다시 로그인하세요"),
  auth: () => t("settings.models.failure.auth", "인증 거절"),
  forbidden: () => t("settings.models.failure.forbidden", "권한 없음"),
  notFound: () => t("settings.models.failure.notFound", "찾을 수 없음"),
  rateLimit: () => t("settings.models.failure.rateLimit", "한도 초과"),
  server: () => t("settings.models.failure.server", "서버 오류"),
  network: () => t("settings.models.failure.network", "네트워크 오류"),
  timeout: () => t("settings.models.failure.timeout", "시간 초과"),
  unknown: () => t("settings.models.failure.unknown", "알 수 없는 까닭"),
});

/* Where each preset's key is made (v2 대응표 5), by preset id. */
const MODEL_KEY_HELP = Object.freeze({
  openrouter: () => t("settings.models.keyHelp.openrouter", "openrouter.ai에 로그인해 Keys 페이지에서 「Create Key」로 만들고 복사합니다."),
  agentrouter: () => t("settings.models.keyHelp.agentrouter", "agentrouter.org에 로그인해 API 키 페이지에서 새 키를 만들고 복사합니다."),
  ollama: () => t("settings.models.keyHelp.ollama", "Ollama는 이 컴퓨터에서 돌아 키가 필요 없습니다. Ollama를 켜 두고 「연결 확인」을 누르세요."),
});

function modelNode(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text != null) node.textContent = text;
  return node;
}

function modelBadge(state, vars) {
  const words = modelStateWords(state, vars);
  const badge = modelNode("span", "settings-state model-state", words.word);
  badge.dataset.state = words.badge;
  badge.dataset.modelState = state;
  return badge;
}

/* "49m old" — how zo says a report's age — as the reader's own language says it. */
function modelReportAge(age) {
  const found = /^(\d+)([smhd]) old$/.exec(String(age ?? ""));
  if (!found) return "";
  const unit = { s: "second", m: "minute", h: "hour", d: "day" }[found[2]];
  try {
    return new Intl.RelativeTimeFormat(document.documentElement.lang || "ko", { numeric: "auto" }).format(-Number(found[1]), unit);
  } catch {
    return "";
  }
}

/* How long ago a kept check was made, in the reader's own language: seconds
 * under a minute, minutes under an hour, hours under two days, then days. */
function modelCheckAge(ms) {
  const seconds = Math.max(0, Math.round(ms / 1000));
  const [value, unit] = seconds < 60 ? [seconds, "second"] : seconds < 3600 ? [Math.floor(seconds / 60), "minute"]
    : seconds < 172800 ? [Math.floor(seconds / 3600), "hour"] : [Math.floor(seconds / 86400), "day"];
  try {
    return new Intl.RelativeTimeFormat(document.documentElement.lang || "ko", { numeric: "auto" }).format(-value, unit);
  } catch {
    return "";
  }
}

/* The states a finished check is kept in — `model_connection_checks::Verdict`. */
const KEPT_CHECK_STATES = Object.freeze(["ok", "usable", "nokey", "unreadable", "badkey", "down", "schema", "failed"]);

/* The quiet check waits for the window's first paint and then an idle
 * moment, so a window start never waits on a provider (t-76500 B). */
const QUIET_CHECK_AFTER_PAINT_MS = 10_000;

const modelConnectionsController = {
  state: null, zo: null, sequence: 0, busy: false, editing: null, view: "catalog", deletion: null,
  /* The last check each saved connection had, by id: made in this window,
   * or kept from an earlier start (`recalled`), with the generation it was
   * made against and when (t-76500). */
  checks: new Map(),
  /* The window's record of those checks as last read or written, with the
   * page's clock at that moment; `false` while it cannot be read — then
   * nothing is checked unasked, as its spacing cannot be kept. */
  memory: null, recalled: new Set(), quieting: false, quietQueued: false,
  /* The last judgment check each candidate had, by its reference. */
  judged: new Map(),
  /* A judgment format picked in 「고급」 for a connection, by its id. */
  formats: new Map(),
  /* The format each connection was last read in as judgment's, by its id:
   * kept after judgment moves to another connection (t-76028 R3). */
  usedFormats: new Map(),
  /* The daily cards: the role being changed, the row picked and its answer. */
  picking: null, draft: null, said: new Map(),
  /* The first question, while it stands. */
  setup: null, setupDismissed: false,

  show(view) {
    showSettingsPane("api-routers");
    this.paintView(view);
  },
  paintView(view) {
    this.view = view;
    el("connection-catalog-heading").hidden = view === "add";
    for (const node of document.querySelectorAll("[data-connection-view]")) node.hidden = node.dataset.connectionView !== view;
  },
  /* 「연결 추가」 asks the first question again (decision 7). */
  add() {
    this.startSetup("add");
    showSettingsPane("judgment-models");
  },
  /* A server the presets do not name, by its address — the editor. */
  addManual() {
    onRouterPresetChange("custom");
    el("router-preset-select").value = "custom";
    el("connection-auth").value = el("router-key-input").disabled ? "none" : "new";
    say(el("connection-form-title"), () => t("settings.connections.add", "연결 추가"));
    this.addFields();
    this.show("add");
  },
  addFields() {
    const requiresAuth = Boolean(this.editing?.provider.requires_auth);
    const saved = requiresAuth && this.editing.keyAvailable;
    el("connection-auth").querySelector('[value="saved"]').disabled = !saved;
    // Existing keys belong to their saved connection. This UI never copies one.
    el("connection-auth").querySelector('[value="none"]').disabled = requiresAuth;
    el("connection-key-field").hidden = el("connection-auth").value !== "new";
    if (el("connection-auth").value !== "new") el("router-key-input").value = "";
    el("connection-auth-note").textContent = el("connection-auth").value === "saved"
      ? t("settings.connections.referenceHint", "저장된 키는 이 연결의 참조로만 사용합니다. 화면에 읽어오지 않습니다.")
      : t("settings.connections.registrationHint", "연결을 등록해도 지금 쓰는 모델은 바뀌지 않습니다. 「모델」에서 고르세요.");
  },
  preset(preset) {
    el("connection-auth").value = (preset && this.location(preset.base_url) === "local") || el("router-key-input").disabled ? "none" : "new";
    this.addFields();
  },

  button(text, action, primary = false) {
    const button = modelNode("button", primary ? "btn btn--primary" : "btn", text);
    button.type = "button";
    button.disabled = this.busy;
    button.addEventListener("click", action);
    return button;
  },
  failure(error) {
    const token = typeof error === "string" ? error : error?.message;
    if (token === "connection_in_use") return t("settings.connections.inUse", "이 연결을 쓰는 용도를 먼저 해제하세요. 이름과 모델 목록은 그대로 수정할 수 있습니다.");
    if (token === "connection_name_conflict") return t("settings.connections.nameConflict", "다른 연결과 구분되는 이름을 입력하세요.");
    return judgmentProviderController.failure(token) ?? routerSaveRefusal(error);
  },
  location(address) {
    try { const host = new URL(address).hostname; return ["localhost", "127.0.0.1", "[::1]"].includes(host) ? "local" : "remote"; }
    catch { return "remote"; }
  },

  async refresh() {
    const sequence = ++this.sequence;
    const [state, zo] = await Promise.all([
      invoke("model_connections").then((value) => ({ value }), (error) => ({ error })),
      invoke("zo_model_list").then((value) => ({ value }), (error) => ({ error })),
      this.recall(),
    ]);
    if (sequence !== this.sequence) return;
    if (!routerPresets.length) {
      try {
        routerPresets = (await invoke("api_router_presets")) ?? [];
        populateRouterPresets();
      } catch {
        // The setup's service list says it has none; the editor still takes an address.
      }
      if (sequence !== this.sequence) return;
    }
    if (state.value && Array.isArray(state.value.connections)) {
      const signature = (value) => JSON.stringify([value?.activeProviderIdentity, value?.connections?.map((row) => [row.id, row.generation])]);
      if (signature(state.value) !== signature(this.state)) this.judged.clear();
      this.hold(state.value);
      this.failed = null;
    } else {
      this.failed = state.error ?? "provider_settings";
    }
    if (zo.value) this.zo = zo.value;
    this.takeRecalled();
    this.autoSetup();
    this.render();
    this.quietSoon();
  },

  /* ---- the kept checks (t-76500) ----------------------------------------- */

  async recall() {
    try {
      this.memory = this.taken(await invoke("remembered_model_connection_checks"));
    } catch {
      this.memory = false;
    }
  },
  /* A record answer as the page holds it, or `false` for anything else. */
  taken(read) {
    if (!Number.isFinite(read?.nowMs) || !Number.isFinite(read?.everyMs) || typeof read.connections !== "object") return false;
    return { ...read, readAt: performance.now() };
  },
  nowMs() {
    return this.memory ? this.memory.nowMs + (performance.now() - this.memory.readAt) : null;
  },
  /* A kept check stands for a connection this page has no answer for yet,
   * once per page and generation; a kept judgment check only for the
   * generation it was made against. */
  takeRecalled() {
    if (!this.memory) return;
    for (const connection of this.state?.connections ?? []) {
      const kept = this.memory.connections?.[connection.id];
      if (!kept || this.recalled.has(`${connection.id}:${kept.generation}`)) continue;
      this.recalled.add(`${connection.id}:${kept.generation}`);
      if (kept.state && !this.checks.has(connection.id)) {
        this.checks.set(connection.id, { state: kept.state, count: kept.count, checkedMs: kept.checkedMs, generation: kept.generation, recalled: true });
      }
      if (kept.generation !== connection.generation) continue;
      for (const judged of kept.judgments ?? []) {
        const reference = JSON.stringify({ id: connection.id, generation: kept.generation, model: judged.model, protocol: judged.protocol });
        if (!this.judged.has(reference)) this.judged.set(reference, { state: judged.state, checkedMs: judged.checkedMs, recalled: true });
      }
    }
  },
  /* A finished check, written to the record: state words only, never what a
   * server said. A record the window refused to write answers `{ unsaved }`
   * (t-76500 r4): the page shows the check, and says the next start will not
   * — the file still holds the one before it. */
  async keep(command, answer) {
    if (!KEPT_CHECK_STATES.includes(answer.state)) return null;
    try {
      const memory = this.taken(await invoke(command, { answer }));
      if (!memory) return null;
      this.memory = memory;
      return memory.connections?.[answer.id] ?? null;
    } catch (error) {
      return { unsaved: String(error?.message ?? error) };
    }
  },
  /* A saved connection's state: its key out of reach, its last check, or
   * 「설정이 바뀜」 when that check was made against another generation. */
  connectionCheck(connection) {
    if (connection.provider.requires_auth && !connection.keyAvailable) return { state: this.keyMissing(connection), check: null };
    const check = this.checks.get(connection.id);
    if (!check) return { state: "unknown", check: null };
    if (check.generation != null && check.generation !== connection.generation) return { state: "changed", check: null };
    return { state: check.state, check };
  },
  /* A check the record refused stays on the page marked as not kept, while
   * it is still the check `standing` answers. */
  unsaved(kept, standing) {
    const check = kept?.unsaved ? standing() : null;
    if (!check) return;
    check.unsaved = true;
    this.render();
  },
  /* Whether a connection is checked without a press: never checked, checked
   * against another generation, or longer ago than the record's spacing —
   * and never while it is being checked or its key is out of reach. */
  dueQuietly(connection) {
    if (!this.memory || (connection.provider.requires_auth && !connection.keyAvailable)) return false;
    const check = this.checks.get(connection.id);
    if (!check) return true;
    if (check.state === "checking") return false;
    if (check.generation != null && check.generation !== connection.generation) return true;
    return check.checkedMs != null && this.nowMs() - check.checkedMs >= this.memory.everyMs;
  },
  /* After what called it has finished its own turn — a press that checks a
   * connection itself marks it first. */
  quietSoon() {
    if (this.quietQueued || !this.memory) return;
    this.quietQueued = true;
    window.setTimeout(() => {
      this.quietQueued = false;
      void this.quietRecheck();
    }, 0);
  },
  /* The due connections, one at a time, with the light check only — the
   * model list, never a judgment request (ZO m-76513 ③). */
  async quietRecheck() {
    if (this.quieting) return;
    this.quieting = true;
    const asked = new Set();
    try {
      for (;;) {
        const connection = (this.state?.connections ?? []).find((one) => !asked.has(one.id) && this.dueQuietly(one));
        if (!connection) break;
        asked.add(connection.id);
        await this.checkConnection(connection, { quiet: true });
      }
    } finally {
      this.quieting = false;
    }
  },
  /* The window's start: after the first paint and an idle moment, the
   * record is read and the due connections are checked (t-76500 B). */
  async wake() {
    await this.recall();
    if (!this.memory) return;
    if (!this.state) {
      try {
        this.hold(await invoke("model_connections"));
      } catch {
        return;
      }
    }
    this.takeRecalled();
    this.render();
    await this.quietRecheck();
  },

  async sync() {
    try {
      return await invoke("sync_model_connections");
    } catch {
      return null;
    }
  },

  /* ---- what zo lists ---------------------------------------------------- */

  /* Every login zo knows of: the providers its model rows name and the ones
   * its reports name, each with its rows and whether a report answered. */
  logins() {
    const rows = this.zo?.models ?? [];
    const reports = this.zo?.reports ?? [];
    const providers = new Set(rows.map((row) => row.provider));
    for (const report of reports) providers.add(ZO_REPORT_PROVIDER[report.provider] ?? report.provider);
    const order = (provider) => {
      const at = MODEL_LOGIN_ORDER.indexOf(provider);
      return at < 0 ? MODEL_LOGIN_ORDER.length : at;
    };
    return [...providers].sort((a, b) => order(a) - order(b)).map((provider) => {
      const own = reports.filter((report) => (ZO_REPORT_PROVIDER[report.provider] ?? report.provider) === provider);
      const answered = own.filter((report) => report.ok);
      const models = rows.filter((row) => row.provider === provider);
      return {
        provider,
        name: MODEL_LOGINS[provider]?.() ?? t("settings.models.login.other", "{{provider}} 로그인", { provider }),
        state: this.loginState(own, answered, models),
        report: answered[0] ?? own[0] ?? null,
        models: [...models.filter((row) => row.band !== "superseded"), ...models.filter((row) => row.band === "superseded")],
      };
    });
  },
  /* What a login's reports say, as zo divides them (ZO m-76593, m-76597): a
   * report from another place than a zo pane reads its login is that place's
   * (`credentialStore`; an older zo sends none); an answer is the list now;
   * a source zo skipped for want of a credential is no login; any other
   * failure kept last time's list, which is said as such — never 「로그인 안
   * 됨」, which a failed read of a signed-in account is not (t-76598). */
  loginState(own, answered, models) {
    // The pane's label names where a zo pane reads its CLAUDE login, so only
    // a Claude report is held to it; a report without a label is not judged.
    const pane = this.zo?.paneCredentialStore;
    const store = (report) => report.credentialStore ?? report.credential_store ?? null;
    if (pane && own.some((report) => report.provider === "anthropic" && store(report) && store(report) !== pane)) return "elsewhere";
    if (answered.length) return "ok";
    if (!own.length) return models.length ? "unknown" : "nologin";
    if (own.some((report) => report.diagnosis?.skipped === true)) return "nologin";
    return models.length ? "stale" : "failed";
  },
  /* A login whose models zo would run: one it lists, unless it is not
   * signed in. */
  loginUsable(login) {
    return login.models.length > 0 && login.state !== "nologin";
  },
  /* What a login's line says beyond its badge: how many models and when zo
   * checked them, or why the list is not today's, in one line — the failure
   * by the window's diagnosis, its status and kind, never the provider's
   * answer (t-76873). */
  loginWords(login) {
    const age = modelReportAge(login.report?.age);
    const failure = login.report?.diagnosis;
    // zo's own fields say what failed (`failureKind`, `httpStatus`); an older
    // zo names none, and then the line only says it failed (t-76968).
    const kind = failure && typeof failure.kind === "string" ? (MODEL_FAILURE_KINDS[failure.kind] ?? MODEL_FAILURE_KINDS.unknown)() : "";
    const detail = failure && Number.isInteger(failure.status) ? [`HTTP ${failure.status}`, kind].filter(Boolean).join(" · ") : kind;
    if (login.state === "ok") {
      return [t("settings.connections.modelCount", "모델 {{count}}개", { count: login.models.length }), age ? t("settings.models.zoChecked", "zo가 확인한 때: {{age}}", { age }) : ""].filter(Boolean).join(" · ");
    }
    if (login.state === "stale") {
      return [age ? t("settings.models.staleWhen", "목록은 이전 확인 것({{age}})", { age }) : modelStateWords("stale").word,
        detail ? t("settings.models.staleFailed", "지금 조회 실패: {{detail}}", { detail }) : t("settings.models.staleFailedPlain", "지금 조회 실패")].join(" · ");
    }
    if (login.state === "failed") return detail ? t("settings.models.staleFailed", "지금 조회 실패: {{detail}}", { detail }) : t("settings.models.staleFailedPlain", "지금 조회 실패");
    if (login.state === "unknown") return t("settings.models.loginUnchecked", "zo가 아직 이 로그인을 확인하지 않았습니다.");
    return modelStateWords(login.state).todo;
  },
  /* The routers zo reads — the settings' connections and any its own
   * environment adds — with the saved connection behind each, if any. */
  routers() {
    return (this.zo?.customProviders ?? []).map((provider) => {
      const row = this.state?.connections.find((connection) => connection.provider.name === provider.name) ?? null;
      return {
        name: provider.name,
        row,
        local: row ? this.location(row.provider.base_url) === "local" : false,
        models: (provider.models ?? []).map((id) => ({ id, label: id.startsWith(`${provider.name}/`) ? id.slice(provider.name.length + 1) : id })),
      };
    });
  },
  /* Every model zo would run, one row each (덧붙임 B): every login model —
   * an older generation marked, at the foot of its login — and every router
   * model, as zo spells it. */
  codingRows() {
    const rows = [];
    for (const login of this.logins()) {
      for (const model of login.models) {
        rows.push({ key: model.id, id: model.id, name: model.displayName || model.id, group: login.name, login, old: model.band === "superseded", band: model.band });
      }
    }
    for (const router of this.routers()) {
      for (const model of router.models) {
        rows.push({ key: model.id, id: model.id, name: model.label, group: router.name, router });
      }
    }
    return rows;
  },
  /* A saved `model` as zo reads it: an alias resolved to the row it names. */
  codingKey(model) {
    if (!model) return null;
    const alias = (this.zo?.aliases ?? []).find((row) => row.alias === model);
    return alias?.canonical ?? model;
  },
  codingWay(row) {
    if (row.login) return row.login.name;
    if (row.router?.local) return t("settings.models.wayLocal", "{{name}} · 이 컴퓨터", { name: row.router.name });
    return t("settings.models.wayKey", "{{name}} · API 키", { name: row.router?.name ?? "" });
  },
  codingState(row) {
    if (!row) return "unknown";
    if (row.login) return row.login.state;
    const connection = row.router?.row;
    return connection ? this.connectionCheck(connection).state : "unknown";
  },
  keyMissing(connection) {
    return connection.provider.auth_env && !connection.provider.auth_env.startsWith("ZEROCODE_ROUTER_") ? "unreadable" : "nokey";
  },

  /* ---- what judgment can use --------------------------------------------- */

  judgmentRows() {
    const rows = [];
    const snapshot = managedJudgmentController.snapshot;
    const way = (words, protocol) => [words, judgmentWayWords(protocol)].filter(Boolean).join(" · ");
    for (const model of snapshot?.models?.filter((entry) => managedJudgmentController.held(entry)) ?? []) {
      rows.push({ key: `managed:${model.id}`, kind: "managed", model, protocol: model.protocol, name: model.label, way: way(t("settings.models.wayManaged", "이 컴퓨터 · 앱이 관리"), model.protocol) });
    }
    if (typesafeState?.jevProvider) {
      rows.push({ key: "jev", kind: "jev", protocol: typesafeState.jevProvider.protocol, name: "Jev", way: way(t("settings.models.wayJev", "Jev 서비스"), typesafeState.jevProvider.protocol) });
    }
    for (const connection of this.state?.connections ?? []) {
      const local = this.location(connection.provider.base_url) === "local";
      for (const model of connection.provider.models ?? []) {
        const id = typeof model === "string" ? model : model.id;
        const reference = { id: connection.id, generation: connection.generation, model: id, protocol: this.judgmentFormat(connection) };
        rows.push({
          key: JSON.stringify([connection.id, id]), kind: "connection", connection, reference, protocol: reference.protocol, name: id,
          way: way(local ? t("settings.models.wayLocal", "{{name}} · 이 컴퓨터", { name: connection.provider.name })
            : t("settings.models.wayKey", "{{name}} · API 키", { name: connection.provider.name }), reference.protocol),
        });
      }
    }
    return rows;
  },
  /* What the row can do for judgment before anything is sent: usable, asked
   * when picked, or not — with the reason. Dimming is only ever an answer
   * that came back, or a key the window cannot reach (ZO m-75925). */
  judgmentStanding(row) {
    if (row.kind === "managed") return { group: "usable", state: this.managedState(row.model) };
    if (row.kind === "jev") return { group: "usable", state: typesafeState?.provider?.kind === "jev" && typesafeState.keySaved ? "ok" : "unknown" };
    const connection = row.connection;
    if (connection.provider.requires_auth && !connection.keyAvailable) return { group: "off", state: this.keyMissing(connection) };
    const judged = this.judged.get(JSON.stringify(row.reference));
    if (judged && judged.state !== "usable") return { group: "off", state: judged.state };
    if (this.connectionCheck(connection).state === "down") return { group: "off", state: "down" };
    return { group: "check", state: judged?.state ?? "unknown" };
  },
  /* A connections answer, taken: judgment's format is kept by connection,
   * so moving judgment elsewhere does not forget it (t-76028 R3) — and the
   * settings keep it too, so a restart does not either (t-76211). */
  hold(state) {
    for (const row of state?.connections ?? []) {
      if (typeof row?.judgmentFormat === "string") this.usedFormats.set(row.id, row.judgmentFormat);
    }
    if (state?.judgment?.protocol) this.usedFormats.set(state.judgment.id, state.judgment.protocol);
    this.state = state;
  },
  /* The format a connection is asked for judgment in: the one picked in
   * 「고급」, else the one judgment used it in last, else OpenAI JSON
   * (t-76028 F3, R3). */
  judgmentFormat(connection) {
    return this.formats.get(connection.id) ?? this.usedFormats.get(connection.id) ?? "openai_json";
  },
  currentJudgmentKey() {
    const provider = typesafeState?.provider;
    if (!provider) return null;
    const judgment = this.state?.judgment;
    if (judgment) {
      // A format changed in 「고급」 makes the same model a new pick.
      const connection = this.state.connections.find((row) => row.id === judgment.id);
      return connection && this.judgmentFormat(connection) !== judgment.protocol ? null : JSON.stringify([judgment.id, judgment.model]);
    }
    if (provider.kind === "jev") return "jev";
    if (provider.kind === "local" && managedJudgmentController.snapshot?.models?.some((model) => model.id === provider.model)) return `managed:${provider.model}`;
    return null;
  },

  /* The managed server's state, in the five words (t-59744 ④⑥). The model
   * the settings select is read only through the server the settings name
   * (`managedServer`): another model's server, or a snapshot of another
   * settings identity, says nothing about it. */
  managedState(model) {
    const snapshot = managedJudgmentController.snapshot;
    if (!snapshot) return "unknown";
    if (this.said.get("managed")?.state === "changed") return "changed";
    if (managedJudgmentController.operating()) return "busy";
    const provider = typesafeState?.provider;
    if (provider?.kind === "local" && provider.model === model.id) {
      const server = judgmentProviderController.managedServer();
      if (!server) return "unknown";
      if (server.state === "starting") return server.exit ? "restarting" : "checking";
      return server.state === "ready" ? "running" : "stopped";
    }
    const server = snapshot.server;
    if (server.modelId !== model.id) return snapshot.configuredModelId === model.id ? "stopped" : "none";
    if (server.state === "starting" && server.exit) return "restarting";
    if (server.state === "starting") return "checking";
    if (server.state === "ready") return "running";
    return "stopped";
  },
  /* What a managed model can say the moment it is picked, with no request
   * (t-76028 F4): a server running it whose answers were checked passed; one
   * this computer can run is checked when the apply starts it; anything else
   * keeps its state. */
  managedPick(model) {
    const state = this.managedState(model);
    const server = managedJudgmentController.snapshot?.server;
    if (state === "running" && server?.modelId === model.id && server.checked) return "usable";
    if (["running", "none", "stopped"].includes(state) && managedJudgmentController.localSupported(model)) return "later";
    return state;
  },
  managedReason(model) {
    const server = managedJudgmentController.snapshot?.server;
    if (!server || server.modelId !== model.id || !server.exit) return "";
    return managedJudgmentController.endedState(server.exit, server.state === "starting") ?? "";
  },

  /* ---- recommendations (decision 6) -------------------------------------- */

  recommendCoding(choice) {
    if (choice === "key" || choice === "local") {
      const connection = this.state?.connections.find((row) => row.id === this.setup?.connectionId);
      if (!connection) return null;
      const model = connection.provider.models?.[0];
      const id = typeof model === "string" ? model : model?.id;
      return id ? { key: `${connection.provider.name}/${id}`, why: t("settings.models.recoKey", "이 연결에 들어 있는 첫 모델입니다.") } : null;
    }
    for (const login of this.logins().filter((entry) => this.loginUsable(entry))) {
      const top = login.models.find((model) => model.band === "top");
      if (top) return { key: top.id, why: t("settings.models.recoLogin", "로그인한 계정의 모델 중 zo 목록이 「최상위」로 표시한 모델입니다. 키도 요금 설정도 따로 필요 없습니다.") };
    }
    return null;
  },
  recommendJudgment() {
    const snapshot = managedJudgmentController.snapshot;
    const running = snapshot?.models?.find((model) => snapshot.server.state === "ready" && snapshot.server.modelId === model.id);
    if (running) return { key: `managed:${running.id}`, why: t("settings.models.recoManaged", "이 컴퓨터 안에서 돌아 키도 요금도 없고, 지금 실행 중인 모델입니다.") };
    if (this.setup?.choice === "jev" && this.setup.checked === "ok") return { key: "jev", why: t("settings.models.recoJevChecked", "방금 연결을 확인한 판단 전용 서비스입니다.") };
    if (typesafeState?.jevProvider) return { key: "jev", why: t("settings.models.recoJev", "따로 설치할 것이 없고 판단 전용으로 만든 서비스입니다.") };
    return null;
  },

  /* ---- the first question ------------------------------------------------ */

  autoSetup() {
    if (this.setup || this.setupDismissed || !this.state || !this.zo) return;
    const logins = this.logins().some((login) => this.loginUsable(login));
    if (!logins && this.state.connections.length === 0) this.startSetup("start");
  },
  startSetup(title) {
    this.setup = { title, choice: null, checking: false, checked: null, failure: null, presetId: routerPresets[0]?.id ?? "custom", count: 0, connectionId: null, picking: null, draft: {}, result: null };
    this.renderSetup();
  },
  closeSetup() {
    this.setup = null;
    this.setupDismissed = true;
    this.render();
  },
  choose(choice) {
    if (this.busy) return;
    this.setup = { ...this.setup, choice, checking: false, checked: null, failure: null, count: 0, picking: null, draft: {}, result: null };
    if (choice === "login") void this.checkLogins();
    else if (choice === "local") void this.checkLocal();
    else this.renderSetup();
  },
  async checkLogins() {
    const setup = this.setup;
    setup.checking = true;
    this.renderSetup();
    try {
      this.zo = await invoke("zo_model_list");
    } catch {
      // The rows below say what was last read.
    }
    if (setup !== this.setup) return;
    setup.checking = false;
    setup.checked = this.logins().some((login) => this.loginUsable(login)) ? "ok" : "fail";
    setup.failure = setup.checked === "ok" ? null : "nologin";
    this.render();
  },
  async checkLocal() {
    const setup = this.setup;
    setup.checking = true;
    this.renderSetup();
    await refreshTypeSafe();
    if (setup !== this.setup) return;
    setup.checking = false;
    setup.checked = this.recommendJudgment()?.key?.startsWith("managed:") ? "ok" : "fail";
    setup.failure = setup.checked === "ok" ? null : "none";
    this.render();
  },
  /* ② 연결 확인: the router lists its models with the key just pasted; when
   * it answers, the connection is saved and the key goes to the keychain. */
  async checkKey() {
    const setup = this.setup;
    const started = handNow();
    const preset = routerPresets.find((row) => row.id === setup.presetId);
    const input = el("model-setup-key");
    const key = input?.value.trim() ?? "";
    if (!preset || this.busy) return;
    const local = this.location(preset.base_url) === "local";
    if (!key && !local) return;
    this.busy = true;
    setup.checking = true;
    setup.checked = null;
    this.renderSetup();
    try {
      const models = (await invoke("test_router_connection", {
        sourceId: null, expectedGeneration: null, baseUrl: preset.base_url, auth: preset.auth,
        key: key || null, modelsPath: preset.models?.path || "/models", idField: preset.models?.id_field || "id",
        contextField: preset.models?.context_field || "context_length", clientFingerprint: preset.client_fingerprint || null,
        headers: preset.headers || null,
      })) ?? [];
      if (setup !== this.setup) return;
      await invoke("save_api_router", {
        sourceId: null, expectedGeneration: null, auth: preset.auth, presetId: preset.id, name: preset.name,
        baseUrl: preset.base_url, key: key || null,
        models: models.map((model) => ({ id: model.id, context_window: model.context_length ?? null })),
        clientFingerprint: preset.client_fingerprint ?? null, headers: preset.headers ?? null,
      }, { hand: flowHand(started) });
      // The field drawn now, not the one read at the press: a repaint replaced it.
      const field = el("model-setup-key");
      if (field) field.value = "";
      await this.refresh();
      if (setup !== this.setup) return;
      const connection = this.state?.connections.find((row) => row.provider.name === preset.name);
      setup.connectionId = connection?.id ?? null;
      if (connection) {
        this.checks.set(connection.id, { state: "ok", count: models.length, generation: connection.generation, checkedMs: this.nowMs() });
        const check = this.checks.get(connection.id);
        void this.keep("remember_model_connection_check", { id: connection.id, generation: connection.generation, state: "ok", count: models.length, forgetJudgments: true })
          .then((kept) => this.unsaved(kept, () => this.checks.get(connection.id) === check && check));
      }
      setup.count = models.length;
      setup.checked = "ok";
    } catch (error) {
      if (setup !== this.setup) return;
      setup.checked = "fail";
      setup.failure = modelFailureState(error);
      setup.raw = withoutValue(typeof error === "string" ? error : error?.message ?? String(error), key);
    } finally {
      this.busy = false;
      if (setup === this.setup) {
        setup.checking = false;
        this.render();
      }
    }
  },
  /* ④ Jev: the key is asked about once and kept on the page until 「추천대로
   * 쓰기」 applies Jev with it — the keychain write is the apply's. */
  async checkJev() {
    const setup = this.setup;
    const key = el("model-setup-jev-key")?.value.trim() ?? "";
    if (!key || this.busy || !typesafeState?.jevProvider) return;
    this.busy = true;
    setup.checking = true;
    this.renderSetup();
    try {
      const provider = judgmentProviderController.copy(typesafeState.jevProvider);
      if (!provider.model) provider.model = typesafeState.model?.alias;
      const check = await invoke("preflight_judgment_provider", { provider, expectedProviderIdentity: typesafeState.providerIdentity, key });
      if (setup !== this.setup) return;
      setup.checked = check.answered ? "ok" : "fail";
      setup.failure = check.answered ? null : modelFailureState(check.failure);
      setup.jevKey = check.answered ? key : null;
    } catch (error) {
      if (setup !== this.setup) return;
      setup.checked = "fail";
      setup.failure = modelFailureState(error);
    } finally {
      this.busy = false;
      if (setup === this.setup) {
        setup.checking = false;
        this.render();
      }
    }
  },
  /* 「추천대로 쓰기」: both picks in one press, each said on its own line;
   * a judgment that does not pass its check is not applied (대응표 17). */
  async applySetup() {
    const setup = this.setup;
    const started = handNow();
    if (this.busy) return;
    this.busy = true;
    this.renderSetup();
    const result = { coding: null, judgment: null };
    try {
      if (setup.draft.coding) result.coding = await this.applyCoding(setup.draft.coding, flowHand(started));
      if (setup.draft.judgment && setup.draft.judgment !== this.currentJudgmentKey()) {
        result.judgment = await this.applyJudgment(setup.draft.judgment, flowHand(started), setup.jevKey);
      } else if (setup.draft.judgment) {
        result.judgment = { applied: true, key: setup.draft.judgment, kept: true };
      }
      result.sync = await this.sync();
    } finally {
      this.busy = false;
    }
    if (setup !== this.setup) return;
    setup.result = result;
    await this.refresh();
  },

  /* ---- applying ---------------------------------------------------------- */

  /* A coding pick: a saved connection is checked first (its model list read
   * with its saved key); a failed check keeps the current model. */
  async applyCoding(key, hand) {
    const row = this.codingRows().find((entry) => entry.key === key);
    const connection = row?.router?.row;
    if (connection) {
      const check = await this.checkConnection(connection);
      if (check.state !== "ok") return { applied: false, key, state: check.state, raw: check.raw };
    }
    try {
      this.hold(await invoke("select_coding_model", { model: key, expectedModel: this.state?.codingModel ?? null }, { hand }));
      return { applied: true, key };
    } catch (error) {
      return { applied: false, key, state: "failed", raw: this.failure(error) };
    }
  },
  async applyJudgment(key, hand, jevKey = null) {
    const row = this.judgmentRows().find((entry) => entry.key === key);
    if (!row) return { applied: false, key, state: "failed" };
    // The pick as it stood when pressed, kept whole: what the settings must
    // hold after, whatever the rows are redrawn to meanwhile.
    const picked = this.judgmentPick(row);
    try {
      if (row.kind === "managed") {
        const applied = await managedJudgmentController.activate(row.model);
        if (!applied) return { applied: false, key, state: "failed", raw: managedJudgmentController.failure(managedJudgmentController.lastError) };
        picked.provider = managedJudgmentController.appliedProvider;
      } else if (row.kind === "jev") {
        const provider = picked.provider;
        const expectedProviderIdentity = typesafeState.providerIdentity;
        const check = await invoke("preflight_judgment_provider", { provider, expectedProviderIdentity, key: jevKey });
        if (!check.answered) return { applied: false, key, state: modelFailureState(check.failure) };
        await invoke("set_judgment_provider", { provider, expectedProviderIdentity, key: jevKey }, { hand });
      } else {
        this.hold(await invoke("select_model_connection", {
          role: "judgment", reference: picked.reference, expectedReference: this.state?.judgment ?? null,
          expectedProviderIdentity: this.state.activeProviderIdentity,
        }, { hand }));
      }
      // Read back what was written, whatever wrote it: zo's settings and
      // the connections.
      await refreshTypeSafe();
      try {
        this.hold(await invoke("model_connections"));
      } catch {
        // The difference below reads zo's settings alone then.
      }
      const differs = this.appliedDifference(picked);
      if (differs === null) await this.rememberApplied();
      return { applied: true, key, differs };
    } catch (error) {
      return { applied: false, key, state: modelFailureState(error), raw: this.failure(error) };
    }
  },
  /* Since when the judgment model counts: the window keeps the moment the
   * provider its settings name now was applied (t-76598). A record that
   * cannot be written leaves the dashboard without the line, nothing else. */
  async rememberApplied() {
    try {
      const memory = this.taken(await invoke("remember_judgment_applied"));
      if (!memory) return;
      this.memory = memory;
      jevApplied = memory.applied ?? jevApplied;
    } catch {
      // No line on the dashboard; the apply stands.
    }
  },
  /* A saved connection's model list, read with its saved key: pressed, or
   * quietly because its check is due. The answer is kept for the next start. */
  async checkConnection(connection, { quiet = false } = {}) {
    // Checking a connection again by hand lets its models be asked for
    // judgment again; a quiet check keeps what they answered.
    if (!quiet) {
      for (const reference of [...this.judged.keys()]) {
        if (JSON.parse(reference)?.id === connection.id) this.judged.delete(reference);
      }
    }
    this.checks.set(connection.id, { state: "checking", generation: connection.generation });
    this.render();
    let answer;
    try {
      const models = (await invoke("test_router_connection", {
        sourceId: connection.id, expectedGeneration: connection.generation, baseUrl: connection.provider.base_url,
        auth: connection.auth, key: null, modelsPath: "/models", idField: "id", contextField: "context_length",
        clientFingerprint: connection.provider.client_fingerprint ?? null, headers: connection.probeHeaders ?? connection.provider.headers ?? null,
      })) ?? [];
      answer = { state: "ok", count: models.length };
    } catch (error) {
      const raw = typeof error === "string" ? error : error?.message ?? String(error);
      answer = { state: connection.provider.requires_auth && !connection.keyAvailable ? this.keyMissing(connection) : modelFailureState(error), raw };
    }
    Object.assign(answer, { generation: connection.generation, checkedMs: this.nowMs() });
    this.checks.set(connection.id, answer);
    this.render();
    const kept = await this.keep("remember_model_connection_check", {
      id: connection.id, generation: connection.generation, state: answer.state, count: answer.count ?? null, forgetJudgments: !quiet,
    });
    if (kept?.checkedMs != null && this.checks.get(connection.id) === answer) answer.checkedMs = kept.checkedMs;
    this.unsaved(kept, () => this.checks.get(connection.id) === answer && answer);
    return answer;
  },
  /* A judgment pick is checked the moment it is picked (대응표 19): the same
   * door and request an apply makes (wiki: a candidate judgment provider is
   * asked through the same door and post). */
  async judge(row) {
    const reference = JSON.stringify(row.reference ?? row.key);
    this.judged.set(reference, { state: "checking" });
    this.renderDaily();
    let answer;
    try {
      if (row.kind === "connection") {
        const result = await invoke("check_model_connection", { reference: row.reference, expectedProviderIdentity: this.state.activeProviderIdentity });
        answer = result.preflight.answered ? { state: "usable" } : { state: modelFailureState(result.preflight.failure) };
      } else if (row.kind === "jev") {
        const provider = judgmentProviderController.copy(typesafeState.jevProvider);
        if (!provider.model) provider.model = typesafeState.model?.alias;
        const check = await invoke("preflight_judgment_provider", { provider, expectedProviderIdentity: typesafeState.providerIdentity, key: null });
        answer = check.answered ? { state: "usable" } : { state: modelFailureState(check.failure) };
      } else {
        answer = { state: this.managedPick(row.model) };
      }
    } catch (error) {
      answer = { state: modelFailureState(error) };
    }
    this.judged.set(reference, answer);
    this.renderDaily();
    if (row.kind === "connection") {
      const { id, generation, model, protocol } = row.reference;
      const kept = await this.keep("remember_model_judgment_check", { id, generation, model, protocol, state: answer.state });
      this.unsaved(kept, () => this.judged.get(reference) === answer && answer);
    }
  },

  /* The 이 컴퓨터 row's buttons are the managed server's own commands; a
   * refusal because the settings moved first is said as such (t-59744 ⑥). */
  async managedAction(action, model) {
    this.said.delete("managed");
    managedJudgmentController.lastError = null;
    if (action === "stop") await managedJudgmentController.perform("managed_judgment_stop", { serverId: managedJudgmentController.snapshot.server.id });
    else if (action === "start") await managedJudgmentController.activate(model);
    else if (action === "cancel") await managedJudgmentController.cancel();
    else await refreshTypeSafe();
    const failed = managedJudgmentController.lastError;
    if ((typeof failed === "string" ? failed : failed?.message ?? failed?.code) === "settings_changed") this.said.set("managed", { state: "changed" });
    this.render();
  },

  /* The old judgment setting's key, moved only when a person presses, after
   * one line says from where to where; a refused move changes nothing (ZO
   * m-75925, coordinator m-75952). The old item is left as it was. */
  async adoptLegacyKey(connection) {
    const started = handNow();
    this.adopting = null;
    try {
      this.hold(await invoke("adopt_legacy_judgment_key", { id: connection.id, expectedGeneration: connection.generation }, { hand: flowHand(started) }));
      this.judged.clear();
      this.said.set(connection.id, { words: t("settings.models.legacyMoved", "이전 판단 설정의 키를 이 연결로 옮겼습니다. 키 값은 화면에 읽어 오지 않았습니다.") });
    } catch (error) {
      this.said.set(connection.id, { words: t("settings.models.legacyRefused", "옮기지 않았습니다. 아무것도 바꾸지 않았습니다 — {{reason}}", { reason: this.failure(error) }) });
    }
    this.render();
  },
  legacyButton(connection) {
    const move = this.button(t("settings.models.legacyMove", "이전 판단 키를 이 연결로 옮기기"), () => {
      this.adopting = connection.id;
      this.render();
    });
    move.dataset.legacyMove = connection.id;
    return move;
  },
  legacyConfirm(connection) {
    const note = modelNode("div", "settings-notice model-verdict");
    note.setAttribute("role", "status");
    note.dataset.legacyConfirm = connection.id;
    note.append(modelNode("strong", "", t("settings.models.legacyWhere", "이전 판단 설정이 {{address}}에 쓰던 키를 이 연결({{name}})의 키로 옮깁니다.", { address: connection.provider.base_url, name: connection.provider.name })),
      modelNode("span", "model-result-sub", t("settings.models.legacyHow", "키는 이 Mac의 키체인 안에서만 옮겨지고 화면에 나오지 않습니다. 이전 항목은 그대로 둡니다.")));
    const row = modelNode("div", "settings-action-row");
    const move = this.button(t("settings.models.legacyConfirm", "옮기기"), () => void this.adoptLegacyKey(connection), true);
    move.dataset.legacyConfirmMove = connection.id;
    row.append(move, this.button(t("settings.connections.cancel", "취소"), () => { this.adopting = null; this.render(); }));
    note.append(row);
    return note;
  },

  /* ---- drawing ------------------------------------------------------------ */

  render() {
    this.renderSetup();
    this.renderDaily();
    this.renderConnections();
  },

  /* One model, on one line: its name, how it is reached, a reason when it
   * cannot be picked, and its state. */
  pickRow(role, row, { selected, disabled, why, state, current }, onPick = null) {
    const label = modelNode("label", `model-pick${disabled ? " is-off" : ""}`);
    const input = document.createElement("input");
    input.type = "radio";
    input.name = `model-pick-${role}`;
    input.value = row.key;
    input.checked = selected;
    input.disabled = disabled || this.busy;
    input.dataset.modelPick = row.key;
    input.addEventListener("change", () => onPick?.(role, row));
    const copy = modelNode("span", "model-pick-copy");
    copy.append(modelNode("span", "model-pick-name", current ? t("settings.models.current", "{{name}} (지금 사용 중)", { name: row.name }) : row.name),
      ...(row.way ? [modelNode("span", "model-pick-way", row.way)] : []));
    if (why) copy.append(modelNode("span", "model-pick-why", why));
    label.append(input, copy);
    if (state) label.append(state);
    return label;
  },
  chooser(role, selected, onPick) {
    const field = modelNode("fieldset", "model-chooser");
    field.append(modelNode("legend", "settings-label", role === "coding" ? t("settings.models.pickCoding", "코딩 모델 고르기") : t("settings.models.pickJudgment", "판단 모델 고르기")));
    const group = (label) => field.appendChild(modelNode("p", "model-pick-group", label));
    if (role === "coding") {
      const current = this.codingKey(this.state?.codingModel);
      const rows = this.codingRows();
      let last = null;
      for (const row of rows) {
        if (row.group !== last) {
          last = row.group;
          group(row.login ? row.group : row.router.local ? t("settings.models.wayLocal", "{{name}} · 이 컴퓨터", { name: row.group }) : t("settings.models.wayKey", "{{name}} · API 키", { name: row.group }));
          if (row.login && row.login.state !== "ok") field.append(modelNode("p", "model-pick-note", `${modelStateWords(row.login.state).word} — ${this.loginWords(row.login)}`));
        }
        const state = row.old ? modelNode("span", "settings-badge model-old", t("settings.models.older", "이전 세대")) : row.login ? null : modelBadge(this.codingState(row));
        field.append(this.pickRow(role, row, { selected: selected === row.key, current: current === row.key, state }, onPick));
      }
      if (!rows.length) field.append(modelNode("p", "settings-hint", t("settings.models.noZoList", "zo 모델 목록을 읽지 못했습니다. zo가 설치돼 있는지 확인하세요.")));
      return field;
    }
    const current = this.currentJudgmentKey();
    const rows = this.judgmentRows().map((row) => ({ row, standing: this.judgmentStanding(row) }));
    // Jev's own guidance (docs.typesafe.ai, System One): a System One model
    // answers with calibrated confidence, which the act lines are set
    // against; a general model asked for JSON does not (t-76598).
    const advice = modelNode("p", "settings-hint model-judgment-advice", t("settings.models.judgmentAdvice", "권장: System One 방식(이 컴퓨터 모델 · Jev 서비스). 판단마다 보정된 확률을 주어 자동 적용 기준이 제대로 섭니다."));
    advice.dataset.judgmentAdvice = "";
    field.append(advice);
    for (const [name, label] of [["usable", t("settings.models.groupUsable", "판단에 쓸 수 있음")], ["check", t("settings.models.groupCheck", "고르면 바로 확인합니다")], ["off", t("settings.models.groupOff", "지금은 쓸 수 없음")]]) {
      const members = rows.filter((entry) => entry.standing.group === name);
      if (!members.length) continue;
      group(label);
      for (const { row, standing } of members) {
        const off = name === "off";
        const words = modelStateWords(standing.state, { env: row.connection?.provider.auth_env ?? "" });
        const line = this.pickRow(role, row, {
          selected: selected === row.key, current: current === row.key, disabled: off,
          why: off ? t("settings.models.cannot", "쓸 수 없음 — {{reason}}", { reason: [words.word, words.todo].filter(Boolean).join(" — ") }) : "",
          state: modelBadge(standing.state, { env: row.connection?.provider.auth_env ?? "" }),
        }, onPick);
        if (off && row.connection?.legacyKey) {
          const move = this.legacyButton(row.connection);
          move.classList.add("model-pick-action");
          line.append(move);
        }
        field.append(line);
        if (off && row.connection?.legacyKey && this.adopting === row.connection.id) field.append(this.legacyConfirm(row.connection));
      }
    }
    // Every login model, by name, dimmed with its reason (덧붙임 C).
    const logins = this.logins().flatMap((login) => login.models.map((model) => ({ model, login })));
    if (logins.length) {
      group(t("settings.models.groupNever", "판단에는 쓰지 않음"));
      const fold = modelNode("details", "model-pick-fold");
      fold.dataset.modelLogins = "";
      fold.open = this.loginsOpen === true;
      fold.addEventListener("toggle", () => { this.loginsOpen = fold.open; });
      fold.append(modelNode("summary", "", t("settings.models.loginModels", "로그인 모델 {{count}}개 — 판단에는 쓰지 않음", { count: logins.length })));
      fold.append(modelNode("p", "model-pick-note", t("settings.models.loginWhy", "구독 로그인은 제품의 자동 요청에 쓰지 않습니다. 코딩 모델로는 고를 수 있습니다.")));
      for (const { model, login } of logins) {
        fold.append(this.pickRow(role, { key: `login:${model.id}`, name: model.displayName || model.id, way: login.name }, { selected: false, disabled: true }));
      }
      field.append(fold);
    }
    return field;
  },
  /* The settings fields a pick changes, for whoever wants to see them. */
  changeTable(role, key) {
    const fold = modelNode("details", "settings-fold model-changes");
    fold.append(modelNode("summary", "", t("settings.models.changes", "고급 — 설정 파일에서 바뀌는 칸")));
    const table = modelNode("table", "model-change-table");
    const head = modelNode("tr");
    for (const word of [t("settings.models.changeField", "설정 칸"), t("settings.models.changeNow", "지금"), t("settings.models.changeAfter", "적용 뒤")]) {
      const cell = modelNode("th", "", word);
      cell.scope = "col";
      head.append(cell);
    }
    const body = modelNode("tbody");
    const line = (field, now, after) => {
      const row = modelNode("tr");
      row.append(modelNode("td", "model-change-key", field), modelNode("td", "", now ?? "—"), modelNode("td", "", after ?? "—"));
      body.append(row);
    };
    if (role === "coding") {
      const row = this.codingRows().find((entry) => entry.key === key);
      line("model", this.state?.codingModel, key);
      line("smart.codingConnection", this.state?.coding ? `${this.state.coding.id} · ${this.state.coding.model}` : null, row?.router?.row ? `${row.router.row.id} · ${row.name}` : null);
    } else {
      const row = this.judgmentRows().find((entry) => entry.key === key);
      const provider = typesafeState?.provider;
      line("smart.judgmentProvider", provider ? `${provider.kind} · ${provider.model}` : null,
        row?.kind === "connection" ? `${row.reference.protocol} · ${row.connection.provider.name} · ${row.name}` : row?.kind === "jev" ? "jev" : row ? `local · ${row.model.id}` : null);
    }
    const thead = modelNode("thead");
    thead.append(head);
    table.append(thead, body);
    fold.append(table);
    return fold;
  },

  renderDaily() {
    const coding = document.querySelector('[data-model-role="coding"]');
    const judgment = document.querySelector('[data-model-role="judgment"]');
    if (!coding || !judgment) return;
    const setupShown = Boolean(this.setup);
    el("typesafe-card").hidden = setupShown;
    if (this.failed && !this.state) {
      coding.replaceChildren(modelNode("p", "settings-hint", this.failure(this.failed)), this.button(t("settings.managed.refresh", "상태 새로고침"), () => void this.refresh()));
      judgment.replaceChildren();
      return;
    }
    coding.replaceChildren(...this.roleBody("coding"));
    judgment.replaceChildren(...this.roleBody("judgment"));
    say(el("model-advanced-route"), () => this.routeWords());
    this.renderFormats();
  },
  /* 「고급 — 판단 응답 형식」: each saved connection's format, the only
   * place it changes. A change forgets the connection's judgment checks. */
  renderFormats() {
    const list = el("model-advanced-formats");
    if (!list) return;
    const fields = (this.state?.connections ?? []).map((connection) => {
      const field = modelNode("label", "settings-field model-format");
      field.dataset.field = "md";
      field.append(modelNode("span", "settings-label", connection.provider.name));
      const select = modelNode("select", "settings-input");
      select.id = `judgment-format-${connection.id}`;
      select.dataset.judgmentFormat = connection.id;
      for (const [value, words] of [["openai_json", "OpenAI JSON"], ["system_one", "System One"]]) {
        const option = modelNode("option", "", words);
        option.value = value;
        select.append(option);
      }
      select.value = this.judgmentFormat(connection);
      select.disabled = this.busy;
      select.addEventListener("change", () => {
        this.formats.set(connection.id, select.value);
        for (const reference of [...this.judged.keys()]) {
          if (JSON.parse(reference)?.id === connection.id) this.judged.delete(reference);
        }
        this.draft = null;
        this.renderDaily();
      });
      field.append(select);
      return field;
    });
    el("model-advanced-format-fold").hidden = !fields.length;
    list.replaceChildren(...fields);
  },
  /* Where each choice is sent, in a beginner's words (t-76598): the
   * connection by the name the person gave it, the way it is asked, and
   * where its address is changed — no provider, endpoint or raw format. */
  routeWords() {
    const coding = this.state?.coding;
    const connection = coding && this.state.connections.find((row) => row.id === coding.id);
    const words = [t("settings.models.routeCoding", "코딩: {{model}}", { model: this.state?.codingModel ?? "—" })];
    if (connection) words.push(t("settings.models.routeConnection", "「{{name}}」 연결로 보냅니다. 주소와 키는 모델 연결의 「설정」에서 바꿉니다.", { name: connection.provider.name }));
    const judged = judgmentProviderWords();
    if (judged) words.push(t("settings.models.routeJudgment", "판단: {{model}}", { model: judged }));
    return words.join(" · ");
  },
  roleBody(role) {
    const nodes = [];
    const picking = this.picking === role;
    if (role === "judgment" && typesafeState?.jev && !typesafeState.jev.on) {
      nodes.push(modelNode("p", "settings-hint model-off", t("settings.models.judgmentOff", "판단을 껐습니다. 고른 모델은 남고, 판단 요청만 보내지 않습니다.")));
    }
    const line = modelNode("div", "model-line");
    line.dataset.modelCurrent = role;
    const copy = modelNode("div", "model-line-copy");
    let state = "unknown";
    if (role === "coding") {
      const key = this.codingKey(this.state?.codingModel);
      const row = this.codingRows().find((entry) => entry.key === key);
      copy.append(modelNode("strong", "model-line-name", row?.name ?? this.state?.codingModel ?? t("settings.models.notChosen", "아직 없음")));
      if (row) copy.append(modelNode("span", "model-line-way", this.codingWay(row)));
      else if (this.state?.codingModel) copy.append(modelNode("span", "model-line-way", t("settings.models.notListed", "zo 목록에 없는 이름입니다.")));
      copy.append(modelNode("span", "model-line-note", t("settings.models.codingNote", "새 zo 세션의 기본 모델입니다. 실행 중인 세션은 그대로입니다.")));
      state = row ? this.codingState(row) : this.state?.codingModel ? "unknown" : "none";
    } else {
      const key = this.currentJudgmentKey();
      const row = this.judgmentRows().find((entry) => entry.key === key);
      const provider = typesafeState?.provider;
      const name = row?.kind === "jev" && provider?.model ? `${row.name} · ${provider.model}` : row?.name ?? provider?.model;
      copy.append(modelNode("strong", "model-line-name", name ?? t("settings.models.notChosen", "아직 없음")));
      // A model a person named by its file is said as such: no
      // check vouches for it the way a catalog model is vouched for.
      const unverified = provider?.kind === "local" && provider.protocol === "openai_json";
      copy.append(modelNode("span", "model-line-way", unverified || !row ? (provider ? judgmentProviderController.providerName(provider) : "") : row.way));
      if (row?.kind === "managed") {
        const reason = this.managedReason(row.model);
        if (reason) copy.append(modelNode("span", "model-line-note", reason));
      }
      if (provider?.protocol === "openai_json") {
        const limit = modelNode("span", "model-line-note", t("settings.models.generalLimit", "일반 LLM은 보정된 확률이 없어 자동 적용으로 늦게 오르거나 오르지 않을 수 있습니다."));
        limit.dataset.judgmentLimit = "current";
        copy.append(limit);
      }
      state = row ? this.judgmentStanding(row).state : provider ? "unknown" : "none";
    }
    const actions = modelNode("div", "model-line-actions");
    const toggle = this.button(picking ? t("settings.models.close", "닫기") : t("settings.models.change", "바꾸기"), () => {
      this.picking = picking ? null : role;
      this.draft = null;
      this.said.delete(role);
      this.renderDaily();
    });
    toggle.setAttribute("aria-expanded", String(picking));
    toggle.dataset.modelChange = role;
    actions.append(modelBadge(state), toggle);
    line.append(copy, actions);
    nodes.push(line);
    const said = this.said.get(role);
    if (said) {
      const note = modelNode("div", `settings-notice model-result${said.ok ? " is-ok" : ""}`);
      note.setAttribute("role", said.ok ? "status" : "alert");
      if (said.differs) note.dataset.appliedDiffers = "";
      note.append(modelNode("strong", "", said.words));
      if (said.sub) note.append(modelNode("span", "model-result-sub", said.sub));
      nodes.push(note);
    }
    if (!picking) return nodes;
    nodes.push(this.chooser(role, this.draft, (picked, row) => {
      this.draft = row.key;
      this.said.delete(role);
      if (picked === "judgment") void this.judge(row);
      else this.renderDaily();
    }));
    if (this.draft) {
      if (role === "judgment") nodes.push(this.judgmentVerdict(this.draft), ...this.judgmentCaveats(this.draft));
      if (role === "coding") {
        const row = this.codingRows().find((entry) => entry.key === this.draft);
        if (row?.router?.row && this.codingState(row) === "unknown") nodes.push(modelNode("p", "settings-hint", t("settings.models.checkFirst", "적용을 누르면 먼저 이 연결을 확인합니다(모델 목록 불러오기). 실패하면 지금 모델을 그대로 씁니다.")));
      }
      nodes.push(this.changeTable(role, this.draft));
    }
    const apply = this.button(t("settings.models.apply", "적용"), () => void this.applyDaily(role), true);
    apply.dataset.modelApply = role;
    apply.disabled ||= !this.draft || this.draft === (role === "coding" ? this.codingKey(this.state?.codingModel) : this.currentJudgmentKey()) ||
      (role === "judgment" && !this.judgmentReady(this.draft));
    const row = modelNode("div", "settings-action-row");
    row.append(apply);
    nodes.push(row);
    return nodes;
  },
  /* What a judgment pick costs, said before it is applied (t-76598): a
   * general model's answers carry no calibrated confidence, and any change
   * of judgment model starts each feature's judging over. */
  judgmentCaveats(key) {
    const row = this.judgmentRows().find((entry) => entry.key === key);
    if (!row || key === this.currentJudgmentKey()) return [];
    const lines = [];
    if (this.judgmentGeneral(row)) {
      const limit = modelNode("p", "settings-hint model-judgment-limit", t("settings.models.generalLimit", "일반 LLM은 보정된 확률이 없어 자동 적용으로 늦게 오르거나 오르지 않을 수 있습니다."));
      limit.dataset.judgmentLimit = "";
      lines.push(limit);
    }
    const restart = modelNode("p", "settings-hint", t("settings.models.judgmentRestart", "판단 모델을 바꾸면 기능마다 판정이 처음부터 다시 쌓입니다."));
    restart.dataset.judgmentRestart = "";
    lines.push(restart);
    return lines;
  },
  /* A row that would judge as a general model: asked for JSON rather than
   * through System One. */
  judgmentGeneral(row) {
    return row?.protocol === "openai_json";
  },
  /* A judgment row's pick, copied whole when Apply is pressed: the kind,
   * the managed model — and, once activated, the provider it applied — the
   * Jev provider it writes, or the connection's full reference — its
   * generation too. */
  judgmentPick(row) {
    if (row.kind === "managed") return { kind: "managed", model: row.model.id, provider: null };
    if (row.kind === "jev") {
      const provider = judgmentProviderController.copy(typesafeState.jevProvider);
      if (!provider.model) provider.model = typesafeState.model?.alias;
      return { kind: "jev", provider };
    }
    return { kind: "connection", reference: { ...row.reference } };
  },
  /* What the settings say after an apply, against the pick (t-76598 ③):
   * read back from zo's settings and the connections, never assumed. `null`
   * when they hold the pick — a connection's at the generation picked, so an
   * address or key changed meanwhile under the same id is a difference; else
   * the words of what they hold. A read that failed holds nothing. */
  appliedDifference(picked) {
    const provider = typesafeRefreshFailed ? null : typesafeState?.provider;
    const unread = t("settings.models.appliedUnread", "설정을 다시 읽지 못했습니다");
    if (!provider) return unread;
    const same = (a, b) => Boolean(a) && Boolean(b) && ["id", "generation", "model", "protocol"].every((field) => a[field] === b[field]);
    const held = (() => {
      if (picked.kind === "jev") return provider.kind === "jev" && (!picked.provider.model || provider.model === picked.provider.model);
      // The whole provider the activation applied — another local server or
      // another protocol under the same model id is another provider — and
      // no connection, which a managed server is never bound to.
      if (picked.kind === "managed") return judgmentProviderController.sameProvider(provider, picked.provider) && !typesafeState.providerConnection;
      const judgment = this.state?.judgment;
      return provider.model === picked.reference.model && provider.protocol === picked.reference.protocol &&
        same(typesafeState.providerConnection, picked.reference) && (!judgment || same(judgment, picked.reference));
    })();
    if (held) return null;
    const words = judgmentProviderWords() || unread;
    const connection = typesafeState.providerConnection;
    // The same connection and model under another generation reads the same
    // in words: say what moved.
    if (picked.kind === "connection" && connection?.id === picked.reference.id && connection.model === picked.reference.model &&
      connection.generation !== picked.reference.generation) {
      return t("settings.models.appliedGeneration", "{{words}} (그 사이 연결의 주소나 키가 바뀜)", { words });
    }
    // The same model id read back from another server reads the same too.
    if (picked.kind === "managed" && provider.model === picked.provider?.model) {
      return t("settings.models.appliedServer", "{{words}} (같은 모델 이름이지만 다른 판단 서버)", { words });
    }
    return words;
  },
  judgmentReady(key) {
    const row = this.judgmentRows().find((entry) => entry.key === key);
    if (!row) return false;
    return ["usable", "later"].includes(this.judged.get(JSON.stringify(row.reference ?? row.key))?.state);
  },
  unsavedNote() {
    const line = modelNode("span", "model-result-sub", t("settings.models.unsaved", "이 확인 결과를 저장하지 못했습니다 — 창을 다시 켜면 이전 결과가 보입니다."));
    line.dataset.checkUnsaved = "";
    return line;
  },
  judgmentVerdict(key) {
    const row = this.judgmentRows().find((entry) => entry.key === key);
    const judged = row ? this.judged.get(JSON.stringify(row.reference ?? row.key)) : null;
    const note = modelNode("div", "settings-notice model-verdict");
    note.dataset.verdict = judged?.state ?? "unknown";
    if (!judged || judged.state === "checking") {
      note.setAttribute("role", "status");
      note.append(modelNode("strong", "", t("settings.models.checking", "연결을 확인하는 중…")), modelNode("span", "model-result-sub", t("settings.models.checkingSub", "판단 확인 질문 세 개를 실제로 보내 봅니다.")));
      return note;
    }
    if (judged.state === "usable") {
      note.classList.add("is-ok");
      note.setAttribute("role", "status");
      note.append(modelNode("strong", "", modelStateWords("usable").word), modelNode("span", "model-result-sub", t("settings.models.usableSub", "판단 확인을 통과했습니다. 적용을 누르면 다음 판단부터 이 모델을 씁니다.")));
      if (judged.unsaved) note.append(this.unsavedNote());
      return note;
    }
    if (judged.state === "later") {
      note.setAttribute("role", "status");
      note.append(modelNode("strong", "", modelStateWords("later").word), modelNode("span", "model-result-sub", t("settings.models.laterSub", "적용을 누르면 이 컴퓨터에서 모델을 준비해 실행한 뒤 판단 확인을 합니다. 통과하지 못하면 지금 판단 모델을 그대로 씁니다.")));
      return note;
    }
    const words = modelStateWords(judged.state, { env: row?.connection?.provider.auth_env ?? "" });
    if (row?.kind === "jev" && judged.state === "nokey") words.todo = t("settings.models.todo.jevKey", "「고급 — Jev 키」에서 Jev 키를 저장하세요.");
    note.setAttribute("role", "alert");
    note.append(modelNode("strong", "", t("settings.models.cannot", "쓸 수 없음 — {{reason}}", { reason: words.word })),
      modelNode("span", "model-result-sub", `${words.todo} ${t("settings.models.kept", "지금 판단 모델은 그대로 씁니다.")}`.trim()));
    if (judged.unsaved) note.append(this.unsavedNote());
    return note;
  },
  async applyDaily(role) {
    const started = handNow();
    const key = this.draft;
    if (!key || this.busy) return;
    this.busy = true;
    this.renderDaily();
    let result;
    try {
      result = role === "coding" ? await this.applyCoding(key, flowHand(started)) : await this.applyJudgment(key, flowHand(started));
      if (result.applied) {
        const sync = await this.sync();
        this.picking = null;
        this.draft = null;
        const name = (role === "coding" ? this.codingRows() : this.judgmentRows()).find((entry) => entry.key === key)?.name ?? key;
        if (result.differs) this.said.set(role, {
          ok: false, differs: true,
          words: t("settings.models.appliedDiffers", "적용을 눌렀지만 다시 읽은 판단 모델은 {{read}}입니다.", { read: result.differs }),
          sub: t("settings.models.appliedDiffersSub", "고른 모델: {{model}}. 다른 창이나 zo가 설정을 바꿨을 수 있습니다. 새로 읽은 뒤 다시 적용하세요.", { model: name }),
        });
        else this.said.set(role, {
          ok: true,
          words: role === "coding"
            ? t("settings.models.appliedCoding", "적용했습니다. 새 zo 세션부터 {{model}}을(를) 씁니다.", { model: name })
            : t("settings.models.appliedJudgment", "적용했습니다. 다음 판단부터 {{model}}을(를) 씁니다.", { model: name }),
          sub: role === "coding" ? t("settings.models.runningKept", "이미 열린 zo 세션은 지금 모델을 그대로 씁니다.")
            : [t("settings.models.appliedRead", "다시 읽은 설정도 {{model}}입니다.", { model: judgmentProviderWords() }), this.syncWords(sync)].filter(Boolean).join(" "),
        });
      } else {
        const words = modelStateWords(result.state ?? "failed", { env: "" });
        this.said.set(role, { words: t("settings.models.notApplied", "적용하지 않았습니다 — {{reason}}", { reason: words.word }), sub: [words.todo, result.raw].filter(Boolean).join(" ") });
      }
    } finally {
      this.busy = false;
    }
    await this.refresh();
  },
  syncWords(sync) {
    if (!sync) return "";
    return t("settings.models.syncWords", "실행 중인 zo {{queued}}개는 다음 요청부터 바뀝니다. 다시 시작해야 하는 zo {{restart}}개 · 확인하지 못한 zo {{unknown}}개.", { queued: sync.queued, restart: sync.restartRequired, unknown: sync.unknown });
  },

  renderSetup() {
    const holder = el("model-setup");
    if (!holder) return;
    holder.hidden = !this.setup;
    if (el("typesafe-card")) el("typesafe-card").hidden = Boolean(this.setup);
    if (!this.setup) {
      holder.replaceChildren();
      return;
    }
    const setup = this.setup;
    const step = setup.result ? 3 : setup.checked === "ok" ? 2 : 1;
    const head = modelNode("div", "model-setup-head");
    head.append(modelNode("h3", "model-role-title", setup.title === "add" ? t("settings.connections.add", "연결 추가") : t("settings.models.setupTitle", "모델 설정 시작하기")));
    const steps = modelNode("ol", "model-steps");
    steps.setAttribute("aria-label", t("settings.models.stepsLabel", "단계"));
    for (const [at, word] of [[1, t("settings.models.step1", "연결 고르기")], [2, t("settings.models.step2", "연결 확인")], [3, t("settings.models.step3", "추천대로 쓰기")]]) {
      const item = modelNode("li", at === step ? "is-now" : "", `${at}. ${word}`);
      if (at === step) item.setAttribute("aria-current", "step");
      steps.append(item);
    }
    head.append(steps);
    const nodes = [head];
    if (setup.result) {
      nodes.push(this.setupDone());
      holder.replaceChildren(...nodes);
      return;
    }
    nodes.push(modelNode("h4", "model-question", t("settings.models.question", "어떻게 연결할까요?")));
    const choices = modelNode("div", "model-choices");
    choices.setAttribute("role", "group");
    choices.setAttribute("aria-label", t("settings.models.question", "어떻게 연결할까요?"));
    for (const [id, mark, title, about] of [
      ["login", "①", t("settings.models.choiceLogin", "이미 로그인한 계정 쓰기"), t("settings.models.choiceLoginAbout", "Claude · ChatGPT · Gemini에 로그인돼 있으면 설정 없이 바로 씁니다.")],
      ["key", "②", t("settings.models.choiceKey", "API 키 넣기"), t("settings.models.choiceKeyAbout", "OpenRouter 같은 서비스의 키를 붙여 넣습니다. 쓴 만큼 요금이 나갑니다.")],
      ["local", "③", t("settings.models.choiceLocal", "이 컴퓨터에서 돌리기"), t("settings.models.choiceLocalAbout", "앱이 모델을 받아 이 Mac 안에서 돌립니다. 키도 요금도 없습니다.")],
      ["jev", "④", t("settings.models.choiceJev", "Jev 서비스"), t("settings.models.choiceJevAbout", "판단 전용 서비스입니다. Jev 키가 필요합니다.")],
    ]) {
      const button = modelNode("button", "model-choice");
      button.type = "button";
      button.dataset.modelChoice = id;
      button.setAttribute("aria-pressed", String(setup.choice === id));
      button.disabled = this.busy;
      button.append(modelNode("span", "model-choice-mark", mark), modelNode("strong", "", title), modelNode("span", "model-choice-about", about));
      button.addEventListener("click", () => this.choose(id));
      choices.append(button);
    }
    nodes.push(choices);
    if (setup.choice === "login") nodes.push(this.setupLogin());
    if (setup.choice === "key") nodes.push(this.setupKey());
    if (setup.choice === "local") nodes.push(this.setupLocal());
    if (setup.choice === "jev") nodes.push(this.setupJev());
    if (setup.checked === "ok") nodes.push(this.setupReco());
    if (setup.title === "add" || this.state?.connections.length || this.logins().some((login) => this.loginUsable(login))) {
      const close = this.button(t("settings.models.setupClose", "닫기"), () => this.closeSetup());
      close.dataset.modelSetupClose = "";
      const row = modelNode("div", "settings-action-row");
      row.append(close);
      nodes.push(row);
    }
    const focused = document.activeElement?.id;
    const typed = { key: el("model-setup-key")?.value ?? "", jev: el("model-setup-jev-key")?.value ?? "" };
    holder.replaceChildren(...nodes);
    if (el("model-setup-key")) el("model-setup-key").value = typed.key;
    if (el("model-setup-jev-key")) el("model-setup-jev-key").value = typed.jev;
    if (focused && el(focused)) el(focused).focus({ preventScroll: true });
  },
  checking() {
    const note = modelNode("div", "settings-notice model-verdict");
    note.setAttribute("role", "status");
    note.append(modelNode("strong", "", t("settings.models.checking", "연결을 확인하는 중…")), modelNode("span", "model-result-sub", t("settings.models.checkingList", "모델 목록을 불러오고 있습니다.")));
    return note;
  },
  loginLines() {
    const lines = modelNode("div", "model-lines");
    for (const login of this.logins()) {
      const line = modelNode("div", "model-line");
      line.dataset.modelLogin = login.provider;
      const copy = modelNode("div", "model-line-copy");
      copy.append(modelNode("strong", "model-line-name", login.name));
      copy.append(modelNode("span", "model-line-way", this.loginWords(login)));
      if (login.models.length) copy.append(this.namesFold(login.models.map((model) => model.displayName || model.id), login.provider));
      const actions = modelNode("div", "model-line-actions");
      actions.append(modelBadge(login.state), ...this.usePills((role) => role === "coding" && login.models.some((model) => model.id === this.codingKey(this.state?.codingModel))));
      line.append(copy, actions);
      lines.append(line);
    }
    return lines;
  },
  /* 「모델 N개」, opened to every name (덧붙임 C). */
  namesFold(names, id) {
    const fold = modelNode("details", "model-names");
    fold.dataset.modelNames = id;
    fold.open = this.namesOpen?.has(id) ?? false;
    fold.addEventListener("toggle", () => {
      this.namesOpen ??= new Set();
      if (fold.open) this.namesOpen.add(id); else this.namesOpen.delete(id);
    });
    fold.append(modelNode("summary", "", t("settings.models.showNames", "모델 {{count}}개 이름 보기", { count: names.length })));
    const list = modelNode("ul", "model-name-list");
    for (const name of names) list.append(modelNode("li", "", name));
    fold.append(list);
    return fold;
  },
  usePills(uses) {
    return [["coding", t("settings.models.usedCoding", "코딩에 쓰는 중")], ["judgment", t("settings.models.usedJudgment", "판단에 쓰는 중")]]
      .filter(([role]) => uses(role))
      .map(([, word]) => {
        const pill = modelNode("span", "settings-state model-use", word);
        pill.dataset.state = "connected";
        return pill;
      });
  },
  setupLogin() {
    const block = modelNode("section", "model-step");
    block.append(modelNode("h4", "", t("settings.models.loginTitle", "① 이 컴퓨터에서 찾은 로그인")));
    block.append(this.setup.checking ? this.checking() : this.loginLines());
    block.append(modelNode("p", "settings-hint", t("settings.models.loginNote", "로그인은 각 CLI(Claude Code · Codex · Antigravity …)가 이미 해 둔 것을 씁니다. 이 화면은 비밀번호나 토큰을 묻지 않습니다.")));
    return block;
  },
  setupKey() {
    const setup = this.setup;
    const block = modelNode("section", "model-step");
    block.append(modelNode("h4", "", t("settings.models.keyTitle", "② API 키 넣기")));
    const service = modelNode("label", "settings-field");
    service.dataset.field = "md";
    service.append(modelNode("span", "settings-label", t("settings.models.service", "어느 서비스인가요?")));
    const select = modelNode("select", "settings-input");
    select.id = "model-setup-service";
    select.disabled = this.busy;
    for (const preset of routerPresets) {
      const option = modelNode("option", "", preset.name);
      option.value = preset.id;
      select.append(option);
    }
    const other = modelNode("option", "", t("settings.models.otherService", "다른 서비스 (주소 직접 넣기)"));
    other.value = "custom";
    select.append(other);
    select.value = setup.presetId;
    select.addEventListener("change", () => {
      if (select.value === "custom") {
        this.addManual();
        return;
      }
      setup.presetId = select.value;
      setup.checked = null;
      this.renderSetup();
    });
    service.append(select);
    block.append(service);
    const preset = routerPresets.find((row) => row.id === setup.presetId);
    const local = preset && this.location(preset.base_url) === "local";
    const help = MODEL_KEY_HELP[setup.presetId]?.() ?? t("settings.models.keyHelp.other", "쓰는 서비스의 계정 페이지에서 API 키를 만들고 복사합니다.");
    block.append(modelNode("p", "settings-hint", t("settings.models.keyWhere", "키 받는 곳: {{help}}", { help })));
    const button = this.button(t("settings.models.check", "연결 확인"), () => void this.checkKey(), true);
    button.id = "model-setup-check";
    if (!local) {
      const field = modelNode("label", "settings-field");
      field.dataset.field = "lg";
      field.append(modelNode("span", "settings-label", t("settings.models.pasteKey", "키 붙여넣기")));
      const input = modelNode("input", "settings-input");
      input.id = "model-setup-key";
      input.type = "password";
      input.autocomplete = "off";
      input.spellcheck = false;
      input.placeholder = t("settings.models.pasteHere", "여기에 붙여 넣기");
      input.disabled = this.busy || el("router-key-input")?.disabled;
      input.addEventListener("input", () => { button.disabled = this.busy || !input.value.trim(); });
      field.append(input);
      block.append(field);
      block.append(modelNode("p", "settings-hint", el("router-key-input")?.disabled
        ? t("settings.apiRouters.noKeychainHint", "이 컴퓨터에는 API 키를 보관할 키체인이 없습니다. 키가 필요 없는 라우터(Ollama 등)만 저장할 수 있습니다.")
        : t("settings.models.keychainNote", "키는 이 Mac의 키체인에 저장됩니다. 설정 파일에는 키 이름만 적고, 화면에 다시 보여 주지 않습니다.")));
      button.disabled = true;
    }
    const row = modelNode("div", "settings-action-row");
    row.append(button);
    block.append(row);
    if (setup.checking) block.append(this.checking());
    else if (setup.checked === "ok") {
      const note = modelNode("div", "settings-notice model-verdict is-ok");
      note.setAttribute("role", "status");
      note.append(modelNode("strong", "", t("settings.models.connected", "연결됨 — 모델 {{count}}개", { count: setup.count })),
        modelNode("span", "model-result-sub", local ? t("settings.models.savedNoKey", "연결을 저장했습니다.") : t("settings.models.savedKey", "키는 키체인에 저장했습니다. 숫자는 서버가 알려 준 모델 수입니다.")));
      block.append(note);
    } else if (setup.checked === "fail") {
      const words = modelStateWords(setup.failure);
      const note = modelNode("div", "settings-notice model-verdict");
      note.setAttribute("role", "alert");
      note.append(modelNode("strong", "", words.word), modelNode("span", "model-result-sub", words.todo));
      if (setup.raw) {
        const fold = modelNode("details", "settings-fold");
        fold.append(modelNode("summary", "", t("settings.status.evidence", "자세히")), modelNode("p", "", setup.raw));
        note.append(fold);
      }
      block.append(note);
    }
    return block;
  },
  setupLocal() {
    const setup = this.setup;
    const block = modelNode("section", "model-step");
    block.append(modelNode("h4", "", t("settings.models.localTitle", "③ 이 컴퓨터에서 돌리기")));
    if (setup.checking) {
      block.append(this.checking());
      return block;
    }
    const lines = modelNode("div", "model-lines");
    const managed = this.judgmentRows().filter((row) => row.kind === "managed");
    for (const row of managed) {
      const line = modelNode("div", "model-line");
      const copy = modelNode("div", "model-line-copy");
      copy.append(modelNode("strong", "model-line-name", t("settings.models.localJudgment", "판단 모델 · {{model}}", { model: row.name })), modelNode("span", "model-line-way", t("settings.models.localJudgmentWay", "앱이 받아 이 Mac에서 돌리는 모델 · 키 불필요")));
      const actions = modelNode("div", "model-line-actions");
      actions.append(modelBadge(this.managedState(row.model)));
      line.append(copy, actions);
      lines.append(line);
    }
    const servers = (this.state?.connections ?? []).filter((row) => this.location(row.provider.base_url) === "local");
    const coding = modelNode("div", "model-line");
    const copy = modelNode("div", "model-line-copy");
    copy.append(modelNode("strong", "model-line-name", t("settings.models.coding", "코딩 모델")), modelNode("span", "model-line-way", servers.length
      ? t("settings.models.localServers", "이 Mac에서 직접 띄운 서버: {{names}}", { names: servers.map((row) => row.provider.name).join(" · ") })
      : t("settings.models.noLocalServer", "이 Mac에서 돌고 있는 코딩 모델 서버를 등록하지 않았습니다.")));
    const actions = modelNode("div", "model-line-actions");
    actions.append(modelBadge(servers.length ? "unknown" : "none"));
    coding.append(copy, actions);
    lines.append(coding);
    block.append(lines);
    if (!managed.length) {
      const note = modelNode("div", "settings-notice model-verdict");
      note.append(modelNode("strong", "", t("settings.models.noManaged", "이 컴퓨터에 받은 판단 모델이 없습니다.")), modelNode("span", "model-result-sub", t("settings.models.noManagedTodo", "「고급 — 이 컴퓨터에서 쓸 판단 모델 받기」에서 받으면 여기에 나타납니다.")));
      const row = modelNode("div", "settings-action-row");
      row.append(this.button(t("settings.models.receiveTitle", "이 컴퓨터에서 쓸 판단 모델 받기"), () => {
        this.closeSetup();
        el("model-advanced").open = true;
        el("judgment-model-source")?.scrollIntoView({ block: "nearest" });
      }));
      note.append(row);
      block.append(note);
    }
    const note = modelNode("div", "settings-notice model-verdict");
    note.append(modelNode("strong", "", t("settings.models.localOnlyJudgment", "이 방법은 판단 모델만 채웁니다.")),
      modelNode("span", "model-result-sub", t("settings.models.localOnlyJudgmentTodo", "코딩 모델은 ① 로그인 계정이나 ② API 키로 하나 더 연결하세요. 이 컴퓨터에서 큰 코딩 모델을 돌리는 것은 고급에서 합니다.")));
    const row = modelNode("div", "settings-action-row");
    row.append(this.button(t("settings.models.alsoLogin", "① 로그인 계정도 연결하기"), () => {
      const carried = this.setup;
      this.choose("login");
      this.setup.carryJudgment = carried.checked === "ok";
    }));
    note.append(row);
    block.append(note);
    return block;
  },
  setupJev() {
    const setup = this.setup;
    const block = modelNode("section", "model-step");
    block.append(modelNode("h4", "", t("settings.models.jevTitleStep", "④ Jev 서비스")));
    block.append(modelNode("p", "settings-row-desc", t("settings.models.judgmentAbout", "판단 모델이란? ZeroCode가 작은 결정을 빠르게 내릴 때 쓰는 모델입니다. 예를 들어 「이 명령이 위험한가」, 「어느 파일부터 볼까」 같은 것입니다. 코딩 모델과 따로 고르며, 작고 빠른 모델이면 충분합니다.")));
    block.append(modelNode("p", "settings-hint", t("settings.models.jevWhere", "키 받는 곳: Jev(TypeSafe) 계정 페이지에서 키를 만들고 복사합니다.")));
    const field = modelNode("label", "settings-field");
    field.dataset.field = "lg";
    field.append(modelNode("span", "settings-label", t("settings.models.pasteJev", "Jev 키 붙여넣기")));
    const input = modelNode("input", "settings-input");
    input.id = "model-setup-jev-key";
    input.type = "password";
    input.autocomplete = "off";
    input.spellcheck = false;
    input.placeholder = t("settings.models.pasteHere", "여기에 붙여 넣기");
    input.disabled = this.busy;
    const button = this.button(t("settings.models.check", "연결 확인"), () => void this.checkJev(), true);
    button.id = "model-setup-jev-check";
    button.disabled = true;
    input.addEventListener("input", () => { button.disabled = this.busy || !input.value.trim(); });
    field.append(input);
    block.append(field, modelNode("p", "settings-hint", t("settings.models.jevKeychain", "키는 「추천대로 쓰기」를 누를 때 이 Mac의 키체인에 저장됩니다.")));
    const row = modelNode("div", "settings-action-row");
    row.append(button);
    block.append(row);
    if (setup.checking) block.append(this.checking());
    else if (setup.checked) {
      const ok = setup.checked === "ok";
      const words = ok ? null : modelStateWords(setup.failure);
      const note = modelNode("div", `settings-notice model-verdict${ok ? " is-ok" : ""}`);
      note.setAttribute("role", ok ? "status" : "alert");
      note.append(modelNode("strong", "", ok ? t("settings.models.jevOk", "연결됨 — 판단에 쓸 수 있음") : words.word),
        modelNode("span", "model-result-sub", ok ? t("settings.models.jevOkSub", "Jev는 판단 전용입니다. 코딩 모델은 ①이나 ②로 하나 더 연결하세요.") : words.todo));
      block.append(note);
    }
    return block;
  },
  setupReco() {
    const setup = this.setup;
    const reco = {
      coding: setup.choice === "login" || setup.choice === "key" || setup.carryJudgment ? this.recommendCoding(setup.choice === "key" ? "key" : "login") : null,
      judgment: this.recommendJudgment(),
    };
    setup.draft.coding ??= reco.coding?.key ?? null;
    setup.draft.judgment ??= reco.judgment?.key ?? null;
    const block = modelNode("section", "model-step model-reco");
    const head = modelNode("div", "model-role-head");
    head.append(modelNode("h4", "", t("settings.models.step3", "추천대로 쓰기")), modelBadge("ok"));
    block.append(head);
    for (const role of ["coding", "judgment"]) {
      const key = setup.draft[role];
      const rows = role === "coding" ? this.codingRows() : this.judgmentRows();
      const row = rows.find((entry) => entry.key === key);
      const line = modelNode("div", "model-line");
      line.dataset.modelReco = role;
      const copy = modelNode("div", "model-line-copy");
      copy.append(modelNode("span", "model-line-way", role === "coding" ? t("settings.models.coding", "코딩 모델") : t("settings.judgment.title", "판단 모델")));
      copy.append(modelNode("strong", "model-line-name", row?.name ?? t("settings.models.notChosen", "아직 없음")));
      if (row) copy.append(modelNode("span", "model-line-way", row.way ?? this.codingWay(row)));
      else if (role === "coding") copy.append(modelNode("span", "model-line-note", t("settings.models.needCoding", "코딩 모델을 쓰려면 ①이나 ②로 하나 더 연결하세요.")));
      const why = reco[role];
      if (row) copy.append(modelNode("span", "model-line-note", key === why?.key ? t("settings.models.why", "추천 이유: {{why}}", { why: why.why }) : t("settings.models.picked", "직접 고름")));
      const actions = modelNode("div", "model-line-actions");
      if (row) {
        const open = setup.picking === role;
        const other = this.button(open ? t("settings.models.closeList", "목록 닫기") : t("settings.models.other", "다른 모델 고르기"), () => {
          setup.picking = open ? null : role;
          this.renderSetup();
        });
        other.setAttribute("aria-expanded", String(open));
        other.dataset.modelOther = role;
        actions.append(other);
      }
      line.append(copy, actions);
      block.append(line);
      if (setup.picking === role) {
        block.append(this.chooser(role, key, (picked, picks) => {
          setup.draft[picked] = picks.key;
          if (picked === "judgment") void this.judge(picks).then(() => this.renderSetup());
          this.renderSetup();
        }));
        if (role === "judgment" && key && key !== reco.judgment?.key) block.append(this.judgmentVerdict(key));
      }
    }
    block.append(modelNode("p", "settings-row-desc", t("settings.models.judgmentAbout", "판단 모델이란? ZeroCode가 작은 결정을 빠르게 내릴 때 쓰는 모델입니다. 예를 들어 「이 명령이 위험한가」, 「어느 파일부터 볼까」 같은 것입니다. 코딩 모델과 따로 고르며, 작고 빠른 모델이면 충분합니다.")));
    const judgmentOk = !setup.draft.judgment || setup.draft.judgment === reco.judgment?.key || this.judgmentReady(setup.draft.judgment);
    const apply = this.button(t("settings.models.step3", "추천대로 쓰기"), () => void this.applySetup(), true);
    apply.dataset.modelApplySetup = "";
    apply.disabled ||= !(setup.draft.coding || setup.draft.judgment) || !judgmentOk;
    const row = modelNode("div", "settings-action-row");
    row.append(apply);
    block.append(row, modelNode("p", "settings-hint", t("settings.models.applyNote", "한 번 누르면 두 모델을 함께 적용합니다. 코딩 모델은 새 zo 세션부터 바뀝니다. 판단 확인을 통과하지 못한 모델은 적용하지 않습니다.")));
    return block;
  },
  setupDone() {
    const result = this.setup.result;
    const block = modelNode("section", "model-step");
    const name = (role, key) => (role === "coding" ? this.codingRows() : this.judgmentRows()).find((entry) => entry.key === key)?.name ?? key;
    const lines = [];
    for (const role of ["coding", "judgment"]) {
      const one = result[role];
      if (!one) continue;
      const label = role === "coding" ? t("settings.models.coding", "코딩 모델") : t("settings.judgment.title", "판단 모델");
      if (one.applied && one.differs) lines.push([false, t("settings.models.appliedDiffers", "적용을 눌렀지만 다시 읽은 판단 모델은 {{read}}입니다.", { read: one.differs })]);
      else if (one.applied) lines.push([true, t("settings.models.doneRole", "{{role}}: {{model}}", { role: label, model: name(role, one.key) })]);
      else lines.push([false, t("settings.models.doneRoleFailed", "{{role}}: 적용하지 않았습니다 — {{reason}}", { role: label, reason: modelStateWords(one.state ?? "failed").word })]);
    }
    const ok = lines.every(([applied]) => applied);
    const note = modelNode("div", `settings-notice model-verdict${ok ? " is-ok" : ""}`);
    note.setAttribute("role", ok ? "status" : "alert");
    note.dataset.modelDone = "";
    note.append(modelNode("strong", "", ok ? t("settings.models.done", "끝났습니다.") : t("settings.models.doneSome", "일부만 적용했습니다.")));
    for (const [, words] of lines) note.append(modelNode("span", "model-result-sub", words));
    note.append(modelNode("span", "model-result-sub", t("settings.models.runningKept", "이미 열린 zo 세션은 지금 모델을 그대로 씁니다.")));
    if (result.judgment?.applied && result.sync) note.append(modelNode("span", "model-result-sub", this.syncWords(result.sync)));
    block.append(note);
    const row = modelNode("div", "settings-action-row");
    const view = this.button(t("settings.models.viewModels", "모델 화면 보기"), () => this.closeSetup(), true);
    view.dataset.modelSetupDone = "";
    row.append(view);
    block.append(row);
    return block;
  },

  renderConnections() {
    const holder = el("model-connection-groups");
    if (!holder) return;
    if (this.failed && !this.state) {
      holder.replaceChildren(modelNode("p", "settings-hint", this.failure(this.failed)), this.button(t("settings.managed.refresh", "상태 새로고침"), () => void this.refresh()));
      return;
    }
    const group = (id, title, lines, after = []) => {
      const section = modelNode("section", "model-group");
      section.dataset.connectionGroup = id;
      section.append(modelNode("h3", "model-role-title", title), ...after);
      const list = modelNode("div", "model-lines");
      list.append(...lines);
      section.append(list);
      return section;
    };
    const line = (id, name, sub, state, extra, actions) => {
      const row = modelNode("div", "model-line");
      row.dataset.connectionId = id;
      const copy = modelNode("div", "model-line-copy");
      copy.append(modelNode("strong", "model-line-name", name), modelNode("span", "model-line-way", sub), ...extra);
      const tail = modelNode("div", "model-line-actions");
      tail.append(state, ...actions);
      row.append(copy, tail);
      return row;
    };
    const codingKey = this.codingKey(this.state?.codingModel);
    const judgmentKey = this.currentJudgmentKey();
    const logins = this.loginLines();
    const remote = [];
    const local = [];
    for (const connection of this.state?.connections ?? []) {
      const isLocal = this.location(connection.provider.base_url) === "local";
      const models = (connection.provider.models ?? []).map((model) => (typeof model === "string" ? model : model.id));
      const missing = connection.provider.requires_auth && !connection.keyAvailable;
      const { state, check } = this.connectionCheck(connection);
      const words = modelStateWords(state, { env: connection.provider.auth_env ?? "" });
      const extra = [];
      // A check kept from an earlier start says how long ago it was made.
      const ago = check?.recalled && check.checkedMs != null && this.memory ? modelCheckAge(this.nowMs() - check.checkedMs) : "";
      if (words.todo && state !== "unknown") {
        extra.push(modelNode("span", "model-line-note", words.todo));
        if (ago) extra.push(modelNode("span", "model-line-note", t("settings.models.checkedAgo", "{{ago}} 확인", { ago })));
      } else if (check?.state === "ok") {
        extra.push(modelNode("span", "model-line-note", ago
          ? t("settings.models.connectedAgo", "연결됨 · {{ago}} 확인 — 모델 {{count}}개", { ago, count: check.count ?? 0 })
          : t("settings.models.connected", "연결됨 — 모델 {{count}}개", { count: check.count })));
      }
      const judgedOff = [...this.judged.entries()].find(([reference, answer]) => JSON.parse(reference)?.id === connection.id && answer.state !== "usable" && answer.state !== "checking");
      if (judgedOff && !missing) extra.push(modelNode("span", "model-line-note", t("settings.models.judgedOff", "판단 확인: {{word}} — {{todo}}", { word: modelStateWords(judgedOff[1].state).word, todo: modelStateWords(judgedOff[1].state).todo })));
      if (check?.unsaved || judgedOff?.[1].unsaved) {
        const unsaved = modelNode("span", "model-line-note", t("settings.models.unsaved", "이 확인 결과를 저장하지 못했습니다 — 창을 다시 켜면 이전 결과가 보입니다."));
        unsaved.dataset.checkUnsaved = connection.id;
        extra.push(unsaved);
      }
      const said = this.said.get(connection.id);
      if (said) extra.push(modelNode("span", "model-line-note", said.words));
      extra.push(this.namesFold(models, connection.id));
      const uses = this.usePills((role) => connection.uses?.includes(role) || (role === "coding" && models.some((model) => `${connection.provider.name}/${model}` === codingKey)));
      const actions = [...uses];
      const recheck = this.button(t("settings.models.recheck", "다시 확인"), () => void this.checkConnection(connection));
      recheck.dataset.connectionCheck = "";
      actions.push(recheck);
      if (connection.legacyKey) actions.push(this.legacyButton(connection));
      actions.push(this.button(t("settings.connections.configure", "설정"), () => this.edit(connection)));
      const remove = this.button(t("settings.connections.delete", "삭제"), () => void this.remove(connection));
      remove.classList.add("account-drop");
      actions.push(remove);
      const sub = `${connection.provider.base_url} · ${t("settings.connections.modelCount", "모델 {{count}}개", { count: models.length })}`;
      const row = line(connection.id, connection.provider.name, sub, modelBadge(state, { env: connection.provider.auth_env ?? "" }), extra, actions);
      if (this.deletion?.id === connection.id) row.append(this.deletionNote(connection));
      if (connection.legacyKey && this.adopting === connection.id) row.append(this.legacyConfirm(connection));
      (isLocal ? local : remote).push(row);
    }
    const managedLines = this.judgmentRows().filter((row) => row.kind === "managed").map((row) => {
      const state = this.managedState(row.model);
      const reason = this.managedReason(row.model);
      const extra = [];
      const about = {
        restarting: t("settings.models.managedRestarting", "실행 환경이 스스로 끝나 앱이 다시 띄우고 있습니다. 「멈추기」를 누르면 다시 띄우지 않습니다."),
        stopped: t("settings.models.managedStopped", "그동안 판단 요청은 보내지 않습니다. 「다시 시작」을 누르면 다시 띄우고 응답을 확인합니다."),
        busy: t("settings.models.managedBusy", "모델을 받는 중이라 지금은 멈추거나 다시 시작할 수 없습니다. 받기가 끝나거나 취소하면 다시 누를 수 있습니다."),
        changed: t("settings.models.managedChanged", "다른 창이나 zo가 설정을 먼저 바꿨습니다. 아무것도 바꾸지 않았습니다. 새로 읽은 뒤 다시 누르세요."),
      }[state];
      if (reason) extra.push(modelNode("span", "model-line-note", reason));
      if (about) extra.push(modelNode("span", "model-line-note", about));
      const actions = [...this.usePills((role) => role === "judgment" && judgmentKey === row.key)];
      const action = (word, name, disabled = false) => {
        const button = this.button(word, () => void this.managedAction(name, row.model));
        button.dataset.managedAction = name;
        button.disabled ||= disabled || managedJudgmentController.busy;
        actions.push(button);
      };
      if (state === "running" || state === "restarting" || state === "checking") action(t("settings.models.stop", "멈추기"), "stop");
      if (state === "stopped" || state === "none") action(t("settings.models.restart", "다시 시작"), "start", !managedJudgmentController.localSupported(row.model));
      if (state === "busy") {
        action(t("settings.models.stop", "멈추기"), "stop", true);
        action(t("settings.models.cancelReceive", "받기 취소"), "cancel");
      }
      if (state === "changed") action(t("settings.models.reread", "새로 읽기"), "reread");
      return line(row.key, row.name, t("settings.models.localJudgmentWay", "앱이 받아 이 Mac에서 돌리는 모델 · 키 불필요"), modelBadge(state), extra, actions);
    });
    const jev = typesafeState?.jevProvider ? [line("jev", "Jev", t("settings.models.jevWay", "판단 전용 서비스 · Jev 키는 키체인에"),
      modelBadge(typesafeState.provider?.kind === "jev" && typesafeState.keySaved ? "ok" : "unknown"), [],
      [...this.usePills((role) => role === "judgment" && judgmentKey === "jev"), this.button(t("settings.models.jevKey", "키"), () => {
        showSettingsPane("judgment-models");
        el("model-advanced").open = true;
        el("judgment-connection-settings").open = true;
        el("judgment-provider-kind").value = "jev";
        judgmentProviderController.changeKind();
        el("judgment-connection-settings").scrollIntoView({ block: "nearest" });
      })])] : [];
    holder.replaceChildren(
      group("login", t("settings.models.groupLogin", "① 로그인 계정"), [...logins.children]),
      group("key", t("settings.models.groupKey", "② API 키"), remote.length ? remote : [modelNode("p", "settings-hint", t("settings.apiRouters.noConnected", "연결된 API 라우터가 없습니다."))]),
      group("local", t("settings.models.groupLocal", "③ 이 컴퓨터"), [...managedLines, ...local].length ? [...managedLines, ...local] : [modelNode("p", "settings-hint", t("settings.models.noManaged", "이 컴퓨터에 받은 판단 모델이 없습니다."))]),
      group("jev", t("settings.models.groupJev", "④ Jev 서비스"), jev),
    );
  },
  deletionNote(connection) {
    const note = modelNode("div", "connection-delete-note settings-notice");
    note.setAttribute("role", "status");
    note.append(modelNode("p", "", this.deletion.message));
    const actions = modelNode("div", "settings-action-row");
    if (connection.uses.length) actions.append(this.button(t("settings.models.title", "모델"), () => showSettingsPane("judgment-models")));
    else if (this.deletion.confirm) actions.append(this.button(t("settings.connections.delete", "삭제"), () => void this.remove(connection, true)));
    actions.append(this.button(t("settings.connections.cancel", "취소"), () => { this.deletion = null; this.render(); }));
    note.append(actions);
    return note;
  },

  edit(row) {
    this.editing = row;
    onRouterPresetChange("custom");
    // Preset reset intentionally clears keys; an edit never reads a key into the DOM.
    this.editing = row;
    el("router-preset-select").value = "custom";
    el("router-name-input").value = row.provider.name;
    el("router-base-url-input").value = row.provider.base_url;
    el("router-auth-header").value = row.auth?.header ?? "Authorization";
    el("router-auth-scheme").value = row.auth?.scheme ?? "";
    testedModels = (row.provider.models ?? []).map(model => typeof model === "string" ? { id: model } : { id: model.id, context_length: model.context_window });
    paintRouterModels(testedModels);
    testedRouterGeneration = routerProbeGeneration;
    updateRouterSaveButtonState();
    el("connection-auth").value = row.provider.requires_auth ? row.keyAvailable ? "saved" : "new" : "none";
    this.addFields();
    say(el("connection-form-title"), () => t("settings.connections.configure", "설정"));
    this.show("add");
    el("router-name-input").focus();
    el("router-card").scrollIntoView({ block: "nearest" });
  },
  async remove(row, confirmed = false) {
    if (row.uses.length) {
      const roles = row.uses.map((role) => (role === "coding" ? t("settings.models.coding", "코딩 모델") : t("settings.judgment.title", "판단 모델"))).join(" · ");
      this.deletion = { id: row.id, message: `${roles} · ${t("settings.connections.inUseNote", "사용 중인 연결은 지울 수 없습니다. 지우려면 「모델」에서 다른 모델을 먼저 고르세요.")}` };
      this.render(); return;
    }
    if (!confirmed) { this.deletion = { id: row.id, confirm: true, message: t("settings.connections.deleteConfirm", "연결과 저장된 인증 참조를 삭제할까요? 서버와 모델 파일은 삭제하지 않습니다.") }; this.render(); return; }
    try { await invoke("remove_api_router", { name: row.provider.name, sourceId: row.id, expectedGeneration: row.generation }, { hand: handNow() }); this.deletion = null; await this.refresh(); }
    catch (error) { this.deletion = { id: row.id, message: this.failure(error) }; this.render(); }
  },
};

requestAnimationFrame(() => window.setTimeout(() => {
  const wake = () => void modelConnectionsController.wake();
  if (typeof window.requestIdleCallback === "function") window.requestIdleCallback(wake, { timeout: QUIET_CHECK_AFTER_PAINT_MS });
  else wake();
}, QUIET_CHECK_AFTER_PAINT_MS));
el("connections-decisions")?.addEventListener("click", () => openJevView());
el("model-open-orchestration")?.addEventListener("click", () => showSettingsPane("orchestration"));
for (const button of document.querySelectorAll("[data-connection-nav]")) button.addEventListener("click", () => modelConnectionsController.show(button.dataset.connectionNav));
el("connections-add")?.addEventListener("click", () => modelConnectionsController.add());
el("connection-auth")?.addEventListener("change", () => { invalidateRouterProbe(); modelConnectionsController.addFields(); });
