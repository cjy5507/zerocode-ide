/* Every road a node is made by, counted in the page — one counter for the paints whose cost a shared runner's clock
 * cannot judge (artifact-flow, board-now, terminal-throughput; t-90089).
 *
 * A wall clock on a GitHub-hosted runner measures the runner (machine-load.mjs); what makes a paint dear is the work it
 * does, and that is the same on any machine: a repaint over the same facts that builds rows again, or a painter that
 * empties a row and makes a span a run a frame. Each call counts one: a clone of a kept prototype is one call however
 * many nodes it holds, so a list that builds its rows one node at a time crosses a bound its prototypes keep under.
 *
 * **It crosses into the page as source** (`nodeCounterSource`), as terminal-frames.mjs's fixture does: a page function
 * carries no closure, so the counter holds nothing from outside and is stood up there with `new Function`:
 *
 *   const counter = new Function(`return (${nodeCounterSource})`)()();
 *   paint();
 *   counter.made;      // calls since it stood or since the last reset()
 *   counter.restore(); // the page's own DOM back
 *
 * `counting` decides, at each call, whether it counts (`() => applying.inside` for one hand's calls alone). */
export function nodeCounter(counting = () => true) {
  let made = 0;
  const undo = [];
  const note = () => {
    if (counting()) made += 1;
  };
  const wrap = (owner, key) => {
    const original = Object.getOwnPropertyDescriptor(owner, key);
    if (!original) return;
    undo.push(() => Object.defineProperty(owner, key, original));
    if (typeof original.value === "function") {
      Object.defineProperty(owner, key, {
        ...original,
        value: function count(...args) { note(); return original.value.apply(this, args); },
      });
    } else if (original.set) {
      Object.defineProperty(owner, key, {
        ...original,
        set: function count(value) { note(); original.set.call(this, value); },
      });
    }
  };
  for (const key of ["createElement", "createElementNS", "createDocumentFragment", "importNode"]) {
    wrap(Document.prototype, key);
  }
  wrap(Node.prototype, "cloneNode");
  wrap(Element.prototype, "insertAdjacentHTML");
  wrap(Element.prototype, "innerHTML");
  return {
    get made() { return made; },
    reset() { made = 0; },
    restore() {
      for (const put of undo.reverse()) put();
      undo.length = 0;
    },
  };
}

export const nodeCounterSource = nodeCounter.toString();
