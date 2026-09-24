# Own-window capture and pointer probe

This standalone macOS 14.4+ harness measures a disposable 800×500 AppKit
window. It uses `SCShareableContent.currentProcess`, never a list of other
applications or windows. It does not change product code or measure task APM.

Compile to an output directory outside the checkout:

```sh
swiftc -O -warnings-as-errors -parse-as-library \
  tools/realtime-design-probe/CaptureProbe.swift -o /tmp/CaptureProbe
/tmp/CaptureProbe /tmp/capture.json
python3 tools/realtime-design-probe/summarize.py /tmp/capture.json
python3 -m unittest discover -s tools/realtime-design-probe -v
```

The default run posts no input. It draws at a nominal 60 Hz, measures an
eight-second rendering baseline, then changes one stream through 30/60/60/30
fps with two seconds of warmup and twelve seconds of measurement per arm.
CPU is this process's user+system CPU, including drawing and instrumentation;
it excludes WindowServer, GPU and system power. Frame age uses SCK displayTime
and the host mach clock. Callback age and age at four-millisecond polling are
reported separately. Neither is a photon-to-action measurement. A configured
minimum frame interval is not a guaranteed delivery rate.

Pointer measurement requires **explicit ledger permission before execution**.
The permission used for the initial experiment required all of these:

1. Accessory app; never activate or makeKey. Only orderFrontRegardless.
2. Owned window interior only; no click, keyboard or scroll. Two short
   intervals at most, with less than five seconds of movement in total.
3. Mouse/key combined-session idle for at least three seconds, waiting at
   most sixty seconds for that condition.
4. Return to the starting pointer location and close the owned window.
5. Capture this process's window only. No other app/window/device enumeration.

Permission for a later run is not inherited from this README. A separate
permission allowed one extra interval under two seconds, sampled solely by
`CGEvent(source: nil).location` every four milliseconds. That mode is:

```sh
/tmp/CaptureProbe /tmp/waypoints.json \
  --permitted-movement --movement-only --cli-only
```

The public CLI performs an 80-point move with 12 steps and returns with 24.
The four-millisecond samples count changed positions; they are not exact
event-post timestamps. CLI process/IPC time and observed path span remain
separate. No-op cursor samples produce zero observed movement.

Without `--cli-only`, a second half-second interval can measure local color
classification and CGEvent post, **only if the disposable executable already
has post permission**. The program never prompts for or changes permissions.
`post()` returning, tap creation, or a CLI success response does not prove
delivery to the receiving application. A sample with no observed reception
must retain that limitation. This harness has no click/result oracle, and
always reports APM as unmeasured.

Raw measurements belong outside git. Missing observations remain null;
the summarizer rejects empty capture runs and incompatible negative frame ages.
