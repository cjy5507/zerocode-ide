/* Native-browser preview using the same backend as the regression gate.
 * Development only: all IPC is fixture data, no account or file mutations. */
import { standBackend, createWindowServer } from "./window-boot.mjs";

let bootstrapSource = "";
await standBackend({
  addInitScript(fn, arg) {
    bootstrapSource += "(" + fn.toString() + ")(" + JSON.stringify(arg) + ");\n";
  },
});
bootstrapSource += `
window.__ARTIFACTS__ = { count: 0, gallery: 9 };
window.addEventListener("load", () => {
  document.title = "ZeroCode Artifact Studio Preview";
  const open = () => {
    if (typeof BOUND === "undefined" || BOUND.size === 0) {
      requestAnimationFrame(open);
      return;
    }
    openArtifacts();
  };
  open();
});
`;
const { files, origin } = await createWindowServer({ bootstrapSource });
console.log(origin);
for (const signal of ["SIGINT", "SIGTERM"]) {
  process.once(signal, () => files.close(() => process.exit(0)));
}
