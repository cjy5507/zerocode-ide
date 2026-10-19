export async function runSettingsOpenAi({ test, assert, assertEqual, backend, withJudgmentStatus, setQualityLocale }) {
  const originalInvoke = backend.invoke;
  backend.invoke = async function (windowId, command, args = {}) {
    if (args.provider?.protocol === "openai_json" && ["judgment_models", "set_judgment_provider"].includes(command)) {
      const address = new URL(args.provider.baseUrl);
      if (args.provider.kind !== "local" || !["localhost", "127.0.0.1", "[::1]"].includes(address.hostname)) {
        this.record(windowId, command, args);
        throw "provider_not_loopback";
      }
      if (command === "judgment_models" && this.openAiListFailure) {
        this.record(windowId, command, args);
        throw this.openAiListFailure;
      }
      if (command === "set_judgment_provider") {
        this.record(windowId, command, args);
        if (args.expectedProviderIdentity !== this.typesafeSettings().providerIdentity) throw "settings_changed";
        this.zoSettings.smart.judgmentProvider = structuredClone(args.provider);
        return this.typesafeSettings();
      }
    }
    return originalInvoke.call(this, windowId, command, args);
  };

  const fixture = async (run) => {
    const previousModels = backend.judgmentModels;
    backend.judgmentModels = [{ id: "acme/decision-a" }, { id: "acme/decision-b" }];
    try {
      return await withJudgmentStatus(run);
    } finally {
      backend.judgmentModels = previousModels;
      delete backend.openAiListFailure;
      delete backend.judgmentModelsWait;
      delete backend.judgmentPreflight;
      delete backend.judgmentPreflightWait;
    }
  };
  const open = async (page) => {
    assertEqual(await page.locator("#judgment-openai-source").count(), 1, "local structured-server connection has no visible entry");
    await page.click("#judgment-openai-source");
    await page.waitForFunction(() => judgmentProviderController.draft?.protocol === "openai_json");
  };
  const listed = async (page) => {
    await page.click("#judgment-refresh-models");
    await page.waitForFunction(() => judgmentProviderController.modelIds.includes("acme/decision-a"));
    await page.selectOption("#judgment-listed-model", "acme/decision-a");
  };

  try {
    await test("로컬 JSON 연결은 기존 서버 옆에 있고 목록 선택 전에는 적용하지 않는다", () => fixture(async (page, active) => {
      const before = backend.calls.length;
      await open(page);
      assertEqual(await page.evaluate(() => judgmentProviderController.draft.kind), "local", "structured server is not local");
      assert(await page.locator("#judgment-save-provider").isDisabled(), "an unlisted model can be applied");
      assert(await page.locator("#typesafe-model-input").getAttribute("readonly") !== null, "structured model selection allows an invented model id");
      assertEqual(backend.typesafeSettings().provider, active, "opening the form applied a model");
      assert(!backend.calls.slice(before).some((call) => ["preflight_judgment_provider", "set_judgment_provider", "managed_judgment_start", "managed_judgment_install"].includes(call.command)), "opening the form started or applied a model");
      return "visible connection · explicit model list · no install/start/apply";
    }));

    await test("로컬 JSON 모델은 목록과 응답 확인 뒤에만 적용하고 미검증이라고 남긴다", () => fixture(async (page) => {
      await open(page);
      const before = backend.calls.length;
      await listed(page);
      const listing = backend.calls.slice(before).find((call) => call.command === "judgment_models");
      assertEqual(listing.args.provider.protocol, "openai_json", "the model list used the native protocol");
      assert(!backend.calls.slice(before).some((call) => call.command === "preflight_judgment_provider"), "choosing a model sent a probe");
      await page.click("#judgment-save-provider");
      await page.waitForFunction(() => !typesafeBusy && judgmentProviderController.state?.provider.protocol === "openai_json");
      assertEqual(backend.typesafeSettings().provider.model, "acme/decision-a", "the listed selection was not applied");
      assertEqual(backend.calls.slice(before).filter((call) => call.command === "preflight_judgment_provider").length, 1, "apply did not make exactly one explicit probe");
      const expected = await page.evaluate(() => t("settings.openai.unverified", "검증 안 된 사람 지정 모델"));
      assert((await page.locator("#judgment-active-model").textContent()).includes(expected), "the current model lost the unverified marker");
      assert(!backend.calls.slice(before).some((call) => /^managed_judgment_(install|start|apply|prepare)/.test(call.command)), "connecting a user server used managed model lifecycle commands");
      return "listed model · one probe · explicit apply · persistent unverified marker";
    }));

    await test("꺼진 로컬 서버와 스키마 실패는 현재 판단 모델을 보존한다", () => fixture(async (page, active) => {
      await open(page);
      backend.openAiListFailure = "transport";
      await page.click("#judgment-refresh-models");
      await page.waitForFunction(() => !judgmentProviderController.loading);
      const unavailable = await page.evaluate(() => t("settings.openai.serverUnavailable", "서버에 연결하지 못했습니다. LM Studio나 Ollama에서 서버를 직접 켠 뒤 주소를 확인하세요. 앱은 서버를 설치하거나 시작하지 않습니다."));
      assertEqual(await page.locator("#judgment-model-status").textContent(), unavailable, "server-off guidance is not explicit");
      assertEqual(backend.typesafeSettings().provider, active, "failed listing changed the model");
      delete backend.openAiListFailure;
      await listed(page);
      backend.judgmentPreflight = { candidateIdentity: "fixture-candidate", activeProviderIdentity: backend.typesafeSettings().providerIdentity, answered: false, failure: "schema", elapsedMs: 1 };
      const before = backend.calls.length;
      await page.click("#judgment-save-provider");
      await page.waitForFunction(() => !typesafeBusy);
      assertEqual(backend.typesafeSettings().provider, active, "invalid structured output changed the model");
      assert(!backend.calls.slice(before).some((call) => call.command === "set_judgment_provider"), "an invalid response reached apply");
      return "server-off guidance · schema refusal · current model kept";
    }));

    await test("늦게 온 JSON 서버 목록은 바뀐 연결 방식을 덮지 않는다", () => fixture(async (page, active) => {
      await open(page);
      let finish;
      backend.judgmentModelsWait = new Promise((resolve) => { finish = resolve; });
      const before = backend.calls.length;
      await page.click("#judgment-refresh-models");
      await backend.waitForCall("judgment-status", "judgment_models", before);
      await page.click("#judgment-local-manual");
      finish([{ id: "stale-model" }]);
      await page.waitForFunction(() => !judgmentProviderController.loading);
      assertEqual(await page.evaluate(() => judgmentProviderController.draft.protocol), "system_one", "the stale list restored its protocol");
      assertEqual(await page.evaluate(() => judgmentProviderController.modelIds), [], "a stale list populated the new connection");
      assertEqual(backend.typesafeSettings().provider, active, "a stale list changed the current model");
      return "stale list discarded across protocols";
    }));

    await test("다른 창이 바꾼 판단 모델은 늦은 JSON 확인이 덮지 않는다", () => fixture(async (page, active) => {
      await open(page);
      await listed(page);
      let finish;
      backend.judgmentPreflightWait = new Promise((resolve) => { finish = resolve; });
      const identity = backend.typesafeSettings().providerIdentity;
      const before = backend.calls.length;
      await page.click("#judgment-save-provider");
      await backend.waitForCall("judgment-status", "preflight_judgment_provider", before);
      const next = { ...active, model: "acme/changed-by-other-window" };
      backend.zoSettings.smart.judgmentProvider = next;
      finish({ candidateIdentity: "fixture-candidate", activeProviderIdentity: identity, answered: true, model: "acme/decision-a", elapsedMs: 1 });
      await page.waitForFunction(() => !typesafeBusy);
      assertEqual(backend.typesafeSettings().provider, next, "a stale check overwrote another window");
      assert(!backend.calls.slice(before).some((call) => call.command === "set_judgment_provider"), "a stale check reached apply");
      return "concurrent model change kept";
    }));

    await test("JSON 연결의 미검증 안내는 다섯 언어와 좁은 화면에서 읽힌다", () => fixture(async (page) => {
      await open(page);
      try {
        await page.setViewportSize({ width: 360, height: 720 });
        for (const locale of ["ko", "en", "ja", "zh", "es"]) {
          await setQualityLocale(page, locale);
          const expected = await page.evaluate(() => t("settings.openai.unverified", "검증 안 된 사람 지정 모델"));
          assert(expected && expected !== "settings.openai.unverified", `missing ${locale} translation`);
          assert((await page.locator("#judgment-openai-notice").textContent()).includes(expected), `${locale} lost the unverified notice`);
        }
        await page.locator("#judgment-refresh-models").focus();
        assertEqual(await page.evaluate(() => document.activeElement.id), "judgment-refresh-models", "the list action cannot receive keyboard focus");
        const button = await page.locator("#judgment-local-openai").boundingBox();
        assert(button && button.x >= 0 && button.x + button.width <= 361, "the local-server action overflows the narrow pane");
      } finally {
        await setQualityLocale(page, "ko");
      }
      return "five locales · keyboard focus · narrow pane";
    }));
  } finally {
    backend.invoke = originalInvoke;
  }
}
