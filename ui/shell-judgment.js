const judgmentProviderController = {
  initialized: false,
  state: null,
  draft: null,
  drafts: new Map(),
  modelIds: [],
  listedServer: null,
  loading: false,
  sequence: 0,

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
      JSON.stringify(this.draft) !== JSON.stringify(this.copy(this.state.provider)));
  },

  current() {
    return this.draft ? this.copy(this.draft) : null;
  },

  failure(token) {
    const row = jevTokenRow(token ?? "");
    return row?.saidKey ? t(row.saidKey, row.said) : null;
  },

  init() {
    if (this.initialized) return;
    this.initialized = true;
    el("judgment-provider-kind")?.addEventListener("change", () => this.changeKind());
    for (const name of ["judgment-endpoint", "typesafe-model-input", "judgment-requires-auth"]) {
      el(name)?.addEventListener("input", () => this.read());
      el(name)?.addEventListener("change", () => this.read());
    }
    el("judgment-refresh-models")?.addEventListener("click", () => void this.refreshModels());
    el("judgment-save-provider")?.addEventListener("click", () => void this.apply());
    el("judgment-revert-provider")?.addEventListener("click", () => {
      if (!this.state?.provider) return;
      this.reset(this.state.provider);
      this.render();
    });
  },

  reset(provider) {
    const changedServer = !this.draft || this.serverKey(this.draft) !== this.serverKey(provider);
    this.draft = this.copy(provider);
    if (changedServer) this.forgetList();
  },

  forgetList() {
    this.sequence += 1;
    this.loading = false;
    this.modelIds = [];
    this.listedServer = null;
    el("judgment-model-options")?.replaceChildren();
    if (el("judgment-model-status")) el("judgment-model-status").textContent = "";
    if (el("typesafe-key-input")) el("typesafe-key-input").value = "";
  },

  changeKind() {
    if (!this.draft || !this.state) return;
    this.drafts.set(this.draft.kind, this.copy(this.draft));
    const kind = el("judgment-provider-kind").value;
    const provider = kind === "jev" ? this.state.jevProvider :
      this.drafts.get(kind) ?? { kind, protocol: "system_one", baseUrl: "", model: "", requiresAuth: kind === "remote" };
    if (!provider) return;
    this.reset(provider);
    this.render();
  },

  read() {
    if (!this.draft) return;
    const previous = this.serverKey(this.draft);
    if (this.draft.kind !== "jev") {
      this.draft.baseUrl = el("judgment-endpoint").value.trim();
      this.draft.requiresAuth = el("judgment-requires-auth").checked;
    }
    this.draft.model = el("typesafe-model-input").value.trim();
    if (previous !== this.serverKey(this.draft)) this.forgetList();
    this.controls();
  },

  paint(state) {
    this.init();
    const wasDirty = this.dirty();
    const changed = this.state?.providerIdentity !== state.providerIdentity;
    this.state = state;
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
    el("judgment-provider-kind").value = this.draft.kind;
    if (document.activeElement !== el("judgment-endpoint")) el("judgment-endpoint").value = this.draft.baseUrl;
    if (document.activeElement !== el("typesafe-model-input")) el("typesafe-model-input").value = this.draft.model;
    el("typesafe-model-input").placeholder = this.draft.kind === "jev" ? this.state.model?.alias ?? "" : "";
    el("judgment-requires-auth").checked = this.draft.requiresAuth;
    el("judgment-endpoint-field").hidden = this.draft.kind === "jev";
    el("judgment-auth-field").hidden = this.draft.kind === "jev";
    const scope = this.draft.kind === "local"
      ? t("settings.judgment.localScope", "이 컴퓨터의 판단 서버에 연결합니다. Jev 키는 보내지 않습니다. 서버 자체의 외부 통신 설정도 확인하세요.")
      : t("settings.judgment.remoteScope", "동의한 프로젝트의 내용이 선택한 판단 서비스로 전송됩니다. 모델을 바꿔도 승인·정지·결과 검증은 유지합니다.");
    el("judgment-provider-scope").textContent = `${scope} ${t("settings.judgment.active", "현재 적용: {{model}}", { model: this.state.provider.model })}`;
    this.controls();
  },

  controls() {
    const unavailable = !this.draft;
    const dirty = this.dirty();
    const busy = typesafeBusy || unavailable;
    const auth = this.draft?.requiresAuth ?? true;
    for (const name of ["judgment-provider-kind", "judgment-endpoint", "typesafe-model-input", "judgment-requires-auth"]) {
      const element = el(name);
      if (element) element.disabled = busy;
    }
    const apply = el("judgment-save-provider");
    if (apply) apply.disabled = busy || !dirty || !this.draft.baseUrl || (!this.draft.model && this.draft.kind !== "jev");
    const revert = el("judgment-revert-provider");
    if (revert) revert.disabled = busy || !dirty;
    const refresh = el("judgment-refresh-models");
    if (refresh) refresh.disabled = busy || this.loading || !this.draft.baseUrl;
    const notice = el("judgment-provider-dirty");
    if (notice) notice.hidden = !dirty;
    const keyField = el("typesafe-key-input")?.closest("label");
    if (keyField) keyField.hidden = !auth;
    const saveKey = el("typesafe-save-btn");
    if (saveKey) saveKey.hidden = !auth;
    const removeKey = el("typesafe-remove-btn");
    if (removeKey) {
      removeKey.hidden = !auth || dirty;
      removeKey.disabled = busy || dirty || !this.state?.keySaved;
    }
    const check = el("typesafe-check-btn");
    if (check) check.disabled = busy || dirty || (this.state?.provider?.requiresAuth && !this.state.keySaved);
    paintTypeSafeSave();
  },

  async apply() {
    if (typesafeBusy || !this.draft || !this.dirty()) return;
    const provider = this.copy(this.draft);
    if (provider.kind === "jev" && !provider.model) provider.model = this.state.model.alias;
    await runTypeSafe(async () => {
      const state = await invoke("set_judgment_provider", { provider });
      typesafeChecked = null;
      this.reset(state.provider);
      this.state = state;
      return state;
    }, () => t("settings.judgment.applied", "판단 모델을 적용했습니다. 연결 확인은 정확도 검증과 별개입니다."));
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
      const models = await invoke("judgment_models", { provider });
      if (sequence !== this.sequence || server !== this.serverKey(this.draft)) return;
      this.modelIds = [...new Set(models.map((model) => model.id))];
      this.listedServer = server;
      const options = this.modelIds.map((id) => {
        const option = document.createElement("option");
        option.value = id;
        return option;
      });
      el("judgment-model-options").replaceChildren(...options);
      el("judgment-model-status").textContent = t("settings.judgment.modelsRead", "모델 {{count}}개를 읽었습니다. 선택한 ID는 그대로 유지합니다.", { count: this.modelIds.length });
    } catch (error) {
      if (sequence !== this.sequence) return;
      el("judgment-model-status").textContent = t("settings.judgment.modelsUnavailable", "목록을 읽지 못했습니다. 서버와 키를 확인하거나 모델 ID를 직접 입력하세요. {{reason}}", { reason: typesafeRefusal(error) });
    } finally {
      if (sequence === this.sequence) {
        this.loading = false;
        this.controls();
      }
    }
  },
};
