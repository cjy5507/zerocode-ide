# Synthetic expense workflow

This fixture starts with a synthetic session.
It sends no external requests and contains no real account data.
Open `index.html` in a new working tab.
Direct links return home; use the approval menu.

The scenario copies last month's document.
It changes the controlled date and amount fields.
It selects the refund form and the team plus finance approval route.
It attaches `receipt.txt` in the modal and confirms the synthetic submission.

Run the browser script checks:

```sh
node ui/tests/expense-workflow.mjs
```

Compare scripted evaluation and structured browser commands:

```sh
node ui/tests/expense-workflow-bench.mjs /tmp/expense-bench
```

The comparison uses real window chrome and shipped browser scripts in Chromium.
It excludes native browser IPC and model latency.
Both drivers already know the intended steps.
Their call counts do not measure how a model learns the workflow.

Run the same scenario through the installed window's CLI:

```sh
node ui/tests/expense-workflow-cli.mjs /tmp/expense-cli
```

The CLI run needs a published build with `upload`.
It closes only the synthetic tab it opened.

Run the real Rust recipe runner and replay the saved procedure:

```sh
ZC_EXPENSE_EVIDENCE=/tmp/expense-recipe cargo test -p zerocode-shell \
  --bin zerocode-shell expense_workflow_runs_and_replays -- --ignored --nocapture
```

This opt-in benchmark requires Node and Playwright Chromium.
Its browser boundary runs shipped scripts in an isolated guest.
It never sends operating-system input to the person's window.

Saved browser procedures use tab aliases:

```text
1. `zerocode-browser open https://example.test/work` — opens $tab
2. `zerocode-browser wait $tab #ready`
```

Each run binds `$tab` to the completed `open` reply.
A procedure without an open needs `recipe-run --tab <browser-label>`.
Additional external tabs use `--params` keys such as `tab-2`.
The runner never chooses another tab when one is missing or closed.
A resumed call after an open needs an explicit tab again.
The stopped report's `browserTabs` lists the labels to supply.

Secret inputs use whole references such as `$secret_text_3`.
An absent secret stops the procedure at the person's turn.
The saver checks remaining values with the core credential classifier.
It refuses a risky value using only line and step numbers.
Passwords, codes and card values stay out of saved input literals.
Secret entry is left to the person in the browser.
