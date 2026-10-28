# Built-in browser tabs: the person's control, revisions, upload, diagnosis

Read this when you work in a built-in browser tab: opening one, what the person's pause or hand does to your requests, the revision a mutation carries, `upload`, `diagnose`, `tabs`, `console` and `network`. The text was moved here word for word from the former single-file guide.

Where a paragraph below says "above" or "below", it means the whole guide, which is now split over several files: the table in `SKILL.md` says which file holds what.

For websites, open a dedicated working tab with `zerocode-browser open <url>`.
This is a real page in the window's browser, not another Chrome or Safari
window and not a copy of an external browser's login. Opening it does not
switch the person's tab. The person can watch its actual snapshots, collapse
the viewer without stopping the page, or pause and take control. Watching
does not grant shared mouse or keyboard access. Capture runs only while the
viewer is visible on platforms that support page snapshots. The backend
limits watch captures to one per second and keeps one native capture in
flight per tab. Agent screenshots share that concurrency limit.
Restored and unassigned working tabs start paused. The person selects an
existing terminal before assigning an unbound tab to an agent.
Human control rejects every agent request for that tab, including reads.
Pausing an owned tab also blocks that agent's new writes to other tabs and
new working tabs. Browser steps inside a walk use the same checks.
Managed working tabs refuse arbitrary `eval`. Use `read`, `marks`,
`fields`, `fill`, and the other known browser commands.
Ordinary website scripts and network requests can continue while paused,
and a script, timer or request the site already started can still finish
after a handoff. A handoff first waits briefly until the browser engine
itself has answered the current navigation this window sent (`goto`, the
person's address bar, a popup, a working tab's first page) and then
reports nothing loading in the affected tabs, including a load the page
began itself. A page's own "finished" message, its `readyState` or time
alone never ends the wait, so a load that has not ended keeps control
paused, and asking again stays paused until it ends or the tab is closed.
On Windows a working tab's loading state is unknown until one navigation
seen from its start has ended; until then a handoff stays paused.
A mutation the command refused before anything was sent (its arguments,
URL, selector, mark, text length, scroll target, expression or pane), or
that the page refused before its first event (a control it cannot find,
or one hidden, covered or disabled, a password field typed by keys, a form
other than the one read), leaves control as it was. A mutation that was
sent and then failed, timed out or was canceled, including the page's own
error answer after its first event, an unreadable `eval` or `find` result
and a `fill` that wrote only some fields, pauses that tab and the owner's
other writes. A `workspace_paused` refusal says who lifts the pause:
with `resume=observe` (JSON: `error.resume`), look at that tab again with
`read`, `marks`, `fields` or `screenshot` and continue from what you see;
with `resume=person`, stop and tell the person: control returns only when
the person hands it back, after the affected page answers a bounded
confirmation request. A `workspace_paused` refusal without that word (an
older window's wording) reads as `resume=person`. A tab the person holds by
hand answers every request at once with `workspace_person_controlled` (JSON:
`error.code`, with `error.resume` = `person`): stop, tell the person where
you are, and wait for their 「다시 맡기기」. Until then that agent's other
browser changes and new working tabs are refused too, naming the held tab.
A look still waiting on that tab's page is answered the same way the moment
the person takes the tab. Every refusal ends with what to do next, most
often `zerocode-browser marks <label>`. Never bypass a refusal with another
surface.
When the person hands control back, the window sends the agent holding the
tab one ledger `status` letter (the tab, its mode, its new revision and
「이어서 하세요」, nothing the person did on the page); the pane then gets
the usual mail pointer. Read the page again before acting,
including navigation; previous form and numbered-element observations
have been discarded. After a person has controlled a working
tab, `console`, `network` and `diagnose` refuse that tab for the rest of its
life (`workspace_history_unsupported`); open a new working tab when you need
them. A working tab's step frame is taken and kept only while the same
agent still controls it at the same revision; otherwise the step's evidence
line says `control_changed`. A frame is never an observation.
Every model uses the same observation rule. A successful managed-tab
`read`, `marks`, `fields`, or agent `screenshot` returns `workspaceRevision`.
The backend issues it only if owner, mode, and revision still match at read
completion. JSON carries the root key. Plain output ends with a backend
`workspaceRevision: <u64>` line outside the page's untrusted-content fence.
A managed-tab mutation (`click`, `type`, `fill`, `scroll`, `goto`, `close`,
`find`) sent without `--workspace-revision` uses the revision of your own
last look at that tab, never another agent's. Before your first look it is
refused `workspace_observation_required`; once the tab moved past your look
it is refused `workspace_changed`. Both name `zerocode-browser marks <label>`:
look again, then resend. `wait` and `find` are not looks. A completed
mutation's answer also ends with `workspaceRevision: <u64>` (a JSON answer
carries the key), so the next mutation can follow without another look;
after a mutation that changed the page, look before you rely on what it
shows. Pass `--workspace-revision <u64>` only to bind an action to one
earlier look: a stale value is refused `workspace_stale`, and repeated or
malformed values are refusals. Never query the current control revision to
fill that input. The page's `documentEpoch`, a number in page text, and
watch snapshots cannot supply it. `open` has no prior page observation.
Ordinary tabs need no revision. `tabs` ends each working tab's line with its
mode (`agent`, `person` or `paused`), your last look's revision and the
tab's current one, and shows no title or address for a tab out of your
hands. A look whose page did not answer also says
`workspace_look_unanswered`: whether the page is still loading, and to look
once more before telling the person. A recipe uses observations made during its current
run; include a read before its managed actions. It discards recorded control
revisions. A code handoff keeps the original revision while the person answers.
Working-tab `viewport` requests report that application cannot be confirmed.
The ownership check uses the current window's terminal identity. The browser
bridge also requires its existing capability token. Ownership prevents
accidental interference between agents; it does not bind that shared token
to a terminal's reported identity.
Returning control permits the agent to continue; it is not evidence that
the agent's task has resumed or completed.

`upload` hands 1 to 16 local files (16 MiB in all, absolute paths, no folders)
to a page's file input — or its label when the input is hidden — or, with
`--drop`, to a drop area; use it when a form asks for a file, since `fill`
refuses file fields. Its answer says the files reached the page's list, not
that the site took them: confirm the site's own acceptance with `wait` or
`read`.

When a page looks wrong, ask `diagnose` before guessing: it reads the pane's
own record and ring and answers one verdict in a fixed order — the server
did not answer (dead by the window's clock), the document itself is 4xx/5xx,
the site gates on the browser's name, a login wall, many failed requests,
script errors, or nothing wrong — followed by the facts it was read from.
`--json` returns those facts and the verdict as data.

`open` answers the new pane's label at once (`열림 browser-N`); when the
window is still opening it the answer says so, and `tabs` shows the pane as
soon as it exists. `close` waits for the pane to leave. `viewport` is a size
only — a preset name or `WxH` — because the embedded engine has no device
emulation; `default` gives the pane its seat back.

`tabs` says where every pane stands: `loading`, `finished`, `dead` (a load
that started and never finished within the window's clock — the server did
not answer; wry itself never reports it) or `blank`, with the page's title,
address, profile and reader.

`console` and `network` read the page's own observation ring — what the page
logged and every request it made (fetch, XHR, images, scripts), with method,
status and timing but never a header or a body. Each answer ends with a
`# seq … · last …` line: pass `--since <last>` to read what came after. A
pane opened before the ring existed answers that it has no ring; reload it.

For an iPhone, an iPad, an Android phone, a simulator or an emulator, use the
`mobile-device` skill. It holds every `zerocode-emulator` command, the look →
press loop and what to say about a phone this window cannot drive; this skill
keeps no copy of them.
