/* How much memory a GGUF file takes to run, for each byte it weighs: its
 * weights mapped whole plus the working memory of a 4,096-token context.
 * Measured 2026-11-10 — a 1.12 GB Q4_K_M file ran at 2.31 GB. A file past
 * the runtime's limit by this measure is still offered, with the warning. */
const RUN_MEMORY_PER_FILE_BYTE = 2;

/* 「모델 받기」: the address field is read three ways — words find
 * repositories on Hugging Face, a repository's address or ID lists the GGUF
 * files in it, and a `.gguf` file's address is that file — and one file is
 * inspected, received, started and applied. A folder or a file on this
 * computer takes the same road from its inspection on. Nothing is received
 * before the person presses the one filled button. */
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
  /* A search or a file list on its way, the repositories a search answered
   * with, the list one repository answered with (and the text it was asked
   * by), and the path picked out of it. */
  finding: false,
  hits: null,
  listing: null,
  picked: null,
  inflight: null,
  request: null,
  sequence: 0,
  stepsSignature: "",
  foundSignature: "",

  get busy() {
    return this.working || this.inspecting || this.finding;
  },

  init() {
    if (this.initialized) return;
    this.initialized = true;
    for (const [name, kind] of [["hub", "hugging_face"], ["local", "local_directory"], ["gguf", "local_file"]]) {
      el(`judgment-source-${name}`).addEventListener("click", () => this.choose(kind));
    }
    el("judgment-source-repository").addEventListener("input", (event) => {
      this.invalidate();
      // A pasted address or ID is what the person came with: it is read at
      // once. Words wait for Enter, so a search is not sent letter by letter.
      if (event.inputType === "insertFromPaste" && !this.reading()?.words) void this.go();
    });
    el("judgment-source-repository").addEventListener("keydown", (event) => {
      if (event.key !== "Enter" || event.isComposing) return;
      event.preventDefault();
      event.stopPropagation();
      void this.go();
    });
    el("judgment-source-hits").addEventListener("click", (event) => {
      const hit = event.target.closest("button[data-repository]");
      if (hit && !hit.disabled) void this.openHit(hit.dataset.repository);
    });
    el("judgment-source-file-list").addEventListener("change", (event) => {
      if (event.target instanceof HTMLInputElement && event.target.name === "judgment-source-file") this.pick(event.target.value);
    });
    el("judgment-source-browse").addEventListener("click", () => void this.chooseFolder(false));
    el("judgment-source-change-destination").addEventListener("click", () => void this.chooseFolder(true));
    el("judgment-source-default-destination").addEventListener("click", () => {
      this.destination = null;
      this.render();
    });
    el("judgment-source-inspect").addEventListener("click", () => void this.go());
    el("judgment-source-use").addEventListener("click", () => void this.act());
    // A repository of files too large to receive here names the other road.
    el("judgment-source-local-server").addEventListener("click", () => modelConnectionsController.addLocalServer());
    el("judgment-source-keep").addEventListener("click", () => {
      const inspection = this.currentInspection();
      if (!this.locked() && !this.inspecting && inspection && this.usable(inspection)) void this.keep(inspection);
    });
    el("judgment-source-cancel").addEventListener("click", () => void this.cancel());
    el("judgment-source-retry").addEventListener("click", () => void this.retry());
    el("judgment-source-folder-instead").addEventListener("click", () => this.choose("local_directory"));
  },

  /* A folder or a file on this computer is inspected as soon as it is
   * chosen — choosing it is the asking; one chosen before is inspected
   * again when its tab is come back to. */
  choose(kind) {
    if (this.locked()) return;
    this.kind = kind;
    this.invalidate();
    if (kind === "hugging_face") return;
    if ((kind === "local_directory" && !this.directory) || (kind === "local_file" && !this.file)) void this.chooseFolder(false);
    else void this.inspect();
  },

  inspectAddress(address) {
    if (this.locked() || !address) return;
    this.kind = "hugging_face";
    el("judgment-source-repository").value = address;
    this.invalidate();
    modelConnectionsController.receive(true);
    void this.go();
    el("judgment-source-result").scrollIntoView?.({ block: "nearest" });
  },

  text() {
    return el("judgment-source-repository").value.trim();
  },

  /* What the field holds: a `.gguf` file's address, a repository's address
   * or ID (`owner/name`), or words to find a repository by. */
  reading(text = this.text()) {
    if (!text) return null;
    try {
      const address = new URL(text);
      if (address.protocol === "https:" || address.protocol === "http:") {
        return /\.gguf$/i.test(address.pathname) ? { file: text } : { repository: text };
      }
    } catch {}
    return /^[\w.-]+\/[\w.-]+$/.test(text) ? { repository: text } : { words: text };
  },

  /* The file picked out of the repository's list, while it can be received. */
  pickedFile() {
    return this.listing?.files.find((file) => file.path === this.picked && file.url) ?? null;
  },

  /* What an inspection is asked about: the folder or file chosen here, the
   * `.gguf` address typed, the file picked from the repository's list —
   * pinned to the commit the list was read at — or, for a repository laid
   * out as a decision model, the repository itself. */
  source() {
    if (this.kind === "local_directory") return this.directory ? { kind: this.kind, directory: this.directory } : null;
    if (this.kind === "local_file") return this.file ? { kind: this.kind, file: this.file } : null;
    const reading = this.reading();
    if (!reading) return null;
    if (reading.file) return { kind: "hugging_face_file", url: reading.file };
    if (!reading.repository || this.listing?.asked !== reading.repository) return null;
    const picked = this.pickedFile();
    if (picked) return { kind: "hugging_face_file", url: picked.url };
    return this.listing.native ? { kind: this.kind, repository: reading.repository } : null;
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
    this.finding = false;
    this.hits = null;
    this.listing = null;
    this.picked = null;
    this.stopInspection();
    this.say("");
    this.render();
  },

  /* The button, Enter and a paste: what the field holds decides the road. */
  async go() {
    if (this.kind !== "hugging_face") return this.inspect();
    const reading = this.reading();
    if (!reading) return null;
    if (reading.file) return this.inspect();
    return reading.repository ? this.list(reading.repository) : this.search(reading.words);
  },

  /* A request for a search or a file list may start: nothing else is under
   * way and the manager's state has been read. */
  findable() {
    return !this.locked() && !this.inspecting && Boolean(managedJudgmentController.snapshot);
  },

  /* Words: the repositories of GGUF files Hugging Face finds for them.
   * `missing` says the words are an ID the Hub would not open: with no
   * answer, that is what is said. */
  async search(words, { missing = false } = {}) {
    if (!this.findable()) return null;
    const sequence = ++this.sequence;
    Object.assign(this, { inspection: null, problem: null, flow: null, hits: null, listing: null, picked: null, finding: true });
    this.say(t("settings.models.searching", "Hugging Face에서 찾는 중…"));
    managedJudgmentController.render();
    try {
      const hits = (await invoke("managed_judgment_search_sources", { query: words })) ?? [];
      if (sequence !== this.sequence) return null;
      this.hits = hits;
      if (missing && !hits.length) this.problem = "source_missing";
      this.say(hits.length
        ? t("settings.models.found", "저장소 {{count}}개를 찾았습니다. 하나를 고르면 그 안의 파일을 보여 줍니다.", { count: hits.length })
        : missing ? managedJudgmentController.failure("source_missing")
          : t("settings.models.foundNone", "찾은 저장소가 없습니다. 다른 검색어로 찾거나 모델 주소를 붙여 넣으세요."));
      return hits;
    } catch (error) {
      if (sequence !== this.sequence) return null;
      this.problem = error ?? "failed";
      this.say(managedJudgmentController.failure(error));
      return null;
    } finally {
      if (sequence === this.sequence) this.finding = false;
      managedJudgmentController.render();
    }
  },

  /* A repository picked out of a search: its ID stands in the field, and
   * the search's other answers stay on the page. */
  async openHit(repository) {
    if (!this.findable()) return null;
    el("judgment-source-repository").value = repository;
    return this.list(repository, { keepHits: true });
  },

  /* A repository's address or ID: what it offers. A decision model's layout
   * is inspected whole; GGUF files are listed for one to be picked — the
   * only one that can be received is picked at once. */
  async list(repository, { keepHits = false } = {}) {
    if (!this.findable()) return null;
    const sequence = ++this.sequence;
    Object.assign(this, { inspection: null, problem: null, flow: null, listing: null, picked: null, finding: true });
    if (!keepHits) this.hits = null;
    this.say(t("settings.models.listing", "저장소의 파일 목록을 읽는 중… 아직 아무것도 받지 않습니다."));
    managedJudgmentController.render();
    let listing;
    try {
      listing = await invoke("managed_judgment_source_files", { repository });
    } catch (error) {
      if (sequence !== this.sequence) return null;
      this.finding = false;
      // An ID the Hub would not open — it answers a repository that does
      // not exist the way it answers one that needs a login — is read as
      // words to find one by.
      if (["source_unavailable", "source_auth_required"].includes(String(error?.message ?? error)) && !/^https?:/i.test(repository) && !keepHits) {
        return this.search(repository, { missing: true });
      }
      this.problem = error ?? "failed";
      this.say(managedJudgmentController.failure(error));
      managedJudgmentController.render();
      return null;
    }
    if (sequence !== this.sequence) return null;
    this.finding = false;
    this.listing = { ...listing, files: Array.isArray(listing?.files) ? listing.files : [], asked: repository };
    const takeable = this.listing.files.filter((file) => file.url);
    if (this.listing.native) return this.inspect();
    // The one file that can be received is picked at once — unless its run
    // would stand at the memory limit: that one the person picks knowingly.
    if (takeable.length === 1 && !this.heavy(takeable[0], this.listing)) {
      this.picked = takeable[0].path;
      return this.inspect();
    }
    if (!this.listing.files.length) {
      // The original weights of a model hold no GGUF file: the Hub names
      // the repositories that hold it as GGUF, and they stand where a
      // search's answers do — one press from their files.
      const made = Array.isArray(listing?.quantized) ? listing.quantized : [];
      if (made.length) {
        this.hits = made;
        this.say(t("settings.models.foundQuantized", "이 저장소에는 원본 가중치만 있고 GGUF 파일이 없습니다. 같은 모델을 GGUF로 만들어 올린 저장소 {{count}}개를 찾았습니다. 하나를 고르면 그 안의 파일을 보여 줍니다.", { count: made.length }));
      } else {
        this.problem = "source_no_models";
        this.say(managedJudgmentController.failure(this.problem));
      }
    } else {
      this.say(takeable.length
        ? t("settings.models.pickFile", "받을 파일 하나를 고르세요. 고르면 그 파일을 확인합니다. 아직 아무것도 받지 않았습니다.")
        : t("settings.models.noFileFits", "이 저장소의 파일 가운데 이 컴퓨터에서 받을 수 있는 것이 없습니다. 줄마다 까닭이 있습니다."));
    }
    managedJudgmentController.render();
    return this.listing;
  },

  /* One file out of the repository's list: it is inspected, not received. */
  pick(path) {
    if (this.locked() || this.inspecting || !this.listing?.files.some((file) => file.path === path && file.url)) return;
    this.picked = path;
    void this.inspect();
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
    let chosenSource = false;
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
        chosenSource = true;
      }
    } catch (error) {
      this.say(managedJudgmentController.failure(error));
    } finally {
      this.working = false;
      managedJudgmentController.render();
    }
    if (chosenSource) void this.inspect();
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
        this.say(t("settings.models.inspected", "모델을 확인했습니다. 아직 아무것도 받지 않았습니다. 받아서 쓰면 받기·실행·응답 확인을 차례로 하고, 통과하면 판단 모델로 씁니다."));
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
      : error ? managedJudgmentController.failure(error, this.modelOf(inspection))
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
        this.say(this.usable(inspection)
          ? t("settings.models.keptUsable", "모델 파일을 받아 두었습니다. 「이 컴퓨터」의 그 줄에서 「판단에 쓰기」를 누르면 실행합니다. 지금 모델은 그대로입니다.")
          : t("settings.models.keptOnly", "모델 파일을 받아 검증했습니다. 이 컴퓨터의 직접 실행은 아직 검증되지 않아 현재 모델은 그대로입니다."));
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
    const result = await this.go();
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
      ?? (this.kind === "local_directory" ? this.directory : this.kind === "local_file" ? this.file : this.pickedFile()?.path ?? this.text()) ?? "";
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
    // A receive that ended has no button left here: the model's own row
    // under 「이 컴퓨터」 is where it is used from.
    const offered = Boolean(inspection) && !current && !flow?.done && (usable || (keepable && !installed));
    use.hidden = !offered;
    use.disabled = !offered || locked || this.inspecting || !this.currentInspection();
    use.classList.toggle("btn--primary", offered);
    say(use, () => this.keepable(this.inspection) ? t("settings.models.downloadOnly", "파일만 받기")
      : this.runnable(this.inspection) ? t("settings.models.runUse", "실행해서 판단에 쓰기")
        : this.kind.startsWith("local_") || this.installed(this.inspection) ? t("settings.models.loadUse", "불러와서 판단에 쓰기")
          : t("settings.models.downloadUse", "받아서 판단에 쓰기"));
    // A model this computer can run may also be received and left for later.
    const keep = el("judgment-source-keep");
    const kept = Boolean(inspection) && usable && !installed && !current && !this.flow;
    keep.hidden = !kept;
    keep.disabled = !kept || locked || this.inspecting || !this.currentInspection();
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
    for (const [name, on] of [["hub", !local], ["local", this.kind === "local_directory"], ["gguf", this.kind === "local_file"]]) {
      el(`judgment-source-${name}`).setAttribute("aria-pressed", String(on));
      el(`judgment-source-${name}`).classList.toggle("is-active", on);
    }
    for (const name of ["hub", "local", "gguf", "repository", "browse", "change-destination", "default-destination"]) {
      el(`judgment-source-${name}`).disabled = locked;
    }
    say(el("judgment-source-directory"), () => this.kind === "local_file" ? this.file ?? t("settings.gguf.pickHint", "GGUF 파일 하나를 고르세요. 원본은 그대로 두고 앱 전용 폴더에 복사합니다.") : this.directory ?? t("settings.models.pickHint", "이미 받은 모델의 폴더를 선택하세요. 원본 파일은 그대로 둡니다."));
    say(el("judgment-source-browse"), () => this.kind === "local_file" ? t("settings.gguf.pickFile", "GGUF 파일 선택") : t("settings.models.pickFolder", "모델 폴더 선택"));
    el("judgment-source-default-destination").hidden = !this.destination;
    el("judgment-source-change-destination").disabled ||= inspection?.allowsDestinationSelection === false;
    const location = el("judgment-source-destination");
    say(location, () => this.currentInspection()?.allowsDestinationSelection === false && this.currentInspection()?.model.installed
      ? t("settings.models.keepLocation", "이미 설치한 모델의 현재 폴더를 사용합니다.")
      : this.destination ?? t("settings.models.defaultFolder", "기본 모델 폴더"));
    location.dataset.tip = this.destination ?? inspection?.defaultDestinationDirectory ?? "";
    // The button stands beside the address field: for a folder or a file on
    // this computer, choosing it is the inspection.
    const reading = local ? null : this.reading();
    const inspect = el("judgment-source-inspect");
    inspect.hidden = local;
    inspect.disabled = locked || this.finding || this.inspecting || !reading || !snapshot?.catalogFingerprint;
    say(inspect, () => {
      const now = this.reading();
      return now?.file ? t("settings.models.inspect", "확인") : now?.repository ? t("settings.models.listFiles", "파일 보기") : t("settings.models.find", "찾기");
    });
    this.renderFound(locked);
    this.renderResult(locked);
    inspect.classList.toggle("btn--primary", el("judgment-source-use").hidden && !this.listing && !this.hits);
    if (typeof modelConnectionsController !== "undefined") modelConnectionsController.paintReceive();
  },

  /* The repositories a search answered with and the files one repository
   * holds. Redrawn only when what they show changes, so a row under the
   * pointer or the keys is not replaced from under them. */
  renderFound(locked) {
    const hub = this.kind === "hugging_face";
    const hits = el("judgment-source-hits");
    const files = el("judgment-source-files");
    const shownHits = hub && this.hits?.length ? this.hits : [];
    const listing = hub ? this.listing : null;
    const shownFiles = listing && !listing.native ? listing.files : [];
    const off = locked || this.inspecting || this.finding;
    const signature = JSON.stringify([shownHits, listing?.asked, listing?.revision, shownFiles, this.picked, off, listing?.templateKnown, document.documentElement.lang]);
    hits.hidden = shownHits.length === 0;
    files.hidden = shownFiles.length === 0;
    // Half or more of a repository's files over the receive limit: it is a
    // model for coding or talk, and the page says where such a model goes.
    const large = shownFiles.filter((file) => file.reason === "size_limit").length;
    el("judgment-source-files-big").hidden = !(large > 0 && large * 2 >= shownFiles.length);
    if (signature === this.foundSignature) return;
    this.foundSignature = signature;
    const count = (value) => {
      try {
        return new Intl.NumberFormat(document.documentElement.lang || "ko", { notation: "compact", maximumFractionDigits: 1 }).format(value);
      } catch {
        return String(value);
      }
    };
    hits.replaceChildren(...shownHits.map((hit) => {
      const item = document.createElement("li");
      const button = document.createElement("button");
      button.type = "button";
      button.className = "model-hit";
      button.dataset.repository = hit.repository;
      button.disabled = off;
      if (listing?.asked === hit.repository) button.setAttribute("aria-current", "true");
      const name = document.createElement("span");
      name.className = "model-hit-name";
      name.textContent = hit.repository;
      const facts = document.createElement("span");
      facts.className = "model-hit-facts";
      facts.textContent = t("settings.models.hitFacts", "받음 {{downloads}} · 좋아요 {{likes}}", { downloads: count(hit.downloads), likes: count(hit.likes) });
      button.append(name, facts);
      item.append(button);
      return item;
    }));
    if (!listing || listing.native) {
      el("judgment-source-file-list").replaceChildren();
      return;
    }
    el("judgment-source-files-title").textContent = t("settings.models.filesTitle", "{{repository}} · GGUF 파일 {{count}}개 · {{license}}", {
      repository: listing.repository ?? listing.asked, count: shownFiles.length, license: listing.license ?? "",
    });
    el("judgment-source-files-about").textContent = [
      t("settings.models.filesAbout", "같은 모델을 다른 크기로 줄인 파일들입니다. 클수록 정확하고 느립니다. 보통 이름에 Q4_K_M이 든 파일이 크기와 품질의 균형이 좋습니다."),
      listing.templateKnown === false ? t("settings.models.templateUnknown", "이 모델의 대화 형식은 실행 엔진이 아는 형식이 아닙니다. 받아도 실행하지 못할 수 있습니다.") : "",
    ].filter(Boolean).join(" ");
    el("judgment-source-file-list").replaceChildren(...shownFiles.map((file) => {
      const row = document.createElement("label");
      row.className = `model-file${file.url ? "" : " is-off"}`;
      const input = document.createElement("input");
      input.type = "radio";
      input.name = "judgment-source-file";
      input.value = file.path;
      input.checked = this.picked === file.path;
      input.disabled = off || !file.url;
      const name = document.createElement("span");
      name.className = "model-file-name";
      name.textContent = file.path;
      const size = document.createElement("span");
      size.className = "model-file-size";
      size.textContent = bytesLabel(file.bytes);
      row.append(input, name, size);
      const heavy = this.heavy(file, listing);
      if (!file.url || heavy) {
        const why = document.createElement("span");
        why.className = "model-file-why";
        if (heavy) why.dataset.fileHeavy = "";
        why.textContent = heavy
          ? t("settings.models.fileHeavy", "실행 메모리 한도({{limit}})에 가까워 실행 중 멈출 수 있습니다", { limit: bytesLabel(listing.memoryLimitBytes) })
          : this.fileRefusal(file.reason, listing);
        row.append(why);
      }
      return row;
    }));
  },

  /* A file that can be received, whose run would stand close to the
   * runtime's memory limit: its weights and its working memory both count. */
  heavy(file, listing) {
    return Boolean(file.url) && listing.memoryLimitBytes > 0 && file.bytes * RUN_MEMORY_PER_FILE_BYTE > listing.memoryLimitBytes;
  },

  /* Why a listed file cannot be received, in the page's words. */
  fileRefusal(reason, listing) {
    if (reason === "size_limit") return t("settings.models.fileTooLarge", "{{limit}}를 넘어 이 컴퓨터에서 돌리지 않습니다", { limit: bytesLabel(listing.maxFileBytes ?? 0) });
    if (reason === "model_format_not_supported") return t("settings.models.fileSplit", "여러 조각으로 나뉜 파일은 받지 않습니다");
    if (reason === "source_hash_unavailable") return t("settings.models.fileUnhashed", "검증 정보가 없어 받지 않습니다");
    return t("settings.models.fileRefused", "받을 수 없는 파일입니다");
  },
};
