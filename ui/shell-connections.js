/* A saved connection is shared; choosing a role never copies its credential. */
const modelConnectionsController = {
  state: null, sequence: 0, busy: false, editing: null,
  drafts: new Map(), results: new Map(),

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
      // The judgment pane's one current server read (refreshTypeSafe) feeds the
      // managed models listed here; this list reads only the connections.
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
            try { await invoke("clear_model_connection", { role: "judgment" }); await refreshTypeSafe(); await this.refresh(); }
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
  roleName(role) { return role === "coding" ? t("settings.connections.coding", "코딩 · 대화") : t("settings.judgment.title", "판단 모델"); },
  rolePanel(role) {
    const selected = this.selected(role);
    const panel = this.node("section", "connection-role");
    panel.append(this.node("h3", "settings-subhead-title", this.roleName(role)));
    const active = this.state?.[role];
    const row = this.state?.connections.find(row => row.id === active?.id);
    const activeName = active && row ? `${row.provider.name} / ${active.model}` : role === "coding" ? this.state?.codingModel : typesafeState?.provider?.model;
    panel.append(this.node("p", "settings-row-desc", t("settings.connections.current", "현재: {{model}}", { model: activeName || t("settings.connections.unselected", "선택하지 않음") })));
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
    }
    const actions = this.node("div", "settings-action-row");
    if (role === "judgment") {
      const check = this.button(t("settings.connections.check", "세 유형 확인"), () => void this.run(role, "check"));
      check.disabled ||= !selected; actions.append(check);
    }
    const apply = this.button(role === "judgment" ? t("settings.connections.applyJudge", "확인하고 사용") : t("settings.connections.applyCoding", "기본 코딩 모델로 사용"), () => void this.run(role, "apply"), true);
    apply.disabled ||= !selected;
    const clear = this.button(t("settings.connections.clear", "용도 해제 · 이전 선택 복원"), () => void this.run(role, "clear"));
    clear.disabled ||= !active;
    actions.append(apply, clear); panel.append(actions);
    if (role === "coding") panel.append(this.node("p", "settings-row-desc", t("settings.connections.codingHint", "새 zo 세션의 기본 모델입니다. 실행 중인 세션과 명시적으로 지정한 모델은 유지합니다.")));
    const result = this.results.get(role);
    if (result) {
      const status = this.node("p", "connection-result", result.message); status.setAttribute("role", "status"); status.dataset.state = result.failed ? "failed" : "ready"; panel.append(status);
      if (result.legacy || role === "judgment" && this.state?.legacyKeyPresent) panel.append(this.node("p", "settings-notice", t("settings.connections.legacyKey", "이 서버에는 이전 판단 설정의 키도 있습니다. 이전 버전이나 재시작 전 zo는 그 키를 사용할 수 있으므로 업데이트 후 다시 시작하세요.")));
    }
    return panel;
  },
  renderRoles() {
    for (const holder of document.querySelectorAll("[data-connection-roles]")) {
      const roles = holder.dataset.connectionRoles === "judgment" ? ["judgment"] : ["coding", "judgment"];
      holder.replaceChildren(...roles.map(role => this.rolePanel(role)));
    }
  },
  render() {
    this.renderRoles();
    const list = el("router-provider-list"); if (!list) return;
    list.replaceChildren();
    for (const row of this.state?.connections ?? []) {
      const line = this.node("div", "connection-row agent-row account-row");
      const body = this.node("div", "connection-row-copy agent-row-body account-body");
      body.append(this.node("strong", "agent-row-name", row.provider.name), this.node("p", "settings-row-desc agent-row-cmd", t("settings.apiRouters.rowSummary", "{{base}} · 모델 {{count}}개", { base: row.provider.base_url, count: row.provider.models?.length ?? 0 })));
      const state = row.keyAvailable ? t("settings.connections.registered", "등록됨 · 응답 미확인") : t("settings.connections.needsKey", "인증 키 필요");
      const uses = row.uses.map(role => this.roleName(role)).join(" · ") || t("settings.connections.noUse", "용도 미선택");
      body.append(this.node("p", "settings-row-desc", `${state} · ${uses}`));
      const actions = this.node("div", "settings-action-row");
      const remove = this.button(t("settings.connections.delete", "삭제"), () => void this.remove(row));
      remove.classList.add("account-drop");
      actions.append(this.button(t("settings.connections.edit", "수정"), () => this.edit(row)), remove);
      line.append(body, actions); list.append(line);
    }
    const services = this.node("div", "connection-services");
    const jev = this.node("div", "connection-row");
    jev.append(this.node("strong", "", "Jev"), this.node("span", "settings-row-desc", typesafeState?.jevProvider?.model ?? ""), this.button(t("settings.connections.edit", "수정"), () => {
      showSettingsPane("judgment-models");
      el("judgment-provider-kind").value = "jev";
      judgmentProviderController.changeKind();
      el("judgment-provider-kind").focus();
    }));
    services.append(jev);
    for (const model of managedJudgmentController.snapshot?.models?.filter(model => managedJudgmentController.held(model)) ?? []) {
      const line = this.node("div", "connection-row");
      const copy = this.node("div", "connection-row-copy");
      copy.append(this.node("strong", "", model.label), this.node("p", "settings-row-desc", managedJudgmentController.modelState(model)));
      line.append(copy, this.button(t("settings.connections.managed", "앱 관리 로컬 모델 · Jev"), () => {
        showSettingsPane("judgment-models");
        el("managed-judgment-panel").scrollIntoView({ block: "nearest" });
      }));
      services.append(line);
    }
    list.append(services);
    el("router-provider-count").textContent = String(this.state?.connections.length ?? 0);
    el("router-provider-note").textContent = this.state?.connections.length ? "" : t("settings.apiRouters.noConnected", "연결된 API 라우터가 없습니다.");
  },
  async run(role, action) {
    if (this.busy) return;
    const reference = this.selected(role); if (action !== "clear" && !reference) return;
    const captured = JSON.stringify(reference);
    this.busy = true;
    this.results.set(role, { message: t("settings.connections.working", "실제 연결을 확인하고 있습니다…") }); this.renderRoles();
    try {
      if (action === "check") {
        const result = await invoke("check_model_connection", { reference, expectedProviderIdentity: this.state.activeProviderIdentity });
        if (captured !== JSON.stringify(this.selected(role))) return;
        this.results.set(role, { failed: !result.preflight.answered, legacy: result.legacyKeyPresent,
          message: result.preflight.answered ? t("settings.connections.checked", "선택 · 점수 · 참/거짓 응답 형식 확인됨") : this.failure(result.preflight.failure) });
      } else {
        this.state = await invoke(action === "clear" ? "clear_model_connection" : "select_model_connection", {
          role, reference, expectedReference: this.state?.[role] ?? null, expectedProviderIdentity: this.state.activeProviderIdentity,
        });
        this.drafts.delete(role);
        this.results.set(role, { message: t("settings.connections.applied", "저장된 설정을 적용했습니다.") });
        await refreshTypeSafe();
        void this.sync();
      }
    } catch (error) { this.results.set(role, { failed: true, message: this.failure(error) }); }
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
    showSettingsPane("api-routers");
    el("router-name-input").focus();
    el("router-card").scrollIntoView({ block: "nearest" });
  },
  async remove(row) {
    if (row.uses.length) {
      showError(t("settings.connections.inUse", "이 연결을 쓰는 용도를 먼저 해제하세요. 이름과 모델 목록은 그대로 수정할 수 있습니다."));
      document.querySelector('[data-connection-roles="all"]')?.scrollIntoView({ block: "nearest" }); return;
    }
    try { await invoke("remove_api_router", { name: row.provider.name, sourceId: row.id, expectedGeneration: row.generation }); await this.refresh(); }
    catch (error) { showError(this.failure(error)); }
  },
};

el("connections-managed")?.addEventListener("click", () => showSettingsPane("judgment-models"));
el("connections-decisions")?.addEventListener("click", () => openJevView());
