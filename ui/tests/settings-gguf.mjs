import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";

export async function runSettingsGguf({ test, assert, assertEqual, backend, withJudgmentStatus, setQualityLocale }) {
  const original = backend.invoke;
  backend.invoke = async function (windowId, command, args = {}) {
    const answer = await original.call(this, windowId, command, args);
    if (["managed_judgment_start", "managed_judgment_switch_model"].includes(command)) {
      const model = this.managedModelState?.models.find((model) => model.id === args.modelId);
      if (model?.engine === "llama_cpp" && this.managedModelState.server.modelId === model.id) {
        const held = this.managedModelState;
        held.server.provider.protocol = "openai_json";
        held.server.inputTemplate = model.inputTemplate;
        if (command === "managed_judgment_switch_model" && held.appliedModelId === model.id) this.zoSettings.smart.judgmentProvider = structuredClone(held.server.provider);
        return this.managedJudgmentSettings();
      }
    }
    return answer;
  };

  const fixture = async (run) => withJudgmentStatus(async (page, active) => {
    const snapshot = backend.managedJudgmentSettings();
    const model = {
      ...structuredClone(snapshot.models[0]), id: "native-fixture", revision: "fixture-gguf-revision",
      label: "Selected GGUF", engine: "llama_cpp", protocol: "openai_json", nativeFormat: "gguf-v3", inputTemplate: "chatml",
      quality: "source_inspected_unverified", installed: false,
      local: { supported: true, downloadOnly: false, reason: null, inputs: ["text", "json"], unsupportedInputs: ["images", "videos"] },
      runtime: { state: "missing", downloadBytes: 37351536, pythonRequirement: "3.12", reason: null },
      existingServer: { supported: false, protocol: "openai_json", modelId: null, requiresCheck: true },
    };
    backend.managedSourceResult = {
      inspectionId: "fixture-gguf-inspection", instanceId: snapshot.instanceId, catalogFingerprint: snapshot.catalogFingerprint,
      model, local: model.local, existingServer: model.existingServer, readyForInstall: true, readyForDownload: true, reason: null,
      defaultDestinationDirectory: "/models/managed", allowsDestinationSelection: true, sourceKind: "local_file",
      downloadBytes: 0, copyBytes: 1024, runtimeDownloadBytes: model.runtime.downloadBytes,
    };
    await page.evaluate(() => {
      window.openPathBrowser = async (options) => {
        window.__GGUF_PICKER_OPTIONS__ = options;
        return ["/models/selected.gguf"];
      };
    });
    return run(page, active, model);
  });
  const chooseFile = async (page) => {
    assertEqual(await page.locator("#judgment-source-gguf").count(), 1, "the model loader has no GGUF file action");
    await page.click("#judgment-source-gguf");
    await page.waitForFunction(() => judgmentModelSourceController.inspection?.model.engine === "llama_cpp" || judgmentModelSourceController.problem);
  };

  try {
    await test("GGUF 파일 선택은 파일 한 개만 검사하고 원래 판단 모델을 유지한다", () => fixture(async (page, active) => {
      const before = backend.calls.length;
      await chooseFile(page);
      assertEqual(await page.evaluate(() => ({ mode: window.__GGUF_PICKER_OPTIONS__.mode, multiple: window.__GGUF_PICKER_OPTIONS__.multiple, system: window.__GGUF_PICKER_OPTIONS__.system })), { mode: "attach", multiple: false, system: true }, "GGUF did not request the existing single-file picker");
      const inspected = backend.calls.slice(before).find((call) => call.command === "managed_judgment_inspect_source");
      assertEqual(inspected.args.source, { kind: "local_file", file: "/models/selected.gguf" }, "the chosen file was converted into a directory import");
      assertEqual(backend.typesafeSettings().provider, active, "inspection changed the active model");
      assert(!backend.calls.slice(before).some((call) => /^managed_judgment_(install|prepare|start|apply|switch)/.test(call.command)), "file selection downloaded or started a runtime");
      const words = await page.evaluate(() => t("settings.gguf.unverified", "사람이 지정한 검증 안 된 모델"));
      assert((await page.locator("#judgment-source-model-support").textContent()).includes(words), "the selected model is presented as verified");
      return "one file · inspect only · active provider kept · model unverified";
    }));

    await test("GGUF 불러오기는 실행 환경과 확인 뒤에만 적용하며 내장 형식을 표시한다", () => fixture(async (page) => {
      await chooseFile(page);
      const before = backend.calls.length;
      await page.click("#judgment-source-use");
      await page.waitForFunction(() => judgmentModelSourceController.flow?.done || judgmentModelSourceController.flow?.failed);
      assert(await page.evaluate(() => judgmentModelSourceController.flow.done), "GGUF load did not finish");
      const commands = backend.calls.slice(before).map((call) => call.command);
      assert(commands.includes("managed_judgment_install_source"), "GGUF was not copied into owned storage");
      assert(commands.includes("managed_judgment_prepare_model_runtime"), "the selected engine was not prepared");
      assert(commands.includes("managed_judgment_switch_model") || commands.includes("managed_judgment_check"), "application did not verify the native response");
      assertEqual(backend.typesafeSettings().provider.protocol, "openai_json", "GGUF was applied as a native System One head");
      const words = await page.evaluate(() => t("settings.gguf.unverified", "사람이 지정한 검증 안 된 모델"));
      assert((await page.locator("#judgment-active-model").textContent()).includes(words), "the applied model lost its unverified marker");
      return "owned copy · selected runtime · verified response · explicit unverified model";
    }));

    await test("GGUF 파일 여러 개를 고르면 임의의 첫 파일을 검사하지 않는다", () => fixture(async (page, active) => {
      await page.evaluate(() => { window.openPathBrowser = async () => ["/models/first.gguf", "/models/second.gguf"]; });
      const before = backend.calls.length;
      assertEqual(await page.locator("#judgment-source-gguf").count(), 1, "the model loader has no GGUF file action");
      await page.click("#judgment-source-gguf");
      await page.waitForFunction(() => !judgmentModelSourceController.working && judgmentModelSourceController.kind === "local_file");
      assertEqual(await page.evaluate(() => judgmentModelSourceController.source()), null, "a multiple selection kept an arbitrary file");
      assert(!backend.calls.slice(before).some((call) => call.command === "managed_judgment_inspect_source"), "multiple GGUF files reached inspection");
      assertEqual(backend.typesafeSettings().provider, active);
      return "multiple files refused · no arbitrary selection · active provider kept";
    }));

    await test("실행 검증 전 GGUF는 파일만 복사하고 실행 환경을 설치하지 않는다", () => fixture(async (page, active) => {
      const inspection = backend.managedSourceResult;
      Object.assign(inspection.local, { supported: false, downloadOnly: true, reason: "engine_unverified" });
      inspection.readyForInstall = false;
      inspection.model.runtime.state = "unverified";
      inspection.model.runtime.reason = "engine_unverified";
      await chooseFile(page);
      assert(!(await page.locator("#judgment-source-model-support").textContent()).includes("CPython"), "download-only selection suggests runtime preparation");
      const before = backend.calls.length;
      await page.click("#judgment-source-use");
      await page.waitForFunction(() => judgmentModelSourceController.flow?.done || judgmentModelSourceController.flow?.failed);
      assert(await page.evaluate(() => judgmentModelSourceController.flow.done), "download-only GGUF copy failed");
      assert(!backend.calls.slice(before).some((call) => /^managed_judgment_(prepare|start|apply|switch|check)/.test(call.command)), "an unverified runtime was prepared or started");
      assertEqual(backend.typesafeSettings().provider, active);
      return "owned copy only · no runtime action · active provider kept";
    }));

    await test("지원하지 않는 GGUF 입력 형식은 현재 모델을 바꾸거나 실행하지 않는다", () => fixture(async (page, active) => {
      backend.managedSourceError = "model_format_not_supported";
      const before = backend.calls.length;
      await chooseFile(page);
      assertEqual(backend.typesafeSettings().provider, active, "an unsupported file replaced the active provider");
      assert(await page.locator("#judgment-source-use").isHidden(), "an unsupported file offers execution");
      assert(!backend.calls.slice(before).some((call) => /^managed_judgment_(install|prepare|start|apply|switch)/.test(call.command)), "unsupported format reached runtime operations");
      return "unsupported format · no download or execution · old provider kept";
    }));

    await test("Hugging Face GGUF 주소 확인은 특정 파일만 고정하고 아직 받지 않는다", () => fixture(async (page, active) => {
      backend.managedSourceResult.sourceKind = "hugging_face_file";
      backend.managedSourceResult.downloadBytes = 1024;
      backend.managedSourceResult.copyBytes = 0;
      await page.click("#judgment-source-hub");
      const address = "https://huggingface.co/acme/fixture/blob/main/selected.gguf";
      await page.fill("#judgment-source-repository", address);
      const before = backend.calls.length;
      await page.click("#judgment-source-inspect");
      await page.waitForFunction(() => judgmentModelSourceController.inspection?.sourceKind === "hugging_face_file");
      assertEqual(backend.calls.slice(before).find((call) => call.command === "managed_judgment_inspect_source").args.source, { kind: "hugging_face_file", url: address }, "the address was not pinned as one file");
      assertEqual(backend.typesafeSettings().provider, active, "inspecting an address replaced the active provider");
      assert(!backend.calls.slice(before).some((call) => call.command === "managed_judgment_install_source"), "address inspection started the weight download");
      return "specific file URL · metadata only · no implicit download";
    }));

    await test("GGUF 안내는 다섯 언어와 좁은 화면에서 파일·추가 Python·입력 형식을 설명한다", () => fixture(async (page) => {
      await chooseFile(page);
      try {
        await page.setViewportSize({ width: 720, height: 480 });
        for (const locale of ["ko", "en", "ja", "zh", "es"]) {
          await setQualityLocale(page, locale);
          const words = await page.evaluate(() => t("settings.gguf.unverified", "사람이 지정한 검증 안 된 모델"));
          const support = await page.locator("#judgment-source-model-support").textContent();
          assert(support.includes(words) && support.includes("CPython") && support.includes("chatml"), `${locale} omits model status, Python or the selected native template`);
        }
        await page.locator("#judgment-source-browse").focus();
        assertEqual(await page.evaluate(() => document.activeElement.id), "judgment-source-browse");
        const button = await page.locator("#judgment-source-gguf").boundingBox();
        const pane = await page.locator('.settings-pane[data-pane="judgment-models"]').boundingBox();
        const layout = await page.evaluate(() => {
          const measure = (selector) => {
            const element = document.querySelector(selector);
            if (!element) return null;
            const box = element.getBoundingClientRect();
            const style = getComputedStyle(element);
            return { xPx: box.x, yPx: box.y, widthPx: box.width, heightPx: box.height, rightPx: box.right,
              scrollWidthPx: element.scrollWidth, clientWidthPx: element.clientWidth,
              whiteSpace: style.whiteSpace, flexWrap: style.flexWrap, minWidth: style.minWidth, overflowX: style.overflowX };
          };
          return { viewportWidthPx: innerWidth, viewportHeightPx: innerHeight, scrollXPx: scrollX,
            locale, rootZoom: getComputedStyle(document.documentElement).zoom,
            button: measure("#judgment-source-gguf"), choices: measure(".judgment-source-choices"),
            source: measure(".judgment-model-source"), pane: measure('.settings-pane[data-pane="judgment-models"]'),
            rail: measure(".settings-rail"), view: measure(".settings-view") };
        });
        console.log(`GGUF_LAYOUT ${JSON.stringify({ ...layout, locatorBox: button, locatorPane: pane })}`);
        if (process.env.GGUF_LAYOUT_EVIDENCE) {
          const folder = resolve(process.env.GGUF_LAYOUT_EVIDENCE);
          await mkdir(folder, { recursive: true });
          await writeFile(resolve(folder, "layout.json"), `${JSON.stringify({ ...layout, locatorBox: button, locatorPane: pane }, null, 2)}\n`);
          await page.screenshot({ path: resolve(folder, "layout.png"), animations: "disabled" });
        }
        assert(button && pane && button.x >= pane.x - 1 && button.x + button.width <= pane.x + pane.width + 1, `the GGUF action overflows the narrow pane: ${JSON.stringify({ ...layout, locatorBox: button, locatorPane: pane })}`);
      } finally { await setQualityLocale(page, "ko"); }
      return "five locales · Python download explained · native format · keyboard and narrow pane";
    }));
  } finally { backend.invoke = original; }
}
