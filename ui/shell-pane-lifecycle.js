/* ---- what the window remembers about a pane ---------------------------------
 *
 * One owner for it, and one way to forget it.
 *
 * A shell leaves a lot behind in this window: what its agent last reported, the
 * words it said, the helpers it ran, the page its conversation stood on. Each of
 * those was a keyed collection in whichever file needed it, and the end of a
 * shell was a list of `.delete(term)` lines that had to name every one of them.
 * A collection added without its line leaked quietly, and the next shell to wear
 * the same id opened on a stranger's badge, sentence or helper row.
 *
 * A collection that belongs to a pane is declared here instead:
 *
 *     const paneModels = paneLifecycle.map("paneModels");
 *
 * It is a real Map (or Set), so every reader and writer stays as it was. What
 * changes is who owns it: `paneLifecycle.dispose(term)` takes the pane out of
 * every collection declared this way. A collection is covered the moment it
 * exists, and there is no list to keep in step with the declarations.
 *
 * What a caller has to know:
 *
 *   map(name, { owns, release })   a Map whose entries belong to panes
 *   set(name, { owns })            a Set whose members belong to panes
 *       A name is declared once; declaring it again throws.
 *       `owns(term, key)` says which keys belong to a pane, for a collection
 *       whose key is not the bare id (`term:3`, `sub:3:a-1`).
 *       `release(key, entry)` is for an entry that holds something the Map
 *       cannot let go of by itself, such as a page in the document or a timer.
 *       It runs for every entry the Map loses — `delete`, `clear` or a disposed
 *       pane alike — before the entry leaves, and the entry leaves even when
 *       `release` throws.
 *   dispose(term)                  the pane has ended
 *       Empties every declared collection of it and answers the names that held
 *       something. Disposing again is safe and answers nothing. A `release`
 *       that throws does not stop the rest: every collection is emptied first,
 *       and the first error is thrown after.
 *   holds(term)                    the names that still hold the pane
 *       Empty after `dispose`, which is what "the next shell at this id starts
 *       empty" means.
 *   sizes()                        { name: entries } for every declared name
 *       In declaration order. The leak census (`__LEAKCHECK__`) spreads it, so
 *       a collection declared tomorrow is counted without being listed anywhere.
 *   ask(request, receive)          a question about panes, answered later
 *       The backend answers on its own time, and a pane can end while an answer
 *       is out; written back after, the answer undoes the disposal and the next
 *       shell at that id opens on a stranger's state. `request` is a function
 *       returning the answer (or its promise); `receive(answer, ended)` is what
 *       writes it, and `ended(pane)` says whether that pane ended while the
 *       question was out — including in the gap between the answer arriving and
 *       `receive` running. The fence is open from the question to the end of
 *       `receive` and no longer, so it remembers only the panes that ended in
 *       that while and is gone with the question: nothing is kept for good, and
 *       a caller cannot forget to close it. A pane that merely has no tab is not
 *       an ended pane: the backend may own an agent this window holds no tab
 *       for, and its answer is still wanted. Answers `receive`'s own result; if
 *       `request` throws or rejects, the fence closes and the error goes on.
 *       `receive` is synchronous: the fence closes as it returns, and an answer
 *       applied after an `await` of its own would be applied outside it.
 *   pending()                      how many questions are still out
 *       Zero when the window is at rest; the leak census reads it, because a
 *       question that never closes is a fence that never does.
 *
 * What is NOT here, on purpose: the screen a shell is drawn on has its own,
 * shorter life (`dropTermScreen` — a card's preview borrows a screen and gives
 * it back while the agent runs on), and a process's own facts end when the
 * process does (`term:exited`). Both are explicit where they happen.
 */
function createPaneLifecycle() {
  const owners = new Map();
  /* One set of ended panes for each question still out. */
  const fences = new Set();

  /* The keys of one collection that belong to `pane`. */
  function keysOf(owner, pane) {
    if (owner.owns === null) return owner.store.has(pane) ? [pane] : [];
    const keys = [];
    for (const key of owner.store.keys()) {
      if (owner.owns(pane, key)) keys.push(key);
    }
    return keys;
  }

  function own(name, store, owns) {
    if (typeof name !== "string" || name === "") throw new Error("pane state needs a name");
    if (owns !== null && typeof owns !== "function") throw new Error(`pane state "${name}": owns must be a function`);
    if (owners.has(name)) throw new Error(`pane state "${name}" is declared twice`);
    owners.set(name, { store, owns });
    return store;
  }

  /* Let `store` hand every entry to `release` as the entry leaves. `delete` and
   * `clear` are replaced on the instance, the way `Map` subclasses would, so a
   * caller that deletes by hand gets what a disposed pane gets. */
  function releasing(store, release) {
    const plainDelete = store.delete.bind(store);
    const holding = store.has.bind(store);
    const entryOf = store.get.bind(store);
    store.delete = (key) => {
      if (!holding(key)) return false;
      const entry = entryOf(key);
      try {
        release(key, entry);
      } finally {
        plainDelete(key);
      }
      return true;
    };
    store.clear = () => {
      let failure = null;
      for (const key of [...store.keys()]) {
        try {
          store.delete(key);
        } catch (error) {
          if (failure === null) failure = { error };
        }
      }
      if (failure !== null) throw failure.error;
    };
  }

  function map(name, options = {}) {
    const store = new Map();
    const owns = options.owns ?? null;
    if (options.release !== undefined) {
      if (typeof options.release !== "function") throw new Error(`pane state "${name}": release must be a function`);
      releasing(store, options.release);
    }
    return own(name, store, owns);
  }

  function set(name, options = {}) {
    if (options.release !== undefined) throw new Error(`pane state "${name}": a Set holds no entry to release; declare a Map`);
    return own(name, new Set(), options.owns ?? null);
  }

  function dispose(pane) {
    // Said first, to every question still out, whether or not the pane held
    // anything: an answer for a pane that ended must not bring it back.
    for (const ended of fences) ended.add(pane);
    const emptied = [];
    let failure = null;
    for (const [name, owner] of owners) {
      const keys = keysOf(owner, pane);
      if (keys.length === 0) continue;
      emptied.push(name);
      for (const key of keys) {
        try {
          owner.store.delete(key);
        } catch (error) {
          if (failure === null) failure = { error };
        }
      }
    }
    if (failure !== null) throw failure.error;
    return emptied;
  }

  function holds(pane) {
    const holding = [];
    for (const [name, owner] of owners) {
      if (keysOf(owner, pane).length > 0) holding.push(name);
    }
    return holding;
  }

  function sizes() {
    const counted = {};
    for (const [name, owner] of owners) counted[name] = owner.store.size;
    return counted;
  }

  async function ask(request, receive) {
    const ended = new Set();
    fences.add(ended);
    try {
      const answer = await request();
      return receive(answer, (pane) => ended.has(pane));
    } finally {
      fences.delete(ended);
    }
  }

  function pending() {
    return fences.size;
  }

  return Object.freeze({ map, set, dispose, holds, sizes, ask, pending });
}

/* The window's one lifecycle. Every script that declares what a pane keeps
 * loads after this file (`ui/index.html`), so the declaration is a top-level
 * statement like the `new Map()` it replaced. */
const paneLifecycle = createPaneLifecycle();

/* The agent state of each shell, and the observation clock beside it.
 *
 * A hook is stamped when this webview observes it, and a retained snapshot from
 * the backend carries the epoch at which the same fact was written. A late
 * snapshot may only replace an older observation, so `observedAt` is the fence
 * and `noteSnapshot` marks a state that came from a snapshot rather than a push
 * (`consumeSnapshot` hears that once, from the push that confirms it). Both ride
 * with the shell: a state deleted by hand takes its observation with it, and a
 * disposed pane leaves neither behind — the fence of a shell that ended must not
 * hold back the first report of the next shell at that id.
 *
 * The three collections are declared like any other, so `holds` names them, and
 * `states.delete`/`states.clear` keep their meaning for a caller that never
 * heard of the lifecycle. */
function paneHookStates(panes = paneLifecycle) {
  const states = panes.map("hookStates");
  const observed = panes.map("hookObservations");
  const fromSnapshot = panes.set("hookSnapshotMarks");
  const deleteState = states.delete.bind(states);
  const clearStates = states.clear.bind(states);
  states.observedAt = (term) => observed.get(term) ?? 0;
  states.observe = (term, at) => observed.set(term, at);
  states.noteSnapshot = (term) => fromSnapshot.add(term);
  states.consumeSnapshot = (term) => fromSnapshot.delete(term);
  states.delete = (term) => {
    observed.delete(term);
    fromSnapshot.delete(term);
    return deleteState(term);
  };
  states.clear = () => {
    observed.clear();
    fromSnapshot.clear();
    clearStates();
  };
  return states;
}
