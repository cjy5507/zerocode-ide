const managedJudgmentController = {
  initialized: false,
  snapshot: null,
  loading: false,
  busy: false,
  activating: false,
  lastError: null,
  statusError: false,
  rows: new Map(),
  candidates: new Map(),
  waiters: new Set(),
  importModel: null,
  importAttempt: null,
  ownedOperation: null,
  restoreRefresh: null,
  subscription: null,
  query: "",
  filter: "all",
  catalogLimitBytes: 512 * 1024,

  init() {
    if (this.initialized) return;
    this.initialized = true;
    el("managed-judgment-refresh")?.addEventListener("click", () => void this.refresh());
    el("managed-judgment-prepare")?.addEventListener("click", () => void this.perform("managed_judgment_prepare_runtime"));
    el("managed-judgment-cancel")?.addEventListener("click", () => void this.cancel());
    el("managed-judgment-search")?.addEventListener("input", (event) => {
      this.query = event.target.value;
      this.render();
    });
    el("managed-judgment-filter")?.addEventListener("change", (event) => {
      this.filter = event.target.value;
      this.render();
    });
    el("managed-judgment-catalog-file")?.addEventListener("change", () => this.render());
    el("managed-judgment-register-submit")?.addEventListener("click", () => void this.registerCatalog());
    el("managed-judgment-import-submit")?.addEventListener("click", () => {
      const sourceDirectory = el("managed-judgment-import-path").value.trim();
      if (!this.importModel || !sourceDirectory) return;
      void this.perform("managed_judgment_install", { ...this.modelArgs(this.importModel), sourceDirectory });
    });
    el("managed-judgment-import-path")?.addEventListener("input", () => this.sayImport(""));
    // The library's rows and the candidates folded under it carry the same actions.
    el("managed-judgment-panel")?.addEventListener("click", (event) => {
      const button = event.target.closest("button[data-model-action]");
      if (!button || button.disabled) return;
      const model = this.snapshot?.models.find((entry) => entry.id === button.dataset.modelId);
      if (!model) return;
      const action = button.dataset.modelAction;
      if (action === "activate") void this.activate(model);
      else if (action === "prepare") void this.perform("managed_judgment_prepare_model_runtime", this.modelArgs(model));
      else if (action === "install") void this.perform("managed_judgment_install", this.modelArgs(model));
      else if (action === "remove") void this.perform("managed_judgment_remove", this.modelArgs(model), { hand: handNow() });
      else if (action === "stop") void this.perform("managed_judgment_stop", { serverId: this.snapshot.server.id });
      else if (action === "connect") judgmentProviderController.connectModel(model);
      else if (action === "inspect-source") judgmentModelSourceController.inspectAddress(model.source);
      else if (action === "import") {
        this.importModel = model;
        this.sayImport("");
        el("managed-judgment-import").open = true;
        el("managed-judgment-import-model").textContent = model.label;
        el("managed-judgment-import-path").focus();
      }
    });
    this.subscription = listen("managed-judgment:changed", (event) => this.accept(event.payload)).then(() => true).catch(() => {
      this.say(t("settings.managed.eventsUnavailable", "진행 상태를 받을 수 없습니다. 새로고침으로 상태를 확인하세요."));
      return false;
    });
  },

  modelArgs(model) {
    return { modelId: model.id, revision: model.revision };
  },

  operating() {
    return ["running", "cancelling"].includes(this.snapshot?.operation?.state);
  },

  visible() {
    const pane = el("typesafe-card")?.closest(".settings-pane");
    return Boolean(pane && !pane.hidden && !settingsView.hidden && settingsPane === "judgment-models" && !document.hidden);
  },

  say(words) {
    el("managed-judgment-status").textContent = words;
  },

  sayImport(words) {
    el("managed-judgment-import-status").textContent = words;
  },

  importKey() {
    return JSON.stringify([this.importModel?.id, this.importModel?.revision, el("managed-judgment-import-path").value.trim()]);
  },

  failure(error) {
    const token = typeof error === "string" ? error : error?.code ?? error?.kind ?? error?.message;
    if (token === "settings_changed") return judgmentProviderController.failure(token);
    if (token === "cancelled") return t("settings.managed.cancelled", "취소했습니다. 현재 적용된 모델은 바꾸지 않았습니다.");
    switch (token) {
      case "insufficient_space":
        return t("settings.managed.error.space", "저장 공간이 부족합니다. 사용하지 않는 모델을 정리한 뒤 다시 시도하세요.");
      case "runtime_missing":
        return t("settings.managed.error.runtime", "실행 환경을 먼저 설치하세요. 이미 실행 중인 판단 서버를 연결할 수도 있습니다.");
      case "platform_unsupported":
        return t("settings.managed.error.platform", "이 컴퓨터의 직접 실행은 지원하지 않습니다. 호환 판단 서버를 연결하세요.");
      case "engine_not_supported":
        return t("settings.managed.error.engine", "이 모델의 실행 엔진은 아직 지원하지 않습니다. 모델을 실행 중인 System One 서버를 연결하세요.");
      case "engine_unverified":
      case "runtime_unverified":
        return t("settings.managed.error.unverified", "이 모델의 직접 실행은 아직 검증 중입니다. 다운로드가 가능하다는 뜻은 아닙니다.");
      case "integrity_failed":
        return t("settings.managed.error.integrity", "파일이 고정된 모델 버전과 일치하지 않습니다. 올바른 파일을 선택하거나 다시 다운로드하세요.");
      case "download_failed":
        return t("settings.managed.error.download", "다운로드하지 못했습니다. 네트워크와 모델 출처를 확인한 뒤 다시 시도하세요.");
      case "native_protocol_failed":
        return t("settings.managed.error.protocol", "판단 응답 형식이 맞지 않습니다. System One 호환 모델과 서버인지 확인하세요.");
      case "runtime_failed":
        return t("settings.managed.error.start", "모델을 시작하지 못했습니다. 실행 환경과 필요한 메모리를 확인하세요. 현재 모델은 유지됩니다.");
      case "runtime_timeout":
      case "operation_timeout":
        return t("settings.managed.error.timeout", "모델이 제한 시간 안에 응답하지 않았습니다. 더 작은 모델을 선택하거나 서버 상태를 확인하세요.");
      case "model_configured":
        return t("settings.managed.error.configured", "현재 선택된 모델입니다. 다른 모델을 적용한 뒤 이 파일을 삭제하세요.");
      case "model_missing":
        return t("settings.managed.error.missing", "모델 파일이 없습니다. 먼저 다운로드하거나 기존 파일을 등록하세요.");
      case "server_stopped":
        return t("settings.managed.error.stopped", "모델 서버가 중지됐습니다. 다시 실행하고 응답을 확인하세요.");
      case "server_changed":
      case "operation_changed":
      case "manager_instance_changed":
        return t("settings.managed.error.changed", "서버 상태가 바뀌었습니다. 상태를 새로 읽고 다시 연결하세요.");
      case "catalog_changed":
        return t("settings.managed.error.catalogChanged", "모델 목록이 다른 곳에서 바뀌었습니다. 새로고침한 뒤 다시 추가하세요.");
      case "manifest_invalid":
        return t("settings.managed.error.catalog", "지원되는 모델 카탈로그가 아닙니다. 고정 버전·파일 크기·해시가 포함된 JSON 파일을 선택하세요.");
      case "unsafe_path":
        return t("settings.managed.error.path", "안전하지 않은 파일 경로가 포함돼 있습니다. 올바른 모델 카탈로그와 폴더를 선택하세요.");
      case "size_limit":
        return t("settings.managed.error.size", "지원하는 파일 크기 제한을 넘었습니다. 더 작은 모델이나 호환 판단 서버를 선택하세요.");
      case "not_native_decision_model":
        return t("settings.managed.error.notDecision", "이 파일은 지원되는 판단 모델 형식이 아닙니다. 일반 채팅 모델이 아닌 선택·점수·참/거짓 판단 모델을 고르세요.");
      case "model_format_not_supported":
        return t("settings.managed.error.format", "이 모델 형식의 실행 엔진은 아직 지원하지 않습니다. 호환 판단 서버를 연결하거나 지원되는 모델을 선택하세요.");
      case "source_changed":
      case "inspection_expired":
      case "source_inspection_changed":
        return t("settings.managed.error.sourceChanged", "확인한 뒤 모델 파일이나 출처가 바뀌었습니다. 모델 확인부터 다시 진행하세요.");
      case "source_invalid":
        return t("settings.managed.error.source", "Hugging Face 모델 주소나 저장소 ID를 확인하세요. 로컬 모델은 설정과 가중치 파일이 함께 있는 폴더를 선택하세요.");
      case "source_unavailable":
        return t("settings.managed.error.sourceUnavailable", "모델 출처에 연결할 수 없습니다. 주소·모델 버전·네트워크를 확인하거나 이미 받은 로컬 폴더를 선택하세요.");
      case "source_auth_required":
        return t("settings.managed.error.sourceAuth", "로그인이나 이용 동의가 필요한 모델입니다. 접근 가능한 공개 모델을 고르거나, 이용 권한으로 직접 받은 로컬 폴더를 선택하세요.");
      case "source_hash_unavailable":
        return t("settings.managed.error.sourceHash", "출처에서 파일 검증 정보를 확인하지 못했습니다. 검증 가능한 모델 버전을 고르거나 직접 받은 로컬 폴더를 선택하세요.");
      case "source_folder_required":
        return t("settings.managed.error.sourceFolder", "파일 하나가 아닌 모델 폴더를 선택하세요. 모델 설정과 가중치 파일이 함께 있어야 합니다.");
      case "destination_unavailable":
        return t("settings.managed.error.destination", "선택한 저장 폴더를 사용할 수 없습니다. 기본 저장 위치로 돌아가거나 쓰기 가능한 다른 폴더를 선택하세요.");
      case "weights_format_invalid":
        return t("settings.managed.error.weightsFormat", "가중치 파일을 읽을 수 없거나 파일 구조가 올바르지 않습니다. 파일을 다시 받거나 완전한 모델 폴더를 선택하세요.");
      case "catalog_conflict":
        return t("settings.managed.error.catalogConflict", "같은 모델 ID가 다른 정보로 이미 등록돼 있습니다. 모델 목록을 새로 읽고 등록된 모델과 출처를 확인하세요.");
      case "model_manifest_missing":
        return t("settings.managed.error.modelManifest", "등록된 모델 정보를 찾지 못했습니다. 모델 목록을 새로 읽거나 원래 출처·폴더에서 다시 추가하세요.");
      case "model_restore_failed":
        return t("settings.managed.error.restore", "새 모델로 전환하지 못했고 이전 모델의 자동 복구도 실패했습니다. 이전 모델 카드에서 다시 사용을 눌러 복구하세요.");
      case "operation_busy":
        return t("settings.managed.error.busy", "다른 모델 작업이 진행 중입니다. 완료하거나 취소한 뒤 다시 시도하세요.");
    }
    return t("settings.managed.failed", "작업 결과를 확인하지 못했습니다. 상태를 새로 읽어 실제 설치·실행·적용 상태를 확인하세요.");
  },

  localSupported(model) {
    return model.local ? model.local.supported === true : model.compatibility === "supported";
  },

  downloadOnly(model) {
    return model.local?.supported !== true && model.local?.downloadOnly === true;
  },

  runtimeFor(model) {
    return model.runtime ?? this.snapshot?.runtime;
  },

  inputSupport(model) {
    const words = { text: t("settings.models.text", "텍스트"), json: t("settings.models.json", "JSON"), images: t("settings.models.images", "이미지"), videos: t("settings.models.videos", "영상") };
    const inputs = model.local?.inputs?.map((input) => words[input]).filter(Boolean).join(", ");
    const unsupported = model.local?.unsupportedInputs?.map((input) => words[input]).filter(Boolean).join(", ");
    return [
      inputs ? t("settings.models.inputs", "입력: {{inputs}}", { inputs }) : "",
      unsupported ? t("settings.models.unsupportedInputs", "미지원: {{inputs}}", { inputs: unsupported }) : "",
    ].filter(Boolean).join(" · ");
  },

  serverModel(model) {
    const server = model.existingServer;
    return server?.supported && server.protocol === "system_one" && typeof server.modelId === "string" && server.modelId.trim()
      ? server.modelId : null;
  },

  // The applied setting is a server of the person's own that answers with this
  // model — not the managed server, whose model is held by its own files.
  // Entries sharing a server model ID (a bundled model and the address the
  // person checked for it) are one connection, so one of them counts: the
  // entry the setting is bound to, else the one received here, else the first.
  connected(model) {
    const snapshot = this.snapshot;
    const modelId = this.serverModel(model);
    if (!this.connectionMatches(model)) return false;
    const same = snapshot.models.filter((entry) => this.serverModel(entry) === modelId);
    const bound = [snapshot.configuredModelId, snapshot.appliedModelId];
    const answering = same.find((entry) => bound.includes(entry.id)) ?? same.find((entry) => entry.installed) ?? same[0];
    return answering?.id === model.id;
  },

  connectionMatches(model) {
    const provider = judgmentProviderController.state?.provider;
    const modelId = this.serverModel(model);
    return Boolean(this.snapshot && modelId && provider && provider.kind !== "jev" && provider.protocol === "system_one" &&
      provider.model === modelId && !judgmentProviderController.managedServer());
  },

  // A row of the library is a model this computer holds (t-69998): its files
  // were received here, the setting or a live managed server is bound to it,
  // or a connected server answers with it. Every other catalog model is only
  // a candidate — a stopped server's last model is not held by it.
  held(model) {
    const snapshot = this.snapshot;
    return model.installed || [snapshot.configuredModelId, snapshot.appliedModelId].includes(model.id) ||
      (snapshot.server.state !== "stopped" && snapshot.server.modelId === model.id) || this.connected(model);
  },

  // The ways to have a model not received yet, in the order its buttons stand.
  ways(model) {
    const local = this.localSupported(model);
    return {
      install: local,
      "inspect-source": this.downloadOnly(model) && Boolean(this.sourceHref(model)),
      connect: Boolean(this.serverModel(model)),
      import: local,
    };
  },

  sourceHref(model) {
    try {
      const source = new URL(model.source);
      return source.protocol === "https:" && !source.username && !source.password ? source.href : null;
    } catch {
      return null;
    }
  },

  sizeLabel(model) {
    return bytesLabel(model.weightsBytes ?? model.downloadBytes);
  },

  matches(model) {
    const query = this.query.trim().toLocaleLowerCase();
    if (query && ![model.label, model.id, model.engine, model.serverModelId, model.license].some((value) => String(value ?? "").toLocaleLowerCase().includes(query))) return false;
    if (this.filter === "local") return this.localSupported(model);
    if (this.filter === "server") return Boolean(this.serverModel(model));
    if (this.filter === "installed") return model.installed;
    return true;
  },

  async registerCatalog() {
    const file = el("managed-judgment-catalog-file")?.files?.[0];
    if (!file || this.busy || this.operating() || !this.snapshot?.catalogFingerprint) return;
    if (file.size > this.catalogLimitBytes) {
      this.say(t("settings.managed.catalogTooLarge", "모델 카탈로그는 512 KiB 이하의 JSON 파일이어야 합니다."));
      return;
    }
    const { instanceId, catalogFingerprint } = this.snapshot;
    this.busy = true;
    this.render();
    try {
      const catalogJson = await file.text();
      const snapshot = await invoke("managed_judgment_register_catalog", { instanceId, expectedCatalogFingerprint: catalogFingerprint, catalogJson });
      if (!this.accept(snapshot)) throw new Error("invalid_snapshot");
      el("managed-judgment-catalog-file").value = "";
      this.say(t("settings.managed.catalogAdded", "모델 목록을 추가했습니다. 파일 다운로드와 모델 적용은 아직 하지 않았습니다."));
    } catch (error) {
      this.say(this.failure(error));
    } finally {
      this.busy = false;
      if (this.visible()) this.render();
    }
  },

  accept(snapshot, fresh = false) {
    if (!snapshot || !Number.isSafeInteger(snapshot.revision) || !snapshot.instanceId || !Array.isArray(snapshot.models) || !snapshot.runtime || !snapshot.server) return false;
    if (this.snapshot) {
      if (snapshot.instanceId !== this.snapshot.instanceId && !fresh) return false;
      if (snapshot.instanceId === this.snapshot.instanceId && snapshot.revision < this.snapshot.revision) return false;
    }
    this.snapshot = snapshot;
    this.statusError = false;
    judgmentModelSourceController.cancelOwnInspection();
    if (this.importAttempt && !this.importAttempt.operationId && this.busy &&
        snapshot.operation?.id !== this.importAttempt.previousId && snapshot.operation?.kind === "download" && snapshot.operation?.modelId === this.importModel?.id) {
      this.importAttempt.operationId = snapshot.operation.id;
    }
    for (const notify of this.waiters) notify();
    if (this.visible()) this.render();
    if (snapshot.operation?.kind === "restore" && snapshot.operation.state === "done" && snapshot.providerIdentity &&
        snapshot.providerIdentity !== judgmentProviderController.state?.providerIdentity) {
      const restored = `${snapshot.instanceId}:${snapshot.operation.id}`;
      if (this.restoreRefresh !== restored) {
        this.restoreRefresh = restored;
        void refreshTypeSafe();
      }
    }
    return true;
  },

  async refresh() {
    if (!this.visible()) return;
    await refreshTypeSafe();
  },

  // `hand` is a person-only command's (shell-boot.js `handNow`, t-75209).
  async perform(command, args = {}, { hand } = {}) {
    if (this.busy || this.operating() || typesafeBusy || judgmentModelSourceController.busy) return;
    this.busy = true;
    const importing = typeof args.sourceDirectory === "string";
    if (importing) {
      this.importAttempt = { key: this.importKey(), operationId: null, previousId: this.snapshot?.operation?.id };
      this.sayImport("");
    }
    this.render();
    this.say("");
    try {
      if (!this.accept(await invoke(command, args, { hand }))) throw new Error("invalid_snapshot");
    } catch (error) {
      if (importing) {
        if (this.importAttempt.key === this.importKey()) this.sayImport(this.failure(error));
      } else this.say(this.failure(error));
    } finally {
      this.busy = false;
      if (this.visible()) this.render();
    }
  },

  async cancel() {
    const operation = this.snapshot?.operation;
    if (!operation || operation.state !== "running") return;
    try {
      this.accept(await invoke("managed_judgment_cancel", { operationId: operation.id }));
    } catch (error) {
      this.say(this.failure(error));
    }
  },

  owns(operation) {
    const owned = this.ownedOperation;
    return Boolean(owned && operation && operation.id === owned.id && this.snapshot?.instanceId === owned.instanceId);
  },

  async cancelOwned() {
    const operation = this.snapshot?.operation;
    if (!this.owns(operation) || operation.state !== "running") return;
    try {
      this.accept(await invoke("managed_judgment_cancel", { operationId: operation.id }));
    } catch (error) {
      this.say(this.failure(error));
    }
  },

  async completed(command, args) {
    this.ownedOperation = null;
    const started = await invoke(command, args);
    if (started?.operation) this.ownedOperation = { instanceId: started.instanceId, id: started.operation.id };
    this.accept(started);
    const operation = started.operation;
    if (!operation) throw new Error("missing_operation");
    await new Promise((resolve, reject) => {
      let timeout;
      let progress = "";
      const finish = (error) => {
        clearTimeout(timeout);
        this.waiters.delete(observe);
        if (error) reject(error); else resolve();
      };
      const observe = () => {
        const current = this.snapshot?.operation;
        if (this.snapshot?.instanceId !== started.instanceId || current?.id !== operation.id) return finish(new Error("operation_changed"));
        if (current.state === "done") finish();
        else if (current.state === "failed" || current.state === "cancelled") finish(current.error ?? current.state);
        else {
          const next = JSON.stringify([current.state, current.receivedBytes, current.totalBytes]);
          if (next !== progress) {
            progress = next;
            clearTimeout(timeout);
            timeout = setTimeout(() => finish(new Error("operation_timeout")), 180_000);
          }
        }
      };
      this.waiters.add(observe);
      observe();
    });
  },

  async activate(model, source = null) {
    this.lastError = null;
    if (this.busy || this.operating() || typesafeBusy || judgmentModelSourceController.busy || !this.localSupported(model)) return false;
    const expectedProviderIdentity = this.snapshot.providerIdentity;
    if (!expectedProviderIdentity) return false;
    const previousServerId = this.snapshot.server.id;
    const instanceId = this.snapshot.instanceId;
    this.busy = true;
    this.activating = true;
    this.say("");
    this.render();
    judgmentProviderController.controls();
    try {
      if (!(await this.subscription)) throw new Error("events_unavailable");
      if (source) {
        await this.completed(source.command, source.args);
        model = this.snapshot.models.find((entry) => entry.id === model.id && entry.revision === model.revision);
        if (!model?.installed) throw "model_missing";
      }
      if (this.runtimeFor(model)?.state === "missing") {
        await this.completed(model.runtime ? "managed_judgment_prepare_model_runtime" : "managed_judgment_prepare_runtime", model.runtime ? this.modelArgs(model) : {});
        model = this.snapshot.models.find((entry) => entry.id === model.id && entry.revision === model.revision);
      }
      if (!model || this.runtimeFor(model)?.state !== "ready") throw "runtime_missing";
      if (!model.installed) {
        await this.completed("managed_judgment_install", this.modelArgs(model));
        model = this.snapshot.models.find((entry) => entry.id === model.id && entry.revision === model.revision);
        if (!model?.installed) throw "model_missing";
      }
      if (this.snapshot.instanceId !== instanceId) throw "server_changed";
      if (this.snapshot.server.state === "ready" && this.snapshot.server.modelId !== model.id) {
        if (!previousServerId || this.snapshot.server.id !== previousServerId) throw "server_changed";
        await this.completed("managed_judgment_switch_model", { ...this.modelArgs(model), serverId: previousServerId, expectedProviderIdentity, instanceId });
      } else {
        if (this.snapshot.server.state !== "ready" || this.snapshot.server.modelId !== model.id) {
          await this.completed("managed_judgment_start", this.modelArgs(model));
        }
        const serverId = this.snapshot.server.id;
        if (!serverId || this.snapshot.server.state !== "ready" || this.snapshot.server.modelId !== model.id) throw new Error("server_not_ready");
        await this.completed("managed_judgment_check", { serverId });
        if (this.snapshot.server.id !== serverId || !this.snapshot.server.checked) throw new Error("server_changed");
        this.accept(await invoke("managed_judgment_apply", { serverId, expectedProviderIdentity }));
      }
      if (this.snapshot.appliedModelId !== model.id) throw new Error("model_not_applied");
      await judgmentProviderController.refreshAfterApply();
      this.say(t("settings.managed.applied", "로컬 모델을 적용했습니다. 응답 확인은 작업 정확도 검증과 별개입니다."));
      return true;
    } catch (error) {
      this.lastError = error;
      this.say(this.failure(error));
      return false;
    } finally {
      this.busy = false;
      this.activating = false;
      judgmentProviderController.controls();
      if (this.visible()) this.render();
    }
  },

  makeActions(model, actions) {
    const row = document.createElement("div");
    row.className = "settings-action-row";
    const buttons = {};
    for (const action of actions) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "btn";
      button.dataset.modelAction = action;
      button.dataset.modelId = model.id;
      buttons[action] = button;
      row.append(button);
    }
    return { row, buttons };
  },

  actionWords(action, model) {
    switch (action) {
      case "prepare": return t("settings.managed.prepare", "실행 환경 설치");
      case "install": return t("settings.managed.download", "다운로드");
      case "activate": return model.installed ? t("settings.managed.activate", "실행하고 적용") : t("settings.models.installUse", "설치하고 사용");
      case "inspect-source": return t("settings.managed.inspectSource", "주소로 확인");
      case "connect": return this.connectionMatches(model)
        ? t("settings.managed.alreadyConnected", "이미 연결됨")
        : t("settings.managed.connectServer", "이 모델의 서버 연결");
      case "import": return t("settings.managed.import", "기존 파일 등록");
      case "stop": return t("settings.managed.stop", "중지");
    }
    return t("settings.managed.remove", "다운로드 파일 삭제");
  },

  makeRow(model) {
    const card = document.createElement("article");
    card.className = "managed-judgment-model";
    card.dataset.modelId = model.id;
    const title = document.createElement("h4");
    const heading = document.createElement("div");
    heading.className = "managed-judgment-model-heading";
    const route = document.createElement("span");
    route.className = "managed-judgment-route";
    heading.append(title, route);
    const facts = document.createElement("p");
    facts.className = "settings-row-desc";
    const state = document.createElement("p");
    state.className = "settings-row-desc";
    const source = document.createElement("a");
    source.className = "settings-jev-link";
    source.target = "_blank";
    source.rel = "noopener noreferrer";
    const { row: actions, buttons } = this.makeActions(model, ["prepare", "install", "activate", "inspect-source", "connect", "import", "stop", "remove"]);
    const details = document.createElement("details");
    details.className = "settings-fold";
    const summary = document.createElement("summary");
    const identity = document.createElement("p");
    identity.className = "settings-row-desc";
    details.append(summary, identity, source);
    card.append(heading, facts, state, actions, details);
    return { card, title, route, facts, source, state, summary, identity, buttons, signature: null };
  },

  // A candidate is one line: its name, its size and the ways to have it — no
  // revision or folder, which only received files can claim.
  makeCandidate(model) {
    const card = document.createElement("li");
    card.className = "managed-judgment-candidate";
    card.dataset.modelId = model.id;
    const facts = document.createElement("p");
    facts.className = "managed-judgment-candidate-facts";
    const name = document.createElement("span");
    name.className = "managed-judgment-candidate-name";
    const size = document.createElement("span");
    const reason = document.createElement("span");
    reason.className = "settings-row-desc managed-judgment-candidate-reason";
    facts.append(name, " ", size, reason);
    const { row: actions, buttons } = this.makeActions(model, Object.keys(this.ways(model)));
    card.append(facts, actions);
    return { card, name, size, reason, buttons, signature: null };
  },

  prune(entries, kept) {
    for (const [modelId, entry] of entries) {
      if (kept.has(modelId)) continue;
      if (entry.card.contains(document.activeElement)) el("managed-judgment-refresh").focus();
      entry.card.remove();
      entries.delete(modelId);
    }
  },

  modelState(model) {
    const snapshot = this.snapshot;
    const operation = snapshot.operation;
    if (operation?.kind === "restore" && operation.modelId === model.id) {
      if (["running", "cancelling"].includes(operation.state)) return t("settings.managed.restoring", "이전에 실행하던 모델을 복원하는 중…");
      if (operation.state === "failed") return t("settings.managed.restoreFailed", "복원 실패 · {{reason}}", { reason: this.failure(operation.error) });
    }
    if (this.appliedModel(model)) return t("settings.managed.inUse", "실행 중 · 현재 적용됨");
    if (this.connected(model)) return t("settings.managed.connectedApplied", "기존 판단 서버로 연결됨 · 현재 적용됨");
    if ((this.statusError || snapshot.providerIdentity !== judgmentProviderController.state?.providerIdentity) &&
        [snapshot.appliedModelId, snapshot.configuredModelId].includes(model.id)) return t("settings.judgment.modelStateUnknown", "현재 설정과 실행 상태를 함께 확인해야 합니다.");
    const restarting = snapshot.server.modelId === model.id && snapshot.server.state === "starting" && snapshot.server.exit ? this.endedState(snapshot.server.exit, true) : null;
    if (restarting) return restarting;
    if (snapshot.server.modelId === model.id && snapshot.server.state === "starting") return t("settings.managed.starting", "모델을 시작하는 중 · 아직 적용하지 않음");
    if (snapshot.server.modelId === model.id && snapshot.server.state === "ready") return t("settings.managed.runningUnapplied", "실행 중 · 아직 적용하지 않음");
    const stopped = snapshot.server.modelId === model.id && snapshot.server.state === "failed" && snapshot.server.exit ? this.endedState(snapshot.server.exit) : null;
    if (stopped) return stopped;
    if (this.configuredModel(model)) return t("settings.managed.configuredStopped", "선택된 모델 · 실행되지 않음. 실행하고 적용을 눌러 다시 연결하세요.");
    if (this.downloadOnly(model)) {
      return model.installed
        ? t("settings.managed.filesReady", "파일 준비됨 · 이 컴퓨터의 직접 실행은 아직 검증되지 않았습니다. 현재 모델은 바뀌지 않습니다.")
        : t("settings.managed.downloadOnlyState", "이 컴퓨터의 직접 실행은 아직 검증 전입니다. 모델 주소로 확인하면 파일만 받아 둘 수 있습니다.");
    }
    if (!this.localSupported(model)) return this.failure(model.local?.reason ?? model.compatibilityReason ?? "platform_unsupported");
    if (model.installed && this.runtimeFor(model)?.state !== "ready") return t("settings.managed.needsRuntime", "모델 파일 설치됨 · 이 모델의 실행 환경을 설치한 뒤 연결하세요.");
    return model.installed ? t("settings.managed.installed", "설치됨 · 아직 적용하지 않음") : t("settings.managed.notInstalled", "아직 다운로드하지 않음");
  },

  endedState(exit, restarting = false) {
    let reason = null;
    switch (exit.cause) {
      case "runtime_memory_limit":
        reason = t("settings.managed.exit.memory", "메모리 한도를 넘어 실행 환경이 스스로 멈췄습니다");
        break;
      case "runtime_timeout":
        reason = exit.task
          ? t("settings.managed.exit.timeoutTask", "{{task}} 요청이 제한 시간을 넘겨 서버를 멈췄습니다", { task: exit.task })
          : t("settings.managed.exit.timeout", "요청이 제한 시간을 넘겨 서버를 멈췄습니다");
        break;
      case "runtime_failed":
        reason = t("settings.managed.exit.failed", "실행 환경과 주고받지 못해 서버를 멈췄습니다");
        break;
      case "runtime_exited":
        if (Number.isSafeInteger(exit.signal)) reason = t("settings.managed.exit.signal", "실행 환경이 끝났습니다(신호 {{signal}})", { signal: exit.signal });
        else if (Number.isSafeInteger(exit.code)) reason = t("settings.managed.exit.code", "실행 환경이 스스로 끝났습니다(종료 코드 {{code}})", { code: exit.code });
        break;
    }
    if (!reason) return null;
    return restarting
      ? t("settings.managed.restartingReason", "다시 띄우는 중 · {{reason}}. 중지를 누르면 다시 띄우지 않습니다.", { reason })
      : t("settings.managed.stoppedReason", "중지됨 · {{reason}}. 실행하고 적용을 눌러 다시 시작하세요.", { reason });
  },

  configuredModel(model) {
    const state = judgmentProviderController.state;
    return this.snapshot?.providerIdentity === state?.providerIdentity && state?.provider?.kind === "local" &&
      this.snapshot.configuredModelId === model.id && state.provider.model === model.id;
  },

  appliedModel(model) {
    const server = judgmentProviderController.managedServer();
    return this.snapshot?.appliedModelId === model.id && server?.modelId === model.id && server.state === "ready" && server.checked === true;
  },

  render() {
    judgmentModelSourceController.render();
    judgmentProviderController.renderStatus();
    const snapshot = this.snapshot;
    const busy = this.busy || this.operating() || typesafeBusy || judgmentModelSourceController.busy;
    if (document.activeElement !== el("managed-judgment-search")) el("managed-judgment-search").value = this.query;
    el("managed-judgment-filter").value = this.filter;
    el("managed-judgment-register").hidden = !snapshot?.catalogFingerprint;
    el("managed-judgment-catalog-file").disabled = busy;
    el("managed-judgment-register-submit").disabled = busy || !snapshot?.catalogFingerprint || !el("managed-judgment-catalog-file").files?.length;
    el("managed-judgment-refresh").disabled = this.loading;
    el("managed-judgment-prepare").disabled = busy || !snapshot || snapshot.runtime.state !== "missing";
    el("managed-judgment-import-submit").disabled = busy || !this.importModel;
    const runtime = el("managed-judgment-runtime");
    const modelRuntimes = snapshot?.models.length > 0 && snapshot.models.every((model) => model.runtime);
    runtime.hidden = modelRuntimes;
    el("managed-judgment-prepare").hidden = modelRuntimes;
    runtime.textContent = !snapshot
      ? t("settings.managed.loading", "로컬 모델 상태를 읽는 중…")
      : snapshot.runtime.state === "ready"
        ? t("settings.managed.runtimeReady", "실행 환경이 준비됐습니다. 모델을 내려받거나 기존 파일을 등록하세요.")
        : snapshot.runtime.state === "unsupported"
          ? t("settings.managed.unsupported", "이 컴퓨터에서 자동 설치를 지원하지 않습니다. 기존 판단 서버 연결을 사용할 수 있습니다.")
          : t("settings.managed.runtimeMissing", "처음 한 번 실행 환경을 설치하세요. 시스템 Python이나 다른 앱의 설정은 바꾸지 않습니다.");
    const operation = snapshot?.operation;
    const progress = el("managed-judgment-progress");
    progress.hidden = !operation || !["running", "cancelling"].includes(operation.state);
    const meter = progress.querySelector("progress");
    if (operation && operation.totalBytes > 0) {
      meter.max = operation.totalBytes;
      meter.value = Math.min(operation.receivedBytes, operation.totalBytes);
    } else {
      meter.removeAttribute("value");
    }
    el("managed-judgment-progress-label").textContent = operation?.state === "cancelling"
      ? t("settings.managed.cancelling", "취소하고 임시 파일을 정리하는 중…")
      : operation?.kind === "restore"
        ? t("settings.managed.restoreWorking", "이전에 실행하던 판단 모델을 복원하는 중…")
        : operation && operation.totalBytes > 0
        ? t("settings.managed.progress", "{{received}} / {{total}}", { received: bytesLabel(operation.receivedBytes), total: bytesLabel(operation.totalBytes) })
        : t("settings.managed.working", "작업 중… 완료될 때까지 현재 모델을 유지합니다.");
    el("managed-judgment-cancel").disabled = operation?.state !== "running";
    if (["failed", "cancelled"].includes(operation?.state) && !this.busy && operation.kind !== "inspect") {
      const reason = this.failure(operation.state === "cancelled" ? "cancelled" : operation.error);
      const words = operation.kind === "restore" && operation.state === "failed"
        ? t("settings.managed.restoreFailed", "복원 실패 · {{reason}}", { reason }) : reason;
      if (operation.id === this.importAttempt?.operationId) {
        if (this.importAttempt.key === this.importKey()) this.sayImport(words);
      } else this.say(words);
    }
    const catalog = snapshot?.models ?? [];
    const held = catalog.filter((model) => this.held(model));
    const offered = catalog.filter((model) => !held.includes(model) && Object.values(this.ways(model)).some(Boolean));
    const models = el("managed-judgment-models");
    const kept = new Set();
    let shown = 0;
    for (const model of held) {
      kept.add(model.id);
      let row = this.rows.get(model.id);
      if (!row) {
        row = this.makeRow(model);
        this.rows.set(model.id, row);
        models.append(row.card);
      }
      row.card.hidden = !this.matches(model);
      if (!row.card.hidden) shown += 1;
      const running = snapshot.server.modelId === model.id && snapshot.server.state !== "stopped" && snapshot.server.state !== "failed";
      const applied = this.appliedModel(model);
      const configured = snapshot.configuredModelId === model.id;
      const connected = this.connected(model);
      const restoring = operation?.kind === "restore" && operation.modelId === model.id && ["running", "cancelling"].includes(operation.state);
      const signature = JSON.stringify([model, busy, running, applied, configured, connected, snapshot.server.state, snapshot.server.exit, snapshot.runtime.state,
        operation?.kind, operation?.state, operation?.error, snapshot.providerIdentity, judgmentProviderController.state?.providerIdentity, this.statusError, t("settings.managed.download", "다운로드")]);
      if (row.signature === signature) continue;
      row.signature = signature;
      row.title.textContent = model.label;
      const local = this.localSupported(model);
      const ways = this.ways(model);
      row.route.textContent = local
        ? t("settings.managed.filterLocal", "이 컴퓨터에서 직접 실행")
        : ways.connect ? t("settings.managed.filterServer", "기존 판단 서버로 연결")
          : t("settings.managed.routeUnavailable", "실행 방법 확인 필요");
      row.facts.textContent = [`${this.sizeLabel(model)} · ${model.license}`, this.inputSupport(model)].filter(Boolean).join(" · ");
      // A pinned revision is a fact of files received here; a connected server
      // answers with whatever revision it runs.
      row.summary.textContent = model.installed ? t("settings.managed.details", "모델 출처와 고정 버전") : t("settings.managed.detailsSource", "모델 출처");
      row.identity.textContent = [model.serverModelId ?? model.id, model.engine, model.installed ? model.revision : null, model.installedDirectory].filter(Boolean).join(" · ");
      row.source.textContent = t("settings.managed.source", "모델 출처 보기");
      const href = this.sourceHref(model);
      row.source.hidden = !href;
      if (href) row.source.href = href;
      else row.source.removeAttribute("href");
      row.state.textContent = this.modelState(model);
      for (const [action, button] of Object.entries(row.buttons)) {
        button.textContent = this.actionWords(action, model);
        button.disabled = busy;
      }
      row.buttons.prepare.hidden = !local || !model.runtime || this.runtimeFor(model)?.state !== "missing";
      row.buttons.install.hidden = model.installed || !ways.install;
      row.buttons.import.hidden = model.installed || !ways.import;
      row.buttons.activate.hidden = !local;
      row.buttons.connect.hidden = !ways.connect;
      row.buttons.connect.disabled ||= this.connectionMatches(model);
      row.buttons["inspect-source"].hidden = model.installed || !ways["inspect-source"];
      row.buttons.activate.disabled ||= applied || !["ready", "missing"].includes(this.runtimeFor(model)?.state);
      row.buttons.stop.hidden = !running || restoring;
      row.buttons.remove.hidden = !model.installed;
      row.buttons.remove.disabled ||= configured || running;
    }
    this.prune(this.rows, kept);
    const list = el("managed-judgment-candidate-list");
    const waiting = new Set();
    let offeredShown = 0;
    for (const model of offered) {
      waiting.add(model.id);
      let item = this.candidates.get(model.id);
      if (!item) {
        item = this.makeCandidate(model);
        this.candidates.set(model.id, item);
        list.append(item.card);
      }
      item.card.hidden = !this.matches(model);
      if (!item.card.hidden) offeredShown += 1;
      const connected = this.connectionMatches(model);
      const signature = JSON.stringify([model, busy, connected, t("settings.managed.download", "다운로드")]);
      if (item.signature === signature) continue;
      item.signature = signature;
      item.name.textContent = model.label;
      item.size.textContent = this.sizeLabel(model);
      item.reason.hidden = !this.downloadOnly(model);
      item.reason.textContent = this.downloadOnly(model) ? this.modelState(model) : "";
      const ways = this.ways(model);
      for (const [action, button] of Object.entries(item.buttons)) {
        button.textContent = this.actionWords(action, model);
        button.disabled = busy;
        button.hidden = !ways[action];
      }
      item.buttons.connect.disabled ||= connected;
    }
    this.prune(this.candidates, waiting);
    // Search covers both the received library and the candidates, including
    // a first visit before any model has been downloaded.
    const tools = el("managed-judgment-library-tools");
    if (catalog.length === 0 && tools.contains(document.activeElement)) el("managed-judgment-refresh").focus();
    tools.hidden = catalog.length === 0;
    el("managed-judgment-library-summary").hidden = held.length === 0;
    el("managed-judgment-library-summary").textContent = snapshot && held.length > 0
      ? t("settings.managed.libraryCount", "모델 {{shown}}개 표시 · 전체 {{total}}개", { shown, total: held.length }) : "";
    el("managed-judgment-no-matches").hidden = !snapshot || catalog.length === 0 || shown + offeredShown > 0;
    el("managed-judgment-none").hidden = !snapshot || held.length > 0 || offered.length === 0;
    el("managed-judgment-empty").hidden = !snapshot || held.length > 0 || offered.length > 0;
    el("managed-judgment-candidates").hidden = offeredShown === 0;
    if (this.query.trim() && offeredShown > 0) el("managed-judgment-candidates").open = true;
    el("managed-judgment-candidates-count").textContent = t("settings.managed.availableCount", "받을 수 있는 모델 {{count}}개", { count: offeredShown });
  },
};
