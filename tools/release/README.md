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

## Which gates a sha owes (2026-09-16)

Each gate judges one tree — `gate-root` the window, its crates and the tools,
`gate-zo` zo-ide. Before a gate the lane diffs the sha against the sha the
matching half of `installed.json` was built from (the app's for root, zo's for
zo — the last green install of that half, never a folded ancestor). A tree the
diff did not touch owes no gate: the same bytes passed it then. The phase line
then reads `gate-zo rc=0 0s … skipped: zo tree unchanged since <sha8> — N
path(s) changed, none its own`.

- The release stamp (`Cargo.toml`, `Cargo.lock`, `tauri.conf.json`,
  `zo-ide/Cargo.toml`, `zo-ide/Cargo.lock`) counts only when a diff changes
  more than version lines; `docs/` and `*.md` never count.
- The crates zo reads from outside its tree belong to both trees; they are read
  off `cargo metadata` in the scratch (today `crates/zerocode-core/`,
  `crates/model-prices/`), not from a list kept by hand.
- Anything the lane cannot tell — no install yet, git or cargo not answering —
  runs the gate.

Measured before the rule (three green runs): gate-root 1105–1615 s, gate-zo
876–1296 s, on releases that each changed one tree. `tests/test_lane.py
GateOwed` pins the rule with stubs (`CHANGED_PATHS`, `STAMP_REAL_CHANGES`,
`ZO_SHARED_PATHS`, `-` for cargo saying nothing).

## The two phases after the swaps

- `bundle-updater` — with a key and U-B's `plugins.updater` present, and only
  under `RELEASE_PUBLISH=1` (an install run skips the second build aloud: the
  publish run builds the archive on its own sha), builds
  `app,dmg` with `createUpdaterArtifacts` on and gathers
  `ZeroCode_<version>_<arch>.app.tar.gz`, its `.sig`, the `.dmg`, the CHANGELOG
  section (`notes.md`) and `latest.json` under
  `~/.local/share/zerocode/release/out/<sha8>/`. Without a key, or before U-B's
  section lands, the phase is skipped aloud (`status.json` `phases[].skipped`,
  `status.sh` `skipped=…`) and the lane stays green.
  The app the DMG and the archive carry is sealed before it is gathered: with
  no distribution identity in the env (`APPLE_SIGNING_IDENTITY`) the lane signs
  it inside-out with `tools/signing/sign-app-bundle.sh` — the install phase's
  signer — packs the archive and the DMG again from the sealed bundle and signs
  the archive with the updater key; with one, Tauri's own seal (and its
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
