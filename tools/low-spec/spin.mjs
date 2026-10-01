// The ruler of the low-spec profile: a fixed CPU loop, timed. The ratio of its
// low-spec time to its normal time is how much slower `taskpolicy -b` made the
// machine for plain script work.
const started = performance.now();
let x = 0;
for (let i = 0; i < 3e8; i += 1) x = (x * 31 + i) | 0;
console.log(`CALIBRATE ${(performance.now() - started).toFixed(1)} ms (${x})`);
