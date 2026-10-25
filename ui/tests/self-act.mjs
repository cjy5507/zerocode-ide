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
      document.getElementById("hand-agent").addEventListener("click", async () => {
        void invoke("computer_take_control");
        await invoke("board_columns_probe");
        void invoke("answer_ask");
        await T.later;
        void invoke("computer_give_back");
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
}
