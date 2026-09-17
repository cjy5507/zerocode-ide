---
name: design
description: >-
  Use the ZeroCode design contract whenever you touch this product's own UI:
  writing or editing ui/shell.css, ui/index.html, ui/tokens.css, a panel, a
  dialog, a row, a badge, or any pixel the window draws. Load it for "UI",
  "화면", "패널", "디자인", "스타일", "CSS", "component", "hover", "색",
  "토큰", "다크/라이트" — and before inventing any color, spacing, radius,
  font, or motion value. Do NOT load it for terminal PROGRAM output, for pages
  the app merely hosts (browser panes, artifacts), or for backend work that
  draws nothing.
---

# ZeroCode Design

You are drawing part of a macOS control room whose one job is answering
**"지금 누가 나를 필요로 하는가?"** without the user rolling their eyes.
Every rule below serves that sentence; the full argument lives in
`docs/design/direction.md`, and most rules are enforced by tests in
`crates/zerocode-shell/src/main.rs` — when a rule names its gate, breaking
the rule breaks the build, so read the gate before negotiating with it.

**Where values live.** `ui/tokens.css` is the machine-readable original —
never restate a hex, a shadow, or a duration anywhere else, this file
included. Reference tokens by role. If a value you need has no token, that
is a design decision to surface, not a literal to inline. The palette itself
is guarded by `the_palette_is_zero_codes_obsidian_atelier`.

## Artifact creation surfaces

The September 12 brief asks for expressive artifact design inspired by Hermes.
The studio and artifact previews may use purposeful color, layered material,
and larger display type. Define those decisions in `ui/tokens.css`, derive
colors from the current theme, and retain clear controls and reduced motion.
This updates the older hue-only-lanes and small-type restrictions for these
creation surfaces. Hosted artifacts follow `artifact-design`.

## The connected workbench

The same brief extends that expression to the knowledge graph, the task board
(and its relation graph) and the workspace board — four maps of one bench
(`docs/design/direction.md` §2-b). The rules that keep it one system:

- **A purpose hue names a view.** `--workbench-hue-knowledge`,
  `--workbench-hue-tasks`, `--workbench-hue-workspaces` and
  `--workbench-hue-artifacts` alias ramps that already flip for light; a view
  binds `--workbench-accent` on its root (tokens.css tail), and every mix that
  reads it (`--workbench-tint`, `--workbench-edge`, `--workbench-accent-ink`,
  `--workbench-ambient`) is resolved on that same element. The hue appears
  where the view is named — masthead mark, nav tab dot, a related door that
  leads there, the one row the view has selected — and nowhere else.
- **State keeps its signals.** Attention, working, done and failed stay on
  `--signal-wait`, `--signal-ready`, `--signal-done` and `--signal-halt` with a
  shape or a word beside the colour; a view hue never stands for a state. On a
  signal-tinted ground, words take `--signal-wait-ink` or `--signal-halt-ink`.
- **Plan, activity and verification stay three facts.** On a workspace row the
  person's stage is a hollow square in the stage colour, the agent's activity a
  status dot whose shape changes with its state, verification the PR pill and
  its doors. Never merge them into one chip or one colour.
- **The selector contract** is `view > .workbench-nav > .workbench-nav-link
  [data-workbench-view][aria-current]` with an optional `.workbench-context`
  pill inside the strip, `.workbench-related > .workbench-related-*` in
  inspectors, `.knowledge-inspector-request` and `.workspace-board-tasks`. The
  strip is drawn under each masthead whatever its DOM position.
- **Width is the view's, not the window's.** Narrow rules ask the board and
  workspace-board containers or the knowledge/artifact tier classes, so a split
  pane at 320px wraps the same way a 320px window does; never add a fixed
  minimum that hides an action. The knowledge canvas stays flat paint.

## The seven habits the gates already enforce

1. **Roles, not colors.** Surfaces climb `--surface-abyss → -well → -deck →
   -raised`; words are `--ink-chalk` (primary) or `--ink-mist` (secondary).
   `--edge-rule` is **border-only** — at 8% alpha a filled shape disappears,
   so anything that must read on its own (thumbs, dots, separators) takes
   `--edge-mark`.
2. **Hue points at lanes, nowhere else.** Lane identity color appears in the
   four places that mean "this lane" — ledger tick, worktree rail, tab dot,
   permission bar. Routine chrome stays neutral (the connected workbench's
   view hues follow the same pointing grammar, above). `--signal-halt` exists
   for destructive actions alone.
3. **State is never color alone.** Live states also change shape or words
   (double hairline for awaiting-permission, notch for blocked, a label for
   screen readers) — a colorblind user must get the same answer.
4. **A state change is eased, not snapped.** Any class that owns a `:hover`
   or state class must be named by a rule carrying the shared motion pair
   `var(--motion-fast) var(--ease-standard)` — the gate is
   `a_state_change_is_eased_not_snapped`, and `prefers-reduced-motion`
   always wins.
5. **Rhythm is the scales.** Spacing sits on `4 / 8 / 12 / 16 / 24 / 32`,
   type on `11 / 12 / 13 / 15 / 18 / 24`, radius on the small ledger scale —
   full-bleed rows divided by 1px rules, not card stacks. A px value outside
   the scales needs a written reason next to it.
6. **Korean wraps by word.** `word-break: keep-all` is the typesetting
   contract; never reintroduce a global `overflow-wrap: anywhere` that
   undoes it.
7. **Every user-facing string rides `t()`** with all four catalogs (en, ja,
   zh, es) filled — `no_catalog_key_is_shipped_unused` and
   `every_key_the_window_asks_for_is_translated` walk both directions, and
   `every_catalog_speaks_its_own_language` rejects a catalog holding another
   language's sentence. A literal in markup is a Korean-only screen for four
   locales.

## Non-negotiables from the platform

- Dark is the default; light rides `data-theme="light"`. Both must work off
  the same tokens — never define a color only inside one theme's block.
- Keyboard focus is always visible: 2px `--ink-chalk` outline, never a lane
  hue.
- The layout survives any resolution — 320px to 3840px wide, 320px tall,
  split panes and UI zoom (the user's 2026-09-12 requirement, replacing the
  old 720px floor). Side panels fold and toolbars wrap; a destination is
  folded, never `display:none`-erased, and no fixed minimum hides an action.
  A surface measures its own width (container query or tier class), never
  the window's.
- Accessibility is gated: the settings surfaces run axe (WCAG A/AA) in
  `ui/tests/settings.mjs`; new controls need names (`aria-label`), and icon
  buttons say their name twice — tooltip and `aria-label` — through the
  existing `labelButton` helper.

## How to verify what you drew

- `just shell-test` runs the design gates named above.
- `node ui/tests/window.mjs` exercises the drawn window; add your surface's
  contract there the way neighboring tests do — measured assertions on
  computed style, not screenshots.
- The visual checklist (dark/light, five lane states, permission modal,
  320px-wide, 320px-tall and split-pane reflow, reduced motion) is in
  `docs/design/direction.md` §5.

## What this direction refuses

High-contrast black-terminal/grey-chrome splits, decorative purple accents,
10px card stacks with accent rails, emoji as section markers, a single font
for everything. If a mockup asks for one of these, the answer is the
sentence at the top of this file, not the mockup.
