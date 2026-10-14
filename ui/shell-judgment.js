const judgmentProviderController = {
  initialized: false,
  state: null,
  draft: null,
  drafts: new Map(),
  modelIds: [],
  listedServer: null,
  loading: false,
  sequence: 0,
  localMode: "managed",

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

  providerName(provider) {
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
    if (sameServer && this.draft.requiresAuth && !this.state.keySaved) {
      return ["key", t("settings.judgment.nextKey", "이 서비스의 API 키를 입력하고 키 저장을 누르세요. 키는 이 서비스에만 사용합니다.")];
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
    el("judgment-provider-kind")?.addEventListener("change", () => this.changeKind());
    for (const mode of ["managed", "manual"]) {
      el(`judgment-local-${mode}`)?.addEventListener("click", () => {
        this.localMode = mode;
        this.render();
      });
    }
    for (const name of ["judgment-endpoint", "typesafe-model-input", "judgment-requires-auth"]) {
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
    el("judgment-listed-model")?.replaceChildren();
    if (el("judgment-listed-model-field")) el("judgment-listed-model-field").hidden = true;
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

  connectModel(model) {
    const modelId = managedJudgmentController.serverModel(model);
    if (!modelId || typesafeBusy || managedJudgmentController.busy || managedJudgmentController.operating()) return;
    const kind = this.draft?.kind === "remote" ? "remote" : "local";
    const previous = this.draft?.kind === kind ? this.draft : this.drafts.get(kind);
    const provider = previous ? this.copy(previous)
      : { kind, protocol: "system_one", baseUrl: "", model: "", requiresAuth: kind === "remote" };
    provider.model = modelId;
    el("judgment-connection-settings").open = true;
    this.localMode = "manual";
    this.reset(provider);
    this.render();
    el(provider.baseUrl ? "typesafe-model-input" : "judgment-endpoint")?.focus();
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
    if (!managedJudgmentController.snapshot && !managedJudgmentController.loading && managedJudgmentController.visible()) void managedJudgmentController.refresh();
  },

  controls() {
    const unavailable = !this.draft;
    const dirty = this.dirty();
    const busy = typesafeBusy || unavailable || managedJudgmentController.activating;
    const managed = this.draft?.kind === "local" && this.localMode === "managed";
    const auth = this.draft?.requiresAuth ?? true;
    const active = el("judgment-active-model");
    if (active) {
      active.textContent = this.state?.provider
        ? t("settings.judgment.activeProvider", "현재 적용된 설정: {{provider}} · {{model}}", {
          provider: this.providerName(this.state.provider), model: this.state.provider.model,
        })
        : t("settings.judgment.activeUnknown", "현재 적용된 모델을 읽지 못했습니다. 연결 설정을 다시 확인하세요.");
    }
    const hint = el("judgment-setup-hint");
    if (hint) {
      const [step, words] = this.draft ? this.setupHint() : ["unavailable", ""];
      hint.dataset.step = step;
      hint.textContent = words;
    }
    for (const name of ["judgment-provider-kind", "judgment-endpoint", "typesafe-model-input", "judgment-requires-auth"]) {
      const element = el(name);
      if (element) element.disabled = busy;
    }
    const apply = el("judgment-save-provider");
    if (apply) {
      apply.hidden = managed;
      apply.disabled = busy || !dirty || !this.draft.baseUrl || (!this.draft.model && this.draft.kind !== "jev");
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
      check.disabled = busy || !this.draft?.baseUrl || (!this.draft?.model && this.draft?.kind !== "jev") || (!dirty && this.state?.provider?.requiresAuth && !this.state.keySaved);
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
    let applied = false;
    await runTypeSafe(async () => {
      const check = await this.probe(provider, expectedProviderIdentity, draftIdentity);
      if (!check.answered) throw { kind: "failed", message: typesafeCheckFailure(check.failure) };
      const state = await invoke("set_judgment_provider", { provider, expectedProviderIdentity });
      typesafeChecked = null;
      this.reset(state.provider);
      this.state = state;
      applied = true;
      return state;
    }, () => t("settings.judgment.checkedAndApplied", "응답을 확인한 모델을 적용했습니다. 연결 확인은 작업 정확도 검증과 별개입니다."));
    if (applied && this.state?.providerIdentity === typesafeState?.providerIdentity) {
      typesafeChecked = "connected";
      paintTypeSafe(typesafeState);
    }
  },

  async probe(provider, expectedProviderIdentity, draftIdentity) {
    const check = await invoke("preflight_judgment_provider", { provider, expectedProviderIdentity });
    if (this.state?.providerIdentity !== expectedProviderIdentity || JSON.stringify(this.current()) !== draftIdentity || check.activeProviderIdentity !== expectedProviderIdentity) throw "settings_changed";
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
      el("judgment-model-status").textContent = t("settings.judgment.modelsUnavailable", "목록을 읽지 못했습니다. 서버와 키를 확인하거나 모델 ID를 직접 입력하세요. {{reason}}", { reason: typesafeRefusal(error) });
    } finally {
      if (sequence === this.sequence) {
        this.loading = false;
        this.controls();
      }
    }
  },
};
