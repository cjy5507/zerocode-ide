# Reflex runtime probe (macOS, t-6765)

Measures the helper's reflex runtime on its own clock, in an optimized build,
with the product's code: `OperatorHand`, `ReflexSession`, the rule book, the
leases and the receipts are compiled from the helper's Core sources. Only the
frame source (captures published at the table's rate), the kernel (a ball that
appears and disappears), the input monitor (it hears what the poster posts)
and the poster are the probe's. The poster records when an event would have
been handed to the window server and posts nothing: no event reaches the
system, so an app receiving input, a click completing or a display showing it
is not measured here.

## Build and run

```sh
out="$(mktemp -d)"
swiftc -O -swift-version 6 -parse-as-library -module-name ReflexProbe \
  crates/zerocode-shell/native/computer-use-macos/Sources/ZeroCodeComputerUseMacOSCore/*.swift \
  crates/zerocode-shell/native/computer-use-macos/Sources/ZeroCodeComputerUseMacOSCore/Perception/*.swift \
  tools/reflex-macos-probe/ReflexProbe.swift -o "$out/reflex-probe"
"$out/reflex-probe" --self-test
"$out/reflex-probe" --fixtures crates/zerocode-core/fixtures --seconds 60 --warmup 2 --trials 40 --hold-ms 300
```

## What it reports

- `sustained`: one run of `valid_basic`'s plan (its rule firing on the ball,
  `max_fires` the plan's whole action budget over the macro's two leaves) for
  `--seconds`, captures at the reflex table's `frames_per_second`. From each
  receipt after `--warmup`: the decision frame's publish to the leaf's first
  event (`decisionToFirstEvent`, by leaf), the part of that spent waiting for
  a capture newer than the lease's source (`captureWait`, kept apart as the
  contract asks), the decision frame to the leaf's admission, the outcomes,
  the leaves a minute, CPU of this process (one core = 100%) and the load
  averages before and after.
- `stopToRelease`: ABBA, `--trials` each. Arm B is the product's hand — a key
  held on it, the stop called from another thread at a random moment of the
  hold, measured from the stop call to the release's post. Arm A reproduces
  the helper before the hand in the same process: `holdKey` slept the whole
  hold with `usleep` and a stop only set a flag, so the key came up when the
  hold ended. Arm A is a model of the old code path, not the old binary.

Percentiles are nearest-rank. The first event of a glide is due one table tick
(`pointer_tick_ns`) after the leaf starts and cannot go before a capture newer
than the lease's source; both are in the number, and the capture wait is also
reported alone.
