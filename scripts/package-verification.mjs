import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

// A skipped zo job is valid only when the PR's successful change detector
// explicitly found no zo inputs changed. Tags and pushes always owe the gate.
export function requirePackageVerification({ root, changes, zo, quality, required, event }) {
  if (event !== "push" && event !== "pull_request") throw new Error("package event is unknown");
  if (root !== "success") throw new Error(`root verification is ${root ?? "unknown"}`);
  if (changes !== "success") throw new Error(`change detection is ${changes ?? "unknown"}`);
  if (quality !== "success") throw new Error(`workflow quality is ${quality ?? "unknown"}`);
  if (required !== "true" && required !== "false") throw new Error("zo verification requirement is unknown");
  if (event !== "pull_request" && required !== "true") throw new Error("pushes and release tags require zo verification");
  if (zo === "success") return;
  if (event === "pull_request" && required === "false" && zo === "skipped") return;
  throw new Error(`zo verification is ${zo ?? "unknown"}`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    requirePackageVerification({
      root: process.env.ROOT_RESULT,
      changes: process.env.CHANGES_RESULT,
      zo: process.env.ZO_RESULT,
      quality: process.env.QUALITY_RESULT,
      required: process.env.ZO_REQUIRED,
      event: process.env.EVENT_NAME,
    });
    console.log("package verification passed for every required workspace");
  } catch (error) {
    console.error(`package verification refused: ${error.message}`);
    process.exitCode = 1;
  }
}
