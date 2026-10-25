/* A saved connection is shared; choosing a role never copies its credential. */
const modelConnectionsController = {
  state: null, sequence: 0, busy: false, editing: null,
  drafts: new Map(), results: new Map(), view: "catalog", expanded: new Set(), deletion: null,

  show(view) {
    showSettingsPane(view === "usage" ? "judgment-models" : "api-routers");
    this.paintView(view);
  },
  paintView(view) {
    this.view = view;
    el("connection-catalog-heading").hidden = view === "add";
    for (const node of document.querySelectorAll("[data-connection-view]")) node.hidden = node.dataset.connectionView === "usage" ? false : node.dataset.connectionView !== view;
    for (const node of document.querySelectorAll("[data-connection-nav]")) node.setAttribute("aria-pressed", String(node.dataset.connectionNav === view));
  },
  add() {
    onRouterPresetChange("custom");
    el("router-preset-select").value = "custom";
    el("connection-location").value = "remote";
    el("connection-auth").value = el("router-key-input").disabled ? "none" : "new";
    say(el("connection-form-title"), () => t("settings.connections.add", "연결 추가"));
    this.addFields();
    this.show("add");
  },
  addFields() {
    const service = ["managed", "jev"].includes(el("connection-location").value);
    el("connection-server-fields").hidden = service;
    el("connection-service-action").hidden = !service;
    el("connection-open-service").hidden = !service;
    el("router-test-btn").hidden = service;
    el("router-save-btn").hidden = service;
    const requiresAuth = Boolean(this.editing?.provider.requires_auth);
    const saved = requiresAuth && this.editing.keyAvailable;
    el("connection-auth").querySelector('[value="saved"]').disabled = !saved;
    // Existing keys belong to their saved connection. This UI never copies one.
    el("connection-auth").querySelector('[value="none"]').disabled = requiresAuth;
    el("connection-key-field").hidden = el("connection-auth").value !== "new";
    if (el("connection-auth").value !== "new") el("router-key-input").value = "";
    el("connection-auth-note").textContent = el("connection-auth").value === "saved"
      ? t("settings.connections.referenceHint", "저장된 키는 이 연결의 참조로만 사용합니다. 화면에 읽어오지 않습니다.")
      : t("settings.connections.registrationHint", "연결을 등록해도 현재 사용 모델은 바뀌지 않습니다. 모델 사용에서 용도에 맞게 선택하세요.");
  },
  preset(preset) {
    if (preset) el("connection-location").value = this.location(preset.base_url);
    el("connection-auth").value = el("connection-location").value === "local" || el("router-key-input").disabled ? "none" : "new";
    this.addFields();
  },

  node(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text != null) node.textContent = text;
    return node;
  },
  button(text, action, primary = false) {
    const button = this.node("button", primary ? "btn btn--primary" : "btn", text);
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
  async refresh() {
    const sequence = ++this.sequence;
    try {
      // The shared status read (refreshTypeSafe) feeds the managed models;
      // this request reads only saved connections.
      const state = await invoke("model_connections");
      if (sequence !== this.sequence) return;
      if (!state || !Array.isArray(state.connections)) throw new Error("provider_settings");
      const signature = value => JSON.stringify([value?.activeProviderIdentity, value?.connections?.map(row => [row.id, row.generation])]);
      if (signature(state) !== signature(this.state)) this.results.clear();
      this.state = state;
      this.render();
      void this.sync();
    } catch (error) {
      if (sequence !== this.sequence) return;
      for (const holder of document.querySelectorAll("[data-connection-roles]")) {
        holder.replaceChildren(this.node("p", "settings-hint", this.failure(error)), this.button(t("settings.managed.refresh", "상태 새로고침"), () => void this.refresh()));
        if ((typeof error === "string" ? error : error?.message) === "settings_changed") {
          holder.append(this.button(t("settings.connections.clear", "용도 해제 · 이전 선택 복원"), async () => {
            try { await invoke("clear_model_connection", { role: "judgment" }, { hand: handNow() }); await refreshTypeSafe(); await this.refresh(); }
            catch (failure) { showError(this.failure(failure)); }
          }));
        }
      }
    }
  },
  async sync() {
    try {
      const status = await invoke("sync_model_connections");
      const node = el("connections-sync-status");
      if (node) node.textContent = t("settings.connections.sync", "다음 요청에서 갱신 {{queued}}개 · 이전 버전 재시작 필요 {{restart}}개 · 상태 확인 불가 {{unknown}}개", { queued: status.queued, restart: status.restartRequired, unknown: status.unknown });
    } catch (error) { const node = el("connections-sync-status"); if (node) node.textContent = this.failure(error); }
  },
  selected(role) { return this.drafts.has(role) ? this.drafts.get(role) : this.state?.[role] ?? null; },
  key(reference) { return reference ? JSON.stringify([reference.id, reference.model]) : ""; },
  change(role, reference) {
    this.drafts.set(role, reference);
    this.results.delete(role);
    this.renderRoles();
  },
  scenario(kind) {
    if (this.busy) return;
    const connections = this.state?.connections ?? [];
    const first = location => connections.find(row => row.provider.models?.length && (!location || this.location(row.provider.base_url) === location));
    const local = first("local"), remote = first("remote");
    const coding = kind === "local" || kind === "shared" ? remote : connections.find(row => row.id === this.selected("coding")?.id) ?? remote;
    const judgment = kind === "local" ? local : remote;
    if (!coding || !judgment) {
      el("connection-scenario-status").textContent = t("settings.connections.missingScenario", "이 배정에 필요한 연결이 없습니다. 연결을 먼저 추가하세요.");
      return;
    }
    el("connection-scenario-status").textContent = t("settings.connections.draftOnly", "배정 초안입니다. 형식을 확인하고 적용하기 전까지 현재 모델을 유지합니다.");
    for (const [role, row] of [["coding", coding], ["judgment", judgment]]) {
      const model = row.provider.models[0];
      this.change(role, { id: row.id, generation: row.generation, model: typeof model === "string" ? model : model.id, protocol: "openai_json" });
    }
  },
  referenceName(reference) {
    const row = this.state?.connections.find(row => row.id === reference?.id);
    return row ? `${row.provider.name} / ${reference.model}` : t("settings.connections.unselected", "선택하지 않음");
  },
  activeName(role) {
    const reference = this.state?.[role];
    if (reference) return this.referenceName(reference);
    return (role === "coding" ? this.state?.codingModel : typesafeState?.provider?.model) || t("settings.connections.unselected", "선택하지 않음");
  },
  checked(role) {
    const result = this.results.get(role);
    return Boolean(result?.checked && result.reference === JSON.stringify(this.selected(role)) && result.identity === this.state?.activeProviderIdentity);
  },
  roleName(role) { return role === "coding" ? t("settings.connections.coding", "코딩 · 대화") : t("settings.judgment.title", "판단 모델"); },
  rolePanel(role) {
    const selected = this.selected(role);
    const panel = this.node("section", "connection-role");
    panel.dataset.role = role;
    const heading = this.node("div", "connection-role-heading");
    heading.append(this.node("h3", "settings-subhead-title", this.roleName(role)));
    const active = this.state?.[role];
    heading.append(this.node("p", "settings-badge", t("settings.connections.current", "현재: {{model}}", { model: this.activeName(role) })));
    panel.append(heading);
    const label = this.node("label", "connection-field");
    label.append(this.node("span", "settings-label", t("settings.connections.choose", "연결과 모델 선택")));
    const select = this.node("select", "settings-input");
    select.disabled = this.busy;
    const empty = this.node("option", "", t("settings.connections.choose", "연결과 모델 선택")); empty.value = ""; select.append(empty);
    for (const row of this.state?.connections ?? []) {
      const group = this.node("optgroup"); group.label = row.provider.name;
      const ids = (row.provider.models ?? []).map(model => typeof model === "string" ? model : model.id);
      if (selected?.id === row.id && !ids.includes(selected.model)) ids.push(selected.model);
      for (const model of ids) {
        const option = this.node("option", "", model); option.value = JSON.stringify([row.id, model]); group.append(option);
      }
      select.append(group);
    }
    select.value = this.key(selected);
    select.addEventListener("change", () => {
      if (!select.value) return this.change(role, null);
      const [id, model] = JSON.parse(select.value);
      const row = this.state.connections.find(row => row.id === id);
      this.change(role, { id, generation: row.generation, model, protocol: selected?.protocol ?? "openai_json" });
    });
    label.append(select); panel.append(label);
    if (this.drafts.has(role)) panel.append(this.node("p", "settings-hint", t("settings.connections.draftOnly", "배정 초안입니다. 형식을 확인하고 적용하기 전까지 현재 모델을 유지합니다.")));
    if (role === "judgment") {
      const protocolLabel = this.node("label", "connection-field");
      protocolLabel.append(this.node("span", "settings-label", t("settings.connections.protocol", "판단 응답 방식")));
      const protocol = this.node("select", "settings-input");
      for (const [value, text] of [["openai_json", "OpenAI JSON"], ["system_one", "System One"]]) {
        const option = this.node("option", "", text); option.value = value; protocol.append(option);
      }
      protocol.value = selected?.protocol ?? "openai_json"; protocol.disabled = this.busy || !selected;
      protocol.addEventListener("change", () => this.change(role, { ...selected, protocol: protocol.value }));
      protocolLabel.append(protocol); panel.append(protocolLabel);
      panel.append(this.node("p", "settings-row-desc", t("settings.connections.checkHint", "선택·점수·참/거짓 응답을 실제 요청으로 확인합니다. 형식 통과는 판단 정확도와 다릅니다.")));
      const formats = this.node("div", "settings-action-row connection-formats");
      for (const type of ["Choice", "Score", "Noul"]) {
        const format = this.node("span", "settings-badge", `${this.checked(role) ? "✓" : "○"} ${type}`);
        format.dataset.formatState = this.checked(role) ? "ready" : "unchecked";
        formats.append(format);
      }
      panel.append(formats, this.node("p", "settings-row-desc", judgmentProviderController.usageWords()));
    }
    const actions = this.node("div", "settings-action-row");
    if (role === "judgment") {
      const check = this.button(t("settings.connections.check", "세 유형 확인"), () => void this.run(role, "check"));
      check.dataset.connectionCheck = "";
      check.disabled ||= !selected; actions.append(check);
    }
    const apply = this.button(role === "judgment" ? t("settings.connections.applyJudge", "확인하고 사용") : t("settings.connections.applyCoding", "기본 코딩 모델로 사용"), () => void this.run(role, "apply"), true);
    apply.dataset.connectionApply = "";
    apply.disabled ||= !selected || role === "judgment" && !this.checked(role);
    const clear = this.button(t("settings.connections.clear", "용도 해제 · 이전 선택 복원"), () => void this.run(role, "clear"));
    clear.disabled ||= !active;
    actions.append(apply, clear); panel.append(actions);
    if (role === "coding") panel.append(this.node("p", "settings-row-desc", t("settings.connections.codingHint", "새 zo 세션의 기본 모델입니다. 실행 중인 세션과 명시적으로 지정한 모델은 유지합니다.")));
    const result = this.results.get(role);
    if (result) {
      const status = this.node("p", "connection-result", result.message); status.setAttribute("role", "status"); status.dataset.state = result.failed ? "failed" : "ready"; panel.append(status);
      if (result.failed && role === "judgment") {
        const help = result.reason === "schema" ? t("settings.connections.unsupportedHelp", "판단 응답 형식을 충족하지 못했습니다. 다른 모델을 고르거나 응답 방식을 확인하세요.")
          : result.reason === "provider_request" ? t("settings.connections.stoppedHelp", "서버에 연결하지 못했습니다. 실행 상태와 주소를 확인하거나 다른 모델을 선택하세요.") : "";
        panel.append(this.node("p", "settings-notice connection-failure-help", `${help} ${t("settings.connections.kept", "현재 판단 모델은 유지합니다.")}`));
      }
      if (result.legacy || role === "judgment" && this.state?.legacyKeyPresent) panel.append(this.node("p", "settings-notice", t("settings.connections.legacyKey", "이 서버에는 이전 판단 설정의 키도 있습니다. 이전 버전이나 재시작 전 zo는 그 키를 사용할 수 있으므로 업데이트 후 다시 시작하세요.")));
    }
    return panel;
  },
  renderRoles() {
    for (const holder of document.querySelectorAll("[data-connection-roles]")) {
      const roles = holder.dataset.connectionRoles === "judgment" ? ["judgment"] : ["coding", "judgment"];
      holder.replaceChildren(...roles.map(role => this.rolePanel(role)));
    }
    const comparison = el("connection-comparison");
    if (comparison) {
      const facts = this.node("table", "connection-comparison");
      facts.append(this.node("caption", "settings-label", t("settings.connections.compare", "배정 비교")));
      const head = this.node("thead"), titles = this.node("tr");
      for (const label of [t("settings.connections.usesTitle", "용도별 모델"), t("settings.connections.active", "현재 사용"), t("settings.connections.proposed", "배정 초안")]) {
        const cell = this.node("th", "", label); cell.scope = "col"; titles.append(cell);
      }
      head.append(titles); facts.append(head);
      const body = this.node("tbody");
      for (const role of ["coding", "judgment"]) {
        const row = this.node("tr"), title = this.node("th", "", this.roleName(role)); title.scope = "row";
        row.append(title, this.node("td", "", this.activeName(role)), this.node("td", "", this.referenceName(this.selected(role)))); body.append(row);
      }
      facts.append(body);
      comparison.replaceChildren(facts);
    }
  },
  render() {
    this.renderRoles();
    const list = el("router-provider-list"); if (!list) return;
    const rows = (this.state?.connections ?? []).map(row => ({ ...row, kind: this.location(row.provider.base_url), models: row.provider.models ?? [] }));
    const managed = managedJudgmentController.snapshot?.models?.filter(model => managedJudgmentController.held(model)) ?? [];
    for (const model of managed) rows.push({ id: `managed:${model.id}`, kind: "managed", provider: { name: model.label }, models: [model.id], uses: typesafeState?.provider?.kind === "local" && typesafeState.provider.model === model.id ? ["judgment"] : [], status: managedJudgmentController.modelState(model) });
    rows.push({ id: "jev", kind: "jev", provider: { name: "Jev" }, models: typesafeState?.jevProvider?.model ? [typesafeState.jevProvider.model] : [], uses: typesafeState?.provider?.kind === "jev" ? ["judgment"] : [], status: typesafeState?.provider?.kind === "jev" ? typesafeState.keySaved ? t("settings.connections.registered", "등록됨 · 응답 미확인") : t("settings.connections.needsKey", "인증 키 필요") : t("settings.connections.authUnknown", "인증 상태 미확인") });
    list.replaceChildren(...rows.map(row => this.connectionRow(row)));
    el("router-provider-count").textContent = String(rows.length);
    el("router-provider-note").textContent = this.state?.connections.length ? "" : t("settings.apiRouters.noConnected", "연결된 API 라우터가 없습니다.");
  },
  location(address) {
    try { const host = new URL(address).hostname; return ["localhost", "127.0.0.1", "[::1]"].includes(host) ? "local" : "remote"; }
    catch { return "remote"; }
  },
  connectionRow(row) {
    const line = this.node("div", "connection-row"); line.dataset.connectionId = row.id;
    if (!["managed", "jev"].includes(row.kind)) line.classList.add("account-row");
    const copy = this.node("div", "connection-row-copy");
    const mark = this.node("div", "connection-icon");
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg"); svg.classList.add("icon"); svg.setAttribute("aria-hidden", "true");
    const use = document.createElementNS("http://www.w3.org/2000/svg", "use"); use.setAttribute("href", row.kind === "managed" ? "#i-terminal" : row.kind === "jev" ? "#i-bot" : "#i-server"); svg.append(use); mark.append(svg);
    const location = row.kind === "managed" ? t("settings.connections.locationManaged", "이 컴퓨터 · 앱에서 관리")
      : row.kind === "local" ? t("settings.connections.locationLocal", "이 컴퓨터 · 실행 중인 로컬 서버")
        : row.kind === "jev" ? t("settings.connections.locationJev", "Jev 서비스") : t("settings.connections.locationRemote", "외부 서버");
    copy.append(this.node("strong", "agent-row-name", row.provider.name), this.node("p", "settings-row-desc agent-row-cmd", `${location} · ${t("settings.connections.modelCount", "모델 {{count}}개", { count: row.models.length })}`));
    const status = row.status ?? (row.keyAvailable ? t("settings.connections.registered", "등록됨 · 응답 미확인") : t("settings.connections.needsKey", "인증 키 필요"));
    const uses = row.uses.map(role => this.roleName(role)).join(" · ") || t("settings.connections.noUse", "용도 미선택");
    const badges = this.node("div", "connection-badges");
    badges.append(this.node("span", "settings-badge", status), this.node("span", "settings-badge", uses));
    copy.append(badges);
    const actions = this.node("div", "settings-action-row");
    const expanded = this.expanded.has(row.id);
    const models = this.button(t("settings.connections.viewModels", "모델 보기"), () => { expanded ? this.expanded.delete(row.id) : this.expanded.add(row.id); this.render(); });
    models.dataset.connectionModels = ""; models.setAttribute("aria-expanded", String(expanded));
    const configure = () => {
      if (row.kind === "managed" || row.kind === "jev") {
        showSettingsPane("judgment-models");
        if (row.kind === "jev") { el("judgment-connection-settings").open = true; el("judgment-provider-kind").value = "jev"; judgmentProviderController.changeKind(); }
        else el("managed-judgment-panel").scrollIntoView({ block: "nearest" });
      } else this.edit(row);
    };
    actions.append(models, this.button(t("settings.connections.configure", "설정"), configure));
    if (!["managed", "jev"].includes(row.kind)) {
      const remove = this.button(t("settings.connections.delete", "삭제"), () => void this.remove(row)); remove.classList.add("account-drop"); actions.append(remove);
    }
    line.append(mark, copy, actions);
    if (this.deletion?.id === row.id) {
      const note = this.node("div", "connection-delete-note settings-notice"); note.setAttribute("role", "status");
      note.append(this.node("p", "", this.deletion.message));
      const actions = this.node("div", "settings-action-row");
      if (row.uses.length) actions.append(this.button(t("settings.connections.usage", "모델 사용"), () => this.show("usage")));
      else if (this.deletion.confirm) actions.append(this.button(t("settings.connections.delete", "삭제"), () => void this.remove(row, true)));
      actions.append(this.button(t("settings.connections.cancel", "취소"), () => { this.deletion = null; this.render(); }));
      note.append(actions); line.append(note);
    }
    if (expanded) {
      const holder = this.node("div", "connection-models");
      for (const model of row.models) {
        const id = typeof model === "string" ? model : model.id;
        const item = this.node("div", "connection-model"); item.append(this.node("span", "", id));
        if (!["managed", "jev"].includes(row.kind)) item.append(this.button(t("settings.connections.usage", "모델 사용"), () => { this.change("judgment", { id: row.id, generation: row.generation, model: id, protocol: "openai_json" }); this.show("usage"); }));
        holder.append(item);
      }
      if (!row.models.length) holder.append(this.node("p", "settings-hint", t("settings.connections.unselected", "선택하지 않음")));
      line.append(holder);
    }
    return line;
  },
  async run(role, action) {
    if (this.busy) return;
    const reference = this.selected(role); if (action !== "clear" && !reference) return;
    const captured = JSON.stringify(reference);
    const identity = this.state?.activeProviderIdentity;
    this.busy = true;
    this.results.set(role, { message: t("settings.connections.working", "실제 연결을 확인하고 있습니다…") }); this.renderRoles();
    try {
      if (action === "check") {
        const result = await invoke("check_model_connection", { reference, expectedProviderIdentity: this.state.activeProviderIdentity });
        if (captured !== JSON.stringify(this.selected(role)) || identity !== this.state?.activeProviderIdentity || this.state?.connections.find(row => row.id === reference.id)?.generation !== reference.generation) return;
        this.results.set(role, { failed: !result.preflight.answered, legacy: result.legacyKeyPresent,
          reason: result.preflight.failure,
          checked: result.preflight.answered, reference: captured, identity,
          message: result.preflight.answered ? t("settings.connections.checked", "선택 · 점수 · 참/거짓 응답 형식 확인됨") : this.failure(result.preflight.failure) });
      } else {
        this.state = await invoke(action === "clear" ? "clear_model_connection" : "select_model_connection", {
          role, reference, expectedReference: this.state?.[role] ?? null, expectedProviderIdentity: this.state.activeProviderIdentity,
        }, { hand: handNow() });
        this.drafts.delete(role);
        this.results.set(role, { message: t("settings.connections.applied", "저장된 설정을 적용했습니다.") });
        await refreshTypeSafe();
        void this.sync();
      }
    } catch (error) { this.results.set(role, { failed: true, reason: typeof error === "string" ? error : error?.message, message: this.failure(error) }); }
    finally { this.busy = false; this.render(); }
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
    el("connection-location").value = this.location(row.provider.base_url);
    el("connection-auth").value = row.provider.requires_auth ? row.keyAvailable ? "saved" : "new" : "none";
    this.addFields();
    say(el("connection-form-title"), () => t("settings.connections.configure", "설정"));
    this.show("add");
    el("router-name-input").focus();
    el("router-card").scrollIntoView({ block: "nearest" });
  },
  async remove(row, confirmed = false) {
    if (row.uses.length) {
      this.deletion = { id: row.id, message: `${row.uses.map(role => this.roleName(role)).join(" · ")} · ${t("settings.connections.inUse", "이 연결을 쓰는 용도를 먼저 해제하세요. 이름과 모델 목록은 그대로 수정할 수 있습니다.")}` };
      this.render(); return;
    }
    if (!confirmed) { this.deletion = { id: row.id, confirm: true, message: t("settings.connections.deleteConfirm", "연결과 저장된 인증 참조를 삭제할까요? 서버와 모델 파일은 삭제하지 않습니다.") }; this.render(); return; }
    try { await invoke("remove_api_router", { name: row.provider.name, sourceId: row.id, expectedGeneration: row.generation }, { hand: handNow() }); this.deletion = null; await this.refresh(); }
    catch (error) { this.deletion = { id: row.id, message: this.failure(error) }; this.render(); }
  },
};

el("connections-managed")?.addEventListener("click", () => showSettingsPane("judgment-models"));
el("connections-decisions")?.addEventListener("click", () => openJevView());

for (const button of document.querySelectorAll("[data-connection-nav]")) button.addEventListener("click", () => modelConnectionsController.show(button.dataset.connectionNav));
for (const button of document.querySelectorAll("[data-connection-scenario]")) button.addEventListener("click", () => modelConnectionsController.scenario(button.dataset.connectionScenario));
el("connections-add")?.addEventListener("click", () => modelConnectionsController.add());
el("connection-location")?.addEventListener("change", () => { invalidateRouterProbe(); modelConnectionsController.addFields(); });
el("connection-auth")?.addEventListener("change", () => { invalidateRouterProbe(); modelConnectionsController.addFields(); });
el("connection-open-service")?.addEventListener("click", () => {
  showSettingsPane("judgment-models");
  if (el("connection-location").value === "jev") {
    el("judgment-connection-settings").open = true;
    el("judgment-provider-kind").value = "jev";
    judgmentProviderController.changeKind();
    el("judgment-connection-settings").scrollIntoView({ block: "nearest" });
  } else el("managed-judgment-panel").scrollIntoView({ block: "nearest" });
});
