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
import { chmod, copyFile, mkdir, readFile, realpath, rename, rm, stat } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { signForDistribution } from "./native-signing.mjs";
import { cargoWorkspaceVersion } from "./package-contract.mjs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
export async function builtZo({ root = ROOT, env = process.env, run = spawnSync } = {}) {
  const profile = env.ZO_STAGE_PROFILE ?? "release";
  const target = env.ZO_STAGE_TARGET_DIR ?? join(root, "zo-ide", "target");
  const build = run("cargo", ["build", "--locked", "--profile", profile, "-p", "zo-ide", "--bin", "zo", "--message-format=json-render-diagnostics"], {
    cwd: join(root, "zo-ide"),
    env: { ...env, CARGO_TARGET_DIR: target },
    encoding: "utf8",
    stdio: ["ignore", "pipe", "inherit"],
    maxBuffer: 64 * 1024 * 1024,
  });
  if (build.error) throw build.error;
  if (build.status !== 0) throw new Error(`cargo build -p zo-ide failed (rc ${build.status})`);
  // Cargo owns the target triple, profile directory and executable suffix.
  // Guessing target/<profile>/zo lost Windows' .exe and custom target layouts.
  const artifacts = (build.stdout ?? "").split("\n")
    .filter(line => line.startsWith("{"))
    .map(line => JSON.parse(line))
    .filter(message => message.reason === "compiler-artifact"
      && message.target?.name === "zo" && message.target.kind?.includes("bin")
      && typeof message.executable === "string" && message.executable.length > 0);
  const paths = [...new Set(artifacts.map(message => message.executable))];
  if (paths.length !== 1) throw new Error(`Cargo reported ${paths.length} zo executable paths; expected exactly one`);
  return paths[0];
}

function saysTheVersion(binary, version, run) {
  const result = run(binary, ["--version"], { encoding: "utf8" });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`staged zo --version failed (rc ${result.status})`);
  const said = result.stdout ?? "";
  const word = said.split(/[\s()]+/).map((w) => w.replace(/^v/, "")).find((w) => /^\d+\.\d+\.\d+/.test(w));
  if (word !== version) {
    throw new Error(`staged zo says ${JSON.stringify(said.trim())} but the workspace version is ${version}`);
  }
}

export async function stageZo({ root = ROOT, env = process.env, run = spawnSync, sign = signForDistribution } = {}) {
  const destination = join(root, "crates", "zerocode-shell", "bin", "zo");
  const source = env.ZO_STAGE_BIN ?? (await builtZo({ root, env, run }));
  if (!(await stat(source).catch(() => null))?.isFile()) throw new Error(`no zo binary at ${source}`);
  const version = cargoWorkspaceVersion(await readFile(join(root, "Cargo.toml"), "utf8"));
  saysTheVersion(source, version, run);

  await mkdir(dirname(destination), { recursive: true });
  const staged = `${destination}.new`;
  await rm(staged, { force: true });
  try {
    await copyFile(source, staged);
    await chmod(staged, 0o755);
    if (sign(staged, { identifier: "zo" })) {
      saysTheVersion(staged, version, run);
    }
    await rename(staged, destination);
  } finally {
    await rm(staged, { force: true });
  }
  return { source, destination, version };
}

// Node resolves the module's links; the CLI spelling may still name one.
// Compare both real paths so a linked checkout cannot report a silent success.
if (process.argv[1] && await realpath(process.argv[1]).catch(() => null) === await realpath(fileURLToPath(import.meta.url))) {
  const { source, destination, version } = await stageZo();
  console.log(`staged zo ${version} from ${source} → ${destination}`);
}
