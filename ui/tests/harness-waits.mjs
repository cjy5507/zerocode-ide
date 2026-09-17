/* Browser-side completion boundaries. Install into each fixture document;
 * predicates name the state being awaited, and a timeout is always a failure. */
export async function installHarnessWaits(page) {
  await page.evaluate(() => {
    window.__UNTIL__ = (predicate, label, timeout = 2500) => new Promise((resolve, reject) => {
      let frame;
      const timer = setTimeout(() => {
        cancelAnimationFrame(frame);
        reject(new Error(`Timed out waiting for ${label}`));
      }, timeout);
      const check = () => {
        try {
          if (predicate()) {
            clearTimeout(timer);
            resolve();
          } else frame = requestAnimationFrame(check);
        } catch (error) {
          clearTimeout(timer);
          reject(error);
        }
      };
      check();
    });

    // Observe the actual scheduled work; holding a named callback lets a test
    // deliver a pre-release flight after mouseup without relying on CPU load.
    window.__WATCH_SCHEDULES__ = (names) => {
      const watched = new Set(names), held = new Set(), pending = new Set();
      const original = {};
      for (const [schedule, cancel] of [["setTimeout", "clearTimeout"], ["requestAnimationFrame", "cancelAnimationFrame"]]) {
        original[schedule] = window[schedule];
        original[cancel] = window[cancel];
        window[schedule] = (callback, ...args) => {
          if (!watched.has(callback?.name)) return original[schedule].call(window, callback, ...args);
          const entry = { name: callback.name, deliver: null, cancel };
          const invoke = (...values) => {
            entry.deliver = () => {
              if (!pending.delete(entry)) return;
              callback(...values);
            };
            if (!held.has(entry.name)) entry.deliver();
          };
          entry.id = original[schedule].call(window, invoke, ...args);
          pending.add(entry);
          return entry.id;
        };
        window[cancel] = (id) => {
          for (const entry of pending.keys()) {
            if (entry.cancel === cancel && entry.id === id) pending.delete(entry);
          }
          return original[cancel].call(window, id);
        };
      }
      return {
        pending: (name) => [...pending.keys()].filter((entry) => entry.name === name).length,
        hold: (name) => held.add(name),
        release: (name) => {
          held.delete(name);
          for (const entry of [...pending.keys()]) if (entry.name === name) entry.deliver?.();
        },
        restore: () => {
          for (const name of [...held]) {
            held.delete(name);
            for (const entry of [...pending.keys()]) if (entry.name === name) entry.deliver?.();
          }
          Object.assign(window, original);
        },
      };
    };
  });
}
