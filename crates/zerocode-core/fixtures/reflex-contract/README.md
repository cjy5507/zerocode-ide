# Reflex contract cases

`manifest.json` names the plan and wire cases. Every `.json` file here is
written in the canonical form below and ends with one newline, so a plan
case's wire is the file's own bytes inside `{"expected":…,"plan":` and `}`:
Rust and Swift both feed those bytes to their real decoders and compare
their re-encoding and hash against them. `lease_cases.json` holds one
base frame and one base lease; each case replaces some of their top-level
fields and names the input, the host time and the expected verdict. Rust and
Swift feed every case to their real `permits` and `observe` methods; the
tests hold no second copy of the rules. Plan parity and lease parity are
separate claims: the plan cases say nothing about leases, and the lease cases
say nothing about plans. `observation_cases.json` does the same for what a
perception kernel answers (below). `limits.json` is the reflex table exactly
as `limits_wire` sends it to the helper (plus the newline).
`capability.json` is the capability table itself and `run_policy_cases.json`
the run policy's cases (both below).

## Version 2: a colour detector carries its spec

A colour detector carries its `color` spec (`game_state::ColorSpec`, R5's
`fixtures/game-state`) inside the plan, so the plan hash covers what the
kernel reads. A colour detector without one, or with a spec
`game_state::validate_color` refuses, is `perception`; a colour detector's
structure is its spec's layout, so its `patches` is 1 and its `scale` 1/1
(`unsupported` otherwise). These checks come after the detector's own ROI and
work checks, so every version-1 case keeps its verdict: the 32 cases were
re-hashed under version 2 with a valid spec on each colour detector, and
`bad_version` is now the version-1 document. `future_version`,
`color_missing`, `color_spec_refused`, `color_patches`, `color_scale` and
`valid_cells` are the cases version 2 adds. A version-1 plan is refused,
never read as a colour detector with nothing to read its ROI with.

## Identifiers (t-9205)

Every id a plan carries — a detector, a rule, a macro, an action — and a
run's own id is 1 to `MAX_IDENTIFIER_BYTES` (64) bytes of `[A-Za-z0-9_-]`.
`id_at_limit` (a 64-byte rule id, `ok`) and `id_over_limit` (65 bytes, `id`)
pin the bound on both sides.

## Capability table (t-9205)

`capability.json` is the one table of what each surface may claim — not a
copy of one: the window compiles the file in (`reflex::capability_wire`),
sends its bytes with every start, and the helper decodes the same bytes
(`ReflexContract.decodeCapabilities`). Each surface has two columns:
`live_reflex` (a reflex run) and `instant_pointer` (`--instant` on the
helper's own verbs), because running plans does not teach the verbs an
instant pointer. The table names the plan `contract` and the `run_policy`
version it was written against; a table of another version claims nothing on
either side, so a helper built for another contract is unsupported.

The macOS desktop alone claims `live_reflex` — the one surface with the live
frames a run reads — and no surface claims `instant_pointer`. A claim is not
a run: the window still asks the helper whether its kernel is installed and
which contract and run policy it reads (`supported`), and the person's
`computer_live_reflex` setting, off until they turn it on (`enabled`).

## Run policy (t-9205)

A start carries a run policy beside the plan — `{"renew":…,"run_ns":…,
"version":1}`, canonical — and the plan and its hash stay what they are.
`run_ns` is at least one nanosecond and at most the table's `max_run_ns`,
counted from the moment the helper first accepts the start: one deadline
that no renewal, pause or answer lengthens. Without `renew` a rule's
`max_fires` is its total for the run, the plan's own meaning. With it a rule
whose `max_fires` are spent gets exactly those back — armed or not, its last
fire, the last frame read, a leaf in flight, the tracks, the epochs, the
evidence taken back, the leases issued, the operator's session count and a
release left unconfirmed all stand. The version is read before anything
else: an absent version or another integer is `version`, a version that is
not an integer is `wire`; then every field must be known and present
(`wire`), and the length inside the table (`budget`).
`run_policy_cases.json` pins this for both decoders; a helper that does not
say `runPolicy: 1` in its handshake is never sent a start.

## Canonical wire

The wire is compact UTF-8 JSON with every object's keys in byte order at
every depth; arrays keep their order and numbers are integers written
exactly. `plan_hash` is the SHA-256 of the same form with the `plan_hash`
value set to the empty string (the key stays). The form does not depend on
how a JSON library keeps its maps: Cargo turns serde_json's `preserve_order`
on for every crate in a build that includes one asking for it (the shell,
hookd), shipped app included, so Rust sorts the keys itself. A wire whose keys are out of order at any depth is refused, never
normalized and run (`wire_unsorted_top_level`, `wire_unsorted_nested`).

## Action lease

`permits` is true only when all of these hold:

- the frame and the lease name the same nonempty run, and the lease allows
  the input;
- the frame is ready and has a positive pixel width and height;
- the frame's capture time is known, and the time it was in hand — its
  capture time, or its delivery time when that came first
  (`observed_host_ns`) — is no later than now and at most
  `max_frame_age_ns` old; the delivery time never stands in for an unknown
  capture time. ScreenCaptureKit stamps a frame with the time the display
  shows it, which can run ahead of the frame's delivery (t-10127):
  `capture_ahead_of_its_delivery` and `capture_far_ahead_of_its_delivery`
  are such frames, read after their delivery; `future_capture` has no
  delivery time and `delivered_after_now` was not yet delivered, so both are
  refused; `age_counts_from_a_capture_before_its_delivery` keeps the age of
  a frame delivered late counting from its capture;
- the lease was issued no later than now, ends after it was issued, lasts at
  most `max_lease_ns`, and its target proof ends no later than the lease;
- the target id is nonempty and the target ROI has a positive width and
  height;
- the owner, stream, geometry, plan and clock epochs of the frame and the
  lease are equal;
- the frame's capture sequence is greater than `source_capture_seq`, so the
  capture that issued the lease never satisfies it, while any newer capture
  does, whether or not its repaint sequence changed;
- now is strictly before the lease end and the target proof end;
- a child slot remains (`used_children < max_children`) and `max_children`
  is at most `max_expanded_actions`.

Age and length limits are inclusive; end times are exclusive.

## Frame cursor

A cursor belongs to exactly one run: a frame from another run is refused, so
a new run needs a new cursor. It observes only ready frames with a nonempty
run id, positive pixel extent, a known capture time and a positive capture
sequence. Within a stream epoch the capture sequence advances and the repaint
sequence does not decrease. A later stream epoch starts a new sequence; an
older one is refused. A refused frame leaves the cursor unchanged. The cursor
has no clock and grants no input: the lease checks capture age and expiry at
action time.

## Macros

Every declared macro, including one no rule uses, must resolve each macro
reference to a macro, be acyclic, have at most `max_macro_depth` nested macro
calls on any path below it and expand to at most `max_expanded_actions`
actions. Unused valid macros are allowed. Rule roots additionally multiply
by `max_fires` in the total executable action budget.

## Observations

What a kernel answers for one detector on one frame is data, never
permission. `admissible` is true only when all of these hold:

- it names the detector, and exactly the frame the runtime handed over —
  run, stream, capture, repaint, geometry, plan and owner epochs, clock and
  capture time (the runtime observes only frames its cursor accepted, so the
  capture time is known);
- known and unknown exclude each other: a value exactly when no reason is
  given, and a known value is never negative (0 means nothing is there);
- the detector's ROI has a place in the frame (`game_state::frame_roi`, the
  one placement). Without one only an unknown answer with no target and no
  cells is honest; with one, the observation's scale is that placement's;
- a target only on a known nonzero value, with a nonzero track, a non-empty
  pixel hitbox inside the placed ROI and the aim point inside the hitbox;
- cells only for a cells layout, one per cell, each a class of the palette or
  0 with a reason — never both — and a share of at most 1000 permille;
- no more samples than the tick allowed.

`aim` carries the point forward by the velocity (pixels a second, integer
division toward zero) from the capture time to the asked time — a capture
stamped after the asked time carries nothing, neither forward nor back
(`a_capture_ahead_of_the_aim_carries_nothing`,
`a_capture_far_ahead_carries_nothing`) — and answers it only while the time
does not run past the frame-age limit, no carry overflows, and the square of
the uncertainty around the point stays inside the hitbox carried the same
way. A lease's time proof is the runtime's: never later than the time the
frame was in hand plus `max_frame_age_ns`.
