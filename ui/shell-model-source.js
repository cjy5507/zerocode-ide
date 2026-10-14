const judgmentModelSourceController = {
  initialized: false,
  kind: "hugging_face",
  directory: null,
  destination: null,
  inspection: null,
  busy: false,
  sequence: 0,

  init() {
    if (this.initialized) return;
    this.initialized = true;
    for (const [name, kind] of [["hub", "hugging_face"], ["local", "local_directory"]]) {
      el(`judgment-source-${name}`).addEventListener("click", () => {
        this.kind = kind;
        this.invalidate();
        if (kind === "local_directory" && !this.directory) void this.chooseFolder(false);
      });
    }
    el("judgment-source-repository").addEventListener("input", () => this.invalidate());
    el("judgment-source-repository").addEventListener("keydown", (event) => {
      if (event.key !== "Enter" || event.isComposing) return;
      event.preventDefault();
      event.stopPropagation();
      void this.inspect();
    });
    el("judgment-source-browse").addEventListener("click", () => void this.chooseFolder(false));
    el("judgment-source-change-destination").addEventListener("click", () => void this.chooseFolder(true));
    el("judgment-source-default-destination").addEventListener("click", () => {
      this.destination = null;
      this.render();
    });
    el("judgment-source-inspect").addEventListener("click", () => void this.inspect());
    el("judgment-source-use").addEventListener("click", () => void this.use());
    el("judgment-source-jev").addEventListener("click", () => {
      if (!judgmentProviderController.state) return;
      el("judgment-connection-settings").open = true;
      el("judgment-provider-kind").value = "jev";
      judgmentProviderController.changeKind();
      el("typesafe-key-input").focus();
    });
  },

  source() {
    if (this.kind === "local_directory") return this.directory ? { kind: this.kind, directory: this.directory } : null;
    const repository = el("judgment-source-repository").value.trim();
    return repository ? { kind: this.kind, repository } : null;
  },

  locked() {
    return this.busy || typesafeBusy || managedJudgmentController.busy || managedJudgmentController.operating();
  },

  invalidate() {
    this.sequence += 1;
    this.inspection = null;
    this.say("");
    this.render();
  },

  say(words) {
    el("judgment-source-status").textContent = words;
  },

  async chooseFolder(destination) {
    if (this.locked()) return;
    this.busy = true;
    managedJudgmentController.render();
    try {
      const chosen = await openPathBrowser({ mode: "folder", start: destination ? this.destination : this.directory });
      if (!chosen.length) return;
      if (destination) this.destination = chosen[0];
      else {
        this.directory = chosen[0];
        this.invalidate();
      }
    } catch (error) {
      this.say(managedJudgmentController.failure(error));
    } finally {
      this.busy = false;
      managedJudgmentController.render();
    }
  },

  currentInspection() {
    const snapshot = managedJudgmentController.snapshot;
    return this.inspection && snapshot?.instanceId === this.inspection.instanceId && snapshot?.catalogFingerprint === this.inspection.catalogFingerprint
      ? this.inspection : null;
  },

  usable(inspection) {
    return Boolean(inspection?.readyForInstall && managedJudgmentController.localSupported({ ...inspection.model, local: inspection.local ?? inspection.model.local }));
  },

  async inspect() {
    const source = this.source();
    if (this.locked() || !source || !managedJudgmentController.snapshot) return;
    const sequence = ++this.sequence;
    this.inspection = null;
    this.busy = true;
    this.say(t("settings.models.inspecting", "모델 형식과 실행 방법을 확인하는 중… 모델 파일은 아직 내려받지 않습니다."));
    managedJudgmentController.render();
    try {
      const result = await invoke("managed_judgment_inspect_source", { source });
      if (sequence !== this.sequence || JSON.stringify(source) !== JSON.stringify(this.source())) return;
      const snapshot = managedJudgmentController.snapshot;
      if (!result?.inspectionId || !result.model || result.instanceId !== snapshot?.instanceId || result.catalogFingerprint !== snapshot?.catalogFingerprint) throw "catalog_changed";
      this.inspection = result;
      this.say(this.usable(result) ? t("settings.models.inspected", "모델을 확인했습니다. 설치하고 사용을 누르면 필요한 준비와 연결을 자동으로 진행합니다.")
        : managedJudgmentController.failure(result.reason ?? result.local?.reason ?? "model_format_not_supported"));
    } catch (error) {
      if (sequence === this.sequence) this.say(managedJudgmentController.failure(error));
    } finally {
      if (sequence === this.sequence) this.busy = false;
      managedJudgmentController.render();
    }
  },

  async use() {
    const inspection = this.currentInspection();
    if (this.locked() || !this.usable(inspection)) return;
    const model = { ...inspection.model, local: inspection.local ?? inspection.model.local };
    const args = {
      inspectionId: inspection.inspectionId,
      instanceId: inspection.instanceId,
      expectedCatalogFingerprint: inspection.catalogFingerprint,
    };
    if (inspection.allowsDestinationSelection && this.destination) args.destinationDirectory = this.destination;
    this.say(t("settings.models.preparing", "다운로드·실행 준비·응답 확인을 진행합니다. 설정은 확인 뒤에만 전환하며, 로컬 모델을 교체하는 동안 판단이 잠시 멈출 수 있습니다."));
    const applied = await managedJudgmentController.activate(model, { command: "managed_judgment_install_source", args });
    if (applied) {
      this.inspection = null;
      this.say(t("settings.models.using", "모델을 불러와 적용했습니다."));
    } else {
      this.say(t("settings.models.notApplied", "새 모델을 적용하지 않았습니다. 아래 진행 결과를 확인한 뒤 다시 시도하세요."));
    }
    this.render();
  },

  render() {
    this.init();
    const locked = this.locked();
    const snapshot = managedJudgmentController.snapshot;
    const inspection = this.currentInspection();
    const local = this.kind === "local_directory";
    el("judgment-source-hub-fields").hidden = local;
    el("judgment-source-local-fields").hidden = !local;
    el("judgment-source-hub").setAttribute("aria-pressed", String(!local));
    el("judgment-source-local").setAttribute("aria-pressed", String(local));
    for (const name of ["hub", "local", "jev", "repository", "browse", "change-destination", "default-destination"]) {
      el(`judgment-source-${name}`).disabled = locked;
    }
    el("judgment-source-jev").disabled ||= !judgmentProviderController.state;
    say(el("judgment-source-directory"), () => this.directory ?? t("settings.models.pickHint", "이미 받은 모델의 폴더를 선택하세요. 원본 파일은 그대로 둡니다."));
    el("judgment-source-default-destination").hidden = !this.destination;
    el("judgment-source-change-destination").disabled ||= inspection?.allowsDestinationSelection === false;
    const location = el("judgment-source-destination");
    say(location, () => this.currentInspection()?.allowsDestinationSelection === false && this.currentInspection()?.model.installed
      ? t("settings.models.keepLocation", "이미 설치한 모델의 현재 폴더를 사용합니다.")
      : this.destination ?? t("settings.models.defaultFolder", "기본 모델 폴더"));
    location.dataset.tip = this.destination ?? inspection?.defaultDestinationDirectory ?? "";
    el("judgment-source-inspect").disabled = locked || !this.source() || !snapshot?.catalogFingerprint;
    el("judgment-source-inspect").classList.toggle("btn--primary", !inspection);
    el("judgment-source-preview").hidden = !inspection;
    el("judgment-source-use").disabled = locked || !this.usable(inspection);
    el("judgment-source-use").classList.toggle("btn--primary", Boolean(inspection));
    say(el("judgment-source-use"), () => this.kind === "local_directory" || this.currentInspection()?.model.installed
      ? t("settings.models.loadUse", "불러와서 사용") : t("settings.models.downloadUse", "다운로드하고 사용"));
    if (inspection) {
      const model = inspection.model;
      el("judgment-source-model-name").textContent = model.label;
      const size = inspection.sourceKind === "local_directory"
        ? t("settings.models.copySize", "원본에서 {{size}} 복사", { size: bytesLabel(inspection.copyBytes ?? 0) })
        : t("settings.models.downloadSize", "다운로드 {{size}}", { size: bytesLabel(inspection.downloadBytes ?? model.downloadBytes ?? 0) });
      const runtime = t("settings.models.runtimeSize", "실행 환경 {{size}}", { size: bytesLabel(inspection.runtimeDownloadBytes ?? 0) });
      el("judgment-source-model-info").textContent = `${size} · ${runtime} · ${model.license}`;
      const inputs = managedJudgmentController.inputSupport({ ...model, local: inspection.local ?? model.local });
      const support = this.usable(inspection)
        ? t("settings.models.ready", "지원되는 판단 모델 형식입니다. 설치 뒤 실행과 응답을 확인합니다.")
        : managedJudgmentController.failure(inspection.reason ?? inspection.local?.reason ?? "model_format_not_supported");
      el("judgment-source-model-support").textContent = [support, inputs].filter(Boolean).join(" · ");
    }
  },
};
