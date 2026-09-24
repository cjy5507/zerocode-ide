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
say nothing about plans.

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
- the frame's capture time is known, no later than now, and at most
  `max_frame_age_ns` old; the delivery time never stands in for an unknown
  capture time;
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
