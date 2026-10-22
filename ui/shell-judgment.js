const judgmentProviderController = {
  initialized: false,
  state: null,
  draft: null,
  drafts: new Map(),
  modelIds: [],
  listedServer: null,
  loading: false,
  sequence: 0,
  keySequence: 0,
  localMode: "managed",
  usageSnapshot: null,

  currentUsage() {
    const usage = this.usageSnapshot;
    if (!this.state?.providerIdentity || usage?.providerIdentity !== this.state.providerIdentity || usage.scope !== "machine" ||
        !Number.isSafeInteger(usage.observedAtMs) || usage.observedAtMs < 0 || typeof usage.unattributedNewer !== "boolean") return null;
    if (usage.state === "unknown") return usage;
    if (usage.coverage !== "window_requests") return null;
    if (usage.state === "none_observed") return usage.lastRequest === null && !usage.unattributedNewer ? usage : null;
    const last = usage.lastRequest;
    if (usage.state !== "observed" || !last || typeof last.feature !== "string" || !last.feature ||
        typeof last.outcome !== "string" || !last.outcome ||
        (last.applied != null && typeof last.applied !== "boolean") ||
        (last.atMs != null && (!Number.isSafeInteger(last.atMs) || last.atMs < 0 || last.atMs > 8_640_000_000_000_000)) ||
        (last.elapsedMs != null && (!Number.isSafeInteger(last.elapsedMs) || last.elapsedMs < 0))) return null;
    return usage;
  },

  usageWords() {
    const usage = this.currentUsage();
    if (!usage || usage.state === "unknown") return usage?.reason === "unattributed"
      ? t("settings.judgment.usageUnattributed", "확인 불가 · 요청 당시 모델 정보가 없는 기록입니다.")
      : t("settings.judgment.usageUnknown", "확인할 수 없음 · 현재 모델의 실제 요청 기록을 확인하지 못했습니다.");
    const scope = t("settings.judgment.usageScope", "이 컴퓨터의 창 요청 · 최근 기록 범위");
    if (usage.state === "none_observed") return `${scope} · ${t("settings.judgment.usageNoneObserved", "최근 기록에서 현재 모델의 실제 요청을 찾지 못했습니다.")}`;
    const last = usage.lastRequest;
    const feature = jevFeature(last.feature)?.name;
    const name = feature ? t(feature.key, feature.source) : t("settings.judgment.usageFeature", "판단 기능");
    const when = last.atMs == null ? t("settings.judgment.usageTimeUnknown", "시각 정보 없음")
      : new Intl.DateTimeFormat(document.documentElement.lang || "ko", { dateStyle: "short", timeStyle: "medium" }).format(new Date(last.atMs));
    const failure = jevTokenRow(last.outcome);
    let outcome = last.outcome === "answered" ? t("settings.judgment.usageAnswered", "응답 성공")
      : last.outcome === "settings_changed" ? t("settings.judgment.usageDiscarded", "설정 변경으로 응답 미사용")
        : t("settings.judgment.usageFailed", "요청 실패");
    if (last.outcome !== "answered" && last.outcome !== "settings_changed" && failure) outcome += ` · ${t(failure.key, failure.word)}`;
    const http = /^http_(\d{3})$/.exec(last.outcome);
    if (http) outcome += ` (HTTP ${http[1]})`;
    const applied = last.applied === true ? t("settings.judgment.usageApplied", "결과 적용됨")
      : last.applied === false ? t("settings.judgment.usageNotApplied", "결과 미적용")
        : t("settings.judgment.usageAppliedUnknown", "결과 적용 여부 모름");
    const words = [t("settings.judgment.usageObserved", "{{scope}} · 확인된 실제 요청: {{feature}} · {{time}} · {{outcome}} · {{applied}}", {
      scope, feature: name, time: when, outcome, applied,
    })];
    if (last.elapsedMs != null) words.push(t("settings.judgment.usageLatency", "{{ms}} ms", { ms: last.elapsedMs }));
    if (usage.unattributedNewer) words.push(t("settings.judgment.usageNewerUnknown", "더 새 기록에는 요청 당시 모델 정보가 없어 결과를 확인할 수 없습니다."));
    return words.join(" · ");
  },

  sameProvider(left, right) {
    return Boolean(left && right && JSON.stringify(this.copy(left)) === JSON.stringify(this.copy(right)));
  },

  managedServer() {
    const state = this.state;
    const snapshot = managedJudgmentController.snapshot;
    if (!state?.providerIdentity || state.provider?.kind !== "local" || managedJudgmentController.statusError || snapshot?.providerIdentity !== state.providerIdentity) return null;
    const server = snapshot.server;
    if (server.modelId === state.provider.model && this.sameProvider(server.provider, state.provider)) return server;
    if (snapshot.configuredModelId === state.provider.model && ["stopped", "failed"].includes(server.state) &&
        (!server.modelId || server.modelId === state.provider.model) && (!server.provider || this.sameProvider(server.provider, state.provider))) return server;
    return null;
  },

  checkStanding() {
    if (!this.state?.providerIdentity) return "unchecked";
    if (typesafeState?.providerIdentity === this.state.providerIdentity && typesafeChecked) return typesafeChecked;
    const server = this.managedServer();
    return server?.state === "ready" && server.checked === true ? "connected" : "unchecked";
  },

  async refreshAfterApply() {
    if (typesafeRefreshing) await typesafeRefreshing.promise;
    await refreshTypeSafe();
  },

  renderStatus() {
    const state = this.state;
    const provider = state?.provider;
    say(el("judgment-active-model"), () => this.state?.provider
      ? t("settings.judgment.activeProvider", "현재 적용된 설정: {{provider}} · {{model}}", {
        provider: this.providerName(this.state.provider), model: this.state.provider.model,
      })
      : t("settings.judgment.activeUnknown", "현재 적용된 모델을 읽지 못했습니다. 연결 설정을 다시 확인하세요."));
    const enablement = el("judgment-current-enablement");
    enablement.dataset.state = typeof state?.jev?.on === "boolean" ? state.jev.on ? "enabled" : "disabled" : "unknown";
    say(enablement, () => {
      const jev = this.state?.jev;
      if (typeof jev?.on !== "boolean") return t("settings.judgment.enablementUnknown", "요청 허용 상태를 읽지 못했습니다.");
      if (!jev.on) return t("settings.judgment.requestsDisabled", "요청 비활성 · 기능 설정과 프로젝트 동의를 확인하세요.");
      if (jev.everywhere) return t("settings.judgment.requestsEverywhere", "모든 프로젝트에서 요청 허용");
      return Number.isSafeInteger(jev.folders) && jev.folders >= 0
        ? t("settings.judgment.requestsPartial", "동의한 폴더 {{count}}개에서 요청 허용 · 현재 프로젝트 허용 여부 모름", { count: jev.folders })
        : t("settings.judgment.requestsConsented", "동의한 프로젝트에서 요청 허용 · 현재 프로젝트 허용 여부 모름");
    });
    const server = el("judgment-current-server");
    const managed = this.managedServer();
    server.dataset.state = managed?.state ?? "unknown";
    say(server, () => {
      const current = this.managedServer();
      if (current?.state === "ready") return t("settings.judgment.serverReady", "실행 중 · 실제 요청 사용과 별개");
      if (current?.state === "starting") return t("settings.judgment.serverStarting", "시작 중");
      if (current?.state === "stopped") return t("settings.judgment.serverStopped", "중지됨 · 적용된 설정은 유지됨");
      if (current?.state === "failed") return t("settings.judgment.serverFailed", "실행 실패 · 적용된 설정은 유지됨");
      return t("settings.judgment.serverUnknown", "현재 모델의 실행 상태를 확인할 수 없습니다.");
    });
    const check = el("judgment-current-check");
    check.dataset.state = this.checkStanding();
    say(check, () => this.checkStanding() === "connected"
      ? t("settings.judgment.responseChecked", "응답 확인됨 · 시험 요청 결과이며 실제 사용과 별개")
      : this.checkStanding() === "failed"
        ? t("settings.judgment.responseFailed", "응답 확인 실패 · 실제 사용 이력과 별개")
        : t("settings.judgment.responseUnknown", "현재 모델의 응답 확인 결과가 없습니다."));
    el("judgment-current-usage").dataset.state = this.currentUsage()?.state ?? "unknown";
    say(el("judgment-current-usage"), () => this.usageWords());
    say(el("judgment-current-note"), () => typesafeRefreshing || managedJudgmentController.loading
      ? t("settings.judgment.statusLoading", "현재 설정과 서버·실제 요청 기록을 읽는 중…")
      : typesafeRefreshFailed || managedJudgmentController.statusError
        ? t("settings.judgment.statusUnavailable", "현재 상태를 모두 읽지 못했습니다. 상태 새로고침으로 다시 확인하세요.") : "");
    el("judgment-status-refresh").disabled = Boolean(typesafeRefreshing || managedJudgmentController.loading);
    const draft = el("judgment-provider-draft");
    draft.dataset.state = this.dirty() ? "unapplied" : provider ? "applied" : "unknown";
    draft.classList.toggle("settings-notice", this.dirty());
    say(draft, () => this.dirty()
      ? t("settings.judgment.draftProvider", "아직 적용하지 않은 연결 초안: {{provider}} · {{model}}", {
        provider: this.providerName(this.draft), model: this.draft.model || t("settings.judgment.modelUnselected", "모델 미선택"),
      })
      : this.state?.provider
        ? t("settings.judgment.editingApplied", "현재 적용된 연결 설정입니다. 값을 바꾸면 변경 초안이 됩니다.")
        : t("settings.judgment.activeUnknown", "현재 적용된 모델을 읽지 못했습니다. 연결 설정을 다시 확인하세요."));
  },

  copy(provider) {
    return {
      kind: provider.kind,
      protocol: provider.protocol,
      baseUrl: provider.baseUrl,
      model: provider.model,
      requiresAuth: provider.requiresAuth,
    };
  },

  serverKey(provider) {
    return JSON.stringify([provider.kind, provider.protocol, provider.baseUrl, provider.requiresAuth]);
  },

  dirty() {
    return Boolean(this.draft && this.state?.provider &&
      (JSON.stringify(this.draft) !== JSON.stringify(this.copy(this.state.provider)) || this.enteredKey(this.draft)));
  },

  enteredKey(provider) {
    return provider?.requiresAuth ? el("typesafe-key-input")?.value.trim() || null : null;
  },

  keyEdited() {
    this.keySequence += 1;
    this.forgetList(true);
    paintTypeSafeStatus("");
    this.controls();
  },

  current() {
    return this.draft ? this.copy(this.draft) : null;
  },

  failure(token) {
    const row = jevTokenRow(token ?? "");
    return row?.saidKey ? t(row.saidKey, row.said) : null;
  },

  providerName(provider) {
    if (provider.kind === "local" && provider.protocol === "openai_json") return t("settings.gguf.unverified", "사람이 지정한 검증 안 된 모델");
    if (provider.kind === "local") return t("settings.judgment.local", "로컬 판단 모델");
    if (provider.kind === "remote") return t("settings.judgment.remote", "다른 판단 서버");
    return "Jev · TypeSafe";
  },

  setupHint() {
    if (this.draft?.kind === "local" && this.localMode === "managed") {
      return ["local", t("settings.managed.setupHint", "실행 환경과 모델을 설치한 뒤 실행하고 적용을 누르세요. 서버 주소나 포트는 직접 입력하지 않아도 됩니다.")];
    }
    if (!this.draft?.baseUrl) {
      return ["connection", t("settings.judgment.nextConnection", "먼저 연결할 판단 서버 주소를 입력하세요. 모델을 고르기 전까지 현재 설정은 유지됩니다.")];
    }
    const sameServer = this.state?.provider && this.serverKey(this.draft) === this.serverKey(this.state.provider);
    if (sameServer && this.draft.requiresAuth && !this.state.keySaved && !this.enteredKey(this.draft)) {
      return ["key", t("settings.judgment.nextKey", "이 서비스의 API 키를 입력하고 연결하고 모델 불러오기를 누르세요. 키는 이 서비스에만 사용합니다.")];
    }
    if (!this.draft.model && this.draft.kind !== "jev") {
      return ["model", t("settings.judgment.nextModel", "모델 목록을 읽고 사용할 모델을 고르세요. 모델 ID를 알고 있다면 직접 입력해도 됩니다.")];
    }
    if (this.dirty()) {
      return ["apply", t("settings.judgment.nextApply", "확인하고 사용을 누르면 먼저 이 모델에 시험 요청을 보냅니다. 응답을 확인한 경우에만 현재 모델을 바꿉니다.")];
    }
    return ["check", t("settings.judgment.nextCheck", "현재 설정이 적용돼 있습니다. 연결 확인은 모델의 응답만 확인하며 작업 정확도를 보장하지 않습니다.")];
  },

  init() {
    if (this.initialized) return;
    this.initialized = true;
    el("judgment-status-refresh")?.addEventListener("click", () => void refreshTypeSafe());
    document.addEventListener("visibilitychange", () => {
      if (managedJudgmentController.visible()) void refreshTypeSafe();
    });
    el("judgment-provider-kind")?.addEventListener("change", () => this.changeKind());
    for (const mode of ["managed", "manual"]) {
      el(`judgment-local-${mode}`)?.addEventListener("click", () => {
        if (mode === "manual" && this.draft?.protocol === "openai_json" && this.managedServer()) this.reset({ kind: "local", protocol: "openai_json", baseUrl: "", model: "", requiresAuth: false });
        this.localMode = mode;
        this.render();
      });
    }
    for (const name of ["judgment-endpoint", "judgment-protocol", "typesafe-model-input", "judgment-requires-auth"]) {
      el(name)?.addEventListener("input", () => this.read());
      el(name)?.addEventListener("change", () => this.read());
    }
    el("judgment-refresh-models")?.addEventListener("click", () => void this.refreshModels());
    el("judgment-listed-model")?.addEventListener("change", (event) => {
      if (!event.target.value) return;
      el("typesafe-model-input").value = event.target.value;
      this.read();
    });
    el("judgment-save-provider")?.addEventListener("click", () => void this.apply());
    el("judgment-revert-provider")?.addEventListener("click", () => {
      if (!this.state?.provider) return;
      el("typesafe-key-input").value = "";
      this.reset(this.state.provider);
      this.render();
    });
  },

  reset(provider) {
    const changed = !this.sameProvider(this.draft, provider);
    const changedServer = !this.draft || this.serverKey(this.draft) !== this.serverKey(provider);
    this.draft = this.copy(provider);
    if (changed) paintTypeSafeStatus("");
    if (changedServer) this.forgetList();
  },

  forgetList(keepKey = false) {
    this.sequence += 1;
    this.loading = false;
    this.modelIds = [];
    this.listedServer = null;
    el("judgment-model-options")?.replaceChildren();
    el("judgment-listed-model")?.replaceChildren();
    if (el("judgment-listed-model-field")) el("judgment-listed-model-field").hidden = true;
    if (el("judgment-model-status")) el("judgment-model-status").textContent = "";
    if (!keepKey && el("typesafe-key-input")) el("typesafe-key-input").value = "";
  },

  changeKind() {
    if (!this.draft || !this.state) return;
    this.drafts.set(this.draft.kind, this.copy(this.draft));
    const kind = el("judgment-provider-kind").value;
    const provider = kind === "jev" ? this.state.jevProvider :
      this.drafts.get(kind) ?? { kind, protocol: kind === "remote" ? "openai_json" : "system_one", baseUrl: "", model: "", requiresAuth: kind === "remote" };
    if (!provider) return;
    this.reset(provider);
    this.render();
  },

  connectModel(model) {
    const modelId = managedJudgmentController.serverModel(model);
    if (!modelId || typesafeBusy || managedJudgmentController.busy || managedJudgmentController.operating()) return;
    const kind = this.draft?.kind === "remote" ? "remote" : "local";
    const previous = this.draft?.kind === kind ? this.draft : this.drafts.get(kind);
    const provider = previous ? this.copy(previous)
      : { kind, protocol: "system_one", baseUrl: "", model: "", requiresAuth: kind === "remote" };
    if (provider.protocol !== "system_one") { provider.protocol = "system_one"; provider.baseUrl = ""; provider.requiresAuth = kind === "remote"; }
    provider.model = modelId;
    el("judgment-connection-settings").open = true;
    this.localMode = "manual";
    this.reset(provider);
    this.render();
    el(provider.baseUrl ? "typesafe-model-input" : "judgment-endpoint")?.focus();
  },

  read() {
    if (!this.draft) return;
    const previousDraft = JSON.stringify(this.draft);
    const previous = this.serverKey(this.draft);
    if (this.draft.kind !== "jev") {
      this.draft.baseUrl = el("judgment-endpoint").value.trim();
      this.draft.protocol = el("judgment-protocol").value;
      this.draft.requiresAuth = el("judgment-requires-auth").checked;
    }
    this.draft.model = el("typesafe-model-input").value.trim();
    if (previousDraft !== JSON.stringify(this.draft)) paintTypeSafeStatus("");
    if (previous !== this.serverKey(this.draft)) this.forgetList();
    this.controls();
  },

  paint(state) {
    this.init();
    const wasDirty = this.dirty();
    const changed = this.state?.providerIdentity !== state.providerIdentity;
    this.state = state;
    if (changed) this.usageSnapshot = null;
    if (!state.provider) {
      this.forgetList();
      this.draft = null;
      this.controls();
      return;
    }
    if (!this.draft || !wasDirty) this.reset(state.provider);
    if (changed) typesafeChecked = null;
    this.render();
  },

  render() {
    if (!this.draft) return;
    managedJudgmentController.init();
    const local = this.draft.kind === "local";
    const managed = local && this.localMode === "managed";
    el("judgment-local-mode").hidden = !local;
    el("managed-judgment-panel").hidden = false;
    el("judgment-manual-fields").hidden = managed;
    for (const mode of ["managed", "manual"]) {
      el(`judgment-local-${mode}`).setAttribute("aria-pressed", String(this.localMode === mode));
    }
    el("judgment-provider-kind").value = this.draft.kind;
    el("judgment-protocol").value = this.draft.protocol;
    el("judgment-protocol-field").hidden = this.draft.kind === "jev";
    if (document.activeElement !== el("judgment-endpoint")) el("judgment-endpoint").value = this.draft.baseUrl;
    if (document.activeElement !== el("typesafe-model-input")) el("typesafe-model-input").value = this.draft.model;
    el("typesafe-model-input").placeholder = this.draft.kind === "jev" ? this.state.model?.alias ?? "" : "";
    el("judgment-requires-auth").checked = this.draft.requiresAuth;
    el("judgment-endpoint-field").hidden = this.draft.kind === "jev";
    el("judgment-auth-field").hidden = this.draft.kind === "jev";
    const scope = this.draft.kind === "local"
      ? t("settings.judgment.localScope", "이 컴퓨터의 판단 서버에 연결합니다. Jev 키는 보내지 않습니다. 서버 자체의 외부 통신 설정도 확인하세요.")
      : t("settings.judgment.remoteScope", "동의한 프로젝트의 내용이 선택한 판단 서비스로 전송됩니다. 모델을 바꿔도 승인·정지·결과 검증은 유지합니다.");
    el("judgment-provider-scope").textContent = scope;
    this.controls();
    managedJudgmentController.render();
    if (!managedJudgmentController.snapshot && !managedJudgmentController.loading && !managedJudgmentController.statusError && managedJudgmentController.visible()) void managedJudgmentController.refresh();
  },

  controls() {
    const unavailable = !this.draft;
    const dirty = this.dirty();
    const busy = typesafeBusy || unavailable || managedJudgmentController.activating;
    const managed = this.draft?.kind === "local" && this.localMode === "managed";
    const auth = this.draft?.requiresAuth ?? true;
    paintKeyField(el("typesafe-key-input"), { busy, kept: this.state?.keysKeptHere });
    this.renderStatus();
    const hint = el("judgment-setup-hint");
    if (hint) {
      const [step, words] = this.draft ? this.setupHint() : ["unavailable", ""];
      hint.dataset.step = step;
      hint.textContent = words;
    }
    for (const name of ["judgment-provider-kind", "judgment-endpoint", "judgment-protocol", "typesafe-model-input", "judgment-requires-auth"]) {
      const element = el(name);
      if (element) element.disabled = busy;
    }
    const apply = el("judgment-save-provider");
    if (apply) {
      apply.hidden = managed;
      apply.disabled = busy || this.loading || !dirty || !this.draft.baseUrl || (!this.draft.model && this.draft.kind !== "jev");
    }
    const revert = el("judgment-revert-provider");
    if (revert) revert.disabled = busy || !dirty;
    const refresh = el("judgment-refresh-models");
    if (refresh) {
      refresh.hidden = managed;
      refresh.disabled = busy || this.loading || !this.draft.baseUrl;
    }
    const notice = el("judgment-provider-dirty");
    if (notice) notice.hidden = !dirty;
    const keyState = el("typesafe-key-state");
    if (keyState) keyState.hidden = dirty;
    const keyField = el("typesafe-key-input")?.closest("label");
    if (keyField) keyField.hidden = !auth;
    const saveKey = el("typesafe-save-btn");
    if (saveKey) saveKey.hidden = managed || !auth;
    const removeKey = el("typesafe-remove-btn");
    if (removeKey) {
      removeKey.hidden = managed || !auth || dirty;
      removeKey.disabled = busy || dirty || !this.state?.keySaved;
    }
    const check = el("typesafe-check-btn");
    if (check) {
      check.hidden = managed;
      check.disabled = busy || !this.draft?.baseUrl || (!this.draft?.model && this.draft?.kind !== "jev") || (!dirty && this.state?.provider?.requiresAuth && !this.state.keySaved && !el("typesafe-key-input")?.value.trim());
    }
    const listed = el("judgment-listed-model");
    if (listed) {
      listed.disabled = busy || this.loading;
      listed.value = this.modelIds.includes(this.draft?.model) ? this.draft.model : "";
    }
    for (const mode of ["managed", "manual"]) el(`judgment-local-${mode}`).disabled = busy;
    paintTypeSafeSave();
  },

  async apply() {
    if (typesafeBusy || !this.draft || !this.dirty()) return;
    const provider = this.copy(this.draft);
    if (provider.kind === "jev" && !provider.model) provider.model = this.state.model.alias;
    const expectedProviderIdentity = this.state.providerIdentity;
    const draftIdentity = JSON.stringify(this.current());
    const key = this.enteredKey(provider);
    let applied = false;
    await runTypeSafe(async () => {
      const check = await this.probe(provider, expectedProviderIdentity, draftIdentity, key);
      if (!check.answered) throw { kind: "failed", message: typesafeCheckFailure(check.failure) };
      const state = await invoke("set_judgment_provider", { provider, expectedProviderIdentity, key });
      typesafeChecked = null;
      el("typesafe-key-input").value = "";
      this.reset(state.provider);
      this.state = state;
      applied = true;
      return state;
    }, () => t("settings.judgment.checkedAndApplied", "응답을 확인한 모델을 적용했습니다. 연결 확인은 작업 정확도 검증과 별개입니다."));
    if (applied && this.state?.providerIdentity === typesafeState?.providerIdentity) {
      const identity = this.state.providerIdentity;
      await this.refreshAfterApply();
      if (identity !== typesafeState?.providerIdentity) return;
      typesafeChecked = "connected";
      paintTypeSafe(typesafeState);
    }
  },

  async probe(provider, expectedProviderIdentity, draftIdentity, key = this.enteredKey(provider)) {
    const keySequence = this.keySequence;
    const check = await invoke("preflight_judgment_provider", { provider, expectedProviderIdentity, key });
    if (keySequence !== this.keySequence || key !== this.enteredKey(provider) || this.state?.providerIdentity !== expectedProviderIdentity || JSON.stringify(this.current()) !== draftIdentity || check.activeProviderIdentity !== expectedProviderIdentity) throw "settings_changed";
    if (typeof check.candidateIdentity !== "string" || !check.candidateIdentity || typeof check.answered !== "boolean") throw "schema";
    return check;
  },

  async checkDraft() {
    if (typesafeBusy || !this.draft) return;
    const provider = this.copy(this.draft);
    if (provider.kind === "jev" && !provider.model) provider.model = this.state.model.alias;
    const expectedProviderIdentity = this.state.providerIdentity;
    const draftIdentity = JSON.stringify(this.current());
    typesafeBusy = true;
    this.controls();
    managedJudgmentController.render();
    paintTypeSafeStatus(t("settings.judgment.checkingDraft", "현재 모델을 유지하면서 새 연결을 확인하는 중…"));
    try {
      const check = await this.probe(provider, expectedProviderIdentity, draftIdentity);
      const model = check.model?.startsWith(`${check.candidateIdentity}:`)
        ? check.model.slice(check.candidateIdentity.length + 1) : check.model ?? provider.model;
      paintTypeSafeStatus(check.answered
        ? t("settings.judgment.draftAnswered", "{{model}} 응답 확인 · 아직 적용하지 않았습니다. 확인하고 사용을 누르면 전환합니다.", { model })
        : typesafeCheckFailure(check.failure), { standing: check.answered ? "connected" : "failed" });
    } catch (error) {
      paintTypeSafeStatus(this.failure(String(error)) ?? typesafeRefusal(error), { standing: "failed" });
    } finally {
      typesafeBusy = false;
      this.controls();
      managedJudgmentController.render();
    }
  },

  async refreshModels() {
    if (typesafeBusy || this.loading || !this.draft?.baseUrl) return;
    const provider = this.copy(this.draft);
    const server = this.serverKey(provider);
    const sequence = ++this.sequence;
    this.loading = true;
    this.controls();
    el("judgment-model-status").textContent = t("settings.judgment.loadingModels", "서버의 판단 모델 목록을 읽는 중…");
    try {
      const models = await invoke("judgment_models", { provider, key: this.enteredKey(provider) });
      if (sequence !== this.sequence || server !== this.serverKey(this.draft)) return;
      this.modelIds = [...new Set(models.map((model) => model.id))];
      this.listedServer = server;
      const options = this.modelIds.map((id) => {
        const option = document.createElement("option");
        option.value = id;
        return option;
      });
      el("judgment-model-options").replaceChildren(...options);
      const listed = el("judgment-listed-model");
      const unselectedOption = document.createElement("option");
      unselectedOption.value = "";
      unselectedOption.textContent = t("settings.judgment.chooseListed", "서버에서 읽은 모델 선택");
      listed.replaceChildren(unselectedOption, ...this.modelIds.map((id) => {
        const option = document.createElement("option");
        option.value = id;
        option.textContent = id;
        return option;
      }));
      el("judgment-listed-model-field").hidden = this.modelIds.length === 0;
      el("judgment-model-status").textContent = t("settings.judgment.modelsRead", "모델 {{count}}개를 읽었습니다. 선택한 ID는 그대로 유지합니다.", { count: this.modelIds.length });
    } catch (error) {
      if (sequence !== this.sequence) return;
      el("judgment-model-status").textContent = t("settings.judgment.modelsUnavailable", "목록을 읽지 못했습니다. {{reason}}", { reason: typesafeRefusal(error) });
    } finally {
      if (sequence === this.sequence) {
        this.loading = false;
        this.controls();
      }
    }
  },
};
