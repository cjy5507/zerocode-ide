// zo rides along with the app (t-3191, docs/design/versioned-auto-update.md
// §2.4): the release bundle carries `Resources/bin/zo`, which the window
// installs to `~/.local/bin/zo` at boot when it is missing or older.
//
// tauri-build copies `bundle.resources` at COMPILE time and refuses a path
// that is not there, so the binary must exist before `tauri build` runs. This
// script puts it at `crates/zerocode-shell/bin/zo` (gitignored), and only
// `tauri.release.conf.json` names that resource — a dev build (`cargo run`,
// `tauri.conf.json`) carries no zo and asks for none.
//
//   ZO_STAGE_BIN=<path>        copy an already built zo (the lane's own build)
//   ZO_STAGE_TARGET_DIR=<dir>  cargo target dir for the build (default zo-ide/target)
//   ZO_STAGE_PROFILE=<name>    cargo profile for the build (default release)
//
// The staged binary must say the workspace version (§2.1: one version, three
// followers): a zo that says another version is refused, not shipped.
//
// Under a Developer ID (release-package exports APPLE_SIGNING_IDENTITY) the
// staged zo is signed here, before tauri seals the app around it, and must
// still start. tauri signs only the code it places itself — a
// bundle.resources file is copied as data (the bundler of tauri-cli 2.11.4:
// resources add nothing to sign_paths, and codesign runs without --deep) — while
// notarization wants every nested Mach-O under that identity with a secure
// timestamp and hardened runtime. The local lane builds --no-sign and signs
// the whole bundle after (tools/signing/sign-app-bundle.sh).
import { spawnSync } from "node:child_process";
import { chmod, copyFile, mkdir, readFile, rename, rm, stat } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { signForDistribution } from "./native-signing.mjs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const DEST = join(ROOT, "crates", "zerocode-shell", "bin", "zo");

function workspaceVersion() {
  const manifest = spawnSync("sed", ["-n", "/^\\[workspace.package\\]/,/^\\[/p", join(ROOT, "Cargo.toml")], {
    encoding: "utf8",
  }).stdout;
  const match = manifest.match(/^version = "([^"]+)"/m);
  if (!match) throw new Error("the workspace version is not in Cargo.toml [workspace.package]");
  return match[1];
}

async function builtZo() {
  const profile = process.env.ZO_STAGE_PROFILE ?? "release";
  const target = process.env.ZO_STAGE_TARGET_DIR ?? join(ROOT, "zo-ide", "target");
  const build = spawnSync("cargo", ["build", "--profile", profile, "-p", "zo-ide"], {
    cwd: join(ROOT, "zo-ide"),
    env: { ...process.env, CARGO_TARGET_DIR: target },
    stdio: "inherit",
  });
  if (build.status !== 0) throw new Error(`cargo build -p zo-ide failed (rc ${build.status})`);
  return join(target, profile === "dev" ? "debug" : profile, "zo");
}

function saysTheVersion(binary, version) {
  const said = spawnSync(binary, ["--version"], { encoding: "utf8" }).stdout ?? "";
  const word = said.split(/[\s()]+/).map((w) => w.replace(/^v/, "")).find((w) => /^\d+\.\d+\.\d+/.test(w));
  if (word !== version) {
    throw new Error(`staged zo says ${JSON.stringify(said.trim())} but the workspace version is ${version}`);
  }
}

const source = process.env.ZO_STAGE_BIN ?? (await builtZo());
if (!(await stat(source).catch(() => null))?.isFile()) throw new Error(`no zo binary at ${source}`);
const version = workspaceVersion();
saysTheVersion(source, version);

await mkdir(dirname(DEST), { recursive: true });
const staged = `${DEST}.new`;
await rm(staged, { force: true });
await copyFile(source, staged);
await chmod(staged, 0o755);
if (signForDistribution(staged, { identifier: "zo" })) {
  saysTheVersion(staged, version);
}
await rename(staged, DEST);
console.log(`staged zo ${version} from ${source} → ${DEST}`);
