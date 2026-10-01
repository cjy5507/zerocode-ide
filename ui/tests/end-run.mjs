/* How a browser harness ends once its report is printed (2026-10-02).
 *
 * stdout into a pipe is asynchronous on macOS, and `process.exit()` drops what
 * is still queued: with a reader slower than the writer the report is cut at
 * the pipe's 64 KiB. The public CI's macOS leg cut the window suite's report
 * mid-suite — 244 of its lines, no FAIL line, no `N/M passed` — and still
 * exited 1, so the failure it was reporting never reached the log. A run here
 * writes the report to a file, which is synchronous, so it never showed here.
 *
 * So the exit code is set and the process ends by itself once its writes are
 * done; a handle a harness left open cannot hold it past the grace. */

/* Long enough for a slow log reader to take a report of a few MiB. */
export const END_GRACE_MS = 15_000;

export function endRun(code) {
  process.exitCode = code;
  setTimeout(() => {
    process.stderr.write(`the run ended by its ${END_GRACE_MS} ms grace: a handle was still open\n`);
    process.exit(code);
  }, END_GRACE_MS).unref();
}
