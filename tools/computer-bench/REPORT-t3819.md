# t-3819: bounded Computer Use APM and desk ownership audit

The scripted native accessibility fixture exceeded 100 successful actions/minute. This worker's LLM-inclusive random-target loop did not. No product code changed. All three owned fixtures were quit through the guarded CLI with `terminated: true`, `force: false`; desk release was recorded in ledger messages m-3838 and m-3843.

Measured 2026-09-12 on installed ZeroCode 1.3.51; source checkout base `ad4bed8ae15d6122b1d28fea236f19de6ce6a776`. This is a local measurement, not an integration gate or a claim about all real tasks. Machine-readable results: [results-20260912.json](results-20260912.json).

| Lane | Correct / attempted | Input-call wall | Entire loop wall | Input-call APM | Entire loop APM |
|---|---:|---:|---:|---:|---:|
| Scripted, fresh semantic AXPress per step | 180 / 180 | 43.700 s | 76.498 s | 247.14 | **141.18** |
| Scripted, app-coordinate batches of five | 10 / 10 | 6.543 s | 6.909 s | 91.70 | **86.84** |
| This Codex worker, random target, one LLM decision per step | 10 / 10 | 2.352 s | 53.145 s | 255.15 | **11.29** |

The first row ran continuously for more than a minute. The coordinate row is a short diagnostic, not a sustained-minute result. The random row uses this existing worker's actual model/tool turns, with ten separate commands chosen after reading each returned AX tree. No additional model or background coding agent was launched; the exact wire model/effort was not independently measured. The random target can repeat; each correct press still increments the counter and appends a new event. Two initial calibration presses are excluded from the table. The observation comparison below adds another 20 presses, excluded from these lanes.

“Input-call wall” includes the public CLI process and bridge, provider action and returned app snapshot; it is not bare native event injection time. The full scripted loop also includes stop/hotkey checks, fresh geometry/index lookup, native event-oracle checks, AX readback and report bookkeeping. LLM wall starts when the initial AX look is available and ends after the tenth action's oracle/readback; it includes all intervening model decisions and tool round trips. Launch, quit and later report writing are excluded. APM numerators exclude waits, observations, failed/no-op calls and batch parent requests. Ten child actions in two batches earn ten actions, never twelve. Conservative failed rounds earn no successful actions.

The native AppKit fixture owns a unique bundle ID, executable, PID and nonce. Only its Amber/Blue button handlers update its counter and ordered event file. The oracle requires the exact new labels in order, a matching count delta, zero new errors, and unchanged owner/PID. A transport success with no new button event fails. Semantic actions use a fresh advertised `AXPress` index every time; coordinate batches use newly observed button frames converted from screen to window coordinates. No desktop mouse/key command, clipboard, existing user app, account, website, terminal UI or ZeroCode window was controlled. Normal coordinate `click --app` can raise its own fixture window; semantic AXPress does not move the pointer. Thus this run does not measure cursor animation or general mouse-motion throughput.

## Existing real-time eye versus settling

Ten paired rounds alternated which style went first after a verified semantic action. The public CLI's existing desktop `observe` stream was warmed once; no second capture loop was added. Each action was followed by an AX oracle look before the timed desktop look, which gives the app roughly one extra AX round trip to paint. This affects the interpretation of settle cost.

| Post-action desktop observation | Samples | Median wall |
|---|---:|---:|
| Latest frame (`observe --viewer <owned-id>`) | 10 | **35.34 ms** |
| Settled (`observe --viewer <owned-id> --settle`) | 10 | **134.51 ms** |

All 20 actions were correct. Settle reported true for every sample, with 58–114 ms waited. The physical HID watcher was active throughout this comparison and observed no interruption. Latest-frame pixel freshness was **not** independently checked; the AX/native event oracle establishes action correctness, not that a returned screenshot contains the newest counter. The raw screenshot paths are retained in the scratch calls log, not committed or inspected for unrelated user content.

The median observation saving is about 99 ms. This worker's live loop averaged 5.315 s/action; only 0.235 s/action was inside the input CLI. The remaining 50.794 s across ten actions includes model/tool turn overhead plus guards/readback. This harness cannot split network, model inference and tool scheduler time further. A faster latest-frame look can reduce observation cost but does not establish a 100 APM LLM loop. Predictable fixtures allow scripts or bounded verified sequences; arbitrary real tasks add decisions, layout changes, application waits and recovery.

## Proven eye.py measurement repair

Previously `cost()` chose `helper_pid()` before its first screenshot could launch a cold helper. The supplied root result `/private/tmp/zerocode-post-restart-apm-20260912/eye-cost.json` consequently reported unknown CPU for the whole run. `eye.py` now makes and reports a separate warmup screenshot, then finds the PID for measured samples. A regression test simulates exactly this launch ordering. A subprocess failure also cannot be reported as a successful look merely because stdout contains an `ok:true` envelope.

The repeated live probe exited 0, found helper PID 63627 and reported:

| Read-only measurement | Wall | Helper CPU |
|---|---:|---:|
| Separate warmup screenshot | 109.0 ms | not measured |
| Screenshot, 20 samples | p50 34.0 ms; p95 47.3 ms | 8.0 ms/look |
| Observe diff, 20 samples | p50 49.2 ms; p95 52.7 ms | 5.5 ms/look |
| Absent OCR wait, 3 s budget | 2996.6 ms; 11 looks / 1 OCR read | 360 ms |
| Five OCR reads | first 64.2 ms; rest p50 55.8 ms | first 0.0 ms; rest p50 5.0 ms |
| Five seconds idle | 5 s | 2.0 ms/s |

CPU is process `ps time`, quantized at approximately 10 ms; the measured 0.0 ms first-read CPU is below that resolution, not proof of zero work. It includes all work the shared helper performed during each interval. Concurrent work can contaminate it; a helper restart or multiple matching helper processes still prevents a reliable single-process CPU claim. This run warmed an already running helper; cold-start ordering is proven by the focused regression, without deliberately restarting the product. Root's earlier batch-five-waits result (116.7 → 28.5 ms) remains a bridge RTT diagnostic: waits earn zero APM.

## Desk and caller identity: source evidence, no product lock implemented

Source references below describe the checkout base above. Interleaving is a source-established possibility; no competing worker or existing user window was driven to demonstrate it.

| Evidence | Consequence |
|---|---|
| `crates/zerocode-hookd/src/lib.rs:380` — `ComputerRequest` contains only `argv`, `evidence`, `answer`. `BrowserRequest` at :365 additionally has an optional pane field. | Computer requests do not carry caller pane/worker identity. The browser comparison does not make its optional header an authenticated identity. |
| `crates/zerocode-shell/src/hooks.rs:223` mints one bridge computer token at :230; :489–497 passes that shared capability path/token to launched agents. `crates/zerocode-hookd/src/lib.rs:906` checks the dedicated token and :928 enqueues only argv/evidence/answer. | The capability authorizes this window's Computer Use road; it does not distinguish the current desk owner among authorized panes. No token value was read or changed during this task. |
| `crates/zerocode-core/src/computer_use.rs:3594` builds the computer shim with `pane_header: None` at :3615. | The public shim itself supplies no caller pane header for this request type. |
| `crates/zerocode-shell/src/agent_tools_runtime.rs:2087–2089` receives a request then spawns an independent async task. :2132 and :2151 route each batch/recipe step to `desktop_step`; :2375 invokes the per-step closure. | The receive loop does not reserve the desk for an entire batch/recipe. Another request can run between its steps. Request receipt order is not a sequence ownership guarantee. |
| `crates/zerocode-shell/src/computer_use/mod.rs:105–125` holds `CLIENT`'s mutex across one `session.request`. | One helper request is serialized; the mutex is released after that call. It is not a lease across an observe/act sequence, a whole high-level operation containing several helper calls, or a complete batch. |
| `crates/zerocode-shell/src/agent_tools_runtime.rs:2238` `desktop_step` uses `answer_computer_command`, then evidence. :2521 checks stopped state and open confirmation for actions; `computer_loop` :2107–2169 also checks whether its caller still waits. | Stop, confirmation and caller-disconnection guards remain per step. They do not supply caller ownership. The runner retained these guards and did not bypass capability, use `--allow-self`, resume, or reset budgets. |
| `crates/zerocode-shell/src/computer_use/observe.rs:81–107` uses caller-provided `viewer` in the last-frame cache key. `crates/zerocode-shell/native/computer-use-macos/Sources/ZeroCodeComputerUseMacOSCore/AgentSessionOwnership.swift:14` tracks authenticated helper connections. | Viewer IDs separate observation baselines, not authenticated desktop ownership. Native connection ownership describes helper-session lifetime, not ownership of a worker's multi-step desktop sequence. |

A possible schedule is A's batch step 1 → B's action/observation → A's step 2. B can change focus or the helper's snapshot cache before A continues. Coordinates can then target a changed scene; cached indexes can be invalidated by another observation. The global helper mutex limits simultaneous provider calls but cannot prove A still owns the intended scene at its next dispatch.

Root review should decide how an authenticated caller identity, owner/lease epoch and a short sequence boundary belong in the product. An arbitrary `--owner`/pane header alone would not authenticate the holder. Any design must still admit read-only status/stop and let the person's stop or handoff win; taking one giant mutex around a waiting batch/confirmation could break those controls. No such product changes or claims of enforced ownership are in this commit. Ledger acquisition/release was coordination, not a product-enforced lease.

## The unconditional 400 ms window recovery

`crates/zerocode-shell/native/computer-use-macos/Sources/ZeroCodeComputerUseMacOS/main.swift:1034–1037` calls `recoverWindow` unconditionally at the start of `click`, before either semantic or coordinate dispatch. `recoverWindow` at :1898 un-hides/activates the app (:1903–1906), selects the requested/focused window (:1917–1934), un-minimizes/raises/focuses it (:1935–1939), and always executes **`Thread.sleep(forTimeInterval: 0.4)` at :1941**. `performSecondaryAction` at :1139–1152 instead performs the advertised action directly, and `actionResult` at :434 still returns an observed app snapshot afterward.

Observed calibration: coordinate click 732.27 ms, semantic `AXPress` 236.01 ms. Ten batched coordinate clicks averaged 654.32 ms/call including transport amortization. The 400 ms sleep is concrete source evidence for part of this gap; this small comparison does not attribute every other millisecond.

A safe candidate for root review is to skip recovery/wait only after a **fresh dispatch-time check** establishes that the exact requested PID and window are already frontmost/focused, visible and usable, with the expected window identity and bounds; verify the relevant input target as needed for the action. “The app is running” or “some window of this app is focused” is insufficient for multi-window apps. When activation, un-minimizing or a Space/window transition is actually necessary, recover and poll the exact target's readiness under a bounded timeout; refuse or reobserve if it never becomes ready, rather than treating a fixed sleep as proof. Revalidate close enough to input delivery that another caller cannot steal focus between the check and event; the sequence-owner issue above remains relevant. Preserve target/self/blocked-app checks, confirmation, stop/pacing and post-action verification. No sleep was removed or shortened in product code here.

## Latest-frame boundary

`crates/zerocode-shell/src/computer_use/observe.rs:209` returns a plain look immediately when `settle` is not true. Its :248–281 `look` takes app snapshots with `getAppState`; only ordinary desktop looks without region/marks use `eye::frame`. Regions and marked desktop looks use a fresh capture. `crates/zerocode-shell/native/computer-use-macos/Sources/ZeroCodeComputerUseMacOS/ScreenEye.swift:301–334` returns the newest buffered frame and caches its encoded answer by repaint sequence; it does not wait for a caller's preceding action.

The helper's frame has `seq`, but the ordinary observe result construction at `observe.rs:321–335` does not propagate it; `computer_use/eye.rs:122–133` also removes it from the desktop screenshot answer. No caller/owner epoch or action-correlated frame timestamp is attached to that plain result. Thus a successful latest-frame reply is not a caller-specific proof that the action has painted. `observe --settle` (:213 and `eye.rs:284–320`) uses repaint history and the helper's last action; it still does not identify which worker owns that action. These are root-owned review points, not evidence that any screenshot in this trial was actually stale.

## Guards, scope and verification

- Initial checks: permissions granted, no confirmation/stop, hotkey armed and audible, screen unlocked. HID idle was first only 0.78 s, so no fixture was opened then. After idle reached 26.2 s, ledger m-3828 announced acquisition. The original hands/random loops reused `Bench.guard` continuously but checked HID only before opening; no stop/signal refusal occurred, but their record cannot establish absence of physical input during the run. The later paired eye test added a physical HID watcher and completed without interruption. The final reusable runner arms that watcher after every launch as well. If HID changes, it leaves the fixture untouched and reports stopped; no automatic resume/cleanup follows.
- The runner reuses `Bench` stop/hotkey/question/capability handling and `Signals`; its short-lived CLI children run in their own process groups and are killed on interruption. Each quit verifies nonce, PID/executable ownership, current guards and actual termination. It never force-quits an existing app. The three owned fixtures produced 192, 10 and 20 native correct events, zero errors, and all terminated normally.
- Native fixture compilation via `swiftc` exited **0** for each of the three disposable app bundles. No package installation, Cargo/npm build/test, additional agent, or product Rust/UI edit occurred.
- `python3 -m unittest discover -s tools/computer-bench -p test_fixture_apm.py`: **exit 0**, 9 focused tests. These cover no-op/replay, wrong-order/extra events, changed identity, bad geometry, stop-before-act/quit, physical HID and its launch wiring, fresh semantic indexes and APM counting.
- `python3 -m unittest discover -s tools/computer-bench -p test_eye.py`: **exit 0**, 4 tests, including cold-helper ordering and subprocess-exit correctness.
- `git diff --check`: **exit 0**. Live CLI runners and read-only eye probe: **exit 0**. Root owns Rust integration gates, including `cargo clippy --all-targets -- -D warnings`; this worker did not run them under its explicit task restriction.

Raw owned-fixture logs and event oracles are in `/private/tmp/zc-apm-t3819-{hands,random,eyes}`; the repaired eye probe is `/private/tmp/zc-apm-t3819-eye.json`. They are temporary and may disappear on system cleanup. Their file hashes and aggregate results are in the committed JSON. This report lives inside the worker worktree and that copy dies with worktree cleanup; the commit carries the report and results for integration. Product screenshot files are also temporary. The report contains all numeric conclusions without requiring those images.

Reproduction: read `skills/computer-use/SKILL.md`, check/announce the desk through the ledger, then use `fixture_apm.py prepare <new-private-dir>`, `open`, `hands --actions 180`, and guarded `close`. For the LLM lane prepare with `--mode random`, read every `--compact` look and issue one explicit `act --labels Amber|Blue` per model turn. `eyes --rounds 10` runs the paired latency probe on an alternating fixture. For coordinate batches explicitly select `--input-path coordinate --batch-size 5`. Never restart an interrupted session automatically or reuse an old oracle as a fresh run.

The counting and stop rules follow [[wiki/computer-use-has-a-user-requested-100-apm-floor]], [[wiki/a-bench-oracle-must-fail-a-run-that-did-nothing]] and [[wiki/a-bench-runner-never-acts-after-a-stop]] (vault source for the user requirement: `raw/2026-09-12-user-computer-use-apm-100-requirement.md`). Durable concept for root's vault update: a shared provider mutex serializes calls, not ownership of an observed multi-step task. Vault edits are left to root because this worker owns only `tools/computer-bench` and scoped fixture files.
