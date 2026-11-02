# What the alerter promises

Time is a whole number of ticks that the caller passes in; nothing reads a clock. A power loss
raises `PowerLoss` out of whatever was running and all memory is gone. The next boot makes a new
`Alerter` with the same rules, store and transport, and the caller repeats the call that was cut
off. The store and the transport are the only things that survive.

**Firing.**
1. A rule fires when its metric has been at or above its threshold for `for_ticks` observations in
   a row. Firing opens a new incident, numbered per rule from 1, and queues one `fire` message.
2. While a rule fires, a reading below the threshold starts a cooling period, and a reading at or
   above it cancels that period. When a period has lasted `quiet_ticks`, counted from the tick of
   the reading that started it, the incident is over and one `resolve` message is queued. With
   `quiet_ticks` 0 that is at once. `observe` and `tick` both notice this.
3. When an incident has been open for `escalate_after` ticks with no `ack`, one `escalate` message
   is queued, once per incident. `observe` and `tick` both notice this. An `ack` or the end of the
   incident cancels it. A rule with `escalate_after` None never escalates.
4. A reading whose tick is not later than the last tick seen for that rule changes nothing. A source
   repeats its last reading after a restart.

**Messages.**
5. A message is a dict with `key`, `rule`, `kind`, `incident` and `tick`, the tick it was queued
   at. Its key is `<rule>:<incident>:<kind>`, the same however often it is sent. Receivers discard
   a key they have already seen, so sending a message twice is harmless, and losing one is not.
6. `observe` and `tick` both try to send what is due, oldest first. A message that fails with
   `Unavailable` is tried again after a delay of 1 tick, then 2, 4, 8 and 16, and 16 from then on.
   The delay runs from the tick of the failed try.
7. A rule's messages arrive in the order they were queued: one waiting holds back the later ones
   of that rule, and it does not hold back those of other rules.

**Restarting.**
8. Everything above survives a power loss at any instant: a run with power losses delivers
   exactly the same keys at the same ticks as a run without them, apart from repeats. A message
   queued is never lost, a state change is never made twice, and a delay and its count carry on
   where they were. The state in the store is plain JSON.
