const managedJudgmentController = {
  initialized: false,
  snapshot: null,
  loading: false,
  busy: false,
  activating: false,
  fetchSequence: 0,
  rows: new Map(),
  waiters: new Set(),
  importModel: null,
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
    el("managed-judgment-models")?.addEventListener("click", (event) => {
      const button = event.target.closest("button[data-model-action]");
      if (!button || button.disabled) return;
      const model = this.snapshot?.models.find((entry) => entry.id === button.dataset.modelId);
      if (!model) return;
      const action = button.dataset.modelAction;
      if (action === "activate") void this.activate(model);
      else if (action === "prepare") void this.perform("managed_judgment_prepare_model_runtime", this.modelArgs(model));
      else if (action === "install") void this.perform("managed_judgment_install", this.modelArgs(model));
      else if (action === "remove") void this.perform("managed_judgment_remove", this.modelArgs(model));
      else if (action === "stop") void this.perform("managed_judgment_stop", { serverId: this.snapshot.server.id });
      else if (action === "connect") judgmentProviderController.connectModel(model);
      else if (action === "import") {
        this.importModel = model;
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
    return !settingsView.hidden && settingsPane === "judgment-models" && !el("managed-judgment-panel").hidden;
  },

  say(words) {
    el("managed-judgment-status").textContent = words;
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
    for (const notify of this.waiters) notify();
    if (this.visible()) this.render();
    return true;
  },

  async refresh() {
    this.init();
    if (this.loading) return;
    const sequence = ++this.fetchSequence;
    this.loading = true;
    this.render();
    try {
      const snapshot = await invoke("managed_judgment_status");
      if (sequence === this.fetchSequence && !this.accept(snapshot, true)) throw new Error("invalid_snapshot");
      this.say("");
    } catch (error) {
      this.say(this.failure(error));
    } finally {
      this.loading = false;
      if (this.visible()) this.render();
    }
  },

  async perform(command, args = {}) {
    if (this.busy || this.operating() || typesafeBusy || judgmentModelSourceController.busy) return;
    this.busy = true;
    this.render();
    this.say("");
    try {
      if (!this.accept(await invoke(command, args))) throw new Error("invalid_snapshot");
    } catch (error) {
      this.say(this.failure(error));
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

  async completed(command, args) {
    const started = await invoke(command, args);
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
      judgmentProviderController.draft = null;
      await refreshTypeSafe();
      this.say(t("settings.managed.applied", "로컬 모델을 적용했습니다. 응답 확인은 작업 정확도 검증과 별개입니다."));
      return true;
    } catch (error) {
      this.say(this.failure(error));
      return false;
    } finally {
      this.busy = false;
      this.activating = false;
      judgmentProviderController.controls();
      if (this.visible()) this.render();
    }
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
    const actions = document.createElement("div");
    actions.className = "settings-action-row";
    const details = document.createElement("details");
    details.className = "settings-fold";
    const summary = document.createElement("summary");
    const identity = document.createElement("p");
    identity.className = "settings-row-desc";
    details.append(summary, identity, source);
    const buttons = {};
    for (const action of ["prepare", "install", "activate", "connect", "import", "stop", "remove"]) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "btn";
      button.dataset.modelAction = action;
      button.dataset.modelId = model.id;
      buttons[action] = button;
      actions.append(button);
    }
    card.append(heading, facts, state, actions, details);
    return { card, title, route, facts, source, state, summary, identity, buttons, signature: null };
  },

  modelState(model) {
    const snapshot = this.snapshot;
    if (snapshot.appliedModelId === model.id) return t("settings.managed.inUse", "실행 중 · 현재 적용됨");
    if (snapshot.server.modelId === model.id && snapshot.server.state === "starting") return t("settings.managed.starting", "모델을 시작하는 중 · 아직 적용하지 않음");
    if (snapshot.server.modelId === model.id && snapshot.server.state === "ready") return t("settings.managed.runningUnapplied", "실행 중 · 아직 적용하지 않음");
    if (snapshot.configuredModelId === model.id) return t("settings.managed.configuredStopped", "선택된 모델 · 실행되지 않음. 실행하고 적용을 눌러 다시 연결하세요.");
    if (!this.localSupported(model)) return this.failure(model.local?.reason ?? model.compatibilityReason ?? "platform_unsupported");
    if (model.installed && this.runtimeFor(model)?.state !== "ready") return t("settings.managed.needsRuntime", "모델 파일 설치됨 · 이 모델의 실행 환경을 설치한 뒤 연결하세요.");
    return model.installed ? t("settings.managed.installed", "설치됨 · 아직 적용하지 않음") : t("settings.managed.notInstalled", "아직 다운로드하지 않음");
  },

  render() {
    judgmentModelSourceController.render();
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
      : operation && operation.totalBytes > 0
        ? t("settings.managed.progress", "{{received}} / {{total}}", { received: bytesLabel(operation.receivedBytes), total: bytesLabel(operation.totalBytes) })
        : t("settings.managed.working", "작업 중… 완료될 때까지 현재 모델을 유지합니다.");
    el("managed-judgment-cancel").disabled = operation?.state !== "running";
    if (operation?.state === "failed" && !this.busy) this.say(this.failure(operation.error));
    if (operation?.state === "cancelled" && !this.busy) this.say(this.failure("cancelled"));
    const models = el("managed-judgment-models");
    const kept = new Set();
    let shown = 0;
    for (const model of snapshot?.models ?? []) {
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
      const applied = snapshot.appliedModelId === model.id;
      const configured = snapshot.configuredModelId === model.id;
      const signature = JSON.stringify([model, busy, running, applied, configured, snapshot.server.state, snapshot.runtime.state, t("settings.managed.download", "다운로드")]);
      if (row.signature === signature) continue;
      row.signature = signature;
      row.title.textContent = model.label;
      const local = this.localSupported(model);
      const serverModel = this.serverModel(model);
      row.route.textContent = local
        ? t("settings.managed.filterLocal", "이 컴퓨터에서 직접 실행")
        : serverModel ? t("settings.managed.filterServer", "기존 판단 서버로 연결")
          : t("settings.managed.routeUnavailable", "실행 방법 확인 필요");
      row.facts.textContent = [`${bytesLabel(model.weightsBytes ?? model.downloadBytes)} · ${model.license}`, this.inputSupport(model)].filter(Boolean).join(" · ");
      row.summary.textContent = t("settings.managed.details", "모델 출처와 고정 버전");
      row.identity.textContent = [model.serverModelId ?? model.id, model.engine, model.revision, model.installedDirectory].filter(Boolean).join(" · ");
      row.source.textContent = t("settings.managed.source", "모델 출처 보기");
      try {
        const source = new URL(model.source);
        row.source.hidden = source.protocol !== "https:" || Boolean(source.username || source.password);
        if (!row.source.hidden) row.source.href = source.href;
      } catch {
        row.source.hidden = true;
        row.source.removeAttribute("href");
      }
      row.state.textContent = this.modelState(model);
      row.buttons.prepare.textContent = t("settings.managed.prepare", "실행 환경 설치");
      row.buttons.install.textContent = t("settings.managed.download", "다운로드");
      row.buttons.activate.textContent = model.installed ? t("settings.managed.activate", "실행하고 적용") : t("settings.models.installUse", "설치하고 사용");
      row.buttons.connect.textContent = t("settings.managed.connectServer", "이 모델의 서버 연결");
      row.buttons.import.textContent = t("settings.managed.import", "기존 파일 등록");
      row.buttons.stop.textContent = t("settings.managed.stop", "중지");
      row.buttons.remove.textContent = t("settings.managed.remove", "다운로드 파일 삭제");
      for (const button of Object.values(row.buttons)) button.disabled = busy;
      row.buttons.prepare.hidden = !local || !model.runtime || this.runtimeFor(model)?.state !== "missing";
      row.buttons.install.hidden = model.installed || !local;
      row.buttons.import.hidden = model.installed || !local;
      row.buttons.activate.hidden = !local;
      row.buttons.connect.hidden = !serverModel;
      row.buttons.activate.disabled ||= applied || !["ready", "missing"].includes(this.runtimeFor(model)?.state);
      row.buttons.stop.hidden = !running;
      row.buttons.remove.hidden = !model.installed;
      row.buttons.remove.disabled ||= configured || running;
    }
    for (const [modelId, row] of this.rows) {
      if (kept.has(modelId)) continue;
      if (row.card.contains(document.activeElement)) el("managed-judgment-refresh").focus();
      row.card.remove();
      this.rows.delete(modelId);
    }
    el("managed-judgment-library-summary").textContent = snapshot
      ? t("settings.managed.libraryCount", "모델 {{shown}}개 표시 · 전체 {{total}}개", { shown, total: snapshot.models.length }) : "";
    el("managed-judgment-no-matches").hidden = !snapshot || snapshot.models.length === 0 || shown > 0;
    el("managed-judgment-empty").hidden = !snapshot || snapshot.models.length > 0;
  },
};
