# Workflow quality baseline

`just baseline --lane a --repeat 2` runs deterministic tools through zo and the
window. `--task Q1` selects a single task; `--repeat` is bounded by `Limits`.
The runner writes `out/<timestamp>-<pid>/runs.jsonl` and one `report.md` table.
`baseline.json` owns the Fields column list, Grid, Limits and task boundaries;
`baseline::Fields` in the Rust integration test owns the closed serde types.
The same serde validator checks the window's JSONL before reporting success.

Lane A spends zero and uses no real credentials or accounts. `out/` is ignored
and can be removed with the worktree. Lane B is an explicit real-provider run,
described below; it has its own record schema and never gets mixed into Lane A.

## Real provider workflow (Lane B)

From a live ZeroCode coordinator pane bound to a run with automatic assignment
and handover policies off:

```sh
just baseline --lane b --authorize-real-run --task Q1 --repeat 1 \
  --agent claude --model claude-sonnet-5-5 --effort max
```

The ledger launches exactly the named agent, model and effort in a worktree.
It refuses a missing or substituted launch. It checks the reported checkout with
Git: a separate linked worktree in the runner's repository, with canonical paths,
and retains that proof in `worker-checkout.json`. The request's `inWorktree` flag
alone is not placement evidence.
Inherited Git plumbing variables are cleared for every child (including fixture
and judge processes), so a hook's repository context cannot redirect this proof
or fixture initialization.
The worker edits a disposable fixture,
and the independent judge must pass after a durable task completion. Before launch,
the unchanged seed is rejected by the edit-policy check; this does not run the
Rust oracle on the seed. Q1 (coding) and Q4 (worktree plus commit) are supported. Other scenarios
are refused rather than silently measured with mocks. A pane the window did not
launch may be able to read the ledger yet unable to summon: use a normal coordinator
pane, never another pane's token.

The default is one run and a 300-second workflow budget, shared by launch, polling
and the final judge. `--repeat` is limited to three; `--timeout-ms` is bounded to
1–900 seconds. Setup commands are individually bounded by that timeout. Failure,
timeout or Ctrl-C stops only the recorded attempt, with a separate five-second
cleanup budget, and closes its task after observing the stop. Human takeover is
left alone; a completed task is preserved. A transient stop failure is retried
once with the same idempotent request. A failed attempt prevents further repeats.
A lost launch response is recovered only from this new task's first recorded
attempt; later attempts are refused. If the bridge cannot
confirm a launch or stop, the failure report preserves the uncertainty and the
idempotent request for operator recovery; it does not claim the worker stopped.
Terminal Ctrl-C can also interrupt an in-flight bridge child; cancellation uses
the same recovery and cleanup path, and keeps any remaining uncertainty.
Evidence includes
the launch identity, both judge logs, a JSONL result and p50/p95 elapsed times.
The runner requires a clean tracked tree and records its source commit. The live
window build is unobserved, so this does not attest the native candidate binary.
Elapsed includes launch, work, judging and any failure cleanup. The current ledger does not expose
provider-effective model, human-origin input, token billing or memory samples for
this measurement: these stay `null`, and sample counts state the missing coverage.
No dollar spend-cap claim is made; the old Grid/spend-cap definitions belong to the
unimplemented provider comparison, not this explicit ledger run.

This lane uses the account the window already selected and contacts its real provider.
It is opt-in, separate from deterministic release correctness. Its coding/worktree
results do not claim native-window or browser coverage. The deterministic runner
checks are required by `just quality-runner-test`; no real-provider launch occurs
in that gate.

Completed worker panes and their product checkouts follow the ledger's normal
retention and reclaim policy. `worker-stop` closes a stopped pane; its checkout
still follows that policy. The runner does not call release or abandon. Disposable fixtures
and evidence remain under `out/`. The prompt prohibits reading answer patches, which
are still present in the product checkout; this is not a blinded model comparison.

## Fixture and judge contract

Each `tasks/Q*/prompt.txt` is one line. Each executable `verify.sh ROOT` returns
zero only for a verified task. ROOT is an isolated fixture checkout plus
`evidence/`; Q4 also has `worktree/`. The independent judges use git, process exit
codes, captured request envelopes, DOM/PNG files and durable ledger receipts.
They never score an assistant's final prose.
For Q1/Q3/Q4, the Rust oracle lives in `repo/tests/`, outside the allowed production
file. Changing it, or replacing an allowed file with a symlink, fails before
Cargo runs. The oracle still covers singleton, ordinary and reversed ranges.

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

## Required portable workflows

`cd zo-ide && just e2e` also runs `portable_workflow_acceptance` on every host,
including Windows. These tests execute the real zo binary over ordinary pipes
against an isolated loopback provider. They verify a completed tool conversation
across two processes, forced termination after a real file write followed by
restoration of the original instruction and paired tool receipts, and a failed
file read whose error cannot be replaced by the model's completion claim.

The portable tests clear the inherited environment, use temporary configuration
and session stores, disable external credentials and keychain reads, and contact
no real provider. They require neither Unix PTYs nor macOS heap sampling. Their
verdict comes from process status, actual files and captured request envelopes,
not timing or an assistant's final prose. They are required by `verify`; the
quality baseline below remains a separate measurement with required deterministic
workflow verdicts in package CI. Portable
pipe acceptance does not claim coverage of the Windows interactive TUI or native
window rendering.

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

The macOS CI job `quality-baseline` uploads the report, JSONL and evidence for
every run (including release tags). The package verifier requires its independent
workflow verdicts to pass before packaging or signing. Timing samples do not have
pass/fail thresholds. It remains separate from local `just verify` and real Lane B.
Local builds use the window's shared
build-coordination wrapper; ephemeral CI runners execute cargo directly.
