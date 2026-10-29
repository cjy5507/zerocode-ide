import { openWindowTestPage } from "./window-boot.mjs";

/* ZeroCode's own window as a Computer Use target (t-73906): the page's half.
 *
 * The backend asks the page (`computer:self-act`) for a look, for one
 * control's facts, or to press one control, and the page answers by id
 * (`computer_self_answer`). Judging is the backend's (core `judge`); what this
 * suite holds is the page's part of the person's words — an agent works in
 * the window while the person keeps their pointer, focus and keys:
 *
 *  - a look lists the visible controls with the marks the window already
 *    draws: the question popup, a secret field and the dialog around it, a
 *    terminal, a person-only container, a settings pane, the person's field;
 *  - a secret field's value never leaves the page;
 *  - a press clicks or fills the control in the document — the person's
 *    focus and the text in their field stay as they were;
 *  - a press whose control changed since its facts were read presses
 *    nothing, so facts the backend judged are the facts pressed.
 *
 * The controls it presses are fixtures the suite puts on the page, so a
 * press tests the page's own road and not the handler of a real button.
 *
 *   WINDOW_SUITES=self-act node ui/tests/window.mjs */

const installHands = () => {
  const T = {
    answers: new Map(),
    next: 100,
    fire(name, payload) {
      for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
    },
    /* One request to the page, the way the backend sends it; the answer the
     * page gave by id, or null when none came. */
    async ask(request, waitMs = 1500) {
      const id = ++T.next;
      T.fire("computer:self-act", { id, request });
      const until = Date.now() + waitMs;
      while (!T.answers.has(id) && Date.now() < until) {
        await new Promise((done) => setTimeout(done, 15));
      }
      return T.answers.get(id) ?? null;
    },
    /* A look's control by the name it showed. */
    named(look, name) {
      return look?.ok?.controls?.find((control) => control.name === name) ?? null;
    },
    clicks: [],
    inputs: [],
  };
  window.__ANSWER__.computer_self_answer = ({ id, answer }) => {
    T.answers.set(id, JSON.parse(JSON.stringify(answer)));
    return true;
  };
  const fixture = document.createElement("div");
  fixture.id = "self-act-fixture";
  fixture.style.cssText = "position:fixed;left:8px;bottom:8px;z-index:1;background:#000;padding:4px";
  fixture.innerHTML = `
    <button id="probe-button" type="button">Probe button</button>
    <input id="probe-field" type="text" aria-label="Probe field" value="">
    <textarea id="person-field" aria-label="Person field"></textarea>
    <div role="dialog" aria-label="Probe dialog">
      <input id="probe-secret" type="password" aria-label="Probe secret" value="SENTINEL-not-a-secret-51">
      <button type="button">Probe save</button>
    </div>
    <section class="terminal"><button type="button">Probe terminal</button></section>
    <div data-person-only><button type="button">Probe person only</button></div>
    <section class="settings-pane"><label><input type="checkbox" aria-label="Probe setting"></label></section>`;
  document.body.appendChild(fixture);
  document.getElementById("probe-button").addEventListener("click", (event) => T.clicks.push(event.isTrusted));
  document.getElementById("probe-field").addEventListener("input", () => T.inputs.push(document.getElementById("probe-field").value));
  window.__SELF_T__ = T;
};

async function scenario(browser, origin, name, ok, body) {
  const { page } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(installHands);
    await body(page);
  } catch (error) {
    ok(`the ${name} scenario ran to its end`, false, String(error?.stack ?? error).split("\n").slice(0, 3).join(" | "));
  } finally {
    await page.close();
  }
}

export async function testSelfAct(browser, origin, ok) {
  await scenario(browser, origin, "ordinary", ok, async (page) => {
    // The person is in their own field, typing.
    await page.focus("#person-field");
    await page.keyboard.type("draft");
    const said = await page.evaluate(async () => {
      const T = window.__SELF_T__;
      const look = await T.ask({ kind: "look" });
      const button = T.named(look, "Probe button");
      const field = T.named(look, "Probe field");
      const person = T.named(look, "Person field");
      const facts = button && await T.ask({ kind: "facts", lookId: look.ok.lookId, id: button.id });
      const pressed = button && await T.ask({ kind: "do", lookId: look.ok.lookId, id: button.id, act: "click", facts: facts?.ok?.facts });
      const fieldFacts = field && await T.ask({ kind: "facts", lookId: look.ok.lookId, id: field.id });
      const filled = field && await T.ask({ kind: "do", lookId: look.ok.lookId, id: field.id, act: "setValue", value: "main.rs", facts: fieldFacts?.ok?.facts });
      return {
        answered: look !== null,
        lookId: look?.ok?.lookId ?? null,
        button: button && { role: button.role, facts: button.facts },
        field: field && { role: field.role },
        person: person && { facts: person.facts },
        factsRole: facts?.ok?.role ?? null,
        pressed: pressed?.ok?.done ?? null,
        filled: filled?.ok?.done ?? null,
        clicks: T.clicks,
        inputs: T.inputs,
        fieldValue: document.getElementById("probe-field").value,
        focus: document.activeElement?.id ?? null,
        personValue: document.getElementById("person-field").value,
      };
    });
    ok("the page answers a look by id", said.answered && said.lookId !== null, JSON.stringify(said));
    ok("a plain button reads as a button with no mark", said.button?.role === "AXButton"
      && Object.values(said.button?.facts ?? { x: true }).every((value) => value === false || value === null), JSON.stringify(said.button));
    ok("a plain field reads as a text field", said.field?.role === "AXTextField", JSON.stringify(said.field));
    ok("the facts of a control name its role again", said.factsRole === "AXButton", String(said.factsRole));
    ok("the person's field reads as theirs: focused, typed a moment ago",
      said.person?.facts?.personFocus === true && Number.isFinite(said.person?.facts?.typedMsAgo) && said.person.facts.typedMsAgo < 2000,
      JSON.stringify(said.person));
    ok("the button is clicked once, by the page", said.pressed === true && said.clicks.length === 1, JSON.stringify(said.clicks));
    ok("the field is filled and hears its input", said.filled === true && said.fieldValue === "main.rs" && said.inputs.at(-1) === "main.rs",
      JSON.stringify([said.fieldValue, said.inputs]));
    ok("the person's focus stays in their field", said.focus === "person-field", String(said.focus));
    ok("the person's text stays as they typed it", said.personValue === "draft", said.personValue);
  });

  await scenario(browser, origin, "marks", ok, async (page) => {
    const said = await page.evaluate(async () => {
      const T = window.__SELF_T__;
      const look = await T.ask({ kind: "look" });
      const facts = (name) => T.named(look, name)?.facts ?? null;
      return {
        secret: facts("Probe secret"),
        save: facts("Probe save"),
        terminal: facts("Probe terminal"),
        personOnly: facts("Probe person only"),
        setting: facts("Probe setting"),
        leaked: JSON.stringify(look).includes("SENTINEL-not-a-secret-51"),
      };
    });
    ok("a password field is secret", said.secret?.secret === true, JSON.stringify(said.secret));
    ok("a button beside a secret field says so", said.save?.besideSecret === true && said.save?.secret === false, JSON.stringify(said.save));
    ok("a control in a terminal says so", said.terminal?.terminal === true, JSON.stringify(said.terminal));
    ok("a control in a person-only container says so", said.personOnly?.personOnly === true, JSON.stringify(said.personOnly));
    ok("a control in a settings pane says so", said.setting?.setting === true, JSON.stringify(said.setting));
    ok("a secret field's value never leaves the page", said.leaked === false);
  });

  await scenario(browser, origin, "values", ok, async (page) => {
    // The person's words in the window (t-74975): a draft they typed, a field
    // the window filled for them, a settings address, the name beside a
    // secret, a draft in an editable block. A look carries their length only.
    await page.evaluate(() => {
      const more = document.createElement("div");
      more.id = "self-act-values";
      more.style.cssText = "position:fixed;left:8px;top:8px;z-index:1;background:#000;padding:4px";
      more.innerHTML = `
        <input id="kept-field" type="text" aria-label="Kept field" value="SENTINEL-kept-text-62">
        <section class="settings-pane"><input id="setting-address" type="email" aria-label="Setting address" value="SENTINEL-address-63"></section>
        <div role="dialog" aria-label="Login dialog">
          <input id="login-name" type="text" aria-label="Login name" value="SENTINEL-user-64">
          <input type="password" aria-label="Login secret" value="">
        </div>
        <div id="draft-block" contenteditable="true" aria-label="Draft block">SENTINEL-block-65</div>
        <div role="textbox" contenteditable="true">SENTINEL-unnamed-67</div>`;
      document.body.appendChild(more);
    });
    await page.focus("#person-field");
    await page.keyboard.type("SENTINEL-draft-61");
    await page.focus("#probe-button");
    const said = await page.evaluate(async () => {
      const T = window.__SELF_T__;
      const first = await T.ask({ kind: "look" });
      const field = T.named(first, "Probe field");
      const read = await T.ask({ kind: "facts", lookId: first.ok.lookId, id: field.id });
      const filled = await T.ask({ kind: "do", lookId: first.ok.lookId, id: field.id, act: "setValue", value: "main.rs", facts: read.ok.facts });
      const look = await T.ask({ kind: "look" });
      const shown = (name) => {
        const control = T.named(look, name);
        return control && { value: control.value, withheld: control.withheld ?? null, name: control.name };
      };
      return {
        filled: filled?.ok?.done ?? null,
        leaked: (JSON.stringify(first) + JSON.stringify(look)).match(/SENTINEL-[a-z-]+-\d+/g) ?? [],
        draft: shown("Person field"),
        kept: shown("Kept field"),
        address: shown("Setting address"),
        user: shown("Login name"),
        block: shown("Draft block"),
        agents: shown("Probe field"),
      };
    });
    ok("a look carries none of the person's words", said.leaked.length === 0, JSON.stringify(said.leaked));
    ok("the person's draft reads as its length", said.draft?.value === null && said.draft?.withheld === "SENTINEL-draft-61".length, JSON.stringify(said.draft));
    ok("a field's text the window put there reads as its length", said.kept?.value === null && said.kept?.withheld === "SENTINEL-kept-text-62".length, JSON.stringify(said.kept));
    ok("a settings address reads as its length", said.address?.value === null && said.address?.withheld === "SENTINEL-address-63".length, JSON.stringify(said.address));
    ok("a name beside a secret field reads as its length", said.user?.value === null && said.user?.withheld === "SENTINEL-user-64".length, JSON.stringify(said.user));
    ok("an editable block's draft is neither its name nor its value", said.block?.name === "Draft block" && said.block?.value === null
      && said.block?.withheld === "SENTINEL-block-65".length, JSON.stringify(said.block));
    ok("the text an agent's own fill put there still reads", said.filled === true && said.agents?.value === "main.rs" && said.agents?.withheld === null,
      JSON.stringify(said.agents));
  });

  await scenario(browser, origin, "private words", ok, async (page) => {
    // A browser pane's fill strip (t-79331): its buttons show the person a
    // saved address and a past entry — the window's own painter draws them
    // over synthetic ones. A look names each button by its place, never by
    // the name, the address or the value it shows; a control in a
    // private-words mark with no plain name is named nothing.
    const said = await page.evaluate(async () => {
      const T = window.__SELF_T__;
      const bar = document.createElement("div");
      bar.className = "browser-fillbar";
      bar.dataset.personOnly = "";
      bar.style.cssText = "position:fixed;left:8px;top:8px;z-index:2";
      document.body.appendChild(bar);
      paintBrowserFillBar(bar, {
        fillOffer: newFillOffer("https://shop.example.com/order", {
          addresses: [{ id: "a", label: "SENTINEL-name-71 · SENTINEL-street-72", kinds: 6 }],
          entries: [{ name: "memo", label: "SENTINEL-entry-73" }],
        }),
      });
      const marked = document.createElement("div");
      marked.dataset.privateWords = "";
      marked.style.cssText = "position:fixed;left:8px;top:48px;z-index:2";
      marked.innerHTML = `<button type="button" title="SENTINEL-tip-74">SENTINEL-words-75</button>`;
      document.body.appendChild(marked);
      const look = await T.ask({ kind: "look" });
      return {
        shown: bar.textContent,
        named: bar.querySelectorAll("[data-private-words]").length,
        leaked: JSON.stringify(look).match(/SENTINEL-[a-z-]+-\d+/g) ?? [],
        picks: (look?.ok?.controls ?? []).filter((control) => /1/.test(control.name) && control.facts?.personOnly).map((control) => control.name),
      };
    });
    ok("a look names the fill strip's saved address by its place, never by the person's name or address",
      said.shown.includes("SENTINEL-name-71") && said.shown.includes("SENTINEL-entry-73") && said.named === 2
        && said.leaked.length === 0 && said.picks.length === 2,
      JSON.stringify(said));
  });

  await scenario(browser, origin, "values-after-the-person", ok, async (page) => {
    // An agent filled the field, and then the person typed in it: the text is
    // theirs again.
    const asked = await page.evaluate(async () => {
      const T = window.__SELF_T__;
      const look = await T.ask({ kind: "look" });
      const field = T.named(look, "Probe field");
      const read = await T.ask({ kind: "facts", lookId: look.ok.lookId, id: field.id });
      const filled = await T.ask({ kind: "do", lookId: look.ok.lookId, id: field.id, act: "setValue", value: "main.rs", facts: read.ok.facts });
      return filled?.ok?.done ?? null;
    });
    await page.focus("#probe-field");
    await page.keyboard.type("-SENTINEL-66");
    await page.focus("#probe-button");
    const said = await page.evaluate(async () => {
      const T = window.__SELF_T__;
      const look = await T.ask({ kind: "look" });
      const field = T.named(look, "Probe field");
      return { value: field?.value ?? null, withheld: field?.withheld ?? null, text: document.getElementById("probe-field").value };
    });
    ok("a field the person typed in after an agent's fill reads as its length", asked === true && said.value === null
      && said.withheld === said.text.length && said.text.endsWith("-SENTINEL-66"), JSON.stringify(said));
  });

  await scenario(browser, origin, "asking", ok, async (page) => {
    const said = await page.evaluate(async () => {
      const T = window.__SELF_T__;
      T.fire("computer:confirm", { id: "confirm-1", kind: "payment", label: "Place order", verb: "click", timeoutMs: 120000 });
      await new Promise((done) => setTimeout(done, 120));
      let answered = 0;
      window.__ANSWER__.computer_confirm_answer = () => { answered += 1; return true; };
      const look = await T.ask({ kind: "look" });
      const inPopup = (look?.ok?.controls ?? []).filter((control) => control.facts?.asking === true);
      const first = inPopup[0];
      // Facts that do not match the control — a backend that judged other
      // facts than the page's — press nothing.
      // The facts read first, as the backend reads them; then a press on
      // other facts than those.
      const read = first && await T.ask({ kind: "facts", lookId: look.ok.lookId, id: first.id });
      const forged = first && await T.ask({
        kind: "do", lookId: look.ok.lookId, id: first.id, act: "click",
        facts: { ...read.ok.facts, asking: false },
      });
      const scrim = document.getElementById("ask-scrim");
      return {
        inPopup: inPopup.length,
        forged: forged?.ok?.done ?? null,
        open: !scrim.hidden,
        answered,
      };
    });
    ok("the popup's buttons read as asking", said.inPopup >= 2, String(said.inPopup));
    ok("a press on facts the page does not hold presses nothing", said.forged === false, String(said.forged));
    ok("the question stays open and unanswered", said.open && said.answered === 0, JSON.stringify(said));
  });

  await scenario(browser, origin, "stale", ok, async (page) => {
    const said = await page.evaluate(async () => {
      const T = window.__SELF_T__;
      const look = await T.ask({ kind: "look" });
      const button = T.named(look, "Probe button");
      const facts = await T.ask({ kind: "facts", lookId: look.ok.lookId, id: button.id });
      document.getElementById("probe-button").remove();
      const gone = await T.ask({ kind: "facts", lookId: look.ok.lookId, id: button.id });
      const pressed = await T.ask({ kind: "do", lookId: look.ok.lookId, id: button.id, act: "click", facts: facts.ok.facts });
      const old = await T.ask({ kind: "do", lookId: -1, id: 0, act: "click", facts: {} });
      return { gone: gone?.ok?.gone ?? null, pressed: pressed?.ok?.done ?? null, old: old?.ok?.done ?? null, clicks: T.clicks.length };
    });
    ok("a control gone from the document reads as gone", said.gone === true, JSON.stringify(said));
    ok("a press on a control gone presses nothing", said.pressed === false && said.clicks === 0, JSON.stringify(said));
    ok("a press from a look the page no longer holds presses nothing", said.old === false, JSON.stringify(said));
  });

  /* Whose hand made a command (t-74927). The window refuses a person-only
   * command by the hand the page says made it, never by a clock: every call
   * carries `zerocode-hand`. What an agent's press makes — in its click, after
   * an await, after a wait whose end nobody times, in a change the press
   * fired — says `agent`; the person's own click right after it says
   * `person`; the update clock's quiet download says `program`. */
  await scenario(browser, origin, "hand", ok, async (page) => {
    await page.evaluate(() => {
      const T = window.__SELF_T__;
      T.hands = [];
      const said = (command) => (args, options) => {
        T.hands.push([command, options?.headers?.["zerocode-hand"] ?? null]);
        return command === "update_check" ? { phase: { phase: "available" }, next: "download" }
          : command === "update_download" ? { phase: { phase: "idle" } }
            : true;
      };
      for (const command of ["computer_take_control", "computer_give_back", "answer_ask", "answer_approval",
        "jira_connect", "update_check", "update_download", "board_columns_probe"]) {
        window.__ANSWER__[command] = said(command);
      }
      let release;
      T.later = new Promise((done) => { release = done; });
      T.release = release;
      const fixture = document.getElementById("self-act-fixture");
      fixture.insertAdjacentHTML("beforeend", `
        <button id="hand-agent" type="button">Hand agent</button>
        <input id="hand-check" type="checkbox" aria-label="Hand check">
        <button id="hand-person" type="button">Hand person</button>`);
      // A road that awaits carries the hand it started with (t-75209).
      document.getElementById("hand-agent").addEventListener("click", async () => {
        const started = handNow();
        void invoke("computer_take_control");
        await invoke("board_columns_probe");
        void invoke("answer_ask", undefined, { hand: flowHand(started) });
        await T.later;
        void invoke("computer_give_back", undefined, { hand: flowHand(started) });
      });
      document.getElementById("hand-check").addEventListener("change", () => void invoke("jira_connect"));
      document.getElementById("hand-person").addEventListener("click", () => void invoke("answer_approval"));
    });
    const agent = await page.evaluate(async () => {
      const T = window.__SELF_T__;
      const press = async (name) => {
        const look = await T.ask({ kind: "look" });
        const control = T.named(look, name);
        const facts = await T.ask({ kind: "facts", lookId: look.ok.lookId, id: control.id });
        return (await T.ask({ kind: "do", lookId: look.ok.lookId, id: control.id, act: "click", facts: facts.ok.facts }))?.ok?.done ?? null;
      };
      const pressed = [await press("Hand agent"), await press("Hand check")];
      // The press's last step waits on something only the test ends: no clock
      // decides whose hand it was when it ends.
      T.release();
      await T.later;
      await new Promise((done) => setTimeout(done, 0));
      return { pressed, hands: T.hands.splice(0) };
    });
    await page.click("#hand-person");
    const person = await page.evaluate(async () => {
      const T = window.__SELF_T__;
      await askUpdateCheck(UPDATE_KNOCK.clock);
      return T.hands.splice(0);
    });
    const hand = (hands, command) => hands.filter(([name]) => name === command).map(([, by]) => by);
    ok("the agent's presses were made", agent.pressed.every((done) => done === true), JSON.stringify(agent.pressed));
    ok("a command the press's click makes says agent", hand(agent.hands, "computer_take_control").join() === "agent", JSON.stringify(agent.hands));
    ok("a command after the press's await says agent", hand(agent.hands, "answer_ask").join() === "agent", JSON.stringify(agent.hands));
    ok("a command after a wait nobody timed says agent", hand(agent.hands, "computer_give_back").join() === "agent", JSON.stringify(agent.hands));
    ok("a command a change the press fired makes says agent", hand(agent.hands, "jira_connect").join() === "agent", JSON.stringify(agent.hands));
    ok("the person's own click right after says person", hand(person, "answer_approval").join() === "person", JSON.stringify(person));
    ok("the update clock's quiet download says program", hand(person, "update_download").join() === "program", JSON.stringify(person));
  });

  /* Whose hand made a command is the flow's, not the page's last input
   * (t-75209, R3 of t-74927). An agent's press starts a flow; while that flow
   * awaits, the person clicks something else and presses a key; the flow's
   * person-only command still does not say `person` — whether it carries the
   * hand its press started with (a real road: the restart door's census and
   * its `leave_now`) or carries none (a handler that awaits and then calls).
   * A question the agent's flow opened stays the agent's when the person
   * answers it. The person's own roads keep their hand: the band's
   * 「제어권 가져오기」 right after an agent's press, the restart door they
   * press themselves through its census, and the window close's question
   * they answer. */
  await scenario(browser, origin, "hand per flow", ok, async (page) => {
    await page.evaluate(() => {
      const T = window.__SELF_T__;
      T.hands = [];
      const census = [];
      T.census = census;
      const said = (command) => (args, options) => {
        T.hands.push([command, options?.headers?.["zerocode-hand"] ?? null]);
        if (command === "busy_census") return new Promise((done) => census.push(done));
        return command === "leave_now" || command === "leave_when_idle" ? null : true;
      };
      for (const command of ["computer_take_control", "computer_give_back", "board_columns_probe",
        "busy_census", "leave_now", "leave_when_idle", "desk_ack"]) {
        window.__ANSWER__[command] = said(command);
      }
      let release;
      T.later = new Promise((done) => { release = done; });
      T.release = release;
      const fixture = document.getElementById("self-act-fixture");
      fixture.insertAdjacentHTML("beforeend", `
        <button id="flow-agent" type="button">Flow agent</button>
        <button id="flow-restart" type="button">Flow restart</button>
        <button id="flow-elsewhere" type="button">Flow elsewhere</button>
        <input id="flow-enter" type="text" aria-label="Flow enter">
        <button id="flow-enter-press" type="button">Flow enter press</button>`);
      // Enter in a field presses its button by the page's own click(), the way
      // the hand-over code's field does: still the person's key.
      document.getElementById("flow-enter").addEventListener("keydown", (event) => {
        if (event.key === "Enter") document.getElementById("flow-enter-press").click();
      });
      document.getElementById("flow-enter-press").addEventListener("click", () => {
        void invoke("computer_give_back", undefined, { hand: handNow() });
      });
      document.getElementById("flow-agent").addEventListener("click", async () => {
        await invoke("board_columns_probe");
        await T.later;
        void invoke("computer_give_back");
      });
      document.getElementById("flow-restart").addEventListener("click", () => void askBeforeRestart("probe-door"));
      // The band, standing: a window shared with an agent.
      T.fire("computer:share-state", { target: "probe", label: "Probe", takenBack: false });
    });
    const press = (name) => page.evaluate(async (name) => {
      const T = window.__SELF_T__;
      const look = await T.ask({ kind: "look" });
      const control = T.named(look, name);
      const facts = await T.ask({ kind: "facts", lookId: look.ok.lookId, id: control.id });
      return (await T.ask({ kind: "do", lookId: look.ok.lookId, id: control.id, act: "click", facts: facts.ok.facts }))?.ok?.done ?? null;
    }, name);
    const census = (busy) => page.evaluate((busy) => {
      const T = window.__SELF_T__;
      const done = T.census.shift();
      done?.({ road: "restart", door: "probe-door", busy: { busy }, waitMin: null });
      return done !== undefined;
    }, busy);
    const settle = () => page.evaluate(() => new Promise((done) => setTimeout(done, 0)));
    const take = () => page.evaluate(() => window.__SELF_T__.hands.splice(0));
    const elsewhere = async () => {
      await page.click("#flow-elsewhere");
      await page.keyboard.press("Shift");
    };
    const hand = (hands, command) => hands.filter(([name]) => name === command).map(([, by]) => by);

    // The agent's handler awaits; the person moves elsewhere before it ends.
    const pressedFlow = await press("Flow agent");
    await elsewhere();
    await page.evaluate(() => window.__SELF_T__.release());
    await settle();
    const flow = await take();

    // The agent presses the restart door; its census is out when the person
    // moves elsewhere; the census comes back with nothing busy.
    const pressedRestart = await press("Flow restart");
    await elsewhere();
    const agentCensus = await census(false);
    await settle();
    const restart = await take();

    // The agent presses the restart door again; the census says busy and the
    // question opens; the person answers it 「지금 다시 시작」.
    await press("Flow restart");
    await census(true);
    await page.waitForSelector("#ask-no", { state: "visible" });
    await page.click("#ask-no");
    await settle();
    const answered = await take();

    // The person's own hand: 「제어권 가져오기」 right after an agent's press,
    // and the restart door through its census.
    await press("Flow agent");
    await page.click("#computer-band-take");
    const band = await take();
    await page.click("#flow-restart");
    await census(false);
    await settle();
    const own = await take();
    // The window's close asks (a backend event, no page hand), the person
    // answers 「지금 종료」.
    await page.evaluate(() => {
      for (const handler of window.__LISTENERS__["exit:ask"] ?? []) {
        handler({ payload: { road: "close", door: null, busy: { busy: true }, waitMin: null } });
      }
    });
    await page.waitForSelector("#ask-no", { state: "visible" });
    await page.click("#ask-no");
    await settle();
    const closed = await take();
    // The person's Enter right after an agent's press, through the page's own
    // click(); and the same button pressed by the agent.
    await press("Flow elsewhere");
    await page.focus("#flow-enter");
    await page.keyboard.press("Enter");
    const entered = await take();
    await press("Flow enter press");
    const agentPressed = await take();

    ok("the agent's flow presses were made", pressedFlow === true && pressedRestart === true && agentCensus === true,
      JSON.stringify({ pressedFlow, pressedRestart, agentCensus }));
    ok("a flow's command after the person moved elsewhere names no hand", hand(flow, "computer_give_back").join() === "none", JSON.stringify(flow));
    ok("the restart door the agent pressed stays the agent's though the person moved", hand(restart, "leave_now").join() === "agent", JSON.stringify(restart));
    ok("a question the agent's flow opened stays the agent's when the person answers", hand(answered, "leave_now").join() === "agent", JSON.stringify(answered));
    ok("the person's 「제어권 가져오기」 right after an agent's press says person", hand(band, "computer_take_control").join() === "person", JSON.stringify(band));
    ok("the person's own restart door says person through its census", hand(own, "leave_now").join() === "person", JSON.stringify(own));
    ok("the person's answer to the window's close says person", hand(closed, "leave_now").join() === "person", JSON.stringify(closed));
    ok("the person's Enter that presses a button by click() says person", hand(entered, "computer_give_back").join() === "person", JSON.stringify(entered));
    ok("the same button the agent pressed says agent", hand(agentPressed, "computer_give_back").join() === "agent", JSON.stringify(agentPressed));
  });

  /* The roads t-74975 made the person's — a push, a discard, a checkout's
   * removal — await before their person-only command: Sync fetches and pulls
   * first, the staged discard unstages and reads git's status, the cleanup
   * asks, then re-measures the row. Each carries the hand its press started
   * with (t-75153, the hand rule of t-75209): the person's press reaches the
   * window as `person`, and the same road an agent's press started says
   * `agent`, which the second wall refuses. */
  await scenario(browser, origin, "person-only roads that await", ok, async (page) => {
    await page.evaluate(() => {
      const T = window.__SELF_T__;
      T.hands = [];
      const row = {
        id: "probe-row", path: "/work/probe-checkout", name: "probe-checkout", force: false,
        git: { dirty_files: 0 }, blockers: [], reasons: [],
      };
      const said = (command, answer = null) => (args, options) => {
        T.hands.push([command, options?.headers?.["zerocode-hand"] ?? null]);
        return answer;
      };
      Object.assign(window.__ANSWER__, {
        scm_fetch: () => null,
        scm_pull: () => null,
        upstream_status: () => ({ upstream: "origin/probe", ahead: 1, behind: 0 }),
        unstage_paths: () => null,
        scm_status: () => ({ changed: [{ path: "loose.txt", code: "??" }, { path: "kept.txt", code: " M" }] }),
        workspace_cleanup_scan: ({ only }) => ({ rows: only ? [row] : [] }),
        scm_push: said("scm_push"),
        delete_untracked: said("delete_untracked"),
        discard_paths: said("discard_paths"),
        remove_worktree: said("remove_worktree"),
      });
      const fixture = document.getElementById("self-act-fixture");
      fixture.insertAdjacentHTML("beforeend", `
        <button id="road-sync" type="button">Road sync</button>
        <button id="road-discard" type="button">Road discard</button>
        <button id="road-cleanup" type="button">Road cleanup</button>`);
      document.getElementById("road-sync").addEventListener("click", () => void runScmRemote("sync"));
      // The discard dialog's own Delete button, pressed in the same event.
      document.getElementById("road-discard").addEventListener("click", () => {
        openScmDiscard({ kind: "area", area: "staged", paths: ["loose.txt", "kept.txt"] });
        el("scm-discard-go").click();
      });
      document.getElementById("road-cleanup").addEventListener("click", () => void removeCleanupRows([row]));
    });
    const press = (name) => page.evaluate(async (name) => {
      const T = window.__SELF_T__;
      const look = await T.ask({ kind: "look" });
      const control = T.named(look, name);
      const facts = await T.ask({ kind: "facts", lookId: look.ok.lookId, id: control.id });
      return (await T.ask({ kind: "do", lookId: look.ok.lookId, id: control.id, act: "click", facts: facts.ok.facts }))?.ok?.done ?? null;
    }, name);
    // Every road ends in its person-only command; the cleanup's question is
    // answered by the page (a yes no hand made), so only the press decides.
    const reached = (command) => page.waitForFunction(
      (command) => window.__SELF_T__.hands.some(([name]) => name === command), command, { timeout: 5000 });
    const answerCleanup = async () => {
      await page.waitForFunction(() => wsclean.confirming !== null, null, { timeout: 5000 });
      await page.evaluate(() => closeCleanupConfirm(true));
    };
    const take = () => page.evaluate(() => window.__SELF_T__.hands.splice(0));
    const hand = (hands, command) => hands.filter(([name]) => name === command).map(([, by]) => by);

    await page.click("#road-sync");
    await reached("scm_push");
    const personSync = await take();
    await page.click("#road-discard");
    await reached("discard_paths");
    const personDiscard = await take();
    await page.click("#road-cleanup");
    await answerCleanup();
    await reached("remove_worktree");
    const personCleanup = await take();

    const pressedSync = await press("Road sync");
    await reached("scm_push");
    const agentSync = await take();
    const pressedDiscard = await press("Road discard");
    await reached("discard_paths");
    const agentDiscard = await take();
    const pressedCleanup = await press("Road cleanup");
    await answerCleanup();
    await reached("remove_worktree");
    const agentCleanup = await take();

    ok("the agent's presses on the three roads were made", pressedSync === true && pressedDiscard === true && pressedCleanup === true,
      JSON.stringify({ pressedSync, pressedDiscard, pressedCleanup }));
    ok("the person's Sync pushes as the person after its fetch and pull", hand(personSync, "scm_push").join() === "person", JSON.stringify(personSync));
    ok("the person's staged discard deletes and restores as the person after the unstage",
      hand(personDiscard, "delete_untracked").join() === "person" && hand(personDiscard, "discard_paths").join() === "person",
      JSON.stringify(personDiscard));
    ok("the person's cleanup removes as the person after its question and re-measure",
      hand(personCleanup, "remove_worktree").join() === "person", JSON.stringify(personCleanup));
    ok("a Sync an agent pressed pushes as the agent", hand(agentSync, "scm_push").join() === "agent", JSON.stringify(agentSync));
    ok("a discard an agent pressed deletes and restores as the agent",
      hand(agentDiscard, "delete_untracked").join() === "agent" && hand(agentDiscard, "discard_paths").join() === "agent",
      JSON.stringify(agentDiscard));
    ok("a cleanup an agent pressed removes as the agent", hand(agentCleanup, "remove_worktree").join() === "agent", JSON.stringify(agentCleanup));
  });

  /* A pre-canonical build kept the person's "don't ask again" for a running
   * terminal in this webview only. A boot carries no hand, so it writes
   * nothing of it — no person-only call, no error — and the window still asks
   * (t-75153). The person's own "don't ask again" saves it as the person and
   * takes the old key away. */
  const LEGACY_KEY = "zerocode.confirm-close-running.v1";
  const SKIP = "set_skip_close_terminal_with_running_process_confirm";
  const { page: legacy } = await openWindowTestPage(browser, origin, {
    before: (page) => page.addInitScript(({ key, skip }) => {
      localStorage.setItem(key, "off");
      window.__LEGACY_HANDS__ = [];
      window.__ANSWER__[skip] = (args, options) => {
        window.__LEGACY_HANDS__.push(options?.headers?.["zerocode-hand"] ?? null);
        return { skip_close_terminal_with_running_process_confirm: args?.skip === true };
      };
    }, { key: LEGACY_KEY, skip: SKIP }),
  });
  try {
    const boot = await legacy.evaluate((key) => ({
      calls: [...window.__LEGACY_HANDS__],
      asks: confirmCloseRunning,
      kept: localStorage.getItem(key),
      error: stageError ?? null,
    }), LEGACY_KEY);
    ok("a boot with the old key makes no person-only call and says no error", boot.calls.length === 0 && !boot.error, JSON.stringify(boot));
    ok("a boot with the old key still asks before closing a running terminal", boot.asks === true, JSON.stringify(boot));
    await legacy.evaluate(() => {
      const button = document.createElement("button");
      button.id = "legacy-ask";
      button.type = "button";
      button.textContent = "Legacy ask";
      button.style.cssText = "position:fixed;left:8px;bottom:8px;z-index:1";
      document.body.append(button);
      button.addEventListener("click", () => void askAboutRunning({ agent: null }, [], handNow()));
    });
    await legacy.click("#legacy-ask");
    await legacy.waitForSelector(".ask-remember input", { state: "visible" });
    await legacy.click(".ask-remember input");
    await legacy.click("#ask-yes");
    await legacy.waitForFunction((key) => localStorage.getItem(key) === null, LEGACY_KEY, { timeout: 5000 });
    const remembered = await legacy.evaluate(() => ({ calls: [...window.__LEGACY_HANDS__], asks: confirmCloseRunning }));
    ok("the person's \"don't ask again\" saves it once, as the person, and the old key goes",
      remembered.calls.join() === "person" && remembered.asks === false, JSON.stringify(remembered));
  } catch (error) {
    ok("the old-key scenario ran to its end", false, String(error?.stack ?? error).split("\n").slice(0, 3).join(" | "));
  } finally {
    await legacy.close();
  }

  /* Whose hand a trusted event is (t-75431, R5 of t-75209). The browser marks
   * trusted what the page's own focus() or click() sets off as well — a blur,
   * a checkbox's change — so only the person's own moves (a pointer, a touch,
   * a key, the text they type) are the person's by themselves. A checkbox's
   * change is the hand of the click that flipped it; the OpenCode cookie box
   * saves on blur as the hand that wrote its text. An agent's flow that moves
   * the focus after an await saves nothing as the person's, nor does the
   * person leaving a box an agent filled, nor a page's click() on a checkbox
   * after an await; the person leaving the box they typed in, pressing Enter
   * in it, and clicking a checkbox or its label still say person. */
  await scenario(browser, origin, "hand by event kind", ok, async (page) => {
    await page.evaluate(() => {
      const T = window.__SELF_T__;
      T.hands = [];
      const said = (command) => (args, options) => {
        T.hands.push([command, options?.headers?.["zerocode-hand"] ?? null]);
        return command === "set_opencode_cookie" ? { opencode_cookie_configured: true } : true;
      };
      for (const command of ["jira_connect", "set_opencode_cookie", "board_columns_probe"]) {
        window.__ANSWER__[command] = said(command);
      }
      T.gates = [];
      T.gate = () => new Promise((done) => T.gates.push(done));
      const fixture = document.getElementById("self-act-fixture");
      fixture.insertAdjacentHTML("beforeend", `
        <label><input id="kind-check" type="checkbox" aria-label="Kind check"><span id="kind-check-words">Kind check words</span></label>
        <button id="kind-flip" type="button">Kind flip</button>
        <button id="kind-move" type="button">Kind move</button>`);
      document.getElementById("kind-check").addEventListener("change", () => void invoke("jira_connect"));
      // An agent's flow that, after an await, clicks a checkbox by the page's
      // own click() — and one that moves the focus.
      document.getElementById("kind-flip").addEventListener("click", async () => {
        await T.gate();
        document.getElementById("kind-check").click();
      });
      document.getElementById("kind-move").addEventListener("click", async () => {
        await T.gate();
        document.getElementById("opencode-workspace").focus();
      });
    });
    const press = (name) => page.evaluate(async (name) => {
      const T = window.__SELF_T__;
      const look = await T.ask({ kind: "look" });
      const control = T.named(look, name);
      const facts = await T.ask({ kind: "facts", lookId: look.ok.lookId, id: control.id });
      return (await T.ask({ kind: "do", lookId: look.ok.lookId, id: control.id, act: "click", facts: facts.ok.facts }))?.ok?.done ?? null;
    }, name);
    const release = () => page.evaluate(() => new Promise((done) => {
      for (const open of window.__SELF_T__.gates.splice(0)) open();
      setTimeout(done, 0);
    }));
    const take = () => page.evaluate(() => window.__SELF_T__.hands.splice(0));
    const hand = (hands, command) => hands.filter(([name]) => name === command).map(([, by]) => by);

    // The checkbox: an agent's flow clicks it by click() after an await; the
    // person clicks it, and its label.
    const flipped = await press("Kind flip");
    await release();
    const pageFlip = await take();
    await page.click("#kind-check");
    const personFlip = await take();
    await page.click("#kind-check-words");
    const labelFlip = await take();

    // The cookie box, in its settings pane.
    await page.evaluate(() => {
      setSettingsOpen(true);
      showSettingsPane("appearance");
    });
    const cookie = "#opencode-cookie";
    // The person types and leaves the box by clicking the next one.
    await page.click(cookie);
    await page.keyboard.type("auth=typed-by-person-1");
    await page.click("#opencode-workspace");
    const left = await take();
    // The person types and presses Enter.
    await page.click(cookie);
    await page.keyboard.type("auth=typed-by-person-2");
    await page.keyboard.press("Enter");
    const entered = await take();
    // The box holds text no hand typed; an agent's flow moves the focus after
    // an await.
    await page.click(cookie);
    await page.evaluate(() => { document.getElementById("opencode-cookie").value = "auth=written-by-page"; });
    const moved = await press("Kind move");
    await release();
    const agentMoved = await take();
    // An agent fills the box the person stands in; the person leaves it.
    await page.click(cookie);
    await page.evaluate(() => asAgentPress(() => selfActFill(document.getElementById("opencode-cookie"), "auth=filled-by-agent")));
    await page.click("#opencode-workspace");
    const agentFilled = await take();

    ok("the agent's flow presses were made", flipped === true && moved === true, JSON.stringify({ flipped, moved }));
    ok("a page's click() on a checkbox after an await names no person", hand(pageFlip, "jira_connect").join() === "none", JSON.stringify(pageFlip));
    ok("the person's click on a checkbox says person", hand(personFlip, "jira_connect").join() === "person", JSON.stringify(personFlip));
    ok("the person's click on a checkbox's label says person", hand(labelFlip, "jira_connect").join() === "person", JSON.stringify(labelFlip));
    ok("the person leaving the cookie box they typed in saves as person", hand(left, "set_opencode_cookie").join() === "person", JSON.stringify(left));
    ok("the person's Enter in the cookie box saves as person", hand(entered, "set_opencode_cookie").join() === "person", JSON.stringify(entered));
    ok("an agent's flow moving the focus saves no person's cookie", hand(agentMoved, "set_opencode_cookie").join() === "none", JSON.stringify(agentMoved));
    ok("the person leaving a box an agent filled saves as the agent's", hand(agentFilled, "set_opencode_cookie").join() === "agent", JSON.stringify(agentFilled));
  });
}
