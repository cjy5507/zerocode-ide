# Changelog

## [1.1.20] — 2026-09-24

_since v1.1.19 (47 commits)_

### feat
- feat(window): an agent's open names the pane it borrows for, a returned device takes that pane's mirrors, and the status bar counts what is lent (t-6336 D2)
- feat(jev): a repeated run answers its questions from the judgment memo — the table's `repeat` column says the cache stands `on` there while its person left it `auto` (t-6385 U2)
- feat(jev): every seat names the two confidence lines that split its answers into abstain, confirm and act — recorded, not yet read, and one counter draws a seat's curve per fifth and per band for the replay and the day the lines move (t-6342 4)
- feat(jev): every seat is held against its cheapest reader and a label that never says no — the table names each seat's baseline and three wanted disagreements, the writers stamp the baseline's own mark beside theirs, and the judge, the CLI and the dashboard say which line holds a seat (t-6342 3)
- feat(conversation): every code block wears the extension's copy over its corner, and a copy says it copied with a check for the panel's 2 s (t-6323 A9)
- feat(conversation): a picture a message carries stands as the extension's pill — fetched only once it is in view — and a press opens it whole; the transcript reader no longer drops a line that carries one (t-6323 A8)
- feat(jev): a pinned model narrows a summons' options to the agents that run it — the code reads the catalog's launch table and the family's gauge, asks nothing when one agent is left, and tells the question the model when two are (t-6342 2)
- feat(conversation): a todo call draws its list under the extension's head, and in the Focus view the newest list stands out of its fold (t-6323 A7)
- feat(conversation): a helper at work stands at the list's foot — its description and latest step, its tokens, tools and time — from the session's own task frames (t-6323 A6)
- feat(conversation): the list keeps to its foot until the person leaves it — 50px, upward intent leaves at once, a send comes home, as the extension's list does (t-6323 A5)
- feat(conversation): the spinner says Claude Code's own verbs — picked again at the extension's beats and swept in with its `▌`, while a screen reader hears one word (t-6323 A4)
- feat(conversation): the extension's keys — Esc interrupts the turn from anywhere on the page, Shift+Tab steps the mode in Claude Code's order and words, ArrowUp/Down walk the prompts, Ctrl+J breaks the line (t-6323 A3)
- feat(conversation): a tool row's file is a door to the file tab, a read opens where it began and says its lines, and an answer's `path:12` keeps its line (t-6323 A2)
- feat(conversation): a row folds the way the extension's panel does — a tool body's side stops at 60px, a diff's box at 200px, what the person said at 60px, each with 「더 보기」 (t-6323 A1)

### fix
- fix(emulator): a device the door booted for an agent's pane is a loan, and goes down when that pane's work ends (t-6336 D1)
- fix(window): an agent's emulator mirror is seated in the asking pane's checkout, and never turns the person's head from another (t-6379 A2)
- fix(emulator): the emulator door names the pane that asked, and its open carries that terminal to the window (t-6379 A1)
- fix(conversation): a closed conversation's page leaves the leaf's host with its tab (t-6323 B2)
- fix(jev): the two seats stopped on their own evidence stand off when Jev is switched on, and zo asks its step seat nothing on a wire whose cache is keyed on effort — the provider catalog says which wire that is (t-6342 5)
- fix(jev): an effort move is graded only where the seat's answer moved what was carried — both effort seats read one rule, and a move held back, recorded, or the rule's own says why instead of passing for progress (t-6342 1b)
- fix(jev): a placed worker's pane is graded against the room it stood in, and only once somebody could have seen it — a quiet window with nobody in front of the pane leaves `unseen` instead of a mark (t-6342 1a)
- fix(zo): a recall turn that read and cited none of the notes it was handed carries no mark — it compared the order with nothing, and says so under `notCompared` (t-6342 1d)
- fix(jev): a stall answer is graded by one question — did the silence need the coordinator's hand — so a long tool the worker came back from on its own agrees, and a side that says nothing leaves its word under `notCompared` (t-6342 1c)
- fix(conversation): the chat's face keeps the platform's sans after the person's choice — the default Geist fell to the engine's serif (t-6323 A0)

### perf
- perf(emulator): an Android command is answered the moment it exits, and a look reads its display and AVD beside its dump instead of before and after it (t-6385 U3)
- perf(errand): a phone walk begins its next judgment on the screen its press settled on and answers it while the look is taken, instead of before the press (t-6385 U5)
- perf(window): a poll at the conversation's cap restyles the rows that moved, not the whole list — five sibling rules elsewhere keyed on every element (t-6323 B3)
- perf(conversation): a row far from view keeps its height, not its body — the 400-turn page stands the bodies of the rows within two screens (t-6323 B1)
- perf(emulator): an iOS look skips the grid under rows that answer their own centre, and an iOS press answers once the tree it led to stops changing, counting the walk's words there (t-6385 U4)
- perf(emulator): an iOS press by number is proven at the one point it lands on, and the tree is read only when that point cannot prove it (t-6385 U1)

### refactor
- refactor(zo): the step governor's label bookkeeping leaves `plan` — one helper writes the label a step owes, one owes the next step its label — so the function is back under clippy's length line with nothing it does changed (t-6342)

### docs
- docs(skill): the emulator's open seats its mirror in the agent's checkout and lends the device it boots to the agent's pane (t-6379, t-6336)
- docs(jev): public docs name the private tables and the recall mark in plain code, not as links — rustdoc's gate refuses a public page that points at a private item (t-6342)

### test
- test(emulator): a lent simulator goes down with its borrower and an unlent one stays up, on a real device (t-6336)
- test(window): the harness file server reads a file before it writes a header — a miss answers 404 once instead of throwing on a second writeHead and taking the suite down
- test(settings): the settings harness answers the loan line's boot question (t-6336 D2)
- test(jev): the branching seed finds the emulator row by its declaration and the label audit's fixture names the OpenAI family — a column added to every row and a versioned model id in a test module the literal gate reads as source were the gate's two reds (t-6342)
- test(jev): a label audit grades every seat's ledger on this machine again by t-6342's rules, beside the marks the ledgers hold — with each seat's baseline, its confidence curve and bands, and what the new rules would have asked — and asks nothing (t-6342 6)
- test(emulator): the walk bench starts its children through the one door and keeps each look's legend (t-6385)
- test(emulator): a phone walk timed on a simulator of our own, with no window, no mirror tab and none of the person's ledgers (t-6385)
- test(window): the conversation view's weight — one 400-turn transcript, five numbers (t-6323 B0)

## [1.1.19] — 2026-09-23

_since v1.1.18 (1 commits)_

### test
- test(knowledge): a frame budget is judged only on a quiet machine — one helper reads the load once, three budgets ask it, and every metric line names the load

## [1.1.18] — 2026-09-23

_since v1.1.17 (12 commits)_

### feat
- feat(jev): the dashboard and the card in one design — sizes, weights and tints are tokens on the window's own scale, every text clears 4.5:1 in both treatments, the head stays while the rows scroll, a week nobody asked says why, the strip's figures stand over quiet labels with the states apart, and the switch over the table wears the card's one frame (t-6277 D10)
- feat(jev): each feature wears one chip of four — off, recording, applying, or waiting on a person, named for the key or the consent it waits on; a feature under its bar is recording and says why in the tone of a miss (t-6277 D8)
- feat(jev): each feature on the dashboard names its id under its name and tips what it judges in one sentence — the first of the paragraph its drawer shows, from the same key, in the window's own tooltip (t-6277 D7)
- feat(jev): a feature's row opens a drawer — its last twelve judgments, how often they matched, what a screen feature's guards stopped and what it handed to the person, the model and the version cut away, and what it sends; the picker under the table is gone (t-6277 D6)
- feat(jev): a person turns Jev on and off with one switch — the card keeps the key, a line to the dashboard and 고급 folded shut, the dashboard wears the same switch over its table, and no feature offers a mode to choose (t-6277 D9)
- feat(jev): one switch decides a feature nobody chose for — each use's table row names the mode it stands at while Jev is on, the switch writes every folder as one word, and a press on or off is the core's to spell (t-6277 D9, core)

### fix
- fix(zo): a step row files the chat model it ran on as `stepModel`, so `model` on the step seat's ledger means only the Jev version that answered (t-6284)
- fix(jev): only a request or a mark names the version that answered — zo's step rows, which carried each step's chat model under `model`, no longer cut the step seat's marks away at every step (t-6284)


## [1.1.17] — 2026-09-23

_since v1.1.16 (30 commits)_

### feat
- feat(zo): every OAuth login has a discovery row — xAI (an XAI_API_KEY or the Grok CLI's login) and Kimi Code — with one xAI credential resolution the client and the list share, and xAI's spending limit said as "no subscription or credits" (t-6248, C5)
- feat(zo): 거부된 응답 뒤 공급자 폴백 — Anthropic 계열 refusal_fallback을 후보 목록으로, 두 번 거부면 quota와 같은 cross-provider 길로 넘기고 연속 거부 뒤 pre-arm, 폴백이 없으면 거부된 교환을 빼고 한 번 더 (t-6269)
- feat(jev): what did not come back is a chip in words, and the ones a person clears are buttons to where they are cleared — one token table for the dashboard and the key check (t-6243 D5)
- feat(jev): each feature's state is one chip, one sentence and the samples still owed, and the model is named once over the table (t-6243 D2)
- feat(jev): a strip over the dashboard, the busiest features first and the unused ones folded into one row — the day's requests against the limit ride every refresh (t-6243 D1)
- feat(orchestration): a worker briefing says its purpose first — ordinary engineering on the person's own repository, where product code may name refusals, safeguards or security tools without asking for any of them
- feat(jev): the dashboard and the card speak a person's words — features, requests, response rate, accuracy and the bar for automatic use, not seats, rows, windows, thresholds, ledgers, probes and rising (t-6243 D0)
- feat(jev): the challenger's row, its blind and its reservation — the day's share counts what is reserved, the judge sees two designs under no name, and a receipt outranks the comparison (t-6151, second cut)

### fix
- fix(zo): a provider client's debug line never carries its bearer, and the xAI credential tests read this machine's Grok login out of the way (t-6248 follow-up)
- fix(zo): the pre-arm notice reads its threshold and cooldown from the runtime's constants — no "2" and no "~30m" spelled out beside REFUSAL_DRY_TURN_THRESHOLD and REFUSAL_DRY_COOLDOWN (t-6269 follow-up)
- fix(shell): the system folder panel comes forward when it opens — the helper activates itself, raises the panel once it stands, and the window hands over activation cooperatively 350 ms after the spawn
- fix(zo): a shipped model the provider's whole list no longer names is marked withdrawn — the picker and zo models say so, routing passes it over, and its family alias follows the living release or says there is none (t-6248, C4)
- fix(jev): the samples bar is the reason while it counts, and a row keeps to three lines — nine features in use fit a 1080p screen with no sideways scroll (t-6243 D2 follow-up)
- fix(jev): a small sample says its size, not a share — accuracy under half its window, a response rate under five rows, and a feature the door refused throughout (t-6243 D3)
- fix(zo): a zo that started without a Claude login finds it at the next person's turn, or when the window's credentials change, and asks the model list again (t-6248, C3)
- fix(zo): a skip is due the moment this process holds the credential the writer lacked, and stays quiet where nothing is configured (t-6248, C2)
- fix(zo): a credential that is there but cannot be used is a failure, not a skip — only a machine with no login says "skipped" (t-6248, C1)

### perf
- perf(jev): the week is one picture of two lines — response rate and accuracy — drawn only from three days with values; the latency line leaves for its own column (t-6243 D4)

### test
- test(release): the flake list names the pending-steer resubmit's harness wait — it ran out under load 14 with a worker build beside it and passed alone three times
- test(release): the flake list names the pty lane's foreground wait — it ran out under load 32 with the pty untouched and passed alone three times
- test(shell): the folder panel's table names its raise delay, and the flake list names the headless login's wall-clock budget
- test(release): the flake list names the session-recall search bound — it overran under load 46-120 with session_recall untouched and passed alone three times
- test(zo): C3's stamp test takes the env lock — it points CLAUDE_CONFIG_DIR at its own login, and the next-turn test beside it read that login (t-6248, C3)
- test(zo): the patch review replay asks the same patches under four task readings — the person's newest words, their last three, the model's plan, the turn's todo plan — and none ranks regret (t-6232)
- test(zo): the cron registry takes its creation second as a parameter, and the wake-up test reads a known clock instead of the wall's — the one-in-sixty race at the minute boundary leaves the flake list (t-6230)


## [1.1.16] — 2026-09-23

_since v1.1.15 (23 commits)_

### feat
- feat(settings): the patch review seat's row on the Jev card, in all five languages, after the challenger's (t-6203)
- feat(zo): the patch review seat — four Noul questions of every patch an edit wrote, one code verdict, one line under on, hindsight labels, a digest on every row (t-6203)
- feat(jev): the patch review seat's row — a written patch, the person's words and the evidence's tail, four questions, hindsight marks — and a request's whole digest beside its fingerprint (t-6203)
- feat(jev): a control a press cannot take back asks nine in ten, and a fork never explores one (t-6187 A3)
- feat(jev): a screen question asks two free guards beside its choice, and an acting walk will not press on a screen that gives it orders or on a wall (t-6187 A2)
- feat(jev): every row names the version that answered, a seat is judged on the newest version's rows, and a person can pin the model (t-6187 A1)
- feat(jev): the challenger seat — a model nobody has evidence for is asked for the same design on one attempt in five, and the two are compared with their names hidden (t-6151)

### fix
- fix(ui): the challenger row's Korean reads 견줍니다 and 켜는 것입니다 — three typos in the settings card
- fix(zo): the patch review's row is written beside the result, not before it, and its public docs link nothing private (t-6203)

### refactor
- refactor(jev): a Noul wears the closed choice's envelope instead of spelling its keys again (t-6187 A2)
- refactor(jev): the seat switches, the classifier and the model pin write `smart` through one door (t-6187 A1)

### docs
- docs(jev): the patch review caps are measured on the patches themselves, sidecars not counted twice, and the row's files are rustfmt-clean (t-6203)
- docs(jev): may_send names the model field it writes, not a private helper rustdoc cannot link (t-6187 A1)

### test
- test(zo): the edit-thread timing runs off and shadow in both orders (t-6203)
- test(zo): what the patch review costs an edit's own thread, off and shadow, measured (t-6203)
- test(jev): the patch review replay — every patch this machine's zo sessions wrote, asked from before it and graded by the turns after it, in a home of its own under $0.20 (t-6203)

### release
- release: the cron minute-boundary test and the growing-PTY spinner test join the load-flake list

## [1.1.15] — 2026-09-23

_since v1.1.14 (62 commits)_

### feat
- feat(window): a `read --full` after a fold labels the fold regretted, and the read measurement shows page medians and totals without the bot walls (t-6162 F6)
- feat(jev): the judgment cache offers `on` — a labeled seat carries all four words, the person's own override among them (seat contract, second correction; t-6132)
- feat(computer-use): the second rung — a screen judgment under its press floor is put to the frontier, headless, before the walk steps back to the person (t-6132 S3)
- feat(computer-use): a goal walk asked to `--overlap` judges the next look before the press lands — the question begun on the last look is used when the next look asks it to the byte, dropped when it does not (t-6132 S2)
- feat(window): every CLI that signs itself in has a row — the login table carries the survey's twenty-nine, and says plainly where it cannot read one (t-6120)
- feat(jev): the judgment cache seat — a screen question the door already cleared with the same bytes is answered from a memo, and a Flow walked again asks the wire nothing (t-6132 S1)
- feat(window): the branching seat — a forked phone step tries the judgment's top candidates on a saved AVD and presses the result Jev picks (t-6044)
- feat(recall): a page five readers left unopened is not brought in by the graph — RecallDemand seam behind with_demand, unaddressed pages sink past the hub lift and stop arriving as neighbours; own words still recall them. Not wired to a ledger yet (the recall seat labels only its first note). Replay over 681 queries: read-history slots 29.1%→34.9%, unaddressed slots 35.1%→8.5%, 63 read pages displaced by newly admitted neighbours
- feat(jev): the mention rerank seat — the @ popup and the /resume list keep their fuzzy page, and one judgment may reorder it only while the selection still sits on row one (t-6042)
- feat(window): the notify seat — one ring judged where today's table decides, held or dropped only under a risen auto, and labeled by the person's own hand (t-6043)
- feat(window): the browser read folds a page's furniture — one Jev seat cuts the body at its landmarks, asks each block in parallel shards, and the next press labels it (t-6041)
- feat(jev): the agent tool seat — zo's `Jev` tool and `zo jev ask|choose|score` through the one door (t-6040)
- feat(jev): the compaction seat — tool results leave the summary by relevance, not age, and the turns after grade the drop (t-6039)
- feat(knowledge): the vault's graph grafts the code its pages name — files ⬢ and definitions ✚ on measured lines (t-5970)
- feat(window): Grok and Kimi sign in from the accounts pane — one CLI login table, Codex on the shared runner (t-6003)
- feat(zo): `impact` — the callers and tests one definition reaches, asked before changing it (t-5970)
- feat(knowledge): the lens's picture exports as one self-contained HTML artifact (t-5966 G4)
- feat(knowledge): paths between two pages are one calculator in core, asked by the window and by `zo vault path` (t-5966 G3)
- feat(knowledge): every relation names the road that wrote it — measured, declared or inferred (t-5966 G1)
- feat(codegraph): find_references can answer one definition, not a spelling (t-5970)
- … 2 more feat commits

### fix
- fix(window): a branching row carries fingerprints of the goal and the device's name, not the words (t-6162 F12)
- fix(window): a read row carries the host and the path's fingerprint, never the path (t-6162 F8)
- fix(zo): the compaction replay takes one point per transcript and shows medians beside the pooled shares, with a Wilson floor on regret (t-6162 F5, F10)
- fix(zo): the mention replay's intent no longer carries the file it names — the path and its name come out as substrings, and the table counts what is left (t-6162 F4)
- fix(branching): the replay bounds the first pass alone — a Wilson lower bound over pooled passes was a sample twice its size (t-6162 F2)
- fix(branching): a fork is wanted only when the seat is torn — a lead under the table's margin, or a leader under the press floor — and the golden stops spelling its own answers (t-6162 F3)
- fix(jev): no seat rises with no marks to hand — the sample floor holds a hindsight seat exactly as it holds a comparison seat (t-6162 F1)
- fix(jev): one mode set per kind of seat — every labeled seat offers off|shadow|on|auto, and the notify and branching rows stop being the odd ones out (t-6162 F7)
- fix(ui): the agent-tool hint names the Jev tool and the zo jev command without backticks — no user-facing string is written in markdown
- fix(jev): the adversarial verification's nine confirmed defects — consent pointers, replay leaks, thin labels, recall reads, hedge seed (t-5961)

### docs
- docs(window): the notify replay's table says what a hand is (t-6162 F9)

### test
- test(zo): the recall seat's rise test dates its labels inside the window it judges (t-6162, F1 follow-up)
- test(window): two tests meet the F1 floor and the F3 margin after the S-wave merge (t-6162)
- test(core): the recipe-run manual line the flow tables are pinned to now shows [--rescue] (t-6132 S3 follow-up)
- test(window): the bell ladder contracts name the rungs without their visibility — the shipped-source reader strips pub(super) (t-6043)
- test(knowledge): an edge label's word is its first text node — the tooltip title sits behind it (t-5966)

### release
- release: the parked-pointer hook test joins the load-flake list
- release: the google_login seat-retake test joins the load-flake list

### diet
- diet(zo): two deferred-tool hook lines shed fifteen characters — the r49 tool bucket holds the agent tool seat beside the code graph

## [1.1.14] — 2026-09-22

_since v1.1.13 (19 commits)_

### feat
- feat(zo): the composer's `@` opens Codex 0.155.1's mention popup — a nucleo file search, skills, and this session's files and vault pages first (t-5871)
- feat(jev): auto rises for the first time — the window forgives a bad minute, a seat with no reader to compare against is judged on its own ledger, and no judgment is spent on a window that cannot be full (t-5875)
- feat(zo-tui): thinking says what it is doing, its body stands as a folded cell, helper rows carry model and tail, and the status row names the route Jev chose (t-5872)
- feat(summon): the choice is asked about the task and against this ledger's own record (t-5873)

### fix
- fix(jev): a hedge leaves only where its second copy has room to land (t-5874)
- fix(zo): the terminal reader is crossterm's poll-based tty source — a key that lands beside a SIGWINCH no longer waits for the next keystroke
- fix(orchestration): the quota-wall witness knows a model's own cap — "You've reached your … limit"

### test
- test(zo): the page-mention completion test reads the key's outcome, as its must_use asks (t-5871)
- test(zo): the `@` popup golden, and the last two lints (t-5871)
- test(summon): the row keeps what the question was told, and a replay measures the seat on it (t-5873)

### release
- release: four runtime spawn tests join the load-flake list
- release: two tools-crate process spawns join the load-flake list
- release: the personal-data scan forgives every retina asset name, not only the square ones
- release: the iOS helper corpse test is a load flake the lane judges solo

## [1.1.13] — 2026-09-22

_since v1.1.12 (21 commits)_

### feat
- feat(jev): the three unlabeled seats get a "what came next" row — routing labels the turn's route by its attempt, placement labels where the person left the pane, recall labels whether the first note was read — and recall rises on its own labels
- feat(jev): the window's Jev dashboard — every seat's numbers, decisions and trend on one tab (t-5807)
- feat(zo): the key ladder names the rung that answered (t-5805)
- feat(jev): a consented checkout's linked worktrees are consented too (t-5805)

### fix
- fix(ime): a commit that leaves only half a letter waits for the key that caused it (t-5835)
- fix(ime): the half letter's beat is read at the door, not raced against its own timer (t-5835)
- fix(emulator): the helper's own starts, deaths and replacements reach the window log (t-5763)
- fix(emulator): one iOS helper death, one line — and the tests that pinned the old silence (t-5763)
- fix(orchestration): a pointer a knock already took is not parked again while its offer is young
- fix(jev): an unarmed process leaves no seat row in the person's own home (t-5805)
- fix(emulator): a frame reader that walks away costs the iOS helper a thread, not its life (t-5763)

### docs
- docs(agents): Windows is a build target for Codex sessions too

### test
- test(contracts): the gate that says half-built jamo never reach the pty now asks about the hold too (t-5835)
- test(zo): the ledger-cost measurement takes its source folder from the environment
- test(zo): the labels branch's summary tests call the test-local `one` the dashboard introduced

### release
- release: the public snapshot is scanned as the staged index, which is what gets pushed


## [1.1.12] — 2026-09-21

_since v1.1.11 (48 commits)_

### feat
- feat(release): the home-path row reads a Windows path too
- feat(release): a re-entry gate for private values — one table, one recipe
- feat(composer): the context meter is a ring you can also press to empty
- feat(window): a turn's tool work stands behind one summary, and the summary counts without counting
- feat(settings): a conversation remembers which way it is read
- feat(window): the composer counts the helpers running inside its pane
- feat(window): every row says who spoke it, and `/copy` hands the last answer over
- feat(window): the plan stands on the card that asks about it, and the refusal takes a reason
- feat(ask): a plan permission carries its plan, and its refusal carries the words
- feat(catalog): each console names its own word for winning context back
- feat(wire): a session says how full its context is, in the two numbers it was given
- feat(window): words typed mid-turn wait in the window, not in a pty nobody can see

### fix
- fix(zo): the OpenAI account follows the ZeroCode window, wherever zo runs
- fix(emulator): the probe's cold boot spells the snapshot flag as the one constant
- fix: the fixtures the gate reads leave docs/ too, and one carried a real address
- fix(emulator): an Android device that leaves the bridge is rested on, not hammered (t-5761)
- fix: nothing in the tree reads a design note, so a clone without docs/ builds
- fix(release): the gate's own fixtures live in the one table that waives them
- fix(emulator): an AVD is never launched onto a snapshot the emulator refused
- fix(window): the conversation list scrolls itself, never the page
- fix(conversation): the composer wears the state after the hook moves it, and the pins read the road as it is
- fix(composer): the queue gates on the pane the words go to, not on a helper's own running
- fix(integration): the merged console test closes its first arm, the design note carries lanes B and C, and the other-pane pin waits out the follow-up read

### perf
- perf(transcript): the context reader turns lines away by the byte before it parses one

### docs
- docs(agents): what never goes into git, as a standing rule beside the gate
- docs(design): Windows like the Mac — the builder is the first blocker, then unsigned installers and a two-platform feed
- docs: the abide borrow candidates and the model-catalog de-hardcoding plan
- docs(plans): the third wave's order is written down before it is placed

### test
- test(window): the queue suite waits for the wire's first read, not a clock
- test(window): the fold is pinned by what it must not lose, and the wave's order is written down

### style
- style(shell): the auth fixture's alphabet is laid out the way the root rustfmt lays it

### chore
- chore(release): the gate's cost is a measurement, not an estimate
- chore(tools): the moved rulers tell you where they are now
- chore: the tree names no person, machine or office before it goes public
- chore(zo): the scoreboard baseline is a measurement, not a note — it lives under zo-ide/bench
- chore: keep design notes out of the repository before it goes public

### release
- release: the lane publishes the source snapshot after the release, and says so when it cannot
- release: the public source repository carries one squashed commit per release

## [1.1.11] — 2026-09-21

_since v1.1.10 (19 commits)_

### feat
- feat(emulator): the first second of an iOS pane — it draws while the device boots, and the device outlives the window (t-5645)
- feat(emulator): an Android pane resumes a device that was already there, and says so from its first frame (t-5644)
- feat(zo): a step effort governor — effort per request inside the turn, held on Anthropic's wire (t-5633)
- feat(orchestration): move a worker's effort between turns through its row's door (t-5637)
- feat(zo): rank the skills with one Jev question each, and take the index out of the prompt (t-5629)
- feat(window): the conversation streams for every agent that can, wears the panel's own measures, and says why when it cannot

### fix
- fix(shell): a silence starts when the child last wrote, read once from the PTY — not now minus elapsed
- fix(zo): pay the tool plane for two deferred names, and stop two tests trusting an isolation they never had (t-5629)
- fix(shell): pin the scrcpy server to 3.3.4 so the Android pane mirrors again on Android 15
- fix(zo): say when to reach for the skill search, and clear two clippy lints (t-5629)

### refactor
- refactor(zo): one reader for whether a Jev seat has risen (t-5629)

### docs
- docs(emulator): the iOS half of the first second, measured (t-5645)
- docs(design): the first-second table carries the Android numbers t-5644 measured

### test
- test(shell): the quiet-since tests live in the unit-test file, and the stall-probe contract reads the epoch millisecond


## [1.1.10] — 2026-09-21

_since v1.1.9 (34 commits)_

### feat
- feat(zo): `zo mcp add --project --trust` — consent is its own act, and its own document (t-5584)
- feat(jev): the screen seats' auto rises on the walks it got right (t-5460)
- feat(zo): `zo mcp list|get|add|remove|login|logout` — the configured servers, from outside a session

### fix
- fix(shell): the macOS permissions page shows what macOS answers this window (t-5587)
- fix(accounts): the keychain says it does not know the account instead of inventing one (t-5419)
- fix(emulator): one table says where adb and emulator live (t-5451)
- fix(shell): the machine's hold names the window it belongs to (t-5444)
- fix(jev): a walk under a recording seat says why it pressed nothing (t-5455)
- fix(api): a wall another zo already paid for is answered before the request
- fix(orchestration): a summons reads its options before it is written down
- fix(window): Escape closes only what it reads, and the browser's restore record hears only the person
- fix(jev): a gauge too old to refuse on is no wall in the summon options, and a summons nobody offered is no comparison (t-4839)
- fix(emulator,ios): a subview two parents share is not a missing observation (t-5445)
- fix(api,tools): a wall hours away ends the retry ladder at once, and a parked provider is not probed

### perf
- perf(jev): warm the wire at the doors a walk starts behind, not inside the walk

### docs
- docs(jev): the screen seats' auto landed — a 35-row window at 900‰ and one root ledger as the count
- docs(jev): the screen seats' auto — walk-level agreed, one ledger under ~/.zo/jev, the ask deadline as the line
- docs(jev): zo under a quota wall — the L0 ladder against an unreachable retry-after
- docs(jev): the installed 1.1.9 browser seat answers in 242–290 ms after its cold first ask
- docs(jev): what the installed 1.1.9 shows for the emulator and routing seats

### test
- test(api): the parked-window door's hint is compared in whole seconds


## [1.1.9] — 2026-09-21

_since v1.1.8 (10 commits)_

### feat
- feat(jev): an acting routing seat runs the probe it skipped one turn in five as a control row
- feat(jev): a screen question carries what was pressed and what the screen shows, a dead login is its own stall cause the coordinator hears, a summons option carries the ledger's history, and one pooled socket answers every question

### fix
- fix(computer-use,emulator): an app answers to its bundle's own name, the iOS exporter says what is on top at each centre, a dead HID helper is replaced before it is handed out (t-5518)
- fix(runtime): hand a main-turn capacity wall to the quota escape after a burst, not after the account budget
- test(contracts): read the emulator road without whitespace (the v1.1.9 lane red)

### docs
- docs(jev): the seat accuracy wave — root causes, landings and the wire bench
- docs(core): the app-name rule's link names the function, not the macro
- docs(zo): zo 1.1.8 ↔ Codex CLI 0.155.1 gap table and closing plan (t-5506, analysis only)

## [1.1.8] — 2026-09-20

_since v1.1.7 (1 commits)_

### fix
- fix(window): seat a worker as a tab first and move it only on the placement seat's answer

## [1.1.7] — 2026-09-20

_since v1.1.6 (1 commits)_

### feat
- feat(jev): every seat records under auto and acts once its evidence stands

## [1.1.6] — 2026-09-20

_since v1.1.5 (3 commits)_

### feat
- feat(window): draw the conversation view as the Claude Code extension does and stream every delta by the next frame
- feat(jev): count the screen seats — stamp their rows and read every session's ledger

### fix
- fix(window): log where a worker pane is seated beside its spawn line

## [1.1.5] — 2026-09-20

_since v1.1.4 (2 commits)_

### fix
- fix(orchestration): a taken-back batch tombstones the receipt that named it

### release
- release: publish the app installer beside the feed it reads

## [1.1.4] — 2026-09-20

_since v1.1.3 (1 commits)_

### fix
- fix(jev): let the routing seat's auto rise on evidence a window can reach

## [1.1.3] — 2026-09-20

_since v1.1.2 (24 commits)_

### feat
- feat(computer-use): add mobile Jev walks with guarded presses
- feat(emulator): add deterministic mobile Flow checks
- feat(flow): stream the recorded run in a live console
- feat(emulator): number mobile controls and pin marked clicks

### fix
- fix(browser): invalidate find matches when page text changes
- fix(jev): count request preparation against the call deadline
- fix(tools): the one analysis script the gate imports loads on the python the gate actually finds
- fix(flow): bound merged history and live step caches
- fix(flow): bind retries and report links to recorded step origins
- fix(orchestration): require the live seat when restoring sleepers
- fix(flow): preserve the requested range during automatic recovery
- fix(flow): retain the workspace selected before queueing
- fix(orchestration): align task updates with safe boot repair
- fix(emulator): bind mobile looks to device identity and address
- fix(emulator): use logical Android display geometry for input

### refactor
- refactor(jev): share rubric fingerprints

### docs
- docs: record a51fa56a whole-gate results
- docs: record main-only consolidation and remaining verification
- docs(astro): record integrated mobile verification and final timings
- docs(jev): distinguish judgment cadence from evidence window
- docs(astro): the effort a summons carries is the task's difficulty, and the seat named for that choice keeps score rather than making it
- docs(astro): the working document stands on today's ground, and the doc it called optional is the one that moved

### test
- test(zo): isolate cooldown writes and deterministic routing expectations
- test(router): isolate probe endpoint overrides across routing tests

## [1.1.2] — 2026-09-19

_since v1.1.1 (2 commits)_

### fix
- fix(jev): both roads that judge draft the words they judged — the one mode that makes labels matter was the one that wrote none
- fix(browser): the marks answer a program reads carries the fence's flag instead of its marker lines

## [1.1.1] — 2026-09-19

_since v1.1.0 (22 commits)_

### feat
- feat(jev): a judged task can be labelled, because the words are written down where they still exist
- feat(jev): the judge runs — a seat's own ledger is judged as it fills, and a rise or a fall is written where the rows are
- feat(settings): each Jev seat carries its own ledger's numbers, and the card asks zo for them rather than counting
- feat(jev): `auto` decides — a seat rises on its own evidence, falls on the line it breaks, and says which one
- feat(jev): every seat's ledger is counted by one reader, natively — the numbers a seat is promoted on are now the numbers a person can read
- feat(jev): a walk asks only where the road forks — and the look, not the judgment, was the price: 262 ms of every step went to badges nobody opens
- feat(jev): the placement question gets a door and a row of its own, and nothing knocks on it — one room is implemented and no row says a judgment would choose a better one
- feat(memory): the bottom level is not injected — a note the judgment says bears on nothing is one the turn does not get, and 5 of 74 recalls were nothing else

### fix
- fix(computer-use): a walk may name its place by a pane, which the usage has always offered and the gate refused
- fix(jev): a seat's cost is read from the judgment rate table, which has had a price since the launch post
- fix(jev): the shadow freezes every path before it detaches — a unit test's rows were landing in a person's real ledger
- fix(ui): a late frame asks again before it moves the reader, the folded history says so once more, and a count that the chase decides stops being a verdict
- fix(release): the judgment that decides flake or real gets the calm machine a gate gets — it was running on the heat the zo gate had just left
- fix(ui): a new turn stops taking the reader's scroll, and the words the window ships are the words its tests and translations carry
- fix(release): a release goes where the app is looking — read off this checkout's origin it went to the source repository, which is private and which nothing polls
- fix(zo): public documentation stops pointing at items only its own crate can see — the doc gate was red on five links and no release could pass it
- fix(release): the repository a release goes to is read through the clone the gate runs in — read from the checkout alone it named a path, and the lane published nowhere
- fix(orchestration): a store this actor could not open is answered, not the end of it — a full disk shut the ledger and freeing the disk did not reopen it
- fix(settings): the fold's summary eases into its hover and 더 만들기's switch names its order — two contracts the narrowed gate walked past
- fix(orchestration): the grace asks the live teams to seat a sleeper before it ends one — three finished workers were announced dead with a coordinator sitting right there
- fix(bash): a full disk stops the work, not the hands — the floor now admits a command that can only look or only free
- fix(memory): the vault reading is Hit@k, and each k is its own call — renaming it costs the claim it was quoted for, and the path fix costs one line

## [1.1.0] — 2026-09-18

_since v1.0.0 (18 commits)_

### feat
- feat(settings): the sixth seat joins the card in the row grammar, and the count of a seat's modes is read off the table instead of typed beside it
- feat(routing): why the judgment did not run is written down — the reason existed, in memory, and died with the process that knew it
- feat(settings): the gate in front of the routing seat stands on the card — a mode nothing can reach now says so on the row it is set on (t-4713)
- feat(settings): a field is as wide as its value and the leftover width is the words — the API router pane, written as the grammar every pane inherits
- feat(jev): a judgment's second request leaves, and the day counts it — the real routing ledger names an 864 ms delay, read in 495 us
- feat(summon): which agent carries a summons becomes a judgment — the shadow writes what Jev would have chosen beside the three words the coordinator typed
- feat(type-value): the seat that writes a typed value is chosen by measurement, and the measurement is the table — the 400 ms bar was not met, and one reading would have picked the wrong row
- feat(jev): a judgment that has not answered yet is asked again, at a delay the wall sets — 63.6% of routing answers land in time, now 81.8%
- feat(rerank): a refused judgment names the rule it broke, so `schema` stops being one word for nine
- feat(jev): the recall judgment gets an exit — `on` reorders what a turn reads, and the routing judgment turns out never to be asked
- feat(supply-chain): the picture says what is wrong and now the report says what to raise — one line per dependency a member declares, and everything raising it closes

### fix
- fix(chat): the tool turn's tight gap becomes a token, and a fallback nothing could ever reach stops being a second copy of 11px
- fix(settings): a label with a sentence about it is two things, not one line — the switch rows' copy reads as [label + one line] now that the words have a measure
- fix(jev): one envelope for every closed choice — a tag four seats spelled by hand, and the one that forgot it has never written a row
- fix(rerank): the reply's checks allow for the rounding the wire does — 56.7% of judgments discarded, now 0%
- fix(ui): optimize conversation rendering, auto-scroll and composer stop state

### perf
- perf(computer-use): one node, one call — the macOS tree walk asks for its attributes together instead of fourteen at a time

### test
- test(memory): what recall MISSES, measured against labels a person already wrote — the right page is in the eight 83.4% of the time

## [1.0.0] — 2026-09-18

ZeroCode's first release from this repository. The program itself is not new —
this is the tree the 1.3 line reached, published as 1.0.0 from a repository
that begins here. The notes for the builds before it are kept below under the
version numbers those builds carried.

zo reads the router keys the window keeps — TypeSafe's among them — from the
window's keychain, once per process. A read the keychain could not answer was
remembered as the answer "this machine keeps no such key": `security` never
running, an interaction this session does not have, a lock another process
holds, all filed as absence. zo then went without a key the machine does have
for as long as the process lived, and every routing judgment after it recorded
`no_key` beside a keychain item sitting right there. A read that is not an
answer is no longer remembered — the next caller that needs the key asks once
more. An item that is genuinely absent, a platform that keeps no router keys
and the keychain kill switch are answers, and are still asked only once.

A browser pane no longer reports the same cookie import failure for ever.
Cookies brought in from another browser wait in a staging file until a pane of
that profile can lay them down, and any refusal kept the whole file for a
retry — but 519 of the 2,245 waiting on one machine here carried an expiry
that had already passed, which a cookie store refuses every time it is asked.
A cookie that can never be laid down is now dropped instead of retried, so the
staging empties and the notice stops.

The terminal keeps up on a slow machine. A pane no longer queues screens the
webview has not drawn yet: the screen pulls the latest frame when it can
paint, one paint per answer. A key typed while output is flowing is echoed
without waiting for the next display frame — on a throttled six-times-slower
webview the echo's p95 fell from 75.8 to 57.1 ms, and across fifteen flowing
scenes it is 5.3 to 22.5 ms faster, with no extra CPU or IPC.

Text in a terminal is as sharp as Terminal.app's. The terminal no longer
inherits the window's antialiased font smoothing (stem sharpness 0.76 → 0.99
against Terminal.app's 1.00), and a narrow symbol SF Mono lacks — such as
Claude Code's ⎿ — no longer pushes the rest of its line half a cell to the
right. Most of the remaining colour difference to Terminal.app is the theme:
Horizon Dark is the closest built-in match to its Clear Dark profile.

The knowledge graph's points now say what they are: pages are circles, links
without a page are dashed rings, sources are diamonds, and the shapes for
software components and vulnerabilities are ready for the supply-chain view.
Every shape at a given size covers the same area, the legend draws the same
shapes, and the window picks its drawing engine itself — the SVG/GL choice is
gone from the menu. Drawing on a Retina display no longer halves points and
lines, and translucent ink is no longer faded twice.

The knowledge graph can show a project's supply chain. Turn the supply-chain
lens on in the View menu and the Cargo and npm lockfiles of the workspace come
in as squares — one per component, inked by ecosystem, with a thicker edge for
the workspace's own crates — and the vulnerabilities OSV knows about them come
in as triangles on a severity ramp, joined to what they affect. Only the
members, their direct dependencies and the paths a vulnerability reaches stand;
the rest fold into a "+N" on the member that reached them first. A card names
a component's ecosystem, version, origin and lockfiles, or a vulnerability's
aliases, score, fixed versions and the path from a member, with a link to its
OSV page. Search takes kind:component, kind:vulnerability and sev:high. Only
public registry packages are ever looked up — git, path and private-registry
dependencies stay on the machine — and an answer is kept for a day. With a
thousand components the first picture takes 34 ms on this machine's GPU, and
the graph draws exactly as before while the lens is off.

A pane's conversation view reads like a chat again. What the person typed
stands on the right, and an answer from an agent with no streaming wire — zo's
— arrives a word at a time in the row that holds it instead of appearing whole
when the turn closes. The model chip now reads which model wrote the pane's
newest answer from the pane's own transcript, so a pane adopted after the fact,
or one whose window restarted, shows its model again. A slash command's replay
— the caveat, the command's name, its arguments, its local output — is no
longer shown as somebody speaking. The view no longer goes blank at the first
message either: a later report of the same session keeps the path to the
conversation the window already knew. The composer's model and command popups
stay inside the window.

Every place Jev sits is a row on the same card. Settings → API routers →
TypeSafe paints the use table itself, so the recall rerank, the stall sweep
and worker placement are switches a person can reach rather than keys to edit
in `~/.zo/settings.json` by hand — two of the five seats were reachable
before. Each row says where it stands and offers the same words, off, record
only, apply and auto, with the hint below it spelling out what applying would
mean there; a seat with no apply stage offers no way to apply. A seat added to
the table arrives on the card with it.

Every request to TypeSafe's Jev now passes one door: a key, the switch, the
person's consent for the workspace (`smart.jev.workspaces`), an optional daily
budget, and lines that may carry a credential withheld before anything is
sent. Browser recovery is judged for the folder the walk was started from, so
switching it on in a consented workspace now works. A worker that stops on a
silence nothing recognises can have its likely cause recorded by Jev, beside
what the coordinator did next — recorded only, never acted on.

Workers started by the orchestration ledger get their briefing again with
Claude Code 2.1.274, which drops anything typed before its input box appears;
the window now waits for the box. A worker stopped by a transient API error
can be resumed automatically under a declared order
(`handover-policy --on-transient-error resume`, three tries). A window opened
from the Dock shows Claude and Codex usage again, and a detached checkout no
longer makes every pull-request checkout refetch its checks.

A passing check must be shown able to fail. A verification receipt says a check
is green on the commit it names; it does not say the work made it green. A
check declared `must_fail_first` must now also carry a failing receipt for the
same command, bound to a different tree, before its pass is believed — so a
tree that was already green, or a check that cannot fail, no longer clears the
publish gate. And where a newly started worker should stand can be put to Jev
as a fifth use (`smart.workerPlacement`, off unless switched on), recorded
beside the measured rule that still places every worker.

_The sections below describe builds this program shipped before this
repository began again on 2026-09-18._

## [1.3.109] — 2026-09-17

TypeSafe's routing judgment can now be applied, not only recorded. Settings →
API routers → TypeSafe has a decision mode with three answers: Off (the
default, nothing is sent), Record only, and Apply. Both sending modes send the
first 2,000 characters of each task to api.typesafe.ai. Record only logs the
judgment beside zo's own routing probe; Apply uses a validated judgment
through the existing conservative routing rules, and falls back to the
existing probe for that task on an error or when no answer arrives within
1.5 seconds.

A multi-line paste into a zo pane the window had restored no longer submits
at its first line break. The stored screen of a restored pane now goes into
the terminal together with its spawn, ahead of the program's first byte, so
every mode a program sets — bracketed paste, the keyboard protocol, mouse
tracking, focus reporting, a hidden cursor — stays set. A pane already
running keeps the mode it has; panes restored by this build come up right.

v1.3.107 and v1.3.108 did not reach anyone. v1.3.107's release lane went red
on a test that gave a fake Codex two seconds to reach its composer on a busy
machine; v1.3.108's went red at the window crate's lint, which refused the
extra parameter the restored-screen change gave the command that resumes a
conversation. Everything in both ships here — zo can put every recall's notes
to TypeSafe's judgment in shadow (`smart.rerankShadow`, off unless switched
on) without changing what the turn reads, and
`tools/decision_shadow_summary.py` reads the routing decision ledger without
labels.

## [1.3.108] — 2026-09-17

A multi-line paste into a zo pane the window had restored no longer submits
at its first line break. When the window came back, it replayed each pane's
last screen with a command it sent after the pane had started, and that
replay ends by resetting the terminal's modes — bracketed paste among them.
A resumed zo had already switched bracketed paste on by then, so the reset
switched it off for the pane's whole life and a paste went in as typed keys.
The stored screen now goes into the terminal together with the spawn, ahead
of the program's first byte, so every mode a program sets — bracketed paste,
the keyboard protocol, mouse tracking, focus reporting, a hidden cursor —
stays set. A pane already running keeps the mode it has; panes restored by
this build come up right.

v1.3.107 did not reach anyone: its release lane went red on a test that gave
a fake Codex two seconds to reach its composer on a busy machine. Everything
in it ships here — zo can put every recall's notes to TypeSafe's judgment in
shadow (`smart.rerankShadow`, off unless switched on), without changing what
the turn reads, and `tools/decision_shadow_summary.py` reads the routing
decision shadow's ledger without labels.

## [1.3.107] — 2026-09-17

zo can put every recall's notes to TypeSafe's judgment, in shadow. With
`smart.rerankShadow` set to `shadow` in zo's settings, each recall a turn
performs is sent — off the turn, after recall has settled — to System One
as a Score question, and what the judgment would have reordered is written
to `<state>/smart-router/rerank-shadow.jsonl` beside the routing decision
shadow. Nothing the turn reads changes: the judgment stands behind the
vault's own ordering and cannot undo what the graph decided. It stays off
unless switched on, because it sends the vault's summaries rather than the
task's text, and consent to one is not consent to the other.

`tools/decision_shadow_summary.py` reads the routing decision shadow's
ledger without labels: rows, answer rate, uncached latency, and how often
the probe and the judgment agreed.

## [1.3.106] — 2026-09-17

v1.3.105 did not reach anyone: its release lane went red twice on test
fixtures that gave a shell script two seconds to start on a busy machine.
Everything in it ships here — above all, scrolling back through a pane's
history and coming back down shows the live screen again instead of the
pane sticking at the bottom on a stale window.

Those fixtures now have two named ceilings (ten seconds for a script that
must finish, five for a deadline it must overrun), so a loaded gate measures
the code, not the machine.

The window's event log stops filling with the SCM observer's thirty-second
tick — the line is written when the tick is news: slower than one GitHub
call's own ceiling, or out of calls. And a worker that ran in the main
checkout, or in a directory no project lists, is no longer reported as a
checkout the reclaim sweep "kept" at every boot: it was never the sweep's
to reclaim, and the line now says so.

## [1.3.105] — 2026-09-17

Scrolling back through a pane's history and coming back down shows the live
screen again. A return that was not a short, quiet scroll — a fling longer
than the screen, or a scroll down while the program was printing — sent the
window only the rows the program had touched, or none at all, and the window
kept the history it had been showing under a scrollbar that said live: the
pane looked stuck at the bottom, and each new line pushed the old rows up
into a mixed screen. Every such return is now a whole frame; a short, quiet
scroll is still the cheap shift it was.

Two release-gate checks stopped measuring the machine's load instead of the
code: the knowledge graph's 1020-page shortest path is timed as the best of
five runs, and the lane's pane-opening test gives its fake zo ten seconds to
publish an address, not two.

## [1.3.104] — 2026-09-17

v1.3.103 did not finish its gate, so it reached no one — everything in it
ships here, unchanged. The run stopped on a window test whose quiet clock was
not quiet: it stopped the window's 30-second clock by clearing three of the
maps that clock reads, after the clock had come to read five, and a pane an
earlier test left behind kept it ticking inside a 300 ms count. The test now
asks the clock itself whether it is quiet.

TypeSafe (Jev) has a place in Settings, under API routers. Save the API key
there and it is kept in the macOS keychain; every zo reads it from there itself
— one this window starts and one typed into a terminal alike — and nothing zo
starts inherits it. The same card turns the decision shadow on (record only;
routing never changes) or off, and 「연결 확인」 asks zo to put the shadow's own
question about a task nobody wrote to TypeSafe once with that key, showing the
model and how long it took, or why nothing answered. From a terminal the same
check is `zo decision-shadow check`.

The workspace you are looking at no longer turns bold as unread when its own
agent reports. The moment the news was read and the moment it arrived were two
clock readings, and when they fell a millisecond apart the card counted news
you were looking at as unseen.

## [1.3.103] — 2026-09-17

A conversation reopened from the sidebar while its workspace was still
bringing the same conversation back no longer starts a second agent on it. For
zo the second process died at the first one's writer lease and left an error
over a conversation that was already returning — five times on 2026-09-16. The
window now asks once, before anything is started, whether the conversation is
open or opening, and goes to that pane instead; pressing the same
past-conversation row twice lands on the one pane too.

A zo pane closed while zo was running ends the way /exit ends it. zo printed
its exit summary onto the terminal that had just closed and exited with a
panic — 129 of the last 300 zo panes the window launched.

zo can record a typed judgment of each task beside its routing probe, from
TypeSafe's System One (`jev-latest`) — off by default. With
`smart.decisionShadow: "shadow"` and `TYPESAFE_API_KEY` set, the first 2,000
characters of every probed task go to api.typesafe.ai and the answer is written
to the project's decision-shadow ledger under the task's fingerprint; nothing
routes on it and no turn waits for it. `zo --doctor` reports the ledger, and
`zo decision-shadow eval --labels <file>` scores the probe and the judgment
against a person's labels.

## [1.3.102] — 2026-09-17

v1.3.101 never finished its gate, so it reached no one — agents starting
again in a window opened from Terminal.app or iTerm2 ships here, unchanged.
The run stopped at the shell lint: a terminal test asked a map whether it held
a key with get().is_none() where clippy asks with contains_key. It asks that
way now.

A Claude worker launched into a repository its Claude configuration had never
trusted stopped at Claude's "Quick safety check" dialog, which stood in front
of the briefing until the launch gave up. Before the worker starts, the window
now marks that repository trusted in the configuration the worker will read
(CLAUDE_CONFIG_DIR included), under Claude's own config lock, keeping the
file's permissions and every other setting. A worktree is trusted through its
repository, the way Claude itself checks.

When a briefing still does not go in, the refusal names the readiness that
never came — no bracketed-paste handshake, output that never went quiet (and
how long ago it last wrote), no composer glyph, or no cursor — instead of
"Err(Timeout)".

An agent's `zerocode-browser open` puts its tab on the strip without taking
the person's stage, so an agent's research no longer flickers the terminal
the person is reading.

The live zsh history test runs zsh the way a pane runs it, so it passes when
the test runner itself was started from Terminal.app.

## [1.3.101] — 2026-09-17

Agents start again in a window that was opened from Terminal.app or iTerm2.
Every pane used to inherit that terminal's name (TERM_PROGRAM) and session id,
and Claude Code, believing it was inside Terminal.app, asked for the cursor
position five times a second to catch a Cmd+K clear. The window answered every
question, so an idle Claude pane never went quiet — and a worker's briefing and
a mail pointer both wait for that quiet, so launches failed and their worktrees
were removed. Panes now say they are ZeroCode, and the launching terminal's
session identity stays behind; zsh panes also stop running Terminal.app's
per-session history script against one shared session id.

The terminal answers when a program asks which terminal it is (XTVERSION), so
Claude Code now asks for synchronized output and draws each full-screen repaint
as one frame instead of letting a half-drawn one show.

A terminal frame is addressed to the webview that shows it, which keeps frames
on the right screen if the window ever holds two webviews.

## [1.3.100] — 2026-09-17

v1.3.98 and v1.3.99 never finished their gate, so neither reached anyone —
the terminal freeze fix and the knowledge graph's second hand written under
those two versions ship here, unchanged. Both runs stopped at the format
check: the new contract for the terminal's frame road was not wrapped where
rustfmt wraps it (v1.3.98's first run had also run out of disk). The contract
is wrapped now.

A routing outcome records the confidence the routing probe stated about
itself, including the "low" verdicts the fusion gate throws away, so the
hand-set confidence rules can later be checked against what actually
happened. No routing decision changes.

## [1.3.99] — 2026-09-16

The knowledge graph gets a second hand. A hand-written WebGL2 painter draws the
same picture in three instanced calls, so ten thousand pages stop being a
slideshow: the first picture of 10,000 pages lands in 159 ms against a 600 ms
budget, and the harness's scene time halves (160.3 s → 80.1 s). The SVG hand is
still the default and the GL one is chosen from 「보기」; both draw through one
painter interface, and a round of coordinator verification closed nine places
the automatic parity never asked about — a hover that never reached the GL
picture, ring labels that missed the grid's boxes, a failed GL start that left
its canvas and context behind, a tier change that never refit.

A checkout's changes, executions, receipts and decisions are read in one place.
The evidence view never runs anything and never creates anything, and an old
green test is never called current.

## [1.3.98] — 2026-09-16

A terminal that stopped drawing now comes back. v1.3.97 sent frames down a
Tauri channel, and a channel is an ordered stream: the webview half holds
every message after a missing one, forever, and the road the change was made
for — payloads over 8 KB, parked and fetched — drops its message on a failure
nothing in the backend can see. One dropped frame froze a pane for good, screen
and keystroke echo together ("입력도 안돼고 화면이 멈춤", "안살아남"). Frames
travel under an event named for the window reading them instead, so the
narrowing that channel was for is kept — Tauri skips a webview holding no
listener for a name before it serialises anything — while a frame that fails
now costs one frame. A line erased after a background tab comes back also stops
staying on screen: a snapshot is a frame the consumer installs, so the row
ledger becomes it too.

## [1.3.97] — 2026-09-16

A terminal's frames go down the channel of the window reading it. They
used to be broadcast to every open webview as JavaScript source (19.9 KB a
frame on an ordinary page, 115 KB on a dense TUI), parsed by each window
whether or not it held the terminal, twice with a board popout open. A
window now hands its own channel over with the call that declares which
terminals it is reading, and the pump sends each frame only there; a surface
that has not learned the channel still receives the old event, frame by
frame. The throughput harness also settles the glyph-atlas question: a canvas
lower bound matches the idle floor, so the atlas is not built.

## [1.3.96] — 2026-09-16

Terminal output stops waiting on itself. A round now drains what the child
has within a time budget and naps only the rest of the beat, so a 10 MiB
`cat` through a real pty crosses in 253 ms instead of 773 ms (40 MiB/s, 15
rounds instead of 42). The five-second `lsof` sweep of pane cwds no longer
runs on the pump's thread, so the five-frame hitch it caused every five
seconds is gone. Palette lookups stop forcing 256 style reads per miss.

A row that was written but did not change is not a row to send. A
full-screen TUI redraw that moved one spinner used to cross the wire as
every row (sixty at 220×60); the grid now keeps a ledger of what the window
was last handed and sends only rows whose content changed — one row, or no
frame at all when nothing did. A run's cells share one style object, and
sliding every node by the screen's own height is no longer done.

Measured before and after in both commits; the instrument is
`ui/tests/terminal-throughput.mjs`.

## [1.3.95] — 2026-09-16

A pane's 「대화」 opens at the end of its transcript. A session hours old
carries tens of megabytes behind it, and the view used to read them from the
first byte, painting days-old turns as if they were arriving now. The first
read now asks for the transcript's tail, the page says that the earlier turns
are folded (they stay on the pane's screen and in its session file), and the
hand-over's history read starts there too.

## [1.3.94] — 2026-09-16

The v1.3.93 changes, shipped: a pane's 「대화」 hands its conversation to the
streaming wire only when the CLI's screen actually leaves (the exit command
typed as a line, the pane's retirement as the receipt, the wire stopped and
the reason shown otherwise, one line in the window log each time); a
conversation view opened on a long transcript reads it to the end at once;
the view's clock counts this turn, not the pane's age. v1.3.93's lane was red
on a source contract that still spelled the answer card's old key walk — the
contract now reads the one walker the card and the hand-over share.

## [1.3.93] — 2026-09-16

A pane's 「대화」 hands its conversation to the streaming wire only when the
CLI's screen actually leaves. The exit command is typed the way a person
types a line (the text, then Enter), and the window waits for the pane to
retire before the wire stands; a screen that keeps the conversation gets its
wire stopped and the reason shown, so one session is never spoken for by two
processes. Each hand-over leaves a line in the window log.

A conversation view opened on a long transcript reads it to the end at once
instead of one chunk a second — the history no longer streams in as if it
were being said — and the view's clock counts this turn, not the pane's age.

## [1.3.92] — 2026-09-16

A pane's 「대화」 pressed mid-turn now says what it is waiting for. The
hand-over to the streaming wire happens only between turns; until then the
transcript view stands, and it stood silent — a person saw "nothing
changed". The page now says the conversation continues as a live session
once this turn ends, in four locales, and the line goes when the hand-over
fires or the toggle comes off. The page's notices are found and said by
their i18n key, so two of them never take each other for the one already
there.

## [1.3.91] — 2026-09-16

A streaming answer is revealed a word at a time: the wire page paces its
deltas into one steady front of fading words, keeps settled blocks still, and
hands over to the closed turn only after the reveal drains.

zo: the conversation anchor marker may outlive the rolling tail. An opt-in 1h
anchor (`cache.anchorTtl: "1h"`, or `ZO_CACHE_ANCHOR_TTL`) keeps the cached
conversation prefix across a person's break, where the five-minute anchor
re-billed the whole prefix on the way back — 47% of every re-billed token on
this machine's ledger. The rolling marker stays at five minutes under every
policy, sub-agents keep both markers short, and the request ledger's `ttl`
column records the policy per request so the soak is judged from the rows
(`docs/analysis/tools/cache-anchor-ttl-summary.py`,
`docs/analysis/cache-rewrite-anatomy-20260916.md`). The default is unchanged.

The release lane owes a gate only to the tree the sha changed: it diffs
against that half's last green install and skips the other gate, saying so on
the phase line.

## [1.3.90] — 2026-09-16

A Claude Code pane's 「대화」 is the same conversation on its wire: the toggle
resumes the pane's session as a streaming wire tab in the pane's seat (the
pane's CLI is told `/exit` once the wire is up), and the wire tab's 「화면」
launches the CLI's own screen back on the same session. A pane mid-turn keeps
its transcript view and is handed over when the turn ends.

A pane's conversation view breathes only while its turn is out — the status
line and the live dots follow the hook's state.

zo: every model switch now files the plan scorer's shadow row tagged with the
door it came through — the quota and refusal fallbacks, the overload demotion,
a deep-gate leg's client swap and `/model` reach a switch observer, and a
forced switch is scored against the models left standing (the model behind a
quota wall, and its provider, drop out of the candidate set). The A→B→A
record is a reader over the request ledger (`cache-return-summary.py`), not a
new column: everything it needs was already on the rows. Nothing decides
differently yet; the rows are the soak's evidence.

zo: the three places a policy branched on a model's family by string are
catalog facts now — a lineup's `refusal_fallback` row names what a
safety-classifier refusal retries on, the fan-out decomposition model is the
provider's Fast tier for every provider, and the cold-start specialty seed is
`priors.specialty_seed`. The literal gate refuses a policy that branches on a
family word, with the grammar seeds allowed.

## [1.3.89] — 2026-09-16

zo panes report to the window again. The window hands its children the
bridge's endpoint file rather than the hook token itself, and zo's own
reporter demanded the token from its environment — so a zo pane posted no
hook at all: no session, no transcript, an empty conversation view. The
reporter now takes the endpoint file as its launch coordinates when the
environment carries no token.

A pane's conversation view reads on the pane's own hooks: the tool call and
the answer appear the moment they are written, not on the next 1 s tick.

The release script's guidance names the six release files and forbids
`-am` — a shared checkout's uncommitted work must not ride a release.

## [1.3.88] — 2026-09-16

A wire session's page streams what the agent is saying: the words arrive
delta by delta under the last turn — a thought as an open fold, the answer as
its own row — and close into turns the moment the message does. The session
wakes the page each time it has something new (`wire:update`; live deltas at
most every 33 ms, everything else at once), so the poll no longer paces the
words; its tick stays as the floor.

Claude Code itself can now be driven on its wire: 「화면 없이 선으로 열기 →
Claude Code」 runs `claude -p` fed and read as stream-json — the same partial
messages its own panel streams from — and every permission question
(AskUserQuestion included) stands in the conversation as a card whose answer
goes back on stdio: allow, allow for the session with the CLI's own rule
suggestions, or deny. The mode chip cycles the permission modes the CLI lists
in its own `--help`; the model menu lists the catalog's models for it.

Antigravity's wire door is gone until `agy` has a print mode that can ask: it
has no `--acp`, and its stream-json mode denies every tool without a person to
ask, so the door only ever failed.

zo: a main turn's verdict rows carry the shape the turn ran under — the host
deposits it by attempt when it decides the prelude, and the gate, the check
and the review recorders withdraw it.

## [1.3.87] — 2026-09-16

The prompt cache remembers what every model still holds, not only the last
one. Until now a session that moved from one model to another kept a single
"previous request" slot, so the first model's warm prefix vanished from the
record the moment the second model answered — a return to it, or a comparison
between the two, had nothing to read. The session state now carries a
per-model table (prefix tokens, when it was observed, the TTL its markers
asked for), carried forward on every request on both provider paths, and the
plan scorer's shadow prices a stay against a switch with it: staying pays
only the growth since the last hit, switching pays the rewrite.

## [1.3.86] — 2026-09-16

The price of a token is one table both trees read. The window's price table
(`model-prices.json` and its lookup) moves out whole into a leaf crate,
`crates/model-prices`; the window calls the same functions through it, and zo
now reads the same table (`api::model_price`) — Anthropic rows through their
cache multipliers, OpenAI rows through their cached-input rate, a model the
table does not name priced as unknown, never as free. The plan scorer's
shadow row therefore carries dollars: every candidate the table names is
priced, and its ranking by cost is real instead of a fallback on time and
tokens.

## [1.3.85] — 2026-09-16

zo learns to price a turn's plan before it decides one. Four decisions that
used to be made in four places — which model, at what effort, alone or split,
checked how — now stand side by side as candidates in a plan scorer
(`runtime::model_router::plan`) that sums what deciding costs, what the work
costs, what a model switch rewrites, what splitting and merging costs, and
what verifying and retrying costs, in tokens and in time. Staying on the
current model is always a candidate; another model has to beat it by a margin
its own verified record has earned; a person's pin is a constraint, never a
score; an unpriced model is never treated as free. For now the scorer runs
in shadow: every turn files what it would have chosen beside what the turn
ran, to `state/smart-router/plan-shadow.jsonl`, and changes nothing
(`smart.plan.shadow`, `minPassPercent`, `switchMarginPercent`).

One key joins the four ledgers. Every request row, timing row, outcome row
and plan-shadow row now names the attempt it was spent on — a main turn as
`<session>@<turn>`, a spawned agent as `<agent>#<generation>` — so what one
attempt cost, in time and in tokens, and whether it was verified, can be
read from disk (`docs/analysis/tools/attempt-join.py`). Request rows also
carry the effort they ran at and the output they produced; a spawned agent's
requests land on disk at all (its client had no recorder before). A gate's
exit code — `/goal`'s gate, the deep gate's check command — is written down
as an objective verdict, and the cost of deciding (the routing probe, the
fan-out decomposition) is a `classify` row of its own, outside the learning
sample and the accuracy report.

The outcome record names the plan shape the work ran under (`solo`,
`host-prelude:<w>`, `delegate`, `parallel:<w>`) and the attempt it served,
and the catalog carries the scorer's cold-start figures as priors — measured
ones cited, estimates marked.

v1.3.84 never installed: its zo gate went red on pedantic lints in the plan
scorer's first commit; those are fixed here.

## [1.3.84] — 2026-09-16

A CLI that can be driven without its screen opens as a conversation tab:
the agent chip's menu offers 「화면 없이 선으로 열기 (GUI 세션)」 for Codex,
which the window then runs as `codex app-server` over JSON-RPC — its turns
arrive as rows, an edit as its diff, and every question it has (a command,
a file change, a permission, a multiple-choice question) stands in the
conversation as a card whose buttons are the choices Codex itself offers,
answered without a terminal. The composer sends down the wire, the model
menu lists the wire's models, the palette names the session.

Antigravity's `/` palette now lists the commands `agy --print /help` prints —
the previous list had been read from a different program (Gemini CLI).

zo: a spawn its dispatch policy declines (a small task kept inline, a model
reserved for orchestration) is headed 「Spawn declined — kept inline」 instead
of 「Agent spawn failed」 — a verdict, carried as its own kind end to end.

v1.3.83 never installed as an app: the composer script was loaded by the
page and named by neither ship list; both name it now, and the shell's own
tests refuse a page reference the build would not ship.

## [1.3.83] — 2026-09-15

A CLI that can be driven without its screen opens as a conversation tab:
the agent chip's menu offers 「화면 없이 선으로 열기 (GUI 세션)」 for Codex,
which the window then runs as `codex app-server` over JSON-RPC — its turns
arrive as rows, an edit as its diff, and every question it has (a command,
a file change, a permission, a multiple-choice question) stands in the
conversation as a card whose buttons are the choices Codex itself offers,
answered without a terminal. The composer sends down the wire, the model
menu lists the wire's models, the palette names the session.

Antigravity's `/` palette now lists the commands `agy --print /help` prints —
the previous list had been read from a different program (Gemini CLI).

zo: a spawn its dispatch policy declines (a small task kept inline, a model
reserved for orchestration) is headed 「Spawn declined — kept inline」 instead
of 「Agent spawn failed」 — a verdict, not a fault.

## [1.3.82] — 2026-09-15

An edit's row in the conversation view wears the diff cut from the tool
call's own input — an Edit's old and new strings, a Write's whole content, a
Codex patch — drawn with the review surface's rows and word marks. A snippet
keeps its gutters blank (the call never said where in the file it landed); a
whole file counts from 1. The line-diff algorithm zo's tools use moved into
the shared core, so the window and zo draw one diff.

A question the hook describes — a permission request with its tool, summary
and the edit it asks about, or a multiple-choice question — now stands in
the conversation as the extension's card, above the composer, with allow ·
deny · 「대신 지시하기」 (decline, and the composer takes the words). A
question the hook only signalled still brings the terminal screen back.

Also carries everything v1.3.81 announced: that build never installed — its
lane stopped on a unit test still naming the previous page grammar, fixed
here.

## [1.3.81] — 2026-09-15

A terminal pane running an agent can wear its conversation instead of its
screen. The title bar's `>_ 터미널 · ◐ 대화` toggle swaps the two in the same
slot with a 220 ms fade, the conversation fed from the pane's own transcript;
a question the program asks on its screen brings the screen back on its own.

The conversation reads as the Claude Code extension's panel does: the person's
words in a left bubble, the agent's prose behind a quiet dot, every tool call
its own `● name target` row with `└ first line of the result` under it — the
dot on the agent's accent while the call is out, green when the result is in,
the halt ink when it failed — a thought folded behind its own heading, and
`✻ Pondering…` under the transcript while the run is out, in the CLI's own
mark and word. Only the accent changes per agent (Claude Code, Codex, zo,
Gemini); every other CLI wears the window's one accent and mark.

Under the box, the extension's chips: the agent chip (mark · name · model)
opens the model menu — the models that CLI really drives, from the live model
catalog, a pick sent down the CLI's own road (`/model <id>`, or the bare
command and the screen back where the CLI only opens its picker) — and the
other installed CLIs, each opening a fresh pane; the permission-mode chip
shows the mode the hook reported and cycles it with the CLI's own key. Typing
`/` opens the palette the CLI itself would show: for Claude Code the commands
this installation announces for this checkout (built-ins, skills, plugins,
MCP prompts), for zo its own `commands --json`, for Gemini CLI the installed
bundle's, and for Codex a documented list named as such, with the installed
version on the palette's head.

Two fixes a person hit today: a tab's agent mark no longer stays a letter
when the pane was painted before the catalog answered, and a terminal's
leftover width and height are split onto both edges in whole pixels, so a
full-screen program ends the same distance from every side instead of
leaving a ragged strip at the right and the bottom.

## [1.3.80] — 2026-09-15

Scrolling through a terminal's history is a shift of the rows, not a new
screen. Every wheel notch used to resend and repaint the whole window — 12.8 ms
a notch at 60×220 in the harness, the stutter a person feels next to
Terminal.app on a smaller machine — and now the grid answers a quiet move with
how far the view moved and the rows it exposed, while the window moves its row
nodes and paints the edge: 0.5 ms a notch. A move beside new output, over a
collapsed fold, on the alternate screen or longer than the screen stays the
absolute frame it was.

The agent relations board files a worker whose worktree was removed after its
work landed under its project, by the checkout's location, instead of out of
scope — the five second-wave workers had sat under 범위 밖 while the sidebar
showed none of them.

## [1.3.79] — 2026-09-15

The DMG carries a sealed app. v1.3.78's DMG shipped the linker's adhoc
signature over unsealed resources — Tauri signs only with a distribution identity
in the env — and another Mac's Gatekeeper called it "damaged" under quarantine.
The release lane now signs the bundle inside-out with the install phase's signer
when no distribution identity stands, packs the archive and the DMG again from the
sealed app, and refuses to gather anything that does not verify `--deep --strict`.
A locally signed app still meets the "unverified developer" dialog once; only a
Developer ID signature with notarization removes it.

The built-in browser's door names the element a person could see. A page that keeps
a hidden twin of its form puts the twin first in the document; `wait` timed out
with the control on screen, and click and type would have taken the copy nobody
sees. One picker walks every match for the first visible one, in every verb, and
the page scripts are proven in Chromium against hidden twins.

## [1.3.78] — 2026-09-15

The agent relations board fits its frame: an automatic fit now takes the zoom
step below its ratio, never the one above. The release lane had measured the
difference — a ratio of about 0.845 rounded up to 0.85 drew a 1074px picture
4px past a frame it had fitted at 0.84 the run before, and the second pass
landed on the same step. The board's fit test waits for the file panel's fold
to finish instead of a 120ms timer ten milliseconds longer than the transition.

_v1.3.77 was tagged and never installed: its lane went red on this rounding.
Its notes below ship in this build._

## [1.3.77] — 2026-09-15

The project nav bar reads by lineage. A workspace leads with the task the ledger
seated in it (the title a person reads, ahead of a branch named after a task id)
and says a branch that is its own name only once, keeping `← main` as the small
note of where it was cut from; the original words wait in the tooltip. An agent at
work reads in two lines — the task with its status and clock, then agent · model ·
what it is doing now — while a resting agent stays one line, so the list keeps its
density. The status is a short word in the same column on every row, and a turn
that ended keeps a quiet check until the ledger has verified, merged or deployed
the work — only then does it turn green. Ownership is a rail and an indent rather
than a frame around the card, and running agents wear a subtle activity border.

Two fixes ride along. The built-in browser loads the address a person types: Chromium's
HTTPS upgrade used to rewrite an intranet `http://` admin to https and time out on a
port the firewall drops, so the page never loaded — the upgrade is off by name. And the
terminal's quick-command menu now lists every command the settings pane does: one saved
for another project, or for a checkout since reclaimed, stands under that project's
name instead of vanishing from every menu.

## [1.3.76] — 2026-09-15

The Flow engine, second wave — one release for everything since v1.3.74. The
first wave's live measurement (450 APM replay, 131 ms a step, `evidence --verify`
in 31 ms) showed four defects and they are fixed: a click's `--text` is the
screen's word, kept in the log so a saved walk replays; browser steps outside an
automation run land in the session folder; a Flow at `evidence: full` keeps the
helper's own picture for every step (the walk's flags follow its evidence level,
the writer frames a step by the PNG its answer exported — 20 of 20 frames where
the desk had kept 1); and a walk cut short judges its lines at no wait.

Then the operator's half. Money moves under engine-level contracts
(docs/design/flow-engine-guarded-money-path.md): the transaction is the caller's
parameters (`money: id= amount= recipient=`), never a screen's; one step carries
`— money`; a `dry` Flow rehearses up to it and stops; a `guarded` Flow needs a
confirmation bound to the transaction (`--confirm <txn>`, or `auto` under a cap),
asks the page to show the amount and the recipient at no wait, and writes one
ledger line under a lock before the hand moves — the same transaction never moves
twice. A `## Trigger` line starts each round of `recipe-run --repeat --until`
when it flips past the last round's stand; `--arena` rehearses a document against
a recorded evidence folder without touching a desk. The emulator is a recipe
door like the browser (`zerocode-emulator` lines are saved, replayed, framed and
bound by package), the browser door numbers what a person could hit (`marks`,
`click --mark N`, `screenshot --marks`), and the window's computer-use settings
carry a Flow card that reads each document's policy, evidence level, fingerprint
and last verdict, and rewrites the two Flow sections in place.

_v1.3.75 was tagged and never installed: its lane run was folded into this one._

## [1.3.75] — 2026-09-15

The Flow first wave measured live on the installed build, and the two defects the desk showed
fixed. A saved walk of text-targeted clicks now replays: a click's `--text` is the screen's word
(the control named by what it reads), kept in the evidence log beside a check's needle, while what
the hand types stays a length — so `recipe-save` no longer turns a press into a parameter and the
walk no longer stops at its first step. Browser steps in a pane outside an automation run land in
the window's session folder beside its computer steps, so `recipe-save` keeps them and `evidence`
shows the `--value-stdin` line without its value. The measurement itself (docs/analysis/
computer-bench.md): replay 450 APM at 131 ms a step, 10/10 verdicts, no false fingerprint stops,
the environment binding stopping after one act, `evidence --verify` reproducing the report in 31 ms. The knowledge view's design touches (22c64040) ride along.

## [1.3.74] — 2026-09-15

The Flow engine, first wave: a recipe document gains a `## Flow` section that turns a
saved walk into a checkable QA flow — judged against a baseline, bound to an identity
fingerprint (an act aimed outside its recorded hosts and apps is refused), and replayed
with a `guarded` policy that parses but is refused until its money contracts exist. Every
walked step now reports its act/settle/verify/pointer time and names its evidence line;
the one evidence writer seals each run into a self-contained `report.html` with SVG rings
and numbered badges over the recorded rectangles, and `evidence --verify` reproduces the
report and re-judges the verdict from the folder alone. Browser steps join recipes and
replay through the browser door; browser `type` never writes a password field with its
keys, and `--value-stdin` is the setter-only road in. The bench tally reads a walk's
stage timings and scales the median step against the 200-APM human floor. Design,
adversarial review (23 findings, 9 P0), and the implementation plan ship alongside.

## [1.3.73] — 2026-09-14

_since v1.3.72 (2 commits)_

### fix
- fix(knowledge): ease the primary page action hover

### style
- style(knowledge): give ring labels and relation details room to breathe

## [1.3.72] — 2026-09-14

_since v1.3.71 (2 commits)_

### 지식 그래프 — 승인된 시안(`docs/design/knowledge-graph-20260914/prototype.html`)의 형태로
- **첫 방문은 주변 탐색.** 331쪽 볼트의 전체 지도는 첫 화면으로 읽히지 않았다. 처음 여는 볼트는 가장 최근에 회상된 페이지(없으면 색인이 아닌 최다 연결 페이지)의 주변 탐색으로 서고, 이후는 그 볼트의 마지막 모드다. 모드 표의 `first` 행이 정하고, 노드 수는 규칙 어디에도 없다.
- **주변 탐색은 중심을 도는 고리.** 전엔 전체 지도의 좌표를 그대로 써서 이웃이 지도 위 제자리에 흩어지고, 폭만 맞춘 카메라 탓에 세로로 넘쳤다(1009×825 판에서 이웃이 y 942·992·−179). 이제 깊이 k의 점은 k번째 고리에 서고(첫 고리는 군집 순, 바깥은 어버이의 각도 순), 카메라는 두 축을 다 담는다. 전체 지도의 좌표는 따로 들고 있다가 돌아갈 때 그대로 돌아오며, 주변 탐색 중에도 전체 지도는 뒤에서 앉는다.
- **머리는 다섯.** 모드 세그먼트·자라는 검색·「목차·일지 표시」·상태·「보기」. 렌즈·태그·시간 슬라이서·저장 시야·다시 읽기는 「보기」 팝오버 뒤에 있다(모든 티어).
- **캔버스.** 상자 없는 빵부스러기 아래 「연결 깊이 1단계 2단계 3단계 · 페이지 n」(카드에서 옮김), 왼쪽 아래 군집 점 범례(누르면 밝힘), 오른쪽 아래 「− 100% + ⌗ ?」 — 점 모양·선 종류 범례는 「?」 뒤에 접혔다.
- **차분한 옷.** 주변 탐색에서 선은 가늘고 옅으며 중심의 바퀴살만 또렷하다. 점은 군집 색으로 평평하고, 중심은 크고 후광을 두르며, 이름표는 전부 선다. 수치는 전부 토큰이다.
- 지식 그래프 36/36·창 스위트 1,241/1,241·반응형 워크벤치·shell 1,513·소스 계약 470 초록.

## [1.3.71] — 2026-09-14

_since v1.3.70 (1 commits)_

### 창 — v1.3.68부터 main을 빨갛게 하던 창 하네스 둘 (t-4140 회귀)
- **연결 워크벤치 크래시.** 판이 없는 워크벤치 항목(지식·워크스페이스 항목, 원장만 아는 워커)의 카드에서 `agentCardTerm`이 `card.pane.startsWith`로 죽었다 — t-4140 S2가 `workbenchRelatedActions → taskBoardSeatOf`로 부르기 시작한 길. 판이 없으면 판 번호도 없다(`sub:` 판과 같은 답).
- **320·360px 잎에서 지식 머리 가로 넘침(400px).** 상태 문구(`페이지 72/72 · 링크 124 · 군집 …`, 107px)가 줄어들지 않는 `flex: none` 벌이라 검색 칸을 0으로 누르고 머리를 넘치게 했다. compact·tiny 티어에서는 낭독기에게만 선다 — 시트의 `.sr` 규칙 하나를 함께 쓴다(복사 없음). 창 스위트 1,241/1,241·지식 그래프 35/35·shell 470 초록.
- v1.3.68·v1.3.69·v1.3.70은 설치되지 못했다(v1.3.68 root 게이트 빨강, v1.3.69는 큐 접힘, v1.3.70 같은 빨강). 이 판이 세 버전의 내용을 함께 싣는다.

## [1.3.70] — 2026-09-14

_since v1.3.69 (1 commits)_

### zo — 모델 등급 분류기: 프로바이더마다 카탈로그 신호로 등급을 매기고, 구현은 난이도 사다리를 걷는다
- **규칙.** 각 프로바이더의 가장 유능한 모델(fable·astra)은 계획·검증·어려운 승격만, 그다음(opus·sol — 각자 둘째라서 동급)은 어려운 구현, 나머지는 보통·쉬운 일이며 쉬운 일은 현재 릴리즈의 최저가(luna·sonnet·flash). 이 등급을 사람이 목록으로 쓰지 않는다 — **분류기가 알아차린다**(`runtime::model_router::tiering`): 같은 계보의 옛 릴리즈는 「대체됨」으로 걸러 어떤 자리에도 서지 않고(`claude-fable-5` 옆의 `-5-1`, `opus-4-8` 옆의 `opus-5`), 나머지는 Deep 티어(선언 frontier 또는 Ultra 천장)·effort 천장·플래그십 토큰(prior 표)·릴리즈 순위·class 순으로 줄 세워 1등급(Deep일 때 Top)·2등급·나머지를 매긴다. 새 모델이 카탈로그에 뜨면 다음 인벤토리에서 자리가 바뀐다(astra 발견 → astra Top, sol Second).
- **라우팅(Architect; Classic은 예전 그대로).** Coding/Debugging은 난이도로 rung을 걷는다 — Trivial/Small → Easy·Medium·Hard(위험도 High/Critical은 쉬운 rung에서 시작하지 않음), Medium → Medium·Hard·Easy, Large → Hard·Medium; 실패 1회는 한 단 위, 2회는 1등급을 연다. 구현 레인(스폰·워크플로 단계·Architect EXEC)은 같은 프로바이더·Top 아님·대체됨 아님. 검증 풀은 Top 밴드만 — 2등급은 제 위가 접속돼 있는 한 검증하지 않는다. EXEC 구현자는 턴의 난이도를 따른다: astra Large → sol, Small → luna; fable Large → opus, Small → sonnet.
- **난이도 판정.** `autoClassifier` 미설정은 이제 `probed` — 스폰 난이도는 Fast 모델의 판정을 키워드 바닥 위에 융합한 값. 명시값은 그대로. 스폰마다 프로브를 부르는 첫 토큰 비용은 아직 안 쟀다(A/B 남음).
- **시험이 핀 것.** 세 프로바이더+옛 저가 릴리즈+이름 추측 Deep+발견된 새 플래그십 인벤토리의 밴드·rung; 새 플래그십이 Top을 가져가고 옛 Top이 Second로; 계보의 옛 릴리즈는 대체됨; astra·fable·flash 메인의 난이도·위험·실패별 사다리; 2회 실패의 Top 개방; 턴 난이도별 EXEC 구현자; 분류기 기본. runtime 2,211·tools 1,401 초록, clippy·fmt 깨끗.

## [1.3.69] — 2026-09-14

_since v1.3.68 (1 commits)_

### zo — Architect의 검증·설계 풀은 접속 인벤토리에서 매 턴 계산한다
- **무엇이 틀렸었나.** VERIFY/PLAN 후보가 `smart.deepTierModels` 목록 **순서**였다. 목록 `[opus, openai-latest, fable, google-latest]`에서 astra 세션의 검증은 첫 줄 opus, terra 세션의 설계도 opus, `google-latest`(지금은 flash 릴리즈)가 「deep」 풀에 있었다. 구현 제외는 카탈로그 예약 별칭을 따라 움직였는데 검증 풀만 정적이었다 — 「최상위」가 둘이었다. 실측(route-outcomes 30일): deep-verify 레그 45건이 sol·opus 메인 아래서 opus-5·sol로 돌았다.
- **지금.** `runtime::dynamic_deep_tier_models`가 접속 인벤토리의 Deep 티어 중 근거 있는 출처(프로바이더 선언·Ultra 천장; 이름 토큰 추측 `…-pro`는 더 나은 것이 없을 때만)를 카탈로그 `orchestration_rank`(별칭이 옮겨가면 따라감) → effort 천장 → 릴리즈 순으로 늘어놓는다. `deepTierModels`는 그 풀을 **좁히는 필터**일 뿐이다 — 풀 밖 항목은 버리고 stderr에 한 번 말하며, 남는 게 메인뿐이면 동적 풀 전체를 쓰고, 메인이 Deep이면 목록과 무관하게 풀에 든다. Deep이 하나도 없으면 카탈로그 예약 집합. 설정 스냅샷 하나·프로바이더 프로브 하나로 quota·VERIFY·PLAN·EXEC를 다 푼다(전엔 프로브 셋).
- **시험이 핀 것.** astra 메인: 검증 fable, 설계 없음(스스로), 구현 terra. fable 메인(교차 금지): 대역 검증자 없음 — opus가 제 등급 위를 검증하지 않는다. terra 메인: 설계 fable. 목록이 접속 최상위를 하나도 안 가리키면 동적 풀. 시험 8건 red→green, runtime 2,206·tools 1,400 초록.

## [1.3.68] — 2026-09-14

두 시안의 **형태**를 제품 화면으로. 색 토큰과 글꼴은 앱 것 그대로, 배치·비율·위계는 시안을 따른다.

### 창 — 지식 그래프: 전체 지도 | 주변 탐색 (t-4140)
- **툴바의 「전체 지도 | 주변 탐색」.** 주변 탐색은 선택 노드와 깊이(1·2·3, 표 `KNOWLEDGE_FOCUS_DEPTHS`)의 하위 그래프만 DOM에 그린다 — 흐리기가 아니라 빠짐. 전체 지도의 좌표·군집·카메라는 보존되어 돌아오면 그대로다. 빵부스러기와 ← 이전(방문 이력, 상한 표 `KNOWLEDGE_EXPLORE`). 차수 12 이상의 허브는 관계 종류별 묶음으로 접히고(+N) 펼치기는 명시적. 실측(1,000쪽·2,513선, 3회): 전체 1000/2513 → 깊이 1 10/16(전환 7.5~9.2 ms + 프레임 5~10 ms) → 깊이 2 26/66 → 복귀 30~32 ms + 58 ms(점 1,000개 되붙음); 첫 그리기 67~77 ms(기준 80), 기존 METRIC 불변.
- **진입 규칙.** 첫 방문은 전체 지도, 워크벤치의 「연결 보기」로 들어오면 그 페이지의 주변 탐색, 이후는 볼트별 마지막 자리(설정 문서 `second_brain_explore`, 장면 옆 한 줄; 400 ms 디바운스). 노드 수로 자동 전환하지 않는다 — 순수 함수 `knowledgeStartMode`에 소스 계약.
- **검색 후보 목록.** 입력 중 제목·폴더·관계 힌트가 있는 후보(상한 8), ↑↓·Enter·Esc, 줄마다 「주변 탐색으로」. 기존 토큰 자동완성·저장 문구는 그대로.
- **오른쪽 패널은 두 탭.** 페이지(개요 또는 카드: 제목·요약·들어오는/나가는 관계(모양 표식+방향+낱말)·원문 열기)와 활동(BUS LOG). 「목차·일지 표시」 설정(기본 켜짐)은 그림에서만 뺀다 — 관계에서 자동 제외하지 않는다.
- **연결 만들기.** 「연결 추가」 → 대상 검색 → 유형·방향 → 미리보기 → 저장, 토스트 되돌리기·⌘Z·Alt+드래그. 저장은 새 Rust 문 `second_brain_relate` 하나(`zerocode_core::second_brain_relate`): `wiki/` 페이지의 frontmatter 관계 키 다섯(`related`·`implements`·`depends_on`·`supersedes`·`contradicts`)에 항목 하나를 넣거나 빼며 파일은 통째 교체(temp 옆에 두고 rename), 나머지 줄은 바이트 그대로. `raw/`는 쓰지 않는다. 되돌리기는 같은 문의 `remove`.
- **접근성.** 노드·후보·관계 목록을 키보드로, 콤보박스 aria, 포커스 링, reduced-motion. Fable xhigh 워커, 커밋 16(조각마다 red → feat).

### 창 — 작업 상황판 「관계」 보기를 승인된 시안의 형태로 (t-4145)
- 시안(`docs/design/agent-relations-preview-20260914`)의 배치를 그대로: 작업 레일(넓은 판 172px·서랍 155px·목록 티어 아래 접힘, 토큰 둘), 머리(eyebrow·제목·통계), 툴바(범위·검색·상세도), 캔버스 머리/바닥(범례·손짓 힌트·확대), 인스펙터(글리프 타일=노드 아이콘 타일, 관계/활동/상세 탭). 동작·온톨로지·티어 표(side 1100·list 700·tight 400)는 불변 — 시안의 950px 문턱은 옮기지 않았다(2레인 그림이 넘친다). 실측(1280×860, 3회 중앙값): 첫 판 47.6→51.3 ms, 선택 30회 33.5→33.5, 범위 왕복 166→166, 티어 왕복 89.9→106.2; **형태의 비용**: 1280 창에서 레일 때문에 자동 맞춤 1.0→0.82(그림 판 808→636px). 첫 시도의 `.file-view:has(…) > *`는 선택 30회를 41→158 ms로 만들어 마크업 클래스(`.agent-board`)로 회복(함정 248). Fable xhigh 워커, 커밋 4.

### 게이트
- t-4145 코디네이터 게이트: agent-relations 30/30·adversarial 28/28·창 스위트 993/993·settings 117/117·contracts 468. t-4140 코디네이터 게이트(관계 보드 위 rebase 충돌 없음): 그래프 시험 35/35 ×2·창 스위트(window+knowledge-live) 965/965·settings 117/117·fmt·clippy(core·shell all-targets)·core 1440·shell bins 1513·contracts 470.

## [1.3.67] — 2026-09-14

main 전체를 한 번에 싣는 판. 1.3.66(브라우저 file:// 탭 복원 기록)은 게이트 중 이 판에 접혀 설치되지 않았다 — 내용은 그대로 실린다.

### 창 — 에이전트 관계 워크벤치 (다른 세션, 412b1798·2bc461f3)
- 작업 상황판이 실제 실행 범위와 관계의 근거를 검사한다(`feat(agent-board)`), 범위별 접힘 상태를 유지하고 실행 좌석을 맞춘다, 완료된 시도와 살아 있는 인스펙터 능력을 보존한다, 라이브 갱신 중 응답 작성 위치가 고정된다. 닫힌 시도는 보드 행에 과업·dispatch 신원을 유지한다(t-4048 Fable 적대 재현). 시험: 관계 시험이 창 실행기 묶음으로 게이트에 들고, 완료 카드 분류·유지된 인스펙터 카드의 상태 갱신·Fable 적대 케이스가 픽스처에 남는다. 승인된 관계 미리보기 문서.

### 릴리즈 레인 (t-4004 · t-4123)
- **gate-zo 674→1208 s의 원인을 숫자로 갈랐다.** 증가 = b931d803이 zo `test`를 `-p zo-ide`→`--workspace`로 넓힘(시험 1,143→6,215, test 컴파일 74→431~447 s). 증폭 = launchd plist에 `ProcessType`이 없어 CPU·I/O 스로틀(rustc 우선순위 20): test 컴파일 launchd 기본 중앙 598 s vs `Interactive` 243 s(n=2) vs 터미널 222 s. 고침: `install-launchd.sh` plist에 `ProcessType=Interactive` 한 줄(재설치로 적용), `verify-every-recipe.sh`가 레시피마다 `<== verify recipe <name> rc=<n> <secs>s`를 남긴다(레인 파서 불변, 실제 출력으로 시험). 「버전 범프 재컴파일」 가설은 기각 — 새 클론 mtime 때문에 매 게이트 전부 재컴파일되는 것은 674 s 시절도 같았다(후속 후보). opus 워커.
- **레인이 큐를 접는다.** 큐에 후손 sha가 있으면 조상은 `superseded`(꺼낼 때·gate-root·gate-zo·flakes 앞; push 뒤엔 끝까지 빌드). `status.sh --wait`는 끝난 sha를 제 `out/lane-<sha8>.log` 끝줄로 답한다(superseded 5) — 레인이 넘어간 뒤 영영 걸리던 구멍. 드라이런 5케이스, `test_lane.py` 77/77. 오늘 64·65·66이 5분 간격으로 큐에 서서 사람이 손으로 접은 뒤의 고침.

### 게이트
- 1.3.66까지의 내용은 각 착지 때 코디네이터 게이트를 지났고(1.3.66: 창 하네스 1211/1211), 레인 tooling은 `test_lane.py` 77/77·bash -n·plist. 이 판 전체는 레인 gate-root/gate-zo가 판정한다.

## [1.3.66] — 2026-09-14

### 창 — 브라우저 탭 복원 기록 (t-4088)
- **file:// 탭은 복원 기록에 적지 않는다.** 아티팩트를 발행하면 그 주소는 앱 데이터 아래 `file://`이고(`artifact_publish.rs`), 파일 트리의 「ZeroCode 브라우저에서 열기」도 같은 스킴을 연다 — 주소 사다리는 `file:`을 일부러 허용한다. 그런데 탭 목록을 저장하는 문(`set_browser_open_tabs`)은 웹 주소와 빈 페이지만 받고, 걸린 행 하나로 **저장 전체**를 오류로 돌려 「브라우저 탭 주소를 저장할 수 없습니다: file:///…」 토스트가 탭이 바뀔 때마다 떴다(13:14, Computer Use 중 아티팩트 발행 직후). 그동안 다른 탭도 하나도 기억되지 않았다. 창은 이제 문이 받는 주소만 기록한다(`rememberedBrowserAddress`; 방문 기록이 인라인으로 쓰던 웹 주소 판정은 `isWebAddress` 하나로). 문의 규칙(설정 파일에 이 기계의 경로를 적지 않음)과 탭이 열리는 동작은 그대로다. red-first: 하네스 케이스 하나(https 탭 + file 탭 → 기록엔 https만) 1210/1211 → 1211/1211. 코디네이터 직접 수정.

### 게이트
- 창 하네스 전체(main 체크아웃) 1211/1211 · 소스 계약 `browserOpenTabRecords` 블록 조건(reader 철자·`mobile:` 없음) 정적 확인(레인 gate-root가 실행).

## [1.3.65] — 2026-09-14

### 창 — 하네스 실행기 하나·워커 묶음은 제 페이지 (t-4017)
- **창 하네스가 실행기 하나로 돈다.** `ui/tests/window.mjs`는 검사 약 1,200개가 한 흐름이었다 — `ok()`가 배열에 모으고 파일 끝에서 보고를 찍으니, 잡히지 않은 예외 하나가 뒤의 모든 줄과 보고 자체를 날렸고(2b641238), `*_ONLY` 블록 열두 개가 선택과 보고 루프를 각자 복사했다. 이제 `createRunner()`가 묶음을 이름으로 등록하고(explorer → 모듈 리그 → vault → workers → 공유 흐름 `window` → 이름으로만 닿는 셋), 선택은 `WINDOW_SUITES`·`--suite`·옛 `<NAME>_ONLY`(이름에서 유도, 표 없음)로, 던지는 묶음은 그 묶음을 이름한 FAIL 한 줄이 되고 나머지는 계속 돈다(첫 전체 실행에서 세션 teardown이 브라우저를 죽였는데 보고가 486/487로 나왔다 — 예전엔 아무것도 없었다). 없는 이름을 고르면 조용한 0/0이 아니라 FAIL. 보고 형식은 글자까지 같다(`PASS  name  — detail`, `N/M passed`, 종료 코드) — 레인과 코디네이터 감시는 그대로 읽는다. `just window-runner-test`(node 시험 아홉, ~100 ms)가 verify에 선다.
- **워커·헬퍼 시나리오가 제 페이지에 선다.** `workers.mjs` — 좌석 시험 여덟(seatedBack·liveSeatRecovered·sleepingReservation·restoredSplit·restoredBirth·workerFold·partedStage·strandedWorker)과 헬퍼 명부 시험 열(subRows·helperRowFocus·doneHelpers·foldedHelper·helperPaneLeft·helperDoing·helperRowPeek·laneRowPeek·doingNow·walkedOut), 26 검사가 공유 흐름의 것 그대로. 페이지가 묶음과 함께 죽으니 `workerFold`의 활성 경로 복원과 이웃을 위한 뒷정리가 사라졌다(v1.3.57·58의 위치 의존 빨강이 난 자리). 단정 하나만 모양이 바뀌었다: `restoredBirth`가 영어 리터럴로 비교하던 행의 낱말을 카탈로그의 `t()`로 읽는다. `window-boot.mjs`가 모든 페이지의 공통(`PRIMARY_EVENT`·하네스 손·`faults` 싱크·페이지마다 제 context — 페이지가 쥔 context는 axe의 `newPage()`를 거절하기 때문)을 든다.
- **단독 = 전체, 숫자로.** `window-suite-parity.mjs`가 묶음을 혼자 돌리고 전체를 돌려 같은 이름의 verdict를 대조하고 초를 찍는다. 실측(이 Mac, 레인 게이트 옆에서): 전체 1210/1210 전·후 같은 이름 1,210개; workers 단독 26/26 4 s; parity 26 검사 verdict 동일. 워커 교차 실측(같은 기계, 레인 게이트 옆, 각 3회): 전체 실행 전 422/424/424 s(중앙값 424)·후 435/423/424 s(중앙값 424) — 실행기가 시간을 더하지 않는다; workers 단독 4/4/4 s. Fable xhigh 워커 t-4017, 3커밋.

### 게이트
- 코디네이터 게이트(detached 워크트리, main 위 rebase 충돌 없음): window 전체 1210/1210(기준선 이름 집합과 대조 이름 1,210개 동일·차이 0), workers 단독 26/26 (4 s), runner node 시험 9/9, parity 26 검사 verdict 동일, settings 117/117, source contracts 468.

## [1.3.64] — 2026-09-14

### 창 — Windows 콘솔 억제 (AO #5309)
- **창이 자식을 띄우는 문이 하나가 됐다.** release는 `windows_subsystem = "windows"`라 창에 콘솔이 없고, 그 아래에서 `git`·`gh`·`security`·`adb`·`powershell` 같은 콘솔 자식을 맨 `Command::new`로 띄우면 Windows가 자식마다 검은 콘솔 창을 깜빡인다. `proc::quiet_command`(와 tokio 판)가 `CREATE_NO_WINDOW`를 다는 유일한 자리(상수 정의 1회)가 되어 출하 스폰 113곳과 시험 2곳이 그 문을 지난다. 예외는 `bin/zerocode-mirror.rs` 3곳 — 사람의 콘솔을 물려받아야 하는 콘솔 shim이라 그 플래그는 정확히 틀린 답. unix에서는 문이 `Command::new` 그 자체다(분기 없음). `system_fonts.rs`의 자체 `CREATE_NO_WINDOW` 블록은 사라졌다.
- **소스 계약이 되돌아감을 막는다.** `tests/source_contracts/quiet_children.rs`가 크레이트 `src` 전체를 걸어 읽어(손으로 든 목록 없음) 맨 `Command::new(`가 문 밖에 0곳, 플래그는 문에만, 문은 플래그만 얹음을 고정한다. 기존 계약 둘(벤더 CLI 경계·호스트 경계)은 스폰을 문의 철자로 센다. 동작 불변 시험 둘: Debug 비교(program·args·env·cwd, std·tokio)와 실제 실행(종료코드·stdout·env·cwd).
- **실측**: 생성 비용 debug 200k회×3 — `Command::new` 85~87 ns, `quiet_command` 83~85 ns(차이 없음). release 측정은 빌드가 두 번 SIGKILL로 끊겨 숫자 없음. opus 워커 t-3995, 7커밋. 슬라이스 밖에 남은 것: 창 프로세스 안에서 도는 다른 크레이트의 스폰(zerocode-core 9곳 — `host.rs`의 LocalHost git 포함 — ·zerocode-orchestrator 30곳)은 여전히 맨 `Command::new`라 Windows에서 깜빡일 수 있다(후속 과업).

### 창 — 지식 그래프 (사용자 세션, 2623fef2)
- **커지는 그래프가 노드 신원을 재사용하고 삭제된 좌표를 회수한다.** 페이지 하나가 늘 때 새로 만드는 SVG 노드 1,001개 → 1개, 삭제·교체 반복 뒤 보관 좌표 1,161개 → 40개(현재 페이지 수). 팬 처리 중앙값 1,000노드 1.7 → 0.1 ms(p95 2.9 → 0.4), 5,000노드 13.8 → 0.2 ms(p95 15.8 → 1.1). 첫 표시(93 → 92 ms)와 최장 프레임 간격(47 → 48 ms)은 변화 없음 — 그 개선은 주장하지 않는다. 그래프 전용 브라우저 검사 26/26(새 5개: 렌즈로 숨긴 페이지의 좌표 보존·실제 삭제 시 회수·제목 변경 뒤 선택/호버 유지 등), 창 하네스 1,209/1,209. 측정과 재현 명령은 `docs/design/knowledge-graph-20260914/measurements.md`. 같은 폴더에 전체/주변 탐색 시안(`prototype.html`, 제품 미적용).

### 게이트
- 코디네이터 게이트(detached 워크트리, main 2623fef2 위 rebase 충돌 없음): fmt·clippy(shell all-targets) 초록, shell `--bins` 1509, source contracts 468, core 1436, `just win-check` 초록(rc 0, 경고 106줄 = 워커 기준선과 동일, 오류 0, proc.rs를 이름하는 경고 없음). 워커 게이트도 전부 exit 0(win-check 2회).

## [1.3.63] — 2026-09-14

### 창 — 인증·준비 스냅샷 (AO AgentReadinessProvider 본뜸)
- **에이전트마다 「바이너리가 있나·로그인돼 있나」를 한 스냅샷으로 두고, 표시와 실행이 목적별 신선도로 그것을 읽는다.** `zerocode-core::readiness`(스냅샷·캐시·`Limits`, 오버레이 `readiness.display_fresh_ms`(5분)·`readiness.launch_fresh_ms`(30초)·`readiness.evidence_chars`, 바닥 1초)와 셸 `readiness_runtime`(문 셋: 소환의 `ensure`(Launch)·픽커의 `ensure_seen`(Display)·원장 `agent-list`의 `observe`(peek+배경 sweep 한 스레드)). 프로브는 능력 표의 `auth_probe` 열(첫 소비자)이 정하고 기존 검사(`accounts::signed_in`·`keychain_says`·`codex_accounts`·`gh::integration_status`)를 재사용한다 — 새 프로브 복제 없음, 오버레이 읽기는 `settings_runtime::u64_overlay` 하나를 checks와 나눠 쓴다. `unknown`은 판정이 아니다(증인이 못 본 것은 절대 「로그인 필요」로 접지 않음). 키체인은 이 창이 심은 관리 홈만 묻고 사람의 항목은 읽지 않는다. 실측: 캐시 적중 0 µs, 미스(파일 증인) 207~239 µs(20회 평균) — 런치 경로에 더한 지연 없음. 소스 계약 6개가 리터럴(`300_000`·`30_000`·`from_secs(`)과 이름 분기를 막는다. Fable xhigh 워커 t-3996, 4커밋.
- **로그아웃된 에이전트는 실행 전에 「로그인 필요」로 보인다.** 설정의 에이전트 행과 워크트리 픽커가 같은 필드로 말하고, 호버에 증인의 말(`openai: auth.json none (~/.codex)` 식)을 단다. `zerocode-orc agent-list`의 `readiness`(auth·binary·observedAtMs·ageMs·evidence) — 소환이 거절할 이유를 코디네이터가 먼저 읽는다. 워커의 마지막 커밋(하네스 시험·SKILL.md)은 창 재시작으로 워커 판이 죽어 코디네이터가 그대로 실어 게이트했다.

### 게이트
- 코디네이터 게이트(detached 워크트리, main 725cf370 위 rebase 충돌 없음): fmt·clippy(core·shell all-targets) 초록, core 1436, shell `--bins` 1506(codex_queue 형제 1회 흔들림 → solo ×3 초록, 플레이크 표에 있음), source contracts 465, settings 하네스 초록, UI 하네스 1210/1210(워커 시험을 페이지 낱말로 읽게 고친 뒤; 첫 실행 1209/1210은 영어 페이지 위 한국어 리터럴).

## [1.3.62] — 2026-09-14

### 창 — PR 피드백 루프 (AO scm-observer 본뜸, Apache-2.0)
- **에이전트의 체크아웃에 열린 PR을 창이 30초 박동으로 지켜보고, CI 실패·변경 요청 리뷰·병합 충돌·병합됨을 그 에이전트에게 한 번씩만 편지한다.** `zerocode-core::scm_observer`가 순수 모델(CI·리뷰·병합 가능성·종결 상태의 독립 의미 커서, 관측→영속→반응→수락한 편지만 ack, 헤드별 CI 예산과 리뷰 ID로 반복 넛지 차단, 해소 시 충돌 재무장, 종결 전이는 모든 알림 닫기), 셸이 시계·`gh` 프로세스·원자 영속·원장 배달을 맡는다. Checks 패널과 같은 durable book·같은 keyed 원장 ack를 쓴다. 관측 영수증이 원장에 결정적으로 남아 잃어버린 로컬 ack가 재시작 뒤 두 번째 편지 없이 재생된다. 포크 PR은 base 저장소 좌표로 읽는다. 대상당 `gh` 호출 최대 7(발견·CI 셋·주석·리뷰 GraphQL·스택 한 쪽), 틱당 32. astro 워커 t-3993, 4커밋(51e39578·17fa5c32·f68d6fa2·a83506a4).
- **PR 알림은 원인이 풀리면 스스로 닫힌다.** CI 실패·병합 충돌은 기존 붙박이 토스트를 재사용하고 `resolved_at`이 닫는다. 손으로 닫는 것은 표시만 바꾸고 사실을 ack하거나 재무장하지 않는다.
- **숫자는 전부 `Limits` 표에서.** `gh` 호출 시한(10초)이 마지막 리터럴이었다 — `checks.gh_call_ms`로 옮겨 설정 오버레이가 느린 네트워크에서 늘릴 수 있다(바닥은 `poll_ms_min`). 오버레이 키 전체는 `docs/design/pr-feedback-observer.md`.

### 릴리즈 레인
- **codex 사이드카 프로브 시험의 형제(`bound_queue_process_receives_the_fixed_pointer_and_minimal_environment`)를 알려진 부하 플레이크로 등재.** 부하 11~19에서 3회 중 1회 `Unknown`; 단독 3회 초록. 레인이 solo ×3으로 재판정한다.

### 게이트
- 코디네이터 게이트(detached 워크트리, main efb52099 위 rebase): fmt·clippy(core·shell all-targets) 초록, core 시험 초록, shell `--bins` 3회(플레이크 표의 넛지 시험 둘과 위 형제만 흔들림, 각 solo ×3 초록), source contracts 초록, UI 하네스 1209/1209.

## [1.3.61] — 2026-09-14

### 창 — 하네스 능력 표 (AO ports/agent.go 본뜸)
- **쓰기 문은 에이전트의 행에 묻고 이름을 말하지 않는다.** 에이전트마다의 제출 길·제출 확인·시작 신호와 quiet·steer·빈 컴포저 증명·막힘 신호·인증 프로브·resume·spawn·포인터 경로를 `zerocode-core` 능력 표 한 곳(`capabilities.rs`, `AgentSpec::harness`)이 답하고, 런치·resume·워커 split·자동화·mail 붙여넣기의 쓰기 문 22곳이 그 표를 읽는다. 소스 계약 셋이 표 밖의 에이전트 이름 분기를 막는다. 행동 변화는 없다(기존 시험 전부 초록, core +7·bins +1·contracts +3). 과거 P0 둘(prefill 미제출·백틱 실행)이 난 자리가 한 표로 모였다. Fable xhigh 워커 t-3994.

### 보드 — 자율 상태
- **일시 정지된 자율 목표와 사람의 주의가 필요한 상태를 가른다**(astro, 2b71b6c7): 보드·사이드바·CSS·autonomy-board 시험.

### 참고
- 1.3.60(zo tools 시험 핀)은 초록으로 설치됐다.

## [1.3.60] — 2026-09-14

### zo — 시험
- **슬롯 회수 시험이 벽시계 대신 사실을 기다린다.** 1.3.57 레인의 zo 게이트에서 tools 시험 둘이 함께 빨갰다. `reclaim_extension…`은 20 ms 마감과 150 ms 발행의 순서를 스케줄러에 맡겨 부하에서 완료가 먼저 도착하면 연장 횟수가 0으로 읽혔고, `readonly_refill_live…`는 그 패닉이 poison시킨 공유 env 락을 `unwrap()`하다 연쇄로 죽었다. 발행 스레드는 이제 회수 루프가 연장을 기록하는 cfg(test) 표식을 기다린 뒤 발행하고, 락은 나머지 90곳처럼 `into_inner`로 잡는다. 코어 수를 넘는 CPU 부하 아래 둘 ×15·tools 전체 1회 초록. 제품 코드 변경은 없다.

## [1.3.59] — 2026-09-14

### 창 — 시험
- **worker 접힘 시험이 같은 종류의 시험 옆에 선다.** 1.3.58의 시험 고침(활성 워크스페이스 복원)은 뒤의 카드 시험 셋을 살렸지만 그 사이의 헬퍼 행 시험 아홉 개를 깨뜨렸다 — 시험 파일 앞뒤가 서로 다른 상태 규약(카탈로그 카드 vs 픽스처 경로)을 전제한다. 시험을 이미 worker 자리 이벤트를 쓰는 초반 시험 둘(`restoredSplit`·`restoredBirth`) 바로 뒤로 옮겼다. 제품 코드 변경은 없다.

### 참고
- 1.3.57·1.3.58은 이 시험 배치 문제로 root 게이트가 빨갰고 발행되지 않았다. 이 판이 1.3.57의 내용(implement#0 행 클릭, astro의 보드 자율 동기화 5aed9838)을 그대로 실어 다시 건다.

## [1.3.58] — 2026-09-14

### 창 — 시험
- **worker 접힘 시험이 활성 워크스페이스를 되돌린다.** 1.3.57에 실린 새 하네스 시험이 worker 자리 이벤트로 카탈로그를 새로 읽으며 창의 활성 워크스페이스를 카탈로그 것으로 옮겨 놓았고, 뒤의 카드 시험 셋(워크스페이스 점·질문 바닥·전환 하이라이트)이 「이미 여기」 길에서 새 프로젝트 행을 못 그려 v1.3.57 레인의 root 게이트가 빨갰다. 시험이 들어올 때의 활성 경로를 나갈 때 복원한다. 제품 코드 변경은 없다.

### 참고
- 1.3.57의 변경 기록에 빠졌던 항목: **보드의 목표·루프 상태가 턴 상태와 따로 흐른다**(astro, 5aed9838). 목표/루프가 `running`이면 턴이 끝나도 카드는 Working, `paused`면 Attention이고 `pause_reason`이 실린다. 세션 상태 프레임은 훅 상태를 재방송하지 않고 `pane:autonomy` 이벤트로 온다(질문 판을 지우거나 두 번째 턴 종료를 만들던 부작용 제거). 사이드바 행·작업 보드에 자율 낱말(추구 중·다음 실행 대기·반복 완료·중단됨).
- 1.3.57은 root 게이트(위 시험 오염)로 발행되지 않았고, 이 판이 같은 내용에 시험 고침을 더해 다시 건다.

## [1.3.57] — 2026-09-14

### 창 — 사이드바 에이전트 행
- **worker 표면 헬퍼도 제 판에 접힌다.** 워크플로가 띄운 zo 팀원(`implement#0`)은 제 탭을 가진 worker 표면으로 오는데, 그 자리 이벤트(`term:worker`)가 `term:split`과 달리 헬퍼 id를 싣지 않아 명부 행이 부모 시계를 단 채 남고, 눌러도 부모 판으로 가서 아무 일도 없어 보였다(「에이전트가 클릭해도 안 보임」). 이제 자리 이벤트도 헬퍼 id를 싣고(재착석 포함), 판이 첫 훅을 말하기 전이라도 헬퍼 행의 클릭은 그 헬퍼의 판이 선 탭으로 간다.

## [1.3.56] — 2026-09-13

### 창 — 재시작 복원
- **한 대화는 한 번만 이어진다.** 저장된 기록 둘이 같은 zo 세션을 가리키면 재시작마다 resume이 두 번 걸렸다 — 둘째는 첫째의 writer lease에 막혀 채널을 못 열고(20 s 뒤 거절) 또는 두 런치가 키체인 쓰기에서 겹쳐(`security 종료 45`) 거절됐고, 그 자리에 **새 zo**가 떠서 사람은 「이어지지 않고 새로 시작」을 봤다. 이제 이미 서 있거나 뜨는 중인 대화를 가리킨 둘째 기록은 아무것도 들지 않은 평범한 셸로 열리고, 다음 저장부터 파일은 그 대화를 한 번만 이름한다.
- **거절된 wake는 새 에이전트가 아니다.** 이어서 열지 못한 대화의 자리는 그 기록을 든 평범한 셸이고, 토스트가 이유를 말한다. 새 에이전트의 첫 보고가 기록의 대화를 덮던 길이 닫혔다.
- **판 열기 거절이 즉시, 이유와 함께.** zo가 채널을 발행하기 전에 끝나면(lease 거절 등) 20 s를 기다리지 않고 종료 코드와 터미널 마지막 줄(`resume failed: … writer lease held by zo pid …`)로 거절한다.

### 계정 — 키체인
- **키체인 쓰기는 한 줄로 선다.** 같은 계정의 런치 둘이 delete→add를 겹치면 둘째 add가 `errSecDuplicateItem`(45)로 죽어 런치가 거절됐다. 이 프로세스의 쓰기는 직렬화되고, 중복이면 delete→add를 한 번 더 하며(삭제가 거절되면 그 말로 실패), 오류에는 `security`의 종료 코드와 stderr 첫 줄이 실린다(비밀은 실리지 않는다). `-U`는 쓰지 않는다 — 갱신은 옛 항목의 접근 목록을 남긴다.

### 오케스트레이션 (zo) — 다른 세션의 작업
- **읽기 전용 워커가 끝나면 슬롯을 바로 채운다**(9a39abaf, readonly-rolling 워커): 강제 ReadOnly·비격리에서만 보충, 같은 실행 세대의 물리 종료 확인, 예산 정산 대기, 지속 watchdog, 중단 표식·부분 결과 보존, 취소 뒤 후속 스폰 차단. tools 1,396 통과(3,279개 가상시간 스케줄 검증; 실측 성능 주장 아님). 1.3.55 레인의 zo clippy 빨강(aaab43cc의 too_many_lines·f64 비교)도 이 커밋이 고친다.

### 참고
- 1.3.55 레인은 root(사이드바 소스 계약 문장)·zo(clippy) 둘 다 빨강이라 발행되지 않았다. 이 판이 둘을 고쳐 다시 건다.

## [1.3.55] — 2026-09-13

### zo — 세션 lease
- **거절이 누가 쥐었는지 말한다.** lease를 잡는 zo가 잠금 파일(`<session>.jsonl.lock`)에 pid·cwd·시각을 찍고, 같은 세션을 열려던 zo는 `writer lease held by zo pid 96468 in /path, for 27 min`처럼 듣는다. 도장은 「누구」이지 「쥐었는가」가 아니라, 놓인 뒤 남은 도장은 holder로 치지 않는다.
- **`/resume` 목록이 미리 말한다.** 다른 zo가 쥔 세션 행에 `● open in another zo: …`가 붙는다 — 골라서 거절당하기 전에.

### 창 — 사이드바
- **에이전트가 도는 워크트리는 숨지 않는다.** 창이 만들지 않은 워크트리(external)는 숨김 축에 걸리지만, 그곳의 판에서 에이전트가 돌고 있으면 목록에 남는다. 「네비바에 안 보여서」 살아 있는 zo를 못 찾고 다른 판에서 같은 세션을 열려던 사건의 고침.

## [1.3.54] — 2026-09-13

### Computer Use — 배치가 계획 하나가 된다
- **클릭이 대상을 글로 부른다.** `click --app A --text <t> | --role <r> [--label <l>]`가 find와 같은 매처로 컨트롤을 부르고, 누르기 직전의 새 트리에서 고른다. 번호(요소 인덱스·마크)와 달리 낡지 않으므로 배치의 어느 걸음 뒤에도 쓸 수 있다 — 익숙한 계획(라벨로 클릭, 입력, 키, wait-for)을 한 호출에 통째로. 매치가 하나면 누르고, 여럿이면 낱말을 통째로 읽는 것("Save" ≠ "Save As…")이 하나일 때만 누른다. 그래도 여럿이면 후보 다섯을 이름해 `ambiguous_target`, 없으면 `element_not_found` — 누르기는 추측하지 않는다. 결제·이체·삭제 확인은 같은 선택의 라벨로 판정한다.
- **zo Computer 도구**: `left_click {app, label, role}`가 같은 클릭을 쓴다(클릭의 `text`는 수식키 그대로). 도구 설명은 「누를 때마다 보지 말고 익숙한 계획은 라벨로 통째로 배치하라」고 안내한다.
- **실측(설치본 1.3.52, 소유 픽스처, 이 세션의 모델이 고름)**: 걸음마다 보고 고르면 10/10·6.9 APM(모델 턴 8.4 s/걸음, 손 0.2 s), 한 번 보고 5걸음 배치면 5/5·22.3 APM. 손은 규칙 제어로 141~276 APM — 병목은 손이 아니라 왕복이다. 벤치에 `--input-path text` 레인(글로 부르는 클릭 배치)을 더했다. 근거 `tools/computer-bench/results-20260913-llm-batch.json`.

### 참고
- 1.3.53 뒤 다른 세션의 오케스트레이터 고침 셋도 실린다: store 동시 열기 안전(297012d1), 워크플로 데드라인·설정 복구 시험 앵커(1893e94b), WAL 시작 재시도 예산(b83f9bd2).

## [1.3.53] — 2026-09-13

### 창 — 작업 상황판 ↔ 지식 그래프
- **회상 추적이 작업과 지식 그래프를 잇는 간선이 됐다.** 작업 상황판의 참여자 상세에 「참고한 지식」이 선다 — 훅이 그 판의 프롬프트 앞에 끼운 페이지들(볼트 `.zerocode/recall.jsonl`)을 좌석(세션, 없으면 판) 기준으로 최신순 12개까지. 줄을 누르면 그래프가 그 점을 고르고 가운데로 간다. 「지식에서 검색」은 그대로 검색이다.
- **그래프에서 작업으로 가는 문.** BUS LOG의 회상 줄과 페이지 카드의 「이 페이지를 본 작업」이 그 페이지를 본 판을 이름한다. 지금 열린 판은 작업 상황판의 그 카드로 가는 단추이고, 닫힌 판은 이름만 남는다. 아티팩트 서랍의 「작업」 점프도 같은 문을 쓴다.

### 창 — 아티팩트
- **디자인 스튜디오에 닫기가 생겼다.** 비교·결정/대시보드/설명·소개/작은 앱을 열면 닫을 길이 「접기」(폼만)뿐이었고 타일 네 장은 같은 높이로 남았다. 오른쪽 위 X가 스튜디오 전체를 한 줄(눈썹·제목·「펼치기」)로 접고 갤러리가 그 높이를 받는다(하네스 229 → 58 px, 갤러리 366 → 537 px). 눌린 타일을 다시 누르면 폼이 닫히고, 접힘은 창 안에서 유지되는 사람의 선택이다. 쓰던 초안은 접었다 펼쳐도 남는다.
- **「초안을 받을 에이전트」가 에이전트의 도착·이탈을 따라온다.** 갤러리가 서 있는 동안 새 판에 에이전트가 앉아도 목록이 그대로였다(「Codex만 표시」). 이제 판의 에이전트가 바뀌는 순간(hook·명부 재적재·판 닫힘) 목록을 다시 그린다.

### 참고
- 1.3.52 뒤 다른 세션의 수정도 이 판에 실린다: 터미널 시험 격리와 워크플로 저장소 경주(db7defe6), 연속 관측의 스트림 기록 복구(32eacbe4), 편집기 초안·파일 권한 보존(b931d803), zo Computer 관측 재사용·프레임 보존(d5d24a2a), computer-bench 완료 기록·공유 시계(a15c7649), 아티팩트 초안 평문·URI 전역 인식(537b8651), 복구 계약 정본 경로(dd5a139e).

## [1.3.52] — 2026-09-13

- Computer Use: remove artificial input pacing, reuse fresh dispatch observations, validate matching helper policies, and protect whole action sequences while keeping stop and passive sensing available.
- Workbench: connect workspace/task/knowledge/artifact navigation, expose nested controls to accessibility, follow live stages by default, and preserve explicit manual stages and draft destinations.
- Orchestration: preserve execution contracts through verification, reject ignored task-update fields, and learn once per actual attempt with verified outcomes and correct model attribution.
- Artifacts: require creation evidence, preserve cross-session history during cleanup, use immutable versions for previews and feedback, and keep new-agent drafts unsent until requested.
- Browser: initialize document-start observation correctly and cover it in the standard verification gate.

## [1.3.51] — 2026-09-12

### zo — 라우터(게이트웨이)
- **게이트웨이가 대화 자체를 거절하면 그렇다고 말한다.** zo가 붙인 회상을 빼고 다시 보냈는데도 게이트웨이의 내용 필터가 거절하면, 이제 턴이 날것의 `400 … content-blocked`로만 끝나지 않는다. 거절된 것이 zo가 붙인 것이 아니라 대화 자체라고 알린다. 받아들일지는 게이트웨이가 정하니 바꿔 말하거나 `/model`로 다른 공급자를 고르라고 함께 알린다. 요청 id가 든 원래 오류 줄은 그대로 남는다.
- **`/model`로 모델을 바꿔도 회상 보류가 이어진다.** 같은 게이트웨이의 다른 모델로 바꾸면 회상이 다시 붙어, 거절되는 요청이 한 번 더 나갔다. 이제 새 모델도 그 게이트웨이에 회상 없이 묻는다.
- 첫 공급자 모델(Claude·ChatGPT·Gemini 로그인)의 동작은 바뀌지 않는다.
- 실측(참고): AgentRouter는 이 계정에서 한국어(세 글자 이상)·일본어·스페인어 사용자 글을 `content-blocked`로 거절한다. 영어·중국어·러시아어는 통과한다. 모든 모델(claude-opus-5·gpt-6-astra·deepseek-v4-flash·glm-5.3 등)과 두 형식(OpenAI·Anthropic)에서 같으므로, zo가 아니라 게이트웨이의 정책이다. 한국어 작업은 로그인된 첫 공급자로 하면 된다.

### 창 — 오케스트레이션
- **에이전트가 떠난 셸에는 우편 안내 줄을 치지 않는다.** 사람이 셸에서 띄운 코디네이터가 턴을 마친 뒤 종료되면, 다음 우편의 안내 줄("Run `zerocode-orc check`")이 셸에 입력되고 zsh가 그 백틱을 실행했다. 이제 안내 줄을 치기 전에 그 판의 전경 프로세스를 커널에 묻는다.

### 참고
- 1.3.48~1.3.50은 GitHub에 발행되지 않았다(설치만 한 레인). 셋의 내용도 이 판에 담긴다(자세한 것은 CHANGELOG의 각 절).
  - 1.3.50 — 한 번도 말하지 않은 Claude 판은 창 재시작 뒤 새 Claude로 돌아온다.
  - 1.3.49 — 번들 안 zo도 앱과 같은 신원으로 서명하고, 서명한 번들의 중첩 실행 파일은 모두 한 번씩 떠야 통과한다.
  - 1.3.48 — `/goal` 게이트는 명령의 종료 코드로 판정하고 화면 검사(`screen:`)를 받는다. zo Computer 도구에 `observe` 동작과 요소 누르기. 번호 보기는 새로 찍는다.

## [1.3.50] — 2026-09-12

### 창 재시작
- **한 번도 말하지 않은 Claude 판은 재시작 뒤 새 Claude로 돌아온다.** Claude는 세션을 시작할 때 세션 ID를 알리지만, 대화 파일은 첫 메시지와 함께 생긴다. 그래서 열어만 두고 말하지 않은 판은 ID만 있고 대화가 없다. 창은 이런 판을 재시작 때 `claude --resume`으로 되살렸고, claude는 「대화를 찾을 수 없음」으로 1초 만에 끝났다. 판 기록은 남으므로 다음 재시작 때도 똑같이 죽었다(한 판이 13번). 이제 기록이 가리키는 대화 파일이 확실히 없으면 실행 버튼과 같은 새 Claude를 띄운다. 이 기계에 저장된 Claude 판 기록 124개 가운데 6개가 이런 경우였다.

## [1.3.49] — 2026-09-12

### 릴리즈·서명
- **번들 안 zo도 앱과 같은 신원으로 서명한다.** 레인이 만든 1.3.48의 Mach-O 14개 가운데 `Contents/Resources/bin/zo` 하나만 링커가 붙인 ad-hoc 서명이었고, 식별자도 빌드마다 바뀌었다(`zo-fb11eae0df3341aa`). `codesign --verify`는 `--deep`을 붙여도 Resources를 봉인된 데이터로만 읽기 때문에 이것을 통과시켰다. 이제 레인의 서명기가 zo를 안쪽부터 서명한다(식별자 `zo`, leaf 고정 요구 사항). Developer ID로 빌드할 때는 `stage-zo`가 hardened runtime과 보안 타임스탬프를 붙여 서명하고, 서명된 zo가 여전히 뜨는지 확인한다.
- **서명한 번들의 중첩 실행 파일은 모두 한 번씩 떠야 통과한다.** 레인의 서명 단계와 패키지 smoke가 같은 게이트를 돈다. 번들 안의 모든 Mach-O가 앱과 같은 서명자인지 보고, 중첩 실행 파일마다 서명된 바이트로 한 번씩 띄운다. 이때는 패널·소켓·권한 확인 전에 답하고 끝나는 인자만 쓴다. zo는 `--version`, pick과 Computer Use 헬퍼는 사용법을 찍는 인자, mirror와 Chromium 헬퍼는 인자 없이 띄운다(Chromium 헬퍼는 프레임워크를 로드한 뒤 끝난다). 실측 0.91 s. probe 행이 없는 실행 파일이 새로 들어오면 게이트가 멈춘다.

## [1.3.48] — 2026-09-12

### zo — /goal
- **게이트는 명령의 종료 코드로 판정한다.** Bash 도구는 실패한 명령도 오류 표시 없이 답하고, 종료 코드는 답 안의 `returnCodeInterpretation`에만 담긴다. 그래서 모델이 게이트를 시킨 대로 한 번 돌리면, `cargo test`가 101로 실패해도 목표가 **완료**로 끝났다. 이제는 종료 코드가 0일 때만 통과로 본다. 게이트를 그대로 돌리는 모의 모델로 도는 헤르메틱 e2e가 이 거짓 완료를 잡는다: `--check false`는 수리 한 번 뒤 멈추고, `--check true`는 시작 뒤 사람 입력 없이 완료된다.
- **화면 검사**: `/goal … --check screen:window:<제목 조각>`과 `--check screen:text:<글자>`를 쓸 수 있다. ZeroCode 창의 Computer Use가 그 창이나 글자가 화면에 보이는지 10초까지 기다리고, 그 종료 코드로 판정한다. `/loop --check`에도 같다.
- 검사 낱말(`cargo:`·`git:`·`grep:`·`screen:`)은 이제 한 표에서 읽는다. 알려진 접두사 뒤의 오타(`cargo:tests`)는 시작할 때 거절한다.

### zo — Computer 도구
- **`observe` 동작**: 앱을 이름으로 주면 그 창의 접근성 트리를 보여 준다. 줄마다 앞에 요소 번호가 붙는다. `ocr`을 켜면 화면 글자도 함께 주고, 모델의 마지막 보기 이후 바뀐 곳은 언제나 준다.
- **요소 누르기**: `left_click`에 `app`과 `element_index`를 주면 좌표를 짐작하지 않고 그 요소를 접근성으로 누른다. 트리가 걸음 사이에 바뀔 수 있으므로 배치 걸음으로는 쓸 수 없다.

### Computer Use
- 번호를 매기는 데스크톱 보기(`observe --marks`)는 연속 눈의 최신 프레임 대신 새로 찍는다. 최신 프레임은 창 목록을 읽기 전의 것일 수 있어, 그 사이 창이 움직이면 배지가 다른 순간의 픽셀에 앉기 때문이다. 번호 없는 보기는 그대로 눈을 쓴다.

## [1.3.47] — 2026-09-12

### zo — 회상(지식 그래프)
- **한 낱말짜리 한국어 메시지는 지식 그래프가 이름 붙인 것만 회상한다.** 한국어는 두 글자 조각으로 맞춰 보기 때문에, 세션을 "하이"로 열면 `하이픈`·`하이라이트`가 든 메모가 요청에 붙었다. 이제 메시지가 한글 덩어리 하나이면, 그 낱말이 메모 요약이나 볼트 페이지 제목에 쓰인 이름으로 시작할 때만 회상한다. 예: `배포해줘`는 `배포`를 댄다. 이름에 조사가 붙어 있어도 된다. `회상을`로만 나오는 이름도 `회상`으로 닿는다. 조사는 목록으로 두지 않고, 그래프의 이름들이 서로 떼어 보여 준 꼬리(두 쌍 이상)에서 배운다.
- 실측(실제 저장소 220항목 + 볼트): 한국어 인사말 18개 중 회상이 붙던 9개가 3개로 줄었고, 그 토큰은 8,947 → 3,046이다. 주제어는 한국어 16/16, 영어 14/14 그대로이고 붙는 내용도 같다.
- 영어 낱말은 원래 통째로 맞춰 보므로 바뀌지 않는다. 두 낱말 이상 메시지의 회상은 순위와 내용이 바이트까지 같다(측정 프로브 654개). 회상 속도도 같다(p50 162 → 161 µs).

### 참고
- 1.3.43~1.3.46은 GitHub에 발행되지 않았다. 1.3.45는 발행 단계에서 디스크 하한(8G < 10G)에 걸렸고, 나머지 셋은 설치만 한 레인이었다. 넷의 내용도 이 판에 담긴다(자세한 것은 CHANGELOG의 각 절).
  - 1.3.46 — Computer Use 연속 눈(보는 동안 화면 스트림, `observe --settle`, `watch`, OCR은 바뀐 곳만 다시 읽기). 아티팩트를 잇달아 발행해도 「store busy」로 떨어지지 않음.
  - 1.3.46에 실렸지만 절에는 없는 변경: zerocode-browser가 페이지 글을 싣는 답과 Jira·Linear 글이 한 울타리(untrusted) 안에 온다. 쿼터 벽 교체 워커는 앞 워커가 멈춘 자리(마지막 말·최근 도구 호출, 2 KiB, 자격 증명은 가림)를 받아 시작한다. 브라우저 find 답은 호스트가 다시 센 숫자만 싣는다.
  - 1.3.45 — 게이트웨이의 내용 필터가 zo가 붙인 회상을 거절해도 턴이 죽지 않음(회상을 거둬들이고 한 번 더, 세션 동안 기억).
  - 1.3.44 — 데스크톱 번호 보기는 통째로 가려진 창을 걷지 않음, zo 보기에 번호를 실을지는 벤치 A/B(`ZO_COMPUTER_MARKS`).
  - 1.3.43 — Computer Use 번호로 가리키기(`observe --marks`·`click --mark N --look L`), 글자 칸 요소 클릭은 초점만.

## [1.3.46] — 2026-09-12

### Computer Use — 연속 눈
- **데스크톱을 보는 동안 헬퍼가 화면을 계속 지켜본다.** 첫 데스크톱 보기 때 ScreenCaptureKit 스트림을 열고, 마지막 보기 뒤 30초가 지나면 닫는다. 그 사이 메뉴 막대에 시스템의 화면 기록 표시가 뜬다. 데스크톱 보기와 `screenshot`은 새로 찍지 않고 스트림의 최신 프레임을 답한다. PNG는 화면이 다시 칠해졌을 때만 새로 만든다.
- **행동 뒤 보기는 행동이 그린 것이 끝날 때까지 기다린다(`observe --settle`).** 다시 칠해진 곳이 0.1초 동안 조용하면 본다. 행동 뒤 최대 1초까지 기다리고, 아무것도 바뀌지 않으면 0.35초 뒤에 본다. 그래서 미끄러져 들어오는 시트를 반쯤 그려진 채로 보여 주지 않는다. 1초가 지나도 움직이면 `settled: false`로 알린다. ZeroCode 자기 창의 다시 칠함과, 행동 직전부터 움직이던 곳(시계·영상·스피너)은 세지 않는다. zo의 행동 뒤 보기는 이제 이 보기 한 번이다.
- **`watch [--until change|quiet]`**: 사진을 찍지 않고 화면이 바뀌거나 멈출 때까지 기다린 뒤, 어디가 언제 바뀌었는지 답한다. zo에는 `watch` 동작으로 들어간다.
- **OCR은 바뀐 곳만 다시 읽는다.** OCR `wait-for`는 읽는 곳이 바뀌었을 때만 다시 읽는다. 데스크톱 `read --ocr`·`find --ocr`는 마지막 읽기 뒤 다시 칠해진 줄만 다시 읽는다. 조각이 넷을 넘거나 화면의 절반을 넘으면 전체를 읽는다. 전(v1.3.43~44)에는 5초짜리 OCR 대기가 화면을 6번 읽어 헬퍼 CPU를 8.2초 썼고, `read --ocr`를 되풀이하면 매번 CPU 1,190 ms가 들었다.
- 전 수치(v1.3.43): 데스크톱 보기 한 번에 헬퍼 CPU 35.5 ms. 뒤 수치는 이 판으로 재시작한 뒤 `tools/computer-bench/eye.py cost`로 잰다.
- 눈을 열 수 없는 환경(Windows, 화면 기록 권한 없음)에서는 예전처럼 캡처한다.
- 고침: 귀(소리 듣기)가 오래 읽히지 않아 스스로 멈출 때, 자기 큐 안에서 `queue.sync`를 불러 헬퍼가 멈출 수 있던 것.

### 아티팩트
- **아티팩트를 잇달아 발행해도 「artifact store busy」로 떨어지지 않는다.** 저장소 잠금은 파일에 거는 flock이다. 다른 스레드가 자식 프로세스를 fork 방식으로 띄우면, 자식은 exec 전 몇 ms 동안 방금 풀린 잠금의 사본을 쥐고 있다. 그 사이에 발행하면 거절됐다. 이제 발행은 250 ms까지 5 ms마다 다시 잡아 본다. 다른 발행이 정말 잡고 있는 잠금은 여전히 거절한다. zo 도구 시험이 전체 실행 다섯 번 중 세 번 떨어지던 것도 이것이었다.

## [1.3.45] — 2026-09-12

### zo — 라우터(게이트웨이)
- **게이트웨이의 내용 필터가 zo가 붙인 회상을 거절해도 턴이 죽지 않는다.** AgentRouter처럼 요청 글을 검열하는 중계기는 `content-blocked`(400)나 `sensitive_words_detected`(500)로 거절한다. 이런 거절이 오면 zo는 이번 요청에 스스로 붙인 회상 기억을 거둬들이고 한 번 다시 보낸다. 그 세션 동안은 그 게이트웨이에 회상을 붙이지 않으므로, 거절은 세션당 한 번이다. 실측: `/Users/dev`에서 "하이"를 보내면 첫 요청은 400이었고, 회상 없이 다시 보낸 요청은 200으로 답이 왔다.
- 이 거절은 같은 요청으로 다시 보내지 않는다. 전에는 요청 id 속 숫자열(`…520…`)이 일시적 오류로 읽혀 같은 요청을 한 번 더 보냈다.
- 첫 공급자 모델(Claude·ChatGPT·Gemini 로그인)의 동작은 바뀌지 않는다.

### 참고
- 1.3.43·1.3.44는 발행되지 않았다. 둘의 내용도 이 판에 담긴다(자세한 것은 CHANGELOG의 각 절).
  - 1.3.44 — 데스크톱 번호 보기는 통째로 가려진 창을 걷지 않음, zo 보기에 번호를 실을지는 벤치 A/B(`ZO_COMPUTER_MARKS`).
  - 1.3.43 — Computer Use 번호로 가리키기(`observe --marks`·`click --mark N --look L`), 글자 칸 요소 클릭은 초점만.

## [1.3.44] — 2026-09-12

### Computer Use — 번호 보기
- **데스크톱 번호 보기가 통째로 가려진 창을 걷지 않는다.** 대상은 사진 위 맨 앞의 ZeroCode 아닌 문서 창 가운데, 앞 창들에 다 가려지지 않고 일부라도 보이는 창이다. 전에는 사진과 겹치기만 하면 대상이 되어 전체 화면 ZeroCode 창 뒤의 창까지 걸었다. 그 보기는 번호 0개에 +149 ms(88.6 → 237.5 ms)가 들었다. 이제 그런 보기는 창 목록만 읽고 `window_not_found`로 답한다.
- 설치본 실측(v1.3.43, N = 20): 앱 보기에서 번호가 더하는 비용은 Finder +29.6 ms·범례 590 토큰, Chrome 웹 페이지 +17.6 ms·444 토큰으로 기준 안이다.
- **zo 보기에 번호를 실을지는 벤치 A/B가 정한다.** 기본은 여전히 끔(`COMPUTER_LOOKS_CARRY_MARKS`)이다. `ZO_COMPUTER_MARKS=1`로 뜬 zo만 보기에 번호를 싣고 `mark` 필드를 받는다. 벤치 표에 `marks` 설정이 생겼다. `bench.py run --config default --config marks`는 두 설정을 실행마다 번갈아 돌리고, `tally.py --versus default:marks`는 시나리오별 줄과 모든 시나리오를 합친 줄(`*`)로 둘을 견준다. 러너는 설정이 정하는 키를 자기 환경에서 물려주지 않는다. 러너가 정하는 키를 설정이 덮으려 하면 아무것도 움직이기 전에 거절한다.
- `marks.py cost`는 번호를 붙일 것이 없다고 답한 보기도 그 자리의 비용으로 잰다. 판정의 이름은 `affordable`로 바뀌었다.

## [1.3.43] — 2026-09-12

### Computer Use — 번호로 가리키기
- **`observe --marks`: 보기가 한 창의 누를 수 있는 컨트롤에 번호를 그린다.** 이름 준 앱의 창이나, 사진 위 맨 앞의 ZeroCode 아닌 문서 창이 대상이다. 답의 `marks`에 번호·역할·이름·중심이 줄마다 있다. 배지는 불투명한 노랑 위 검은 숫자(대비 14.9:1)이고, 서로 겹치지 않으며 다른 컨트롤을 덮지 않는다. 한 보기에 99개까지 매긴다.
- **`click --mark N --look L`: 그 번호의 컨트롤을 요소 길로 누른다.** 정지·결제/삭제 확인·증거는 다른 누름과 같다. 보기 뒤 그 컨트롤이 움직였거나 바뀌었으면 누르지 않고 `element_not_found`로 거절한다. 행의 글이 바뀐 경우와 별이 다른 행으로 옮겨 간 경우도 거절한다. 창을 앞으로 가져온 뒤 중심의 맨 위가 그 컨트롤이 아닐 때(메뉴·대화상자에 가려짐)도 거절한다. `--look`은 필수라 다른 에이전트의 보기를 내 번호로 누르지 않는다. 보기는 2분이 지나면 다시 봐야 한다.
- 번호를 매기지 않는 것: 스크롤 영역에 중심이 잘린 컨트롤, 메뉴·패널·Dock·다른 창에 가려진 컨트롤, 서로 가를 수 없는 닮은꼴. 데스크톱 보기는 창이 사진 전후로 가만히 있을 때만 매긴다. 같은 자리·같은 픽셀이면 번호를 다시 쓴다(`sameAsLastLook`).
- macOS에서 글자 칸을 요소로 클릭하면 초점을 준다. 전에는 확인 동작(=Return)이 불려 입력이 제출될 수 있었다. `--element-index` 클릭에도 똑같이 적용된다. Windows 제공자도 번호의 핀과 중심 검사를 따른다.
- zo는 `mark` 필드와 범례를 준비만 해 두었다. 보기에 번호를 자동으로 싣는 스위치(`COMPUTER_LOOKS_CARRY_MARKS`)는 설치본에서 보기 비용을 잰 뒤에 켠다(`tools/computer-bench/marks.py cost`).

## [1.3.42] — 2026-09-12

### zo — 라우터
- **셸 판에 직접 친 zo도 창에서 연결한 라우터의 모델을 쓴다.** 창은 라우터 키를 자기가 띄운 zo에만 넘긴다. 그래서 셸 판에 `zo`를 친 경우에는 키가 없어, 키가 필요한 라우터(AgentRouter 등)의 모델이 목록에서 빠졌다. 이제 zo는 받지 못한 라우터 키를 처음 필요할 때 창이 저장한 키체인 항목에서 한 번 읽어 자기 안에만 둔다. 환경 변수에는 넣지 않으므로 zo가 띄우는 MCP 서버·도구 셸·훅에는 여전히 가지 않는다.

### 참고
- 1.3.39·1.3.40·1.3.41은 발행되지 않았다. 셋의 내용도 이 판에 담긴다(자세한 것은 CHANGELOG의 각 절).
  - 1.3.41 — 크래시·QA·점수표가 과업을 올리는 길을 한 모듈로 합침(QA 비트 305 µs → 16 µs), `task-list --open`, zo 점수표 수정 다섯, 키체인 없는 컴퓨터의 라우터 판.
  - 1.3.40 — Computer Use 벤치(E2), zo `--allowed-tools`·`ZEROCODE_SECOND_BRAIN=off`, Windows(ConPTY·에이전트 `.cmd` 문·kill-on-close 잡), hookd 파일 모드.
  - 1.3.39 — Computer Use 레시피 재생(`recipe-run`), 비밀번호 칸에는 키를 쓰지 않음.

## [1.3.41] — 2026-09-12

### 창 (ZeroCode.app) — 과업이 되는 길(크래시·QA·점수표)
- **QA 실패 하나가 거절당해도 뒤의 실패를 막지 않는다.** 이번 부팅에 세 번 거절된 실행은 사유와 함께 제쳐 두고 그 뒤 실패를 계속 올린다. 전에는 가장 오래된 실패가 포기되면 다음 부팅까지 모든 실패가 기다렸다.
- 원장을 쓸 수 없는 부팅(부팅 때 한 번 정해지고 부팅 안에서는 풀리지 않는다)에서는 한 줄만 적고 더 묻지 않는다. 전에는 QA 길이 1초마다 같은 줄을 window-errors.log에 적었다.
- 올린 QA 실패의 대기 파일은 스탬프와 함께 지운다. 비트 한 번의 비용이 이번 주 올린 실패 20개일 때 305µs에서 16µs로, 100개일 때 1.49ms에서 11µs로 줄었다. 이제 올린 개수와 무관하다.
- 세 길이 같은 기계(판정·부팅 기억·원장 문·문장·이벤트)를 세 벌 갖고 있었다. 이제 `seat_triage` 모듈 하나를 함께 쓴다. 거절 한도를 넘긴 줄은 세 길 모두 「… — set aside until the next boot」로 적는다.

### 창 (ZeroCode.app) — API 라우터
- 키체인이 없는 컴퓨터(macOS가 아닌 곳)에서는 라우터 판이 macOS 키체인을 약속하지 않는다. 키를 보관할 곳이 없다고 먼저 말하고 키 칸을 내주지 않는다. 키가 필요 없는 라우터(Ollama 등)는 그대로 저장된다. 전에는 모든 플랫폼에서 「키는 macOS 키체인에 저장된다」고 적어 두고, 키를 넣은 저장을 그제야 거절했다.

### 오케스트레이션
- `task-list --open`: 완료·실패가 아닌 과업 전부를 보여 준다. 「아직 열린 과업이 있나?」를 묻는 쪽이 상태 이름을 따로 갖고 있을 필요가 없다.

### zo — 점수표
- `--file-tasks`가 열린 과업을 제대로 본다. 전에는 `ready`·`running`·`in_progress`·`claimed` 넷을 물었는데, 원장에 있는 상태는 `ready` 하나뿐이었다. 그래서 진행 중(dispatched)·대기(pending)·보류(blocked) 과업이 보이지 않아 다음 날 같은 회귀에 과업을 또 만들었다. 이제 `task-list --open`을 한 번 묻는다(원장 조회 4회 148ms → 1회 44ms).
- 발견 하나가 거절돼도 나머지는 계속 올리고, 거절은 보고의 `refused`에 적는다. 전에는 첫 거절에서 실행 전체가 멈췄다.
- 원장을 읽지 못하면(판에 묶인 런이 없는 등) 과업은 올리지 않는다. 발견은 그대로 판정해 보고에 싣고 `--defer`로 넘기며, 발견마다 그 이유를 `refused`에 적는다.
- `--since`에 모르는 단위(`7일`)나 시계를 넘는 기간을 주면 패닉하지 않고 거절한다.
- 플레이크 목록을 읽지 못하면 0으로 치지 않고 「읽지 못함」으로 둔다. 발견 `flakes:unread`로 알리고, 그 상태로는 기준선을 쓰지 않는다. 전에는 0으로 쳐서 목록이 늘어도 보이지 않았다.
- `crates` 디렉터리 아래의 실제 프로젝트를 픽스처로 버리지 않는다. 크레이트 시험은 증거 원장을 쓰지 않는다(adf395ae).

### 참고
- 1.3.39(레시피 재생·보안 입력)와 1.3.40(Computer Use 벤치 E2·zo `--allowed-tools`·Windows)은 발행되지 않았다. 둘의 내용도 이 판에 담긴다.

## [1.3.40] — 2026-09-12

### Computer Use — 벤치 (E2)
- **성공률을 모델 밖에서 잰다.** `tools/computer-bench/`의 표 하나(`bench.json`)를 두 레인이 걷는다. 손 레인은 시나리오의 참조 명령을 모델 없이 걸어 시나리오·준비·판정이 맞는지 증명하고 걸음 지연을 잰다. 라이브 레인은 한 줄 프롬프트를 헤드리스 zo에 준다. 첫 셋은 macOS 기본 앱(TextEdit 바꾸고 저장, 계산기 곱셈, Finder 정리)이고, 성공은 파일 바이트·파일 트리·계산기 표시가 정한다. 모델의 자기 보고는 `claimed`로만 적는다. 성공률마다 Wilson 95% 구간을 붙이고, 기준선과는 구간이 갈라질 때만 「올랐다/내렸다」라고 한다.
- 자동화 「bench:computer」로만 돈다(`bench.py setup`이 만들 값을 찍는다). 손으로 연 터미널은 거절한다. 시작 전에 보기만 하는 점검(스모크·단축키·권한·예산·잠긴 화면·앱과 창)을 하고, 10초 카운트다운 동안 입력이 있으면 시작하지 않는다.
- `tally.py`가 거절을 문장이 아니라 코드로 센다. 걸음 줄에 `code`(창이 400자로 자르기 전의 거절 코드)와 `acts`(움직인 걸음)가 남고, `status`에 `confirming`(열린 물음 수)이 생겼다.
- **러너는 정지 뒤 아무것도 하지 않고, 받은 것을 돌려준다.** 여는 것과 누르는 것 앞마다 정지·단축키가 들리는지(`hotkeyHears`)·열린 물음을 읽고, 걸리면 벤치를 끝낸다. Ctrl+C·패인 닫힘·SIGTERM은 zo를 죽이고 끝낸다. 사람의 클립보드는 데스크를 잡는 순간 항목·타입 전부(글자·그림·복사한 파일) 개인 폴더에 맡겨 두고, 실행마다 비운 뒤, 끝에 되돌린다(`bench.py restore-clipboard`). 정리는 준비가 연 프로세스만 닫고, 벤치의 것이 아닌 창이 보이면 강제 종료하지 않는다.
- 계산기는 켤 때 마지막 값을 되살리므로 준비가 지우고 0임을 확인한다. 곱만 말한 모델은 통과하지 못한다. 모델 자신의 `stop`·`resume`은 실패로, 사람의 정지는 판정 제외로 센다. 손 증명이 없거나 zo가 스스로 오류로 끝난 실행은 무효로 따로 센다. 설정마다 폴더가 따로다.
- 첫 표는 아직 없다. 사람이 고른 순간에 돌려야 한다(데스크를 움직인다).

### zo
- `--allowed-tools <이름들>`: 적은 도구만 광고하고, 다른 도구 호출은 실행 전에 거절한다. 오타는 시작할 때 거절한다. 벤치는 `--allowed-tools Computer`로 띄운다.
- `ZEROCODE_SECOND_BRAIN=off`: 설정이 볼트를 선언해도 이 프로세스에서는 볼트를 읽지도 쓰지도 않는다. 빈 값은 전처럼 「없음」이라 설정이 채운다.

### Windows
- ConPTY는 셸이 실행한 명령이 쥐고, pty에 띄운 에이전트는 끝까지 쥔다. 전에는 zo.exe가 첫 도구 뒤에 사라진 것으로 판정됐고, 재사용된 pid의 고아가 패인의 명령으로 읽혔다. 패인 앞 프로그램 이름은 `.exe` 없이 읽고, 경로보다 프로그램 이름을 먼저 댄다.
- 그룹 정책이 스크립트를 막아도 에이전트의 `.cmd` 문이 진짜 에이전트에 닿는다. 전에는 「scripts Disabled」나 「only signed」에서 어느 패인에서도 `claude`가 뜨지 않았다. 굽은 아포스트로피(O’Neil)가 든 경로도 PowerShell 리터럴 안에 머문다.
- 시간 제한이 있는 자식은 첫 명령 전에 kill-on-close 잡에 들어간다. 전에는 그 사이에 뜬 손자가 잡을 빠져나가 제한 시간을 넘겨 살았다. 검증 루트의 사적 공간 검사가 Windows에서도 돈다(소유자와 DACL).

### hookd
- 소유자 전용 `hooks.json`(0400·0700·0500)이 설치와 롤백을 거쳐도 제 모드를 지킨다. 전에는 0600으로 돌아왔다.

## [1.3.39] — 2026-09-11

### Computer Use — 레시피 재생
- **`recipe-run`: 저장한 절차를 한 호출로 걷는다.** 걸음마다 혼자 보낸 명령과 같은 길(정지·속도·확인·증거)을 지나고, 사람의 차례·마지막 한 번·저장 화면의 요소/창/pid 번호·실패한 확인·흔적 없는 누름·사람의 손·시간 예산에서 멈춰 `next`로 이어 간다. 멈춘 걸음은 `recipe_stopped`로 거절되고(보고가 페이로드), 끝까지 걸으면 ok. zo `Computer` 도구도 `recipe_run`.
- 걸음을 **깎지 않는다**: 남은 시간에 안 맞는 걸음은 시작 전에 `budget`. 앞 걸음이 표보다 오래 걸린 만큼 여유를 더 남긴다. quit·activate(5 s)·open(10 s)의 고정 대기도 센다.
- 사람의 손은 **실제 포인터를 걸음 전후로 읽어** 판정하고(앱 클릭이 옮긴 자리는 걸음의 몫), 누름의 착지는 그 점 둘레 240pt(그 점의 디스플레이), 타자는 칸을 되읽어 판정한다. ok인데 일이 안 된 걸음(`run` 실패·`launch` 창 없음·`quit` 안 끝남)은 `unfinished`로 멈춘다(batch도 같다).
- 로그에는 레시피 자신의 줄이 남는다(채운 값은 남지 않는다). 걸은 세션을 다시 저장하면 걸음 플래그는 버리고, 돌려받은 누름은 `--confirming <kind>`로, 사람이 한 걸음은 한 걸음으로 남는다. `--allow-self`·긴 글라이드·플래그 자리의 `{{값}}`은 거절한다.

### Computer Use — 키는 비밀을 쓰지 않는다
- 비밀번호 칸(macOS `AXSecureTextField`, Windows UIA `IsPassword`)에 타자·붙여넣기·붙여넣기 단축키·글자 키(`key a`)를 보내면 아무것도 보내기 전에 `secure_input`으로 거절한다. 사람이 친다(`handoff`, zo에도 생김). 받는 앱이 키보드 보안 모드를 쥐고 있어도 같다. 판정은 코어 표 셋(`secret_entry`·`key_writes`·`text_entry`)을 Rust와 Swift가 함께 읽는다.
- Tab이 든 글자는 조각으로 치며 초점이 옮겨 가 자리 잡은 곳에서 다시 판정한다. 한 줄 칸의 줄바꿈은 누름이라 거절한다(`key return`이 확인을 거친다). 한 칸에 머무는 글자는 되읽어 `verified`/`value_unchanged`. `status`에 `hotkeyHears`·`secureInput`.
- **한 손은 사람의 것**: 사람이 건 정지(단축키·창의 멈춤 버튼)는 창의 재개만 푼다. 에이전트의 `resume`은 `stopped`로 답한다. 헬퍼가 단축키로 멈추면 창이 알아채 띠에 재개 버튼이 뜬다.
- 좌표로 누르는 앱 클릭도 결제·삭제 확인을 거친다(누를 앱 안의 이름표를 읽는다).

## [1.3.38] — 2026-09-11

### 창 (ZeroCode.app) — API 라우터
- **라우터 줄마다 제 키.** 한글 이름 둘(또는 한 프리셋에서 만든 둘째 줄)이 한 변수와 한 키체인 항목을 나눠 써서, 한 게이트웨이의 키가 다른 게이트웨이로 가던 문제를 고쳤다. 줄의 변수는 첫 저장 때 백엔드가 정하고 유지한다. 옛 창이 이미 나눠 쓰게 써 둔 줄은 키를 다시 넣어 저장하면 제 변수로 옮겨 간다. 「지우기」는 표시 이름에서 짐작한 항목이 아니라 그 줄의 변수에 붙은 키만 지운다.
- 키 없이 저장한 줄(Ollama)은 `requires_auth: false`로 적혀 zo가 쓴다. 모델마다 프로브가 읽은 컨텍스트 창이 따로 적힌다(전에는 첫 모델의 값이 모두에게). 다시 저장해도 사람이 직접 넣은 키(`include_usage` 등)는 남는다.
- Cmd+N으로 연 zo 레인도 라우터 키를 받는다. zo 리더의 라우터 키가 codex·claude·gemini 팀메이트로 새지 않는다. macOS가 아닌 곳에서는 키를 받자마자 저장을 거절한다(전에는 「저장되었습니다」라고 하고 아무 데도 두지 않았다).

### zo
- **창이 준 라우터 키는 zo 안에만 있다.** zo는 시작하자마자 그 키를 환경에서 제 안으로 옮긴다. 1.3.36 실측에서 zo가 띄운 MCP 서버 다섯(`npx …@latest`로 받은 제3자 코드 포함)이 키를 물려받아 들고 있었다. 이제 MCP 서버, 도구 셸, 훅 어느 자식도 받지 않는다.
- Gemini: Code Assist 프로젝트를 계정별로 기억한다. 계정을 바꾸면 이전 계정의 프로젝트를 보내던 문제를 고쳤다. `GOOGLE_CLOUD_PROJECT`가 기억한 프로젝트보다 우선한다. 한 응답이 도구 id를 되풀이하면 호출마다 제 결과를 붙인다. 되풀이된 id의 답 없는 호출도 봉인한다. 비스트리밍 호출도 거절된 프로젝트에서 회복한다.
- 컨텍스트 창과 출력 상한을 `providers[]`의 모델마다 적을 수 있다.

### 자가 개선 루프(점수표)
- 포기한 발견 하나가 다른 발견을 막지 않는다. 원장을 쓸 수 없을 때 window-errors.log에 1초마다 적지 않고 시작과 끝에 한 줄씩 적는다. 열린 과업이 있는 키는 새 과업을 만들지 않는다. 기준선에도 표본 20 하한을 적용한다. 비트와 창이 같은 인박스를 쓴다. 벤치·임시 작업공간은 픽스처로 친다. 시끄럽게 시작한 게이트는 조용한 실행이 아니다.

### 릴리즈 레인
- 이름 없이 실패한 레시피(컴파일 실패 등)는 이름 있는 실패 옆에 있어도 진짜 빨강이다. 1.3.37 레인에서 `shell-test`는 디스크가 차서 컴파일조차 못 했는데 설정 하네스만 단독 판정받고 통과했다. 그 테스트는 이 판에서 합친 트리로 돌려 통과를 확인했다.

### 참고
- 1.3.37(Computer Use 손·눈·귀, 설치됨·이 판을 쓰는 시점에 미발행)의 내용도 이 판에 담긴다.

## [1.3.37] — 2026-09-11

### Computer Use — 한 번 판단, 여러 동작
- **`batch`**: 사람의 손에 익은 연속 동작(클릭→타자→탭→타자→엔터)을 한 요청으로 보낸다. 각 걸음은 혼자 보낸 명령과 같은 길을 타서 정지·속도·확인 물음·증거가 걸음마다 그대로 걸리고, 첫 거절에서 멈춰 어느 걸음까지 했는지 답한다. zo의 `Computer` 도구도 `batch`(steps)를 받아 호출 한 번, 보기 한 번으로 끝낸다.
- **사람이 대답하는 동안 기계의 시계가 끊지 않는다.** 모든 명령이 65초에 끊겨, 결제·삭제 확인 물음(120초)과 사람의 차례(`handoff`, 최대 600초)가 대답 도중 잘렸다. 이제 명령마다 기다릴 시간을 코어의 표 하나에서 정하고, 브리지·셸(605초)·zo·batch가 모두 그 표를 읽는다.
- **확인 물음을 우회하던 길 넷을 닫았다.**
  - 에이전트가 `--confirmed`를 직접 붙여 물음을 건너뛸 수 있었다. CLI에서 뺐다.
  - 물음이 열린 동안 다른 명령으로 「허용」을 누를 수 있었다. 그동안 모든 동작을 `person_asked`로 거절한다.
  - `--app`이나 창 id로 ZeroCode 자신을 가리키는 동작을 막는다(`app_blocked`).
  - `hold-key`(기본 버튼을 쏘는 키)와 버튼 위에서 놓는 `mouse-drag`도 누름으로 보고 묻는다.
- batch 안에서 `wait-for`가 요소 트리를 새로 만든 뒤, 옛 요소 번호로 다른 버튼을 누를 수 있었다. 이제 거절한다.
- batch 걸음의 증거 스크린샷에 batch 끝 화면이 그 걸음 이름으로 찍혔다. 이제 같은 화면의 뒤 동작이 따라온 걸음은 그 뒤 프레임이 보여 준다.
- `state.json`(오퍼레이터가 한 일의 기억)에 타이핑한 글자가 그대로 남았다. 이제 길이만 남는다.
- zo 도구의 동작 표를 코어에서 읽는다. 스키마가 4,519자에서 4,009자로 줄었다.
- `status`가 속도 표(초당 10·연타 20)를 헬퍼 세션과 무관하게 답한다. 배치 벤치(`tools/computer-bench/batch_rtt.py`)는 기본으로 포인터를 움직이지 않는다.

## [1.3.36] — 2026-09-11

### zo
- 자율 턴(`/goal`·`/loop`·cron)의 future를 턴이 실제로 도는 자리에서 박스에 담는다. 전에는 턴 전체 상태가 `drive_autonomy`에 실려, 키를 누를 때마다 16,456바이트 future가 만들어졌다. 이제 360바이트다. 1.3.35 zo 게이트의 clippy(`large_futures`)가 여기서 멈췄다.

### 창 (ZeroCode.app) — 지식 그래프
- **저장한 시야와 검색어가 창을 다시 열어도 남는다.** 전에는 다시 연 창의 목록이 비었고, 그 창의 첫 저장이 디스크의 시야를 전부 지웠다. 이제 설정 문서 한 곳에서 읽고 쓴다.
- 슬라이서의 흐림과 경로 강조가 다음 프레임(팬·배율·선택·검색 한 글자)에도 남는다. 경로 선의 강조 규칙은 아무것도 고르지 못하던 선택자를 고쳤다.
- Shift-클릭 경로가 이전 선택의 포커스 흐림을 걷는다. 렌즈를 바꾸거나 새로고침하면 경로를 두 페이지의 열쇠로 다시 찾는다. 전에는 자리 번호를 그대로 써서 엉뚱한 페이지를 밝혔다.
- `hub` 토큰은 그림이 허브로 그린 점만 밝힌다(차수 4 이상 전부 → 그린 허브). 슬라이서 프리셋(1시간·24시간·30일)은 그 벽시계 창으로 자른다. 언어를 바꿔도 슬라이서 글자가 「전체」로 돌아가지 않는다.
- 태그 칩이 검색 상자를 따라가고, 토큰과 이름이 같은 태그(`hub`)는 `tag:`로 적는다. 시야를 복원하면 그 렌즈(소스·안 부른 페이지)까지 되살린다.

### 릴리즈 레인
- 레시피가 시그널로 끝나면(레인이 게이트를 해체 중이면) 레시피 루프가 다음 레시피를 띄우지 않는다. 띄우면 쓸려 나가는 스크래치에서 레인보다 오래 살아남는다.
- 게이트 target은 이번 실행에서 마지막으로 쓰인 뒤, 디스크가 20G 아래면 비운다. 오늘 두 레인이 코드가 아니라 디스크 하한에서 멈췄다(17:32: 루트 게이트 초록, 끝난 23G target을 둔 채 7G에서 거절).
- launchd 에이전트의 PATH에 `/usr/sbin`이 들어갔다(`install-launchd.sh` 재설치). 이제 큐로 도는 레인도 부하를 읽는다.

### 참고
- 1.3.34와 1.3.35의 내용(라우팅·레인 수정, Computer Use의 손·눈·귀)도 이 판에 담겨 발행된다. 1.3.34 레인은 디스크 하한 때문에 코디네이터가 멈췄고, 1.3.35 레인은 zo clippy에서 빨갛게 끝났다. 1.3.35 레인에서 새 게이트 방식(모든 레시피·`--no-fail-fast`)이 zo 테스트 대상 14개를 모두 돌려, 실패가 clippy 하나뿐임을 보였다.

## [1.3.35] — 2026-09-11 (미발행 — 레인 gate-zo의 clippy large_futures로 빨강; 내용은 1.3.36에 포함)

### Computer Use
- **클릭이 모델이 본 곳에 떨어진다.** 모델은 1280×831 픽셀 그림으로 1512×982 포인트 화면을 보고 픽셀로 말하는데, zo의 `Computer` 도구가 그 숫자를 포인트로 그대로 눌러 모든 클릭이 원점 쪽으로 15% 빗나갔다. 이제 도구가 마지막으로 보여준 그림의 프레임(코어 `ShotFrame` 한 곳)으로 들어오는 좌표·영역·창 크기와 나가는 모든 위치를 번역한다. 영역은 Anthropic 규약대로 두 모서리 `[x0, y0, x1, y1]`.
- **증거는 응답 뒤에 남는다.** 모든 동작이 답하기 전에 350 ms 잠들고 화면을 한 번 더 찍었다(실측 466 ms, 동작 천장 분당 79). 이제 한 작성자가 순서대로 남기고(연타는 마지막만 프레임, 보기는 제 그림이 프레임, 앱 동작은 창 영역을 화면 촬영 — 요소 번호 캐시를 건드리지 않음), `verdict`·`evidence`·`recipe-save`는 쓰기가 끝날 때까지 기다린다. `screenshot` 619 ms가 촬영 자체(~90 ms)만 남는다.
- **스크린샷을 한 번만 인코딩한다.** Retina 원본 PNG를 만들어 버리고 1280으로 줄여 다시 만들던 것을 첫 칸부터 만든다: 67.7 ms → 14.7 ms(같은 589,294 바이트). 사다리 바닥을 긴 변 픽셀(320)로 바꿔 5K·6K 화면도 바이트 예산이 지켜진다(6K는 사다리가 비어 원본이 그대로 나갔다).
- **손이 스스로 멈추지 않는다.** 분당 180 동작에서 정지하고 사람의 `resume`을 기다리던 가드가 속도 조절(연타 20, 초당 10)이 됐다. 세션 5,000 상한과 ⌃⌥⎋ 정지는 그대로.
- **동작 뒤 보기가 빠르고 정직하다.** zo는 동작 뒤 `observe --diff`로 보고(619 → ~90 ms), 화면이 다시 그려질 시간(settle)을 준 뒤에도 바뀐 게 없으면 그림 없이 `changed: false`라고 말한다(이미지 ~1,418 토큰 절약). "무엇이 바뀌었나"는 `--viewer`로 그 모델 자신의 마지막 보기와 비교한다.
- **소리를 듣는다.** `listen-start [--app]`·`sound-wait --label …`·`sound-read`·`listen-stop` — 스크린샷이 이미 쓰는 「화면 및 시스템 오디오 기록」 권한으로(새 요청 없음) 시스템 내장 분류기(303 라벨)를 0.75 s 창으로 돌린다. 이 Mac에서 시스템 효과음 3번 모두 afplay 실행부터 306~381 ms에 "bell"/"beep"로 명명.
- 적대적 리뷰(23 에이전트, 확인 19·기각 0)의 지적을 전부 반영: 첫 동작이 위치를 답하는 경우의 프레임, 화면 기록 없는 경우, QA용 픽셀 허용치 대신 observe 전용(8), Swift 표와 코어 상수의 드리프트를 막는 소스 계약 등.

## [1.3.34] — 2026-09-11 (미발행 — 레인을 디스크 하한 앞에서 코디네이터가 멈춤; 내용은 1.3.36에 포함)

### zo
- **연결한 라우터가 선언한 id라도 맨 id는 1st-party로 간다.** 1.3.31부터는 창에서 연결한 라우터가 `claude-opus-4-8`처럼 사람이 직접 쓰는 id를 선언하면, zo가 그 id를 라우터 모델로 판정했다. 그러면 시작할 때 Claude 로그인을 붙이지 않은 채 Anthropic 클라이언트를 만들어 인증 실패가 났다. 이제 내장 목록에 있는 맨 id는 라우터가 선언해도 1st-party로 가고, `<라우터>/<모델>`로 골라야 라우터로 간다. 게이트웨이 자신의 슬래시 id(OpenRouter의 `anthropic/claude-x`)는 세 클라이언트 빌더 모두에서 게이트웨이로 간다.
- **`<프로바이더>/<모델>`로 고르면 그 프로바이더로 간다.** 경로, 와이어 id, 컨텍스트 창과 출력 상한이 모두 고른 프로바이더를 따른다. 예전에는 다른 게이트웨이가 같은 문자열을 id로 들고 있으면(OpenRouter의 `deepseek/deepseek-chat`) `DeepSeek/deepseek-chat`을 골라도 그쪽 키와 요금으로 갔다.
- 커스텀 프로바이더 조회가 호출마다 목록 전체를 복사하지 않는다. 모델 350개 기준으로 `context_window_for_model` 67 µs → 4.6 µs, 모델 조회 20 µs → 0.6 µs.

### 릴리즈 레인
- **플레이크 하나가 게이트의 나머지를 가리지 않는다.** `verify`의 레시피는 앞 레시피가 실패해도 전부 돈다(`verify-every-recipe.sh`). `cargo test`도 `--no-fail-fast`로 모든 테스트 바이너리를 돈다. 브라우저 하네스는 어느 것이 빨개져도 이름이 붙고 단독 ×3으로 판정된다. 1.3.31 레인은 zo 테스트 대상 14개 중 1개만 돌고 발행됐었다.
- `flakes.txt`에 적힌 이름은 단독 판정 상한(4)에 세지 않는다. 이제 부하 아래에서 함께 쓰러지는 여섯 개를 적어 둔 것이 실제로 판정을 바꾼다.
- 부하를 읽지 못하면 그렇다고 말하고 조용한 기계로 치지 않는다. launchd PATH에 `/usr/sbin`이 없어 `sysctl`을 못 찾았고, 빈 값이 "조용함"으로 읽혀 게이트가 아무 부하에서나 시작하던 것을 고쳤다.
- 지식 그래프 halo 판정은 같은 라운드의 짝 비율로 견준다. 캡처 시간이 두 봉우리(~49/~57 ms)로 갈려서 모드별 중앙값끼리 비교하면 코드가 아니라 표본이 봉우리에 나뉜 모양을 쟀다. 10회 실측에서 옛 판정은 2회 실패했고 새 판정은 0회였다.

### 참고
- 1.3.33의 내용(AgentRouter 연결, Gemini 도구 호출 id 중복 400)도 이 판과 함께 1.3.36으로 발행된다.

## [1.3.33] — 2026-09-11 (미발행 — 루트 게이트의 지식 그래프 타이밍 플레이크를 레인이 이름 붙이지 못해 빨강; 내용은 1.3.36에 포함)

### 창 (ZeroCode.app)
- **AgentRouter가 연결된다.** AgentRouter는 알아보는 클라이언트가 아니면 키를 보기 전에 401로 거절한다. 그래서 AgentRouter 프리셋 행에만 Claude Code 클라이언트 신원을 데이터로 적었다. 연결 시험은 그 행일 때만 설치된 Claude Code의 User-Agent를 보내고, 저장된 항목에도 같은 값이 적혀 zo 판이 같은 신원으로 요청한다. OpenRouter, Ollama, 사용자 정의 게이트웨이에는 다른 클라이언트인 척하지 않는다.
- 연결 시험이 거절되면 게이트웨이가 준 이유를 그대로 보여 준다. 「키를 고칠 일」과 「클라이언트를 안 받는 곳」이 구분된다.

### zo
- **Gemini가 대화 중 같은 도구 호출 id를 다시 발급해도 세션이 죽지 않는다.** Gemini는 이 id를 스스로 붙이는데 대화 전체에서 유일하게 지키지 않는다. 36턴 전 id를 다시 주면 그 뒤 모든 요청이 details 없는 `400 INVALID_ARGUMENT`로 거절됐다. 이제 요청을 만들 때 겹친 id의 뒤쪽 호출과 그 결과에 같은 새 id를 붙인다. 저장된 기록은 바꾸지 않는다. 이 기계의 사고 세션 두 개를 실패 시점에서 재생하면 1.3.31은 400, 이 판은 통과한다.

## [1.3.32] — 2026-09-11

### 창 (ZeroCode.app)
- **설정에서 연결한 라우터의 키가 zo 판에 실제로 전달된다.** 1.3.31의 API 라우터 판은 키를 키체인에 두고 zo 설정에는 변수 이름만 적었는데, 그 변수를 채워 주는 곳이 없어 zo가 그 프로바이더를 키 없음으로 봤다. 이제 zo를 띄울 때만 창이 관리하는 라우터의 키를 키체인에서 읽어 넣는다. zo의 `/connect`로 만든 항목과 다른 에이전트는 건드리지 않는다. 비용은 라우터 하나당 약 18 ms이고, 라우터가 없으면 추가 비용이 없다.

### zo
- **연결한 프로바이더의 모델이 모델 선택 목록과 `zo models`에 뜬다.** 1.3.31까지는 선택 목록이 내장 모델만 보여 줘서, 창에서 연결한 라우터 모델이 zo에 0줄이었다. 이제 쓸 수 있는 프로바이더의 모델을 `프로바이더/모델` 형태로 내장 모델 뒤에 붙인다. 키가 없는 프로바이더는 선택 목록에서 빠지고, `zo models`에는 어떤 키 변수가 비었는지 표시한다.

## [1.3.31] — 2026-09-11

### 창 (ZeroCode.app)
- **설정 「API 라우터」 판.** OpenRouter·AgentRouter 프리셋(코드가 아니라 데이터 행 `api-routers.json`)과 커스텀 행; 키는 키체인(`dev.zerocode.router.<id>`, `-w` 인자 형식)에만; 「연결 시험」이 그 라우터의 `/models`를 실제로 불러 모델 id 표를 보이고, 체크한 것만 zo가 읽는 `~/.zo/settings.json`의 `providers[]` 한 항목으로 upsert(다른 키·순서 보존; 경로는 zo의 `ZO_CONFIG_HOME → ZO_HOME → ~/.zo` 사슬). zo 코드 변경 없이 zo 피커에 뜬다. (Gemini 워커 w-3677 + 창 계약 여섯 고침)
- 1.3.30분(미발행): 자가 개선 루프의 세션 없는 닫힘 — 창의 `scoreboard_inbox` 비트 필러, launchd 시계, `just verify`의 `tools-test`; Windows 에이전트 미러 shim `.impl.ps1`+`.cmd`. **루프는 09-11 12:55 실측 증명**: 시계→`zo scoreboard --defer`→창 비트→원장 과업 t-3682(디스크 17 G), 재발화 중복 0.

### zo
- **커스텀 프로바이더가 선언한 모델은 접두사 추측을 이긴다.** OpenRouter식 `anthropic/claude-x`가 Anthropic으로 가서 404 나던 것(스텁 서버 실측) — 명시 선택 `<이름>/<모델>`은 설정된 프로바이더 이름에서 가르고, 선언된 전체 id는 그 프로바이더로 통째로 보낸다. 라우터 이름은 코드에 없다.
- `zo scoreboard --defer <파일>`(1.3.30분): 판 신원 없는 시계에서도 발견을 창의 인박스에 남긴다.

### 참고
- 1.3.30 두 번째 레인은 코디네이터가 빌드 중인 릴리즈 캐시를 지워 bundle-updater에서 빨강(함정 #208) — 설치본은 1.3.30이었고 발행은 이 판이 대신한다.

## [1.3.30] — 2026-09-11 (미발행 — 앱·zo는 설치됐고 GitHub 발행은 1.3.31에 포함; bundle-updater 중 릴리즈 캐시를 지운 코디네이터의 실수)

### 창 (ZeroCode.app)
- **자가 개선 루프가 세션 없이 닫힌다.** `scoreboard_inbox`: 스탠딩 오더 비트가 `scoreboard/pending.jsonl`(zo `scoreboard --defer`가 남긴 발견)을 읽어 크래시·QA와 같은 문(`file_task_through_seat`)으로 좌석의 자격으로 `task-create`한다. 같은 키는 7일에 한 번(스탬프), 7일 지난 발견은 뉴스가 아니다, 한 부팅에 세 번 거절되면 다음 부팅으로. 시계는 launchd `dev.zerocode.scoreboard`(`tools/scoreboard/install-launchd.sh`, 09:00).
- **Windows: 에이전트 미러 shim의 동반 파일.** 무확장 `#!/bin/sh` shim은 Windows PATH 해석에 안 잡혀 `claude`가 미러를 건너뛰었다 — 문 넷과 같은 모양의 `<agent>.impl.ps1`(BOM·엔드포인트·팀 토큰 로딩·`$LASTEXITCODE`) + `<agent>.cmd`. 감사(09-10) §5의 공백 넷이 전부 코드로 착지; 실행 검증은 CI 결제 복구 뒤.

### zo
- **`zo scoreboard --defer <파일>`** — 판(pane) 신원이 없는 launchd 시계에서도 발견을 창의 인박스 파일에 남긴다(한 줄 = 한 발견, `found_at` 포함). `--file-tasks`(판 안)는 그대로.

### 참고
- `just verify`에 `tools-test`(레인 dry-run 58·bump 16·점수표 시계 4) — 게이트 밖에 있던 파이썬 시험 셋.

## [1.3.29] — 2026-09-11

### 창 (ZeroCode.app)
- **지식 그래프 4차, 2·3단계.** **저장 시야(★)**: 렌즈·태그 칩·검색·선택·깊이·카메라(fit 대비 배율)를 이름 붙여 저장/불러오기/삭제, 볼트 경로별로 앱 설정에 JSON 한 줄(없는 노드 선택은 조용히 버림). **검색 토큰 문법**: `tag:zo rel:contradicts since:7d kind:ghost hub` + 자유 텍스트, 디밍만(멤버십·좌표 불변), 자동완성 칩, 저장 구절 칩. 1020노드 첫 그림 60~73 ms·최악 프레임 42~44 ms, 시험 14→16.
- **Windows: 터미널을 쥔 프로그램을 안다.** ConPTY에는 전경 프로세스 그룹이 없어 「에이전트가 말없이 떠났는지」와 보내기 메뉴 셋째 길이 비어 있었다 — 프로세스 트리(Toolhelp)의 셸의 가장 깊은 자손을 전경으로 읽고 이미지 경로로 이름을 낸다. 크로스 컴파일 초록, 실행 검증은 CI 결제 복구 뒤.
- **Windows: 자격 파일은 주인만 읽는다.** hookd의 Codex auth 미러·런치 락·백업이 Windows에서 폴더 ACL을 그대로 상속하던 것을 `icacls`(상속 끊고 OWNER RIGHTS 한 항목)로 0600 동등물로.

### zo
- **`zo scoreboard` — 자기 개선 루프의 빠진 독자.** 요청 타이밍(모델별 첫 바이트 p50/p90)·캐시 브레이크율·조용한 게이트 시간·플레이크 수·디스크를 24 h 창으로 모아 기준선(`zo-ide/docs/bench/results/scoreboard-baseline.json`, 초록 릴리즈 뒤 사람이 `--write-baseline`)과 비교하고, 넘은 축마다 수치를 실은 원장 과업을 쓴다(`--file-tasks`, 열린 과업은 중복 없음). 하루 한 번 `zo cron`이 쏜다. 시험 슬러그의 원장은 읽지 않는다.

### 참고
- 지식 그래프 시험 파일의 계약 둘(호버 전환·미사용 i18n 키)을 게이트 회원이 되자마자 잡아 고쳤다.

## [1.3.28] — 2026-09-11

### 창 (ZeroCode.app)
- **지식 그래프 4차(Bloom의 문법, 1단계).** 툴바에 **시간 슬라이서**(수정 시각·회상 시각 범위, 기본 「전체」; 디밍만 하고 좌표·멤버십은 그대로 — 1020노드 드래그 프레임 1 ms)와 **최단 경로**(노드를 고른 뒤 Shift-클릭 → 타입 관계를 우선하는 BFS 경로를 밝히고 인스펙터에 A → … → B 사슬, 관계 낱말은 프론트매터 키 그대로, Esc로 해제; 1020노드 0.6 ms). 그래프 코드는 `ui/shell-knowledge.js`로 분리(3,569줄). 설계: `docs/design/knowledge-graph-round4-bloom-grammar.md` — Neo4j Bloom 자체(그래프 DB 서버)를 들이지 않고 Scene·Slicer·경로·검색 구절·보기 규칙만 우리 SVG 엔진에 들인다.
- **지식 그래프 시험 14개가 게이트 회원이 되었다.** 이 파일은 09-07 이후 게이트 밖에서 나흘 동안 빨간 채였다(live 행 셋이 0일 때 `is-clean`으로 그리는 설계와 「clean 행 없음」 단정의 충돌) — 단정을 바로잡고 `just verify`에 넣었다.
- **Windows: 경계 있는 자식 프로세스가 돈다.** `bounded_process`가 Windows에서 `UnsupportedPlatform`을 답하던 것을 Job Object(`KILL_ON_JOB_CLOSE`)+파이프 리더 스레드+같은 마감 루프로 구현 — 워크플로 권위 git ref 발행(`LocalGitRefPublisher`)과 시험 증거 검증이 Windows에서 「지원 안 함」을 벗는다. 크로스 컴파일 초록, 실제 Windows 실행 검증은 CI 결제 복구 뒤.
- 정비: core·shell `orchestration.rs`의 인라인 시험을 `orchestration/tests.rs`로(제품 소스 34,793→16,717 · 19,425→7,244줄; 증분 빌드 shell 17.5→13.1 s).

### zo
- **증거 원장(캐시 브레이크·요청 타이밍)은 호스트가 무장한 프로세스만 쓴다.** 런타임 크레이트의 단위 시험 각본 응답(6000→1000)이 진짜 홈 `~/.zo/projects/…crates-runtime…`에 캐시 브레이크 행을 남기던 것을 막았다 — 09-11 새벽의 「브레이크 10행 전부 프로바이더 쪽」은 그 픽스처였다(철회).

### 참고
- 릴리즈 레인은 시끄러운 기계에서 게이트를 시작하지 않는다(1분 load ≤ 12까지 최대 900 s 대기, phase 줄마다 `load=`). 어젯밤 load 30~43에서 gate-zo 22231 s(조용할 때 649~1342 s)였던 사고의 교정.
- pty e2e 여섯(「terminal settle」 대기)은 부하 플레이크 목록에.

## [1.3.27] — 2026-09-11

### 창 (ZeroCode.app)
- **Claude 계정의 scoped 키체인 항목이 온전하게 심어진다.** 09-06부터 `security add-generic-password -w`에 비밀을 프롬프트(stdin)로 넘겼는데 그 프롬프트는 128바이트만 저장해(실측 562→128), 창이 심은 `Claude Code-credentials-<hash>` 항목이 전부 토큰 중간에서 잘린 문서였다. 이제 CLI와 같이 인자로 넘기고, 파싱되지 않는 항목은 「깨진 쓰기」로 보고 다음 실행 때 파일에서 다시 심는다(완전한 로그아웃 문서는 그대로).
- **Windows 빌드가 다시 컴파일된다.** 제품 코드 7곳(보드 배지 `set_badge_label`·알림 복귀 `RunEvent::Reopen` 등 macOS 전용 Tauri API, iOS 에뮬레이터 스텁 시그니처, Rust 2024 `unsafe extern`, pty `foreground_programs`)과 unix 전용 시험 5곳. `just win-check`(cargo xwin) 루트 12 에러→0. 두 `verify` 레시피가 도구가 있으면 Windows 크로스 컴파일을 끝에 돈다. Windows 배지는 라벨 대신 카운트(같은 상한 99).

### zo
- **관리 Claude 디렉터리(`CLAUDE_CONFIG_DIR`)의 로그인을 파일과 CLI의 scoped 키체인 항목 중 최신인 쪽에서 읽고, 갱신하면 둘에 되쓴다.** CLI 2.1.261+가 grant를 키체인으로 회전한 뒤 zo가 낡은 파일의 토큰으로 갱신해 `Claude Code OAuth refresh failed: invalid_grant`가 나던 결함.

### 참고
- CI `verify.yml`은 09-09부터 GitHub 결제 실패로 모든 잡이 시작되지 않는다(Windows 러너 검증 공백) — 계정 Billing & plans 복구가 필요하다. Windows 기능별 상태는 `docs/design/windows-parity-audit-20260910.md`.
- 레인의 알려진 플레이크 목록에 스케줄러 90초 wakeup e2e(부하 아래 마감 놓침, 단독 53 s 초록)를 더했다.

### zo — 첫 토큰(Gemini)
- **Gemini 세션의 첫 요청이 3~4 s 빨라졌다.** Code Assist 프로젝트 해석(`loadCodeAssist`/`onboardUser`)이 프로세스마다 첫 요청 앞에 돌았다(원장: 1회차 6.07 s, 2회차 1.83 s). 이제 로그인(grant 지문) 아래 기억한다 — 새 프로세스 첫 바이트 3.2~6.1 s(중앙 4.06)→1.8~1.9 s. 백엔드가 기억한 프로젝트를 거절하면 잊고 한 번 다시 푼다.

### 참고
- 1.3.26은 레인이 중단되어 발행되지 않았다(내용은 이 판에 포함): `verify`의 Windows 크로스 컴파일 회원이 레인의 찬 스크래치에서 의존성 전부를 빌드해 gate-root가 2시간을 넘겼다. 회원은 이제 Windows 타깃이 이미 따뜻할 때만 돈다.

## [1.3.26] — 2026-09-10 (미발행 — 1.3.27에 포함)

### 창 (ZeroCode.app)
- **Claude 계정의 scoped 키체인 항목이 온전하게 심어진다.** 09-06부터 `security add-generic-password -w`에 비밀을 프롬프트(stdin)로 넘겼는데 그 프롬프트는 128바이트만 저장해(실측 562→128), 창이 심은 `Claude Code-credentials-<hash>` 항목이 전부 토큰 중간에서 잘린 문서였다. 이제 CLI와 같이 인자로 넘기고, 파싱되지 않는 항목은 「깨진 쓰기」로 보고 다음 실행 때 파일에서 다시 심는다(완전한 로그아웃 문서는 그대로).
- **Windows 빌드가 다시 컴파일된다.** 제품 코드 7곳(보드 배지 `set_badge_label`·알림 복귀 `RunEvent::Reopen` 등 macOS 전용 Tauri API, iOS 에뮬레이터 스텁 시그니처, Rust 2024 `unsafe extern`, pty `foreground_programs`)과 unix 전용 시험 5곳. `just win-check`(cargo xwin) 루트 12 에러→0. 두 `verify` 레시피가 도구가 있으면 Windows 크로스 컴파일을 끝에 돈다. Windows 배지는 라벨 대신 카운트(같은 상한 99).

### zo
- **관리 Claude 디렉터리(`CLAUDE_CONFIG_DIR`)의 로그인을 파일과 CLI의 scoped 키체인 항목 중 최신인 쪽에서 읽고, 갱신하면 둘에 되쓴다.** CLI 2.1.261+가 grant를 키체인으로 회전한 뒤 zo가 낡은 파일의 토큰으로 갱신해 `Claude Code OAuth refresh failed: invalid_grant`가 나던 결함.

### 참고
- CI `verify.yml`은 09-09부터 GitHub 결제 실패로 모든 잡이 시작되지 않는다(Windows 러너 검증 공백) — 계정 Billing & plans 복구가 필요하다. Windows 기능별 상태는 `docs/design/windows-parity-audit-20260910.md`.
- 레인의 알려진 플레이크 목록에 스케줄러 90초 wakeup e2e(부하 아래 마감 놓침, 단독 53 s 초록)를 더했다.

## [1.3.25] — 2026-09-10

### zo
- **모든 프로바이더 요청이 타이밍 한 줄을 남긴다.** 조립→보냄, 보냄→첫 바이트, 보냄→첫 visible 블록, 보냄→스트림 끝을 시도 수·와이어 모델·메시지 수와 함께 `<프로젝트 state>/request-timings/timings.jsonl`에 요청마다 기록한다. 첫 토큰이 느릴 때 「요청이 떠나기 전인가, 첫 바이트 전인가, 모델 안인가」를 벤치 없이 파일에서 가를 수 있다. 재시도는 시도를 다시 열고, 아주 빠른 스트림도 첫 바이트를 잃지 않는다(완료 뒤 드레인되는 블록도 스탬프).

### 참고
- 벤치 수집기가 zo 세션의 청구 thinking 토큰(`output_tokens_details.thinking_tokens`)을 읽는다(r50 §10-나 숙제). 과제 13 전사 0→2,647.

## [1.3.24] — 2026-09-10

### zo
- **첫 토큰이 빨라졌다 — 라우팅 프로브가 본 요청 앞을 막지 않는다.** 턴마다 Haiku 분류 프로브(「You are a routing classifier…」)가 먼저 나가고 본 요청은 그 응답 뒤에 나갔다(실측 왕복 ~0.65 s, r50에서 zo 첫 토큰 2.47 s 대 CC 1.75 s의 원인). 프로브 판정을 읽는 곳은 deep-gate VERIFY 레그(옵트인)와 Architect exec 계약 둘뿐이므로, 둘 다 없는 기본 구성에서는 프로브를 돌리지 않는다. 하네스(프로브 왕복 0.6 s 재현)에서 본 요청 출발: 「이 설정값이 뭔지 알려줘」 0.619→0.008 s, r50 과제 13·15 0.614→0.008 s; `ZO_AUTO_VERIFY=1`이면 그대로 프로브를 기다린다(0.616 s).
- **실패한 이름 있는 테스트 실행은 구현 과제다.** 「./run_tests.sh fails. Make parse_csv … obey its docstring」을 키워드 표가 `docstring`의 `docs`로 「문서 편집(Trivial/Writing)」으로 읽던 것을 Medium/Debugging으로 읽는다. `docs`는 이제 낱말 경계로만 맞는다.

### 참고
- 하네스(`tools/agent-parity`)에 `PARITY_PRELUDE_DELAY_S` 손잡이 — 목 서버가 프로브 응답을 붙들어 첫 토큰 앞 직렬 비용을 재현한다.

## [1.3.23] — 2026-09-10

### zo
- **답이 중간에 끊기던 결함(Gemini).** 모델이 도구 호출을 본문 텍스트(`call:default_api:bash{…}`)로 써 버리면 턴이 그대로 끝나 「출력이 끊긴」 것처럼 보였다. turn-end-gate는 이 누출을 알았지만 마지막 800바이트만 봐서 여러 줄 인자(스크립트)가 마커를 밀어내면 놓쳤다. 이제 마커와 닫힘 기호를 짝지어 답의 끝을 읽는다 — 3일 전사 3,543건에서 누출 6건 포착 4→6, 거짓 양성 0.
- **배경 명령 스무 개를 띄우는 값이 CC와 같아졌다.** 태스크 레지스트리가 커밋마다 F_FULLFSYNC 세 번(파일·디렉터리·디렉터리)을 내던 것을 rename에서 멈추는 「발행」으로 바꿨다(팀·크론 레지스트리는 그대로 내구). 축 F 20건: 종료 폭 1.05→0.14 s, 스폰 간격 23–273→1–13 ms, 알림 도달 두 박자→한 박자.
- **라우터 피드백 힌트에 비용 항.** 같은 성공 기록이면 출력 토큰을 더 쓰는 모델의 넛지가 낮아진다(경로의 가장 싼 평균 대비 25% 허용, 3배에서 반값 페널티).
- **예상 밖 프롬프트 캐시 브레이크가 원장에 남는다.** 이유·토큰 하락·전후 캐시 읽기·와이어 모델·회차·요청 메시지 수를 `<프로젝트 state>/prompt-cache/breaks.jsonl`에 기록해 표식 수술의 재료를 모은다.
- 라우터 정확도 표의 첫 시도·재작업 분모에서 fold 결정을 뺐다(구조적 왜곡 제거, 헤드라인 불변).

### 창 (ZeroCode.app)
- **분할 판 복원이 실제 폭으로.** 두 내비바를 감안해 분할 판을 참 폭으로 되살린다.
- **hang 보고에 뷰 인구조사.** 마우스 이동 중 2초 멈춤의 진범 후보(AppKit 트래킹 영역 재수집)를 가르기 위해 30초마다 NSView 수·트래킹 영역 합을 세어 hang 부스러기·크래시 요약에 함께 싣는다. 부스러기 링의 자격증명 추정이 `native::…` 심볼 경로를 지우던 것도 고쳤다.

### 참고
- **r50 점수표 재측정(zo v1.3.22 vs Claude Code, 7과제).** 기계 축은 그대로 이김(시작→컴포저 0.28배·부팅 정착 0.05배·RSS 0.07배·캐시 읽기 0.30배·캐시 쓰기 0.50배, 전부 7-0), 턴 축은 전부 동률, 첫 토큰만 r38 대비 회귀(1.79→2.47 s, CC 1.75). codex 팔은 쿼터 벽(09-15 리셋)으로 무효. thinking은 이제 청구값으로 읽는다(0.99배). `zo-ide/docs/analysis/turn-axis-r50.md`.
- 벤치 드라이버 둘 고침(제품 아님): codex 부팅 자기 갱신 픽커에 과제 프롬프트를 넣지 않음; 마커 없는 첫 토큰 자가 스피너 프레임을 내용으로 세지 않음.

## [1.3.22] — 2026-09-10

### zo
- **도구 예산이 벽이 아니라 보고로 끝난다.** 정찰(Explore) 서브에이전트가 64회 반복 상한을 채우고 보고 없이 실패하던 낭비(이 저장소 실측: 26런 중 3런, 런당 18~21k 출력 토큰·150~187 도구 호출)를 없앴다. 마지막 두 회차에는 「N회 남았다, 지금 가진 것으로 최종 답을 써라」 리마인더가 붙고, 마지막 회차는 도구 호출을 금해 런이 모델의 보고로 끝난다. 사람이 보는 대화 턴(상한 없음)에는 아무 영향이 없다.
- **정찰(Explore) 서브에이전트의 반복 상한이 32회로.** 잘 도는 정찰은 4~27회에 끝났고(12런), 64회를 채우는 건 헤매는 런뿐이었다(6런, 런당 18~28k 토큰). 상한을 절반으로 내려 헤매는 비용을 반으로 줄이되, 위의 마무리 규칙으로 상한에서도 보고가 남는다. 사람이 보는 스폰은 상한 없음 그대로.
- **이미지 입력을 못 받는 모델에 그림을 붙이면 조용히 죽지 않는다.** 텍스트 전용 엔드포인트가 이미지를 거절할 때 원인과 회복 방법(비전 모델로 바꾸기 또는 첨부 제거)을 한 줄로 말하고, ChatGPT 웹소켓 오류 문구도 바로잡았다.

### 참고
- 1.3.21은 레인이 중단되어 발행되지 않았다. 그 내용(터미널 한글 글리프, Computer Use 타이핑 중복, 런처 라벨, 스킬 인덱스 예산)은 이 버전에 모두 포함된다.

## [1.3.21] — 2026-09-10 (미발행 — 1.3.22에 포함)

### 창 (ZeroCode.app)
- **터미널 한글이 낱말로 읽힌다.** 모노 폰트에 한글이 없어 폴백 페이스가 두 칸의 1.4칸만 채우던 것을, 글리프를 행 높이 안에서 키우고(14px에서 1.09×) 남는 폭을 양쪽으로 나눠 두 칸 가운데에 놓도록 고쳤다. 정렬은 그대로(음절당 정확히 두 칸), 행 높이도 그대로. 「자 체 는」이 「자체는」으로.
- **Computer Use 타이핑이 두 번 찍히던 결함.** 헬퍼가 접근성으로 넣은 글자를 필드(터미널 입력)가 바로 소비해 읽어보기가 비면 「길 없음」으로 여기고 합성 키 입력으로 다시 쳤다 — `echo abc`가 `echo abcecho abc`로. 이제 쓰기가 성공했으면 폴백하지 않는다(Windows 쪽 공유 판정과 같은 어휘). 붙이기는 ⌘V가 소비될 시간을 두고 클립보드를 복원해 낡은 내용이 붙던 것도 함께 끝.
- ＋ 런처의 「새 Terminal」「새 Markdown」이 「새 터미널」「새 마크다운」으로.

### zo
- **스킬 인덱스가 900토큰 예산을 넘지 않는다.** 설치된 스킬이 늘어도 매 요청 비용이 자라지 않도록, 우선순위 순으로 담다가 넘치는 꼬리는 「…and N more installed skills」 한 줄로 접는다(`Skill` 도구는 이름으로 어느 스킬이든 연다). 예산은 런타임 상수 하나에서 게이트와 렌더러가 같이 읽는다.

## [1.3.20] — 2026-09-10

### zo — 오케스트레이션 정확도, 나머지 완주
- **참석하지 않은 구현 스폰은 기본으로 검증받는다.** 헤드리스·자율·서브에이전트 턴에서 코드를 쓴 Agent 스폰이 끝나면 검증자(`code-reviewer`, verdict 스키마)가 자동으로 붙고, 검증이 fail이면 그 finding을 실어 구현자를 다시 돌린다 — 난이도별 회차 천장(`Trivial 1·Small 2·Medium 3·Large 4`)까지, 천장에선 자백한다(초록 칠 없음). 사람이 보는 턴은 그대로(되묻기 한 번, Esc가 차단기). `ZO_AUTO_VERIFY=0`으로 끌 수 있다.
- **검증자의 헛발은 적발이 아니다.** 검증자 출력이 무효(Invalid)여서 기록되는 실패는 `validator` 주체로 따로 세어 적발률을 부풀리지 않는다.
- **명령이 판정한 verdict가 학습에 들어간다.** 단일 항목 phase의 검증 명령(테스트·게이트) 결과가 `objective` 근거의 검증 결정으로 기록되어, 「모델 의견만으로 통과한 비율」이 실측이 된다.
- **에이전트 축 라우팅이 살아 있다.** 같은 route에서 실제로 잘 된 에이전트(claude/codex/gemini)의 후보 모델에 결과 근거의 보너스를 얹는다 — 다른 피드백과 같은 상한, 사용자가 지정한 모델은 그대로.
- **접힌 스폰·재작업이 실측이 된다.** 팬아웃에서 새 것 없이 끝난 레인은 `fold`로 기록되고, 재작업률은 검증이 잡아 다시 손댄 비율로 읽는다.

- **하단 모델 표기가 와이어의 진실을 말한다.** Fable 안전 분류기가 거절해 Opus로 재시도하면(그리고 쿼터 폴백·과부하 강등·에스컬레이션·deep PLAN/VERIFY 레그·Architect 구현자 스왑 때도) 푸터의 모델 칸이 실제로 요청이 나간 모델로 바뀌고 이유 한 낱말(`safety fallback`·`quota fallback`·…)이 붙는다 — 세션 모델로 돌아오는 다음 요청이나 `/model` 선택이 지운다. 런타임은 「어느 모델이 와이어에 있나」를 리졸버 하나(`wire_model`)로 답하고, 거절 판정·쿼터 회계·요청 조립이 전부 거기서 파생한다. `--json`은 `wire_model` 이벤트를 낸다.
- **게이트 거짓 빨강 제거.** 하네스 예산 게이트가 개발자 기계의 `~/.zo/skills`를 「제품이 싣는 하네스」로 재고 있었다(설정 홈 사슬 전체를 읽는 탓). 이제 모든 홈을 봉인하고 스킬 버킷 0을 핀한다 — 기계에 스킬이 늘어도 릴리즈가 막히지 않는다.

### 창 (ZeroCode.app)
- 보드 헤더 정확도 스트립이 위의 새 신호(객관 verdict·접힌 스폰·재작업)를 그대로 비춘다.
- **정확도 스트립이 칸반 보기에서도 보인다.** 목록 보기에서만 zo에 리포트를 묻던 결함을 고쳐, 보드를 어느 보기로 열어도 매 그림마다 활성 프로젝트의 리포트를 다시 묻는다(실창 Computer Use 확인에서 잡힌 것).

### 설치 (친구에게 공유할 때)
- macOS(Apple Silicon): `ZeroCode_<버전>_aarch64.dmg`를 열어 `/Applications`로 끌어 놓는다. 이 빌드는 Developer ID 공증이 없어 **처음 실행 한 번** 「확인할 수 없는 개발자」가 뜬다 — macOS 15 이상은 시스템 설정 → 개인정보 보호 및 보안 → 「그래도 열기」(첫 차단 뒤 나타남), macOS 14 이하는 우클릭 → 열기. 그 뒤엔 정상. 공증은 Developer ID 인증서가 준비되면 다음 릴리즈부터.

## [1.3.19] — 2026-09-10

### zo — 오케스트레이션 정확도(결과로 승부)
- **라우터가 「어느 모델」만이 아니라 결정 전체를 학습할 바탕이 생겼다.** 결과 스토어(`route-outcomes.jsonl`)의 레코드가 결정 종류(model·agent·decompose·verify·fold)와 재작업·개입 표식을 실어, 종류별 성공률과 「가장 약한 결정」을 집계한다. 옛 레코드(457건, `signal:"verdict"`만 있는 것)도 검증 결정으로 읽어 역사 표본이 버려지지 않는다.
- **완주 루프의 정책이 순수 함수가 됐다.** 난이도별 회차 천장 표, 천장에서는 자백한다(무한 재시도 금지). 검증 적발률과 「모델 판정만으로 통과한 비율」(정직한 위험 게이지)을 잰다.
- **에이전트 축 라우팅.** 같은 route에서 실제로 잘 된 에이전트(claude/codex/gemini)를 결과 근거로 고르는 `preferred_agent_for_route` — 표본 바닥 아래거나 동점이면 기본을 지킨다. 두 랭커 모두 원시 성공률이 아니라 **신뢰도 램프를 곱한 마진**으로 순위한다(운 좋은 2/2가 검증된 15/16을 이기던 소표본 왜곡 제거).
- **결과 스토어 무결성.** 동시 기록이 줄을 지퍼처럼 섞어 충돌마다 레코드 둘이 사라지던 결함(실스토어 3,912줄 중 5줄)을 한 번의 `write_all`로 고쳤다.
- `zo --orchestration-accuracy`: 현재 프로젝트의 정확도 리포트를 한 줄 JSON으로 낸다.

### 창 (ZeroCode.app) — 워크스페이스 보드
- **보드 헤더에 오케스트레이션 정확도 스트립.** 첫 시도 성공·검증 적발·재작업·모델 판정만·접힌 스폰 비율과 가장 약한 결정, 표본 수를 활성 프로젝트 기준으로 보인다. 데이터는 zo를 exec해 받고(창은 zo 내부를 링크하지 않는다), 증거가 없으면 숫자를 지어내지 않고 「해당 없음」으로 말한다.

## [1.3.18] — 2026-09-09

### 창 (ZeroCode.app) — 워크스페이스 보드
- 보드 착지가 루트 게이트를 통과한다. 앞 커밋이 게이트 없이 올라와 셋을 깼던 것을 마무리했다: (1) 보드 ⌘K 검색 포커스가 인라인 전역 keydown이라 소스 계약이 그걸 키보드 라우터로 오인해 키보드 계약 5개가 깨졌다 — named 핸들러로 빼 라우터를 다시 찾게 했다(동작 동일). (2) PR/리뷰 상태 필터 7개 라벨이 한국어만 있어 en/ja/zh/es 사용자가 한국어를 봤다 — 4개 로케일 번역 추가. (3) 카드 기본 상태를 첫 스테이지로 바꾼 것(카드 몰림 해소)에 맞춰 창 하네스 기대를 첫 스테이지 기준으로 정정.

## [1.3.17] — 2026-09-09

### zo
- **사용자가 메시지에서 지정한 모델로 서브에이전트가 뜬다.** 「fable 검토 받아」인데 리뷰어가 기본 Opus로 떴다 — Anthropic 모델명을 모르는 Gemini 부모가 문서 속 감독 페르소나 "Fable"로 읽고 모델은 프로젝트 기본값을 썼다. 이제 사용자 턴의 한 낱말을 레지스트리로 읽어(별칭·정식 id·짧은 표기, 근접 스냅 없음 — 산문의 `table`은 Fable이 안 됨) 단 하나의 모델명이면 그것을 `Pinned` 라우트로 못 박아, 라우터·호출 인자보다 우선한다. 두 모델을 말하면(비교) 못 박지 않고, 한국어 조사가 붙어도 이름을 뽑는다. 어휘는 카탈로그에서 읽어 모델이 늘어도 코드를 안 고친다. 돌아가는 위임 줄은 이제 **실제** 모델을 말한다(`code-reviewer started on claude-fable-5-1 · …`) — 지정이 전사에 보인다. 요청 토큰 비용 없음(런타임 판정).

## [1.3.16] — 2026-09-09

### zo
- **문장 머리에 절대경로를 붙여도 모델에 전달된다.** `/Users/dev/Desktop/web.config 이 파일 확인해봐`가 「zo에 없는 명령」으로 거절되던 결함. `/`가 슬래시 명령과 유닉스 절대경로를 동시에 열어, 제출 문 둘이 경로 전체를 명령 이름으로 읽었다. 이제 한 분류기(`slash::classify`)가 판정한다: 첫 낱말 안에 `/`가 더 있으면 경로, 한 낱말이라도 디스크에 그 이름이 있으면(`/tmp 정리해줘`) 경로, `//`는 평문 이스케이프, 빈 `/`는 팝업. 제출 문 둘과 자동완성 팝업이 같은 함수를 쓴다.

## [1.3.15] — 2026-09-09

### 창 (ZeroCode.app) — 내장 브라우저(크로미움)
- **켤 때마다 뜨던 키체인 프롬프트가 사라진다.** 내장 크로미움이 쿠키 키를 로그인 키체인의 범용 항목 「Chromium Safe Storage」에서 읽었는데, Team ID 없는 로컬 서명은 macOS가 빌드마다 다른 `cdhash` 파티션으로 취급해 「항상 허용」을 눌러도 다음 빌드가 다시 물었다(하루 네 번, 같은 프로세스 안에서도 두 번 — securityd 로그 실측). 이제 크로미움에 `--use-mock-keychain`을 넘겨 키를 프로필 옆에 두므로 키체인을 건드리지 않고 남의 항목을 공유하지도 않는다. 쿠키 저장은 프로필 디렉터리 권한이 지킨다(Linux basic 저장소와 동급). **내장 브라우저에서 이 판 이전에 로그인한 사이트는 한 번 다시 로그인해야 한다**(옛 키로 암호화된 쿠키는 읽지 않음). Developer ID 서명이 생기면 스위치를 뺄 수 있다.

## [1.3.14] — 2026-09-09

_1.3.13의 「마크다운 에디터 선택색」 항목은 사실이 아니었다: 토큰 값은 바뀌었지만 보이는 층에는 적용되지 않았다. 적대 검증이 렌더로 잡아 이 판에서 실제로 고쳤다._

### 창 (ZeroCode.app) — 마크다운 에디터
- `.md` 파일의 선택 하이라이트가 **실제로** 터미널 선택색이 된다. 이전(1.3.13 포함)에는 CodeMirror 기본 라이트 테마의 연보라 `#d7d4f0`가 다크 창에 그대로 새고 있었다 — CodeMirror가 자기 선택 층을 다섯 클래스 깊이의 셀렉터로 칠하는데 우리 규칙은 셋이라 특이도에서 졌기 때문이다. 같은 체인으로 동률을 만들어 우리 토큰(`--term-selection-bg`, 터미널과 동일·사용자 설정 따름)이 이긴다. 선택 글자색은 터미널 선택 글자색이라 채워진 선택 위에서 읽힌다(대비 다크 4.59·라이트 7.80). 새 계약이 두 테마에서 층의 실제 색·글자색·대비를 핀한다.

### 내장 브라우저(크로미움) — 알려진 공백 기록
- CEF 판은 오류 없이 **멈춘** 로드(응답 없는 dev 서버)에 대해 스피너를 멈추는 20초 타이머가 없다(wry 판은 있음). `tabs`/`diagnose`는 시계로 dead를 맞게 답하지만 창 스피너는 계속 돈다. t-3624로 추적.

## [1.3.13] — 2026-09-09

### 창 (ZeroCode.app) — 마크다운 에디터
- `.md` 파일에서 글을 선택하면 터미널과 **같은** 하이라이트가 칠해진다. 이전에는 선택색이 옅은 라벤더(강조색 30%)라 다크 모드에서 고른 글자가 거의 안 보였다. 이제 선택색은 터미널 선택색(`--term-selection-bg`, 사용자 설정 따름)이라, 파일에서 텍스트를 고르는 모습이 터미널의 Codex CLI에서 고르는 것과 같다. 글자색도 터미널 선택 글자색을 써 채워진 선택 위에서 읽힌다.

### 창 (ZeroCode.app) — 내장 브라우저(내부 정리)
- `open_browser_pane`을 공유 셋업과 엔진별 본문(크로미움 / wry)으로 갈랐다. 크로미움 빌드가 죽은 WebKit 꼬리를 컴파일하며 `allow(unreachable_code)`로 덮던 것을 없앴다. 동작 변화 없음.

## [1.3.12] — 2026-09-09

_v1.3.11의 첫 실행에서 드러난 둘: 설치 직후 창이 뜨기 전에 죽던 서명 결함(설치본은 손으로 재서명해 띄웠음)과, 닫은 크로미움 탭이 터미널 위에 남던 누수._

### 창 (ZeroCode.app) — 내장 브라우저(크로미움)
- 브라우저 탭을 닫으면 판이 실제로 닫힌다. 닫기가 Tauri 웹뷰 조회(`get_webview`)로 판을 찾았는데 크로미움 판은 그 조회에 없어, 이름표만 태우고 네이티브 뷰는 그 자리에 온 터미널을 덮은 채 남았다(창 재시작만 지웠음; 「터미널창을 덮어버리는 버그」). 다른 판 명령과 같은 엔진 중립 핸들(`browser_pane_of`)로 숨기고 닫는다 — 계약 시험이 그 자리를 핀.

### 릴리즈 레인 — 서명
- 로컬 서명이 hardened runtime(`--options runtime`)을 붙이지 않는다. hardened runtime의 library validation은 프로세스와 같은 Team ID의 라이브러리만 허용하는데 로컬 신원(ZeroCode Local Signing·adhoc)엔 Team ID가 없어, 1.3.11은 자기 Chromium Embedded Framework를 dlopen하지 못하고 창이 뜨기 전에 종료됐다. 권한 plist는 앱과 CEF 헬퍼 다섯에 그대로 실린다 — hardened runtime은 Developer ID 서명의 층. 시험이 두 신원 모두 `--options`가 없음을 핀.

## [1.3.11] — 2026-09-09

_v1.3.10은 레인에서 멈춰 배포되지 않았다(zo e2e 골든이 버전 자릿수에 묶여 있었고, 창 하네스의 시간 민감 검사 하나가 병행 부하에 흔들림). 크로미움 내장 브라우저·붙여넣기 정규화·사이드바 재귀속 후속·게이트 정리는 이 판에 함께 실린다 — 1.3.7~1.3.10 절 참고._

### zo 게이트
- e2e 골든의 버전 마스크가 버전의 **폭**도 가린다(토큰 뒤 첫 공백 런을 한 칸으로). 첫 두 자리 패치에서 부팅 카드 1행이 어긋나던 결함; 골든은 그대로.

## [1.3.10] — 2026-09-09

_v1.3.9는 레인에서 멈춰 배포되지 않았다(CEF 래퍼가 cmake Ninja를 요구하는데 기계에 ninja가 없었음). 크로미움 내장 브라우저·붙여넣기 정규화·사이드바 재귀속 후속은 이 판에 함께 실린다 — 1.3.7~1.3.9 절 참고._

### 창 (ZeroCode.app) — 내장 브라우저(크로미움) 게이트 정리
- 크로미움 병합이 저장소 게이트를 처음 통과하게 했다: clippy 4곳(let 체인·`is_none_or`·`browser_pane_of` 꼬리식), 크로미움 빌드 형상에서 도달 불가가 되는 WebKit 경로는 한 함수에 두고 컴파일러에 사유를 말함(cfg_attr, 분리는 t-3621), 소스 계약 등록부에서 chromium 모듈 항목 제거(스캐너 규약), `ExitRequested { api, code, .. }`로 바뀐 실행 루프 앵커 갱신.
- 릴리즈 레인 도구 목록에 `cmake ninja` — 빠지면 초입에 이름으로 거절한다.

## [1.3.9] — 2026-09-09

_v1.3.7·v1.3.8은 릴리즈 레인에서 멈춰 배포되지 않았다(살아 있는 헬퍼 통합 시험이 권한이 다 켜진 기계에서 처음 돌며 실패, rustdoc 비공개 링크). 그 내용(붙여넣기 정규화·사이드바 재귀속 후속)은 이 판에 함께 실린다._

### 창 (ZeroCode.app) — 내장 브라우저
- **macOS 내장 브라우저가 Chromium(CEF 152)으로 바뀐다.** 창 안 브라우저 판이 WebKit 대신 Chromium Embedded Framework 위에서 뜨고, 사이트별로 격리된 쿠키 프로필을 쓴다. 번들에 CEF 프레임워크와 헬퍼 앱 다섯(`ZeroCode Helper`, GPU·Renderer·Plugin·Alerts)·`zerocode-cef-helper`가 실리고, 서명은 `chromium.entitlements.plist`(hardened runtime)로 한다. 최소 macOS 14. 브라우저 쿠키 가져오기·탐색 상태·다운로드 처리·스모크(`scripts/chromium-cef-smoke.macos.sh`) 포함. (astra 작업, `feat/chromium-browser` 병합)

### zo
- 붙여 넣은 텍스트 정규화(탭·CR·이스케이프) — 1.3.7 절 참고.

### 창 (ZeroCode.app) — 사이드바
- 재개된 에이전트 판도 제 프로세스를 따라 워크트리를 옮긴다 — 1.3.8 절 참고.

### 게이트
- 살아 있는 Computer Use 핸드셰이크 통합 시험은 opt-in(`--ignored`)으로. 권한이 모두 허용된 기계에서 헬퍼가 시험 프로세스를 인가 피어로 보지 않는 문제는 t-3620.

## [1.3.8] — 2026-09-09

### 창 (ZeroCode.app) — 사이드바
- **재개된 에이전트 판도 제 프로세스를 따라 워크트리를 옮긴다.** 창이 직접 띄운(재개한) 에이전트는 pty 자식 자체라, 「자식이 앞에 있으면 맨 셸」로 본 1.3.6의 스윕이 그 판을 건너뛰어 zo 세션이 `.zo/worktrees/chromium-browser`에 서 있어도 `main` 아래에 남았다(화면 캡처로 확인). 이제 백엔드는 모든 판의 전경 프로세스 cwd를 사실로 보고하고, 「에이전트 판만 따라간다」는 정책은 창이 `tab.agent`/`paneAgents`로 판단한다 — 맨 셸에서 `cd`해도 탭이 다른 그룹으로 튀지 않는다.

## [1.3.7] — 2026-09-09

### zo
- **붙여 넣은 텍스트를 컴포저가 들기 전에 정규화한다.** 컴포저는 글자를 `unicode-width`로 재고 바이트를 그대로 터미널에 쓰는데, 메일·표에서 복사한 탭·CRLF·이스케이프는 폭 0으로 세어져 터미널만 탭 정지·행 머리로 뛰어 캐럿과 글자 자리가 갈라졌다(「복붙하고 인풋창에 포커스 위치가 이상해」). CRLF/CR→LF, 탭→공백(`PASTE_TAB_SPACES`), ESC 시퀀스(CSI·OSC)는 통째로 건너뛰고 그 밖의 C0·DEL은 버린다. 타이핑·히스토리 되부르기는 그대로.

## [1.3.6] — 2026-09-09

_v1.3.5는 릴리즈 레인의 포맷 검사에서 멈춰 배포되지 않았다. 그 내용(SFTP 넷·Computer Use 권한 안내 교정)은 이 판에 함께 실린다._

### 창 (ZeroCode.app) — 사이드바
- **판이 에이전트를 따라 워크트리를 옮긴다.** zo의 `EnterWorktree`나 `cd`로 전경 프로세스가 다른 체크아웃(`.zo/worktrees/*`)에 들어가면 사이드바가 그 판을 그 워크트리 카드 아래로 옮긴다(가장 깊은 포함 워크트리, 접두사만 같은 형제는 제외). 그동안은 태어난 자리(`main`)에 고정돼 작업은 `main` 아래에, 새 워크트리는 비어 보였다. 백엔드는 5초 박자로 창의 셸이 아닌 것이 앞에 선 판의 cwd만 lsof 한 번으로 읽어 변화만 `term:cwd`로 알린다.

### 창 (ZeroCode.app) — Computer Use
- **권한 복구 카드를 닫을 수 있다.** 닫은 뒤 같은 권한이 그대로 빠져 있으면 에이전트가 거부된 동작을 되풀이해도 카드가 다시 서지 않고(다른 권한이 빠지거나 카드의 버튼을 누르면 돌아옴), 거부마다 헬퍼 프로브를 새로 띄우던 것을 3초 재확인 간격으로 묶었다. zo가 스크린샷을 재시도할 때마다 「시스템 설정 안내」가 계속 떠 닫히지 않던 결함.

## [1.3.5] — 2026-09-09

### 창 (ZeroCode.app) — SFTP 파일
- **네이티브 SSH 호스트의 SFTP가 OpenSSH 서버에 연결된다.** sshd는 `subsystem`·`exec`를 띄우며 `CHANNEL_SUCCESS`보다 먼저 `WINDOW_ADJUST`를 보내는데(OpenSSH_8.0 실측), 채널 응답을 한 메시지만 읽어 「the SSH channel returned an unexpected protocol message」로 끊던 결함. 응답 대기는 분류기+루프(`channel_reply`) 하나를 `open_sftp`와 PTY 요청 사다리가 함께 쓴다.
- 실패한 SFTP 채널은 살아 있는 전송 위에 다시 연다. 「전송은 살아 있음·파일 없음」으로 캐시돼 다음 시도마다 같은 오류를 즉시 되돌리던 세션은 사라졌고, 그 즉시 거부가 렌더러의 unhandled rejection이 되던 구독 순서도 고쳤다.
- **서버당 한 행.** 저장된 SSH 호스트와 `~/.ssh/config` 별칭이 같은 `user@host:port`면 한 행으로 접히고 별칭이 이긴다(시스템 ssh가 사용자 config를 그대로 따르고 터미널 연결을 재사용). 한 계정에 별칭이 여럿이면 같은 라벨의 저장 호스트와 짝을 짓는다. 흡수된 호스트 id는 `nativeHostId`로 행에 남아 원격 워크스페이스 카드에서 열어도 같은 행이 열린다.
- 두 판 첫 행에 `..`(상위 폴더). 선택·드래그·파일 도구에 들지 않고, 클릭·Enter로 부모를 연다.
- 빈 검색어는 백엔드 영어 문장 대신 「검색어를 입력하세요.」로 거절한다(카탈로그 5개).
- SFTP 떠 있는 버튼이 「플로팅 워크스페이스 표시」 토글 옆 안쪽, 같은 높이에 선다. 토글이 어디로 끌려가도 따라간다(`paintFloatTrigger`가 위치·크기·가까운 반쪽을 CSS 변수로 공개).
- 하네스: `SFTP_ONLY=1 node ui/tests/window.mjs`로 파일 관리자 수트만 따로 돈다.

### 창 (ZeroCode.app) — Computer Use
- **권한 안내·재설정·드래그 타일이 실제로 심사받는 행을 말한다.** 화면 기록의 심사 주체는 헬퍼가 아니라 앱 자체(`dev.zerocode.app`, 목록의 「ZeroCode」)인데 토스트·설정 카드·CLI `next_step`·「권한 재설정」·헬퍼 타일이 모두 「ZeroCode Computer Use」를 가리켜 「권한 줬는데도 계속 뜨는」 상태를 만들었다. `permissions.json`의 권한별 `subject`로부터 셸이 심사 행(`judged_rows`: bundle id·이름·끌어 넣을 번들)을 보고에 실고, 모든 문이 그 행을 쓴다.

## [1.3.4] — 2026-09-08

### 창 (ZeroCode.app) — Computer Use
- **화면 기록 권한이 빌드를 넘어 유지된다.** 화면 기록 TCC는 헬퍼가 아니라 책임 프로세스인 메인 앱 `dev.zerocode.app`을 심사하는데, 메인 앱이 adhoc 서명이라 빌드마다 코드 해시가 바뀌어 켜 둔 「ZeroCode」 행이 조용히 거부됐다(tccd 「Failed to match existing code requirement」). 레인이 앱을 헬퍼→mirror·pick→바깥 앱 순으로 로컬 안정 신원으로 서명한다(`tools/signing/sign-app-bundle.sh`). 손쉬운 사용은 호출 프로세스를 직접 봐서 이미 유지됐다.
- 권한 상태 조회(`permissions`)가 OS 요청과 설정 창을 열지 않는다. `--id`를 준 요청만 연다. 3초 폴링마다 설정 창이 튀던 결함.
- 권한 요청이 헬퍼의 「Enable ZeroCode Computer Use」 드래그 창을 함께 띄운다. `+` 파일 선택창은 앱 번들 속을 못 보고, 행은 스스로 생기지 않는다. Orca·ChatGPT 헬퍼와 같은 「여기로 끌어 놓으세요」 경험.
- `mouse-move --steps N`: 사람 속도의 이동을 헬퍼가 한 왕복에 네이티브로 보간한다(이징 경로는 Core 순수 함수). 왕복 41 ms × 22단계가 1회로.
- `observe --diff`가 앱·창별로 마지막 프레임을 유계 표로 기억한다. 다른 앱을 본 뒤에도 diff가 유효.
- `tools/computer-bench/onboarding.sh`: 설치 직후 한 번 도는 온보딩(권한 둘 요청·대기 → 스크린샷·창 목록·OCR·observe → 증거).

### 코디네이션·릴리즈
- 오케스트레이션 스킬에 루트 clippy 필수, 메인 체크아웃은 병합·레인 전용 규칙. `bump.sh`가 두 잠금 파일을 올린다. 하네스 픽스처의 중복 case 라벨을 소스 계약이 거절한다.
- zo `Artifact export`: 발행된 불변 버전을 단일 파일로 내보낸다.
- SFTP 병합이 남긴 창 하네스 빨강 넷을 고쳤다. SFTP 대화상자 넷은 열 때 만들고 닫을 때 지우는 일회용이라 포커스 인벤토리(부팅 시 `aria-modal` 자식을 셈)에 없었다 → 상주 껍데기로. `.sftp-* .field`의 `background` shorthand가 select의 chevron `background-image`를 지웠다 → longhand. `paintGithubPanel`의 「소스 없음」 규칙에 근거 없이 덧붙은 「프로젝트 없음」 AND를 v1.3.3으로 복원. 팀메이트 시험은 SFTP가 넣은 3판 캡(리더+2, 이후 워커 목록)으로 갱신.

## [1.3.3] — 2026-09-08

### zo
- `Artifact` 도구: 완성된 HTML을 창의 아티팩트 갤러리(또는 헤드리스면 `~/.zo/artifacts`)에 버전으로 발행하고 로컬 주소를 돌려준다(`publish`·`list`·`read`). 같은 파일의 재발행은 같은 id의 다음 버전이고, 스켈레톤(charset·viewport·리셋·color-scheme)은 순수 함수가 씌운다. 발행 전 이번 턴에 `artifact-design`(차트는 `dataviz`, 그림은 `artifact-diagramming`)을 읽지 않았으면 거절한다. 세 스킬은 우리 말로 새로 써서 각 에이전트 홈에 설치되고, 시스템 프롬프트는 청중이 있는 완성물을 페이지로 발행해 링크를 건네라고 말한다(하네스 예산 안, 도구 1998/2000).

### 창 (ZeroCode.app)
- Computer Use 권한: 헬퍼를 기계마다 한 번 만드는 로컬 서명 신원(「ZeroCode Local Signing」)으로 서명해 빌드가 바뀌어도 macOS 권한 행이 유지된다(ad-hoc 서명은 빌드마다 신원이 달라 「켰는데도 not-granted」였다). `permissions --id`는 OS 요청을 실제로 내고 정확한 설정 창을 열며 확인 전 헬퍼를 다시 띄우고, `--reset`은 묵은 행을 지운 뒤 다시 요청한다. 설정 → Computer Use 카드에 권한 두 행(상태·설정 열기·다시 확인·권한 재설정)과 permission_denied 복구 카드, 다섯 언어. ad-hoc→로컬 신원 첫 전환은 권한 둘을 한 번 다시 허용해야 한다.
- 창 안 브라우저: 원격 페이지에 새던 웹뷰 흔적을 없앴다. Tauri 코어·플러그인 주입과 IPC 핸들러를 게스트에서 걷어내고(blank에서 태어나 WKUserContentController를 비운 뒤 클록·링만 다시 등록해 항해), 알림 플러그인이 덮어쓴 `Notification`을 순정으로 되돌리고, 관측 링의 fetch·XHR·console 래퍼를 Proxy로 바꿔 「native code」로 보이게 했다. Cloudflare Turnstile은 「확인 중」 무한 루프 대신 체크박스 단계까지 온다(자동 통과는 미확인).
- 번들: 릴리즈 오버레이의 `bundle.resources`를 맵으로 바꿔 `ZeroCode Computer Use.app`이 다시 실리고, `zerocode-mirror`·`zerocode-pick`을 명시적 `[[bin]]`으로 실어 폴더 픽커가 돈다. 릴리즈 레인은 헬퍼 셋이 없는 번들을 거절하고, 설치 런은 zo를 한 번만 짓고 업데이터 번들을 건너뛴다.

## [1.3.2] — 2026-09-08

### 창 (ZeroCode.app)
- 릴리즈 번들에 `ZeroCode Computer Use.app`이 다시 실린다. 릴리즈 설정의 `bundle.resources`가 리스트라 macOS 설정의 맵을 통째로 덮어써, 1.3.0·1.3.1 설치본에서는 모든 Computer Use 호출이 「ZeroCode Computer Use.app was not found」로 답했다. 소스 계약이 tauri-cli의 설정 병합을 모델링해 헬퍼·zo·라이선스를 핀하고, 릴리즈 레인은 헬퍼 없는 번들을 빌드 실패로 거절한다.

## [1.3.1] — 2026-09-08

Computer Use가 「앱 하나의 요소 클릭」에서 「사람이 컴퓨터로 하는 모든 동작」의 오퍼레이터가 된다(`docs/design/computer-use-full-operator.md`).

### 창 (ZeroCode.app)
- 데스크톱 동사: 화면 전체·영역 스크린샷과 `zoom`, 화면 좌표의 마우스 이동·클릭·드래그·스크롤, 키 조합·누르고 있기·타자, `wait`, 디스플레이 표.
- 앱·창·시스템: `launch/quit/activate/open`, 창이 직접 도는 `run`(시간·바이트 상한), 데스크톱의 모든 창 목록과 창 이동·크기·최소화·줌·닫기, 클립보드 읽기·쓰기.
- 의미 층: `find`(접근성 트리 또는 `--ocr`로 픽셀 글자), `wait-for`(요소·글자·창 제목, 표 60 s), `read`(트리 글자 덤프 또는 OCR). Vision OCR은 헬퍼 Core에 있어 렌더한 이미지로 시험된다.
- 한 손: 데스크톱 어디서나 ⌃⌥⎋·`zerocode-computer stop`·창의 정지 띠가 진행 중 입력을 다음 이벤트에서 끊고, 예산(분당 180·세션 5,000)을 넘기면 스스로 멈춘다. ZeroCode 자신의 창은 다루지 않는다.
- 마지막 한 번은 사람이: 결제·이체·삭제 단추에 닿는 누름은 창이 한 줄 묻고 사람이 허용한 뒤에만 보낸다(설정 셋, 기본 켜짐, 다섯 언어 낱말 표).
- 증거: 모든 행동이 단계 줄과 뒤 프레임을 남긴다 — 자동화 실행 폴더가 없으면 세션 폴더(7일 보존·프레임 500)에, 아티팩트 갤러리의 Evidence로.
- QA: `verdict`가 실행 폴더에 판정을 남기고 실패는 스탠딩 오더 비트가 과업으로 올린다; `compare`가 기준 이미지와 화면을 픽셀로 비교해 빨간 diff 그림을 준다.
- 눈·기억·복구: `observe --diff`(트리+프레임+글자+변화 사각형), `state.json`과 `status.state`(연속 실패·반복 감지), `handoff`(「사람이 할 차례」 카드), `recipe-save/list/show`(성공한 절차를 문서로).
- Windows 계약 층: 모든 동사가 「답함 / 보류 / 창의 것」 표에 놓이고 보류 메서드는 `unsupported_capability`로 거절된다(구현은 보류).
- 창의 소스 계약 아홉이 좌석 병합분과 함께 다시 초록.

### zo
- 1급 `Computer` 도구: Anthropic computer-use 어휘와 창의 의미 층을 한 액션 표로, 판의 `zerocode-computer` shim 한 길로만 실행하고 스크린샷을 이미지 블록으로 받으며 행동 뒤 한 번 본다(지연 도구).

### 도구
- `tools/computer-bench`: 시나리오 표와 증거 폴더만 읽는 집계(`tally.py`), 「보기만」 스모크(`smoke.sh`).

## [1.3.0] — 2026-09-08

첫 공개 릴리즈. 버전은 같은 리포에서 배포되던 이전 세대 zo CLI(1.2.7)의 계보를 잇는다.

### 창 (ZeroCode.app)
- 자동 업데이트: 서명된 피드(`latest.json`)로 새 버전을 확인하고, 정책 셋(알림 · 자동 내려받기 · 끄기)과 채널(안정 · 베타), 「지금 확인」·「이 버전 건너뛰기」, 버전 이력을 설정 판 「업데이트」에서 다룬다. 교체는 재시작 직전에 한 번의 rename으로.
- zo CLI가 앱과 함께 온다: 부팅 때 `~/.local/bin/zo`를 앱 버전에 맞춘다(더 새 zo는 손대지 않는다).
- 창 재시작이 워커 좌석을 지킨다: 되살아난 판이 같은 워커로 다시 앉고, 「새 빌드 준비됨」이 재시작이 끊을 워커 수를 말한다.
- 브라우저 판의 사용자 에이전트는 결정 한 곳: 리더 > 사이트별 표(정확 호스트) > Safari 이름. 설정 판에 표 편집기.
- 입력줄 「+」 첨부(칩 · 보내는 형식 · OS 드롭 · ⌘V 그림), 창 안 브라우저를 에이전트 도구로, 폴더 패널의 메인 스레드 탈출.
- 접속마다 모든 OAuth 계정의 최신 모델(codex 자신의 목록 끝점)과 고른 모델 유지.
- 쿼터 벽은 두 증인으로 판정하고 인계는 선언된 스탠딩 오더만 걷는다. 크래시는 다음 부팅에 과업이 된다.
- 릴리즈 레인이 창 밖(launchd)에서 돌고, 이제 서명 자산과 피드를 만들어 요청 시 공개한다.

### zo
- 사용량 한도(429)는 재접속이 아니라 리셋 시각을 실은 즉시 쿼터 탈출. 참석 턴은 반복 가드로 끝나지 않는다.
- 도는 편집 셀은 나중 announce에 빼앗기지 않고(가짜 「Failed to apply patch」 없음), 화면보다 큰 셀은 접혀 입력줄이 늘 보인다.
- 배경 bash의 완료는 전경 `Ran …` 셀과 같고, 살림 알림(컨텍스트 트림 · 컴팩션 예고)은 전사가 아니라 상태 줄에 잠깐 선다.
- 팀메이트 판이 떠나면 부모 명부에서 즉시 은퇴한다.
