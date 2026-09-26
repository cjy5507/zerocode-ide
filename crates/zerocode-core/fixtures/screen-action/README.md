# Screen action cut cases (t-10324)

A screen question offers at most `MAX_ACTION_CANDIDATES` (12) of a look's
controls. When a look holds more, `screen_action::pick` keeps the ones the
goal's words name most (`Pick::ByGoal`) and hands them back in the look's own
order; `Pick::InOrder` is the cut every question made before — the first
twelve the look numbered.

Each `.json` file directly in this directory is one case:

- `goal` — the sentence a person wrote.
- `items` — the look's numbered controls, as the marks answer carries them
  (`mark`, `role`, `label`, `centerX`, `centerY`).
- `fields` (optional) — the fields a page's look read beside them (the
  snapshot's `fields`: `mark`, `kind`, `secret`, `label`, `placeholder`,
  `near`).
- `expected` — the numbers a person would press next; any one of them offered
  is the answer offered.

`screen_action::tests::the_regression_set_offers_every_answer` asks every case
the way a walk asks it, under both cuts in one binary, prints where the best
expected number stood before (the look's order) and after (the goal's
ranking), and fails if the goal's cut leaves an answer out.

| case | what it holds | answer |
| --- | --- | --- |
| `thirteenth` | a sidebar of 17 rows, the folder named by the goal 13th | 13 |
| `last` | 30 settings, the goal's toggle last; a dozen others hold the goal's `on` | 30 |
| `repeated-rows` | ten rows of the same two buttons (`편집`, `삭제`), then the goal's | 21 |
| `korean-particles` | a Korean goal whose words carry particles (`모드를`) | 16 |
| `similar-names` | a File menu of `Save`, `Save All`, `Save As…`, `Save as Template…`; the goal quotes one | 17 |
| `offscreen` | a page's footer link below the fold, 26th | 26 |
| `two-answers` | two controls that both send (`Send`, `Send message`) | 14 or 22 |
| `field-placeholder` | an unlabelled search box a page's field names by its placeholder | 15 |

`held-out/` holds three cases no test reads (`synonym`, `short-words`,
`korean-send`): they are run by the ignored
`held_out_cases_are_reported` and their results are reported, never held to.
