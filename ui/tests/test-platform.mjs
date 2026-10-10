// The platform a window or settings run judges as.
//
// It is the machine's own `process.platform` unless `ZO_TEST_PLATFORM` names
// another one (`win32`, `darwin`, `linux`). A Windows decision path then runs
// on any machine: the page is told it is on Windows (`navigator.platform`,
// `userAgentData`), and the expectations take the Windows keys and spellings
// from the same value. The runners keep the default, so their runs are the
// same as before.
const REPORTED = Object.freeze({ darwin: "MacIntel", win32: "Win32", linux: "Linux x86_64" });

const named = process.env.ZO_TEST_PLATFORM || process.platform;
if (!Object.hasOwn(REPORTED, named)) {
  throw new Error(`ZO_TEST_PLATFORM must be one of ${Object.keys(REPORTED).join(", ")}, not "${named}"`);
}

/** The platform this run judges as: `darwin`, `win32` or `linux`. */
export const TEST_PLATFORM = named;

/** The navigator platform the page reports, or null when the page reports the machine's own. */
export const PLATFORM_OVERRIDE = named === process.platform ? null : REPORTED[named];

/** The primary modifier as a synthetic-event spread: ⌘ on macOS, Ctrl elsewhere. */
export const PRIMARY_EVENT = named === "darwin"
  ? Object.freeze({ metaKey: true })
  : Object.freeze({ ctrlKey: true });
