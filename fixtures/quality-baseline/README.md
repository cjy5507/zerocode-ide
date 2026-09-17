# Workflow quality baseline

`just baseline --lane a --repeat 2` runs deterministic tools through zo and the
window. `--task Q1` selects a single task; `--repeat` is bounded by `Limits`.
The runner writes `out/<timestamp>-<pid>/runs.jsonl` and one `report.md` table.
`baseline.json` owns the Fields column list, Grid, Limits and task boundaries;
`baseline::Fields` in the Rust integration test owns the closed serde types.
The same serde validator checks the window's JSONL before reporting success.

Lane A spends zero. Grid and the real-provider spend cap are definitions only;
`--lane b` fails before building or launching anything. No real credentials or
accounts are used. `out/` is ignored and can be removed with the worktree.

## Fixture and judge contract

Each `tasks/Q*/prompt.txt` is one line. Each executable `verify.sh ROOT` returns
zero only for a verified task. ROOT is an isolated fixture checkout plus
`evidence/`; Q4 also has `worktree/`. The independent judges use git, process exit
codes, captured request envelopes, DOM/PNG files and durable ledger receipts.
They never score an assistant's final prose.

- Q1: missing inclusive endpoint in `src/lib.rs`; singleton fails, reversed range remains covered.
- Q2: addition in `double` and inverted `isEven`; two Agent calls each read and fix one file. Calls use explicit blocking mode, because the TUI's omitted mode is detached. Both terminal receipts must reach the parent request. Their tasks are independent; dispatch is sequential to avoid racing the shared agent store's initial creation.
- Q3: the Q1 defect, interrupted after a persisted tool call. SIGTERM kills zo; the window is destroyed and reboots with the saved `interrupted` pane. Its actual `resume_session` request controls the continuation. The resumed provider request must carry the original user prompt.
- Q4: the Q1 defect; `EnterWorktree`, patch, commit. The branch and main's clean state are independently checked.
- Q5: stale page status; production `zerocode-browser` transport, a Chromium DOM read and an actual PNG under `evidence/`.
- Q6: stale handoff status; production `zerocode-orc send --type worker_done` and `task-update --result` transports. The result schema and actual changed file set must agree.

`answers/*.patch` are reference fixes, not judge inputs. Run
`python3 fixtures/quality-baseline/tests/judges.py` to verify every planted red,
answer green, missing evidence rejection, out-of-scope change rejection, and
running-vs-completed receipt rejection. Cargo subprocesses inside this test
inherit the outer build coordinator's slot (never nest its lock).

## What the measurements mean

Elapsed time includes process startup, workflow, shutdown and the independent
judge. Reports use **min of N**, while intervention and token counts are sums.
First-visible is the first PTY render byte after submission, including composer
echo, not model time-to-first-token. The PTY counter distinguishes scripted
input from human-origin writes; deterministic runs require zero human writes.
SSE usage is counted from actual message_start/message_delta usage envelopes,
including child calls. CPU records architecture and host RAM records bytes.
Q5's deterministic window script has no model tokens.

`ZO_PROBE_PAINT=<path>` retains its row decisions and also writes
`<path>.frames.jsonl`: turn start, per-frame draw-to-flush milliseconds and
pending render-channel blocks plus rendered lines, then turn end with the process id. Only an active probe
reads the queue or clock; the disabled hot path has a lazy-argument unit test
and `probe-off.rs` measures its branch against a control in an optimized build
(with the runtime flag opaque on every iteration, so it cannot be hoisted).
The existing row diagnostics run inside measured draws, so these are probe-on
paint costs. Turn-end heap collection is excluded from workflow elapsed time.

Window rows use the same pane frame door as the window gate (`__TERM_FEED__`:
the frames go into the stub backend's pull share and the pane pulls them on its
own cadence). An isolated replay of each task's result submits
`Limits.pane_burst_frames` deltas for `Limits.pane_samples` paints. Harness counters record outstanding frames;
CDP submission marks are paired with same-thread engine `Paint` completions.
LayoutCount and the timeline are retained in `evidence/window-paint.json`.
This measures the pane renderer under a fixed workload, not the task's native
window latency. Q3 still independently verifies renderer destruction/restore.

`rss_peak_mb` keeps its schema name but means **maximum sampled turn-end live
malloc bytes / MiB**, never ps RSS or a continuous peak. `heap.py` attaches to
the live zo process after its turn-end mark, or to the pane renderer process
identified by the CDP trace. Exact `heap --showSizes` count/byte rows are folded
by allocation size and must sum to the all-zones totals. `Sizes:` labels are
rounded, so they are not summed. Window rows use the larger zo/renderer sample,
not their sum. Chromium's PartitionAlloc/V8 mappings are outside this malloc
histogram; it is not a total renderer footprint. Raw heap output and folded
classes remain in evidence. Sampling requires macOS's heap tool; other hosts
fail explicitly instead of substituting an unrelated memory counter.

The window suite shares `ui/tests/window-boot.mjs` with `window.mjs`. Its backend
is deterministic: native Tauri/WKWebView and the live orchestration persistence
engine are not under test. Production shims are generated by
`cargo run -p zerocode-core --example quality_baseline_shims -- <output>`;
their requests go to fresh loopback endpoints and their child environment has
no live ZeroCode coordinates. This is a wire/renderer baseline, not evidence
about provider reasoning quality or the native browser's speed.

The macOS CI job `quality-baseline` is warning-only and uploads the report,
JSONL and evidence for every run (including release tags). It is separate from
`just verify` and does not gate signing. Local builds use the window's shared
build-coordination wrapper; ephemeral CI runners execute cargo directly.
