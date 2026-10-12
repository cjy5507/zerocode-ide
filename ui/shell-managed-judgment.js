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

  init() {
    if (this.initialized) return;
    this.initialized = true;
    el("managed-judgment-refresh")?.addEventListener("click", () => void this.refresh());
    el("managed-judgment-prepare")?.addEventListener("click", () => void this.perform("managed_judgment_prepare_runtime"));
    el("managed-judgment-cancel")?.addEventListener("click", () => void this.cancel());
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
      else if (action === "install") void this.perform("managed_judgment_install", this.modelArgs(model));
      else if (action === "remove") void this.perform("managed_judgment_remove", this.modelArgs(model));
      else if (action === "stop") void this.perform("managed_judgment_stop", { serverId: this.snapshot.server.id });
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
    return !settingsView.hidden && settingsPane === "api-routers" && !el("managed-judgment-panel").hidden;
  },

  say(words) {
    el("managed-judgment-status").textContent = words;
  },

  failure(error) {
    const token = typeof error === "string" ? error : error?.code ?? error?.kind;
    if (token === "settings_changed") return judgmentProviderController.failure(token);
    if (token === "cancelled") return t("settings.managed.cancelled", "취소했습니다. 현재 적용된 모델은 바꾸지 않았습니다.");
    return t("settings.managed.failed", "작업 결과를 확인하지 못했습니다. 상태를 새로 읽어 실제 설치·실행·적용 상태를 확인하세요.");
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
    if (this.busy || this.operating()) return;
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
      };
      const timeout = setTimeout(() => finish(new Error("operation_timeout")), 180_000);
      this.waiters.add(observe);
      observe();
    });
  },

  async activate(model) {
    if (this.busy || this.operating() || !model.installed || model.compatibility !== "supported" || this.snapshot?.runtime.state !== "ready") return;
    const expectedProviderIdentity = this.snapshot.providerIdentity;
    if (!expectedProviderIdentity) return;
    this.busy = true;
    this.activating = true;
    this.say("");
    this.render();
    judgmentProviderController.controls();
    try {
      if (!(await this.subscription)) throw new Error("events_unavailable");
      if (this.snapshot.server.state !== "ready" || this.snapshot.server.modelId !== model.id) {
        await this.completed("managed_judgment_start", this.modelArgs(model));
      }
      const serverId = this.snapshot.server.id;
      if (!serverId || this.snapshot.server.state !== "ready" || this.snapshot.server.modelId !== model.id) throw new Error("server_not_ready");
      await this.completed("managed_judgment_check", { serverId });
      if (this.snapshot.server.id !== serverId || !this.snapshot.server.checked) throw new Error("server_changed");
      const applied = await invoke("managed_judgment_apply", { serverId, expectedProviderIdentity });
      this.accept(applied);
      if (applied.appliedModelId !== model.id) throw new Error("model_not_applied");
      judgmentProviderController.draft = null;
      await refreshTypeSafe();
      this.say(t("settings.managed.applied", "로컬 모델을 적용했습니다. 응답 확인은 작업 정확도 검증과 별개입니다."));
    } catch (error) {
      this.say(this.failure(error));
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
    const buttons = {};
    for (const action of ["install", "activate", "import", "stop", "remove"]) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "btn";
      button.dataset.modelAction = action;
      button.dataset.modelId = model.id;
      buttons[action] = button;
      actions.append(button);
    }
    card.append(title, facts, source, state, actions);
    return { card, title, facts, source, state, buttons, signature: null };
  },

  modelState(model) {
    const snapshot = this.snapshot;
    if (snapshot.appliedModelId === model.id) return t("settings.managed.inUse", "실행 중 · 현재 적용됨");
    if (snapshot.server.modelId === model.id && snapshot.server.state === "starting") return t("settings.managed.starting", "모델을 시작하는 중 · 아직 적용하지 않음");
    if (snapshot.server.modelId === model.id && snapshot.server.state === "ready") return t("settings.managed.runningUnapplied", "실행 중 · 아직 적용하지 않음");
    if (snapshot.configuredModelId === model.id) return t("settings.managed.configuredStopped", "선택된 모델 · 실행되지 않음. 실행하고 적용을 눌러 다시 연결하세요.");
    return model.installed ? t("settings.managed.installed", "설치됨 · 아직 적용하지 않음") : t("settings.managed.notInstalled", "아직 다운로드하지 않음");
  },

  render() {
    const snapshot = this.snapshot;
    const busy = this.busy || this.operating();
    el("managed-judgment-refresh").disabled = this.loading;
    el("managed-judgment-prepare").disabled = busy || !snapshot || snapshot.runtime.state !== "missing";
    el("managed-judgment-import-submit").disabled = busy || !this.importModel;
    const runtime = el("managed-judgment-runtime");
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
    for (const model of snapshot?.models ?? []) {
      kept.add(model.id);
      let row = this.rows.get(model.id);
      if (!row) {
        row = this.makeRow(model);
        this.rows.set(model.id, row);
        models.append(row.card);
      }
      const running = snapshot.server.modelId === model.id && snapshot.server.state !== "stopped" && snapshot.server.state !== "failed";
      const applied = snapshot.appliedModelId === model.id;
      const configured = snapshot.configuredModelId === model.id;
      const signature = JSON.stringify([model, busy, running, applied, configured, snapshot.server.state, snapshot.runtime.state, t("settings.managed.download", "다운로드")]);
      if (row.signature === signature) continue;
      row.signature = signature;
      row.title.textContent = model.label;
      row.facts.textContent = `${model.engine} · ${bytesLabel(model.downloadBytes)} · ${model.license} · ${model.revision}`;
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
      row.buttons.install.textContent = t("settings.managed.download", "다운로드");
      row.buttons.activate.textContent = t("settings.managed.activate", "실행하고 적용");
      row.buttons.import.textContent = t("settings.managed.import", "기존 파일 등록");
      row.buttons.stop.textContent = t("settings.managed.stop", "중지");
      row.buttons.remove.textContent = t("settings.managed.remove", "다운로드 파일 삭제");
      for (const button of Object.values(row.buttons)) button.disabled = busy || model.compatibility !== "supported";
      row.buttons.install.hidden = model.installed;
      row.buttons.import.hidden = model.installed;
      row.buttons.activate.hidden = !model.installed;
      row.buttons.activate.disabled ||= applied || snapshot.runtime.state !== "ready";
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
    el("managed-judgment-empty").hidden = !snapshot || snapshot.models.length > 0;
  },
};
