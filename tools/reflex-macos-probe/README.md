# Reflex runtime probe (macOS, t-6765, t-9205)

Measures the helper's reflex runtime on its own clock, in an optimized build,
with the product's code, and judges each run with an oracle of its own:
`OperatorHand`, `ReflexSession`, the rule book and its renewal, the leases,
the receipts and their acknowledgement, the run's deadline alarm and — with
`--kernel perception` — R5's `PerceptionKernel` are compiled from the
helper's Core sources. Only the frame source (captures published at the
table's rate), the scripted kernel of the default mode (a ball that appears
and disappears), the input monitor (it hears what the poster posts), the
poster, the receipt collector and the fixture the poster posts to are the
probe's. No event reaches the system: the poster hands each event to a
synthetic fixture and records when it would have been handed to the window
server, so an app receiving input, a click completing or a display showing it
is not measured here.

## Build and run

```sh
out="$(mktemp -d)"
swiftc -O -swift-version 6 -parse-as-library -module-name ReflexProbe \
  crates/zerocode-shell/native/computer-use-macos/Sources/ZeroCodeComputerUseMacOSCore/*.swift \
  crates/zerocode-shell/native/computer-use-macos/Sources/ZeroCodeComputerUseMacOSCore/Perception/*.swift \
  tools/reflex-macos-probe/ReflexProbe.swift -o "$out/reflex-probe"
"$out/reflex-probe" --self-test --fixtures crates/zerocode-core/fixtures
"$out/reflex-probe" --fixtures crates/zerocode-core/fixtures --kernel perception --renew --max-fires 4 \
  --seed 20260925 --seconds 60 --warmup 2 --trials 40 --hold-ms 300 --ticks 400
```

## Kernels

- `--kernel scene` (the default): the probe's scripted kernel answers the ball
  where the scene shows it; every capture lends one blank buffer.
- `--kernel perception`: each capture is drawn — black ground and, where the
  scene shows it, the ball pure red inside `valid_basic`'s ROI — and R5's own
  kernel reads those pixels (frame → observation → rule candidate). This is
  the readiness evidence for a helper that installed the kernel: its handshake
  flag says a kernel is there, this says it sees.

## The oracle

A run exits 1 when the oracle fails it, whatever its numbers say, and 0 when
it passes. The oracle reads the fixture's own record and the probe's own
counts — the runtime's receipts only as a claim to check against what the
fixture got — and every check is exact:

- the fixture starts clean (cleared, and read so before the run);
- the kernel read the frames;
- the kernel named the ball, with a target, on a capture that showed it, and
  named nothing on a capture that hid it;
- every event the hand posted reached the fixture;
- no press landed where the ball was not shown on a capture within the
  reflex table's frame age, and the ball was hit;
- every click receipt marked done is a hit at the fixture;
- every button that went down came up, at the fixture and at the hand.

`--fault` makes a run the oracle must fail, in the probe's own parts:
`kernel-uncalled` (the frames are lent without calling the kernel),
`inert-poster` (events reach the monitor's echo but never the fixture),
`wrong-target` (the scripted kernel answers the other corner),
`restored-state` (the fixture keeps an earlier run's record and ignores being
cleared) and `kernel-blind` (the real kernel reads frames whose pixels never
show the ball the scene says is there). `--self-test --fixtures` runs a short
clean run of each kernel, which must pass, and each fault that applies to it,
which must fail; without `--fixtures` it checks the statistics and the seeded
schedule only.

## The run

`valid_basic`'s plan, its rule firing on the ball being there, `--max-fires`
times a quota under `--renew` (the run policy's renewal: a spent rule gets its
fires back once the hand is free, on a new edge) or times the whole run
without it (default: the plan's whole action budget over the macro's two
leaves). The run's deadline is its policy's length (`--seconds`), set when it
starts and rung by the product's alarm; the probe waits a quarter second past
it. A collector thread reads the receipts after the last one it holds and
acknowledges them every `--collect-ms` (the window's collector's road, without
the disk).

The stimulus: without `--seed`, 18 captures shown and 12 hidden, the ball
alternating between two corners (the t-6765 run). With `--seed`, each cycle's
lengths (10–11 shown, 3–4 hidden: about 257 appearances a minute at 60 fps)
and the ball's place in the ROI are drawn from the seed — a schedule the run
cannot see.

## What it reports

- `sustained`: `hitsPerMinute` and `misses` from the fixture (successful
  actions and wrong inputs — a move and its click are one action), `ups` and
  `downs`, `appearancesPerMinute` of the stimulus, what the kernel saw,
  `fires`, `renewals` and `renewalGap` (how long each spent quota waited to
  come back), `endedBy` (the deadline, normally), and from each receipt after
  `--warmup`: the decision frame's publish to the leaf's first event
  (`decisionToFirstEvent`, by leaf), the part of that spent waiting for a
  capture newer than the lease's source (`captureWait`), the publish of that
  newer capture to the event (`permittingFrameToFirstEvent`), the decision
  frame to the leaf's admission, the outcomes, CPU of this process (one core =
  100%) and the load averages before and after.
- `stopToRelease`: ABBA, `--trials` each (none with `--trials 0`). Arm B is
  the product's hand — a key held on it, the stop called from another thread
  at a random moment of the hold, measured from the stop call to the
  release's post. Arm A reproduces the helper before the hand in the same
  process: `holdKey` slept the whole hold with `usleep` and a stop only set a
  flag, so the key came up when the hold ended. Arm A is a model of the old
  code path, not the old binary. `productBothArms` pools the two B arms.
- `ticks` (with `--ticks N`): one perception tick's cost — R5's kernel reading
  one drawn frame per tick with the table's samples and no deadline — ABBA
  across the default and user-interactive thread classes, `N` ticks an arm,
  for the run's own plan and for one detector reading 256,000 samples; each
  arm's p50/p95/max and ticks over 5 ms, and the same pooled by class.

Percentiles are nearest-rank. The first event of a glide is due one table tick
(`pointer_tick_ns`) after the leaf starts and cannot go before a capture newer
than the lease's source; both are in the number, and the capture wait is also
reported alone.
