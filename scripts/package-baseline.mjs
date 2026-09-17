import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  ROOT,
  artifactUploadName,
  compareSemver,
  packageIdentity,
} from "./package-contract.mjs";

const RELEASE_WORKFLOW_PATH = ".github/workflows/verify.yml";
const RELEASE_WORKFLOW_FILE = RELEASE_WORKFLOW_PATH.split("/").at(-1);

function git(args, root = ROOT, optional = false) {
  try {
    return execFileSync("git", args, { cwd: root, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
  } catch (error) {
    if (optional) return null;
    throw new Error(`git ${args.join(" ")} failed: ${error.stderr?.toString().trim() || error.message}`);
  }
}

export function selectBaselineRun(runs, expectedTag, expectedRevision, expectedWorkflowId) {
  const matches = runs
    .filter((run) => (
      run.workflow_id === expectedWorkflowId
      && run.event === "push"
      && run.status === "completed"
      && run.conclusion === "success"
      && run.head_branch === expectedTag
      && run.head_sha === expectedRevision
      && run.path?.split("@", 1)[0] === RELEASE_WORKFLOW_PATH
    ))
    .sort((left, right) => (
      (right.run_attempt ?? 0) - (left.run_attempt ?? 0)
      || Date.parse(right.created_at) - Date.parse(left.created_at)
    ));
  if (matches.length === 0) {
    throw new Error(`release blocker: successful ${RELEASE_WORKFLOW_PATH} run for ${expectedTag} was not found`);
  }
  return matches[0];
}

export function selectBaselineArtifact(artifacts, expectedName, expectedRevision, expectedRunId) {
  const matches = artifacts
    .filter((artifact) => (
      artifact.name === expectedName
      && artifact.expired === false
      && artifact.workflow_run?.head_sha === expectedRevision
      && artifact.workflow_run?.id === expectedRunId
    ))
    .sort((left, right) => Date.parse(right.created_at) - Date.parse(left.created_at));
  if (matches.length === 0) {
    throw new Error(`release blocker: retained signed artifact ${expectedName} for ${expectedRevision} was not found`);
  }
  return matches[0];
}

export function verifyArchiveDigest(bytes, digest) {
  if (!digest) throw new Error("GitHub artifact digest is missing");
  const [algorithm, expected] = digest.split(":", 2);
  if (algorithm !== "sha256" || !/^[0-9a-f]{64}$/i.test(expected ?? "")) {
    throw new Error(`unsupported GitHub artifact digest: ${digest}`);
  }
  const actual = createHash("sha256").update(bytes).digest("hex");
  if (actual !== expected.toLowerCase()) throw new Error("downloaded baseline archive digest differs from GitHub evidence");
}

function writeOutput(name, value) {
  if (!process.env.GITHUB_OUTPUT) return;
  process.stdout.write(`baseline ${name}: ${value}\n`);
  return writeFile(process.env.GITHUB_OUTPUT, `${name}=${value}\n`, { flag: "a" });
}

async function githubJson(url, token) {
  const response = await fetch(url, {
    headers: {
      Accept: "application/vnd.github+json",
      Authorization: `Bearer ${token}`,
      "X-GitHub-Api-Version": "2022-11-28",
    },
  });
  if (!response.ok) throw new Error(`GitHub artifact lookup failed with HTTP ${response.status}`);
  return response.json();
}

async function downloadArchive(url, token) {
  const response = await fetch(url, {
    headers: {
      Accept: "application/vnd.github+json",
      Authorization: `Bearer ${token}`,
      "X-GitHub-Api-Version": "2022-11-28",
    },
    redirect: "manual",
  });
  if (response.status !== 302) throw new Error(`GitHub artifact download failed with HTTP ${response.status}`);
  const location = response.headers.get("location");
  if (!location) throw new Error("GitHub artifact download redirect is missing");
  const storageUrl = new URL(location);
  if (storageUrl.protocol !== "https:") throw new Error("GitHub artifact storage redirect is not HTTPS");
  const archive = await fetch(storageUrl);
  if (!archive.ok) throw new Error(`GitHub artifact storage download failed with HTTP ${archive.status}`);
  return Buffer.from(await archive.arrayBuffer());
}

export async function downloadPreviousPackage(platform, output, options = {}) {
  const root = resolve(options.root ?? ROOT);
  const identity = await packageIdentity({ root });
  const currentTag = options.currentTag ?? process.env.GITHUB_REF_NAME;
  if (currentTag !== `v${identity.version}`) {
    throw new Error(`release tag ${currentTag || "<missing>"} differs from package version v${identity.version}`);
  }
  const currentRevision = git(["rev-list", "-n", "1", currentTag], root);
  const workflowRevision = options.workflowRevision ?? process.env.GITHUB_SHA;
  if (workflowRevision && currentRevision !== workflowRevision.toLowerCase()) {
    throw new Error("checked-out release tag does not match GITHUB_SHA");
  }

  const previousTag = git(["describe", "--tags", "--match", "v*", "--abbrev=0", `${currentTag}^`], root, true);
  if (!previousTag) return { available: false, currentTag };
  const previousConfig = JSON.parse(git(["show", `${previousTag}:crates/zerocode-shell/tauri.conf.json`], root));
  if (previousTag !== `v${previousConfig.version}`) {
    throw new Error(`previous tag ${previousTag} differs from its package version v${previousConfig.version}`);
  }
  if (compareSemver(previousConfig.version, identity.version) >= 0) {
    throw new Error(`release version ${identity.version} is not newer than ${previousConfig.version}`);
  }

  const repository = options.repository ?? process.env.GITHUB_REPOSITORY;
  const token = options.token ?? process.env.GITHUB_TOKEN;
  const apiUrl = options.apiUrl ?? process.env.GITHUB_API_URL ?? "https://api.github.com";
  if (!repository || !token) throw new Error("GitHub repository or token is missing for upgrade baseline lookup");
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository)) {
    throw new Error("invalid GitHub repository identity");
  }
  const previousRevision = git(["rev-list", "-n", "1", previousTag], root);
  const expectedName = artifactUploadName(platform, previousTag, previousConfig.productName);
  const workflow = await githubJson(
    new URL(`${apiUrl}/repos/${repository}/actions/workflows/${encodeURIComponent(RELEASE_WORKFLOW_FILE)}`),
    token,
  );
  if (!Number.isInteger(workflow.id) || workflow.path !== RELEASE_WORKFLOW_PATH) {
    throw new Error(`release blocker: ${RELEASE_WORKFLOW_PATH} identity differs on GitHub`);
  }
  const runQuery = new URL(`${apiUrl}/repos/${repository}/actions/workflows/${workflow.id}/runs`);
  runQuery.searchParams.set("branch", previousTag);
  runQuery.searchParams.set("event", "push");
  runQuery.searchParams.set("head_sha", previousRevision);
  runQuery.searchParams.set("status", "success");
  runQuery.searchParams.set("per_page", "100");
  const runListing = await githubJson(runQuery, token);
  const run = selectBaselineRun(
    runListing.workflow_runs ?? [],
    previousTag,
    previousRevision,
    workflow.id,
  );
  const artifactQuery = new URL(`${apiUrl}/repos/${repository}/actions/runs/${run.id}/artifacts`);
  artifactQuery.searchParams.set("name", expectedName);
  artifactQuery.searchParams.set("per_page", "100");
  const listing = await githubJson(artifactQuery, token);
  const artifact = selectBaselineArtifact(
    listing.artifacts ?? [],
    expectedName,
    previousRevision,
    run.id,
  );
  const bytes = await downloadArchive(artifact.archive_download_url, token);
  verifyArchiveDigest(bytes, artifact.digest);

  const destination = resolve(output);
  await mkdir(dirname(destination), { recursive: true });
  await writeFile(destination, bytes, { flag: "wx" });
  return {
    available: true,
    archive: destination,
    tag: previousTag,
    version: previousConfig.version,
    revision: previousRevision,
    artifactName: expectedName,
  };
}

async function main() {
  const [platform, flag, output] = process.argv.slice(2);
  if ((platform !== "macos" && platform !== "windows") || flag !== "--output" || !output) {
    throw new Error("usage: node scripts/package-baseline.mjs <macos|windows> --output archive.zip");
  }
  const result = await downloadPreviousPackage(platform, output);
  await writeOutput("available", result.available);
  if (result.available) {
    await writeOutput("archive", result.archive);
    await writeOutput("tag", result.tag);
    await writeOutput("version", result.version);
    await writeOutput("revision", result.revision);
    process.stdout.write(`downloaded upgrade baseline ${result.artifactName}\n`);
  } else {
    process.stdout.write("no ancestor release tag: this tag seeds the upgrade baseline\n");
  }
}

if (resolve(process.argv[1] ?? "") === fileURLToPath(import.meta.url)) {
  await main();
}
