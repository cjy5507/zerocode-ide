const judgmentModelSourceController = {
  initialized: false,
  kind: "hugging_face",
  directory: null,
  file: null,
  destination: null,
  inspection: null,
  problem: null,
  flow: null,
  working: false,
  inspecting: false,
  inflight: null,
  request: null,
  sequence: 0,
  stepsSignature: "",

  get busy() {
    return this.working || this.inspecting;
  },

  init() {
    if (this.initialized) return;
    this.initialized = true;
    for (const [name, kind] of [["hub", "hugging_face"], ["local", "local_directory"], ["gguf", "local_file"]]) {
      el(`judgment-source-${name}`).addEventListener("click", () => this.choose(kind));
    }
    el("judgment-source-repository").addEventListener("input", (event) => {
      this.invalidate();
      if (event.inputType === "insertFromPaste" && this.source()) void this.inspect();
    });
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
    el("judgment-source-use").addEventListener("click", () => void this.act());
    el("judgment-source-cancel").addEventListener("click", () => void this.cancel());
    el("judgment-source-retry").addEventListener("click", () => void this.retry());
    el("judgment-source-folder-instead").addEventListener("click", () => this.choose("local_directory"));
    el("judgment-source-jev").addEventListener("click", () => {
      if (!judgmentProviderController.state) return;
      el("judgment-connection-settings").open = true;
      el("judgment-provider-kind").value = "jev";
      judgmentProviderController.changeKind();
      el("typesafe-key-input").focus();
    });
  },

  choose(kind) {
    if (this.locked()) return;
    this.kind = kind;
    this.invalidate();
    if ((kind === "local_directory" && !this.directory) || (kind === "local_file" && !this.file)) void this.chooseFolder(false);
  },

  inspectAddress(address) {
    if (this.locked() || !address) return;
    this.kind = "hugging_face";
    el("judgment-source-repository").value = address;
    this.invalidate();
    void this.inspect();
    el("judgment-source-result").scrollIntoView?.({ block: "nearest" });
  },

  source() {
    if (this.kind === "local_directory") return this.directory ? { kind: this.kind, directory: this.directory } : null;
    if (this.kind === "local_file") return this.file ? { kind: this.kind, file: this.file } : null;
    const repository = el("judgment-source-repository").value.trim();
    if (!repository) return null;
    try {
      const address = new URL(repository);
      if (/\.gguf$/i.test(address.pathname)) return { kind: "hugging_face_file", url: repository };
    } catch {}
    return { kind: this.kind, repository };
  },

  locked() {
    const operation = managedJudgmentController.snapshot?.operation;
    return this.working || typesafeBusy || managedJudgmentController.busy ||
      (managedJudgmentController.operating() && operation?.kind !== "inspect");
  },

  invalidate() {
    this.sequence += 1;
    this.inspection = null;
    this.problem = null;
    this.flow = null;
    this.inspecting = false;
    this.stopInspection();
    this.say("");
    this.render();
  },

  stopInspection() {
    if (!this.request) return;
    this.request.cancelWanted = true;
    this.cancelOwnInspection();
  },

  cancelOwnInspection() {
    const request = this.request;
    const snapshot = managedJudgmentController.snapshot;
    const operation = snapshot?.operation;
    if (!request?.cancelWanted || request.cancelSent || snapshot?.instanceId !== request.instanceId ||
      operation?.kind !== "inspect" || operation.state !== "running" || operation.sourceRequestId !== request.id) return;
    request.cancelSent = true;
    invoke("managed_judgment_cancel", { operationId: operation.id })
      .then((answer) => managedJudgmentController.accept(answer), () => {});
  },

  say(words) {
    el("judgment-source-status").textContent = words;
  },

  async chooseFolder(destination) {
    if (this.locked()) return;
    this.working = true;
    managedJudgmentController.render();
    let inspectFile = false;
    try {
      const file = !destination && this.kind === "local_file";
      const chosen = await openPathBrowser({ mode: file ? "attach" : "folder", multiple: false, system: true, start: destination ? this.destination : file ? null : this.directory });
      if (!chosen.length) return;
      if (file && chosen.length !== 1) {
        this.file = null;
        this.invalidate();
        this.say(t("settings.gguf.pickHint", "GGUF 파일 하나를 고르세요. 원본은 그대로 두고 앱 전용 폴더에 복사합니다."));
        return;
      }
      if (destination) this.destination = chosen[0];
      else {
        if (file) this.file = chosen[0]; else this.directory = chosen[0];
        this.invalidate();
        inspectFile = file;
      }
    } catch (error) {
      this.say(managedJudgmentController.failure(error));
    } finally {
      this.working = false;
      managedJudgmentController.render();
    }
    if (inspectFile) void this.inspect();
  },

  currentInspection() {
    const snapshot = managedJudgmentController.snapshot;
    return this.inspection && snapshot?.instanceId === this.inspection.instanceId && snapshot?.catalogFingerprint === this.inspection.catalogFingerprint
      ? this.inspection : null;
  },

  modelOf(inspection) {
    return { ...inspection.model, local: inspection.local ?? inspection.model.local };
  },

  usable(inspection) {
    return Boolean(inspection?.readyForInstall && managedJudgmentController.localSupported(this.modelOf(inspection)));
  },

  keepable(inspection) {
    return Boolean(inspection?.readyForDownload && !this.usable(inspection) && managedJudgmentController.downloadOnly(this.modelOf(inspection)));
  },

  libraryModel(inspection) {
    return managedJudgmentController.snapshot?.models.find((model) => model.id === inspection.model.id && model.revision === inspection.model.revision) ?? null;
  },

  installed(inspection) {
    return Boolean(inspection && (this.libraryModel(inspection)?.installed ?? inspection.model.installed));
  },

  current(inspection) {
    const model = inspection ? this.libraryModel(inspection) : null;
    return Boolean(model && managedJudgmentController.appliedModel(model));
  },

  installArgs(inspection) {
    const args = {
      inspectionId: inspection.inspectionId,
      instanceId: inspection.instanceId,
      expectedCatalogFingerprint: inspection.catalogFingerprint,
    };
    if (inspection.allowsDestinationSelection && this.destination) args.destinationDirectory = this.destination;
    return args;
  },

  async inspect() {
    const source = this.source();
    if (this.locked() || !source || !managedJudgmentController.snapshot) return null;
    const sequence = ++this.sequence;
    this.inspection = null;
    this.problem = null;
    this.flow = null;
    this.inspecting = true;
    this.stopInspection();
    this.say(t("settings.models.inspecting", "모델 형식과 실행 방법을 확인하는 중… 모델 파일은 아직 내려받지 않습니다."));
    managedJudgmentController.render();
    while (this.inflight) {
      await this.inflight;
      if (sequence !== this.sequence) return null;
    }
    const request = { id: crypto.randomUUID(), instanceId: managedJudgmentController.snapshot?.instanceId, cancelWanted: false, cancelSent: false };
    this.request = request;
    const pending = invoke("managed_judgment_inspect_source", { source, requestId: request.id });
    const settled = pending.then(() => {}, () => {});
    this.inflight = settled;
    void settled.then(() => {
      if (this.inflight === settled) this.inflight = null;
      if (this.request === request) this.request = null;
    });
    try {
      const result = await pending;
      if (sequence !== this.sequence || JSON.stringify(source) !== JSON.stringify(this.source())) return null;
      const snapshot = managedJudgmentController.snapshot;
      if (!result?.inspectionId || !result.model || result.instanceId !== snapshot?.instanceId || result.catalogFingerprint !== snapshot?.catalogFingerprint) throw "catalog_changed";
      this.inspection = result;
      if (this.usable(result)) {
        this.say(t("settings.models.inspected", "모델을 확인했습니다. 설치하고 사용을 누르면 필요한 준비와 연결을 자동으로 진행합니다."));
      } else if (this.keepable(result)) {
        this.say(t("settings.models.inspectedKeep", "모델 형식을 확인했습니다. 이 컴퓨터의 직접 실행은 아직 검증 전이라 파일만 받아 둘 수 있습니다. 아직 아무것도 내려받지 않았습니다."));
      } else {
        this.problem = result.reason ?? result.local?.reason ?? "model_format_not_supported";
        this.say(managedJudgmentController.failure(this.problem));
      }
      return result;
    } catch (error) {
      if (sequence === this.sequence) {
        this.problem = error ?? "failed";
        this.say(managedJudgmentController.failure(error));
      }
      return null;
    } finally {
      if (sequence === this.sequence) this.inspecting = false;
      managedJudgmentController.render();
    }
  },

  begin(intent, steps) {
    this.flow = {
      intent,
      steps,
      reached: 1,
      failed: false,
      done: false,
      previousOwned: managedJudgmentController.ownedOperation,
    };
    this.stepsSignature = "";
    return this.flow;
  },

  flowOperation() {
    const owned = managedJudgmentController.ownedOperation;
    const operation = managedJudgmentController.snapshot?.operation;
    return this.flow && owned && owned !== this.flow.previousOwned && managedJudgmentController.owns(operation) ? operation : null;
  },

  reusable(inspection) {
    const model = inspection?.model.installed ? this.libraryModel(inspection) : null;
    return model?.installed && managedJudgmentController.localSupported(model) ? model : null;
  },

  runnable(inspection) {
    const model = this.reusable(inspection);
    return Boolean(model && managedJudgmentController.runtimeFor(model)?.state === "ready");
  },

  selected(inspection) {
    const model = inspection ? this.libraryModel(inspection) : null;
    return Boolean(model && !managedJudgmentController.appliedModel(model) && managedJudgmentController.configuredModel(model));
  },

  async act() {
    const inspection = this.currentInspection();
    if (this.locked() || this.inspecting || !inspection) return;
    if (this.usable(inspection)) await this.use(inspection);
    else if (this.keepable(inspection)) await this.keep(inspection);
  },

  async use(inspection) {
    const reused = this.reusable(inspection);
    const steps = !reused ? ["inspect", "download", "verify", "runtime", "start", "check", "apply"]
      : this.runnable(inspection) ? ["inspect", "start", "check", "apply"] : ["inspect", "runtime", "start", "check", "apply"];
    const flow = this.begin("use", steps);
    this.say(t("settings.models.preparing", "다운로드·실행 준비·응답 확인을 진행합니다. 설정은 확인 뒤에만 전환하며, 로컬 모델을 교체하는 동안 판단이 잠시 멈출 수 있습니다."));
    const applied = reused
      ? await managedJudgmentController.activate(reused)
      : await managedJudgmentController.activate(this.modelOf(inspection), { command: "managed_judgment_install_source", args: this.installArgs(inspection) });
    if (flow !== this.flow) return;
    flow.done = applied;
    flow.failed = !applied;
    const error = managedJudgmentController.lastError;
    this.say(applied ? t("settings.models.using", "모델을 불러와 적용했습니다.")
      : error ? managedJudgmentController.failure(error)
        : t("settings.models.notApplied", "새 모델을 적용하지 않았습니다. 아래 진행 결과를 확인한 뒤 다시 시도하세요."));
    managedJudgmentController.render();
  },

  async keep(inspection) {
    const flow = this.begin("keep", ["inspect", "download", "verify", "keep"]);
    const { id, revision } = inspection.model;
    this.working = true;
    this.say(t("settings.models.keeping", "모델 파일만 받아 검증합니다. 실행 환경 설치·실행·적용은 하지 않으며 현재 모델은 그대로입니다."));
    managedJudgmentController.render();
    try {
      if (!(await managedJudgmentController.subscription)) throw new Error("events_unavailable");
      await managedJudgmentController.completed("managed_judgment_install_source", this.installArgs(inspection));
      if (!managedJudgmentController.snapshot.models.some((model) => model.id === id && model.revision === revision && model.installed)) throw "model_missing";
      if (flow === this.flow) {
        flow.done = true;
        this.say(t("settings.models.keptOnly", "모델 파일을 받아 검증했습니다. 이 컴퓨터의 직접 실행은 아직 검증되지 않아 현재 모델은 그대로입니다."));
      }
    } catch (error) {
      if (flow === this.flow) {
        flow.failed = true;
        this.say(managedJudgmentController.failure(error));
      }
    } finally {
      this.working = false;
      managedJudgmentController.render();
    }
  },

  async cancel() {
    if (this.inspecting) {
      this.stopInspection();
      this.sequence += 1;
      this.inspecting = false;
      this.problem = "cancelled";
      this.say(managedJudgmentController.failure("cancelled"));
      managedJudgmentController.render();
      return;
    }
    if (this.flowOperation()) await managedJudgmentController.cancelOwned();
  },

  async retry() {
    const intent = this.flow?.failed ? this.flow.intent : null;
    const result = await this.inspect();
    if (!result || !intent || result !== this.inspection) return;
    if (intent === "keep" && this.keepable(result)) await this.keep(result);
    else if (intent === "use" && this.usable(result)) await this.use(result);
  },

  track() {
    const flow = this.flow;
    const operation = this.flowOperation();
    if (!flow || flow.done || flow.failed || !operation) return;
    const advance = (step) => {
      flow.reached = Math.max(flow.reached, flow.steps.indexOf(step));
    };
    if (["download", "import"].includes(operation.kind)) {
      advance(operation.state === "done" ? "verify" : "download");
    } else if (operation.kind === "runtime") {
      advance("runtime");
    } else if (operation.kind === "start" || operation.kind === "switch") {
      advance(operation.kind === "switch" && operation.state === "done" ? "apply" : "start");
    } else if (operation.kind === "check") {
      advance(operation.state === "done" ? "apply" : "check");
    }
  },

  stepLabel(step) {
    switch (step) {
      case "inspect": return t("settings.models.step.inspect", "확인");
      case "download": return this.inspection?.sourceKind?.startsWith("local_") ? t("settings.models.step.copy", "복사") : t("settings.models.step.download", "받기");
      case "verify": return t("settings.models.step.verify", "검증");
      case "keep": return t("settings.models.step.keep", "보관");
      case "runtime": return t("settings.models.step.runtime", "실행 환경");
      case "start": return t("settings.models.step.start", "시작");
      case "check": return t("settings.models.step.check", "응답 확인");
    }
    return t("settings.models.step.apply", "적용");
  },

  renderSteps(operation, running) {
    const flow = this.flow;
    const list = el("judgment-source-steps");
    list.hidden = !flow;
    if (!flow) {
      if (list.childElementCount) list.replaceChildren();
      this.stepsSignature = "";
      return;
    }
    const states = flow.steps.map((step, index) => flow.done || index < flow.reached ? "done"
      : index === flow.reached ? (flow.failed ? "failed" : "active") : "pending");
    const bytes = running && operation.totalBytes > 0 && ["download", "import"].includes(operation.kind)
      ? t("settings.managed.progress", "{{received}} / {{total}}", { received: bytesLabel(operation.receivedBytes), total: bytesLabel(operation.totalBytes) }) : "";
    const labels = flow.steps.map((step) => this.stepLabel(step));
    const words = flow.steps.map((step, index) => states[index] === "active" && step === "download" && bytes ? `${labels[index]} · ${bytes}` : labels[index]);
    const signature = JSON.stringify([flow.steps, states]);
    if (signature === this.stepsSignature) {
      for (const [index, item] of [...list.children].entries()) {
        if (item.textContent !== words[index]) item.textContent = words[index];
      }
      return;
    }
    this.stepsSignature = signature;
    list.replaceChildren(...flow.steps.map((step, index) => {
      const item = document.createElement("li");
      item.dataset.step = step;
      item.dataset.state = states[index];
      if (states[index] === "active") item.setAttribute("aria-current", "step");
      item.textContent = words[index];
      return item;
    }));
  },

  renderResult(locked) {
    const row = el("judgment-source-result");
    const inspection = this.inspection;
    row.hidden = !this.inspecting && !inspection && !this.problem;
    const usable = this.usable(inspection);
    const keepable = this.keepable(inspection);
    const installed = this.installed(inspection);
    const current = this.current(inspection);
    const selected = !current && this.selected(inspection);
    row.dataset.state = this.inspecting ? "inspecting"
      : !inspection ? (this.problem === "cancelled" ? "cancelled" : "unsupported")
        : current ? "current" : usable ? "ready" : keepable ? (installed ? "kept" : "download-only") : "unsupported";
    el("judgment-source-model-name").textContent = inspection?.model.label
      ?? (this.kind === "local_directory" ? this.directory : this.kind === "local_file" ? this.file : el("judgment-source-repository").value.trim()) ?? "";
    const currentBadge = el("judgment-source-badge-current");
    currentBadge.hidden = !current && !selected;
    currentBadge.dataset.badge = current ? "current" : "selected";
    say(currentBadge, () => this.current(this.inspection) ? t("settings.models.badgeCurrent", "현재 사용 중")
      : t("settings.models.badgeSelected", "선택됨 · 실행되지 않음"));
    const installedBadge = el("judgment-source-badge-installed");
    installedBadge.hidden = !inspection;
    say(installedBadge, () => this.installed(this.inspection)
      ? t("settings.models.badgeInstalled", "파일 준비됨") : t("settings.models.badgeNotInstalled", "아직 받지 않음"));
    const readiness = el("judgment-source-badge-readiness");
    readiness.hidden = !inspection;
    say(readiness, () => this.usable(this.inspection) ? t("settings.models.badgeReady", "이 컴퓨터에서 실행 가능")
      : this.keepable(this.inspection) ? t("settings.models.badgeDownloadOnly", "파일만 받기 · 실행 검증 전")
        : t("settings.models.badgeUnsupported", "지원하지 않음"));
    el("judgment-source-model-info").hidden = !inspection;
    el("judgment-source-model-support").hidden = !inspection;
    if (inspection) {
      const model = inspection.model;
      const size = inspection.sourceKind?.startsWith("local_")
        ? t("settings.models.copySize", "원본에서 {{size}} 복사", { size: bytesLabel(inspection.copyBytes ?? 0) })
        : t("settings.models.downloadSize", "다운로드 {{size}}", { size: bytesLabel(inspection.downloadBytes ?? model.downloadBytes ?? 0) });
      const runtime = keepable ? "" : t("settings.models.runtimeSize", "실행 환경 {{size}}", { size: bytesLabel(inspection.runtimeDownloadBytes ?? 0) });
      el("judgment-source-model-info").textContent = [size, runtime, model.license].filter(Boolean).join(" · ");
      const inputs = managedJudgmentController.inputSupport(this.modelOf(inspection));
      const support = model.engine === "llama_cpp" ? t("settings.gguf.unverified", "사람이 지정한 검증 안 된 모델") : usable ? t("settings.models.ready", "지원되는 판단 모델 형식입니다. 설치 뒤 실행과 응답을 확인합니다.") : "";
      const template = model.inputTemplate ?? this.libraryModel(inspection)?.inputTemplate;
      const format = template ? t("settings.gguf.template", "입력 형식: {{template}}", { template }) : "";
      const preparation = usable && model.engine === "llama_cpp" && (inspection.runtimeDownloadBytes ?? 0) > 0 ? t("settings.gguf.python", "새 실행 환경에는 CPython 압축 25 MB가 포함됩니다. 앱 전용 폴더에 설치하며 전역 Python은 바꾸지 않습니다.") : "";
      el("judgment-source-model-support").textContent = [support, format, preparation, inputs].filter(Boolean).join(" · ");
      el("judgment-source-model-support").hidden = !support && !inputs;
    }
    const operation = this.flowOperation();
    const flow = this.flow;
    const running = Boolean(flow && !flow.done && !flow.failed && operation && ["running", "cancelling"].includes(operation.state));
    this.renderSteps(operation, running);
    const progress = el("judgment-source-progress");
    progress.hidden = !this.inspecting && !running;
    if (running && operation.totalBytes > 0) {
      progress.max = operation.totalBytes;
      progress.value = Math.min(operation.receivedBytes, operation.totalBytes);
    } else {
      progress.removeAttribute("value");
    }
    const use = el("judgment-source-use");
    const offered = Boolean(inspection) && !current && (usable || (keepable && !installed));
    use.hidden = !offered;
    use.disabled = !offered || locked || this.inspecting || !this.currentInspection();
    use.classList.toggle("btn--primary", offered);
    say(use, () => this.keepable(this.inspection) ? t("settings.models.downloadOnly", "파일만 받기")
      : this.runnable(this.inspection) ? t("settings.models.runUse", "실행하고 사용")
        : this.kind.startsWith("local_") || this.installed(this.inspection) ? t("settings.models.loadUse", "불러와서 사용")
          : t("settings.models.downloadUse", "다운로드하고 사용"));
    const cancel = el("judgment-source-cancel");
    cancel.hidden = !this.inspecting && !running;
    cancel.disabled = !this.inspecting && operation?.state !== "running";
    const retry = el("judgment-source-retry");
    retry.hidden = this.inspecting || !(flow?.failed || (this.problem && !inspection));
    retry.disabled = locked || !this.source();
    const folder = el("judgment-source-folder-instead");
    folder.hidden = this.inspecting || Boolean(inspection) || !this.problem || this.problem === "cancelled" || this.kind !== "hugging_face";
    folder.disabled = locked;
  },

  render() {
    this.init();
    this.track();
    const locked = this.locked();
    const snapshot = managedJudgmentController.snapshot;
    const inspection = this.currentInspection();
    const local = this.kind.startsWith("local_");
    el("judgment-source-hub-fields").hidden = local;
    el("judgment-source-local-fields").hidden = !local;
    el("judgment-source-hub").setAttribute("aria-pressed", String(!local));
    el("judgment-source-local").setAttribute("aria-pressed", String(this.kind === "local_directory"));
    el("judgment-source-gguf").setAttribute("aria-pressed", String(this.kind === "local_file"));
    for (const name of ["hub", "local", "gguf", "jev", "repository", "browse", "change-destination", "default-destination"]) {
      el(`judgment-source-${name}`).disabled = locked;
    }
    el("judgment-source-jev").disabled ||= !judgmentProviderController.state;
    say(el("judgment-source-directory"), () => this.kind === "local_file" ? this.file ?? t("settings.gguf.pickHint", "GGUF 파일 하나를 고르세요. 원본은 그대로 두고 앱 전용 폴더에 복사합니다.") : this.directory ?? t("settings.models.pickHint", "이미 받은 모델의 폴더를 선택하세요. 원본 파일은 그대로 둡니다."));
    say(el("judgment-source-browse"), () => this.kind === "local_file" ? t("settings.gguf.pickFile", "GGUF 파일 선택") : t("settings.models.pickFolder", "모델 폴더 선택"));
    el("judgment-source-default-destination").hidden = !this.destination;
    el("judgment-source-change-destination").disabled ||= inspection?.allowsDestinationSelection === false;
    const location = el("judgment-source-destination");
    say(location, () => this.currentInspection()?.allowsDestinationSelection === false && this.currentInspection()?.model.installed
      ? t("settings.models.keepLocation", "이미 설치한 모델의 현재 폴더를 사용합니다.")
      : this.destination ?? t("settings.models.defaultFolder", "기본 모델 폴더"));
    location.dataset.tip = this.destination ?? inspection?.defaultDestinationDirectory ?? "";
    el("judgment-source-inspect").disabled = locked || !this.source() || !snapshot?.catalogFingerprint;
    this.renderResult(locked);
    el("judgment-source-inspect").classList.toggle("btn--primary", el("judgment-source-use").hidden);
  },
};
