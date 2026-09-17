/* Same fixture IPC as the regression gate, for native-browser visual review. */
import { standBackend, createWindowServer } from "./window-boot.mjs";
import { workspaceBoardFixture } from "./workspace-board.mjs";
import { taskBoardFixture } from "./task-board.mjs";
let bootstrapSource = "";
await standBackend({ addInitScript(fn, arg) {
  bootstrapSource += `(${fn.toString()})(${JSON.stringify(arg)});\n`;
} });
bootstrapSource += `window.addEventListener("load", () => {
  const start = async () => {
    if (typeof BOUND === "undefined" || !BOUND.size) { requestAnimationFrame(start); return; }
    await (${workspaceBoardFixture.toString()})();
    (${taskBoardFixture.toString()})();
    setWorkspaceBoardOpen(false);
    window.__VAULT__ = { pages: 72, linksPer: 2, ghosts: 4, tags: ["design", "runtime", "verification"] };
    secondBrainVault = "/vault";
    paintKnowledgeEntry();
    document.title = "ZeroCode Connected Workbench Preview";
  }; start();
});`;
const { files, origin } = await createWindowServer({ bootstrapSource });
console.log(origin);
for (const signal of ["SIGINT", "SIGTERM"]) process.once(signal, () => files.close(() => process.exit(0)));
