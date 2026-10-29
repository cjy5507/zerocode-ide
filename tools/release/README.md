# tools/release — the release lane, the version, the feed

The lane itself (queue → gate → push → build → swap) is
`docs/design/release-lane-off-the-window.md`; the versioned, signed, published
half is `docs/design/versioned-auto-update.md`. Every number, name and path is
a table constant at the top of `lane.sh` (`lane.sh --table` prints it) or of
`bump.sh`; nothing below repeats one.

Root verification, including browser harnesses, finishes before zo verification
starts. Any isolated browser retry judgments also finish before release builds.
This ordering is fixed; there is no parallel-gate override. App and zo release
builds may still overlap after verification, when no harness is running.

## The version (one hand: `bump.sh`)

    tools/release/bump.sh {patch|minor|major}

Moves the root `Cargo.toml` `[workspace.package] version`,
`crates/zerocode-shell/tauri.conf.json` `version` and `zo-ide/Cargo.toml`
`version` to the same semver (contract `the_three_versions_agree`), and opens
`## [x.y.z] — YYYY-MM-DD` at the top of `CHANGELOG.md` with the commit titles
since the last `vX.Y.Z` tag grouped by prefix, for a person to trim. It refuses
(rc 3) when the three already disagree, or when `v<next>` exists here or on
the release repo (`gh api`; the repo carries an older CLI's `v1.2.x`). It
neither commits nor tags; it prints the next step. The lane never raises the
version — bump first, commit, tag, then queue the sha.

## The updater key (once, on the lane machine)

The updater's private key never enters the tree: `.gitignore` refuses
`*.key` and the contract `no_private_key_in_the_tree` reads every tracked file
for a minisign secret-key box, plain or base64 (which is how `tauri signer`
writes it).

    mkdir -p ~/.local/share/zerocode/release/keys
    npx tauri signer generate -w ~/.local/share/zerocode/release/keys/updater.key

Answer the password prompt with an empty line (the lane runs unattended), or
write the password to `updater.key.password` beside the key — the lane passes
that file's contents to the build. The command writes two files:

- `updater.key` — the private key (`UPDATER_KEY` in the table). Stays here.
- `updater.key.pub` — the public key. U-B puts this file's contents verbatim
  in `tauri.conf.json` `plugins.updater.pubkey`.

The two must be one pair: the updater refuses a signature from any other key.
The lane reads the `.pub` beside the key and the scratch's
`plugins.updater.pubkey`, writes the key id (`updater_pubkey`) to
`status.json`, and is red on a mismatch before signing anything.

## The private values that must not ship (`pii-scan.py`)

The source repository publishes a release snapshot, so a value that points at
the machine it was written on must not be in the tree. `just pii-check` reads
every tracked file and exits 1 while it holds one; it is the first recipe
`verify` names, so it refuses before anything compiles (3.9 s over 1,687 files,
median of 3 on an idle machine).

Everything it knows is the `RULES` table at the top of the script — one row per
category, each carrying what it finds (`find`), what it forgives (`allow`) and
which paths it does not read (`skip`). `pii-scan.py --table` prints it. The
four rows are a home directory (written out, or slugged into a session id), an
address on a real private network, a mailbox outside the reserved example
domains, and a token or key of a shape a provider actually issues.

A fifth category has no shape a regex can know: a name. `BANISHED` beside the
table carries the SHA-256 of each word this tree was cleared of once — a
customer, a product, an internal host — so the gate refuses the word without
the script ever spelling it, and a digest publishes nothing. It matches a
whole word, which is how a path, a host or a project name carries one; a name
welded into a longer identifier is past it and is caught by reading the diff.

Nothing else in the script names a value, and nothing outside it does either:
a new exception is a new entry in that row's `allow`, or — for a single site
that cannot be rewritten, like the throwaway key a signing test needs — a
`pii-scan: allow <category> — <why>` comment on that line or the one above it,
which is visible in the diff that adds it.

The placeholder vocabulary a rewrite should reach for: `/Users/dev` for a home
directory, the RFC 5737 documentation addresses (192.0.2.x, 198.51.100.x) for a
network address, and an `example.com` / `.test` / `.invalid` domain for a
mailbox. `tests/test_pii_scan.py` pins every row from both sides.

## Which gates a sha owes (2026-09-16, narrowed 2026-10-12)

Each gate judges one tree — `gate-root` the window, its crates and the tools,
`gate-zo` zo-ide. Before a gate the lane diffs the sha against the sha the
matching half of `installed.json` was built from (the app's for root, zo's for
zo — the last green install of that half, never a folded ancestor). A diff
that changed nothing, or only version lines in the release stamp (`Cargo.toml`,
`Cargo.lock`, `tauri.conf.json`, `zo-ide/Cargo.toml`, `zo-ide/Cargo.lock`),
owes no gate: the same bytes passed it then. The phase line then reads
`gate-zo rc=0 0s … skipped: zo tree unchanged since <sha8> — N path(s)
changed, none its own`.

Any other changed path owes both gates. No path is known to lie outside
either: the root gate's `pii-check` reads every tracked file (docs, README and
zo-ide included), core tests read zo-ide sources and markdown fixtures, the
core build script turns `skills/` into code, and zo tests read the root
`fixtures/` and `skills/` at run time. Until 2026-10-12 the lane skipped
`docs/` and `*.md` and split the paths between the halves by folder; a change
to a skill, a core fixture or a file one half reads from the other skipped a
gate that read it.

Anything the lane cannot tell — no install yet, git not answering, a stamp
diff git cannot read, a path with a space or a control character in it (the
lane reads git's list NUL-separated) — runs the gate.

Measured before the rule (three green runs): gate-root 1105–1615 s, gate-zo
876–1296 s, on releases that each changed one tree. `tests/test_lane.py
GateOwed` pins the rule with stubs (`CHANGED_PATHS`, `STAMP_REAL_CHANGES`);
`GateOwedOnGit` runs `gate_owed` and `stamp_only` on a real repository.

## A coordinator's gate receipt (2026-10-22, t-47516)

The coordinator gates each landing on its build line before it cuts a release
from it, so the lane's gates ran the same recipes on the same tree a second
time: v1.1.84 (`b0965106`) spent 3168 s in gate-root and 1719 s in gate-zo on
the tree the coordinator's l64 had gated green an hour before. A cut can now
carry that gate's receipt:

    python3 tools/release/coord_receipt.py make <build-queue>/done/0-*-coord-<label>-*.job > receipt.json
    tools/release/enqueue.sh <sha> --receipt receipt.json

`make` reads every job the build line ran — the rc file, the `head <sha> tree
<tree>` its log opened with, one green `RC <key>=<rc>` line and one log per
`entry`, a `verdict: bad=0`, one owner (`# who:`), run (`# run:`) and folder
(`# cwd:`) for all — and seals the bytes it read (sha256). `enqueue.sh` reads the
receipt once, as the lane does (a receipt that does not stand queues nothing),
and keeps it as `receipts/<sha>.json` before the queue file appears. At each gate
the lane reads all of it again and:

- refuses it — the gate runs whole, and the phase says why (`"receipt":"refused:
  …"` in status.json, `receipt=` from status.sh) — when a job is red or capped,
  the jobs name two trees, owners, runs or folders, a sealed byte changed, the
  receipt is older than its last job, a job is not one the build line ran (its
  files outside `RELEASE_BUILD_LINE`'s `done/`, or no `RUN` and green `DONE` line
  of it under its owner in that line's `runner.log`), the jobs say another run
  than `RELEASE_COORD_RUN` or an owner other than a coordinator's
  `coord-<label>`, git finds a job's head not to hold the tree its jobs printed,
  or that tree to differ from the sha's apart from the stamp's version lines and
  the release notes edited in place (modified, a plain `100644` file on both
  sides — never linked, removed or given another mode), or a job ended before
  its head was committed;
- counts a recipe as gated only where an entry ran exactly `just <recipe>` in the
  gate's folder from the top of the checkout — a bare `just <recipe>` for the
  root, `zsh -c 'cd zo-ide && just <recipe>'` for zo, and nothing at all when the
  jobs ran anywhere but the top (their `# cwd:` holds no `.git`): from zo-ide a
  bare `just test` is zo's — weighed against the gate's own list
  (`verify-every-recipe.sh --list`): the lane's strict `win-check` is never
  covered by `win-check-if-available`, which says SKIPPED on a cold target, and
  zo's tests run by package are no `just test`;
- owes `NOTES_READERS` again when the release notes changed since the gated tree
  — bump.sh writes them into every release commit, and only pii-check reads them;
- runs only what is left owed (`verify-every-recipe.sh --only`), and skips the
  phase aloud when nothing is: `gate-root rc=0 0s … skipped: receipt
  coord-<label> of <run> as its jobs say (the lane's coordinator run), tree
  <tree8>: 16/16 root recipes green — logs <folder>`.

Nothing in a receipt is signed. The owner and run are what the jobs say of
themselves (`# who:`, `# run:`), held only to the lane's own values; the seal says
the bytes did not change after `make`, not who made them; and every file read —
the jobs, the runner's log, the entry logs — is one any agent of the same user can
write. The checks stop a job copied by hand, refused or never run by the line, one
of another run or owner, a head and tree that disagree, and a gate of another tree
(ZO's review of t-47516); they do not stop one who writes all of those files on
purpose.

The receipt is kept beside the lane log as `out/receipt-<sha8>.json`; a sha that
ends red or refused keeps it for its next turn. `tests/test_coord_receipt.py`
pins the reading; `tests/test_lane.py Receipt` pins the lane with stubs and
`ReceiptOnGit` the same on a real repository.

## The quick check before a batch (2026-10-25, t-73910)

10-25's batch gate went round four times on reds a read of the text finds, each
seen only when the gate reached it: a bundled skill's text with no release line
(l78d, about 7 min in), a tools seed behind the Rust kinds it copies (l78d, about
21 min), a launch road past the shell's literal count (l78g, about 13 min), a
call at load the settings test's fake backend has no case for (l78h, about 90
min). Before a batch head is gated — and a worker before it hands over —

    python3 tools/release/prebatch.py --base main --head <batch head>

reads the paths the two trees differ in and runs only the checks that read one
of them: `skill-release`, `backend-count` (a count assert over a crate's
`shipped_backend()`, with the test's own number), `settings-ipc`, and each
`tools-test` line of the justfile whose test names a changed file, run in a
checkout of the head made for it and removed after, four at once (over 30 s it
is said unchecked: the gate's to run). `test_lane.py` and the computer bench's
`test_bench.py` ran past that on every 10-25 head, so they run only with
`--slow` and are otherwise said unchecked; the default run stays under 10 s. It prints `RC <check>=<rc> (<s>s)` with reasons,
`unchecked <path>` for a path no check reads, and `RC prebatch=<rc>`; it exits 1
on a red and 2 when a revision cannot be read. It runs no cargo and stands in for
no gate: a green says only that these checks found nothing, and every recipe the
gate owes still runs. `tests/test_prebatch.py` pins each rule.

## A job names its tasks: `# task:` (2026-10-25, t-73910)

A build-line job's head may carry one line naming the ledger tasks it gates or
checks, comma-separated:

    # task: t-73910, t-74181

The board's schedule (`crates/zerocode-shell/src/schedule_timeline.rs`) reads it
off the job's file in `in/`, `run/` or `done/` and sums, per task, the jobs that
named it and ran to an end, how many were red and their run time — each job's
whole, never shared out; a batch's job says how many tasks it shared. Only `t-`
and digits are read; any other piece is dropped. `# what:` stays the person's
words and is never read for a task, so a job with no `# task:` line ties to no
task, and that task's line time is unknown, not zero. The runner reads the line
as the comment it is. The coordinator's gate script and a worker's own job each
write it.

## Re-gating the same head from a receipt (`gate_receipt.py`, 2026-10-25, t-73910)

The core's gate reuse (`orchestration::gate_reuse`) answers hit only for the same
head, tree and declared inputs gated before under the same live coordinator: a
batch rebuilt with any other member is always a miss
(`a_different_head_with_the_same_tree_misses`). It pays where the same head goes
up again — a batch whose only red was a measurement job, or a job withdrawn and
put back. On 10-25 none of the last 300 build-line jobs used it, and the four
l78 heads (c, f, g, h) were all different, so it would have saved 0 minutes that
day.

A build-line job reaches it only through the window's pinned launcher: called
directly, `gate_receipt.py` answers `miss` ("use the independently pinned
external launcher"). The shape of a job entry (not yet run on the line — every
`<…>` is the coordinator's):

    zerocode gate-bootstrap \
      --launcher <pinned gate_launcher.py> --launcher-sha256 <sha256> \
      --python <python> --python-sha256 <sha256> --stdlib <stdlib> --stdlib-sha256 <sha256> \
      --gate-job '<JobIntent JSON, core gate_reuse::JobIntent>' \
      -- --policy <manifest> --policy-sha256 <sha256> --repo <gate checkout> \
      -- verify --repo <gate checkout> --gate gate-root --task <t-…> --attempt <dispatch|none> \
         --scope deterministic --metadata <the first run's metadata.json> --log <its gate.log> \
         -- just test

`run` in place of `verify` (with `--output <folder>` for the metadata and log)
executes and records the first time. A `measurement` or `nondeterministic` scope
never hits: window measurements and benches always run. A hit still leaves the
four mandatory checks (`native`, `windows`, `flake-judgment`, `publish`) owed.

The policy is not pinned (`gate-policy-pin`), by the coordinator's decision
(m-74459). Pinning would let a deterministic job on a head gated before skip as
a hit, with "reused — original record r" kept in the ledger. Against it: every
record misses after a coordinator generation change
(`a_completed_receipt_misses_after_a_coordinator_generation_change`) or a change
of the pinned policy bytes; without the pinned launcher and manifest every job
misses (fail closed — an unrun check is never called passed); and each job pays
the fingerprint (`timings.fingerprintMs` in the answer). The gain stays unknown
until heads are gated again.

## What status.json says of the lane itself (2026-10-29, t-83513)

The board showed this lane on every project's 빌드 줄 card — another project's
board said 「새 버전 발행」 for ZeroCode — and named its steps from a table of
this lane's step names kept in the window. status.json now says both itself:

- `git_common_dir` — the folder every checkout of the released repository
  shares (`git rev-parse --path-format=absolute --git-common-dir` of
  `RELEASE_REPO`; `RELEASE_GIT_COMMON_DIR` in the table). The window shows the
  lane only on a project whose checkout shares that very folder, told by its
  identity on disk (device and inode, `update_runtime::lane_releases`) — never
  by a path's spelling or the repository's name. Another project's board shows
  its own build line, or says in one sentence that it has none.
- `plan` — the steps the lane runs, in order, each with its name in the
  window's five languages: `[{"name":"gate-zo","words":{"ko":"zo 시험",…}},…]`.
  `PHASE_WORDS` in the table is the one place (`name|ko|en|ja|zh|es` a line;
  `PHASES` is its first column, `PHASE_LANGS` the languages), and
  `lane.sh --plan` prints the list. The board draws any lane's steps from it
  and keeps no table of its own, so a lane it never knew stands in its own
  words; the schedule plans a running lane's steps ahead by it.

## A recipe that writes nothing (2026-10-29, t-83513)

On 10-29 v1.1.104's gate-zo stood 55 minutes in a deadlocked test, and neither
its log nor the lane's got a line. While a gate runs the lane now looks at its
log every `QUIET_POLL_SECS`: the recipe is the log's last `==> verify recipe`
mark, its silence how long the log has not grown, and its usual time the
median of what it took in the newest `QUIET_SAMPLES` lanes that ended green
(their gate logs' `<== verify recipe <name> rc=0 <secs>s` lines).

- Silent `QUIET_FACTOR` times its usual time, and at least `QUIET_FLOOR_SECS`:
  the lane log says `stalled: <gate> <recipe> — <span> without output (usual
  <span>); ended red at <span>`, status.json carries `stalled`
  (`{"gate","recipe","quiet_since_ms","cap_secs"}`, the board's 「멈춘 듯」),
  and `status.sh` ends its line with `stalled=<gate>/<recipe>`. Output that
  comes back takes the word back (`— output again`); the lines the gate writes
  for a recipe the lane ended are not output come back.
- Silent `QUIET_CAP_FACTOR` times its usual time, and at least
  `QUIET_CAP_FLOOR_SECS`: the lane sends the gate `SIGUSR1`. zo's `test`, usual
  955 s over the green lanes of 10-29, is said at 32 min and ended at 48 — the
  deadlock that day stood about 55 min. `verify-every-recipe.sh --stoppable`
  (the lane's gate always runs with it) then ends that recipe's own process
  tree — found by parent id, TERM and then KILL — writes just's own `recipe …
  was terminated` line for it, so the flake judgment reads it as a red recipe
  (`recipe:<name>`, red for real even beside the failures it named: the tests
  after the stop never ran, so no solo pass of those names clears it), and goes
  on to the next recipe. The lane signals only the gate process it started, by
  the pid it holds; no process is found by name.
- A recipe with no green past is said at the floor and never ended: nothing
  says what is long for it.

`tests/test_lane.py` `Quiet` pins the medians, the limits and both roads on the
real gate script (`RELEASE_STUB_GATE_<LANE>_JUSTDIR` drives it over a justfile
of the test's own); `Stoppable` pins the script alone.

## The warm targets

Each gate and build keeps its own cargo target under
`~/.local/share/zerocode/release/target/` (`TARGET_LANES`), so registry crates
are built once and stay warm. Before the gates, `trim_targets` drops two kinds
of dead weight from each: the incremental caches (`CARGO_INCREMENTAL=0` never
reads them), and every build unit of a crate this repository defines
(`workspace_units.py`, which takes the names from the scratch's `Cargo.toml`
files, minus any name `Cargo.lock` gives a registry or git source). The scratch
is a fresh clone, so each run rebuilds those units anyway, and the ones an
earlier version left are never read again: on 2026-10-13 they were 10.4 of
zo-gate's 12.4 GiB and 11.4 of root-gate's 19 GiB. Only a target still over
`TARGET_CAP_GB` after that is cleaned whole, which costs its next gate every
registry crate (245 for v1.1.62's zo gate, about 6 minutes).
`tests/test_workspace_units.py` proves the drop on a real cargo build.

## The two phases after the swaps

- `bundle-updater` — with a key and U-B's `plugins.updater` present, and only
  under `RELEASE_PUBLISH=1` (an install run skips the phase aloud), gathers
  `ZeroCode_<version>_<arch>.app.tar.gz`, its `.sig`, the `.dmg`, the CHANGELOG
  section (`notes.md`) and `latest.json` under
  `~/.local/share/zerocode/release/out/<sha8>/`. Without a distribution identity
  it ships build-app's app, the one the install phase put in place: one build
  serves both, and the phase is red when the sealed app's cdhash is not the
  installed app's. With an identity it builds `app,dmg` again with
  `createUpdaterArtifacts` on, because Tauri signs and notarizes only inside
  its own build. Without a key, or before U-B's
  section lands, the phase is skipped aloud (`status.json` `phases[].skipped`,
  `status.sh` `skipped=…`) and the lane stays green.
  The app the DMG and the archive carry is sealed before it is gathered: with
  no distribution identity in the env (`APPLE_SIGNING_IDENTITY`) it is
  build-app's app, signed inside-out at build time with
  `tools/signing/sign-app-bundle.sh` and installed as is; the lane packs the
  archive and the DMG from it and signs the archive with the updater key, and
  never signs the app again (a re-sign moves the outer cdhash, so the shipped
  app would not be the installed one); with an identity, Tauri's own seal (and its
  notarization when `APPLE_ID`·`APPLE_PASSWORD`·`APPLE_TEAM_ID` stand beside it)
  is kept. Either way `codesign --verify --deep --strict` must pass or the phase
  is red: Tauri without an identity ships the linker's adhoc signature over
  unsealed resources, which another Mac's Gatekeeper calls "damaged" under
  quarantine (v1.3.78, 2026-09-15).
- `publish` — only under `RELEASE_PUBLISH=1`. Creates `v<version>` on
  `RELEASE_GITHUB_REPO` with the assets and the feed (`gh release create`);
  refuses (rc 3, the queue file is not put back) when the tag already exists.
  `RELEASE_CHANNEL=beta` publishes a prerelease and moves the `beta` tag to a
  prerelease that carries the feed, so `…/releases/download/beta/latest.json`
  follows — unless the `beta` release on the repo is not ours (no `latest.json`
  + `ZeroCode_` asset), which is refused and left alone.
  `RELEASE_LEGACY_MANIFEST=1` sends the old zo CLI's `manifest.txt`,
  `SHA256SUMS`, `zo-v<version>-<triple>` for all three targets and
  `legacy/install.sh` (verbatim) along; a missing target refuses the publish
  whole, because the old installer requires all three rows.

`RELEASE_PUBLISH` is not something the launchd plist carries: the first and
every deliberate publish is a direct run in a terminal by the coordinator,
after the person's go —

    RELEASE_PUBLISH=1 tools/release/lane.sh <full sha>
    tools/release/status.sh

  Run it **detached from the terminal session that starts it**. A publish lane
  takes ~50 minutes, and a lane started as a plain background job dies with the
  session that started it (2026-09-10: two v1.3.21 lanes ended `SIGTERM` when
  the coordinator's session restarted; the tag stayed, nothing was published).
  A new process session survives that:

      python3 -c 'import os,subprocess; subprocess.Popen(
          ["tools/release/lane.sh", "<full sha>"],
          env=dict(os.environ, RELEASE_PUBLISH="1"),
          stdout=open(os.path.expanduser("~/.local/share/zerocode/release/logs/lane.log"), "ab"),
          stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL, start_new_session=True)'
      tools/release/status.sh        # poll; the lane reports its phase here

  Never stop a gate by process name (`pkill -f "just verify"`): the lane's own
  gate is a `just verify` too, in its scratch clone, and dies with it.

## Out of scope, with a place kept

- Windows/Linux app assets (Actions is billing-blocked; `platforms` carries
  only what this machine builds).
- Developer ID signing and notarisation (`notarize` would sit between
  `bundle-updater` and `publish`).
