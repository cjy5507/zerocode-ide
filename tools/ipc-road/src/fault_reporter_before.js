// The window's fault reporter as `ui/shell-boot.js` had it at ba6d76230, before t-20972:
// the report's own rejection is left uncaught, so a backend that refuses it turns it
// into a fault, which is reported, which is refused. Kept verbatim as the 'before' of
// the storm pages (`--thumb storm`); the 'after' is read from the shell-boot.js beside it.
function describeWindowFault(event) {
  if (event.reason !== undefined) return `unhandled rejection: ${event.reason}`;
  return `${event.message} (${event.filename}:${event.lineno})`;
}

for (const fault of ["error", "unhandledrejection"]) {
  window.addEventListener(fault, (event) => {
    try {
      window.__TAURI__.core.invoke("log_window_error", {
        message: describeWindowFault(event),
      });
    } catch {
      // Reporting must never be the thing that fails.
    }
  });
}
