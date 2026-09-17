import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const SIGNER = fileURLToPath(new URL("../tools/signing/sign-developer-id.sh", import.meta.url));

/* Tauri copies resources and macOS.files as data. These callers sign their
 * staged code under the same explicit policy before the outer seal is made. */
export function signForDistribution(path, { identifier = "", entitlements = "" } = {}) {
  if (!process.env.APPLE_SIGNING_IDENTITY) return false;
  const signed = spawnSync("bash", [SIGNER, path, identifier, entitlements], { stdio: "inherit" });
  if (signed.error) throw signed.error;
  if (signed.status !== 0) throw new Error(`codesign of staged code failed (rc ${signed.status}): ${path}`);
  return true;
}
