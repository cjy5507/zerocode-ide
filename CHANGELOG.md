# Changelog

## [1.1.50] — 2026-10-03

### feat

- feat(window): the sidebar and the git panel say from git whether a checkout's work is in main — a second chip beside the task's phase reads In main, N not in main, No commits, or Check needed when the ledger and git disagree; its tooltip names the compare ref, when that ref last moved and the commit that took the work, and a landed checkout with no live session and nothing unsaved offers the existing clean-up review. Nothing is fetched for it, and git is asked in the background, four checkouts at a time (t-22104).
- feat(computer-use): Jev's judgment while Computer Use acts reads the hand's own grounds — what the hand finished since the last screen reading and how old the capture is against its limit — instead of only the readings and the totals (on 09-27 it said "pause" to 172 of 177 readings of a healthy run), and a run standing still no longer asks again on every reading (t-22110).
- feat(conversation): the conversation view takes the approved redesign — a head with the agent, the model and a running clock (it could read 「0초」 while running), flat tool rows that name each tool in the window's words and say what the result means (an HTTP status and size for a page read, a count of results for a search), thinking folded to one line, a shell row showing its first output lines, a turn rail to jump through a long conversation, and a status stack over the composer (to-dos, background tasks, queue, git) that hides when empty (t-22100).
- feat(file-tree): the file tree shows what every agent is doing — files an agent reads and writes light up on the file and its folders, and the tree follows the agent to the newest one (a setting); each changed file shows its added and removed lines, folders roll up their changes, the tree's head shows the branch and how far it is ahead or behind, and the badges stay current while the tree is open (t-24298).
- feat(file-tree): an @path in a prompt reveals that file in the tree, the file selected in the tree is offered to the agent with the next prompt (Claude, Codex), arrow keys move through the tree, a double-click opens a file in its default app, the agent's git and gh steps show in a line under the tree, and a folder can be pinned as the tree's root (t-24298).

### fix

- fix(computer-use): the look after an action says "nothing changed" only when nothing did — it reads a frame taken after the action, compares at the display's own resolution, keeps the action's baseline apart from a later look, and reads an unreadable frame as unknown; agents are no longer told to zoom in to confirm (t-15517, ported in t-22110).

### perf

- perf(computer-use): an answer from Jev or a newly written plan that comes back between two screen readings is carried out within 50 ms instead of waiting up to a second for the next reading; an answer is checked against the reading just made, so an answer asked under the previous plan is never carried out (a race the new path would have opened: 31 of 50 runs → 0 of 50) (t-22110).

### internal

- internal(tests): zo's stop-one-helper end-to-end test waits until the first helper is seen in its long step before the person's stop, so on a loaded parallel run the stop no longer overtakes the helper's first answer (t-22035).
- internal(tests): zo's tools-off request test and deep-leg advertised-set test take the env lock that every test opening a loop scope holds, so a scope another test opened no longer adds loop_schedule to their requests and fails them on a loaded run (t-24523).
- internal(tests): the window's wire suites (composer-queue, context-meter, plan-card, wire-live-stream) wait for the agent list before they open a wire page, as wire-session does, so on a slow runner the context-meter chip is no longer looked for before its agent row exists (t-24973).

## [1.1.49] — 2026-10-03

### feat

- feat(computer-use): Computer Use keeps watching the screen and judging with Jev while a big model writes a new plan — the re-plan is written in the background and runs only if the run, plan and screen it was asked about still stand; a stale reading holds the plan instead of acting on it; the newest re-plan request wins. Measured with a stand-in model that thinks 1.5 s: screen reads during the think 0 → 51, the longest read 1508 → 1.6 ms (t-21754, taking over t-21494).
- feat(conversation): code in a conversation's answers is coloured with the editor's own parser and theme colours, light and dark — the colouring runs in slices under 4 ms, a block still being streamed stays plain until it closes, and copying gives the code unchanged (t-22095).

### fix

- fix(orchestration): a mail pointer the window cannot type is no longer dropped in silence — the person at that pane sees a notice in their language saying mail is waiting and why it was not typed, and the sender's ledger gets a letter naming the pane and the reason (never the message's body), once (t-21017 part 2).
- fix(zo): switching a long conversation from a GPT model to Claude with /model no longer gets every request declined as "reasoning extraction" — the other model's reasoning summaries are not sent to Claude as text any more (the answers and tool calls still are), so the conversation goes on instead of having to start over (t-21709).
- fix(orchestration): a coordinator's mail no longer waits forever for a pane whose turn ended without the window hearing it — the pane's own terminal (its agent's ready prompt, or a long silence) ends the hold; mail for a pane the person took over, or for a run a restart left unbound, is told to its sender once instead of waiting in silence (t-21565).
- fix(zo): a window that connects the moment zo starts now finds the session's capabilities, status and history from its first request — zo writes its events address and starts answering only after they are in, so the first subscribe can no longer come back without them; two tests that read zo's advertised tools no longer race a loop test in CI (t-21349).
- fix(computer-use): a start's own settings close the live road for good — the setting read again later can close it but never reopen it (t-21754).
- fix(window): the window no longer floods itself with its own error reports — a refused report is caught and rationed (ten at once, then one a second), a hidden page the window draws for a thumbnail or an export gets no line to the backend, a terminal's mouse motion is sent once per cell instead of once per pixel, and a hang report lists the work that took the most time first (t-20972; the 7.4 s hang of 10-01 itself was not reproduced — the chain these close is the cause the evidence fits).
- fix(jev): Jev's automatic choice of a summons' model and effort can be promoted again — it no longer has to beat a baseline no summons could measure, and its quality checks stay; each model's record is kept for each effort that actually ran, a request's tokens are counted once, and a project whose cost is unknown leaves the total unknown instead of adding it as zero.
- fix(window): a pane's bell flash lets go under Reduce Motion instead of staying lit (t-21351).

### perf

- perf(conversation): while an answer streams, a new word repaints only the words — the composer and the page's head stay still and finished blocks are drawn once — so typing keeps up: the slowest 1 % of keys while streaming 13.6 → 11.6 ms on Chromium and 27 → 23 ms on WebKit (the installed app's engine; its 60 Hz frame keeps it above 12 ms), and the line that wraps the composer to a second row no longer restyles every row of a long conversation (t-22095).

### internal

- internal(ci): every CI leg runs every recipe of its verify and ends naming each red one, so one run shows every failure instead of stopping at the first — a Windows flake had hidden the window crate's tests for days (t-21326).
- internal(ci): the Windows CI leg's two failures are fixed — a store sidecar SQLite is still deleting is waited out on SQLite's own terms instead of being called an unsafe path, and the silent-server test's deadline follows the machine's measured python start (t-21270).
- internal(tests): the code-sign-clone hard-link test runs a clone of its own test binary, because macOS now kills a copied system program at launch (red in every full shell suite since this morning).
- internal(ci): the window tests judge the same page on the public macOS CI runner as on a Mac — the harness pins the runner's WebGL and Reduce Motion differences that turned 19 checks red there (t-21351).

## [1.1.48] — 2026-10-02

### feat

- feat(jev): a summons that leaves the agent open asks which agent in the same single Jev request as the difficulty and the model-and-effort pair — one round trip instead of two (wait p50 518 → 261 ms on a 250 ms judge), and the agent seat's rows can now be graded by what the work came to (t-16578).

### fix

- fix(window): opening a finished worker's workspace no longer starts an empty agent in it — the ledger names only a seat somebody is coming back to, so the workspace opens your own default and is reclaimed on time; five finished workers' folders each held an empty Claude (about 190 MB) and their build output (t-19779).
- fix(macOS): the window deletes the stale signed copies of itself that the embedded Chromium leaves at every restart, crash or force quit (0.4 GiB each, 31 on one machine) — once it has painted, on a background thread, only this app's own copies that no process runs; the copy the running window needs stays (t-20243).
- fix(Windows): the window crate's tests now run on Windows, and the defects they found are fixed — the status bar's memory read 0, search results came back with backslashes, a drive-letter checkout was never cleaned up, file-tree undo keyed files wrongly, an agent could not resume after a stop, crash history was rewritten while its own file was open (t-20781).
- fix(Computer Use): when the window refuses a generated autopilot plan, the retry sends the refused answer back as data with the reasons, so the model repairs its plan instead of starting blind; the contract checks are unchanged (t-21257).
- fix(window): helper conversations distinguish finished thoughts from live work, retain completed reports after hand-in, show the assignment and genuine role metadata, and open read-only from the board without taking over the CLI; conversation doors are made on demand so initial DOM and listeners do not increase (t-21172).
- fix(window): the coordinator's mail notice reaches a pane again after you used a slash command (such as /model) or a picker in it — the window had treated that line as holding your unsent words forever; it now lets the notice in once the agent is at rest, and still never types over words you are writing (t-21017).

### internal

- internal(zo): a runtime test that counted a helper channel's connections at once now waits for the first one to reach the listener, so it no longer fails now and then (t-20666).
- internal(release): the release lane builds without cargo's incremental cache and removes the one earlier runs left — a mid-lane root gate held 11 of its 26 GiB as that cache and brought the disk to 11 GiB (t-19990).
- internal(tests): a browser test harness ends once its whole report is written, so a slow log reader (the CI runner) sees every line and the failure it reports; before, the macOS CI cut the window suite's report at 64 KiB.
- internal(tests): the orchestrator's pipe holder test waits up to 10 s for other tests' file descriptors to close before it calls a leak — one instant reading went red on the macOS CI runner.
- internal(zo): test-only Jev mocks release idle listeners when dropped instead of keeping them to process exit; the sampled tools-test descriptor peak fell from 253 to 57 without raising the 256 limit or lowering test concurrency (t-20571).
- internal(zo tests): the ledger road's no-tmux test reads the calls' words, not the file's text — a checkout whose folder name contains tmux no longer fails it.
- internal(tests): two regressions found on main and fixed the same morning — a signed-copy test hard-linked the running test binary (nine window tests then failed in every full run), and ask_for's documentation linked a private item (the doc check failed).
- internal(Computer Use): the reflex bench reports, per run, how many screen frames were actually judged each second, how old the screen was when each first input was sent, and how soon the fixture changed after a press — the groundwork for measuring real-time control (t-21455).

## [1.1.47] — 2026-10-02

### feat

- feat(zo): /rewind works in the zo that ships — `/rewind` lists the file edits zo's write tools made this session, turn by turn, `/rewind N [force]` puts turn N's files back, and `/rewind turn` drops the last turn from the conversation and its saved transcript; before, zo answered "/rewind is not in zo" (t-19459).

### fix

- fix(computer-use): a Computer Use judgment waits only as long as the errand has left — before, a judge that never answered could hold an errand still without end; one deadline now covers a remembered answer and the fresh ask behind it, and with no time left no request is sent (t-19772).
- fix(zo): rewinding a session turn removes the whole turn, every tool round and zo's own Tool-role tool results included — before, a turn that used a tool lost only its final reply, or nothing when it ended in a tool result; shipped zo reaches this through /rewind only once t-19459 lands (t-19202).
- fix(restore): after a window restart an agent conversation comes back as itself — in the workspace its session is stored under, back in its ledger seat — and an agent typed into a plain shell gets a ledger seat too, so it can read and send mail; a letter to a pane whose seat a restart vacated is refused naming the run, instead of being accepted and reaching nobody (t-20088).
- fix(windows): three things a Windows user would hit — the lane token re-sync failed with "Access is denied" after its first call, a lock file over a junction leaked the wrong refusal, and the settings replace now retries a handle an antivirus holds for a moment — and the tests that assumed Unix paths, scripts or LF bytes no longer fail the Windows CI (t-20436).
- fix(zo): opening a helper's pane gives tmux 12 s — past that, or when tmux refuses, the helper is withdrawn and closes without working, and a pane tmux had already cut for it is closed; before, a hung tmux held the helper's start with no time limit (t-19898).

### perf

- perf(window): the window paints before it starts its browser engine — the engine's start held the main thread 0.5 s on an ordinary Mac and 1.1-1.9 s on a low-spec one before the first paint; it now starts right after it, or at the first browser pane — and every start writes its own boot timeline (13 phases, in ms, no paths or text) to boot-timeline.json and one log line; a conversation tab waiting to be woken says so with its place in the line (t-20078).
- perf(jev): a hook event reads only what was appended to the seat's Jev ledger since the last one, instead of parsing the whole ledger every time — 4,000 lines 14.6 ms -> 0.05 ms per event, 40,000 lines 135 ms -> 0.065 ms, on the low-spec profile 53 ms -> 0.14 ms (t-19914).
- perf(conversation): a long conversation dresses the rows coming into reach together instead of one by one — on a 4x-throttled CPU scroll frames over 16.7 ms drop from 64% to 57% and the delta paint p50 from 6.4 to 5.6 ms, with nothing moved at normal speed (t-20445).

### internal

- internal(tests): the idle-beat tests take the shared test window before the beat lock, as every other test does, so the shell suite cannot deadlock on them (t-19506 follow-up).
- internal(perf): a low-spec profile for every performance number the window harness gives — normal, taskpolicy -b and a 4x CPU throttle, the power state written beside each run; first finding: a long conversation's delta drawing slows 3.3x where plain script slows 1.55x (t-19683).
- internal(windows): nine Windows-only lint sites outside the window crate are fixed, so the public CI's Windows leg passes its lint step (t-5773).
- internal(ci): a build runner signs the Computer Use helper ad hoc instead of creating and trusting a local identity — the public macOS CI leg had waited 90 minutes on a password prompt (t-5773).
- internal(windows): the window crate is clean under Windows clippy (106 sites to 0) and `just win-check` lints instead of only compiling, so Windows-only warnings are caught on the Mac before CI (t-20408).
- internal(zo): five zo tests that passed only on a machine with a non-Claude provider set up and more than three cores now state the provider and helper window they test, so zo-ide-verify passes on a clean macOS CI runner (t-20300).
- internal(ci): the window crate's tests pass on a clean macOS runner — 30 restore and account-switch tests no longer ask the machine whether the claude CLI is installed, two timing tests wait for their condition — and a restart nudge's watch now runs its beats before its time bound ends it on a slow machine (t-20432).

## [1.1.46] — 2026-10-01

### feat

- feat(computer-use): Computer Use keeps pressing on a Mac with a rotated display or its built-in screen closed — macOS draws the pointer there as a small window of its own, the helper took it for the window a press lands on, and automation stopped at its first press; the helper now looks past the pointer's own picture, judges the press on the real window beneath, and still refuses ZeroCode's own window with the pointer resting on it. An app window that publishes no window number is found by its position and size when exactly one window fits, so it can be brought to the front and moved, and a screenshot can ask for its region at point resolution (the window does not ask for it yet). The helper's handshake, signature, permissions and its refusal to drive ZeroCode itself are unchanged (t-19369).

### fix

- fix(window): a paste no longer holds the whole window while the app that owns the clipboard is slow to hand it over — the window reads the clipboard off its main thread, says 「클립보드가 내용을 넘겨주기를 기다리는 중입니다…」 after 0.7 s, drops an answer that comes past 30 s, and a picture on the clipboard — PNG, TIFF, or a phone's JPEG or HEIC — is pasted as a PNG file; before, WebKit read the clipboard on the window's main thread and a slow owner froze the window for as long as it took (2.03 s in the report) (t-19409).
- fix(board): a task that finished with nothing handed in for review reads 「완료 — 검토 기록 없음」 on the board, the worker rows and the sidebar, instead of standing for ever at 「검증 대기」 (t-19328).
- fix(ledger): work that was folded into another task, handed over or outdated is closed as such, so the board stops calling it 실패, and a settle pass closes the open tasks nobody will ever hand in (t-19159).
- fix(jev): a skill-search request that got no reply is recorded as unanswered, not as a refused reply, so one bad minute of the network no longer turns Jev's skill suggestion off (t-19255).
- fix(zo): zo spawns a sub-agent through the ledger again — a launch that named no effort was stopped the moment its worker came up, with 「ledger launch receipt differs from the requested agent/model/effort」, because the effort the ledger chose for it was compared with "none"; the receipt is now held only to what the launch pinned, and a start the window refused while it was still opening another pane is asked again instead of failing (t-19774).
- fix(zo): a helper pane's wait keeps its time limit, its cancel and an answer that has landed while tmux is slow or silent — each question to tmux now runs on a thread of its own and is ended, with its process group, after 3 s; before, the wait asked tmux inside its own loop with no time limit, so a landed answer waited, a limit ran over, a cancel went unseen and a hung tmux server hung the wait (t-18917).

### perf

- perf(window): a long helper conversation draws faster — a 400-step conversation in 55.0 ms instead of 62.2 ms, a mixed 400-row page in 77.7 ms instead of 87.7 ms, and 238 fewer event listeners on the 400-step one: each step's mark is a CSS mask instead of an SVG, and one listener on the list opens and closes every row (t-16914).
- perf(window): an idle window no longer rewrites its whole task ledger every second — a beat that heard nothing writes nothing, and every reader of one ledger state shares one copy; on a large ledger an idle beat takes 19.5 ms instead of 297 ms and about 1.2 s of CPU a minute instead of 17.7 s, and on efficiency cores only (a low-spec machine) 60 ms instead of 791 ms and 3.5 s of CPU a minute instead of 45 s (t-19506).

### internal

- internal(tests): the errand wall test and the Google sign-in callback test prove their claims on a loaded machine, so release gates stop going red on them (t-19329).
- internal(build): CI compiles with the toolchain the local gates run — rustc 1.94.0 in both workspaces, and zo-ide's rustfmt.
- internal(bench): the Computer Use bench measures how long an input takes to reach the app (press→receive, appear→receive), apart from how long the hand took to press; a kept run read again gives OS delivery p50 0.86 ms / p95 6.48 ms (t-19772, first slice).

## [1.1.45] — 2026-10-01

### feat

- feat(window): the sidebar tells 작업 중, 검증 대기 and 완료 apart by the ledger — work a worker reported waits in its own 검증 대기 lane with an hourglass and a word, 완료 (a check and a word) holds only what a coordinator verified, merged or deployed, and working rows stay as they were (t-18902).
- feat(window): the helper conversation view matches its mockup — a shell step is titled by what it does (「셸 grep … · shop-app」), times the file never stamped are hidden instead of 「0.0초」, a now line says what the helper is doing, a list under a sentence renders as a list, a web search says how much came back, and a strip under the brief counts the steps by kind with failures; five languages (t-18702).
- feat(window): an artifact's 「내보내기」 also writes a PDF (A4 pages, text kept) or a picture (PNG of the whole page) drawn by the window's own WebKit, and zerocode-artifact export --out x.pdf|x.png converts by the extension — it used to write the page's HTML bytes into a file named .pdf (t-18558).

### fix

- fix(window): the 「폴더 선택 창이 아직 열려 있습니다」 notice goes away when the folder panel ends — answered, cancelled, lost or its helper gone — whichever road opened it, and its 취소 closes it at once; before, only the project-open button's own answer could dismiss it, so a panel an agent or the onboarding opened left the notice standing for ever (t-18551).
- fix(zo): a draft taller than the screen scrolls inside zo's composer, so the caret's row and the letters typed at the end of a long paste are always drawn; before, the composer grew past the screen and the last lines of a long paste were typed where nobody could see them (t-17194).
- fix(window): after a restart, a restored Claude worker's continuation is sent without a person's Enter even when the pane takes a minute to read its input — the Enter alone is pressed again until the pane takes it (Claude's catalog row: 90 s), stopped at once by the pane working, a person's keystroke, a draft or a question, and the words are never typed twice; seven panes resumed at once went from 0/7 to 7/7 arriving (t-18353).
- fix(orchestration): a reply to a question another run's coordinator asked now reaches the run that asked (`deliveredTo`), so coordinators stop answering each other with sends and the board stops piling open questions (t-18649).
- fix(zo): zo keeps each message's own time through a full rewrite of its session file (compaction, rewind, fork, heal), so the window's helper page can show how long a step took instead of 「0.0초」 (t-18703).

### perf

- perf(zo): an idle zo wakes about 0.1 times a second instead of 4 — its three one-second timers become one beat that runs only while a helper or a background task does — and a pane helper's answer is heard the moment it lands instead of at the next quarter-second look, with the parent spawning tmux 12 times in 20 s instead of about 225 (t-17057).

### internal

- internal(release): the release lane compiles Windows even when its gate targets are cold, and a failed cross-check or a missing tool is a named red instead of a skipped green; a change to the lane's shared gate policy runs both gates again (t-19107).

## [1.1.44] — 2026-09-30

### feat

- feat(window): every question the window puts to the person is one popup in the middle of the window, one at a time with the count of those waiting — a tool permission, an agent's own question (zo's model switch), Computer Use's last-step confirm and its hand-over, and the window's own confirms; Enter gives the kind's safe default and Escape refuses (t-17514)
- feat(window): a running helper's page can stop that one helper where its agent's catalog row names a road (zo today): one press asks once, the answer's word stands for a beat, and the head says the person stopped it only when the roster says the helper ended (t-16943)
- feat(zo): zo's Codex model list reaches the models the ChatGPT backend shows only to newer Codex versions, such as gpt-6.1-sol — the request names the newest Codex this machine has seen, floor 0.159.2 (t-17403)
- feat(orchestration): `worker-return --worker <id>` hands a pane the person took back to the ledger by name, refused while their own unsent words sit on its line (t-17644)

### fix

- fix(window): after a window restart, words the window left in an agent's composer with their Enter untaken are sent by one Enter alone at the composer's next ready — never left for the person, never typed twice (t-17037)
- fix(window): the words the window places in an agent's composer never carry the person's clipboard — a delivery with no words writes no bytes (the empty paste frame Claude Code read as Cmd+V), an Enter alone is the Enter alone, and every catalog row keeps one whole paste frame (t-17274)
- fix(window): a person's Enter that only sends words the window placed in a worker's pane (a resume nudge, a mail pointer) takes nothing over, so the mail pointer, stop and release keep working for that worker; a taken pane stays the person's even when its worker reports (t-17644)
- fix(window): the title bar keeps the window's width however many tabs it holds — the strip scrolls, the tab in front is brought clear of the fade, the controls on the right stay on screen (sixty tabs made a 10437 px bar in a 720 px window before); a board card's foot wraps its clocks on the narrowest rail instead of spilling (t-17078)
- fix(zo): a Sonnet or Haiku decline follows the provider's own routes, so a category the provider routes nowhere stands after one retry; the ladder's two questions (switch models, retry without the images) are questions, not tool permissions, and the window draws them as such; a search or read of a session transcript masks its thinking blocks (t-17474)
- fix(zo): a zo started by a zo ranks its parent's published model catalog under its own discovery, so a helper answers a model name with what it can see (t-17403)

### perf

- perf(zo): zo's painter writes what changed on the screen and nothing else — a changed row goes out as the cells that changed, a row that only moved is scrolled by the terminal, a pen or caret the terminal already holds is not sent again: 39 to 93 percent fewer bytes for the same screen, waiting for the model 3,234 → 1,484 bytes a second (t-17056)

### internal

- test(zo): the quality baseline's Q3 is pinned on what the product does today (t-17783); the autopilot's stale-or-late-answer test is listed as a load flake the lane judges solo

## [1.1.43] — 2026-09-30

_since v1.1.42 (134 commits)_

### feat
- feat(conversation): a helper's page says whose it is (the parent conversation, one press back, the parent's other helpers in a strip), what it was asked (a card with the instruction's first sentences and its stated conditions) and what a person can do here (speak to the parent); the composer stands on the pane's own conversation (t-15683)
- feat(computer-use): the pointer's own picture on a window list is known by three facts together (t-12979)
- feat(summon): one request at a summons carries the difficulty and the model-and-effort pair, so Jev is asked once, not twice (t-15554)
- feat(zo): zo's IDE channel can stop one inline helper by id while the parent's turn and the other helpers go on; the window's button is a later piece (t-16031)
- feat(shell): a helper's page names its parent, shows what it was asked and offers only what can be done (t-15683)
- feat(retry_signal): the provider's own server failing a request is told from the wire, the request and a capacity wall (t-15565)
- feat(conversation): the conversation view reads as a person would read it — a step is one closed line, a kind in a row folds into one, a thought is its heading, the foot line names the step, a finished helper opens on its report; one table of tool names in the core (t-15682)
- feat(conversation): a step is one closed line, a kind in a row folds into one, a thought is its heading, the foot line names the step, a finished helper opens on its report (t-15682)

### fix
- fix(zo): a decline that stands keeps the compaction it survived, so a second bare continue does not fold the conversation again (t-16786)
- fix(zo mcp): `zo mcp remove <name> --project` also clears a name that an older remove left in the trust record after its server was gone (t-16536)
- fix(shell): a long model name wraps inside its pill instead of pushing the page sideways (t-15683)
- fix(zo): helpers started together in one message never share an id (t-16031)
- fix(zo): a discovered alias row no longer hides the shipped row of its name, so Opus 5.5 keeps its refusal routes and a cyber decline can go to Opus 4.8 again (t-16493)
- fix(zo): a decline the provider routes nowhere, coming back on the next turn of the same conversation, is surfaced after one request, in words that say a bare continue will be declined again (t-15890)
- fix(computer-use): the pointer the window server lists as a window covers nothing, moves no window, and does not open the fence around ZeroCode's own window (t-12979)
- fix(bench): the workbench's press point leaves the pointer's own picture out (t-12979)
- fix(computer-use): a press where the pointer rests on ZeroCode's own window is refused before the helper is asked (t-12979)
- fix(computer-use): the pointer's picture is owned by "Window Server", up to 160 points a side (t-12979)
- fix(computer-use): a look whose only change is the pointer's picture stood still (t-12979)
- fix(computer-use): the pointer the window server lists as a window covers nothing (t-12979)
- fix(zo): a decline the provider routes nowhere, coming back for the same conversation, is surfaced at once — no second same-model retry, no second compaction — in words that say a bare continue will be declined again (t-15890)
- fix(shell): the card's whole text is a region under a name of its own (t-15683)
- fix(artifacts): a document from another project opens read-only from the gallery instead of failing with "path escapes the project", once per gesture, and the version picker reads through the store (t-16006)
- fix(zo): the router test that reads spark's Fast from the catalog takes the env lock (t-15568)
- fix(zo): one publish of other size words in the runtime lib tests, and the tests beside it read the shipped ones (t-15568)
- fix(conversation): an open streaming thought's body is written once a frame, and never under a selection (t-15682)
- fix(conversation): a thought being read stays open, with the keyboard on its line, when its words close into their turn (t-15682)
- fix(conversation): turning the Focus view on keeps the members a reader had open and the keyboard's place (t-15682)
- fix(shell): a helper's page, sidebar row and menu say the model the helper runs on, never its parent's (t-15625)
- fix(zo): a 5xx that outlives its retry ladder demotes that turn one tier, as a 529 does (t-15565)
- fix(zo mcp): `zo mcp remove <name> --project` also takes the name out of the project's trust record, so a server added later under that name asks before it runs (t-15566)
- fix(computer-use): two screen questions asked at one moment no longer share one answer file, so a press chosen for another screen cannot land here (a0591bc4)
- … 8 more fix commits

### test
- test(shell): the source-reading unit tests take part 2's helper page as it landed — a condition marker is a reader, and the helper page's head is a sixth speller of the tool count (t-15683)
- test(conversation): openHelperConversation waits for a turn to be in the page, not to be seen — a finished helper folds every turn but the person's own (t-15683)
- test(conversation): openHelperConversation waits for the first turn that is displayed, not the briefing bubble (t-15683)
- test(zo): a second bare continue after a standing decline stands too — the decline a standing turn keeps again still carries the compaction it survived (t-16786, red)
- test(zo): clippy on the new tests — a helper that always answers Some, and a lock guard held over the test's awaits (t-15890)
- test(mcp): a name an older project remove left in the trust record comes out through remove (t-16536, red)
- test(shell): the 360px page's room to read is measured before a long parent title is laid in (t-15683)
- test(zo): the standing-decline decision tests keep the compaction the decline survived, and the seam test's changed conversation walks the whole ladder (t-15890)
- test(shell): the 360px page also holds a long parent title, and names what sticks out (t-15683)
- test(shell): the pane composer's Shift+Enter is pressed in the box the person has focused (t-15683)
- test(zo): one clock reading makes ids that differ and only grow, so two Agent calls started together never share one (t-16031, red)
- test(zo): the tests that pin the shipped opus and sonnet take the env lock (t-16493)
- test(zo): a discovered alias row keeps the shipped duties it leaves empty (t-16493, red)
- test(zo): the incident-shape test names the routes it stands on, so it holds before and after a discovery row stops shadowing the shipped one (t-15890, for t-16493)
- test(shell): the pane's composer stays on screen when the pane narrows, as a helper page's composer was held to (t-15683)
- test(shell): the composer's dock, frame and draft are measured on the pane's own conversation, as a helper's page was held to them (t-15683)
- test(zo): the 09-29 decline in the catalog state the person's build ran with — a continue after a cyber decline routed nowhere asks once and says so (t-15890)
- test(shell): the footer keeps a place for a second control, and "no stop" is a rule about agents without a road (t-15683)
- test(computer-use): red — a press whose point cannot be read is refused by the window, closed (t-12979)
- test(bench): red — the workbench presses where ZeroCode's control is, with the pointer resting on it (t-12979)
- … 37 more test commits

### style
- style(shell): the helper page's notes say what the merged catalog says about stopping one helper (t-15683)
- style(shell): the helper page's still-list and focus-list sit where part 1's edits to the same lists will not collide (t-15683)

### other
- … 15 more other commits

## [1.1.42] — 2026-09-29

_since v1.1.40 (110 commits)_

### feat
- feat(summon): Jev picks a model and its effort together, easy and ordinary work runs a tier down at its highest effort, and a summons says why (t-14437)
- feat(pane): a summoned worker's turn starts with its likely files, and every pane's agent can ask zerocode-find (t-14869)
- feat(hook): a pane turn's ruler counts its calls as the t-14656 baseline did (t-14869)
- feat(artifacts): an annotation made on a page the tab followed away to is recorded too (t-14586)
- feat(summon): zo's lineup is every provider zo lists, an agent with no lineup says why, and a launch left to its CLI says so (t-14437)
- feat(settings): the summons table shows what runs today and picks from today's lineup; a new model is said once in a line of its own (t-14437)
- feat(summon): Jev picks the model a summons runs on from today's lineup, and a new model takes a share of easy work first (t-14437)
- feat(summon): today's lineup makes the summons choices — a new model is offered without anybody typing it (t-14437)
- feat(zo): zo models --json rows carry the tier classifier's band and rungs and the efforts each model accepts (t-14437)
- feat(jev): each use records what its choices offer for none of these (t-13096)
- feat(bench): a measured covered round names the cover seat's word, counts an autopilot's end for the person, and times a person's own run (t-12979)

### fix
- fix(probe): a child that ended before the kill is the state the kill was for
- fix(zo): a helper's words reach the main once, and Esc hands a blocking helper's result to the background road (t-15207 = t-11459 + t-11460)
- fix(board): the desk names the two idle reasons in four locales (t-15313)
- fix(zo): an unattended turn that keeps writing is not ended at two hours, and the cut names its limit (t-15208)
- fix(zo): SpawnMultiAgent's collection is a quiet window — a working helper is never cut for its age (t-15208)
- fix(workflow): a phase has no wall clock unless somebody names one, and the wall that ends an agent says which (t-15208)
- fix(zo): one age rule for every road that could end a helper, and a pane running a tool call is not quiet (t-15208)
- fix(window): the folded-model notice says a newer version arrived, in plain Korean (t-14437)
- fix(summon): a challenger's turn does not wait on the model seat's answer (t-14437)
- fix(summon): efforts are ranked by one order, the widest ladder's, so an effort below low is ranked too (t-14437)
- fix(orchestration): a taken-over worker's unread mail is told to its coordinator, once and named so, and nothing is typed into the person's pane (t-15313)
- fix(orchestration): mail stays unread until it is acknowledged, a worker at rest with unread mail or in its own wait is told once, and a stall is one notice per episode (t-15313)
- fix(zo): words a person types while a zo turn works are on disk when the model reads them, so a restart before the turn ends no longer resumes without them (t-11457)
- fix(file-pick): a pane's candidates come from the task's own words, files ranked by how many they name (t-14869)
- fix(summon): the lineup-less origin is a test's road only (t-14437)
- fix(summon): every row zo lists is ranked, an older release included, and zo's fallback runs the provider its own settings run (t-14437)
- fix(jev): the model seat is judged by the work its answers launched, like the difficulty seat, and a challenger's turn grades neither (t-14437)
- fix(summon): the lineup reader has a name of its own beside zo's slash catalog, and the model seat's tests share the difficulty seat's deferred host (t-14437)
- fix(jev): the model seat offers abstain beside every model, and says so in the abstain column (t-14437)
- fix(knowledge): a galaxy wears its own colour cell and a star's centre keeps its edit colour in the universe (t-14081)
- fix(orchestration): the pointer stops fighting a person's draft — one offer per watermark until the draft goes, and a collected pointer is not called uncollected (t-14585)
- fix(window): a reported checkout takes the reclaim's one judgment, and one road at a time holds a checkout (t-12773)
- … 10 more fix commits

### perf
- perf(summon): agent-list reads settings only for the agents that can launch a difficulty (t-14437)

### refactor
- refactor(summon): a model's three medians are taken by one loop (t-14437)

### test
- test(probe): a child that ended before the kill does not fail the run (red)
- test(orchestration): two pinned pointer tests read the at-rest count of an unacknowledged batch (t-15313)
- test(zo): a helper's words and a let-go helper's result reach the main (t-15207 = t-11459 + t-11460, red)
- test(zo): the collection tests name their bindings apart and add no time subtraction (t-15208)
- test(board): the desk says what a resting worker rests on (t-15313) — red
- test(zo): the working-turn tests hold an async lock across the turn's awaits (t-15208)
- test(summon): an effort below low is ranked like every other, and a challenger's turn does not wait on the acting seat (t-14437, red)
- test(orchestration): a taken-over worker's unread mail reaches nobody (t-15313) — red
- test(orchestration): a go-ahead leased into an unread output, a worker at rest beside its own wait, and an ordered wait told every five minutes (t-15313) — red
- test(zo): a helper that is working is cut by its age on four roads — the tests that say it (red, t-15208)
- test(runtime): words typed during a tool are on disk when the model reads them (t-11457, red)
- test(pane): the red tests compile — the replay's pane says it is no worker, the endpoint owns its answer (t-14869)
- test(orchestration): a worker briefing names zerocode-find before the task — red (t-14869)
- test(hook): a turn start's road is the catalog's and a pane without one says why — red (t-14869)
- test(pane): zerocode-find answers from the pane's search, is asked beside its answer and graded as asked — red (t-14869)
- test(hookd): zerocode-find asks the window with the agent's words, folder and pane — red (t-14869)
- test(find): zerocode-find reads the agent's words and prints each likely file with its first line — red (t-14869)
- test(pane): a summoned worker's turn brief says the files its one question selected — red (t-14869)
- test(settings): a new model is said once per set of models and a folded one in a line of its own (t-14437)
- test(hookd): a turn's brief joins the prompt's context within the script's budget and carries nothing else — red (t-14869)
- … 20 more test commits

### style
- style(settings): the two lineup fields stand as rustfmt writes them (t-14437)
- style(pane): one condition shelves a turn's file pick (t-14869)
- style(summon): a challenger's turn reads is_multiple_of (clippy) (t-14437)

### other
- … 1 more other commits

## [1.1.40] — 2026-09-29

_since v1.1.39 (25 commits)_

### feat
- feat(bench): presses by number on a covered fixture — the road people's Computer Use takes (t-12979 slice 2)
- feat(reflex): a run whose app window is covered stops, is uncovered and plans again — or ends for the person (t-12979 1c)
- feat(bench): a covered round puts its scene up and writes down what the cover saw (t-12979 slice 2)
- feat(computer-use): a covered mark is uncovered and pressed, or left to the person in one line (t-12979 slice 1)
- feat(jev): a window row names the HTTP version its answer came back over
- feat(bench): a covered round's grade believes the fixture and the cover alone (t-12979 slice 2)

### fix
- fix(computer-use): the cover seat is in the act-line contract and its three sentences go through the catalog (t-12979)
- fix(computer-use): a cover question that brings nothing back leaves the hand to today's rule, not to the person (t-12979)
- fix(jev): the scripted HTTP/1.1 answer's opening is not named like what the server heard (t-13199)
- fix(jev): a question nothing came back for lets go of its client — the next opens a connection of its own
- fix(jev): a goal walk left at auto presses from its first step — the screen seats' way out of recording

### docs
- docs(jev): the cover seat's marks name their grader in words, not by a link to a private item (t-12979)
- docs(release): the crates both trees compile now include jev-socket

### test
- test(jev): a fake endpoint gives its loopback seat back when its test is done
- test(reflex): a run whose app window another window covers stops before it presses again (t-12979 1c, red)
- test(computer-use): a cover question that brings nothing back leaves the hand to today's rule (t-12979, red)
- test(computer-use): a press whose place another window hides is uncovered, not refused (t-12979 slice 1, red)
- test(jev): over HTTP/2 the question after an unanswered one rides the connection that went quiet
- test(bench): a covered round grades a hand that only stopped, or did nothing, at zero (t-12979 slice 2, red)
- test(jev): a goal walk left at auto presses from its first step and its row is marked (red)

### chore
- chore(jev): the lock files name jev-socket at the workspace's version (t-13199)
- chore(bench): the covered rounds' seeds, fixed after the grader and before any scene was drawn (t-12979)

### other
- Merge branch 'wt/t-12979/computer-use' (ac14ccb0) — a press whose place another window hides is uncovered and pressed, or left to the person in one line (slice 1 of "covered, and found and pressed all the same"). Before, a press by a look's mark whose centre another window stood over was refused as not found, the agent was told the control had moved, and the next look dropped the covered control from its table, so to the agent it was not there. Now the window list is read for what stands over the place; the hand stops; the cover seat — a new row of the Jev table, asked once with four independent questions (is it covered, what kind of thing covers it, which move, can the move be undone) over facts only: the owning app, the layer, the bounds and the share hidden, never a title or what a window shows — says which move; the target's own window is brought to the front or moved to a clear place, the list is read again after each move, and the mark is pressed once it shows. Nothing in front is read, answered, moved or closed. A system dialog, another app's window above ordinary windows, a modal, or an answer unsure of itself leaves the hand still and asks the person in one line, in five languages, naming the app and nothing else; an agent is told `covered`. Left at `auto` the seat acts from the start and falls on its own marks (the rule of t-13091); when its answer does not come — a timeout, a refusal, a malformed answer — the hand goes by today's rule, the one the product uses with the seat off, which itself asks the person about another app's window above. What covers a point is one function, the one a look already dropped covered controls with. The branch also carries the covered rounds' grader and seeds for the bench (a hand that only stopped, or did nothing, scores zero; seeds fixed after the grader and before any scene was drawn). Red first: nine of twelve of the driver's checks failed at named assertions before (abacf26d), and two more before the fall to today's rule (828ccaf3). The worker's targeted runs on 63d4866f (ac14ccb0 adds bench tools only): the driver 13 of 13, the autopilot 43 of 43, the settings words 28 of 28, zerocode-core's jev and protocol groups 455 of 455, clippy of the root workspace and pii-check 0. The coordinator read the driver and the rule, and gated the landing tree: see the release commit's gate. The reflex autopilot stops a run whose stage is covered, uncovers it the same way and plans again (slice 1c), and the bench gains the covered scenes' runner. Left for the next slices: moving an ordinary covering window aside. (t-12979 slice 1; coordinator review)
- Merge branch 'wt/t-13199/jev' (6374754e) — a Jev question nothing came back for lets go of its client, so the next one opens a connection of its own. On the night of 2026-09-29 TypeSafe's service answered a third to a half of requests for twenty minutes, and the Computer Use measurement's rounds went all or nothing — 62 of 62 answered, then 0 of 58, 63 of 63, and three rounds of 0 of 4 — while new connections made at the same time were answered two or three times in five. The window and zo each keep one HTTP client per process, and over HTTP/2 a request that timed out left its connection in the pool, so every question after it rode the connection that had gone quiet. One rule now stands in one place, a leaf crate both trees compile (`jev-socket`, no dependencies): a request that ends with no response — a timeout, a transport failure — lets go of its client and builds the next there and then, outside the next question's wall; a request that was answered, a status included, keeps its connection; nothing is asked twice. zo's Jev requests ride a client of their own instead of the pool every provider shares, with the same number of connections. Every Jev row the window writes names the HTTP version its answer came back over. Red first (57df6d90), on a scripted local server and no key: over HTTP/2 the question after an unanswered one rode the same connection ([0, 0] where [0, 1] was wanted) in the window and in zo alike, while HTTP/1.1 already opened its own. Measured on that server before and after: with half the connections silent, HTTP/2 answered 500 of 1,000 questions before and 987 after (HTTP/1.1 987 both times); healthy, the median answer took 210 µs before and 184 µs after; with every connection silent, 100 questions took 15.27 s before and 15.28 s after; building a client takes 68 µs at the median. The coordinator's gate found one follow-up and fixed it on the branch (6374754e): a binding in zo's test server named too like another for zo's pedantic lint. Coordinator verification in the warm gate checkout, the worker having run no build: the leaf crate's tests, the window's systemone and computer_use groups, zerocode-core's jev group 288 of 288, source_contracts 535 of 535, zo's api systemone group, the door contract in zo's tools, clippy of the root workspace and of zo's api, and pii-check all exit 0. The real service speaks HTTP/2 (read without a key on 2026-09-29). (t-13199; coordinator review)
- Merge branch 'wt/t-13091/jev-c1-auto' (a823e967) — a goal walk left at `auto` presses from its first step, which is the screen seats' way out of recording. In seven days of this machine's Computer Use, 26,124 steps, the three screen seats (browser, desktop, emulator) were asked nothing: a goal walk under `auto` asked once, pressed nothing, and the mark a seat rises on is written only on pressed rows, so the seats could never rise. They now start acting, as the summons seats did in t-11989: a person's `on`, or an `auto` its own marks have not taken back, presses; a person's `shadow` records; a late, malformed or missing answer is not carried out and the walk hands `needs_fallback` to its caller; and a seat that starts acting falls on its own marks alone, since for it a fall is for good. Every door that stood between a judgment and a press still stands — the press floor and its destructive rung, the money bar, the injected and walled screens, the pin checked again at the press. A stopped recorded walk's clearing, a press in the middle of a walk someone wrote down, still presses under `auto` only on a rise the judge recorded (`promote::risen`, `systemone::applies_once_risen`). The Computer Use skill says so in the `needs_fallback` line. Red first (b0d6f9c0): four named assertions failed — the table row (left Recording, right Applying), the standing of a seat nobody judged, the goal walk's presses (0, wanted 1) and the seat's fall on its own misses — while the helper test the refactor touched passed. Coordinator verification on a823e967 in the warm gate checkout, the worker having run no build: zerocode-core's library 2196 of 2196, the window's computer_use group 338 of 338, systemone, typesafe_settings and jev_scope 44 of 44, source_contracts 535 of 535, zo's tools library 1804 of 1804, clippy of the root workspace, of the window and of zo's tools exit 0, pii-check exits 0. Left to measure after deployment: the seats' seven-day request count, which the acceptance line wants above zero. (t-13091; coordinator review)

## [1.1.39] — 2026-09-29

_since v1.1.38 (76 commits)_

### feat
- feat(artifacts): delivered annotations are recorded per page, and the header band leads with 「주석」, 「나란히」 and 「내보내기」 (t-11959)
- feat(knowledge): close to a picked galaxy its own pages are the name candidates, up to twelve; shown plates take the keyboard; a cut name drops the space before its ellipsis (t-12443 ⑥, designer m-12670)
- feat(knowledge): 「움직임」 in the view menu — the universe's slow turn at rest and its flowing filaments on one switch that the next universe keeps (t-12443 ⑦)
- feat(knowledge): the universe names itself — galaxy plates, filament counts, page names and the star tip, placed as v4 places them; the legend's four universe rows and a galaxy's six bright stars (t-12443 ⑥)
- feat(artifacts): a publication reaches the person — 「새 N」 on the maker's tab, one status line when it is off screen, and the first publish opens beside it (t-11958)
- feat(knowledge): the inspector speaks the universe — thickest filaments as sender → receiver, a picked galaxy's shape, pages and two-week share, a page's "how long ago", and relation editing lands flat (t-12443 ⑤)
- feat(knowledge): the universe answers the hand — pick, point, press, clusters, search and paths light their stars and constellation lines, and the camera flies there (t-12443 ⑤)
- feat(knowledge): filaments flow between galaxies from the side that sent more — v4's pairs, particles and arrays (t-12443 ④)
- feat(knowledge): the universe's galaxies wear the prototype's bodies — discs, cores, nebulae, nameless stars and the sky (t-12443 ③)
- feat(knowledge): the universe's pages are the prototype's named stars in galaxies built from the flat map's clusters, through its bloom and tone mapping (t-12443 ②)
- feat(knowledge): the whole map opens as a universe where WebGL2 stands, and 2D | 3D in the head moves between them (t-12443 ①)
- feat(zo): delegation opens ledger workers and brings their reports back to the tool (t-11962)
- feat(knowledge): the overview fills the canvas — the map's bounds take 0.9 of it above the legend's band (t-12029 fit)
- feat(knowledge): surprising links — the single line between two large topics stands out as a numbered curve (t-12029 bridges)
- feat(knowledge): a cluster's name is one line at its disc's edge; its pages and main page are in its tip (t-12029 oneLine)
- feat(knowledge): colour sorts the topics again — the eight largest clusters wear the eight hues, the rest are quiet (t-12029 colour)
- feat(knowledge): clusters show their branches — wider discs, small points, leaves that reach out (t-12029 spread)
- feat(zo): a frame probe that times each draw by phase without the per-row log (t-11961)
- feat(knowledge): the overview bundles the lines between clusters into one tie per pair (t-12029 bundle)

### fix
- fix(window): the board card's +N eases its hover like every other control (t-12077)
- fix(usage): an expired Grok login is read on the ordinary cadence, not on every two-second ask
- fix(window): a board card's +N is a button that opens the other agents in the card and folds them again (t-12077)
- fix(knowledge): pressing 3D while the universe folds takes the fold back — the same universe rises again instead of being let go when the fold lands (t-12443 ⑦)
- fix(knowledge): both hands seat the topic plates by one rule — the SVG plates stand above the points as the GL ones do, so the label grid no longer differs by hand (t-12443, coordinator m-12652)
- fix(artifacts): in a narrow beside-group the header band wraps its buttons and keeps its title (t-11958)
- fix(knowledge): a picked galaxy is close wherever it flew, page names are cut like the flat map's, the tip wraps inside the stage (t-12443 ⑥, designer m-12642)
- fix(zo): Esc during a tool ends the turn at that tool, so a steer typed during it becomes the fresh turn every time (t-11961)
- fix(knowledge): a galaxy's tint leans no further than v4's palette did — 0.8 + 0.2 * linear(hue) (t-12443 ②, designer m-12570)
- fix(knowledge): a hub's light stops at the prototype's ratio, and a galaxy quieter than the others turns elliptical in a busy vault (t-12443 ②, designer m-12546)
- fix(zo): the compact Agent schema keeps its detached contract within the harness budget (t-11962)
- fix(knowledge): a small canvas keeps a label's room at the map's edge, a ghost takes the pointer across its disc, and three checks read the new sizes (t-12029 fit, spread)
- fix(zo): the gate's clippy and the catalog contract pass on the merged tree (t-11961)
- fix(knowledge): a map with no tie keeps its lines between topics; only a bridge's one line steps aside for its curve (t-12029 bundle)
- fix(zo): the manifest table keeps a settled helper's stamp and drops its body (t-11961)
- fix(zo): a table stops being held once a blank line closes it (t-11961)
- fix(zo): the roster watchers share what they read, and leave a quiet store unlisted (t-11961)
- fix(zo): a frame asks no model catalog, and the roster watcher reads only a manifest that changed (t-11961)
- fix(zo): the turn loop asks the terminal for input only when the terminal woke it (t-11961)

### perf
- perf(artifacts): a tab with nothing new passes the 「새 N」 paint without asking the document (t-11958)
- perf(zo): the frame asks the pty its size with one ioctl on the terminal it draws on (t-11961)

### refactor
- refactor(knowledge): one name for the kept galaxy that sizes the plate and the close-in threshold, and no array per frame while the filament counts rest (t-12443)

### docs
- docs(zo): the input module names parking_lot as code (t-11961)

### test
- test(usage): an expired Grok login asked for an hour on a stopped clock is read on every ask (red)
- test(window): a board card's +N opens the other agents in the card and folds them again (t-12077 red)
- test(browser): the annotation bundle is pasted after the typed input, as R2 made it (t-11959)
- test(artifacts): delivered annotations leave a record, and the header band leads with 「주석」 (t-11959, red)
- test(knowledge): the plate check asks both hands for the same seat at the disc's edge, now that both stand their plates above the points (t-12443, coordinator m-12652)
- test(knowledge): the universe measure writes the machine's load beside each size and says whether the spin kept one universe (t-12443 ⑧, designer m-12698)
- test(knowledge): the universe's numbers on a real GPU in v4's frame — first 3D frame, frame work, burst, spin, memory, rest, fly and the 60 s heap; 「움직임」 wears the lens segment (t-12443 ⑧)
- test(artifacts): a publication reaches the person — 「새 N」, one status line, and the page beside its maker (t-11958, red)
- test(zo): a tool cancelled together with the turn's stop ends the turn, and the steer typed during it stays queued (t-11961, red)
- test(knowledge): the pixel comparison of the two painters enlarges the map's sizes too (t-12029 spread)
- test(zo): a benchmark that drives zo and the other agent CLIs through one scripted conversation (t-11961)
- test(window): the knowledge graph's first fit is read against the overview's own fit token (t-12029 fit)
- test(zo): a finished manifest is remembered by its stamp, not its body (t-11961, red)
- test(zo): a table closed by a blank line is no longer held, and what follows it commits as it comes (t-11961, red)
- test(zo): pin no repaint without a change, no idle ticker, flat bytes per delta, and one reader of the terminal (t-11961)
- test(zo): a turn's watcher reads nothing the session read, and a quiet store is not listed again (t-11961, red)
- test(zo): a frame reads no model catalog, and the roster watcher reads no unchanged manifest (t-11961, red)
- test(zo): the terminal's input stream is asked only when it has woken us (t-11961, red)

### release
- release: the Esc-with-a-pending-steer e2e leaves the flake list — it was a race in the runtime, and t-11961 fixed it

### merge
- merge: carry main's native pane and sidebar fixes into ledger delegation (t-11962)

### measure
- measure(zo): the final table on a quiet machine — zo against main, Claude Code, Codex and Gemini CLI (t-11961)

### other
- Merge branch 'wt/t-12077/n' (ca4f7b37) — a workspace board card's 「+N」 is a button that opens the other agents inside the card and folds them again. Before, the card showed its first three agents and a dead word for the rest, and pressing it only closed the board. Now it opens every agent in the card and turns into 「접기」; Tab reaches it, Enter and Space press it, and the hand stays on it when the card is drawn again. It flips the same open state as the sidebar's twist through one function and speaks through the twist's own dresser (aria-expanded, a spoken name and tip in five languages); the three is a named constant. Red first (ecc2b5df): the new check failed on a SPAN that was no button, and the press closed the board under the checks after it (14 of 19 passing); on the merged tree the workspace-board suite passes 33 of 33. The coordinator's gate found one follow-up and fixed it on the branch (ca4f7b37): the new button's hover changed colour with no transition, so the contract that holds every hover state to an eased rule failed on it (535 of 536) — it joins the window's shared motion list, and source_contracts passes 536 of 536. shell-lint and pii-check exit 0. Verified by the coordinator on 8b2f3470 with both night branches merged (6b3ca605); the worker ran no build. The full gate is the release lane of v1.1.39. (t-12077; coordinator review)
- Merge branch 'wt/t-11645/grok-2-2-17-4-436' (1e9c173c) — an expired Grok login is read on the ordinary cadence instead of on every two-second ask, and the accounts card says the login expired. The Grok gauge keeps a held reading only under the account its session file names; the expired-session answer named no account, so every ask dropped the last answer, read again past the refetch floor and the failure backoff, and answered `fetching`, which kept the window asking on its two-second tick — the window's log held 4,436 such reads in 2 h 17 min on 2026-09-28. The expired answer now carries the session's account, as every other answer off a held session already did; the card's existing 「로그인 만료」 line (five catalogs) shows. The door takes its clock and the Grok read its home so a test can walk an hour of asks on a stopped clock. Red first (b60c6d8d): the named test failed at its own assertion; on the merged tree the test passes, the usage group passes 94 of 94, the window's two label contracts pass, source_contracts 536 of 536, shell-lint and pii-check exit 0. Verified by the coordinator on 8b2f3470 with both night branches merged (6b3ca605); the worker ran no build. The full gate is the release lane of v1.1.39. (t-11645; coordinator review)
- Merge branch 'wt/t-11961/zo-cli-streaming-cpu-input-lag-and-memor' (3c58fa25) — zo stops spending its main thread where nothing asked for it, so a key typed while an answer streams reaches the screen sooner and a waiting or idle zo costs less CPU. The worker sampled a release build with `sample` through the benchmark's scenario and fixed five causes, each after a red test. The turn loop polled crossterm's input stream on every wake, and each poll fell into parking_lot's slow path and yielded seven times: 43.5% of the main thread's samples while waiting for the model and 25.5% while streaming sat in `swtch_pri`; the stream is now asked only after its own waker fired, and no main-thread sample lands in `swtch_pri` in any state (37549681). Every paint read files: the `/fast` check resolved the model catalog from disk, and every draw opened `/dev/tty` for the terminal size; the catalog is now asked only while the composer could list `/fast`, and the size comes from one `tcgetwinsize` ioctl on stdout (547f86f0, 574eadb2), which leaves no `__open` on the main thread. The roster watcher re-read the whole agent store every second, twice per turn; with the 432 manifests of the person's project seeded into a hermetic run, idle CPU was 2.6% against 0% with an empty store. It now keeps a stamp per file, shares one table across watchers, leaves a quiet store unlisted except every 30th scan, and remembers a settled manifest by its stamp instead of its body (547f86f0, 9023aa00, 7116c52f; the bodies had taken idle RSS from 18.3 to 19.5 MB). A table held the rest of an answer until the stream ended, so the text after it folded out of sight and arrived in one 6,753-byte burst, and every line after a table re-rendered the answer from the table on; a blank line outside a fence now closes the table and releases it, an open table is held as before, and the streamed bytes still equal the single-shot render (022956b8). The gate found a race main has too: Esc during a tool with a steer pending could fold the steer into the cancelled tool's result and send another request, because the tool cancel woke the turn before the 25 ms stop check. A tool cancelled together with the turn's stop now ends the turn at that tool and the steer starts a fresh turn (41f9e16e); `e2e_esc_with_pending_steer_resubmits_it_as_a_fresh_turn` failed 7 of 15 on main and 0 of 15 on the fix at load 29 to 71, so the next commit takes it off the release lane's flake list. The pane watcher that spawns `tmux list-panes` four times a second per child pane is named for t-11458 and left alone here. The branch adds the benchmark, `just tui-bench` in zo-ide: one local model service speaks the Anthropic, OpenAI Responses and Gemini wires with the same scripted replies to every CLI in a 120×40 pty with a hermetic home, and stops if any credential but its dummy key arrives. The final benchmark ran on a quiet machine (00:29 to 01:17, load 3.6 to 11.4, the CLI order rotated each round, the median of five runs each) against the merge base 49d332a8. CPU falls 98% at idle (0.86 to 0.02%), 78% while waiting for the model (1.98 to 0.43%) and 66% while streaming (2.86 to 0.98%), and zo now uses less CPU than Claude Code, Codex and Gemini in every state (Gemini ties it at idle, 0.01%). Keystroke to echo while streaming goes from 0.16 to 0.11 ms at p50 and from 0.23 to 0.19 ms at p95, against 1.66 and 2.46 ms for Claude Code, 2.01 and 8.78 for Codex, and 4.66 and 8.77 for Gemini. Memory and cold start stay where they were: 18 MB idle and 38 MB at peak against 188 to 1,043 MB for the others, and 114 ms to a ready composer against 222 to 932 ms. The last delta reaches the screen in 0.2 ms instead of 6.1, and with a store of 2,875 helpers zo's idle CPU goes from 6.1% to 0. The table is recorded in 3c58fa25, an empty commit on the gated tree. On the final head zo-ide's fmt, doc and clippy over all targets exit 0, `just test` passes 7,191 with none failing in 51 binaries, `just e2e` passes 111, and root pii-check exits 0. The combined tree is gated once by the v1.1.39 release lane, every `just verify` recipe and the zo workspace's gate, before it is pushed. (t-11961; coordinator review)
- Merge branch 'wt/t-12443/3d-v4' (2ec84ec0) — the knowledge graph's whole map opens as a universe where WebGL2 stands, the prototype v4 「실제 우주」 the person approved on 2026-09-28 ("지금 화면 좋아"): a topic is a galaxy (barred spiral, spiral, elliptical or irregular by its activity), a small group is a cluster or a nebula, a page is a named star whose brightness is its link count and whose colour is how recently it changed, a pair of topics tied by eight lines or more is a filament that flows from the side that sent more, with bloom and tone mapping through a half-float target, and a light theme that prints the same scene as a negative. Picking, hovering, searching, paths, clusters, the inspector (the six thickest filaments, a galaxy's shape, pages and two-week share, a page's "how long ago"), name plates, filament counts, page names, the tip, the legend's four universe rows and a 「움직임」 switch in the view menu all stand in 3D; relation editing and exploring nearby drop to the flat map at the same point; 2D | 3D in the head moves between the two and the vault remembers the choice; a resting universe draws no frame, a hidden pane draws none, and a lost context folds to the flat map in one line. Every number is a --knowledge-3d-* token at the prototype's value; three.js r160 gains one export (PerspectiveCamera, rebuilt byte for byte first). The branch carries the flat map's six cards (t-12029, 0b88f912) and, beside them, one rule for both painters' topic plates over stray points, which made the two painters seat page labels differently on six of seven canvas sizes (7 of 7 alike after), and the 2D→3D race in which a 3D press inside the 1.3 s fold left the pane flat. Measured in WebKit at 1749×1194 against the prototype's own table: 13 draws, frame work at 10,000 pages 1 ms median and 2 ms p95, a 29.2 MB framebuffer, first 3D frame 72/79/108 ms at 850/5,000/10,000 pages, 0 frames in three idle seconds, a flat 60 s heap; on the person's vault, read as numbers only, 867 pages stand in 18 galaxies (the prototype's 18), one cluster and 33 filament pairs. Design review by the prototype's own author closed all eight pieces (t-12138). Red first, piece by piece: the universe suite 0 of 42 before and 42 of 42 after; the whole knowledge harness passes 154 of 154 and the window suite 1906 of 1906 on 2ec84ec0 at a load of about 10; pii-check and tools-test exit 0; the window's source contracts run in the release lane of v1.1.39. In the combined tree `ui/tokens.css` met R2's paper-theme addition beside the nebula line: both stand, R2's `--artifact-new-ink` with its comment and this branch's `--knowledge-nebula-core: 0.05` with its own comment. (t-12443, t-12029, t-12138; coordinator review)
- Merge branch 'wt/t-11959/artifacts-r3-feedback-record-and-artifac' (151b4a1f) — the notes a person sends on an artifact are kept with the version they were made on, and the artifact's header band leads with them. When a draft of annotations reaches the pane the person chose, by either road (pasted into a running agent, or handed to a new one without submitting), `artifact_feedback_record` appends one line to `pages/<id>/feedback.jsonl`: the id, the version the notes were made on, that version's SHA and editable source as the store's own metadata names them, the receiving pane and agent, and the notes. The ask is strict and bounded (a held publication, a version from 1 to the row's own, 1 to 50 notes, selectors up to 1,024 characters and comments up to 4,000 without control characters, 64 KiB a line and 1 MiB a file); it is written under the catalog lock in one appending write, a torn last line is closed and never erased, and the command is refused from any webview but the main one. Rows carry `feedback_count` and `feedback_version`, which the catalog never writes down, and the file is never read as a version, so listing, versions, the door's list and export do not see it. The band's first line is the title, 「피드백 N · vN」 and 「주석」 as the primary action, then 「나란히」 (the version before the one on screen, beside it, both immutable snapshots), 「내보내기」 (the version on screen written into a folder the person picks, never overwriting, its notice saying a file on this machine), 「공유」 and Finder; its second line is the maker line and the version picker; under 480 px of its own width the band folds the four secondary actions into one 「⋯」 menu that acts through the same buttons. A republish now moves an open tab to the new version's snapshot instead of the mutable current copy, as the design notes require (R2's beside scene changed on purpose, red 14 of 15 first). The worker's red tests failed first on ae18a7b0 (the store 27 of 30, the band suite 2 of 4); on 94977db2 root clippy over all targets exits 0, source contracts pass 536 of 536, the window crate's unit tests 2439 and the core's 2206 with none failing, and the band, beside and gallery suites 81 of 81. The shared window flow ran 987 of 988 there: its annotation-bundle check still waited for `send_prompt`, although R2 had moved annotation drafts to `term_paste` on purpose, so the check fails on R2's head as well. 151b4a1f changes only that check, which now reads the bundle from `term_paste` into the chosen pane, requires that no `send_prompt` happened and that no line break trails it, and keeps every word it checked before; the flow then runs 988 of 988. Dark and light screenshots show the band wide, folded and side by side, with axe passing in all six. The combined tree is gated once by the v1.1.39 release lane, every `just verify` recipe and the zo workspace's gate, before it is pushed. (t-11959; coordinator review)
- Merge branch 'wt/t-11958/artifacts-r2-unseen-badges-and-opening-b' (25ade2c0) — an artifact an agent publishes is now seen where it was made. Before, a publish only refreshed the gallery and nothing told the person, and a note drawn on an artifact went into a draft for whichever pane had focus. The door's publish now hands the window its row (`artifacts:published`). A first publish from a visible maker pane that is not a background worker opens beside it, in the right neighbour group, else in a split of a pane at least 720 px wide, else as a tab of the maker's group, with focus left in the maker pane and a hint line that offers 「자동으로 열지 않기」 and 「되돌리기」; a republish updates its open tab in place; any other publish raises 「새 N」 on the maker pane's tab and in the sidebar, with one status-bar line only when the maker pane is off screen. A new setting, `artifacts_auto_open_beside` (on by default, in Settings › Artifacts), turns the opening off, and unseen state survives in localStorage. Annotations now list 「만든 에이전트」 first and focused, and their draft is pasted after what the person typed through `term_paste`, with no Enter and no clearing keys. `.browser-view` no longer lets the toolbar's 466 px min-content widen a 400 px group (the native page was laid 66 px over its neighbour), and the header band wraps so its title survives at 400 px. The colours are the theme's own tokens (`--artifact-new-ink` mixes the tasks hue darker on paper, where it read 3.73:1 before), with no glow, blur or looping motion. The worker's red suite failed first (artifact-beside 1 of 15 on 30dc6721); on the merged head source contracts pass 536 of 536, the focused browser suites (artifact-beside, artifact-gallery, zo-restore, jev-dashboard, browser-recovery, browser-panes-survive, emulator-seat) 181 of 181 at load 25 to 28, settings 139 of 139, and clippy on zerocode-shell over all targets exits 0. Those suites leave out the shared window flow, whose annotation-bundle check still waits for `send_prompt` and so fails on this head alone; the next merge (t-11959, 151b4a1f) makes it read the paste. The combined tree is gated once by the v1.1.39 release lane, every `just verify` recipe and the zo workspace's gate, before it is pushed. (t-11958; coordinator review)
- Merge branch 'wt/t-11962/zo-summons-through-the-ledger-like-claud-2' (33cd9180) — zo's Agent, SpawnMultiAgent members and Workflow phases now summon through the ledger, as Claude and Codex coordinators do. With the team grant and the zerocode-orc door present, each launch carries its exact catalog agent, model, effort and worktree through task-create and worker-start; the window door owns the CLI, pane, tab, checkout and parent edge; and the result comes back in the shapes zo already had (the blocking result, GetAgentCompletion, the background notification), with commit hashes and report pointers kept. There is no provider, model or native fallback: a pin the catalog cannot honor is refused, and a host without the new lifecycle query returns an update blocker before any worker opens. Two native projections are kept from misreading these workers: helper frames omit ledger manifests (a duplicate row stood under the parent), and native API activity reads unknown (Workflow otherwise stopped an external CLI after four minutes of empty stream counters). In the core, `dispatch-show` exposes an attempt's latest worker_done, worker_died or quota_walled fact without consuming the coordinator's delivery, and `worker-stop --dispatch <attempt>` refuses a worker that has been reassigned, so a cancel cannot end another task on that terminal; the orchestration skill's zo delegation section says so. The worker's red tests failed first (the schema, the helper row, the native watchdog, the lifecycle projection and the conditional stop, 0 of 1 each). On the final head: root clippy over all targets exits 0, core orchestration passes 417 of 417, the shell's tests 2959 of 2959 with source contracts 535 of 535, zo's e2e 112 of 112, and the 13 adapter tests plus a three-tool ledger e2e that drives a real zo against a scripted provider and a stand-in zerocode-orc; zo verify's three failures there (wiki indexing 1746 ms against its 1500 ms bound, session search 416 ms against 300, the silent pane shrink) passed alone at load 24 with no bound changed, board-orbit passed alone 89 of 89 three times, and root and zo win-check exit 0 on this head (5 min 18 s and 1 min 39 s, their warnings all in files it does not touch). The combined tree is gated once by the v1.1.39 release lane, every `just verify` recipe and the zo workspace's gate, before it is pushed. Left: the live six-pair smoke (zo, Claude and Codex summoning each other on haiku and gpt-6-luna), which needs a window that carries this core change. (t-11962; coordinator review)
- Merge commit 'fcb42a2a' into wt/t-11958/artifacts-r2-unseen-badges-and-opening-b
- merge main 4e2e1299 (t-12063, Jev dashboard fix) into t-12443
- Merge branch 'wt/t-12029/v2' (0b88f912) into wt/t-12443/3d-v4 — the painters' pixel comparison enlarges the map's sizes too (t-12443)
- Merge branch 'wt/t-12029/v2' (9232103a) into wt/t-12443/3d-v4 — the flat map's last commits: a small pane's margin, the ghost point's pointer and their tests (t-12443)
- Merge branch 'wt/t-12029/v2' into wt/t-12443/3d-v4 — the flat map's six cards are the base the 3D universe view stands on (t-12443)
- Merge main 49d332a8 into wt/t-11961 (t-11458, t-11827, t-11753 landed)

## [1.1.38] — 2026-09-28

_since v1.1.37 (29 commits)_

### feat
- feat(window): the Jev card says, feature by feature, whether it acts or still records and how far along it is (t-11989)
- feat(artifacts): a publication records the pane, folder and seat it came from, and a printed file:// URL opens as the artifact it names (t-11957)
- feat(window): open artifacts on pages and make the gallery searchable by origin (t-11960)

### fix
- fix(jev): a narrow window's cards hold a judged window of a hundred without their figures running together
- fix(window): a resume line a person types is spelled only from plainly safe words, never quoted and trusted (t-12063)
- fix(window): resume_session's eighth argument is its wire, said the way the other commands say it (t-12063)
- fix(window): cmd re-exports resume_line beside resume_session, so the handler list can name it (t-12063)
- fix(jev): a seat that starts acting falls on its marks alone (t-11989)
- fix(jev): the summons' two seats act under the one switch, and their judge still stops them (t-11989)
- fix(window): the way back a stand-in pane names works from any shell — led by a cd into the conversation's workspace (t-12063)
- fix(window): every zo pane a restart brings back wakes in its own workspace, and one that cannot says so (t-12063)
- fix(zo): a zo that cannot open its session says why on its terminal, not only in its log (t-12063)

### docs
- docs(orchestration): the pinned-worker sentence keeps the words its contract reads (t-11989)
- docs(orchestration): coordinators leave the dials to Jev and may leave the agent to it (t-11989)

### ui
- ui(board): the relations graph's agent card speaks Orbit's grammar, and a spawn line is a rail lit by its child's state (t-11956)

### other
- Merge branch 'wt/jev-dashboard/narrow-figures' (96524ed4) — the Jev dashboard's narrow cards hold a judged window of a hundred: a figure that outgrows its column takes a second line between its facts instead of running under the cost beside it, and the narrow check reads fixed short and long figures instead of this machine's count, which had turned the browser gate red or green with whatever Jev counted at the moment (1896 of 1898 at 19:08, 86 of 86 five minutes later). Red first: 86 of 88 with the test change alone; 88 of 88 with the fix, on the machine's numbers and on a stand-in zo feeding the long figure; source_contracts 535 of 535; lint and pii 0. Coordinator gate: run whole on 2df0d9d0, this tree together with the knowledge graph's flat map (t-12029), which was taken out before landing because two of its painter parity checks fail alone as they fail in the harness; the two trees differ in eight files under ui/ and nowhere else. On 2df0d9d0: fmt, lint of the workspace and of the window, doc, workspace test, tools, the window runner, zo's fmt, doc, clippy, test and e2e, and the Windows cross-check of root and of zo. On the landing tree (0fc3125d), what reads ui/ again: pii, the window's bins and source contracts, and the settings, window and knowledge harnesses. All 0 but for what is named here. 2 test name(s) failed once in a parallel run on a loaded machine and passed alone three times each on the landing tree. They are judged as the lane judges them. (run-11955; coordinator review)
- Merge branch 'wt/t-12063/zo' (2de834fe) — a zo conversation that was open when the window closed comes back when the window is started again, whichever workspace stands in front: the window wakes a conversation in the workspace its tab belongs to, a folder it checks against its own catalog, instead of the root that happened to be active, where zo looked in the wrong project store and ended with "session not found". A pane that cannot be woken says why on its own screen, in zo's words, with the command that carries the conversation on; that command is spelled by the backend from words it has validated (an id or a folder holding anything outside letters, digits and _ . : / - is refused, as is a folder the catalog does not know), and the sentence drawn on the screen is stripped of control characters and held to 1,024 characters. A new conversation typed where an old one was refused no longer erases the old one from the layout file: the tab keeps it as owed, the newest eight, and says so in one line. A live zo pane that no record holds at closing time is written down by the window itself. zo says one line on the terminal when it cannot start. In the harness, with two workspaces and three zo panes and the other workspace in front, one pane of three came back before and three of three after; the new suite failed five of five checks on the red commit and passes six of six. Coordinator gate: run whole on 2df0d9d0, this tree together with the knowledge graph's flat map (t-12029), which was taken out before landing because two of its painter parity checks fail alone as they fail in the harness; the two trees differ in eight files under ui/ and nowhere else. On 2df0d9d0: fmt, lint of the workspace and of the window, doc, workspace test, tools, the window runner, zo's fmt, doc, clippy, test and e2e, and the Windows cross-check of root and of zo. On the landing tree (0fc3125d), what reads ui/ again: pii, the window's bins and source contracts, and the settings, window and knowledge harnesses. All 0 but for what is named here. 2 test name(s) failed once in a parallel run on a loaded machine and passed alone three times each on the landing tree. They are judged as the lane judges them. (t-12063; coordinator review)
- Merge branch 'wt/t-11989/jev-switch-on-summons-seats-pick-worker' (10474850) — under the settings card's one Jev switch, the summons' difficulty question and its agent choice act instead of only recording. A summons whose dials are left open launches on the profile row of Jev's answer, and its row says the answer was carried out; `--agent auto` is chosen for under the consent the window observed for the summoning pane and lands on the agent chosen; a seat that starts acting is taken back only on its own marks, and for these two a fall is for good. Before, every row of this machine's difficulty ledger said applied:false with its judge at too_few_compared 0 of 30, and `--agent auto` had three dead roads (the live catalog never answered choose_agent, the ask carried no workspace so the door refused it, and the seat recorded). The settings card now lists each feature's standing, and AGENTS.md and the orchestration skill tell coordinators to leave the dials to Jev unless the person named them. The worker's red commits failed first (42c0f305: core 4 of 7, window 4 of 17; 84c77dd4: 2 of 2), and its live check B on the real Jev answered low at confidence 1.0, applied, and launched opus at medium, with `--agent auto` landing on claude. The landing gate on the merged tree caught two follow-ups, which the coordinator fixed on the branch: a5d0d9e0 keeps the guide's pinned-worker sentence in the words source_contracts reads (534 of 535 before), and 10474850 makes zo's jev_summary test read the agent choice as acting (495 of 496 before). Gate on this tree (45942fe3 on 448f8de9), 18:36 to 19:08: pii, fmt, lint, doc, win-check and window-runner-test exit 0; shell-test passed 2971 of 2972 on the pre-fix tree, the one failure being the guide sentence, and on the final tree shell-lint exits 0, source_contracts passes 535 of 535 and core orchestration 448 of 448; the workspace tests pass 3354 with one failure, hookd's a_codex_without_app_server_is_unsupported race on a path this landing does not touch, which passed alone three times of three at a load of 31 to 35; zo-ide's tools library passes 1804 and the zo-ide library 1062 with none failing; window-browser-test passes 1896 of 1898: both failures are the Jev dashboard's narrow-window check (dark and light), where this machine's live figure from the installed zo 1.1.36 read 98/100 (93%) and overlapped the cost beside it at 319 px. That fault is main's own and follows the data, not this landing: a stand-in zo that lengthens only that figure fails the same two checks on this tree and on 448f8de9's ui alike (84 of 86 each), while this tree passes 86 of 86 with the figure the machine shows now (80/80); the layout fix is a follow-up. Left: live check A after a window restart, and an actor stall of up to 10 s when Jev is down (follow-up). (t-11989; coordinator review)
- Merge branch 'wt/t-11957/artifacts-r1-provenance-on-publish-and-f' (17eb0ec6) — a publication records where it came from, and the address the Artifact tool prints is a link. The artifact door sends the pane key its shell carries beside the folder it already sent (the POSIX door and its PowerShell twin, this door alone), and the window turns the two into the row's origin: the pane, the kept project and the deepest worktree that holds the folder in git's spelling, the agent seated there, and the ledger seat's model, run, worker and task only while that seat's agent is the pane's; a publish from a pane the window does not hold records nothing. The gallery's maker line, the drawer's card, task and worktree buttons and the sidebar's worktree chip now have an origin to read. A printed file:// URL, %20 and all, is a terminal link, and a path under the store's pages/<id>/ opens as that publication at the version it names. Before, every page the tool published was stored with an empty origin; the new tests failed first (core 2 of 10, hookd 1, the window crate 6, the gallery suite 37 of 39, the window suite 972 of 977 with both file:// and both provenance checks red) and pass after. A publish from a live pane through this branch's door recorded run-11955, t-11957, w-11966, term-30, claude and opus with the pane's worktree and project, and the worktree chip counted it. The merge met the gallery's changes in the listing struct and in two test files and keeps both sides: the page-address struct beside the gallery's listing counts, and the provenance test beside the first-screen and recall tests, with the studio-fold test retired as the gallery's menu retired the fold. Gated together as 37462e0c on v1.1.37 (fe9b214c) from 17:49 to 18:27 at a load of 24 to 78: pii-check, fmt-check, shell-lint, lint, doc and win-check exit 0; shell-test passes 2966 with none failing, window-runner-test 10 of 10 and window-browser-test 1898 of 1898; test passes 3346 and fails one, hookd's a_codex_without_app_server_is_unsupported, where the fake old codex's exit outran its stderr ("codex app-server exited: before answering" instead of unsupported), a race on a path this landing does not touch (hookd gains only a bridge test), and that test passed alone three times of three at a load of 43. (t-11957; coordinator review)
- Merge branch 'wt/t-11956/agent-graph-card-d-settle-board-orbit-an' (041a5fab) — an agent card in the relations graph reads the way Orbit's labels do (direction D of the 2026-09-28 card study): a tile in the model family's tones, the title and the model on the first row; a state dot and word, the branch, 「하위 N」 and a small context ring on the second; under a hairline the Now line (verb, target and one age, or 「N분째 조용」 when the pane has gone quiet), the trail and the last message. A question is clamped to two lines and followed by 「답하기 term:N →」, which opens what the inspector's 「터미널 열기」 opens. The card is opaque with a 2px edge in its state's colour, the working dot no longer spins, and the state bubble speaks Orbit's words (「확인 필요 !」, 「✓ 완료」, 「✕ 실패」, 「z z」) from the one function both views now read. A spawn line is a rail, lit while the child works or waits on a person, that ends in a bead of the child's state colour. The board-orbit suite passes 89 of 89 alone three times at a load of 16 to 31; side by side under the same load (76 to 131) the 3D view drew 0.59 to 0.70 frames a second against main's 0.50 to 0.64, so the shared bubble function costs it nothing. Gated together as 37462e0c on v1.1.37 (fe9b214c) from 17:49 to 18:27 at a load of 24 to 78: pii-check, fmt-check, shell-lint, lint, doc and win-check exit 0; shell-test passes 2966 with none failing, window-runner-test 10 of 10 and window-browser-test 1898 of 1898; test passes 3346 and fails one, hookd's a_codex_without_app_server_is_unsupported, where the fake old codex's exit outran its stderr ("codex app-server exited: before answering" instead of unsupported), a race on a path this landing does not touch (hookd gains only a bridge test), and that test passed alone three times of three at a load of 43. (t-11956; coordinator review)
- Merge branch 'wt/t-11960/artifacts-gallery-first-screen-and-filte' (c747bd43) — the artifacts gallery opens on the pages a person published instead of burying them under worker reports and evidence: the kinds stand in page, report and evidence tabs that carry their own counts, and a kind with no rows has no tab; creation folds from a banner into one menu; the list narrows by project, agent and period. Cards are grouped by recency and name their maker only with what the row's origin carries (the agent's mark, the pane, the branch, and no line at all when the origin is empty, so 「출처 없음」 stays the drawer's word), and a row with feedback wears its count. The runtime counts whole tabs and missing files beyond the listing's row cap; a row whose file is gone is hidden by default behind one 「사라진 파일 N행 숨김 · 보기」 line; a page's drawer shows the page's own thumbnail instead of 「이 종류는 미리보기가 없습니다」. At 1280×860 the gallery shows four complete cards over two full rows in both themes, against one full card before, and the gallery suite passes 47 of 47 where the focused run on the old gallery failed its first-screen and recall checks. Gated together as 37462e0c on v1.1.37 (fe9b214c) from 17:49 to 18:27 at a load of 24 to 78: pii-check, fmt-check, shell-lint, lint, doc and win-check exit 0; shell-test passes 2966 with none failing, window-runner-test 10 of 10 and window-browser-test 1898 of 1898; test passes 3346 and fails one, hookd's a_codex_without_app_server_is_unsupported, where the fake old codex's exit outran its stderr ("codex app-server exited: before answering" instead of unsupported), a race on a path this landing does not touch (hookd gains only a bridge test), and that test passed alone three times of three at a load of 43. (t-11960; coordinator review)
- Merge branch 'main' into wt/t-11989/jev-switch-on-summons-seats-pick-worker

## [1.1.37] — 2026-09-28

_since v1.1.36 (40 commits)_

### fix
- fix(zo): the mid-turn release arm reads before the close it stands aside from (t-11753)
- fix(window): every agent a zo session starts can be reached, opened and walked by keyboard (t-11827)
- fix(window): a finished agent that ran in a pane of its own folds with the rest (t-11753)
- fix(zo): a pane child waiting on a person is not quiet (t-11458)
- fix(zo): a teammate's pane is released once its answer is on the manifest, unless a person has touched it (t-11753)
- fix(zo): a pane child is ended for going quiet, not for working an hour (t-11458)
- fix(window): a failed typing request leaves the device's words out of the window log (t-11740)
- fix(window): the Korean-input husk writes the typing's shape, and old dumps are withdrawn at boot (t-11740)
- fix(core): names_the_turn asks is_multiple_of (clippy manual_is_multiple_of) (t-11540)
- fix(board): a prompt names its pane only once its own record says a person typed it (t-11540)
- fix(shell): a person's tab whose turn the restart cut is told once to go on (t-11537 C)
- fix(shell): the census tests name the goodbye's rest in every note they build (t-11548)
- fix(shell): after a restart a resting worker hears of its mail, and a working coordinator goes on without being spoken to (t-11548 t-11537)
- fix(board): the main pane shows what it is doing — a go-on word or the mail pointer never names an agent (t-11540)
- fix(core): one list of the words that only say go on, and the newest prompt that names a turn (t-11540)

### perf
- perf(zo): a pane child still writing is not asked whether it waits on a person (t-11458)

### docs
- docs(core): names_the_turn names its private word list without a link (t-11540)
- docs(core): a public reader names the private voice test without a link (t-11540)

### other
- Merge branch 'wt/t-11827/zo' (e203fb03) — every agent a zo session starts can be reached in the sidebar, opens when it is pressed, and folds away when it has finished: a row of an agent that has no pane opens that agent's own conversation (and says so in one line, in five languages, when there is no record to open) instead of going to the zo pane already in front; the tooltip of a row stands beside the row, not over the next two; the rows are walked by keyboard (arrows, Home, End, Enter) without the workspace list or the terminal taking the key, and each row carries a spoken name. A finished agent that ran in a pane of its own — zo's pane mode and a Claude Code teammate alike — folds into the same 'finished' line as the ones that had none, and zo releases a teammate's pane once its answer is on the manifest, unless a person has pressed a key in it. A new browser suite holds the five cases (1 of 5 before, 5 of 5 after). Coordinator gate on the whole landing (8b8c70ed): pii, fmt, lint of the workspace and of the window, doc, workspace test, shell bins and source contracts, tools, the window runner, the settings, window and knowledge harnesses, zo's fmt, doc, clippy, test and e2e, and the Windows cross-check of root and of zo, all 0 but for what is named here. 1 browser suite(s) held a check that failed on the gated tree — the session-row check still expected a finished pane to keep a row of its own — and passed alone three times with the coordinator's commit that reads it after opening the folded line (fbc0f9c2). They are judged as the lane judges them. (t-11827, t-11753; coordinator review)
- Merge branch 'wt/t-11458/zo-pane-budget-60-t-11354-2' (aa03f3d9) — an agent zo started in a pane of its own is no longer ended for having worked an hour: the parent's wait for one pane turn counts quiet, not age — the time since the child's transcript was last seen to change, timed on the parent's monotonic clock so a shut lid is not read as idleness — and a child with a question to a person open (a permission prompt or a question, read off one new field of the channel's session list, asked only of a child quiet for ten seconds) is not quiet until it is answered. A limit somebody named, by the caller's time budget or the settings key that meant a wall clock before, is still a wall clock, and the words that close a pane say which limit it was, in the clock words the quota notices use. Before: a child writing a line every minute was ended at 3600 s (TimedOut), and one waiting ninety minutes on a person was ended at sixty; both reds reproduced it. The window's HUD reads the child's transcript through the same function the wait does. Coordinator gate on the whole landing (8b8c70ed): pii, fmt, lint of the workspace and of the window, doc, workspace test, shell bins and source contracts, tools, the window runner, the settings, window and knowledge harnesses, zo's fmt, doc, clippy, test and e2e, and the Windows cross-check of root and of zo, all 0 but for what is named here. 1 browser suite(s) held a check that failed on the gated tree — the session-row check still expected a finished pane to keep a row of its own — and passed alone three times with the coordinator's commit that reads it after opening the folded line (fbc0f9c2). They are judged as the lane judges them. (t-11458; coordinator review)
- Merge branch 'wt/t-11740' (4e49e5d0) — the window's log no longer keeps what a person typed: the diagnostic the window writes when a bare jamo leaves for the terminal now records the shape of each character (which of initial, vowel and final it carries; every other character a dot, the count kept) instead of the hangul itself, and the dumps already written are rewritten once at the next start, in the log and its rotated copy, by a temporary file and a rename, without holding the start if it fails. A second road found by reading every call that writes the log: a failed text entry on an emulator quoted the device's error, which quoted the text — it now writes a fixed sentence. A source contract holds both. On this machine's log the rewrite turned 13 dumps and 2,600 trace lines into lines with no hangul (counted, not read). Coordinator gate on the whole landing (813f33b5): pii, fmt, lint of the workspace and of the window, doc, workspace test, shell bins and source contracts, tools, the window runner, the settings, window and knowledge harnesses, the broken-commit input harness, and the Windows cross-check of root, all 0 but for what is named here; zo's tree is main's, which the release lane gated. The window's test that no label is written into the window failed on the merged tree (four comments beside quoted marks named a syllable's parts in hangul) and passes with the coordinator's commit that rewrites the comments (f6295600). 1 unlisted test name(s) failed once in the parallel run, beside a release lane, and passed alone three times each. 1 browser suite(s) held a check that failed once and passed alone three times. They are judged as the lane judges them. (t-11740; coordinator review)
- Merge branch 'wt/t-11540' (4c3304c8) — the main pane is named by what a person last asked of it, never by a word that only says go on, by the window's own mail pointer, or by a prompt its CLI sent itself: one list of go-on words in five languages (a prompt that is only such a word, repeated or punctuated, names nothing; a sentence that begins with one still names its turn), one function that says whether a prompt names its turn, and the newest naming prompt read off the conversation's own record for a pane whose stored name was such a word. A prompt the hook reports is held until the conversation's record says who sent it — a scheduled check the agent set for itself is the CLI's, not a person's — and is then taken as the name once or dropped. The sidebar row, the board card, the 3D label, the at-a-glance line, the detail panel, the scope list and the tab read the name from one function. Measured: settling a prompt reads a 256 KiB window of the record, median 0.46 ms, against 21.5 ms for the 4 MiB read it replaces on that road. The board harness passes 92 of 92 with the new cases; before, the main pane of this window read '계속' on every surface. Coordinator gate on the whole landing (813f33b5): pii, fmt, lint of the workspace and of the window, doc, workspace test, shell bins and source contracts, tools, the window runner, the settings, window and knowledge harnesses, the broken-commit input harness, and the Windows cross-check of root, all 0 but for what is named here; zo's tree is main's, which the release lane gated. The window's test that no label is written into the window failed on the merged tree (four comments beside quoted marks named a syllable's parts in hangul) and passes with the coordinator's commit that rewrites the comments (f6295600). 1 unlisted test name(s) failed once in the parallel run, beside a release lane, and passed alone three times each. 1 browser suite(s) held a check that failed once and passed alone three times. They are judged as the lane judges them. (t-11540; coordinator review)
- Merge branch 'wt/t-11537' (b3acb5a8) — after the window restarts, a worker that was resting hears of its mail, and a coordinator or a person's own conversation that the restart cut goes on without being spoken to: the goodbye writes down the rest it heard of each worker's pane, the pane of a run's coordinator and any conversation whose turn was running, and the wake that brings a conversation back writes that rest into the window's empty turn table (so the mail pointer reaches a resumed Codex worker, whose resume reports nothing) or, where the run was still working or the turn was cut, places one continuation through the road a worker's continuation takes — once a restart, never beside a pointer, never for a pane a person's hand ended. A coordinator's continuation says what its run holds (workers carrying a dispatch, tasks dispatched, mail waiting, counted off the ledger) and that the checks it had scheduled in its session ended with the process. A person's tab is filed under a digest of its identity and its goodbye line names only the terminal and the turn's word. Before: on 2026-09-28 mail for a resumed Codex worker waited five minutes behind 'the window has heard nothing from this pane' until the coordinator replaced the worker, and the coordinator's own pane came back silent until the person spoke. Tests: the red commits fail at the named assertions (3 of 5, then 1 of 8), and the landing tree passes restore_wake 8 of 8, restart_census 11 of 11, restart_nudge 10 of 10 and the 325 orchestration tests, with a store closed and opened again in between. Coordinator gate on the whole landing (813f33b5): pii, fmt, lint of the workspace and of the window, doc, workspace test, shell bins and source contracts, tools, the window runner, the settings, window and knowledge harnesses, the broken-commit input harness, and the Windows cross-check of root, all 0 but for what is named here; zo's tree is main's, which the release lane gated. The window's test that no label is written into the window failed on the merged tree (four comments beside quoted marks named a syllable's parts in hangul) and passes with the coordinator's commit that rewrites the comments (f6295600). 1 unlisted test name(s) failed once in the parallel run, beside a release lane, and passed alone three times each. 1 browser suite(s) held a check that failed once and passed alone three times. They are judged as the lane judges them. (t-11537, t-11548; coordinator review)

## [1.1.36] — 2026-09-28

_since v1.1.35 (9 commits)_

### feat
- feat(knowledge): the rim of stray pages hugs the named discs, and GL plates may stand on it (t-11500)

### fix
- fix(knowledge): neighbouring clusters wear different hues (t-11500)
- fix(knowledge): cluster plates reserve the words they wear, and stand as one line before they fold (t-11500)
- fix(knowledge): GL local exploration wears the SVG's grammar (t-11500)
- fix(knowledge): switching the theme repaints the GL picture in the new theme (t-11500)

### other
- Merge branch 'wt/t-11500/gl' (ad5e73fe) — the knowledge graph's GL picture follows the theme, its local exploration reads as the SVG's does, and the map fills more of the screen: switching the theme repaints the points and lines in the new theme at once (the painter watches the theme, a frame that only moved the camera uploads the new colours, and the swatches it reads do not transition); local exploration under GL drops the overview's backdrop discs and tells spokes from neighbour lines, with the centre enlarged and haloed as the stylesheet says; a cluster's plate reserves the words it wears and stands as one line before it folds; the rim of stray pages starts at the named discs' outline rather than at a circle round the farthest one, and under GL a plate may stand over the rim's unnamed points; neighbouring clusters wear different hues. Measured on WebKit, the engine the app runs on: pixels differing between a switched theme and a fresh draw 38,278 and 58,535 → 0 (overview and local exploration); lines and points that disagreed with the SVG in local exploration 17 of 17 and 10 of 10 → 0; on a person's vault at zoom 1, plate cells outside what was reserved 17 → 0 and overlapping names 2 pairs → 0, the discs' share of the canvas 44% × 48% → 52% × 57%; neighbouring clusters of one hue 3 → 0. Coordinator gate on the landing (15eb7af1): the change is ui files and the window's list of browser built-ins, so it ran what reads them — pii, tools, the window runner, the settings, window and knowledge harnesses, the window's Rust tests and its source contracts — all 0 but for what is named here; the rest of the Rust tree is the one the release lane gated at 4df01fcb. The window's test of what it may call failed on the merged tree (DOMMatrixReadOnly was not listed) and passes with the coordinator's commit that lists it. 1 browser suite(s) held a check that failed once in the run, beside a release build, and passed alone three times; judged as the lane judges them. (t-11500; coordinator review)

## [1.1.35] — 2026-09-28

_since v1.1.34 (10 commits)_

### feat
- feat(jev): the dashboard finds a pane's folder zo never ran in, and says a missing request in words every seat can wear
- feat(jev): the claim and file pick seats are asked of every agent's panes, through the one layer

### fix
- fix(pane_guard): a claim question checks its seat is on before it reads a transcript (t-11349)

### ui
- ui(knowledge): zooming in names more of the map, and the rim's pages stop taking the overview's names

### other
- Merge branch 'wt/t-11349/jev-t-10916' (b79d5e98) — the completion claim and the file pick seats are asked of every agent in this window's panes, through the layer the two tool guards stand on: a person's prompt that is a code task asks the file pick of the window's own project search and the session's recent edits, and a turn that ends saying it is done or passing asks the claim check of what that turn's own calls returned; both are labelled as zo labels them (the files the turn edited, the person's next words), filed in the zo project ledger of the pane's folder with who asked, and judged in the one series. It records only: the hook's reply and the agent's work are untouched and no permission is decided. With all four seats off the hook loop reads nothing (p50 43 → 0 µs an envelope); the seats' modes are a snapshot the window's file watch refreshes when zo's settings change, and settings that cannot be read are off. Rows carry no words, only fingerprints and counts; a turn's evidence is scrubbed of credentials before it is kept, bounded, and dropped when the turn ends. zo's halves of both seats and the name zo gives a project's folder moved to core unchanged (move-only commits). The Jev tab counts both seats agent by agent and says why an agent cannot be seen (one that reports no prompt, in five languages). Measured: a replay over 13 agents was answered 22 of 22 and 16 of 16 (Jev p50 200 and 204 ms); the hook's round trip p50 15.48 → 15.90 ms. Coordinator gate on the whole landing (63863842): pii, fmt, lint of the workspace and of the window, doc, workspace test, shell bins and source contracts, tools, the window runner, the settings, window and knowledge harnesses, zo fmt, doc, clippy, test and e2e, and the Windows cross-checks of root and zo, all 0 but for what is named here. 1 unlisted test name(s) failed once in the parallel run, beside a release build, and passed alone three times each. They are judged as the lane judges them. (t-11349; coordinator review)
- Merge branch 'wt/knowledge/label-zoom' (68e1adbf) — the knowledge graph names more of the map as a person zooms in, and its overview stops naming the crumbs: the label budget grows with the zoom, from the covering zoom (1.5, a token) a name may stand over another page's point where the painter draws names above every point (the GL painter; never over another name, a plate or a control), and below it the pages and ghosts of unnamed clusters on the rim spend none of the budget. Measured on WebKit, the engine the app runs on, with a vault of 843 pages: names at twice the zoom 18 → 59 and at four times 28 → 56, clusters with a name at 1.5 and at 2 of 17 from 9 and 11 to 17, names on the rim's crumbs in the overview 9 → 0, no page error; the SVG painter, whose names sit under later points, keeps avoiding points. Written by a person's zo session and landed by the coordinator at the person's word. Coordinator gate on the whole landing (63863842): pii, fmt, lint of the workspace and of the window, doc, workspace test, shell bins and source contracts, tools, the window runner, the settings, window and knowledge harnesses, zo fmt, doc, clippy, test and e2e, and the Windows cross-checks of root and zo, all 0 but for what is named here. 1 unlisted test name(s) failed once in the parallel run, beside a release build, and passed alone three times each. They are judged as the lane judges them. (t-11493; coordinator review)

## [1.1.34] — 2026-09-28

_since v1.1.33 (53 commits)_

### feat
- feat(jev dashboard): who asked each guard, agent by agent, and whose work it cannot see
- feat(window): ask the tool guards of every agent pane's work, recording only
- feat(jev): the tool guards read every agent's hooks through one layer, and keep one ledger on the machine

### fix
- fix(zo): a person can talk to the main while its agents work — a turn left waiting on background results ends, and no wait holds their words (t-11354)
- fix(zo): login messages name only the ways back this build has (t-11378)
- fix(zo): only a turn a person attends waits for a Claude login; a headless or window-driven turn fails as before (t-11045)
- fix(zo): a turn whose Claude login is gone waits for one and goes on as the same request (t-11045)
- fix(zo): zo speaks as the Claude login its CLI keeps and never spends another tool's refresh token (t-11045)
- fix(core): read a KEY= value where it stands in the command, not a letter cut in two
- fix(zo): /compact runs off the screen's loop, and a declined long conversation is compacted once and asked again
- fix(board): finished work shows as finished — the sidebar card leads with its task and says where its review stands, and the task board keeps the card among the endings with the ledger's stage word (t-10993)
- fix(jev): grade a command on its own facts and a text on the order it held
- fix(jev): the mail triage passes the workspace lint — a batch is asked whether it holds a letter, a const pin is checked at compile time, and the rooms' notice names its attempt in one helper (t-9471)
- fix(jev): the mail triage stands twenty-ninth on a table that grew the summons' difficulty — the table's length, its pins and the seat audit place it (t-9471)

### docs
- docs(zo): two public notes name a private item in plain code, not as a link (t-11045)

### accounts
- accounts: a Claude account nobody runs is renewed by its own CLI, once, and then read (t-10915)

### orchestration
- orchestration: the mail triage reads a coordinator's turn as the pointer reads it (t-9471 meets t-11233)
- orchestration: a lead at rest beside its own background work is pointed at through its composer

### ui
- ui(knowledge): the knowledge graph's GL painter draws through the same three.js
- ui(board): the relations tab's 3D view draws the approved flow board on three.js
- ui(jev): the notify card says "at the computer", not the codebase's word for a seat (t-11010)

### other
- Merge branch 'wt/t-11378/zo-login-zo-login' (7c5cbf57) — zo's login messages name only the ways back this build has: a refused login sent a person to `/login` and `zo login`, neither of which exists (on 09-27 a person read that under a 401 whose token had been revoked). Every message now reads one table of where each provider's login is signed in again (Claude, ChatGPT, Gemini, Grok), a revoked token says to try again first because the newer login is already where zo reads it, an expired one says where to sign in, and a missing one names that provider's road alone. A new test reads the commands this build really registers and fails a message that names one it lacks; it found the same fault in three other sentences, fixed with it. Coordinator gate on the whole landing (741149ba, this branch and t-11354's commit after it, which change zo's workspace alone): pii, source contracts, tools, zo fmt, doc, clippy, test and e2e, and the Windows cross-checks of root and zo, all 0; the root's own labels stand on the gates of the two landings before it, whose files these do not touch. (t-11378; coordinator review)
- Merge branch 'wt/board-3d/threejs' (95530f85) — the relations tab's 3D view is the flow board a person approved on 09-27, drawn with three.js (WebGL2) instead of canvas 2D: the main workspace is a platform on the left and every other workspace an island to its right by lineage, each agent is a robot whose body colour says its model family and whose light says its state, instructions run left to right along glowing rails with comets where work is under way, fresh mail rides between sender and receiver, and a glance line says in one sentence what is flowing. Seats are stable (an arrival moves no one), platforms and name tags never overlap at 6, 20 and 60 agents or on a narrow board, frames stop while the view is hidden, held or under reduced motion, and draw calls do not grow with agents (42 for the scene, 54 with bloom). A window without WebGL2, or one that lost its context, stands on the card view and the 3D button says why. three.js r160 is vendored on the editor's road: one IIFE built offline by a pinned recipe, no worker, no blob and no console warning, shipped with its MIT notice. The knowledge graph's GL painter draws through the same library, so the window keeps one GL renderer (a before and after comparison of four scenes differs in 0 pixels). Measured on WebKit, the engine the app runs on, in the landing tree: 86 of 86 checks, 60 frames a second at 60 agents, frame script p95 1 ms. Written by a person's zo session and taken over by the coordinator at the person's word. Coordinator gate on the landing tree (1709631d): pii, fmt, lint, doc, workspace test, shell bins and source contracts, tools, the window runner, the settings and knowledge harnesses, and the Windows cross-check, all 0. The window harness passed 1869 of 1870 checks; the one that failed is a timing check of the terminal's drawing (terminal-throughput), which the branch does not touch, and its suite passed alone three times, judged as the lane judges a browser check. zo's workspace, which the branch does not touch, stands on the gate of the landing before it. (t-11368; coordinator review)
- Merge branch 'wt/t-11045/zo-claude' (7be3daa2) — zo no longer loses its Claude login while it works: a pane's zo read the shared runtime home's copy of the chosen account's login and renewed it itself, while the CLI in every pane renews the account's own folder — two spenders of one rotating refresh token, so one of them lost (89 refused renewals in one machine's log, then "Claude auth unavailable"). zo now reads the store the CLI keeps, by the CLI's own rule, and never renews, rewrites or copies a Claude Code login: when it has expired zo asks that store's own CLI once (headless, a slash command that calls no model) and reads again; zo's saved login is renewed only when zo minted it. When no login is left, a turn a person attends waits on one line and goes on as the same request once the person has signed in again (sub-agents too); a headless or window-driven turn fails at once, as before. The message names a way back this build has. Measured against a token server that takes a refresh token once, 20 rounds each: zo left without a login 20 → 0, the CLI's login lost 20 → 0, refused renewals 40 → 0. Landed as the worker's six commits replayed onto main, whose tree is the one they were written on. Coordinator gate on the whole landing: the full gate ran on 2aed9279 (pii, fmt, lint, doc, workspace test, shell bins and source contracts, settings and window harnesses, zo fmt, clippy and e2e, and the Windows cross-checks of root and zo, all 0, and zo test (every test binary run to its end) 0 but for the names judged alone below; the tools tests red on one test of the replay seed that read the guards' source words from the file they had left, and zo doc red on two public notes that linked to a private item), and with those two fixed pii, the tools tests, and zo fmt, doc and clippy were run whole on a63541cf, all 0. 1 test name(s) failed once in the parallel run and passed alone three times each, judged as the lane judges an unlisted flake. (t-11045; coordinator review)
- Merge branch 'wt/t-11233/2' (f8368817) — a coordinator at rest beside its own background work is told about its mail: a Stop that carries background tasks is held as working so the card does not ring a completion, and the window wrote the same thing down as a running turn, so mail for that pane was parked for a turn end that had already happened — on 09-27 a coordinator whose background shell watched a gate chain left its workers' questions unanswered for two hours. The held Stop is now the lead's rest to the window (the ledger still hears the turn end only when the parked all-clear replays), and a running turn nothing has spoken for in the stale bound the board already uses reads as at rest, so a turn end lost on a loaded machine no longer holds the mail either. The composer's other doors stand: nothing is typed over a person's words. Four new tests were red on the old behaviour. Coordinator gate on the whole landing: the full gate ran on 2aed9279 (pii, fmt, lint, doc, workspace test, shell bins and source contracts, settings and window harnesses, zo fmt, clippy and e2e, and the Windows cross-checks of root and zo, all 0, and zo test (every test binary run to its end) 0 but for the names judged alone below; the tools tests red on one test of the replay seed that read the guards' source words from the file they had left, and zo doc red on two public notes that linked to a private item), and with those two fixed pii, the tools tests, and zo fmt, doc and clippy were run whole on a63541cf, all 0. 1 test name(s) failed once in the parallel run and passed alone three times each, judged as the lane judges an unlisted flake. (t-11233; coordinator review)
- Merge branch 'wt/t-10916/jev-claude-code-codex-command-guard-tool' (dac11e77) — the command guard and the tool text guard are asked of every agent in this window's panes, not of zo alone: one layer in core reads the hooks the window already installs for an agent as the moments the two guards ask and grade at (a shell command about to run and one that ran, a tool's text handed back and the step after it, the person's prompt, a turn's end), one row an agent says which of those its hooks carry and why not where one is missing, and the window asks zo's own questions off the hook loop and files the rows in the machine's one ledger per seat, with who asked. It records only: the bridge has answered the hook before the window reads the envelope, the script's reply is the bytes it always was and no permission is decided; a seat that is off sends nothing, and every request passes the one door (the key, the switch, the person's consent for the folder, the day's count, redaction). Two grading faults are fixed in the function zo and the window share: a command is not regretted because its turn was cancelled, and a text is graded as an order only when the next step carried it out. zo's shell command rules and the guards' shared half moved to core unchanged (move-only commits), and a rule that cut a letter in two after `KEY=` no longer panics. The Jev tab shows each guard's requests agent by agent and says "cannot see" with the reason where an agent's hooks carry nothing (OpenCode has none; Antigravity is asked after the command ran). Measured: a replay of 610 events over 13 agents was answered 88 of 88 on both seats (Jev p50 219 and 215 ms); the hook's round trip is unchanged (p50 58.1 ms without, 49.6 ms with, inside the noise); by this machine's transcripts three panes over eight hours would ask about 1,252 times a day, about $0.03. Merge resolution: the prompt's words are read past the pasted-content frames (t-10993) by the one reader the card and the guards share. Coordinator gate on the whole landing: the full gate ran on 2aed9279 (pii, fmt, lint, doc, workspace test, shell bins and source contracts, settings and window harnesses, zo fmt, clippy and e2e, and the Windows cross-checks of root and zo, all 0, and zo test (every test binary run to its end) 0 but for the names judged alone below; the tools tests red on one test of the replay seed that read the guards' source words from the file they had left, and zo doc red on two public notes that linked to a private item), and with those two fixed pii, the tools tests, and zo fmt, doc and clippy were run whole on a63541cf, all 0. 1 test name(s) failed once in the parallel run and passed alone three times each, judged as the lane judges an unlisted flake. (t-10916; coordinator review)
- Merge branch 'wt/t-11010/jev-notify' (e4e6f371) — the notify seat is graded on what its labels can actually say, and rings the way it does today when nobody is there: a ring that finds the person away was marked only when a hand came back, so an away ring could only ever count against "batch it" (11 of 479, and 7 of the 10 high-confidence errors), and 37 older labels carried no baseline mark. An away ring is now graded neither way, a label is marked again from its own facts (call, attendance, reaction) wherever marks are read, the ledger itself is never rewritten, and the bell applies the seat only to a ring that finds the person present; thresholds and the promotion rule are unchanged. Replayed on a copy of the ledger: 81.9% becomes 87.9% (lower bound 807) against a baseline of 5.2% on the same 116 marks, the calibration reads whole instead of non-monotone, and the seat rises two answered requests later. Coordinator gate on the whole landing: the full gate ran on ef47136f (pii, fmt, lint, doc, workspace test, tools, settings and window harnesses, zo fmt, doc, clippy, test and e2e, and the Windows cross-checks of root and zo, all 0; shell bins red on one real failure, two Korean lines of the notify card that used a codebase word), and with the wording fixed the labels a line of the UI can reach were run again on 69b90187 (pii, shell bins and source contracts, tools, settings and window harnesses, all 0). 1 other test name(s) failed once in a parallel run and passed alone three times each, judged as the lane judges an unlisted flake. (t-11010; coordinator review)
- Merge branch 'wt/t-10956/zo-compact' (1ced15a9) — zo's /compact no longer holds the screen: both front ends ran the summary synchronously on the screen's own loop, so a 101k-token conversation showed nothing for 59.5 s and ignored Esc (106 s in a real session on 09-22); it is now one asynchronous function for both, raced against the same interrupt as a turn and applied only after the summary arrives, with progress on the status line (silent stretch 59.5 s → 0.3–0.4 s, Esc answered in 0.15 s, a conversation left byte for byte as it was after Esc). And a long conversation a model declines is compacted once and asked again: on 09-27 a conversation begun on gpt-6-astra and switched to Opus 5.5 was not over its window (146–150k) but was declined six times, and the refusal ladder gave up; its last rung now compacts once a turn, sends the person's last words as they were, says that it did, and surfaces the refusal as it is if it is declined again; the turn's record says whether the compaction resolved it, and the notice counts every picture the conversation still carries. Provider pairs after a switch: first request 4 of 4 where a login stood. Coordinator gate on the whole landing: the full gate ran on ef47136f (pii, fmt, lint, doc, workspace test, tools, settings and window harnesses, zo fmt, doc, clippy, test and e2e, and the Windows cross-checks of root and zo, all 0; shell bins red on one real failure, two Korean lines of the notify card that used a codebase word), and with the wording fixed the labels a line of the UI can reach were run again on 69b90187 (pii, shell bins and source contracts, tools, settings and window harnesses, all 0). 1 other test name(s) failed once in a parallel run and passed alone three times each, judged as the lane judges an unlisted flake. (t-10956; coordinator review)
- Merge branch 'wt/t-10993' (3bb936e6) — finished work shows as finished: once a worker hands in and its pane retires, the ledger's agents list dropped its row, so the sidebar fell back to the branch name and the raw prompt ("<pasted_content …> You are a worke…") and the board lost the card. A released worker whose checkout still stands is now carried as a settled row, the sidebar's card and line lead with the task's title and the ledger's own stage (awaiting review, verified, merged, deployed, or failed), the board's answered and ended lanes say the same words, the desk's roster keeps only the summoned, and the pasted-content frame is stripped by one core function the quota wall shares. A worker's claim still never reads as verified or merged (t-6815); the stages move when the coordinator writes the review. Coordinator gate on the whole landing: the full gate ran on ef47136f (pii, fmt, lint, doc, workspace test, tools, settings and window harnesses, zo fmt, doc, clippy, test and e2e, and the Windows cross-checks of root and zo, all 0; shell bins red on one real failure, two Korean lines of the notify card that used a codebase word), and with the wording fixed the labels a line of the UI can reach were run again on 69b90187 (pii, shell bins and source contracts, tools, settings and window harnesses, all 0). 1 other test name(s) failed once in a parallel run and passed alone three times each, judged as the lane judges an unlisted flake. (t-10993; coordinator review)
- Merge branch 'wt/t-10915/claude' (164961b1) — every connected Claude account's usage is read without anyone pressing a button: an account nobody is running keeps an expired login, and its reading came back 401 for as long as the person did not sign in again, so four of this machine's five accounts read "could not be read" and the status bar had no next account to switch to. When an idle account's reading says its token is stale the window now runs that account's own official CLI once, under that account's own folder and never the chosen account's home (headless, a slash command that calls no model: no tokens spent, 2.6 s at the median), lets the CLI renew its own login under the CLI's own lock and reads again; an account with a live pane is left to its own CLI, one account is renewed at a time, and a login the server no longer renews says "login expired, sign in again" in five languages instead of retrying. The window never performs the renewal itself and reads no refresh token. Measured on this machine, once an account: 1 of 5 read before, 4 of 5 after; the fifth is a login that is really gone. A map of every store a Claude login lives in (t-11045) found no store that shares a login with an account's folder except the chosen account's runtime copy, which this renewal never touches. Merge resolution: the core module list names both new modules. Coordinator gate on the whole landing: the full gate ran on ef47136f (pii, fmt, lint, doc, workspace test, tools, settings and window harnesses, zo fmt, doc, clippy, test and e2e, and the Windows cross-checks of root and zo, all 0; shell bins red on one real failure, two Korean lines of the notify card that used a codebase word), and with the wording fixed the labels a line of the UI can reach were run again on 69b90187 (pii, shell bins and source contracts, tools, settings and window harnesses, all 0). 1 other test name(s) failed once in a parallel run and passed alone three times each, judged as the lane judges an unlisted flake. (t-10915; coordinator review)
- Merge branch 'wt/t-9471/h6-mail-triage-shadow-2' (7797d19e) — a coordinator's letters can be triaged by Jev: a new seat, mail_triage (the 29th), asks of each letter that reaches the coordinator whether it needs an answer now, can wait or need not be seen, and how urgent it is, and labels its answer by what the coordinator then did (an acknowledgement alone is not handling). It only records, once a person turns it to shadow in Settings, and the desk's order does not change. Replayed over seven days of the ledger with no new Jev request (2,135 letters), the simple rule by a letter's kind agrees with what the coordinator did 33.6% of the time, the baseline the seat will have to beat. Merge resolution: the table of seats that stamp a task holds both this seat and summon_difficulty (six). Coordinator gate on the whole landing: the full gate ran on ef47136f (pii, fmt, lint, doc, workspace test, tools, settings and window harnesses, zo fmt, doc, clippy, test and e2e, and the Windows cross-checks of root and zo, all 0; shell bins red on one real failure, two Korean lines of the notify card that used a codebase word), and with the wording fixed the labels a line of the UI can reach were run again on 69b90187 (pii, shell bins and source contracts, tools, settings and window harnesses, all 0). 1 other test name(s) failed once in a parallel run and passed alone three times each, judged as the lane judges an unlisted flake. (t-9471; coordinator review)
- Merge branch 'wt/t-10786/t-10638-unknown' (3f359a78) — a summons that names no model takes one from a table a person can edit (Settings › Jev: difficulty to model and effort for each agent; by default Codex runs the easy task on gpt-6-luna at max, the middling on gpt-6-sol at high and the hard on gpt-6-astra at xhigh, and Claude runs opus at medium, high and max), and the summon-difficulty seat is graded on what its summonses came to, not on agreeing with the coordinator: after a task ends its row says whether it finished in one go (a receipted worker_done with no retry and no re-summons), the rework it took, its wall time and its tokens, and the seat rises only when its choices keep the first-try rate while spending less than the baseline, which can now be wrong too; a coordinator's pin is kept as context, never as the answer. Replayed over the 281 past summonses: 29 of 203 tasks needed rework and 272 summonses carried tokens; the first-try rate is not claimed, because past landings left no structured review. The settings card grew past its scroll pane, so the contrast check now brings a control that is scrolled out of sight into view, measures it and puts the scroll back instead of passing it unmeasured (435 rows kept, unknown 2 → 0; a control with no contrast still fails). Coordinator gate on the whole landing: the full gate ran on ef47136f (pii, fmt, lint, doc, workspace test, tools, settings and window harnesses, zo fmt, doc, clippy, test and e2e, and the Windows cross-checks of root and zo, all 0; shell bins red on one real failure, two Korean lines of the notify card that used a codebase word), and with the wording fixed the labels a line of the UI can reach were run again on 69b90187 (pii, shell bins and source contracts, tools, settings and window harnesses, all 0). 1 other test name(s) failed once in a parallel run and passed alone three times each, judged as the lane judges an unlisted flake. (t-10638, t-10786; coordinator review)
- notify seat: an away ring is graded neither way and rings today's way; old labels are marked again from their own facts (t-11010)
- tmp5: merge t-10956
- tmp4: merge t-10993
- tmp3: merge t-10915 onto main+t-10786+t-9471 (core lib.rs: both modules)
- tmp2: merge t-9471 onto main+t-10786 (task_cost: both seats stamp a task, the table holds six)
- tmp1
- Measure scroll-clipped controls before judging contrast
- Grade summon choices by execution outcomes and configure difficulty profiles

## [1.1.33] — 2026-09-27

_since v1.1.32 (22 commits)_

### feat
- feat(reflex): prepare RTS input oracle and current-target aiming comparison

### fix
- fix(reflex): select predicted aiming from matched fast-input trials
- fix(jev): pair baseline labels and bind recall evidence
- fix(reflex): settle queued pointer motion before a press
- fix(api): keep pooled connections driven between sync calls
- fix(jev): judge skill seats after recording their evidence
- fix(bench): score the active hand and keep preparation visible
- fix(bench): distinguish supplied stress ceiling from achieved input rate
- fix(reflex): keep pointer refusal and reuse fixture support for RTS
- fix(bench): use the configured login generator for automatic plans

### release
- release: two zo verifier tests that wait on spawn-thread rows are load flakes

### other
- Merge branch 'wt/t-10384/r10-scope-pid-shift-cmd' (70fcc0c8) — the reflex hand presses keys and chords, right and modified clicks, and drag boxes, keys only to the process of the plan's app with every modifier released in pairs and the closing and switching chords refused from one table; it lets its own queued pointer move settle (8 ms) before a press and aims where the target will be when it presses. Measured on the fixtures with nobody at the keyboard: an RTS-style board mixing keys, box drags, right clicks and shift clicks ran 324.2 actions a minute (hit share 0.994) with a median reaction of 55.7 ms and no wrong input; the stress mode kept pace through 600 targets a minute (377 a minute overall, no wrong input); and with a 16 ms pointer the predicted aim hit at least 0.9954 on each of the R9 seeds 11, 12 and 13 (at 80 ms no aim reached 0.99 on both seeds), so predicted is the default. The model-written plan arm is not measured: the window's Claude login had expired and Codex hit the 30 s plan wall; it waits for the login. Coordinator gate on the whole landing (42abb688): pii, fmt, lint, doc, workspace test, shell bins, tools, swift, settings and window harnesses (1846 of 1846), zo fmt, clippy and e2e, and the Windows cross-checks of root and zo, all 0; zo test 0 but for two runtime tests that failed once in the parallel run and then passed alone three times each and in a full parallel rerun (2456 of 2456), judged as the lane judges an unlisted flake. (t-10384, R10 of t-10223; coordinator review)
- Merge branch 'wt/t-10618/jev-1-4-recall' (5fdd1322) — four Jev seats can be compared with the simple way they stand in for, on the same facts: skill search and skill suggestion mark whether the word-match ranking at the same cut found the skill the turn loaded, compaction whether keeping every block was right by the block being read again within five turns, and branching the outcome observed while exploring, marked as such; every baseline can be wrong (golden cases 0 to 19). Recall labels join their request exactly by query, notes and request time, and a request's settled mark is posted only after its row is on file, so a turn that ends between the two no longer strands a label above its request (0 misattributed; the 103 older labels are left unattributed). Merge resolution: skill search's label carries its baseline and is recorded through the seat judge (t-10575's record_row), and t-10575's two tests call the new signature with the baseline their comparison had. Coordinator gate on the whole landing (42abb688): pii, fmt, lint, doc, workspace test, shell bins, tools, swift, settings and window harnesses (1846 of 1846), zo fmt, clippy and e2e, and the Windows cross-checks of root and zo, all 0; zo test 0 but for two runtime tests that failed once in the parallel run and then passed alone three times each and in a full parallel rerun (2456 of 2456), judged as the lane judges an unlisted flake. (t-10618; coordinator review)
- Merge branch 'wt/t-10246/jev-summon-effort-09-26-16-3x-jev-max' (01c9a0bc) — a summons that names no effort asks Jev how hard its task is (a new seat, summon_difficulty, the 28th) and turns the answer into the agent's effort through the launch table's own ladder and each agent's ceiling; a coordinator's pinned effort is never replaced, and the seat keeps recording until it is graded. Its first grading, agreement with the coordinator's pins (82 of 276 against 250 for always high), measures how well it imitates the coordinator rather than whether an effort was right, so grading by what the summonses came to (first-try finish, rework, tokens) follows in t-10638. The Jev tab shows the seat. Coordinator gate on the whole landing (42abb688): pii, fmt, lint, doc, workspace test, shell bins, tools, swift, settings and window harnesses (1846 of 1846), zo fmt, clippy and e2e, and the Windows cross-checks of root and zo, all 0; zo test 0 but for two runtime tests that failed once in the parallel run and then passed alone three times each and in a full parallel rerun (2456 of 2456), judged as the lane judges an unlisted flake. (t-10246; coordinator review)
- Merge branch 'wt/t-10575/jev-27' (576b09f1) — every Jev judgment was walked once for real: all 27 seats of the window and zo asked a live question, got an answer and wrote their row in a scratch home (27/27), and the audit report says for each whether it is asked, right and applied (before any fix: 0 applying, 7 broken, 17 blocked, 3 unmeasurable). Two causes are fixed here. zo's recall, mention and step-effort questions timed out (1.5 to 10 s) whenever a synchronous Jev call had run earlier in the same process, because the shared HTTP client kept a pooled connection that no runtime drove between calls; the fallback runtime now keeps one I/O worker running (red on a local keep-alive server, then green). And the two skill seats recorded their evidence without ever being judged for promotion; their writers now call the seat judge. Thresholds, pins and settings are unchanged. Coordinator gate on the whole landing (42abb688): pii, fmt, lint, doc, workspace test, shell bins, tools, swift, settings and window harnesses (1846 of 1846), zo fmt, clippy and e2e, and the Windows cross-checks of root and zo, all 0; zo test 0 but for two runtime tests that failed once in the parallel run and then passed alone three times each and in a full parallel rerun (2456 of 2456), judged as the lane judges an unlisted flake. (t-10575; coordinator review)
- Add learned summon difficulty and effort selection
- WIP feat(reflex): the fast hand presses keys to its app's process, right and modified clicks, and drags (t-10384)

## [1.1.32] — 2026-09-27

_since v1.1.31 (2 commits)_

### fix
- fix(hooks): keep Antigravity status reporting out of permission decisions

### other
- Merge branch 'wt/t-10461/antigravity-1-2-11' (60992f1c) — Antigravity 1.2.11 asks every pre-tool hook for a permission decision, and the window's status hook answered with none, so every tool call in every Antigravity session on the machine was refused with an empty reason; the window's status bundle now reports work from PreInvocation and PostToolUse only and never sits in the permission path, and a reinstall clears the old managed pre-tool entry while keeping any hook a person added under the same key. Found when an Antigravity worker could not read a file after the CLI updated itself; reproduced with one `agy -p` shell call (refused → `hook-test-ok` once the entry was gone). Coordinator gate on 60992f1c: hookd rc=0 180 passed 0 failed · pii 0 · fmt 0 · lint 0 · shell bins 0 · Windows root 0 · zo 0. (t-10461; coordinator review)

## [1.1.31] — 2026-09-27

_since v1.1.30 (10 commits)_

### core
- core(t-10372): the value seat's table gains the two login roads and the road a person chooses

### shell
- shell(t-10372): Computer Use's generator runs on the logins the window runs its agents with

### tools
- tools(t-10372): the value probes read no login and speak as no client

### ui
- ui(t-10372): the Computer Use card chooses the generator's road and says whose login it spends

### other
- Merge branch 'wt/t-10372/computer-use-oauth-cli-claude-p' (17d16e4f) — Computer Use's generators run on the person's login instead of an API key: the value a walk types into a field and the plan the reflex autopilot writes are asked of the vendor's own CLI once — `claude -p` as the window's active Claude account (the reading road, no credential written) or `codex exec --ephemeral` under the window's managed Codex home, the question on stdin, in an empty folder of its own, with no session kept and the whole process group killed at the wall — and never with a token the window reads or an identity it borrows. Settings › Computer Use chooses the road (`computer_generator_road`: auto, Claude login, Codex login, my API key, off; auto by default, Claude first), names each login's account and model and says the calls count against that plan's limits; passing from one road to the next is written on the walk's row, the plan's row and the card (`passedOver: claude_login=quota_wall`), and when no road can answer the field goes back to the agent with every reason. The probe's borrowed-identity Anthropic road and its Code Assist imitation are gone, and a contract keeps any such line out of the generator's files. Measured with the active account, five one-value calls a road: Claude login p50 2,268 / p90 2,618 ms at 434–468 input tokens (5,010 / 6,112 ms and 17,194 tokens when asked from the repository folder with thinking on), Codex login p50 3,833 / p90 4,903 ms; with no API key on this machine a walk typed a field over the Claude login, and a reflex plan that Claude's haiku got wrong three times was passed to the Codex login, which wrote one the contract took on the first request. Merge resolution: R9's `Generator::source` and this branch's `pass_over` both kept, and R9's stand-in generator and plan test moved onto `Setup`/`Answered` (the bench's key road keeps its row). (t-10372; coordinator review)
- Merge branch 'wt/t-10343/t-10223-r9-fixture-reflex-py-autopilot-b' (4ab844f5) — the reflex bench runs the autopilot from a goal sentence and says where it fell: `fixture_reflex.py run … --autopilot [goal] [--generator window|stub] [--l1 auto|shadow|off]` has the driver start the window's autopilot (`Autopilot::start`) with bench.json's `reflex_goal`, its ledgers in the bench's own home and the Jev key only in the driver's environment; the oracle gains a tenth check (every plan is the model's) and `tally --reflex` reads the autopilot's own account (roads, applied, not carried out, unanswered, ended, plans) instead of fixed zeroes. Measured on the fixture with nobody at the keyboard, seeds 11·12·13, 60 s each: the hand's plan 201.2/209.5/215.3 actions a minute, the autopilot road with the reflex decision recorded but not applied 207.3/209.2/212.8, both with no wrong input and 15–16 phase changes; with the decision applied the run ended within about a second, because Jev answered `pause` over a reading taken before any target — so a pause over a reading with nothing to press and no press since the last collect is no longer carried out (`why: idle`), after which the runs lasted 4.8–7.1 s at 50–110 a minute, Jev answering `pause` to 172 of 177 readings while the hand was pressing (the question is the next fix). The oracle's floor (0.99) is missed at 0.964–0.982 on both roads by targets that moved before the press. The model-written plan was not measured: no generator road stood on this machine (t-10372 lands next). (t-10343, round R9 of t-10223; coordinator review)
- reflex autopilot: a pause about a hand with nothing to stop is not carried out (t-10343)
- reflex bench: a run the helper forgot is what it last said (t-10343)
- reflex bench: a goal in place of a plan — the autopilot's road on the fixture (t-10343, R9 of t-10223)

## [1.1.30] — 2026-09-26

_since v1.1.29 (9 commits)_

### feat
- feat(reflex): a reflex run's plan written from a goal, and the reflex decision carried out on it (t-10243)

### release
- release: the ui flake entry is the check's name alone, as the lane reads it
- release: list the window harness's spinner timing check as a load flake

### other
- Merge branch 'wt/t-10324/s2-12-12-0' (0b0e24bc) — the screen-action question's twelve candidates are chosen by the goal's words, not by observation order: when a screen offers more controls than the cap, a pure function (`screen_action::pick`, weights in one `PICK_WEIGHTS` table — a quoted phrase two, a word of two or more characters one, a role word one; a Korean word matches by its stem so 「모드를」 finds 「모드」; a field's label, placeholder and nearby text are read, a field's value never, secret or not) scores every control the walk may still choose, keeps the top twelve and then puts them back in observation order — so a screen with twelve or fewer controls, or a goal with no signal, sends the very bytes it sent yesterday (the judgment memo's key survives; the existing `the_slice_that_is_cut_is_the_slice_that_may_be_chosen` test is untouched and green), the rubric stays v6 at its pinned words, the option ids stay `mark:N` and the thirteenth control is proposed and read as `mark:13`; the reverting road is one line, `CANDIDATE_PICK = Pick::InOrder`. Two ledger keys (`candidatesSeen`, `candidatesSignal`) record how many controls the walk saw and whether the goal matched any, on the walk row beside the count sent, with no new word in the question and no new field in the request state. A regression set of eight fixtures (a thirteenth, a last, repeated rows, Korean particles, similar names, off-screen, two right answers, a field placeholder) reads 0/8 → 8/8 for "the right control was among the candidates" in one binary with the choice on and off, three held-out cases kept beside it outside the test list; red first then green (t-10324, S2 of the Codex session's fast-path design D3; Fable review, one round)
- Merge branch 'wt/t-10243/t-10223-r7-l2-reflexplanrequest-0-l1-ref' (9324e484) — a reflex run's plan is written from a goal, and the reflex decision is carried out on it: `zerocode-computer reflex-auto --goal "…" --app <app> --display N --seconds N [--renew] [--l1 auto|shadow|off]` counts the palette of the named app's window alone (no image leaves the machine), asks the generator the person chose in the key card (`type_value::chosen()`, its model; without one the run is refused as `reflex_no_generator` before the helper, the screen or the wire is touched) for the plan as contract JSON whose hash the contract fills in, rounds it through `written_sections → read_sections` with the contract's own refusal sentences at most twice, refuses a replan whose scope is not the admitted one the second time, and starts it through `admit` and the ordinary launch — the plan's scope is the person's word, not the model's. The `reflex_decide` seat's answer now reaches the hand: the seat is off/shadow/on/auto and promotes, with the stall seat's four constants, `negatives_wanted`, `apply_deadline_ms` = the seat's own deadline, the baseline `AlwaysSame("continue")` and the request named `(run, decision)` + `requestAt`; an answer is carried out only when it arrived inside the asker's wall, its reading is younger than `REFLEX_APPLY_MAX_AGE_MS`, its run, epoch and plan hash are the ones running and `applies_in` says the seat applies — otherwise the row says `applied:false` and why (not_auto, stale, epoch_mismatch, plan_mismatch); `continue` records only, `pause` is `reflexStop` with the reason `paused` (and a new plan when the last three collects found nothing), `replan` stops, writes a new plan and starts a new run and epoch, three unanswered or invalid answers in a row end the run as escalated while a door refusal or a wire error is said as what it is. Labels are a share normalised two ways and named so (the bench's hits over what it offered, a screen's pressed over pressed plus missed), and a bench-forced `--l1` is stamped `provenance.forced` with its road left as memo or jev. `reflex-status` answers the autopilot's roads, applied, invalid, unanswered, door, wire and plans. Red first across core, shell and the settings harness, then green; the worker's gate at 9324e484 after a rebase onto v1.1.29: clippy 0, core, shell bins, source contracts, settings harness (t-10243, round R7 of t-10223; Fable review, one round — two follow-ups noted, not landing conditions)
- Merge branch 'wt/t-10311/s1-walk-zo-computer-action-walk-computer' (24d621d2) — zo's Computer tool gains `walk`: a goal walked in one call and answered as one status. The tool takes the goal, one surface (app, pane or platform+device, an id the agent observed), `until`, `max_steps` and the overlap/rescue/replay knobs, validates them with the same core parser the CLI uses, and hands the walk to the window's existing `run_goal` once — no second walking loop in the tool — so the upper model no longer re-decides every action itself. The answer is one of five statuses (verified, needs_verification, needs_fallback, stopped, failed) with the reason, the presses, the last step a judgment answered and the budget left; `ok`, a press count or the model's own "done" never make verified, and when `until` already stood on the screen before the walk `run_goal` now looks once first and answers `untilBefore` (an added key, nothing existing changed) so a walk that proved no new effect stays needs_verification. A feature that is off, has no key or only records answers on the first return with its reason and is not called again, and no walk is preceded by a screenshot for coordinates or followed by a recapture unless the last press left a screen nobody read. The words the window writes and zo reads live once in core (`computer_use::walk_words`); the skill file gains the road-choice table (single action, batch, recipe_run, walk, reflex) with a walk example and the status meanings, installed through the bundled skill road. Six zo tests red first then 25/25, the shell `run_goal` test red without the first look; the worker's gate at 24d621d2: zo clippy, zo test 7,103, zo e2e 103, core 229, shell errand and runtime tests 301, workspace clippy 0 (t-10311, S1 of the Codex session's fast-path design D1; coordinator gate)
- screen question: cut a long look by the goal's words, offered in the look's order (t-10324)
- zo Computer gains `walk`: a goal walked in one call, answered as one status

## [1.1.29] — 2026-09-26

_since v1.1.28 (16 commits)_

### feat
- feat(computer-use): settings turns the live reflex layer on, only when it can run (t-10221)
- feat(reflex): a detector picks which blob its target follows (t-10242)
- feat(zo): the footer gets codex 0.157.1's second row — ← for agents, ? for shortcuts, ⚠ N warnings · f2 to view
- feat(board): the relations tab's picture stands in real 3D space, called 입체 (t-10118)

### fix
- fix(jev): zo 좌석 넷이 모든 선택지의 뜻을 말하고 근거를 state 필드로 보낸다
- fix(launch): a Codex tab starts without the shared daemon whose socket the managed home puts out of reach
- fix(term): a withheld prompt is one sentence in the person's language, said once — the event names the guard's refusal by token and the window words it from one table (t-10159)

### other
- Merge branch 'wt/t-10242/t-10223-r8-detector-pick-first-nearest-l' (94543697) — a reflex detector says which blob its target follows: `Detector.pick` is `first` (the primary as before), `nearest` (to the last fired target's frame point, which the scheduler now holds and hands the kernel with each observation; `first` until a target has fired), `largest` (the roi's area), `newest` or `oldest` (the track id, which only grows), ties to the smaller id, and a primary that is still alive is kept — the pick chooses only when it is gone, so `newest` cannot re-aim every frame and shake a rule's edge. The words live in one `Pick` enum (serde name = word, `Pick::ALL` for the grammar R7 will read), and a plan that names no pick keeps its wire bytes and its plan hash: the forty contract fixtures are unchanged to the byte, the six v1.1.28 ok cases and R4's bench plan decode and re-encode to the same file, `pick_omitted` is the four word cases with the field removed, and the mutation that writes `first` (Rust's `skip_serializing_if`, Swift's encoder) reads red on both sides. Contract VERSION 2 and LIMITS unchanged; the helper's five RPCs take no new argument; receipts keep their eighteen keys and add `pick` and `trackId`. Ten fixtures (six pick words plus omitted, first-written, cells-unsupported and the bench plan) and a kernel scene file, red on both sides first; core 2089 + 11, Swift 258 + 4, the probe's self-test ok, eleven mutations each red at a named assertion (t-10242, round R8 of t-10223; Fable review, one round)
- Merge branch 'wt/t-10221/cu-1-a3-computer-use-t-920' (d1886fcc) — settings turns the live reflex layer on, and only when it can run: Settings › Computer Use gains a 「실시간 반사 층」 card (five languages) with the switch, off by default, and three lines under it — supported (this platform's helper speaks the reflex kernel: the handshake's kernel, plan and run-policy facts), permissions (the accessibility and screen-recording rows as `computer_use_permission_status` already reads them, no second computation) and a check (what a start reads and nothing more — no window stop, the platform row, the helper's handshake and an idle status; zero input). The switch takes only when all three stand, otherwise it stays off, unpressable, with the missing line named; 「지금 점검」 runs the check and keeps its result with the helper's identity (providerVersion, planVersion) in the local data root's `computer-use/live-reflex-check.json`, so a check vouches only for the helper it checked and the person's settings file is written by the switch road alone; a permission taken away after the switch is on never switches it off in silence — the card says 「켜짐 · 지금은 못 돎」 and `admit` refuses the start. No new status command: `computer_use_capabilities` answers through `reflex::settings_answer(DoorFacts::now(computer_live_reflex), …)` so `liveReflex` is `standing()` itself plus the one new `check` field; `set_computer_live_reflex` follows the confirm setting's pattern and is the round-trip contract's twentieth use. The last bench result is not shown; whether the kernel really sees and hits is the bench's (t-10222) to say, and the toggle lands before the installed-app measurement by decision (the worktree measurement of t-10127 — 96%, 211 a minute, no wrong input — already clears item 14's numbers; `stop p95 ≤ 16.6 ms` goes on t-10222's table). Three Rust tests and one contract red first, the settings harness 136/137 → 137/137 (720×480 in five languages included), two mutations caught; the worker's gate at d1886fcc: clippy 0, shell bins 2,326, source contracts 527, settings harness 137/137 (t-10221; Fable review, one round)
- Merge branch 'wt/t-10232/codex-cli-0-157-1-footer-zo-cli-zo-ide-t' (1396515f) — zo's footer gets the second row codex-cli 0.157.1 has: the status row stays as it was (`model effort · cwd | indicators · context`) and under it a hint row reads `← for agents · ? for shortcuts` on an empty prompt, `tab to queue` while a turn runs with a draft, `? / esc close` while the shortcut card is up, with `⚠ N warnings · f2 to view` right-aligned (three fallbacks, the count in amber, the narrow row dropping the shortcuts hint before the agents hint). Plain Left on an empty prompt opens the agents overview from both the idle and the mid-turn road, and stays the cursor key with words or a modifier or an open popup; `?` on an empty prompt toggles the shortcut card above the composer instead of typing itself (esc or any other key closes it and that key is then handled), so `?` followed by Enter no longer submits "?" to the model (measured: zero requests in the e2e); the card lists Compose, Session and Transcript with zo's own keys in three, two or one columns by width plus the slash list it already carried; the conversation's warnings and errors are kept once each by their words (from the one place notes land) and F2 or `/warnings` pages through them, cleared only by `/new` and `/resume`. `footer()` loses its hint argument for one `FooterHints` value and `Ui` gains no bool; the three "Press enter to confirm or esc to go back" literals become one constant. Twenty-eight of the twenty-nine new library tests and the one byte test were red on a stub first; new modules `tui/footer_hints.rs`, `tui/shortcuts.rs`, `tui/warnings.rs`; zo clippy, tests, e2e and contracts green in the worker's checkout (t-10232; the Codex source read at rust-v0.157.1, Apache-2.0; coordinator gate)
- Merge branch 'wt/t-10010/h1-b-jev-b-zo-zo-step-effort-risk-comple' (6e9c7c4c) — the four seats Jev is asked from on the zo side say what every option means and hand their evidence over as state: the step-effort seat asks a question of its own (all twelve options described where two of the four complexity words and none of the four risk words were; the question names the seven state keys it reads; the step's numbers travel as `{task, step, signals}` fields instead of a line appended to the turn's text that a turn past 2,000 characters cut off; the seat's rubric follows the core's constant, 2, and zo writes that version on every judgment row while the chat probe's prompt stays untouched because it also routes the person who never uses Jev), the agent tool's `ask` describes yes and no, the `@` and `/resume` reranks name a row by its visible first line rather than its ordinal or a hidden session id, and the skill suggestion moves each skill's description out of the option sentence into state, sent once where it was sent three times. Audit cells 122 → 128 of 130 and seats passing all five columns 21 → 24; the skill search's second request −45% and its two requests 17,962 → 14,385 bytes on this machine's eighteen skills (−20%), the step seat's request 962 → 1,876 bytes for the descriptions it lacked, asked at most once in five steps; request counts unchanged; all four seats are recording with no comparison rows, so the version rise resets nothing. The routing replay tool's first-version request had been passing the door on the step seat's old pointer and keeps a row of its own for it. Nineteen tests red first (core 5, runtime 7, tools 7), five mutations of the words and keys caught by eleven of them; the worker's gate at 6e9c7c4c: clippy 0, core, shell bins, source contracts, zo clippy, zo test 7,065, replay tool tests, one e2e window-size case red once under a concurrent build and 3/3 alone (t-10010, round B of t-9469; Fable review, one round)
- Merge branch 'coord/codex-daemon-socket' (81a66334) — a Codex tab opened from the window starts again on codex-cli 0.157.1: that release attaches the TUI to a shared background app-server over a unix socket at `<CODEX_HOME>/app-server-control/app-server-control.sock`, and under the window's managed runtime home that path is 122 bytes where macOS's `sun_path` holds 104, so the connect was refused ("path must be shorter than SUN_LEN") and the tab ended with code 1 about ten seconds after it opened — three times in one afternoon, while the same command in a plain terminal under `~/.codex` (60 bytes) worked. A third launch table, `AGENT_LAUNCH_COMPAT_ARGS`, now carries what keeps an agent's CLI starting here as opposed to what lets it work unattended: `codex` gets `-c features.daemon_auto_start=false`, appended after the person's own line whether that line is the default, edited or emptied, never entering the permission judgement or the settings field. A config override rather than `--no-daemon` because it is version-safe — a Codex that predates the key warns that it is ignoring an unrecognised setting and starts, where the flag would be refused by one that predates it; measured in a pty, both keep the 0.157.1 tab alive, and the window's own app-server sidecars are explicit `--remote` roads the daemon never enters. One test red first, then green with the edited and emptied overrides, the catalogue check and the refused-flag check (the coordinator's own fix)
- Merge branch 'wt/t-10159/1-the-line-is-holding-words' (5ef8cf62) — a prompt the window withholds from a pane is said in one sentence, in the person's language, once: the pty's refusal names its reason by a token (`Refusal::token`, one settled event `term:prompt why=<token>`) instead of an English sentence, the window words each token from one table (`TERM_WITHHELD`, five catalogs, a plain notice with the explanation as its tip rather than a red halt), and the same pane with the same reason is told once until the situation changes — a prompt lands there, the hook state moves, or the shell ends. Before, a coordinator pane holding typed words showed two toasts of "터미널 N에 프롬프트를 넣지 않았습니다 — the line is holding words somebody typed…" in a halt tone and wrote the same notice to the clipboard twice; after, one toast, no halt, one clipboard write. The sentence is not "the mail pointer was held" because four doors receive the same refusal (mail pointer, `zerocode-ssh send`, dispatch, continuation) and the sentence has to be true for all four. Four tests red first (pty `every_refusal_has_one_token_the_window_can_key`, shell `the_window_hears_a_withheld_write_by_its_token`, source contract `a_withheld_prompt_is_worded_by_the_window`, window suite `term-withheld` 1/4), then green; the worker's gate at 5ef8cf62: clippy 0, pty all suites, shell bins 2323, source contracts 528/528, window harness 1819/1819 (t-10159; coordinator gate, no review — UI and wording)
- Merge branch 'wt/t-10118/3d-3d-canvas-2' (b27d28ef) — the relations tab's picture is a three-dimensional scene instead of concentric orbits: the main workspace stands at the origin, the other workspaces on a sphere around it, each agent on a small sphere around its workspace and a sub-agent beside its parent, all drawn by a perspective camera onto a canvas 2d floor with a grid, workspace columns and their shadows, shaded spheres and distance fog, with the structure, lineage and dependency lines and the flowing mail dots carried in the same projection; the view is named 「입체」 (3D · 立体 · 立体 · 3D), and no word in any of the five catalogs names a planet, a star, a moon or an orbit (a new harness contract). A resting pane costs no frame: a pane whose hand or focus sits on a label schedules 0 rAF (before 120 a second), as does one the person has turned that has no mail to flow, one under reduced motion and a hidden one; while the camera turns, 60 agents cost 0.5/0.7/0.9 ms p50/p95/max of script a frame (goal ≤ 8 ms; 6 agents 0.2/0.3/0.4; the 2× density 0.4/0.5/0.7) and the writes outside style fell 8 → 0 a frame, while the style writes rose to 29 a frame at 60 agents because every label moves with the camera and to 0 once it stops. The canvas keeps its 356,430 pixels; identifiers, the `.agent-board.is-orbit` boundary and the live-view hooks are unchanged. Eighteen of the 49 new harness cases were red on main first; 75/75 after, board-live 105/105, the window harness 1842/1842 (t-10118; UI-only branch, coordinator gate)

## [1.1.28] — 2026-09-26

_since v1.1.27 (32 commits)_

### feat
- feat(bench): a reflex round never starts beside another one standing on the machine (t-6767)
- feat(bench): a reflex plan may carry several rules a colour, the later ones trying a standing target again (t-6767)
- feat(bench): a reflex run's record keeps the machine's load at the goal and at the verdict (t-6767)
- feat(bench): the realtime reflex round — an owned fixture, an exogenous seeded stimulus and an oracle of its own (t-6767)
- feat(jev): recall, the skill search, claim and file pick pin their words to their versions, and the version comments say where the words are (t-9469)
- feat(walk): a settle-later press that left its page's legend settles before it answers (t-9876)
- feat(jev): the dashboard says why a feature does not apply by itself — the check's result, the confidence bar it acts from or why it has none, and why its rows compared nothing, in its row and its drawer (t-9935)
- feat(desk): folded news in a batch the coordinator holds offers that whole batch beside the folded count, and a quiet episode's line keeps one name while its silence is told again (t-9548)
- feat(board): a finished task says what it cost — its attempts, the wall clock with its waits, the generation model's tokens at the API rate and the Jev requests stamped with it — on the desk's finished rows and the task card (t-9470)
- feat(computer-use): settings keeps the key typing into a page's fields asks with (t-9537)

### fix
- fix(reflex): a frame is read from when it was in hand — the earlier of its capture time and its delivery (t-10127)
- fix(skills): a dead link where a bundled skill's folder should stand is replaced by the skill, so Qwen Code and Grok stop answering "File exists" and the required-skill badge clears (t-10166)
- fix(board): the orbit leaves the folded card picture alone, and the live map says it lives on the card view (t-9532)
- fix(jev): the dashboard reads at a window's width — a feature's row under 85em is one compact card (its name, five figures in a line, its state, the week at its side: 232 → 162 px a row at 74em), the week's strip draws each day as a bar as tall as its requests, an applying feature says the comparisons it applies on, a switched-off one counts nothing toward a judgment it will not make, and the column heads are short in five languages
- fix(bench): a walk only the judgment called done is no speed sample (t-6767 F4)
- fix(jev): summon, stall and branching send their evidence as fields and say only what an option means; the challenger's words are pinned (t-9469)

### docs
- docs(summon-replay): the replay's arms name what each agent's entry in the state carries, not an option's words (t-9469)

### other
- Merge branch 'wt/t-6767/r4-apm-codex-sol-high' (000502af) — the realtime reflex round has a bench of its own: a fixture window the product does not own (`ReflexFixture`, a SpriteKit panel under its own bundle id, never activated), a seeded exogenous stimulus of moving targets, occlusions and phase changes, an oracle that believes only the fixture's own record of what was hit, a driver that starts the window's `reflex::start` and its `Watch` without the window (an ignored test of the shell crate, the one peer the helper's authorisation admits), a `tally --reflex` that reads hits, wrong inputs, receipts and the load at the goal and the verdict, a plan that may carry several rules a colour so a later one tries a target still standing, and a refusal to start a round while another stands on the machine. Measured on the installed v1.1.27 helper, three 60 s runs with nobody at the keyboard and no wrong input: the same plan hit 42/227 and 9/222 (39.5 and 8.8 actions a minute), the relaxed plan 177/218 (175.1) — the numbers that found the reflex layer reading a frame from its display time and threw its fires away (fixed as t-10127, which this bench then measured at 96% and 211 a minute). A walk only the judgment called done is no speed sample (t-6767 F4). Seven red tests first, then 23 green in the fixture's own suite (t-6767)
- Merge branch 'wt/t-10127/r2-sck-displaytime-aim-unaimed-permi' (51222c88) — the reflex layer reads a frame from when it was in hand: the earlier of its capture time and its delivery (`observed_host_ns`, one function in the core and one in the helper), where before the capture time alone was read and ScreenCaptureKit stamps a frame with the moment it is to be displayed, which is often a few milliseconds after the frame arrives (54.6% of 2,616 frames, at most 4.2 ms) — so the hand, which checks the moment a frame lands, found the frame "in the future", could not aim on it and was refused its lease, and threw most of its fires away. The lease's proof and the frame's age now run from the observed moment, aiming carries a point forward by a time that saturates at zero instead of failing, and every receipt carries both stamps. Measured on the reflex fixture with the worktree-built helper (the same signing identity, so the same TCC grant), ABAB, three 60 s runs each way, no wrong input and nobody at the keyboard: hits 7.1/12.7/1.8% → 95.9/96.8/96.8% and 15.6/27.4/3.9 → 211.4/210.7/212.3 actions a minute; the misses left are targets that moved before the press. No new number and no nap; the shared contract fixture gains four cases that were red on both sides first, five mutations read red (t-10127; Fable review, one round)
- Merge branch 'coord/skill-install-dead-link' (188aa0bc) — a dead link where a bundled skill's folder should stand is replaced by the skill: `npx skills add` links each agent's skill folder to one checkout, and when that checkout goes the links stay pointing at nothing, so installing our copy there read "not found" through the link and then hit "File exists" from `create_dir_all` — Qwen Code and Grok failed that way on every press and the "1 required skill missing" badge never cleared. The installer now reads what stands at the folder before it writes: a link whose target is gone is removed and the skill written, with the outcome saying so; a link that still reaches a directory is written through as before; a plain file is somebody else's and is kept with a reason, before any read of "<file>/SKILL.md". Three tests, red first (t-10166; the coordinator's own fix)
- Merge branch 'wt/t-9469/h1-jev-27-2' (9a103d98) — the seats Jev is asked from are audited on five columns and the failing ones fixed: of 27 seats, 122 of 130 audit cells pass (119 before) and 21 seats pass every column (18 before). The summons send each agent's quota, history and record as fields of `agents[]` and describe an option by its meaning only (rubric 5 → 6; the task's title now passes the door, whose red run showed a key-shaped line in a title reaching the wire as an option's words); a stall sends its screen as lines and its transcript as `{role, words}` rows (4 → 5); a fork stops restating each candidate's result in its option (1 → 2, a request 2,898 → 2,330 bytes); the challenger, recall, the skill search, the claim check and the file pick pin their words to their versions (pin tests 19 → 24, seats pinned 21 → 26) and the version comments say where the words are; the core contract places all 27 seats and finds no option without a description. The three seats whose versions rose stand in `auto` and recording with no rows in the current series, so no seat is reset — recall, the one applying, keeps its words and version — and a replay of the twelve seats with ledgers reads the same before and after (t-9469, round A; the zo-side seats are round B, t-10010; Fable review, one round)
- Merge branch 'wt/t-9532/b0-dom' (ef63e57b) — the relations tab's picture leaves the folded card picture alone and the live map says where it lives: under the orbit view the card picture behind it is neither laid out nor dressed (five panes: 76 DOM writes a paint → 0, one node creation and one edge measurement → 0, the same paint's script 0.16 → 0.07 ms, and 0.48 → 0.14 ms with sixty agents), the views the live map draws on are named in one place (`agentGraphLiveViews`), and the "실시간 조율" switch under the orbit stands pressed but unpressable with a tip saying the live map shows on the card view — where before it turned on and changed nothing (a letter cost an event, a folded-picture beat and a timer; now none) — and coming back to the cards redraws the picture whole with one fit from the size watcher, no flag; and, landed first as t-9719, the permission cards measure with the palette (three literals → `--rule-width` and `--radius-pill`, zero pixels moved, a contract on the two cards' selector prefixes) and a TCC row that recorded no requirement says why it reads granted — macOS matches such a row by identifier alone, so there is no signature to check — in the grants table's own doc and a test of its own (t-9532, t-9719; Fable review, one round)
- Merge branch 'wt/t-9876/settle-later' (bdde708a) — a settle-later press that left its page's legend as it was settles before it answers: the door compares the look right after the press with the marks it pressed from (`same_legend`, the one comparison the walk's `Screen::same_as` uses too) and, when nothing in the legend moved, finishes the settle inside the press as `click --mark` always has (from the press's own moment, the same 250/50 ms walls) and answers with the settled look and its settle under the key the walk already reads — only a press that changed the legend answers at once and hangs its settle on the pane, as the second round of t-9712 left it; the walk then takes a settled answer as a settled page and asks in turn, and `reached()` keeps its phone branch to the phone. On the harness (ABBA, n=8) the gap after a press that leaves the legend — a field pressed before a search — falls from 409 to 345 ms (the road before settle-later stood at 356), the steps walk keeps its 247 ms gap with the judgment begun ahead used 16/16 times, a page that answers late still asks 3.00 questions a walk, and no scenario asks more; eight mutations and two harness mutations read red (t-9876; Fable review, one round)
- Merge branch 'wt/t-9935/jev-2-apply-share-notcomparedby' (3b9c68ee) — the Jev dashboard says why a feature does not apply by itself: a row whose check is held by its applied share names that line (the `apply_share` line joins the twelve the table already speaks, and a feature held there no longer reads as if it had cleared the bar), a row whose rows compared to nothing names the two commonest reasons under its chips, and the drawer gains an "automatic application" part with the check's result, the confidence bar the feature acts from or why it draws none (`no_confidence`, `whole`, `one_colour`, `non_monotone`, `no_lift`) and the whole distribution of why rows compared nothing — seventeen reasons in one table (`JEV_REASONS`: the twelve words the seats' own writers use and the five the act-line judge uses), spoken in the four catalogues with the Korean at the call site, a summed scope showing "—" with a tip that the value is per project. On this machine's ledger 24 of the 27 features now say why (0 before), and the pictures' cost is unchanged: an unchanged repaint 1.50 → 1.50 ms, one feature's change 2.00 → 2.00, a full redraw 11.40 → 11.30 and a cold one 11.10 → 11.30, the tallest 1080p row still 55 px (t-9935, absorbing t-9776 — the sample floor stays five with its reason written beside it, the two css literals are the permissions card's and go with t-9719, and the seven first-screen rows at 1080p are a decision, not a drift; Fable review, one round)
- Merge branch 'wt/t-9470/h4-jev' (6b0c7563) — a finished task says what it cost: on the desk's finished rows and on the task card, one line with its attempts, its wall clock with the waits in it, the generation model's tokens and their price at the API rate, and the Jev requests stamped with the task — read through each worker's last conversation (`sessionsKnown` against `sessionsLinked`, so an unlinked or unscanned conversation shows as such), an unknown dollar saying why (`unlinked`, `unpriced_model`, `unsupported_agent`, `unscanned`) rather than a number, the Jev side counting only the four seats that stamp a task and saying how many do not, and the sum memoised per finished task so a quiet beat recomputes nothing (the desk snapshot p50 4.02 → 4.42 ms warm and 4.96 ms cold over 675 finished tasks and 727 dispatches); nothing that names a conversation, a path or a home leaves the ledger (asserted key by key and value by value). Folded news in a batch the coordinator holds now offers that whole batch beside the folded count, and a quiet episode's line keeps one name while its silence is told again (t-9548). Read on this machine, the thirteen tasks the coordinator's run finished tonight cost 14 attempts, 14/14 conversations linked, 1,631.8M tokens, $990.94 at the API rate and 32 stamped Jev requests (t-9470, t-9548; Fable review, one round)
- Merge branch 'wt/t-9537/u3-road-credentialkey-anthropic-api-key' (b59eea72) — Settings keeps the key a page walk types with: the Computer Use section grows a "key for typing" card that draws one field per key the walk actually reads — today the table's chosen row, `haiku`, so one field for `ANTHROPIC_API_KEY`, marked as the key in use and naming the model it asks — with save, remove and a saved/unsaved badge, the value never read back (`type_value_keys` answers names and states only, `save_type_value_key` and `remove_type_value_key` refuse any name the walk does not read, and the keychain item is the one `value::key_service` names); a machine without a keychain folds the field behind the sentence the TypeSafe card already says; the TypeSafe card's own save, remove and state now go through the same `save_key_at`, `remove_key_at` and `key_saved_at` and the same four UI helpers, byte for byte in what it shows and does; and because the walk builds its writer fresh on every walk, a key saved here types on the next walk without a restart. On the harness a walk over a page with a field typed 0/6 times before and 6/6 after (a held key and a loopback endpoint, no real key), the settings harness grows to 136/136, the source contracts to 526 and the shell's unit tests to 2,343 (t-9537; Fable review, one round; the coordinator merged the one conflict, both sides having appended tests to `errand/desk/tests.rs`, by keeping both in order)

## [1.1.27] — 2026-09-26

_since v1.1.26 (35 commits)_

### release
- release: the lane judges a browser harness's FAIL line by name as it judges a cargo test — that one suite re-run alone three times, a listed `ui:` check outside the cap, an unlisted one named with its result beside `installed.json`; the window harness holds every frame late under `WINDOW_FRAME_LAG_MS` so a load flake reads red on a quiet machine (t-9741)

### jev
- feat(jev): a seat's act line is drawn from its own graded answers — the judge tests whether confidence separates being right, stores a per-seat threshold beside the ledgers when it does, and `JevUse::acts_on` is the one place the apply guards read it; the summary says each seat's applied share, its error on that share and the baseline, and why rows compared to nothing (`notComparedBy`). No seat on this machine earns a line yet, so today's behaviour is unchanged (t-9468, t-9556)

### dashboard
- feat(jev): the Jev dashboard draws each seat's days, accuracy trend with its lower bound and baseline, the rows owed before the next judgment, latency and daily tokens with an estimated cost, in the shell's own tokens for dark and light (t-9633)

### permissions
- feat(permissions): the Computer Use permissions card says what macOS will actually do — each TCC row of the app and its helper reads `granted`, `stale` (a grant recorded under an older build's signature), `denied` or `unreadable`, and a stale row offers reset and System Settings (t-6058)

### walk
- feat(walk): a walk step chooses its operation and every target it needs in one Jev request — the action head carries the operation, the click target, the type target and the observation heads; a `Type` operation is a closed observed operation (only a field the snapshot lists, never an id or a command) whose value comes from a generator the person configured in the product's key store, through the existing stdin pipe; the person's subscription login is not a road for it; a repeated value is reused (t-6720)
- feat(browser): the browser's observation is one state, read once — `marks --json` gathers the numbered controls with the document epoch, the input fields, containers, images and rows in one synchronous read (a moved page is read again, then said as `document_moving`); `click --mark` pins the document and the value, re-checks at the press and settles within 250 ms as ready, not ready or invalidated; the walk verb is proven through `run_goal` itself (t-6721)
- feat(jev): the recall seat judges latency on answered calls against its own deadline, a placement the pane never tried is `not_carried` (rubric 2), a notify ring nobody was there for says `away`, and `tools/jev-seat-replay` prints every seat's judgment before and after a change from a read-only ledger copy (t-9427)
- feat(walk): a page press answers before it settles — `click --mark N --settle-later` answers at once with the look the press left and the pane's next `marks` finishes the settle; a page walk asks ahead by default on a page the press changed, and a page that still looks as it did is asked in turn once it settled. On the harness a press step p50 376 → 293 ms and a three-press walk 1,189 → 922 ms; a page that answers late asks no more than before (t-9712)

### feat
- feat(board): 「답할 우편」 is the questions that wait on an answer — the ledger's notices stand under it as 「소식」, one line per quiet episode while the silence goes on, folded into a count after a day, and the desk's three numbers are the backend's (t-9456)
- feat(board): a crowded orbit names only the planets that are doing something (t-9444)
- feat(board): the relations tab draws a planetary system, and remembers which view a person chose (t-9444)

### fix
- fix(board): the crowded-orbit rule names each state in its own selector (t-9444)
- fix(board): a needs-you or failed planet's ring is drawn at its own width (t-9444)
- fix(board): the view toggle stands in the picture's head, so the card toolbar keeps its one line (t-9444)
- fix(board): the orbit carries the live map's leaving door, spends tokens instead of pixels, and keeps a label's side (t-9444)
- fix(jev): a notify ring nobody was there for names why it compares nothing — `notify_call::agreed` says `away` where it used to say nothing (t-9427)
- fix(jev): a pane nobody moved grades only the answer whose room it stood in — a recorded placement the pane never tried is `not_carried`, not wrong (placement rubric 1 → 2, t-9427)
- fix(jev): the latency line reads the answers' own times, so an acting seat no longer falls on the one timeout its answer line forgives — and recall's wall is its own number, measured (t-9427)
- fix(window): a pane's conversation page stops its spinner at the turn's end even when an earlier read already carried the turn's last words — the page is repainted once when the pane's state changes and never on a resting beat (t-9741)

### orchestration
- orchestration: one reading of a wait and one of a silence going on — `Run::awaits_answer` is the step `awaiting_reply` takes, and `Run::quiet_notice_stands` says whether a `went_quiet` notice still tells of the attempt's current silence (t-9456)

### tools
- tools(jev-seat-replay): every seat's judgment before and after a change, from two zo binaries on one read-only copy of the ledgers — the combined table only (t-9427)

## [1.1.26] — 2026-09-26

_since v1.1.25 (72 commits)_

### feat
- feat(reflex): the macOS desktop claims live reflex, and the window says whether a run is supported here apart from whether it is enabled — the capability table's macOS row alone turns `live_reflex` on (no surface claims an instant pointer), `capabilities` and `reflex-status` answer `liveReflex {supported, enabled}`, and the manual says a run is the macOS desktop's with the setting on (t-9205 ON)
- feat(reflex): the window starts, reads and stops a live reflex run by its id — `reflex-start` passes a door and a helper that reads run policy 1 with its kernel and answers `{runId, state}` at once, `reflex-status` and `reflex-stop` carry `--run` to the helper's compare, one watch per run writes the receipts before it acknowledges them, and the reflex decision is asked in shadow about the run's typed state, every sent request counted and no answer applied (t-9205 R3)
- feat(jev): the reflex decision is a record-only row of its own — `reflex_decide` (jevReflexDecide) offers off and shadow, stands at shadow under the one switch, never promotes, and asks one closed choice of `continue`, `pause` or `replan` about a live reflex run's typed state alone, its detector list cut at the plan's detector bound and each name at the 64-byte id bound, one lease on the wire (t-9205 S1a)
- feat(reflex): the helper installs R5's kernel at launch and runs a plan under its policy — evaluator and hand at user-interactive QoS, the window's perception table unchanged, each run its own kernel session, one deadline on an alarm of its own that lets go first, spent quotas renewed without an admission, status/stop/receipts compared against the run they name under one lock, receipts kept until acknowledged and a full queue ending the run, the closing session's stop before exit (t-9205 K)
- feat(reflex): the contract names a run's policy and reads one capability table — `max_run_ns` 120 s and a public 64-byte identifier bound in the one limits table, run policy v1 (`{renew, run_ns, version}`, canonical, version read first) decoded alike by Rust and Swift from shared cases, and `capability.json` the one table both sides read, whose `live_reflex` and `instant_pointer` columns stay false and whose `instant_pointer` alone now gates `--instant` (t-9205 C0)
- feat(jev): wire the challenger arm into zo's spawn path — the draw off the spawn's thread, the day's share read, checked and reserved under one lock, the incumbent's first plan as its design, a blind comparison through the one door, the verifier's verdict as the receipt, and a role's model moving only when the seat stands and the standing passes (t-6263)

### fix
- fix(step-effort): the step effort label keeps its own two-hour wait, apart from the stall label's four, and the wait is pinned to rubric 1 (t-9087 r2, astra R-EFFORT-1)
- fix(t-7538 r6): a put that ended is not a put whose record landed — the record says a put is under way before the home changes hands, and a home whose put never recorded its end is nobody's
- fix(t-7538 r5): a hold nobody could read is not a hold that is not there, a selected read files its answer only under the row whose login it asked with, and a move seen complete is written down before its receipt and kept until a write lands
- fix(t-7538 r4): an account switch rides t-7812's restore roads — its words through the goodbye's one note, its closed program held on the ledger's row against every road, and its yes, receipts and readings bound to the logins they were given for
- fix(t-7538): a Claude account switch moves a pane only on the wall it stands at now, keeps every receipt until the ledger takes it, and never relaunches on a guess
- fix(t-7538): clippy — the inactive account's reader is account_login and the Option view of it is the tests' own; aliases are compared with contains
- fix(t-7538): a Claude account switch keeps every working pane and moves only a walled one, as the same worker, on the model and effort it really ran
- fix(zo): a restore regrets a command only for what it changed — the path itself, a folder holding it, or a path under a folder it made or removed; a folder whose listing alone moved regrets nothing under it (t-9087 r2, astra R-GUARD-1)
- fix(jev): the dashboard says where it counts — the checkout by name, the projects with zo records when it has none, every project summed on one switch, and the features this computer keeps in one place marked (t-9091)
- fix(orchestration): a window that has said its goodbye leaves its sleepers to the next window — neither the grace nor a reseat (t-9091)
- fix(zo): a restore regrets the command that changed what it put back, not every command that named a folder holding it; the command guard's rubric moves 1 -> 2 (t-9087)
- fix(summon): a summons whose model was pinned carries no mark, and the rubric moves 4 -> 5 (t-9087)
- fix(stall): the stall label waits four hours for what followed a silence, and the rubric moves 3 -> 4 (t-9087)
- fix(jev): a judgment window's marks reach back to hold the sample floor — a seat whose marks are sparser than its requests is judged on its marks (t-9087)
- fix: the standing reader's doc names private helpers as code, and the skills answer book has a name (t-6877 round 2)
- fix(jev): keep challenger labels bound across failed reads and start in shadow (t-6263 r6)
- fix(jev): a verifier's watch stamps every directory a file of its tree could be created in — the root of a tree that holds nothing, and every one git tracks nothing in that the ignores do not leave out — and a strike a reader owed is held and written again until the ledger shows it back, so neither a file come and gone nor a strike that failed or wrote nothing gives a contradicted label back (t-6263 r5)
- fix(jev): a verifier's watch stamps every file its tree is written from — HEAD's as well as the index's, bound to the HEAD it saw and to every path the tree holds — a label stands off the record only on the evidence it keeps and never against a verdict still on it, a contradiction a reader sees is struck beside the label so a record that forgets gives nothing back, and every row that asked names the rubric it asked by (t-6263 r4)
- fix(jev): a verdict names the source its verifier read from its start to the end of its turns, carries it whenever the comparison lands, and a label that names no source — or one the record contradicts — is no label to any reader (t-6263 r3)
- fix(jev): the challenger arm's words leave only as the door clears them when they leave, its day book and a dead holder's lock lose no share, the router learns from its samples only while the seat stands behind them, a receipt is the verdict on the source the attempt handed in, every label is followed by its sample once, and the whole road is proven through the spawn itself (t-6263 r2)

### docs
- docs(jev): the every-project sum's round size carries both measurements of one zo run (t-9091)
- docs(orchestration): the grace's refusal is named, not linked, from a public doc (t-9091)

### release
- release: the stuck-shell test joins the flake list — it measures a 5 s wall clock on the machine and read past it once in three loaded full runs, green solo ×3 (t-8938 review)

### jev
- jev: the challenger's row asks the rubric its arm stamps on every row, so a changed question opens the arm's window anew and the words before judge nothing of it (t-6877 round 5)
- jev: a repeated name guesses no asking, the text reader takes the rows reader's step, and a split seat keeps its person's word (t-6877 round 3)
- jev: a label grades one request and inherits its rubric and version; a seat is one question (t-6877 round 2)
- jev: a seat is judged on one rubric's series and stands only on the words it asks now (t-6877)

### orchestration
- orchestration: an unanswered question no longer silences its asker's mail pointer — a named ask retried while its first wait is out joins that question, a pane is held only while it waits mid-turn on its own question, and the three load-flaky tests wait on a clock they stand still instead of the machine's (t-8938)

### tools
- tools(ask-wait-replay): `account_switched` is the ledger's own kind too, and neither an answer nor an ending — t-7538 adds it to `MessageKind::is_the_ledgers_own` on a main whose replay (t-6740) keeps a copy of that list, so the tools gate's source contract failed on the rebased branch (12 tests, 1 failure, the missing word); an account switch keeps the worker id, dispatch and conversation, so `ending_of` is unchanged (t-7538 integration on 22e3084c)

### ui
- ui: skill search and skill suggestion each say what they send, in every language (t-6877 round 2)

## [1.1.25] — 2026-09-25

_since v1.1.24 (85 commits)_

### feat
- feat(orchestration): an asker is told how its receiver stands — one ledger status line threaded on the question per change of the receiver's seat (turn_ended, interrupted, awaiting_input, stalled, the stall seat's judged cause, quota_walled, taken_over, finished, seat_vacated, and the two final words cancelled and exited, the first carrying "do not ask it again, and do not summon a replacement"); a final word wakes the blocked ask, every other rides the deadline answer with the time it was seen; Message::answers no longer reads the ledger's voice or a ledger-only kind as an answer, so nothing it writes into a thread is ever taken as one; one line per question × receiver generation × fact, a five-minute cadence and eight non-final lines at most (t-6740)
- feat(jev): the recall seat's label names every note the turn was shown and what became of it, and recall ranks on those answers when the seat acts (t-6264)
- feat(orchestration): a receipt names its verb and every verb call is tallied by day — `worker-read` files no receipt, so `served` alone could never count it (t-6742)
- feat(orchestration): `worker-transcript` answers a worker's conversation as structured steps, out of the transcript its own ledger row reported — never its screen (t-6742)
- feat(board): the live coordination map over the existing relations graph (t-7288)
- feat(computer-use): the realtime layer's Mac reflex runtime — one hand that releases only what it pressed and settles a refused release, a stop that frees what is held before any teardown and reaches a run still starting, a newer bad frame taking back the evidence the last good one gave, a press judged again once the run's boundary has answered, and the desktop's own self-window and target guards on every reflex press; staged off — `live_reflex` stays false until the perception kernel is wired (t-6765)
- feat(computer-use): the realtime layer's perception kernel — colour boards and blobs read in the helper, a demonstration learned only under approval against 42 held-out scenes, one table of limits measured to the 5 ms tick, and a reading that ran past its deadline answers `unknown(budget)` and is no confirmation's evidence; not wired yet (t-6768)
- feat(tools): a research runner that looks for the questions Jev should be asked — replay inputs and final judgments bound, an uncertain bill stopping every purchase and the judgment (t-6349)

### fix
- fix(orchestration): a receiver notice is told once per fact whatever came between, an ask's deadline is one look whose answer is also its receipt, a late event is never pinned on a seat's next occupant or attempt, and a wait the store refused is told on the next sighting (t-6740 r2)
- fix(orchestration): a late fact belongs to one attempt across every effect of its transition, and a question hears its receiver as it has stood since it was asked — the actor's turn end labelled the asker's line with the attempt the turn ended in, but retired readiness and recorded the silence against whoever held the pane when the report landed: a turn that ended inside D1 and arrived after the same worker took D2 went quiet on D2 (its episode and watermark), and one from a pane's last occupant retired the next occupant's readiness window and channel mark and went quiet on its attempt; now `occupant_at` (the worker already in the seat when the fact began) is the one boundary all three readings share, `worker_fell_silent` also asks `carried_at` that the turn ended in the attempt the worker carries now, and `worker_spoke` takes the sound's own clock on both roads the window hears a sound by — the turn's end and the beat's readiness sweep, whose spoken set now carries each sound's state clock; and by the coordinator's rule (m-8284) a fact that began before a question is its news only while the receiver still stands in the attempt the question was put to, so a question asked after D1 was over never hears how D1 ended, while one asked inside D1 hears it and D2's news both (t-6740 r3, astra m-8246 R3)
- fix(jev): the recall label counts what reached the model and what the turn did, a ledger is known by more than its length, and the replay ranks a showing on its own time (t-6264)
- fix(jev): a quote ends where CommonMark ends it, a changed ledger is held to every window the fold read, and one label on a run of requests is no confirmed showing (t-6264)
- fix(jev): a label closes its run but not every turn of it, so what a run's requests outnumber its labels by is owed on to the reading's next run, and a label there is no confirmed showing and folds only what every request it may answer agrees on (t-6264)
- fix(orchestration): `worker-transcript` masks a call's name and id, masks every text before any cut, and reads one open file to one end inside its two-read budget (t-6742 R1-R3)
- fix(orchestration): a `worker-transcript` retry reads on what the call's budget has left — every open reads through one meter, a cut read's bytes stay spent, and the answer's readBytes is the meter's (t-6742 R2)
- fix(restart): a window restart brings each conversation back once, by one road — the ledger's worker as itself, a person's tab as it stood (t-7812)
- fix(restart): a person's orphan is still never reseated — only a taken-over sleeper is the ledger's to bring back (t-7812)
- fix(restart): a sleeper's conversation is seated before its pane runs, and its continuation is said once — after it may have reached the pane (t-7812 r2)
- fix(board): what the live map cannot know, it says it cannot know (t-7288)
- fix(board): the pulse asks the document where the boards are, not the tab bookkeeping (t-7288)
- fix(board): a waiting row that does not say what it waits for answers nothing (t-7288)
- fix(board): the live map reads t-6815's claim words as their own result stage — a worker's "says verified" never becomes the fact's stage or an assignment (t-7288)
- fix(board): the live map's return, failure, count and same-attempt edges (t-7288)
- fix(board): the live map's own beat pays the return debt and sees review-only facts (t-7936)
- fix(board): the live map's event row eases its hover and paints its quiet text from a declared ink — `--ink-quiet` was never declared anywhere, so the facts, the time and the cannot-know chip fell to `inherit`/transparent, and `.agent-live-event-main:hover` snapped where every other state eases; the row now reads `--ink-mist` (the board's own quiet text) and carries `background-color var(--motion-fast) var(--ease-standard)` like the board's other rows — both caught by the shell source contracts (`every_token_the_window_paints_from_is_declared`, `a_state_change_is_eased_not_snapped`) that the branch's window suite does not run: red in the coordinator gate on fae53db7, green here (t-7936 integration)

### perf
- perf(board): the default board stops carrying three elements per card for a map it is not showing (t-7288)
- perf(board): the dress pass stops walking a board that has no beats to write or clear (t-7288)

### docs
- docs(skill): the ask paragraph says what the receiver's lines in your inbox mean — none is an answer, cancelled and exited are final and wake the ask, and cancelled means not to ask again nor summon a replacement (t-6740)
- docs(orchestration): the skill calls `worker-transcript` a separate read of what the screen does not show, not a cheaper screen (t-6742 R4)

### test
- test(jev): a label a closed run of requests may still owe, and its readings on both sides — red first for the recall replay's crossing runs (t-6264)
- test(orchestration): one `worker-transcript` call has one read budget over every open — a retry after a cut reads on what the first open left, and the answer's readBytes counts what the cut read took (t-6742 R2)
- test(restart): the restart button's contract finds the `app.restart();` statement, not the comment that names it (t-7812)
- test(orchestration): the walled-pane letter test says when its worker's turn began — t-6740 gave `pane_turn_began` the turn's own clock (`began_ms`, da655fb2) after this test was written on 53f6e034, so the branch rebased onto 22e3084c did not compile (E0061 at the one call); it now passes `clock()` as every other caller does (t-6349 integration on 22e3084c)

## [1.1.24] — 2026-09-25

_since v1.1.23 (42 commits)_

### feat
- feat(computer-use): the realtime layer's first unit is a contract, not a runtime — a versioned reflex plan the Flow grammar parses strictly beside the old Flow, an opaque validated plan neither Rust nor Swift can forge, one action-lease rule both languages answer from a shared case table, every declared macro checked for references, cycles and budget, and a canonical wire whose keys are sorted explicitly so the plan hash never depends on serde_json's map order (t-6764)
- feat(zo): the refusal ladder is one table — the same model once, then the category's route under `smart.classifierFallback` (off | ask | auto, default ask), a question before a turn leaves the chosen model, and a declined request's images kept unless the person lets them go (t-6747)
- feat(orchestration): a classifier decline stops no summoned worker — Claude workers launch with the CLI's switch on, a decline that still stops is told on two witnesses and handed over only under a declared order, and every switch of model is written down (t-6747)

### fix
- fix(board): the coordinator desk owes an answer only where the ledger would take one — a closed question, a released asker with no reader left or a seat that never spoke leaves the 「답할 우편」 list instead of standing behind an 「답하기」 the ledger refuses (t-7388)
- fix(orchestration): a switch scan's checks of what its cursor counted walk in rounds whose end is fixed as each begins, carried by the cursor itself — a window rewritten in place is checked within 2N − 1 readings for the N windows counted, however fast the transcript grows, where before a file growing a window a beat named the same window on every beat and the others never again (t-7153 r4, R3a, after astra t-6963 on 2e4ea291)
- fix(orchestration): a switch scan checks one more window of what its cursor counted on every reading, in turn — a transcript rewritten in place under its own inode that kept its first line and the bytes before the cursor is read from its start within one round of readings, where before it stood as the file it replaced for good; the platform's file name comes from the one helper the durable stores use; and behind a hook that stays `working` a record ends a worker only past the handover's own refusals (t-7153 R3a + R4, round 3 closing)
- fix(orchestration): a switch scan knows its file by the platform's own name and what it counted, is bound to the ATTEMPT it read under up to the ledger's fence, and holds a refused reading until the ledger does — a same-first-line replacement or in-place rewrite is read from its start, a worker's earlier attempt's switch is never its next attempt's, and a refused switch survives the rotation of its file (t-7153 R3a/R3b/R3c+P2, after astra t-6963 on 220eb41f)
- fix(orchestration): a told pause dialog no longer closes the paused sweep's reading — a pane behind a hook that stays `working` is read again every beat it stands there, and the record the CLI writes once a key answers the dialog, or another request's record, is told and planned on its own key, where before the dialog's notice skipped the reading and neither sweep ever saw the record (t-7153 R4, after astra t-6963 on 220eb41f)
- fix(zo): a refusal question the person cancelled is not answered by a late yes — the answer is awaited beside the turn's abort and re-checked against it as it lands, so an Allow or AllowOnce after the stop consents to nothing for the turn or the session and lets no declined image go (t-7153 R2, after astra t-6963 on e47e3f60)
- fix(orchestration): a switch scan's cursor counts into the FILE it read, not the path, and a switch is written only for the worker whose attempt was open when its CLI wrote it — a transcript replaced as long or longer is read from its start, and a worker seated later at the same path never inherits an earlier worker's switches (t-7153 R3, after astra t-6963 on e47e3f60)
- fix(orchestration): a decline notice is one RECORD's — the same record is told once, and a later record of the same attempt (another request declined, or the record a pause dialog leaves) is told and planned on its own key, where one notice of any kind closed the attempt to every reading after it (t-7153 R4, after astra t-6963 on e47e3f60)
- fix(orchestration): a decline's category is joined on BOTH of its error record's ids — a category record one id names and the other disputes is a contradiction and names nothing, an empty id is no key, and an id a record does not carry disputes nothing (t-7153 R1, after astra t-6963 on e47e3f60)
- fix(zo): silence is not consent on the refusal ladder — a switch question the prompt ceiling closes unanswered, or that nobody at the keyboard can be asked, leaves the turn on the model the person chose and says why; only `auto` switches unasked (t-7153 P1-2, after t-6747 / astra t-6963)
- fix(orchestration): a classifier decline ends a worker only on the conversation's own record — the category read off that record's own request, the plan carrying the record's key and re-proving it at every boundary and last inside the stop's fence; a decline the screen alone witnessed is diagnostic news that walks nowhere, and the pause dialog is read as the CLI's own layout, never as its words (t-7153 P1-3 + P1-4, after t-6747 / astra t-6963)
- fix(orchestration): the model-deviation scan's cursor moves only past switches the ledger holds — the read is pure, the commit follows the actor's answer, and one beat reads one 256 KiB window (t-7153 P2, after t-6747 / astra t-6963)
- fix(orchestration): a summons that pinned a model is launched with its CLI's classifier switch OFF — Claude `switchModelsOnFlag:false`, zo `--classifier-fallback off` — on both launch roads, and a declared `--on-classifier-decline` never turns it back on (t-7153 P1-1, after t-6747 / astra t-6963)
- fix(orchestration): the decline dialog's term is held to its measurement at compile time, and the notice tests borrow the one witness they pass (t-6747, root clippy)
- fix(zo): the text guard grades today's rule on the host's own word, and marks nothing where the host cannot say (t-7058)
- fix(status): 「한 번 실행」 runs only in a pane it opens, a witness number names one row's file and one attempt, and a read that failed is no snapshot — R1b: every pane the agent already holds is the person's, and the pane table's `idle` is a word about the past, not about the line or the foreground at the write, so the run is always a new pane (the launch door spawns the CLI itself; a CLI outside the catalog gets a new plain shell) and the TUI road alone picks a pane, one the program is still in; R2b: a baseline carries its row and its file, the store takes a number out once and refuses a missing, replaced, expired or other-row number where it is consumed — the wait ends there, with nothing snapshotted in its place — and the watch refuses a baseline of another file; the three commands are one-line calls into door bodies (`hold_witness`, `witness_drop`, `wait`) that the runner's tests walk against a sandboxed home; a road's answer paints no row, and only the newest read of the table paints; R2c: a file that could not be read (permission, I/O, not text) is kept apart from an absent one on every snapshot of the table's roads — the witness, the wait with no number, the headless verb — and a look that failed is not a departure (t-7170 r3, astra t-6963 R1b/R2b/R2c)
- fix(status): 「한 번 실행」 runs the program even when a pane is already held, and the credential file is witnessed before the run — R1: a run is a process start, not a word to a running program: the pane table (`paneProgramLeft`, the backend-observed `idle` of a shell that outlived its program) picks the pane the program has left for the table's bare line (`run_command`, one `shell_line` in the backend) and opens a fresh pane through the launch door otherwise, never typing at a pane the program is still in; R2: `cli_login_witness` fixes a `Baseline` on the host and answers a number the window carries into `cli_login_wait`, which judges the file against that earlier snapshot, so a session the CLI renews on start is an arrival on the first look rather than a 300 s timeout — a refused witness runs nothing, a refused launch drops it (`cli_login_witness_drop`), and a walk that fails still re-reads the rows and forces the gauge once; the two source contracts and the window suite's plan-segment check that ccb24da7 left red (`cargo test --bins` never runs `tests/source_contracts`; the `sb-claude-wait` span it removed was still read) are corrected (t-7170 follow-up, astra m-7239 R1/R2, criteria m-7279)
- fix(status): a usage segment with no figure says what to do, by the read's own failure kind — the Claude segment stood as an icon and a triangle after a switch (a stale-token 401 on the v1.1.20 build) and the expired Grok segment as 「···」 with its reason on hover alone; one table maps the wire's failure kinds to words (login expired → run the program, credentials → sign in again, the rest → retry), reading and not-yet-read say 확인 중…, and the roster row and the accounts pane share one 「한 번 실행」 that opens the CLI bare and waits for its file to change before the gauge is re-read; the usage log line was whole all along — the installed v1.1.20 predates it (t-7170)
- fix(orchestration): require live seat and source-bound task reviews (t-6815)
- fix(orchestration): a task's record is corrected only by the run's live coordinator seat, a worker's verified/merged/deployed are its claim, and a review is bound to the attempt it reviewed — the independent audit's F2 and F3 (t-6815)
- fix(board): the coordinator desk deals its flow and lane by measured heights — a fixed two-column grid always leaves one column's foot empty (t-7448)

### docs
- docs(orchestration): the fingerprint's round is 46 readings for the 11.8 MB transcript a worker wrote here, not "a 12 MB transcript's 47" — 256 KiB windows, as the measurement prints (t-7153 R3a)
- docs(core): the classifier ladder's public doc names DECLINE_CONTINUE_WORDS as code, not as a link into a private item — `just doc` (RUSTDOCFLAGS=-D warnings) refused `rustdoc::private-intra-doc-links` at orchestration.rs:12986 (t-7153 round 3, coordinator root-doc rc 101)
- docs(orchestration): HANDED_IN_HEAD and Run::source name the private handed_in_source as code, not an intra-doc link — rustdoc refuses a public page that links a private item (root-doc on the rebased t-6815)
- docs(orchestration): finish once — the briefing carries the acceptance (red-first names, measurements, the reviewer's contracts and negatives), the closing partner reads it before the summon, worker_done needs red/green receipts, and review findings close in the same task (09-24: four of five landings came back for criteria that could have been written first)
- docs(changelog): the 1.1.23 section keeps its five agreed headings, lists each title once, and states the text guard's version 2 as it stands — a mark on every row, the series kept apart by t-6877 (astra m-7206)

### test
- test(orchestration): a transcript that grows a window a beat must still have every window its switch scan counted checked within a bound — a middle window rewritten in place is found within 2N − 1 readings through the real beat and ledger, and every window (first, middle, last) at every phase of the rounds, the file growing 0, ½, 1 or 2 windows a reading (t-7153 r4, R3a, after astra t-6963 on 2e4ea291)
- test(orchestration): a replacement that keeps the first line and the tail and is LONGER is another file too, and what an unchanged long transcript's reading costs is printed by an ignored measurement (t-7153 R3a, round 3 closing)
- test(zo): the text guard's baseline at its real entries — red on the product before t-7058
- test(orchestrator): the worktree-evidence report is read through Run::review_of, so its fixtures go through the ledger's own roads (t-6815)


## [1.1.23] — 2026-09-24

_since v1.1.22 (48 commits)_

### feat
- feat(zo): the two tool guards — a shell command is handed to the command guard right before it runs and each file, web, window-browser or MCP text before the model reads it; shadow asks beside the call and holds nothing, hindsight labels what became of each (t-6348)
- feat(jev): two guard rows — command_guard asks two Nouls of a shell command before it runs, tool_text_guard asks the screen's instructions guard of every block a tool hands back; both record first, rise on hindsight and are held to today's rule (t-6348, Jev plan 7/8)
- feat(ui): a conversation opens at its foot, keeps its reader's place when looked at again, and a reader above gets a button back to the foot (t-6824)
- feat(settings): the classifier card says what the routing feature now needs — every way but Off reaches it, only the probing way calls the Fast-tier model, and the warning stands for Off alone, in five languages (t-6346)
- feat(router): the routing seat's label reads what the turn did — its calls, the files it wrote, the agents it started, as a level within a band of the judgment's — and marks the keyword tables on every fact the seat is marked on (t-6346)
- feat(router): the routing seat judges every turn and spawn it is asked about — one gate table, the probe only where the answer abstains, and a person's named model never walked down (t-6346, folds t-4727)
- feat(jev): a language column — Hangul's share of a request's letters, per thousand, counted by code over the one Hangul table the vault tokenizer already kept (t-6346)
- feat(router): read the routing seat's second version — Scores along their levels, contrastive Choices on their options, facts on their own lines, and the band the complexity answer falls in as the authority it routes with (t-6346)
- feat(api): a contrastive choice — each option described by what it covers, what it is not for and examples, read back by one `options()`/`levels()` pair (t-6346)
- feat(jev): the routing seat's second-version words in the catalog — two Scores, a ten-intent contrastive Choice with its way out, a kind-of-thinking Choice and six facts, one version and one pinned fingerprint (t-6346)
- feat(jev): suggest skills at turn start with two-stage gate

### fix
- fix(jev): the tool text guard's rubric moves to version 2 where its baseline changed meaning — "fenced before" is the host's own fence now, never a phrase in the body; the version marks every row, and reading the two series apart is t-6877's contract (t-6982 follow-up, run-6774 R1)
- fix(zo): keep tool guard fences and Bash cwd host-owned
- fix(zo): the tool guards pass clippy's pedantic set — a path's stamp is Absent or Present rather than an Option of an Option, the two records of independent facts say why they hold four bools, and a refusal's token is named, not wrapped; plus the command path's cost, measured (t-6348)
- fix(ui): the foot's button moves the focus to the list only when it held the focus itself (t-6824)
- fix(router): the routing memo is keyed by the facts beside the words — a spawn retried after a failed attempt carries `retry_of_failed_attempt`, which the complexity question reads, so the same description and prompt under that fact is another state and the first attempt's remembered answer is not recalled for it; `RoutingFacts` derives `Hash` to sit in the key, the regression drives the spawn judgment three times through the fake System One (asked, recalled, asked again), and the model-literal contract now reads a `*_tests.rs` module file as the test source it is declared as (run-6774 V1, t-6346 follow-up)
- fix(orchestration): `check` reads every word before it acknowledges — `--types` was parsed after the acknowledgement, so a kind nobody spelled refused the command with the delivery already retired in memory, behind a refusal that carries no receipt and asks for no durable write, for the next durable write to persist; the words are read first now, a refused command changes nothing, and the regression test replays the still-open batch after the refusal (run-6774 F1)
- fix(router): the routing label agrees when the router would have picked the tier the work needed — trivial and small the fast tier's, medium the balanced, large the strong, read through the router's own table — because within one band a reader that always said small agreed on 401 of 488 turns (t-6346)
- fix(jev): a recording skill suggestion does not hold the turn — the seat's line is shown only when it acts, so only then does the turn wait for its two requests; a shadow or unrisen auto turn hands the judgment to the runtime's worker threads (the patch-review seat's `detach`) and goes on. The pending entry is seated before the judgment so a load early in the turn is still its first-load label, `judged` says whether the judgment landed while the turn was open, and a turn that ends without one writes no label. The mode is read before the skills are discovered, so a seat that is off costs a turn no directory walk; the suggestion note carries the reminder prefix by its constant (t-6347 follow-up)

### perf
- perf(router): a turn the routing seat only records pays 2.4 ms at its start, not 14.1 — the question leaves detached before any reader's check or model list is built, and `auto` reads its standing off the transition lines alone (36.4 → 2.3 ms on a full ledger) (t-6346)

### docs
- docs(jev): the text guard's version 2 marks its rows but does not yet keep the old series out of the window — the shared promotion reader still windows by model alone, and per-version reading is t-6877's contract (t-6982 follow-up, astra m-7097)
- docs(zo): the text guard's baseline comment says what the comparison does — the rule agreed when an unfenced block was not followed (t-6982 follow-up, astra m-7094)

### test
- test: expose stale skill notes in transient state
- test: isolate skill suggestion rows and scope turn notes
- test(jev): the tool guards' replay — sixty-four cases of each kind (irreversible and safe commands, injected and plain texts) asked through the production door and wire, against today's rule, with bands, latency, bytes and a day's cost read off this machine's week of transcripts (t-6348)
- test(router): the memo regression's closure is not mutable — clippy under -D warnings (t-6346 follow-up)
- test(ui): red — a conversation opens at its foot, a reader above gets a way back, and a conversation looked at again keeps its reader's place (t-6824)
- test(router): the memo regression names the runtime's model inventory by its crate path — the sibling test module has no such name in its parent (t-6346 follow-up)
- test(router): the routing seat replayed on this machine's turns — both versions asked through a door of their own, graded on what each turn did beside the keyword tables and a constant reader, with the probe's recorded answers, calls and turn-start model (t-6346)
- test(jev): a seat that never rises names no apply wall — a core contract for the column, beside main's own fix of the vault-pair row (3e92ec3d); zo's summary test was the only one that held every row to it (t-6346)
- test(ui): the code layer's frame gap is judged against the same number of pages, not an absolute thousand-point budget — the 1,131-node grafted scene's worst gap sits at 97–105 ms on a quiet machine against a 96 ms budget that its own control scene (113 ms) does not meet either, so the bound was measuring the scene's size rather than the graft's cost; the contract's own words hold (grafting costs no more than as many pages would, within the same slack), and the absolute budget stays with the thousand-point scene beside it (t-5970 G2 follow-up)
- test(ui): a frame budget reads the machine's load when it is judged, not once at import — a knowledge run beside three worker builds saw the one-minute load climb from 11 to 37, and the import-time reading judged a 100 ms path-lighting frame against a quiet-machine budget that no longer applied (the v1.1.22 lane's red and two solo reruns); machine-load.mjs now reads the load at judgment time, keeps the import-time value for a runner that judges a whole run by it, and the three wall-clock guards added today read it the same way
- test(ui): the thousand-page halo paint ratio is judged only on a quiet machine — the circle-versus-filter paint ratio (1.12 at load 12.6 on 12 cores, bound 1.1) is a wall-clock comparison like the two guarded beside it; recorded, and judged only when the one-minute load is within the core count (machine-load.mjs)
- test(zo): the file-pick hint test reads its note through the persisted reminder wrapper — the seat's line is persisted as a System message inside `<system-reminder>` tags, so the prefix is not the block's first byte; the assertion looked for it there and has been red since the seat landed (t-6344 follow-up)
- test(ui): the knowledge graph's two wall-clock checks are judged only on a quiet machine — the code layer's first-paint ratio (t-5970 G2) and the path route's 2 ms bound flipped under load 26 (the v1.1.22 lane's solo rerun) and load 60 while passing at load 17; both now follow machine-load.mjs like the frame budgets beside them, recording the number and judging only the functional parts when the one-minute load exceeds the core count

## [1.1.22] — 2026-09-24

_since v1.1.21 (15 commits)_

### feat
- feat(jev): add file-pick shadow seat

### fix
- fix(jev): the vault pair seat names no apply deadline — a seat that never rises has no rise line to time, and the dashboard's contract (`a_seat_that_can_rise_has_a_stage_to_time_it`) reddened the v1.1.21 lane on it; the request's own wire deadline stays in the zo runner (t-6345 follow-up)
- fix: F1 keep arbitrary usage errors out of the window log
- fix: F4 require a successful current gauge before reporting quota lifted
- fix: F3 elide transcript data only after proving its image context
- fix: F2 keep vault proposal writes away from linked sources

### docs
- docs: F6 distinguish observed quota recovery from automatic continuation
- docs: F8 remove a private link from the public worktree room contract

### test
- test(jev): the file-pick seat's promotion is read off the table's own lookup, not asserted on the constant — clippy's assertions_on_constants under `-D warnings` (t-6344 follow-up)
- test: record independent load checks for the two shell timing flakes
- test: F7 reject empty or unsuccessful phone speed measurements
- test: F5 include vault pair replay validation in tools-test

## [1.1.21] — 2026-09-24

_since v1.1.20 (48 commits)_

### feat
- feat(jev): record versioned vault pair proposals for review
- feat(board): the desk's worker roster answers worker-list — every worker the ledger still summons, unhealthy first, by one health word from the ledger's own facts (pane gone, quota wall with its reset, waiting for an answer, asleep) or its pane's own state (in a turn, idle), with its last activity, agent, model and effort, pane, checkout, commits past its base and changed files (t-6588)
- feat(crash): a hang first seen unanswered says how long it really lasted when it ends (t-6388)
- feat(board): the desk answers check --peek — the mail each run's coordinator owes, oldest first, with where each letter stands in its inbox; a question is answered where it stands and a held batch acknowledged whole, both as the coordinator seat through the ledger's own verbs, and the screen invents neither (t-6588)
- feat(board): the desk's task flow answers task-list — every task of the runs in play by one stage word from the ledger's own facts, counted by the backend, the stuck stages (gate with its id and question, blocked with the failed prerequisite, failed) in their signal and opened first (t-6588)
- feat(board): the desk's machine strip answers df, uptime and simctl — the ledger's volume with the verdict the next --worktree summons would meet, the load against the cores, the booted simulators and emulators, and the status bar's own loan sentence (t-6588)
- feat(board): the task board's coordinator desk opens with the release lane's own status — version, sha, the phase running and for how long, the lane's verdict and why a phase was skipped (t-6588)
- feat(usage): every usage read leaves one line in the window's log — the road that answered, what the fast road failed of, how long it took and why, never a token or whose login (t-6583)
- feat(settings): describe claim check in five locales
- feat(zo): record claim checks and next-turn labels
- feat(zo): select same-turn evidence for completion claims
- feat(jev): register bounded completion claim seat
- feat(restart-nudge): a resumed worker is told which commands the restart cut under its pane — 「Commands still running under you when the window restarted were cut: `…` — run again whichever you still need」 (t-6428 5)
- feat(exit): closing the window asks the same census — 「지금 닫으면 도는 일이 끊깁니다 — [끝나면 종료] [지금 종료]」 — and a close nobody answers still closes after a minute (t-6428 5)
- feat(exit): every restart door asks the one census first — 「워커 N명 턴 중 · 배경 작업 M개 — [끝나면 다시 시작] [지금 다시 시작]」 — and a wait stands as one line the beat keeps true (t-6428 3)
- feat(exit): 「끝나면」 waits on the beat for the first gap — nothing running under any worker's pane, nothing unread — and asks again or goes when its road's one table says (t-6428 4)
- feat(exit): the 「새 빌드 준비됨」 notice says the one census — workers mid-turn, background jobs, workers nobody could read — instead of counting live workers a second way (t-6428 2)
- feat(orchestration): under a declared wait, a worker still stopped at its wall after the wall lifted is the coordinator's news, once — the wall's follow-up on the same two-witness rule, and nothing is typed (t-6427)
- feat(orchestration): `--on-quota-wall` takes the closed word `wait`, and a wall's order is walked as a ladder — the wait for a verified reset before any handover (t-6427)
- feat(exit): the window's goodbye names the road it leaves by and, for every live worker, the turn and the commands it cuts (t-6428 1)

### fix
- fix(orchestration): the mail pointer holds its line at a pane whose own last answer was a wall — once while it stands, once when it lifts (t-6560)
- fix(board): the desk fits the first screen — a letter and a worker are two lines, the inbox state is a short word with its sentence as the tip, the seat note stands once, and the task flow breaks between its flow and its stuck stages (t-6588)
- fix(jev): the claim seat recommends `auto` like every seat that promotes, and the Hindsight inventory names it (t-6343 follow-up)
- fix(status): a usage read that lands in half a second is on the bar in half a second — the gauge asks every 250 ms for the first two seconds a read is out, then on the old two-second ticks (t-6583)
- fix(usage): a Codex usage read asks with the copy the panes refreshed — the shared runtime home's, when it holds the same login provably fresher (t-6583)
- fix(usage): the Claude OAuth read asks with the login the CLI keeps refreshing — the selected store's scoped keychain item, through `keychain_says` — before the copy the last switch wrote (t-6583)
- fix(awake): the awake standing is asked off the main thread — it waits on the keeper, and the window asks on every resume (t-6388)
- fix(crash): the hang sampler finds the main thread by its dyld root when the queue label is not the main queue's (t-6388)
- fix(crash): the hang watchdog testifies only for time it was there for — a late nap, a sleep between beats or no screen awake withdraws the judgment (t-6388)
- fix(orchestration): a quota wall stands until its reset and the stall grace after it, not for the rest of the attempt — a walled attempt's later silence is news again, and the next window's wall is walled news again (t-6427)

### perf
- perf(board): the desk stands in the board's first frame — its ledger reading is asked before the board awaits its own two, so the task list never jumps down under a person's eye (first task row shift 1170 → 0 px) (t-6588)
- perf(board): a board repaint writes only what moved — a quiet ledger beat 3 → 0 mutations, a beat that moved one worker's words 32 → 2 (t-6588)

### docs
- docs(orchestration): `WorktreeRoom`'s doc names its private reader in code font instead of linking it — rustdoc under `-D warnings` refuses a public doc that links a private item (t-6588 follow-up)
- docs(orchestration): the closed-word parse names its private checker in plain code — rustdoc refuses a public page that links a private item (t-6427)

### test
- test(quota-wall): replay a transcript's typed pointers through the wall hold (t-6560)
- test(board): the task board's weight on one fixture — sixty tasks, five workers, twenty letters: first paint, paint per poll, mutations per poll, elements (t-6588)
- test(jev): seed completion claim transcript replay
- test(orchestration): the wait rung's `lift_read_ms` is the window's usage refetch floor, pinned — the ledger's table and the gauge's floor are one number (t-6427)
- test(orchestration): a bench for the stall sweep's wall question on an attempt that never walled — one newest-first scan of ten thousand rows (t-6427)

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
