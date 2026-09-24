use super::*;

/// How many things one card remembers having done.
///
/// Twenty, which is a screenful of history and a bill of about two kilobytes
/// per card at the target's clamp ([`zerocode_core::hook::ACTIVITY_TARGET_CHARS`]
/// is 120). The cap is the whole design: an agent working for an hour makes
/// thousands of tool calls, a person can read the last few, and an unbounded
/// list of what an agent did is a memory leak wearing a feature's clothes.
pub(super) const ACTIVITY_RING: usize = 20;

/// At most one `hook:activity` per card per this long.
///
/// An agent at work fires two events per tool call and can fire them faster
/// than a person can read — this window already learned that lesson on the
/// painting side (`scheduleAgentPaint`, one frame) and this is the same floor
/// on the SENDING side, because a webview deserialises every emit on the
/// thread that owes the next keystroke.
pub(super) const ACTIVITY_EMIT_EVERY: Duration = Duration::from_millis(100);

/// One thing an agent did, numbered.
///
/// The number is per card and only ever goes up, so a window that missed an
/// emit can tell — the batch it gets next carries a gap, and `pane_activities`
/// is the road back to what fell in it.
#[derive(Debug, Clone, Serialize)]
pub(super) struct StampedActivity {
    pub(super) seq: u64,
    pub(super) activity: zerocode_core::hook::Activity,
}

/// What one card has been doing, and when it last said so.
///
/// Empty is the honest start for all four fields, so the default is derived:
/// no history, nothing owed, and — the one that matters — `last_emit` at
/// `None` rather than at some `Instant`, so a card's FIRST activity leaves
/// immediately instead of waiting out a window it was never in.
#[derive(Debug, Default)]
pub(super) struct ActivityRing {
    pub(super) held: VecDeque<StampedActivity>,
    pub(super) next_seq: u64,
    /// How many of the newest entries have not been emitted yet.
    pub(super) pending: usize,
    pub(super) last_emit: Option<Instant>,
}

impl ActivityRing {
    /// Remember one, dropping the oldest once the ring is full.
    pub(super) fn note(&mut self, activity: zerocode_core::hook::Activity) {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.held.push_back(StampedActivity { seq, activity });
        while self.held.len() > ACTIVITY_RING {
            self.held.pop_front();
        }
        // Never more than the ring holds: a burst that overflowed the ring has
        // already lost its oldest entries, and claiming to owe the window
        // entries that no longer exist would slice past the front of the deque.
        self.pending = (self.pending + 1).min(self.held.len());
    }

    /// Remember one only if it is not what this card is already saying, and
    /// answer whether it moved.
    ///
    /// For the road whose events are SNAPSHOTS rather than events: zo names
    /// what each helper is doing in every `subagents` frame it sends, so the
    /// same line arrives for as long as the helper is on the same tool, and a
    /// ring that took each of them would spend a repaint per frame saying
    /// nothing new. Hook envelopes still go through [`ActivityRing::note`]
    /// unchanged — those are events, and two identical events are two things
    /// that happened.
    pub(super) fn note_fresh(&mut self, activity: zerocode_core::hook::Activity) -> bool {
        if self
            .held
            .back()
            .is_some_and(|last| last.activity == activity)
        {
            return false;
        }
        self.note(activity);
        true
    }

    /// Everything since the last emit — when the floor allows one.
    ///
    /// **The throttle is here and nowhere else, and it has no timer.** A
    /// deadline needs something to fire it, and the two candidates were both
    /// worse than the debounce: a timer thread per card is a thread per agent
    /// for text, and hanging this on the pump would put hook events — which
    /// arrive when an agent decides something happened — back on the display
    /// clock the forwarding loop was deliberately taken off.
    ///
    /// So a batch rides the NEXT envelope for the same card. What that costs
    /// is exactly one case: the last activity of a burst that is then followed
    /// by silence sits in the ring, unsent, until the agent does something
    /// else. It is bounded (the ring holds it, `pane_activities` hands it
    /// over, and the next event carries it), it is invisible on the surface
    /// this feeds (a card only draws its newest line while the agent is
    /// RUNNING, and an agent that has gone quiet is drawn by its state), and
    /// the first event after any quiet stretch always leaves at once — which
    /// is the case a person actually watches.
    pub(super) fn due(&mut self, now: Instant) -> Option<Vec<StampedActivity>> {
        if self.pending == 0 {
            return None;
        }
        if let Some(last) = self.last_emit
            && now.duration_since(last) < ACTIVITY_EMIT_EVERY
        {
            return None;
        }
        self.last_emit = Some(now);
        let batch = self
            .held
            .iter()
            .skip(self.held.len() - self.pending)
            .cloned()
            .collect();
        self.pending = 0;
        Some(batch)
    }
}

/// File one thing a helper is doing under its own card, and hand the window
/// whatever the floor lets through.
///
/// The snapshot road's end of what [`hook_loop`] does for envelopes: the same
/// ring, the same 100ms floor, the same `hook:activity` shape — because a
/// vendor that composes its own activity line is still describing the thing
/// every other vendor describes, and the window has exactly one `activityLine`
/// to draw it with.
pub(super) fn note_helper_activity(
    app: &AppHandle,
    card: String,
    activity: zerocode_core::hook::Activity,
) {
    let batch = {
        let state = app.state::<AppState>();
        let mut held = state.activities();
        let ring = held.entry(card.clone()).or_default();
        if !ring.note_fresh(activity) {
            return;
        }
        ring.due(Instant::now())
    };
    if let Some(activities) = batch {
        let _ = app.emit(
            "hook:activity",
            PaneActivities {
                pane: card,
                activities,
            },
        );
    }
}

/// Say what a pane's helper roster is NOW — the WHOLE list, not the change:
/// a window that missed an event (or opened after them) is corrected by the
/// next one rather than drifting, and a board popped out later seeds from
/// `pane_subagents` with the identical shape. When the last helper leaves,
/// the Done the roster was holding back lands here — the lead's Stop already
/// happened and nothing else will ever say it again, so this replay is the
/// all-clear. One door for both writers (the lifecycle branch and the
/// roll-call fold), because the replay rule written twice is the replay rule
/// one of them forgets.
pub(super) fn publish_pane_subagents(app: &AppHandle, term: TermId, rows: Vec<hooks::SubagentRow>) {
    // "Empty" is no helper still RUNNING: finished rows stay in the list for
    // the person to open, and they are not what the parked Done waits on.
    let emptied = !hooks::any_helper_running(&rows);
    let _ = app.emit("hook:subagent", PaneSubagents { term, rows });
    if emptied {
        let parked = app.state::<AppState>().pending_done().remove(&term);
        if let Some((worktree, held)) = parked {
            note_pane_state(app, Some(&worktree), &held);
            ring_for_pane(app, &worktree, &held);
        }
    }
}

/// Forward hook events to the window, forever.
///
/// A task rather than a thread, and separate from the pump: hook events arrive
/// when an agent decides something happened, which has nothing to do with the
/// display rate. Putting them on the pump would either delay them by a frame or
/// wake the pump for work it has none of.
///
/// The launch token is checked HERE, against what this window handed the pane,
/// because the state map is here — `hooks::report_of` is given the answer rather
/// than reaching for it.
pub(super) async fn hook_loop(
    app: AppHandle,
    mut events: tokio::sync::mpsc::UnboundedReceiver<zerocode_core::HookEnvelope>,
) {
    while let Some(envelope) = events.recv().await {
        if let Some(term) = hooks::term_of_pane_key(&envelope.pane_key) {
            // The script removed its marker before this envelope could arrive.
            // Clear the beat's cached copy now too, so a paint in this same
            // frame cannot call a recovered bridge unreachable.
            hooks::delivery_arrived(term);
        }
        let expected = hooks::term_of_pane_key(&envelope.pane_key)
            .and_then(|term| app.state::<AppState>().launch_tokens().get(&term).cloned());
        // A mirror announcing itself: an agent run as a command now has a
        // byte-for-byte file, and the window pours it into a real pane
        // (1-fm). The path is held to the mirror directory before anything
        // travels — this channel is the bridge's, and anything local can
        // knock on it.
        if let Some(event) = zerocode_core::hook::envelope_event_name(&envelope)
            && let Some(step) = match event.as_str() {
                "ZerocodeMirrorStart" => Some("worker:mirror"),
                "ZerocodeMirrorEnd" => Some("worker:mirror_end"),
                _ => None,
            }
        {
            if let Some(term) = hooks::term_of_pane_key(&envelope.pane_key)
                && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&envelope.payload)
                && let Some(vendor) = parsed.get("vendor").and_then(serde_json::Value::as_str)
                && let Some(path) = parsed.get("path").and_then(serde_json::Value::as_str)
                && let Some((_, mirrors)) = hooks::mirror_shims()
                && std::path::Path::new(path).starts_with(&mirrors)
            {
                let _ = app.emit(
                    step,
                    MirrorNote {
                        term,
                        agent: vendor.to_string(),
                        path: path.to_string(),
                    },
                );
            }
            continue;
        }
        // The transcript road (t-3233 §2): a Claude or zo pane's turn ending
        // names its transcript, and the artifacts it published and the pages
        // it wrote are registered from the tail. Before the classifiers, for
        // the same reason the activity is: nothing here `continue`s.
        artifact_runtime::note_hook(&app, &envelope);
        // What the agent just DID, before either classifier gets a chance to
        // end the iteration. This road does not `continue`: a tool call is
        // also the event that says the pane is working, and the two facts
        // travel from the same envelope on purpose — the ONLY reason the
        // detail was ever lost is that the classifiers read the envelope for
        // their own answer and dropped what was left.
        if let Some((pane, activity)) = hooks::activity_of(&envelope, expected.as_deref()) {
            // A nested run's tool calls arrive wearing the pane's identity too,
            // and this road does more than draw them: it is where a nested run
            // is DISCOVERED. Ungated, a child's own `codex exec` was registered
            // as the parent pane's run — a grandchild page under the wrong
            // pane, and a finish that the `(pane, vendor)` fallback could spend
            // on the PARENT's page — and the child's every tool call was filed
            // on the lead's card. The person still sees the child's work: it is
            // the page's own raw stream, which is the place for it.
            //
            // The lead's own call passes: at this instant no run of the LEAD's
            // vendor is in flight — the one it is starting is registered a few
            // lines below, and the one it is finishing is still the child's.
            if !hooks::term_of_pane_key(&envelope.pane_key).is_none_or(|term| {
                speaks_for_its_pane(
                    &app,
                    term,
                    envelope.agent.slug(),
                    zerocode_core::session_in_payload(envelope.agent, &envelope.payload)
                        .as_ref()
                        .map(|session| session.id.as_str()),
                    // A tool call is never a turn ending, so it never reaches
                    // the one clause that suppression is reserved for.
                    false,
                )
            }) {
                continue;
            }
            // An agent RUNNING an agent — `codex exec …` under a Bash tool.
            // No SubagentStart will ever come (codex was started as a
            // command, not as a helper), so the command line is the only
            // witness — and the worker leaves through the same roster and
            // the same emit as a real helper, so the sidebar, the board and
            // the orchestration tiles all follow without a second pipeline.
            // The WHOLE command, out of the payload — not `activity.target`,
            // which is clamped to a row's width. Measured consequence of the
            // clamp: `cd /<a long path> && codex exec …` named no agent at
            // all, so no page opened and every report that child sent was
            // taken for the pane's own.
            let nested = (activity.verb == zerocode_core::hook::Tool::Bash)
                .then(|| {
                    zerocode_core::hook::worker_command(&envelope.payload)
                        .as_deref()
                        .and_then(zerocode_core::hook::nested_agent_of)
                })
                .flatten()
                .zip(hooks::term_of_pane_key(&envelope.pane_key));
            crumbs::record(
                "hook",
                format_args!("agent={} tool={:?}", envelope.agent.slug(), activity.verb),
            );
            skills_runtime::note_hook(&app.state::<AppState>(), &envelope, &activity);
            file_tree_hooks::note_hook(&app, &app.state::<AppState>(), &envelope, activity.phase);
            let phase = activity.phase;
            // Whose card this is, read BEFORE the name travels — the emit
            // below takes it — because a helper's tool call is also a NUMBER.
            let helper = hooks::helper_in_card(&pane).map(|(term, id)| (term, id.to_string()));
            let batch = {
                let state = app.state::<AppState>();
                let mut held = state.activities();
                let ring = held.entry(pane.clone()).or_default();
                ring.note(activity);
                // Coalesced HERE rather than in the window: an emit costs the
                // webview a deserialise on the thread that owes the next
                // keystroke, and four agents at work fire faster than any
                // surface can draw. See `ActivityRing::due` for why the floor
                // is a debounce and not a timer.
                ring.due(Instant::now())
            };
            // The count Claude Code puts on a running helper's line ("12 tool
            // uses"), for the vendors that only fire events — zo counts its
            // own and says the number in its frame, and the roster takes the
            // greater of the two, so no helper is ever counted twice.
            //
            // Counted from the START of a call, so the number moves when the
            // tool is picked up rather than a beat later, and republished only
            // when the activity it arrived with was let through: the roster
            // then rides the same floor as the line it stands beside, instead
            // of costing the webview a deserialise per tool call per helper.
            // Taken after the activity lock is dropped, so the two maps are
            // never held at once.
            let moved = batch.is_some();
            let counted = helper
                .filter(|_| phase == zerocode_core::hook::Phase::Started)
                .and_then(|(term, id)| {
                    let state = app.state::<AppState>();
                    let mut held = state.subagents();
                    let roster = held.get_mut(&term)?;
                    let seat = roster.iter_mut().find(|one| one.id == id)?;
                    seat.tool_calls = seat.tool_calls.saturating_add(1);
                    moved.then(|| (term, roster.clone()))
                });
            if let Some(activities) = batch {
                let _ = app.emit("hook:activity", PaneActivities { pane, activities });
            }
            if let Some((term, rows)) = counted {
                publish_pane_subagents(&app, term, rows);
            }
            if let Some((kind, term)) = nested {
                use zerocode_core::hook::Phase;
                /* The CALL names the row, not the vendor. `run:codex` for
                 * every codex run meant two parallel runs of one vendor were
                 * two rows with one id, and the finish that came first
                 * retired whichever row it found — the reversed-finish case
                 * the navigator design names. The id is the tool call's own
                 * (`tool_use_id` and its spellings), which every vendor puts
                 * on the Started AND the Finished of one call; a payload
                 * without one falls back to the pane's own sequence, and its
                 * finish then takes the OLDEST running row of that vendor —
                 * the same guess as before, now only where nothing better is
                 * on the wire. */
                let named = zerocode_core::hook::worker_call_id(&envelope.payload);
                let id_of = |call: &str| format!("run:{}:{}", kind.slug(), call);
                let rows = {
                    let state = app.state::<AppState>();
                    let mut held = state.subagents();
                    let running = held.entry(term).or_default();
                    match phase {
                        // Pushed, never replaced: two parallel runs of the
                        // same vendor are two rows, and each finish takes
                        // exactly its own back.
                        Phase::Started => {
                            let call = named.clone().unwrap_or_else(|| {
                                format!(
                                    "{}:{}:{}",
                                    term,
                                    kind.slug(),
                                    WORKER_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                                )
                            });
                            running.push(hooks::SubagentRow {
                                id: id_of(&call),
                                name: kind.label().to_string(),
                                state: hooks::SubagentState::Running,
                                // A run's own tool calls are filed under its
                                // own card, and counted there like any
                                // helper's.
                                tool_calls: 0,
                                // A worker page's row is lifecycle-owned: its
                                // finish retires it, never a roll call that
                                // has no idea nested vendor runs exist.
                                born_listed: false,
                                transcript: None,
                                registry: None,
                            });
                        }
                        Phase::Finished | Phase::Failed => {
                            // Its own row when the call is named; else the
                            // oldest RUNNING one of that vendor — a finished
                            // row of the same vendor may still stand from an
                            // earlier run, and it is not the one this finish
                            // is for.
                            let own = named.as_deref().map(id_of);
                            let vendor = format!("run:{}:", kind.slug());
                            if let Some(seat) = running.iter_mut().find(|one| {
                                one.state == hooks::SubagentState::Running
                                    && own.as_deref().is_none_or(|own| one.id == own)
                                    && one.id.starts_with(&vendor)
                            }) {
                                hooks::retire_helper(seat);
                            }
                            hooks::bound_done_helpers(running);
                        }
                        Phase::Prompted | Phase::Stopped => {}
                    }
                    let rows = running.clone();
                    if rows.is_empty() {
                        held.remove(&term);
                    }
                    rows
                };
                if matches!(phase, Phase::Started | Phase::Finished | Phase::Failed) {
                    let _ = app.emit("hook:subagent", PaneSubagents { term, rows });
                }
                // And the run's own PAGE — the row alone was reported back as
                // "코덱스 창이 안 보임. 창이 열리면서 보여야 하는데". The
                // window opens a viewer tab on `worker:opened` and fills it
                // from `worker:done`; what travels is read by the core's
                // defensive readers, never straight off the payload.
                match phase {
                    Phase::Started => {
                        let run = named.unwrap_or_else(|| {
                            format!(
                                "{}:{}:{}",
                                term,
                                kind.slug(),
                                WORKER_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                            )
                        });
                        let command = zerocode_core::hook::worker_command(&envelope.payload)
                            .unwrap_or_default();
                        app.state::<AppState>()
                            .workers()
                            .insert(run.clone(), (term, kind.slug()));
                        let _ = app.emit(
                            "worker:opened",
                            WorkerOpened {
                                id: run,
                                term,
                                agent: kind.slug().to_string(),
                                name: kind.label().to_string(),
                                command,
                                worktree: envelope.worktree_id.clone(),
                            },
                        );
                    }
                    Phase::Finished | Phase::Failed => {
                        let run = {
                            let state = app.state::<AppState>();
                            let mut workers = state.workers();
                            let key = named.filter(|id| workers.contains_key(id)).or_else(|| {
                                // A payload that names no id still ends a
                                // run: the most recent page this pane and
                                // vendor opened is the one that finished.
                                workers
                                    .iter()
                                    .find(|(_, seat)| **seat == (term, kind.slug()))
                                    .map(|(id, _)| id.clone())
                            });
                            if let Some(id) = &key {
                                workers.remove(id);
                            }
                            key
                        };
                        if let Some(run) = run {
                            let (output, truncated) =
                                zerocode_core::hook::worker_output(&envelope.payload)
                                    .map_or((None, false), |(text, cut)| (Some(text), cut));
                            let _ = app.emit(
                                "worker:done",
                                WorkerDone {
                                    id: run,
                                    status: if phase == Phase::Failed {
                                        "failed"
                                    } else {
                                        "done"
                                    },
                                    output,
                                    truncated,
                                },
                            );
                        }
                    }
                    Phase::Prompted | Phase::Stopped => {}
                }
            }
        }
        // The roll call Claude rides on its Stop payloads — the only road a
        // BACKGROUND helper's existence is ever reported on: launching one
        // fires no `SubagentStart` at all (measured 2026-08-18 — a synthetic
        // start through the installed hook drew a row while two real
        // background scouts drew none). Orca folds the same array into the
        // roster its lifecycle events feed
        // (`foldClaudeBackgroundTasksIntoRoster`), and so does this. Before
        // the lifecycle branch, and no `continue`: the same envelope may be a
        // `SubagentStop` whose list reconciles the rest of the roster, and it
        // may still be the pane report the roads below answer.
        if let Some((term, reading)) = hooks::background_tasks_of(&envelope, expected.as_deref()) {
            let (rows, retired) = {
                let state = app.state::<AppState>();
                let mut held = state.subagents();
                let running = held.entry(term).or_default();
                let before: Vec<String> = running.iter().map(|one| one.id.clone()).collect();
                let changed = hooks::fold_background_tasks(running, &reading);
                let retired: Vec<String> = before
                    .into_iter()
                    .filter(|id| !running.iter().any(|one| &one.id == id))
                    .collect();
                let rows = changed.then(|| running.clone());
                if running.is_empty() {
                    held.remove(&term);
                }
                (rows, retired)
            };
            // A helper the roll call retired takes its stream with it, the
            // way an explicit stop does — nothing will ever name these keys
            // again. Taken after the roster's lock is dropped.
            for id in &retired {
                app.state::<AppState>()
                    .activities()
                    .remove(&hooks::activity_subagent(term, id));
            }
            if let Some(rows) = rows {
                publish_pane_subagents(&app, term, rows);
            }
        }
        // A helper starting or stopping inside a pane's agent. Asked FIRST and
        // answered separately because `report_of` says nothing about these
        // events by design — a helper is not its parent's state — so they used
        // to fall out of the loop here and reach nobody. They do not call tmux
        // and do not earn a pane (Orca splits only for teams and tmux, and
        // shows helpers as rows: `useWorktreeAgentRows-BZzQmOQU.js:14-30`), so
        // this road ends at a row under the parent card, not at a new card.
        if let Some((term, step, row)) = hooks::subagent_of(&envelope, expected.as_deref()) {
            // And this road asks too, for the reason the two above it do: a
            // nested claude's own helpers arrive wearing the parent's pane key,
            // and this road ends in a `continue` — so an ungated child's
            // `SubagentStart` put a row on the LEAD's card and left, never
            // reaching the report gate at all.
            if !speaks_for_its_pane(
                &app,
                term,
                envelope.agent.slug(),
                zerocode_core::session_in_payload(envelope.agent, &envelope.payload)
                    .as_ref()
                    .map(|session| session.id.as_str()),
                false,
            ) {
                continue;
            }
            // A helper that stopped takes its stream with it. The row goes
            // below; this is the card those activities were filed under, and
            // nothing else will ever name that key again.
            let ended = matches!(step, zerocode_core::hook::SubagentStep::Stop)
                .then(|| hooks::activity_subagent(term, &row.id));
            let rows = {
                let state = app.state::<AppState>();
                let mut held = state.subagents();
                let running = held.entry(term).or_default();
                match step {
                    zerocode_core::hook::SubagentStep::Start => {
                        // Replace rather than append on a repeated id: a vendor
                        // that reports the same helper twice has one helper.
                        if let Some(seat) = running.iter_mut().find(|one| one.id == row.id) {
                            *seat = row;
                        } else {
                            running.push(row);
                        }
                    }
                    zerocode_core::hook::SubagentStep::Stop => {
                        // Finished, not gone: the row and its page outlive
                        // the run, greyed, until the session turns. Unless
                        // this vendor's stop only closed the SPAWN — then
                        // nothing finished and the row goes, because the
                        // helper it announced lives on as the row the roll
                        // call above named by its own id (t-3098).
                        hooks::close_helper_row(
                            running,
                            &row.id,
                            envelope.agent.subagent_stop_closes_the_spawn(),
                        );
                    }
                }
                let rows = running.clone();
                // An empty list is an entry nobody needs; the emit below still
                // carries the empty list, which is what clears the last row.
                if rows.is_empty() {
                    held.remove(&term);
                }
                rows
            };
            // Taken after the roster's lock is dropped, so the two maps are
            // never held at once and no road can ever want them in the other
            // order.
            if let Some(card) = ended {
                app.state::<AppState>().activities().remove(&card);
            }
            publish_pane_subagents(&app, term, rows);
            continue;
        }
        // An agent whose session ended has no more use for what it borrowed
        // (t-6336) — and its shell may stay, so the pane closing would never
        // say so. Gated like every road here: a nested run's `SessionEnd`
        // wears the pane's key, and it is not the pane's session ending.
        if let Some(term) = hooks::session_end_of(&envelope, expected.as_deref())
            && speaks_for_its_pane(
                &app,
                term,
                envelope.agent.slug(),
                zerocode_core::session_in_payload(envelope.agent, &envelope.payload)
                    .as_ref()
                    .map(|session| session.id.as_str()),
                true,
            )
        {
            crate::emulator::borrower_gone(term, crate::emulator::LoanEnd::SessionEnded);
        }
        let Some(mut report) = hooks::report_of(&envelope, expected.as_deref()) else {
            continue;
        };
        // A nested run's hooks wear the PANE's identity — it inherited the
        // pane key and the launch token from the agent that ran it — so the
        // envelope alone cannot tell the child from the lead. Dropped whole,
        // and dropped HERE: the next line retires the lead's parked all-clear,
        // the boundary branch below empties its helper roster, and the three
        // roads at the end write its session, its dot and its ring.
        if !report_speaks_for_its_pane(&app, &report) {
            continue;
        }
        // Whether a PERSON ended this turn. Asked BEFORE the done gate below,
        // because that gate reads it: Orca computes `interrupted` first and
        // hands it to `resolveClaudePaneState` (`agent-hook-listener.ts:2962`
        // then `:3126`), and asking after would gate on a flag that is still
        // false for the event that just declared it. See
        // `declare_pane_interrupt` for why the pane's memory is half the answer.
        declare_pane_interrupt(&app, &mut report, &envelope.payload);
        // Whatever this event says, it is a newer word than a parked
        // all-clear: a lead that spoke again (a fresh prompt, a question, a
        // newer stop) makes the held Done stale, and the gate below re-parks
        // the ones that still stand.
        app.state::<AppState>().pending_done().remove(&report.term);
        if report.session_boundary {
            // A new process owns the pane: stale helpers belonged to the
            // conversation that ended, and left standing they would gate the
            // fresh session's idle row back up to working below — Orca
            // clears roster, tasks and crons on `SessionStart` (its comment
            // names the same reset Codex does).
            clear_pane_subagents(&app, report.term);
        } else if report.state == zerocode_core::hook::HookState::Done
            && pane_still_working(&app, report.term, &envelope.payload, report.interrupted)
        {
            // A turn that ends while helpers still run has not ended for the
            // PERSON. Stop is the lead's own word — with five helpers going,
            // letting it land dropped the card to the completed column and
            // rang a completion mid-work: the map's P0-4. Orca's
            // `resolveClaudePaneState` holds the pane at working on a live
            // roster or a non-empty background inventory; the roster here is
            // the same proxy (rows leave on `SubagentStop`), the inventory
            // is the payload's own.
            //
            // PARKED, not dropped: nothing else may ever say Done for this
            // turn — the lead already said it — so the last helper's stop
            // replays this as the all-clear (Orca re-derives the pane on
            // every roster change; the parked row is our spelling of its
            // `turnCompletedAt` pairing).
            app.state::<AppState>()
                .pending_done()
                .insert(report.term, (envelope.worktree_id.clone(), report.clone()));
            report.state = zerocode_core::hook::HookState::Working;
        }
        // An agent reporting through a hook has identified itself better than
        // the argv guess ever could — a `claude` started by hand inside a plain
        // shell says so here and nowhere else.
        // Replaced, not merely filled in: a report that has been judged the
        // pane's own IS the pane, and a first-writer-wins map left a person who
        // quit one agent in a terminal and started another with a pane
        // remembered as the old vendor forever — which then classified the new
        // agent's every word as foreign and threw it away. The gate above is
        // what makes overwriting safe.
        app.state::<AppState>()
            .agent_terms()
            .insert(report.term, report.agent.slug());
        // And which conversation it is. Kept rather than only forwarded, because
        // the point of the id is to outlive the tab: the window asks for it again
        // when somebody wants to reopen a session that was closed.
        if let Some(reported) = report.session.clone() {
            // Replaced, but not stripped: an event that names the same
            // conversation without saying where it is written down keeps the
            // path the window already learned, or the conversation view goes
            // blank on the person's first message.
            let session = {
                let state = app.state::<AppState>();
                let mut held = state.pane_sessions();
                let carried = reported.carrying_forward(held.get(&report.term));
                held.insert(report.term, carried.clone());
                carried
            };
            codex_queue::bind_session(report.term, &session);
            orchestration::pane_session_reported(report.term, &session, now_epoch_ms());
        }
        // And what it is doing, stamped, and out to the window.
        note_pane_state(&app, Some(&envelope.worktree_id), &report);
        ring_for_pane(&app, &envelope.worktree_id, &report);
        note_automation_completion(&app, &report);
    }
}

/// How long the surface is given to repaint after an action before the
/// window takes the frame that proves it — a tap's ripple, a page's paint.
/// The core's table, which zo's look after an act reads too.
pub(super) const EVIDENCE_SETTLE: Duration =
    Duration::from_millis(zerocode_core::computer_use::COMPUTER_SETTLE_MS);
/// How much of an agent's closing answer a run row keeps as its result.
pub(super) const RUN_RESULT_CHARS: usize = 2000;

/// `<local data root>/automations/<job>/runs/<started at>/` — where one run
/// leaves what it produced. Named by the start instant rather than the run
/// id because it has to exist before the shell (and its terminal number) does.
pub(super) fn run_evidence_dir(
    local_data_root: &Path,
    automation_id: &str,
    at_epoch_ms: i64,
) -> PathBuf {
    local_data_root
        .join("automations")
        .join(automation_id)
        .join("runs")
        .join(at_epoch_ms.to_string())
}

/// Make one run's evidence folder and answer its path, or `None` with a note
/// in the window log: a folder that could not be made is not a failed run.
pub(super) fn make_run_evidence(
    local_data_root: &Path,
    automation_id: &str,
    at_epoch_ms: i64,
) -> Option<String> {
    let dir = run_evidence_dir(local_data_root, automation_id, at_epoch_ms);
    match std::fs::create_dir_all(&dir) {
        Ok(()) => Some(dir.display().to_string()),
        Err(error) => {
            note_window_event(
                local_data_root,
                &format!("automation {automation_id}: evidence folder not made: {error}"),
            );
            None
        }
    }
}

/// The prompt as delivered: the person's words, then one line naming the
/// evidence folder — a QA job told "leave screenshots" needs an answer to
/// "where", and the folder is what the run's row lists afterwards.
pub(super) fn prompt_with_evidence(prompt: &str, evidence_dir: Option<&str>) -> String {
    match evidence_dir {
        Some(dir) => format!(
            "{prompt}\n\nEvidence folder for this run: {dir}\nEvery zerocode-emulator, zerocode-browser and zerocode-computer action is logged there automatically with the screen after it (a quick burst on one surface is shown by its last step's screen). Save any extra files there and finish by writing report.md (a verdict per step, naming the screenshots) and quoting the folder in your answer. A QA scenario also ends with `zerocode-computer verdict --pass` or `--fail --reason <why>` (a failing verdict is filed as a task), and checks a screen against a baseline with `zerocode-computer compare --baseline <png>`."
        ),
        None => prompt.to_string(),
    }
}

/// Sweep the evidence folders of rows the ledger just forgot. Best effort and
/// fenced to the automations tree: a row that named somewhere else is left.
pub(super) fn forget_run_evidence(local_data_root: &Path, forgotten: &[AutomationRun]) {
    let fence = local_data_root.join("automations");
    for run in forgotten {
        let Some(dir) = run.evidence_dir.as_deref().map(Path::new) else {
            continue;
        };
        if dir.starts_with(&fence) {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// A `done` from a run's shell: the row learns the agent completed and what it
/// said, the window hears it, and — when the job asked — the shell is retired
/// so a daily job does not leave yesterday's pane listening. The ledger is
/// read once per `done`, which is rare, and most of those are nobody's run.
pub(super) fn note_automation_completion(app: &AppHandle, report: &hooks::PaneHookReport) {
    if report.state != zerocode_core::hook::HookState::Done || report.session_boundary {
        return;
    }
    let state = app.state::<AppState>();
    let local_data_root = state.local_data_root().to_path_buf();
    // The shell this `done` came from, by fingerprint: the terminal id alone
    // names every shell that ever wore it, and a run's row must not take the
    // word of the id's next occupant.
    let launch = crate::cmd::terminal::launch_of(&state, report.term);
    let completed = {
        let _store = automation_store_domain();
        let mut history = stored_automation_runs(&local_data_root);
        let result = report
            .said
            .as_deref()
            .map(|said| said.chars().take(RUN_RESULT_CHARS).collect::<String>());
        let completed = mark_completed(&mut history, report.term, launch, now_epoch_ms(), result);
        if completed.is_some() {
            let _ = write_automation_runs(&local_data_root, &history);
        }
        // The run's evidence folder, for the verdict it may hold (§4).
        let evidence_dir = completed.as_ref().and_then(|(_, run_id)| {
            history
                .iter()
                .find(|run| run.id == *run_id)
                .and_then(|run| run.evidence_dir.clone())
        });
        (completed, evidence_dir)
    };
    let (completed, evidence_dir) = completed;
    let Some((automation_id, run_id)) = completed else {
        return;
    };
    // A QA scenario that failed leaves its verdict in the folder; the beat
    // files it as a task through the seat (`qa_triage`).
    if let Some(dir) = evidence_dir.as_deref()
        && let Some((false, reason)) = crate::computer_use::evidence::read_verdict(Path::new(dir))
    {
        let _ = crate::qa_triage::note_failure(
            &local_data_root,
            &crate::qa_triage::QaFailure {
                run_id: run_id.clone(),
                automation_id: automation_id.clone(),
                evidence_dir: dir.to_string(),
                reason: reason.unwrap_or_else(|| "failed".to_string()),
                at_ms: now_epoch_ms(),
            },
        );
    }
    let _ = app.emit(
        "automation:completed",
        AutomationCompleted {
            id: automation_id.clone(),
            run: run_id,
            term: report.term,
        },
    );
    let closes = stored_automations(state.config_root())
        .into_iter()
        .find(|automation| automation.id == automation_id)
        .is_some_and(|automation| automation.close_when_done);
    if closes && retire_terminal(&state, report.term) {
        announce_retired_terminal(app, report.term);
    }
}

/// Set `report.interrupted` from this event and the flag the pane was holding.
///
/// **Why the pane's memory is half the answer.** An interrupt is declared once,
/// at a turn boundary, and then CARRIED: claude's own later events for the same
/// turn do not repeat `is_interrupt`, so a reader that looked only at the
/// arriving payload would see the flag once and lose it before the row that
/// people actually read. Orca carries it in `claudeLeadStateByPaneKey`
/// (`agent-hook-listener.ts:2961-2966`); our equivalent memory is `pane_states`,
/// which is why this lives in the window rather than beside the payload readers.
///
/// **And why a non-boundary event drops it rather than keeping it.** Orca says
/// so in its own words — "any other event starts a fresh turn and drops it"
/// (`:2960`) — and [`zerocode_core::hook::interrupt_declared`] answers `false`
/// for exactly that case. So the flag is cleared by the same call that would
/// have set it, and there is no second road that has to remember to clear.
/// The payload rides in rather than on the report: a distilled report carries
/// the FACTS it read, and keeping the raw bytes on it so one more reader could
/// have them would keep every pane's last payload alive for the life of the
/// window.
pub(super) fn declare_pane_interrupt(
    app: &AppHandle,
    report: &mut hooks::PaneHookReport,
    payload: &str,
) {
    let carried = app
        .state::<AppState>()
        .pane_states()
        .get(&report.term)
        .is_some_and(|held| held.interrupted);
    report.interrupted =
        zerocode_core::hook::interrupt_declared(report.agent, &report.event, payload, carried);
}

/// Whether this report is the pane's own word, or a nested run's wearing the
/// pane's identity.
///
/// The facts are gathered one lock at a time — no two of these maps are ever
/// held together — and the judgement itself lives in
/// [`zerocode_core::pane_claim::speaks_for_the_pane`], which is where the
/// reasoning is written down and where it is tested without a window.
///
/// The authority for "is a nested run in flight" is [`AppState::workers`]: the
/// runs whose viewer pages are open, by pane and vendor. NOT the helper roster
/// — a nested claude's `SessionStart` empties that roster on its way in, so a
/// rule resting on it would wave the rest of the child's reports through.
pub(super) fn report_speaks_for_its_pane(app: &AppHandle, report: &hooks::PaneHookReport) -> bool {
    speaks_for_its_pane(
        app,
        report.term,
        report.agent.slug(),
        report.session.as_ref().map(|session| session.id.as_str()),
        report.state == zerocode_core::hook::HookState::Done,
    )
}

/// Whether what just arrived for a pane is the pane's own word.
///
/// Two roads ask, about the same envelope at two different depths: the tool
/// call that a hook carries (which is also how a nested run is discovered) and
/// the pane report that classifier makes of it. They must not be able to answer
/// differently, so the facts are gathered once here — one lock at a time, so no
/// two of these maps are ever held together — and the judgement itself lives in
/// [`zerocode_core::pane_claim::speaks_for_the_pane`], where it is written down
/// and tested without a window.
///
/// The authority for "is a nested run in flight" is [`AppState::workers`]: the
/// runs whose viewer pages are open, by pane and vendor. NOT the helper roster
/// — a nested claude's `SessionStart` empties that roster on its way in, so a
/// rule resting on it would wave the rest of the child's reports through.
pub(super) fn speaks_for_its_pane(
    app: &AppHandle,
    term: TermId,
    vendor: &'static str,
    said_session: Option<&str>,
    incoming_is_done: bool,
) -> bool {
    let state = app.state::<AppState>();
    let nested_run_live = state.workers().values().any(|seat| *seat == (term, vendor));
    let pane_vendor = state.agent_terms().get(&term).copied();
    let pane_turn_active = pane_turn_is_alive(&state, term);
    let known = state
        .pane_sessions()
        .get(&term)
        .map(|session| session.id.clone());
    zerocode_core::pane_claim::speaks_for_the_pane(&zerocode_core::pane_claim::PaneClaim {
        vendor,
        nested_run_live,
        pane_vendor,
        pane_turn_active,
        incoming_is_done,
        said_session,
        known_session: known.as_deref(),
    })
}

/// Whether this pane's own turn is still going.
///
/// Read off the state map, which only ACCEPTED words write — so a pane nothing
/// has ever spoken for has no turn, and its first speaker is its own whatever
/// an argv guess said. `Idle` is our own observation that the agent left, and
/// `Done` is the agent saying the turn ended; either way there is no turn to
/// interrupt.
///
/// Stale after [`zerocode_core::interrupt::STALE_AFTER_MS`], the same decay the
/// board applies for the same reason: a `working` nobody has updated for half an
/// hour is not a turn, it is a pane somebody walked away from — and holding a
/// live agent's words out on the strength of it would be the worse mistake.
pub(super) fn pane_turn_is_alive(state: &State<'_, AppState>, term: TermId) -> bool {
    let now = epoch_ms_now();
    state.pane_states().get(&term).is_some_and(|held| {
        !matches!(
            held.state,
            zerocode_core::hook::HookState::Done | zerocode_core::hook::HookState::Idle
        ) && held.at > 0
            && now.saturating_sub(held.at) <= zerocode_core::interrupt::STALE_AFTER_MS
    })
}

/// Whether a pane whose lead reported done still has work running under it:
/// a live helper row, or the payload's own inventory of background tasks and
/// session crons. The roster is the working-children proxy — rows leave on
/// `SubagentStop` — and the inventories are read off the turn-ending payload
/// itself, both exactly the gates Orca's `resolveClaudePaneState` holds
/// `done` behind.
///
/// **The two halves answer to an interrupt differently, and that asymmetry is
/// the contract** (`resolveClaudePaneState`, `agent-hook-listener.ts:2641-2655`):
///
/// ```text
/// claudeRosterHasWorkingSubagent(roster) ||
///   (!lead.interrupted && (runningNonAgentTask || activeSessionCron))
/// ```
///
/// A helper keeps the pane at `working` whether or not somebody pressed a key —
/// Ctrl+C reaches the lead's foreground, not a child that is running under it.
/// The inventories are the opposite: they are the LEAD's own account of what it
/// had going, and an interrupted lead's account is stale the moment it is
/// interrupted. Orca says the same thing twice for safety — it also DELETES the
/// registry entry when the flag is set (`updateClaudeRunningNonAgentTask`,
/// `:2628-2638`), which we have no equivalent of because we read the inventory
/// off each payload rather than keeping a registry. Without this gate an
/// interrupted `Stop` carrying a stale inventory parks as `working` forever:
/// the person pressed Ctrl+C and the card kept spinning.
///
/// **A divergence that stays, named.** Orca's roster half asks for a
/// non-idle child; ours asks whether the roster is non-empty, because
/// `hooks::SubagentRow` carries no state (the map's ▲ row). So where Orca lets
/// an interrupted done through past a roster of idle children, we still hold it
/// at `working`. That is the pre-existing breadth of this proxy rather than
/// anything this gate adds, and closing it means giving the row a state — a
/// different piece. Reusing this predicate as an INFERENCE guard would inherit
/// the same breadth and refuse too much, which is why the inference door will
/// ask its own question.
pub(super) fn pane_still_working(
    app: &AppHandle,
    term: TermId,
    payload: &str,
    interrupted: bool,
) -> bool {
    // 두 사실을 모으고, 판정은 코어의 한 함수가 내린다 — 여기 인라인으로
    // 적어 두면 그 규칙을 지키는 것이 글자 게이트뿐이 된다.
    let roster_busy = app
        .state::<AppState>()
        .subagents()
        .get(&term)
        .is_some_and(|rows| !rows.is_empty());
    let inventory_busy = zerocode_core::hook::payload_names_live_background_work(payload);
    zerocode_core::hook::done_is_held(roster_busy, inventory_busy, interrupted)
}

/// Record what a pane's agent last said about itself, and tell every window.
///
/// **The only writer of `pane_states`, and the only sender of `hook:agent`.**
/// It was inline in [`hook_loop`] while a hook event was the one thing that
/// could move a pane's state; it is a door now because there is a second
/// reporter — the pump, which watches for the agent that leaves without
/// reporting anything at all (`sweep_departed_agents`). Two writers would be
/// two merges, and the merge below is where a card's two lines are decided:
/// a second copy of it is a card whose prompt survives one road and not the
/// other.
///
/// The state is kept as well as forwarded because the window is not the only
/// audience: a board opened an hour from now was not listening, and it reads
/// this map through `pane_agents`.
///
/// Merged, not replaced. The card's two lines outlive the event that carried
/// them — the prompt until the next prompt, the answer until the next turn ends
/// (which may end saying nothing, and that is the clear).
pub(super) fn note_pane_state(
    app: &AppHandle,
    worktree: Option<&str>,
    report: &hooks::PaneHookReport,
) {
    if report.state == zerocode_core::hook::HookState::Working {
        restart_nudge_runtime::received_working(app, report.term);
    }
    // A pane that reports through hooks holds an agent this window recognises,
    // whoever started it. An agent the window launched carries the capability
    // paths from birth; one somebody typed into a shell inherits none, so the
    // window grants them to the pane now, by file
    // (`hooks::grant_pane_capabilities`).
    if !app
        .state::<AppState>()
        .launch_tokens()
        .contains_key(&report.term)
    {
        hooks::grant_pane_capabilities(report.term);
    }
    // A launch-time worker waits for evidence that its submitting Enter was
    // consumed, not merely written. Only providers whose catalog capability
    // reports prompt submission arm this one-shot channel.
    if report.prompt.is_some() {
        // A prompt going in is the line being emptied, whoever wrote it. The
        // door that types at a pane on somebody else's behalf reads this mark
        // to know whether it may.
        crate::human_input::submitted(report.term);
        // And the evidence a continuation the beat typed at a stopped worker
        // waits for (t-4537).
        orchestration::pane_prompt_submitted(report.term);
        if let Some(waiting) = app
            .state::<AppState>()
            .worker_prompt_submits()
            .remove(&report.term)
        {
            let _ = waiting.send(());
        }
    }
    // Answers the state's own clock, and the turn that just ENDED, for the
    // supervision road below — the facts it needs are merged here and nowhere
    // else, and recomputing them after the borrow closes would be the same
    // merge written twice.
    let (began_ms, ended) = {
        let state = app.state::<AppState>();
        let mut states = state.pane_states();
        let prior = states.get(&report.term);
        if prior.is_none_or(|held| held.state != report.state) {
            crumbs::record(
                "hook",
                format_args!(
                    "pane={} agent={} state={:?}",
                    report.term,
                    report.agent.slug(),
                    report.state
                ),
            );
        }
        let you = report
            .prompt
            .clone()
            .or_else(|| prior.and_then(|held| held.you.clone()));
        // Done replaces WHOLESALE — `None` there is the clear. A working
        // event's words (a tool result landing mid-turn) replace when
        // they exist and carry the prior line otherwise, which is Orca's
        // own merge (`update.lastAssistantMessage ?? previous`, :8842).
        let said = if report.state == zerocode_core::hook::HookState::Done {
            report.said.clone()
        } else {
            report
                .said
                .clone()
                .or_else(|| prior.and_then(|held| held.said.clone()))
        };
        // No carry at all for the ask: an event that is not the asking is
        // the agent having moved past the question (index.js:8840). The
        // full prompt and the approval ride the same rule — their keys
        // answer THIS question. Which is also what clears the question of an
        // agent that was killed while asking one: the pane goes `Idle`, and
        // `Idle` is not the asking.
        // 제출 판정의 낱말도 같은 규칙을 탄다 — 판이 지나간 질문을 끝낼 수
        // 있다고 답하면 그 키는 지금 화면에 있는 무엇인가로 들어간다.
        let (ask, ask_prompt, approval, submit_shape) =
            if report.state == zerocode_core::hook::HookState::NeedsAttention {
                (
                    report.ask.clone(),
                    report.ask_prompt.clone(),
                    report.approval.clone(),
                    report.submit_shape,
                )
            } else {
                (
                    None,
                    None,
                    None,
                    zerocode_core::ask::SubmitShape::NotAQuestion,
                )
            };
        let now = epoch_ms_now();
        // The state's own clock moves only when the STATE does — a repeated
        // `working` is the same stretch of work, however many tool events
        // arrive inside it, and on a `done` it is when the turn ENDED.
        let state_started_at = zerocode_core::hook::state_clock(
            prior.map(|held| (held.state, held.state_started_at)),
            report.state,
            now,
        );
        // 도우미의 기다림은 리드의 말을 **밀어낸다**. 그리고 리드는 제
        // 도우미가 답을 받았다는 이유로 다시 말하지 않으므로, 밀려나는 이
        // 순간을 놓치면 되돌릴 말이 남지 않는다. 규칙은 코어가 쥔다
        // (`hook.rs wait_stash`) — 판정을 여기 펴 쓰면 되돌리는 쪽과 챙기는
        // 쪽이 서로 다른 규칙을 갖게 된다.
        //
        // `prior`를 읽는 마지막 자리이기도 하다. 답은 Copy 두 칸이라 빌림이
        // 여기서 끝나고, 아래의 `states.insert`가 그 자리에 들어설 수 있다.
        let before_wait = zerocode_core::hook::wait_stash(
            report.state,
            report.child_attributed,
            prior.map(|held| zerocode_core::hook::BeforeWait {
                state: held.state,
                interrupted: held.interrupted,
            }),
            prior.and_then(|held| held.before_wait),
        );
        // Session status is metadata rather than a lifecycle edge. Keep its
        // last autonomous-work snapshot across the ordinary hook/turn reports
        // that replace the rest of this row.
        let autonomy =
            if report.state == zerocode_core::hook::HookState::Idle || report.session_boundary {
                None
            } else {
                prior.and_then(|held| held.autonomy.clone())
            };
        // 재시작을 살아남는 사본 (P0-13): 위 병합을 그대로 원장에 눕힌다 —
        // 병합 규칙을 두 번 쓰면 하나는 틀린다. 워크트리를 아는 소식은 자리를
        // 정하고 그 자리를 기억하며, 워크트리를 모르는 소식
        // (`clear_departed_agent`의 `foreground-returned`)은 새 자리를 지을 수
        // 없으니 **기억된 자리 하나**만 고친다.
        //
        // 꼬리 맞춤이었던 자리다. `term`은 열쇠의 NUL 뒤에 있으니 한 워크트리
        // 안에서는 정확했지만, 원장은 워크트리를 가로질러 산다:
        // `key.ends_with("\0{term}")`는 **모든** 워크트리의 같은 번호를 함께
        // 고쳤다. 번호는 재시작마다 `FLOAT_TERM + 1`에서 다시 발급되므로
        // (`next_term`), 새 판 7의 조용한 이탈이 지난 세션 판 7의 소식을
        // 워크트리마다 하나씩 Idle로 덮었다 — 모듈이 열쇠 설계로 피했다고
        // 적어 둔 바로 그 위험이다(`last_status.rs:16-20`). 자리를 모르면
        // 고칠 것도 없다: 가로질러 추측하는 것이 그 버그였다.
        //
        // 자물쇠는 한 번에 하나씩 쥔다 — `report_speaks_for_its_pane`이 같은
        // 규율을 같은 이유로 지킨다.
        // done의 출처 둘은 **한 함수를 지나 한 번에** 잠긴다. 완료가 아닌
        // done이라는 같은 말을 두 이름으로 하는 것이므로, done이 아닌 줄에서는
        // 둘 다 아무 뜻이 없다(`hook.rs done_provenance`).
        let session_boundary =
            zerocode_core::hook::done_provenance(report.state, report.session_boundary);
        let interrupted = zerocode_core::hook::done_provenance(report.state, report.interrupted);
        if let Some(worktree) = worktree {
            let seat = last_status::key(worktree, report.term);
            state.last_status_seats().insert(report.term, seat.clone());
            state.last_statuses().insert(
                seat,
                last_status::LastStatus {
                    worktree: worktree.to_string(),
                    agent: report.agent.slug().to_string(),
                    state: report.state,
                    at: now,
                    state_started_at,
                    you: you.clone(),
                    said: said.clone(),
                    session: report.session.clone(),
                    resumable: report.resumable,
                    received_at: now,
                    session_boundary,
                    interrupted,
                },
            );
        } else {
            // 자리를 먼저 셈하고 가드를 놓는다 — 문장을 나누는 것이 곧
            // "한 번에 하나"다(`if let` 사슬은 임시 가드를 문장 끝까지 쥔다).
            let seat = state.last_status_seats().get(&report.term).cloned();
            if let Some(seat) = seat
                && let Some(held) = state.last_statuses().get_mut(&seat)
            {
                held.state = report.state;
                held.at = now;
                held.state_started_at = state_started_at;
                held.received_at = now;
                // 출처도 함께 고친다. 이 길의 소식은 `Idle`이라 clamp가 둘 다
                // 떨어뜨리고, 그것이 맞다: 사람이 멈춘 턴이든 되살린 세션이든
                // **프로세스가 떠난 판**은 그 어느 것도 아니다. 남겨 두면 지난
                // 턴의 출처가 새 사실에 붙어 서 있게 된다.
                held.session_boundary = session_boundary;
                held.interrupted = interrupted;
            }
        }
        schedule_last_status_persist(app);
        states.insert(
            report.term,
            PaneState {
                state: report.state,
                at: now,
                state_started_at,
                you,
                said,
                ask,
                ask_prompt,
                approval,
                submit_shape,
                session_boundary,
                interrupted,
                before_wait,
                autonomy,
            },
        );
        // The power system hears every state through this same door: a
        // WORKING pane refreshes its hold, anything else releases it
        // (Orca's `setStatuses` runs off the same status stream, P0-17).
        let awake_before = state.awake().status();
        state.awake().note_pane(
            report.term,
            report.state == zerocode_core::hook::HookState::Working,
        );
        let awake_after = state.awake().status();
        if awake_after != awake_before {
            let (mode, active) = awake_after;
            let _ = app.emit_to(
                MAIN_WINDOW_LABEL,
                "computer-awake:changed",
                AwakeStatus { mode, active },
            );
        }
        // The window's half of `agentWait`: every hook report IS an
        // evaluation, and NeedsAttention is the one state that means a
        // person — stamped with when that state began, which outlives the
        // dozens of tool events one waiting stretch can produce.
        orchestration::pane_attention_noted(
            report.term,
            (report.state == zerocode_core::hook::HookState::NeedsAttention)
                .then_some(state_started_at),
            now,
        );
        (
            state_started_at,
            (report.state == zerocode_core::hook::HookState::Done).then_some((
                state_started_at,
                interrupted,
                now,
            )),
        )
    };
    // A supervised worker that ends a turn saying nothing leaves its task
    // dispatched forever, because the only automatic completion is a
    // `worker_done` the worker RUNS. The ledger decides whether that silence is
    // news — an interrupted turn is not, and neither is a worker waiting on an
    // answer it asked for.
    match ended {
        Some((turn_ended_ms, interrupted, now)) => {
            orchestration::pane_turn_ended(report.term, turn_ended_ms, interrupted, now);
        }
        // Anything but a finished turn means the pane is NOT at rest: an
        // agent working, or one stopped at a question of its own — and the
        // mail pointer must not type into either. `NeedsAttention` above all:
        // that composer is holding a question for the PERSON. Heard at the
        // moment that state began, as a finished turn is heard at its end.
        None => orchestration::pane_turn_began(report.term, began_ms),
    }
    let _ = app.emit("hook:agent", report.clone());
    if report.state == zerocode_core::hook::HookState::Done
        && let Some(cleanup) = app
            .state::<AppState>()
            .completed_worker_cleanups()
            .remove(&report.term)
    {
        schedule_completed_worker_cleanup(app, report.term, cleanup);
    }
}

/// 마지막 소식 뒤 한 숨(`last_status::PERSIST_DEBOUNCE`) 자고 적는다 —
/// `watch_popout_bounds`와 같은 세대 문법: 새 소식이 세대를 올리면 앞선
/// 잠은 깨어나 자기가 낡았음을 보고 그냥 돌아간다. 한 턴이 쏟는 수십 개의
/// 도구 이벤트가 fsync 수십 번이 되지 않는 이유의 전부다(Orca
/// `scheduleStatusPersist`, server.ts:3170-3186).
pub(super) fn schedule_last_status_persist(app: &AppHandle) {
    use std::sync::atomic::Ordering;
    let mine = app
        .state::<AppState>()
        .last_status_writes()
        .fetch_add(1, Ordering::Relaxed)
        + 1;
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(last_status::PERSIST_DEBOUNCE);
        if app
            .state::<AppState>()
            .last_status_writes()
            .load(Ordering::Relaxed)
            != mine
        {
            return;
        }
        write_last_statuses_now(&app);
    });
}

/// 지금 당장, 디바운스 없이 — 종료 길이 쓰는 문이다(Orca
/// `flushStatusPersistSync`): ExitRequested 뒤에는 잠에서 깰 스레드가 없다.
pub(super) fn write_last_statuses_now(app: &AppHandle) {
    let state = app.state::<AppState>();
    let path = state
        .local_data_root()
        .join(app_paths::artifact_file::LAST_STATUS);
    let entries = state.last_statuses().clone();
    let mut written = state.last_status_written();
    match last_status::save(&path, &entries, written.as_deref()) {
        Ok(Some(bytes)) => *written = Some(bytes),
        Ok(None) => {}
        // 저장이 죽어도 벨과 보드는 살아야 한다 — husk 한 줄이 다음
        // 조사를 위한 전부다.
        Err(error) => {
            note_window_event(state.local_data_root(), &format!("last-status: {error}"));
        }
    }
}

/// 지난 세션의 판들 — 부팅한 창이 살아있는 판 곁에 흐리게 그릴 목록.
/// 열쇠(worktree·죽은 term)는 창에 무의미하므로 값만, 최신이 먼저 —
/// 보드의 정렬 열쇠와 같은 `at` 내림차순.
#[tauri::command]
pub(super) fn last_statuses(state: State<'_, AppState>) -> Vec<last_status::LastStatus> {
    let _crumb = crate::crumbs::Command::enter("last_statuses");
    let mut listed: Vec<last_status::LastStatus> =
        state.last_statuses().values().cloned().collect();
    listed.sort_by_key(|one| std::cmp::Reverse(one.at));
    listed
}

/// Ring the OS about a pane's stop, under the rules that keep ringing rare.
///
/// The decisions are `zerocode_core::notify`'s (Orca's own: cooldown 5s per
/// worktree, suppression at the watched screen, a 1.5s quiet before
/// "finished"); what lives here is the clock, the focus fact, and the OS call.
pub(super) fn ring_for_pane(app: &AppHandle, worktree: &str, report: &hooks::PaneHookReport) {
    use zerocode_core::notify::{self, Ring};
    // A session boundary is nobody's completion: the `done` it wears exists
    // so a resumed pane does not spin, and ringing "finished" for opening an
    // old conversation is exactly what Orca's `sessionBoundary` flag keeps
    // out of every completion-reactive consumer.
    if report.session_boundary {
        return;
    }
    let Some(ring) = notify::ring_of(report.state) else {
        return;
    };
    // 마스터·종류별 게이트는 여기 없다: `ring_now`의 한 사다리가 전부
    // 검사한다 — armed 완료 벨도 fire 시점에 같은 문을 지나므로, 조용한
    // 시간 동안 바뀐 설정이 그대로 존중된다.
    match ring {
        // `ring_of` never yields a push — that road is `ring_zo_push` — but a
        // push that reached here would ring at once, like attention.
        Ring::Attention | Ring::Push => {
            let (ask, said) = pane_words(app, report.term);
            ring_now(
                app,
                RingNotice {
                    worktree,
                    term: Some(report.term),
                    agent: report.agent.slug(),
                    ring,
                    // An attention ring never reads it — Orca's verb table asks
                    // only the `done` arm (`notification-options.ts:60-65`) — and
                    // passing the report's own answer rather than a literal keeps
                    // that a fact about the table instead of a fact about this
                    // call site.
                    interrupted: report.interrupted,
                    ask,
                    said,
                },
            )
        }
        // Armed, not fired: agents end a turn and immediately start the next,
        // and ringing on every Done would ring mid-conversation. The quiet
        // passes, and the ring fires only if the pane still says Done —
        // checked by the STAMP, because a newer Done is a different turn.
        Ring::Completion => {
            let app = app.clone();
            let worktree = worktree.to_string();
            let agent = report.agent.slug();
            let term = report.term;
            let armed_at = app
                .state::<AppState>()
                .pane_states()
                .get(&term)
                .map(|held| held.at);
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(
                    notify::DONE_QUIET_MS as u64,
                ))
                .await;
                let still = {
                    let state = app.state::<AppState>();
                    let states = state.pane_states();
                    states
                        .get(&term)
                        .map(|held| (held.state, held.at, held.interrupted))
                };
                // 깃발은 **발사 시점의 줄에서** 읽는다. 무장 시점의 보고서가
                // 아니다: 조용한 1.5초 안에 같은 턴에 대한 이벤트가 더 올 수
                // 있고, Orca도 알림을 보낼 때 스토어의 행에서 읽는다
                // (`use-notification-dispatch.ts:202`). 무장 때 베껴 두면
                // 그 1.5초 동안 사람이 누른 키가 낱말에 반영되지 않는다.
                if let Some((state, at, interrupted)) = still
                    && state == zerocode_core::hook::HookState::Done
                    && armed_at == Some(at)
                {
                    let (ask, said) = pane_words(&app, term);
                    ring_now(
                        &app,
                        RingNotice {
                            worktree: &worktree,
                            term: Some(term),
                            agent,
                            ring: Ring::Completion,
                            interrupted,
                            ask,
                            said,
                        },
                    );
                }
            });
        }
    }
}

/// Notice the agents that left without saying so, and clear the panes they
/// left behind.
///
/// **The road for every agent whose end is not an event.** Two of them are
/// ordinary: a vendor with no working hook channel on this machine, and an
/// agent somebody QUIT rather than let finish (`codex`, then `/exit`, and they
/// are back at their prompt). Neither closes the terminal — the shell is still
/// there — so `term:exited` never fires either, and the pane's last `working`
/// stood for the life of the window. That is the reported bug: a sidebar saying
/// "Codex 작업 중" over a checkout where nothing was running.
///
/// What replaces the missing event is [`PtyTransport::foreground_is_child`] — which
/// process group the kernel is currently pointing the terminal at. The
/// judgement about what a RUN of those answers means is
/// `zerocode_core::agent_exit`'s, and it is there because it is the part with a
/// decision in it; what lives here is the two locks and the order they are
/// taken in.
///
/// Cheap on purpose, in three ways that compound:
///
/// - **Only the panes that claim something.** `agent_terms` is the panes this
///   window believes hold an agent at all (usually none to three), and
///   `claims_running` narrows that to the ones whose state would draw as busy.
///   A pane already at `Idle` is never asked again, which is what stops this
///   from re-clearing the same pane twice a second forever.
/// - **Only twice a second.** The pump turns at the display rate; this rides it
///   on a gate, because which process holds a terminal changes once a session.
/// - **One question to the kernel.** `tcgetpgrp` on a descriptor we already
///   own, next to a pid we already have.
///
/// (Orca measures the same fact and pays enormously more for it: a `ps -axo
/// pid=,ppid=,stat=,command=` over every process on the machine, whose
/// descendants are then walked looking for the `+` that marks the foreground
/// group — `resolveAgentForegroundProcessFromPs`,
/// daemon-ready-identity-CjFvutLo.js:3768. A process scan per pane is expensive
/// enough that it needs a 500ms snapshot cache, a four-at-a-time concurrency
/// cap, an eight-per-second rate limit and exponential backoff on failure
/// (remote-runtime-pty-recovery-state-mqdD2xBC.js:611-612, 1045-1050) — four
/// pieces of machinery that exist only because the measurement is expensive.
/// It also WRITES INTO the user's `.bashrc` and `.zshrc` to inject OSC 133
/// markers, for the reason its own comment gives: without them "bash users …
/// keep a stuck 'working' spinner for up to 30 min after the CLI exits without
/// sending a Stop/SessionEnd hook" (:998). We ask the terminal directly and
/// touch nobody's shell configuration.)
pub(super) fn detach_zo_channel_if_owned(
    state: &AppState,
    session: &str,
    owner: ZoChannelOwner,
) -> bool {
    let owned = {
        let mut owners = state.channel_owners();
        if owners.get(session) != Some(&owner) {
            false
        } else {
            owners.remove(session);
            true
        }
    };
    if owned {
        subscribed(state.subscriptions()).remove(session);
        state.channels().remove(session);
    }
    owned
}

pub(super) fn release_zo_subscription(state: &AppState, term: TermId, adoption: &ZoPaneAdoption) {
    let Some(session) = adoption
        .session
        .as_deref()
        .filter(|_| adoption.owns_subscription)
    else {
        return;
    };
    detach_zo_channel_if_owned(state, session, ZoChannelOwner::Term(term));
}

pub(super) fn detach_adopted_zo(app: &AppHandle, term: TermId) {
    let adoption = app.state::<AppState>().zo_adoptions().remove(&term);
    let Some(adoption) = adoption else { return };
    release_zo_subscription(&app.state::<AppState>(), term, &adoption);
    clear_pane_subagents(app, term);
    if adoption.owns_subscription
        && let Some(session) = adoption.session
    {
        let _ = app.emit(
            "session:ended",
            json!({ "session": session, "reason": null }),
        );
    }
}

pub(super) fn abandon_pending_zo_adoption(app: &AppHandle, term: TermId, pid: u32, file: &Path) {
    let state = app.state::<AppState>();
    let mut adoptions = state.zo_adoptions();
    if adoptions
        .get(&term)
        .is_some_and(|held| held.pid == pid && held.file == file && held.session.is_none())
    {
        adoptions.remove(&term);
    }
}

pub(super) fn start_zo_adoption(app: &AppHandle, term: TermId, pid: u32) {
    if !zo_integration_runtime::retry_due(&app.state::<AppState>(), term, pid) {
        return;
    }
    let runtime = std::env::temp_dir();
    use zo_integration_runtime::Reason;
    let channel = match zerocode_lane::discover_pane_channel(&runtime, pid) {
        Ok(Some(channel)) => channel,
        Ok(None) => {
            zo_integration_runtime::discovery(app, term, pid, Reason::DiscoveryMissing);
            return;
        }
        Err(_) => {
            zo_integration_runtime::discovery(app, term, pid, Reason::DiscoveryInvalid);
            return;
        }
    };
    zo_integration_runtime::discovery(app, term, pid, Reason::Discovering);
    let file = zerocode_lane::pane_channel_file(&runtime, pid);
    {
        let state = app.state::<AppState>();
        let mut adoptions = state.zo_adoptions();
        if adoptions.contains_key(&term) {
            return;
        }
        adoptions.insert(
            term,
            ZoPaneAdoption {
                pid,
                file: file.clone(),
                session: None,
                owns_subscription: false,
            },
        );
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let Some(token) = channel.token.clone() else {
            zo_integration_runtime::adoption_reason(&app, term, pid, Reason::AuthRejected);
            abandon_pending_zo_adoption(&app, term, pid, &file);
            return;
        };
        let Ok(address) = channel.addr.parse() else {
            zo_integration_runtime::adoption_reason(&app, term, pid, Reason::DiscoveryInvalid);
            abandon_pending_zo_adoption(&app, term, pid, &file);
            return;
        };
        let probe = zerocode_lane::probe_session_server(address, &token);
        if probe != zerocode_lane::ServeProbe::Ready {
            let reason = match probe {
                zerocode_lane::ServeProbe::Unauthorized => Reason::AuthRejected,
                zerocode_lane::ServeProbe::NotListening => Reason::ConnectRefused,
                _ => Reason::InfoInvalid,
            };
            zo_integration_runtime::adoption_reason(&app, term, pid, reason);
            abandon_pending_zo_adoption(&app, term, pid, &file);
            return;
        }
        let Ok(session) = pane_channel_session_id(channel.addr.clone(), Some(token.clone())) else {
            zo_integration_runtime::adoption_reason(&app, term, pid, Reason::InfoInvalid);
            abandon_pending_zo_adoption(&app, term, pid, &file);
            return;
        };
        if channel.session.as_ref().is_some_and(|id| *id != session) {
            zo_integration_runtime::adoption_reason(&app, term, pid, Reason::SessionMismatch);
            abandon_pending_zo_adoption(&app, term, pid, &file);
            return;
        }
        let still_current = app
            .state::<AppState>()
            .zo_adoptions()
            .get(&term)
            .is_some_and(|held| held.pid == pid && held.file == file && file.is_file());
        if !still_current {
            zo_integration_runtime::adoption_reason(&app, term, pid, Reason::DiscoveryStale);
            return;
        }
        let provider = zerocode_core::ProviderSession {
            key: zerocode_core::SessionKey::SessionId,
            id: session.clone(),
            transcript_path: None,
        };
        let provider = {
            let state = app.state::<AppState>();
            let mut held = state.pane_sessions();
            // Adoption learns the id from the channel, never a path. If this
            // pane already reported where it writes, keep it.
            let carried = provider.carrying_forward(held.get(&term));
            held.insert(term, carried.clone());
            carried
        };
        orchestration::pane_session_reported(term, &provider, now_epoch_ms());
        let owns_subscription = crate::cmd::project::attach_zo_pane_channel(
            &app,
            app.state::<AppState>().inner(),
            ZoChannelOwner::Term(term),
            channel.addr.clone(),
            Some(token),
            session.clone(),
            None,
        );
        let recorded = {
            let state = app.state::<AppState>();
            let mut adoptions = state.zo_adoptions();
            if let Some(held) = adoptions.get_mut(&term)
                && held.pid == pid
                && held.file == file
            {
                held.session = Some(session.clone());
                held.owns_subscription = owns_subscription;
                true
            } else {
                false
            }
        };
        if !recorded && owns_subscription {
            detach_zo_channel_if_owned(
                &app.state::<AppState>(),
                &session,
                ZoChannelOwner::Term(term),
            );
            return;
        }
        let _ = app.emit("agents:changed", ());
    });
}

pub(super) fn reconcile_zo_adoptions(app: &AppHandle, processes: &HashMap<TermId, u32>) {
    let live_subscriptions = subscribed(app.state::<AppState>().subscriptions()).clone();
    let stale: Vec<TermId> = app
        .state::<AppState>()
        .zo_adoptions()
        .iter()
        .filter_map(|(term, adoption)| {
            let process_moved = processes.get(term) != Some(&adoption.pid);
            let file_gone = !adoption.file.is_file();
            let stream_gone = adoption
                .session
                .as_ref()
                .is_some_and(|session| !live_subscriptions.contains(session));
            (process_moved || file_gone || stream_gone).then_some(*term)
        })
        .collect();
    for term in stale {
        detach_adopted_zo(app, term);
    }
    let attached: HashSet<TermId> = app
        .state::<AppState>()
        .zo_adoptions()
        .keys()
        .copied()
        .collect();
    for (&term, &pid) in processes {
        if !attached.contains(&term) {
            start_zo_adoption(app, term, pid);
        }
    }
}

/// Agents nobody launched and no hook announced — found by asking the kernel
/// who is holding each plain pane.
///
/// The twin of [`sweep_departed_agents`]: that one notices an agent LEAVING a
/// pane this window already believed in, and this one notices one ARRIVING in
/// a pane it did not. They share the gate, the twice-a-second cadence and the
/// one-lock-at-a-time discipline.
///
/// It exists because the roster the sidebar and the board read (`pane_agents`)
/// is `agent_terms`, which only a launch or a hook used to write. So a `zo` —
/// or a `claude`, or a `codex` — somebody typed into a plain shell was
/// invisible on both surfaces, while the send menu, which asked the kernel for
/// itself, knew about it perfectly well. That split answer is the report
/// ("zo는 아예 감지도 못함"), and the repair is not a second asker but ONE
/// owner: this sweep writes the map and every surface reads the map.
///
/// Cheap in the same ways its twin is, plus one of its own: a pane whose
/// foreground group IS the shell this window spawned cannot be running an
/// agent, and that question is a single ioctl — so the two heavier lookups
/// behind `foreground_programs` (the executable path and the argv) are paid
/// only for a pane where something else has actually taken the terminal.
/// The pid whose `zo-events-<pid>.addr` file names a pane's Zo channel, when
/// the pane holds a Zo that still needs adopting.
///
/// Two Zos qualify. One was detected in front of a shell — `named` says so,
/// and the sweep's own naming already paid for the lookup. The other is the
/// window's own child: this window launched it, so `child_in_front` is
/// `Some(true)`, the sweep names nothing (its arrival is the launch's, not
/// the sweep's), and `launched_zo` — the pane is among those the window
/// launched as Zo — is what says the pty's foreground process is the Zo
/// whose file to read. Adopted panes keep reporting it: the reconcile reads
/// a pid that stops arriving as a process that moved. Before this clause a
/// launched worker was never adopted at all: its briefing sat parked
/// waiting for a subscriber that could only be found through this pid
/// (live report 2026-09-02: four summons, four "did not accept its
/// briefing: Err(Timeout)", the composer empty every time).
///
/// A window-launched Zo that has dropped to a shell (`child_in_front` false,
/// nothing Zo-named in front) is not a Zo to adopt, and neither is another
/// agent's pane.
pub(super) fn zo_channel_pid(
    named: Option<&'static str>,
    child_in_front: Option<bool>,
    launched_zo: bool,
    pid: impl FnOnce() -> Option<u32>,
) -> Option<u32> {
    let detected = named == Some(AgentKind::Zo.slug());
    // Only while nothing else was named in front: a pane the sweep found
    // another agent holding is that agent's, whatever this window launched.
    let own_child = child_in_front == Some(true) && launched_zo && named.is_none();
    (detected || own_child).then(pid).flatten()
}

pub(super) fn sweep_arrived_agents(app: &AppHandle) {
    // Panes somebody else already speaks for. A launch and a hook each have
    // their own arrival AND departure rules; this sweep never touches them.
    let spoken_for: std::collections::HashSet<TermId> = app
        .state::<AppState>()
        .agent_terms()
        .keys()
        .copied()
        .collect();
    let mine_before: std::collections::HashSet<TermId> = app
        .state::<AppState>()
        .foreground_agents()
        .keys()
        .copied()
        .collect();
    let adopted_before: std::collections::HashSet<TermId> = app
        .state::<AppState>()
        .zo_adoptions()
        .keys()
        .copied()
        .collect();
    // Every pane this window launched as Zo, channel or no channel. The sweep
    // keeps handing over such a pane's child pid for as long as the Zo is in
    // front: `reconcile_zo_adoptions` reads a pid that stopped arriving as
    // "the process moved" and tears the subscription down — which, when only
    // channel-less panes reported a pid, happened on the very next beat after
    // adoption, and took the worker's prompt acknowledgement with it.
    let launched_zo: std::collections::HashSet<TermId> = app
        .state::<AppState>()
        .agent_terms()
        .iter()
        .filter_map(|(term, agent)| (*agent == AgentKind::Zo.slug()).then_some(*term))
        .collect();
    let zo_needing_channel: std::collections::HashSet<TermId> = {
        let state = app.state::<AppState>();
        let sessions = state.pane_sessions().clone();
        let live = subscribed(state.subscriptions()).clone();
        state
            .agent_terms()
            .iter()
            .filter_map(|(term, agent)| {
                (*agent == AgentKind::Zo.slug()
                    && sessions
                        .get(term)
                        .is_none_or(|session| !live.contains(&session.id)))
                .then_some(*term)
            })
            .collect()
    };
    // The kernel's answer, taken under each pane's own terminal lock with
    // nothing else done under it: that lock is the one that pane's
    // keystrokes need, and no other pane waits at all.
    //
    // Two questions, and the cheap one gates the dear one — a pane whose
    // foreground group IS the shell this window spawned cannot be running an
    // agent, so the executable and argv lookups behind `foreground_programs`
    // are paid only where something else has actually taken the terminal.
    let looks: Vec<ForegroundAgentLook> = {
        let state = app.state::<AppState>();
        let entries = state.terminals().entries();
        entries
            .iter()
            .filter(|(term, _)| {
                mine_before.contains(term)
                    || adopted_before.contains(term)
                    || zo_needing_channel.contains(term)
                    || !spoken_for.contains(term)
            })
            .map(|(term, held)| {
                let pty = lock_pty(held);
                let child_in_front = pty.foreground_is_child();
                let named = if child_in_front == Some(true) {
                    None
                } else {
                    pty.foreground_programs()
                        .into_iter()
                        .find_map(|program| zerocode_core::agent::agent_spec_by_process(&program))
                        .map(|spec| spec.id)
                };
                let process =
                    zo_channel_pid(named, child_in_front, launched_zo.contains(term), || {
                        pty.foreground_process_id()
                    });
                ForegroundAgentLook {
                    term: *term,
                    child_in_front,
                    agent: named,
                    process,
                }
            })
            .collect()
    };
    let looked: std::collections::HashSet<TermId> = looks.iter().map(|look| look.term).collect();
    // `process` is only ever taken for a Zo — detected in front of a shell,
    // or launched by this window and still waiting for its channel — so it
    // needs no second look at `agent`, which is deliberately `None` for the
    // window's own child (see `zo_channel_pid`).
    let zo_processes: HashMap<TermId, u32> = looks
        .iter()
        .filter_map(|look| Some((look.term, look.process?)))
        .collect();
    let mut arrived: Vec<(TermId, &'static str)> = Vec::new();
    let mut departed: Vec<TermId> = Vec::new();
    let mut closed: Vec<TermId> = Vec::new();
    {
        let state = app.state::<AppState>();
        let mut mine = state.foreground_agents();
        for look in looks {
            let term = look.term;
            if let Some(agent) = look.agent {
                if !mine.contains_key(&term) {
                    arrived.push((term, agent));
                }
                // Something other than the shell holds it, which is what arms
                // the watch and ends any run of quiet behind it.
                let watch = mine.get(&term).copied().unwrap_or_default();
                mine.insert(term, watch.look(false));
                continue;
            }
            // Nothing here names an agent. WHETHER that is a departure is
            // `zerocode_core::agent_exit`'s judgement and not this function's:
            // one look is one instant, and a shell legitimately holds its own
            // terminal for an instant between two commands. A platform that
            // will not answer leaves the watch untouched — "no opinion" is not
            // evidence.
            let Some(watch) = mine.get(&term).copied() else {
                continue;
            };
            let Some(child_in_front) = look.child_in_front else {
                continue;
            };
            let moved = watch.look(child_in_front);
            if moved.agent_left() {
                departed.push(term);
            } else {
                mine.insert(term, moved);
            }
        }
        for term in &departed {
            mine.remove(term);
        }
        // A pane that has closed is nobody's claim any more. Shell ids are
        // never reissued, so an entry for one that is gone is a leak rather
        // than a memory.
        //
        // A pane that closes WITH an agent in it is a session that ended, so
        // it is reported as one — the agent worked until the pane went away,
        // and dropping that time would make the figure quietly low for every
        // person who closes tabs instead of quitting agents.
        for term in mine.keys().filter(|term| !looked.contains(term)) {
            closed.push(*term);
        }
        mine.retain(|term, _| looked.contains(term));
    }
    reconcile_zo_adoptions(app, &zo_processes);
    if arrived.is_empty() && departed.is_empty() && closed.is_empty() {
        return;
    }
    {
        let state = app.state::<AppState>();
        let mut terms = state.agent_terms();
        for (term, agent) in &arrived {
            terms.entry(*term).or_insert(agent);
        }
        for term in &departed {
            terms.remove(term);
        }
    }
    // The stats head counts agent sessions HERE, where they are seen to begin
    // and end, rather than at the launch command. Two reasons: the terminal is
    // a terminal, so an agent a person typed themselves is as real a session
    // as one this window started; and counting the spawn in one place and the
    // time in another would let the two figures describe different sets of
    // sessions, which is the mistake nobody could see.
    {
        let now = epoch_ms_now();
        for (term, _) in &arrived {
            stats_events_store::agent_began(*term, now);
        }
        for term in departed.iter().chain(closed.iter()) {
            stats_events_store::agent_ended(*term, now);
        }
    }
    // The surfaces hear roster changes as hook events, and this is a roster
    // change no hook will ever announce — so it is said out loud, once, and
    // only when something actually moved.
    let _ = app.emit("agents:changed", ());
}

pub(super) fn sweep_departed_agents(app: &AppHandle) {
    // Which panes are worth asking about, decided before any pty is touched.
    //
    // Three maps are read to answer it and NO TWO OF THEM ARE EVER HELD AT
    // ONCE — the same discipline `pane_agents` keeps a few hundred lines below,
    // and for the same reason: every guard taken while holding another is a
    // pair of locks some future road can want in the other order, and the
    // terminal pool is the lock every keystroke in the window needs.
    let claiming: Vec<TermId> = app
        .state::<AppState>()
        .agent_terms()
        .keys()
        .copied()
        .collect();
    let asking: Vec<TermId> = {
        let state = app.state::<AppState>();
        let states = state.pane_states();
        claiming
            .into_iter()
            .filter(|term| zerocode_core::claims_running(states.get(term).map(|held| held.state)))
            .collect()
    };
    if asking.is_empty() {
        // Nothing claims an agent, so nothing can be stale — and the watches
        // held for panes that have since gone quiet are memory about a question
        // nobody is asking.
        app.state::<AppState>().foreground_watch().clear();
        return;
    }
    // The kernel's answer for each, taken under that pane's own terminal
    // lock and nothing else done under it: the lock its keystrokes need,
    // and nobody else's.
    let looks: Vec<(TermId, Option<bool>)> = {
        let state = app.state::<AppState>();
        let terminals = state.terminals();
        asking
            .into_iter()
            .map(|term| {
                (
                    term,
                    terminals
                        .handle(term)
                        .and_then(|held| lock_pty(&held).foreground_is_child()),
                )
            })
            .collect()
    };
    let left: Vec<TermId> = {
        let state = app.state::<AppState>();
        let mut watch = state.foreground_watch();
        // The watches of panes nobody asked about this round. A pane that
        // stopped claiming an agent — it reported `Done`, or the terminal went
        // — must not keep a half-finished run that a later `working` would
        // inherit and finish early.
        let live: HashSet<TermId> = looks.iter().map(|(term, _)| *term).collect();
        watch.retain(|term, _| live.contains(term));
        let mut left = Vec::new();
        for (term, seen) in looks {
            // A question the operating system would not answer is not evidence.
            // Left as it was rather than folded either way: counting silence as
            // a departure would empty every pane on a platform that has no
            // foreground groups at all.
            let Some(child_in_front) = seen else { continue };
            let folded = watch.entry(term).or_default().look(child_in_front);
            if folded.agent_left() {
                // The entry goes with the verdict, so the next agent to run in
                // this same shell starts from an unarmed watch rather than
                // inheriting a run that already reached its answer.
                watch.remove(&term);
                left.push(term);
            } else {
                watch.insert(term, folded);
            }
        }
        left
    };
    for term in left {
        clear_departed_agent(app, term);
    }
}

/// The event name an inferred answer lands under.
///
/// Named for what was OBSERVED rather than borrowed from a vendor's
/// vocabulary, exactly like `clear_departed_agent`'s `foreground-returned`:
/// no agent sent this. Answering an `AskUserQuestion` emits no hook at all —
/// the agent simply resumes — so a keystroke reaching the pty is the only
/// signal there will ever be.
pub(super) const ANSWERED_EVENT: &str = "question-answered";

/// Which road brought the answer.
pub(super) enum AnswerRoad<'a> {
    /// A person typed into the pty. The keystroke has to be one that FINISHES
    /// the whole prompt, which is [`zerocode_core::ask::answers_whole_prompt`]'s
    /// question.
    Keystroke(&'a str),
    /// The ask card walked the TUI to the end of its own answer. It knows what
    /// it answered, so the shape test is already spent — Orca gives that road
    /// a separate door for the same reason
    /// (`inferQuestionAnsweredFromCurrentStatus`,
    /// `agent-question-answered-inference.ts:104-110`).
    Card,
}

/// The worktree this process seated a pane's news under, when it seated one.
///
/// The ledger's seat is the only `term`→worktree map this process keeps: the
/// id arrives on an envelope and nothing else files it. A pane that never
/// reported has no seat, and a ring with no worktree has nowhere to take the
/// person, so the absence is the honest answer rather than a guess.
///
/// One lock at a time, for the reason `note_pane_state` writes down beside its
/// own seat lookup.
pub(super) fn seated_worktree(state: &AppState, term: TermId) -> Option<String> {
    let seat = state.last_status_seats().get(&term).cloned()?;
    let held = state.last_statuses();
    held.get(&seat).map(|row| row.worktree.clone())
}

/// A question the pane was blocked on has just been answered where no hook
/// will ever say so. Put the pane back where the wait found it.
///
/// The gates are Orca's, which states them twice against two records — once in
/// the renderer (`agent-question-answered-inference.ts:31-38`) and again in
/// the main process (`server.ts:1006-1029`) so a racing real hook always wins.
/// Ours is one row under one lock, so the second statement would compare a
/// fact with itself; what survives the collapse is the LIST:
///
/// - claude alone. The other vendors' waits are not this question.
/// - the pane is actually stopped.
/// - the stop is a QUESTION and this keystroke finishes the whole of it —
///   both folded into [`zerocode_core::ask::SubmitShape`], so a permission
///   wait stays sticky and a digit mid-walk answers nothing.
/// - the news is fresh. Orca's window is `AGENT_STATUS_STALE_AFTER_MS` and
///   ours is the same number under its own name
///   ([`zerocode_core::board::STALE_AFTER_MS`]): a half-hour-old `waiting` is
///   a pane whose agent probably died, and clearing it on a stray Enter would
///   report progress nobody is making.
///
/// **One divergence, and it is the map's.** Orca also refuses a row whose
/// state was restored from a snapshot and never confirmed this runtime
/// (`restoredUnconfirmed`, `server.ts:1009`). We carry no such mark — the
/// map's ▲ 「복원 provenance」 — so a hydrated `waiting` can be cleared here by
/// a keystroke. That is the pre-existing breadth of having no provenance
/// rather than anything this door adds, and it closes when that row does.
pub(super) fn clear_answered_wait(app: &AppHandle, term: TermId, road: &AnswerRoad<'_>) {
    let state = app.state::<AppState>();
    let agent = state
        .agent_terms()
        .get(&term)
        .copied()
        .and_then(zerocode_core::AgentKind::from_slug);
    if agent != Some(zerocode_core::AgentKind::Claude) {
        return;
    }
    let now = epoch_ms_now();
    // Everything the row has to say, gathered under one lock and released
    // before anything is written — `note_pane_state` takes it again.
    let restored = {
        let states = state.pane_states();
        let Some(held) = states.get(&term) else {
            return;
        };
        if held.state != zerocode_core::hook::HookState::NeedsAttention {
            return;
        }
        if let AnswerRoad::Keystroke(data) = road
            && !zerocode_core::ask::answers_whole_prompt(data, held.submit_shape)
        {
            return;
        }
        if now.saturating_sub(held.at) > zerocode_core::board::STALE_AFTER_MS {
            return;
        }
        zerocode_core::hook::answered_restore(held.before_wait)
    };
    // Down the doors a real report would have used, for the reason
    // `clear_departed_agent` gives above: a second road is a second chance for
    // one surface to be missed, and it is always the one nobody watches.
    //
    // `None` for the worktree — there is no envelope, so the ledger's REMEMBERED
    // seat is corrected and no new one is invented.
    let report = hooks::PaneHookReport {
        permission_mode: None,
        term,
        agent: zerocode_core::AgentKind::Claude,
        state: restored.state,
        // A restored `done` keeps whose doing it was; `done_provenance` drops
        // it on anything else, which is why it can be passed unconditionally.
        interrupted: restored.interrupted,
        session_boundary: false,
        // Nobody's helper: this is the WINDOW speaking. And the stash the flag
        // feeds is emptied by that answer, which is the point — the pane has
        // its own word back.
        child_attributed: false,
        // The question is over, so nothing here can be finished by a key.
        submit_shape: zerocode_core::ask::SubmitShape::NotAQuestion,
        event: ANSWERED_EVENT.to_string(),
        session: None,
        resumable: false,
        prompt: None,
        said: None,
        ask: None,
        ask_prompt: None,
        approval: None,
        model: None,
    };
    note_pane_state(app, None, &report);
    // And the bell, on the same pair the all-clear replay already uses
    // (`publish_pane_subagents`) and for the same reason it is used there: the
    // lead's own `Stop` happened before the wait displaced it, and nothing is
    // ever going to say it again. A restore to `working` rings nothing —
    // `ring_of` has no word for it — so this only speaks when a finished turn
    // comes back.
    if let Some(worktree) = seated_worktree(&state, term) {
        ring_for_pane(app, &worktree, &report);
    }
}

/// The event name an inferred interrupt lands under — observed, not reported,
/// like [`ANSWERED_EVENT`] beside it.
pub(super) const INFERRED_INTERRUPT_EVENT: &str = "keystroke-interrupt";

/// What a stop gesture looks like once it is BYTES.
///
/// Read off the encoded input rather than `press.key`, which is what Orca's
/// renderer sees too (`observeSentTerminalInput`). Two things come free from
/// asking it here: `Ctrl+[` produces the same `0x1b` as the Escape key and a
/// key-name test would miss it, and an Escape that is only the PREFIX of a
/// longer sequence (`\x1b[A`) is more than one byte, so it cannot be mistaken
/// for the gesture.
pub(super) fn intent_of(bytes: &[u8]) -> Option<zerocode_core::interrupt::Intent> {
    match bytes {
        [0x1b] => Some(zerocode_core::interrupt::Intent::PlainEscape),
        [0x03] => Some(zerocode_core::interrupt::Intent::CtrlC),
        _ => None,
    }
}

/// This pane's turn, as much of it as the inference reads — or `None` when
/// there is no agent, no report, or a vendor this window cannot name.
///
/// `blocked_on_question` comes from the questions SHAPE rather than a tool's
/// name, which is the discrimination `AgentTurn` documents and the same fact
/// `SubmitShape` folds one road over.
pub(super) fn agent_turn_of(
    state: &AppState,
    term: TermId,
) -> Option<(zerocode_core::AgentKind, PaneState)> {
    let agent = state
        .agent_terms()
        .get(&term)
        .copied()
        .and_then(zerocode_core::AgentKind::from_slug)?;
    let held = state.pane_states().get(&term).cloned()?;
    Some((agent, held))
}

/// A stop gesture reached a pane's pty. Decide what it was, and act.
///
/// The machine is pure and lives in core; what belongs here is the CLOCK — the
/// settle timer Orca runs in the renderer — and the pane's facts.
pub(super) fn observe_stop_gesture(app: &AppHandle, term: TermId, bytes: &[u8]) {
    use zerocode_core::interrupt::{AgentTurn, Observed};
    let Some(intent) = intent_of(bytes) else {
        return;
    };
    let state = app.state::<AppState>();
    let now = epoch_ms_now();
    let observed = {
        let held = agent_turn_of(&state, term);
        let turn = held.as_ref().map(|(agent, row)| AgentTurn {
            agent: *agent,
            state: row.state,
            blocked_on_question: row.ask_prompt.is_some(),
            prompt: row.you.as_deref().unwrap_or_default(),
            updated_at: row.at,
            state_started_at: row.state_started_at,
            // No such mark on this side — the map's ▲ 「복원 provenance」. A
            // hydrated row therefore counts as confirmed here, which is the
            // same pre-existing breadth the answered-wait door carries.
            restored_unconfirmed: false,
        });
        let mut machines = state.interrupt_inference();
        machines
            .entry(term)
            .or_default()
            .observe(intent, turn.as_ref(), now)
    };
    match observed {
        Observed::Nothing | Observed::FirstEscape { .. } => {}
        Observed::Infer(call) => apply_inferred_interrupt(app, term, &call),
        Observed::Armed { flush_at } => {
            // The settle, on the shape `answer_ask` already uses: a newer
            // gesture bumps the stamp and this thread finds its own gone.
            let generation = {
                let mut sends = state.inference_sends();
                let slot = sends.entry(term).or_insert(0);
                *slot += 1;
                *slot
            };
            let app = app.clone();
            std::thread::spawn(move || {
                let wait = flush_at.saturating_sub(epoch_ms_now()).max(0);
                std::thread::sleep(std::time::Duration::from_millis(wait.unsigned_abs()));
                let state = app.state::<AppState>();
                if state.inference_sends().get(&term) != Some(&generation) {
                    return;
                }
                let call = {
                    let held = agent_turn_of(&state, term);
                    let turn = held.as_ref().map(|(agent, row)| AgentTurn {
                        agent: *agent,
                        state: row.state,
                        blocked_on_question: row.ask_prompt.is_some(),
                        prompt: row.you.as_deref().unwrap_or_default(),
                        updated_at: row.at,
                        state_started_at: row.state_started_at,
                        restored_unconfirmed: false,
                    });
                    let mut machines = state.interrupt_inference();
                    machines
                        .entry(term)
                        .or_default()
                        .flush(turn.as_ref(), epoch_ms_now())
                };
                if let Some(call) = call {
                    apply_inferred_interrupt(&app, term, &call);
                }
            });
        }
    }
}

/// The inference is confident. Put the outcome down — or refuse it.
///
/// **The refusals are the substance, and Orca states them at APPLY time rather
/// than at observe time** (`inferInterrupt`, `server.ts:901-996`), because a
/// settle timer runs for half a second and the pane can move inside it.
///
/// The first is not a refusal but a fork: claude's Escape on its own QUESTION
/// is not an interrupt at all, it is the question being dismissed, and Orca
/// hands it to the answered path (`dismissesClaudeQuestion`, `:934-941`).
/// Synthesizing a done there would report a turn a person stopped when what
/// they did was decline to answer yet.
///
/// The rest is [`zerocode_core::hook::gesture_leaves_work_running`]: a gesture
/// cannot have stopped work it never reaches.
pub(super) fn apply_inferred_interrupt(
    app: &AppHandle,
    term: TermId,
    call: &zerocode_core::interrupt::InterruptCall,
) {
    let state = app.state::<AppState>();
    let Some((agent, held)) = agent_turn_of(&state, term) else {
        return;
    };
    if !zerocode_core::interrupt::same_agent(agent, call.agent) {
        return;
    }
    if agent == zerocode_core::AgentKind::Claude
        && call.intent == zerocode_core::interrupt::Intent::PlainEscape
        && held.state == zerocode_core::hook::HookState::NeedsAttention
        && held.ask_prompt.is_some()
    {
        clear_answered_wait(app, term, &AnswerRoad::Card);
        return;
    }
    if held.state != zerocode_core::hook::HookState::Working {
        return;
    }
    let roster_busy = state
        .subagents()
        .get(&term)
        .is_some_and(|rows| !rows.is_empty());
    // The inventory half, through the only record of it this window keeps.
    //
    // Orca holds two registries (`claudeRunningNonAgentTaskPaneKeys`,
    // `claudeActiveSessionCronPaneKeys`) written on every event; ours reads
    // background work off each payload and remembers it only when it HELD a
    // `done` — the parked all-clear. So this asks a narrower question than
    // `:958-963`: "is this pane's `working` really a lead's `done` that
    // something is still holding up".
    //
    // **The gap is named because it errs the unsafe way.** A pane genuinely
    // working with a background shell running gets no refusal here, and Orca
    // would refuse it. The case we DO catch is the one where nothing would
    // correct us — the lead has already spoken its last word. Closing the rest
    // means keeping the registry, which is its own piece (gap map §8).
    let inventory_busy = state.pending_done().contains_key(&term);
    if zerocode_core::hook::gesture_leaves_work_running(roster_busy, inventory_busy) {
        return;
    }
    let report = hooks::PaneHookReport {
        permission_mode: None,
        term,
        agent,
        state: zerocode_core::hook::HookState::Done,
        interrupted: true,
        session_boundary: false,
        child_attributed: false,
        submit_shape: zerocode_core::ask::SubmitShape::NotAQuestion,
        event: INFERRED_INTERRUPT_EVENT.to_string(),
        session: None,
        resumable: false,
        prompt: None,
        said: None,
        ask: None,
        ask_prompt: None,
        approval: None,
        model: None,
    };
    note_pane_state(app, None, &report);
    if let Some(worktree) = seated_worktree(&state, term) {
        ring_for_pane(app, &worktree, &report);
    }
}

/// One pane whose agent has gone, put down through the doors an agent's own
/// report would have used.
///
/// Deliberately NOT a new pipeline. The state travels through
/// [`note_pane_state`] and the helpers through the same `hook:subagent` emit a
/// `SubagentStop` would take, so every surface that already draws an agent
/// stopping — the tab badge, the workspace dot, the card's agent rows, the
/// door's badge, the board's columns, a popped-out board in another window —
/// is corrected by machinery that was already there and already tested. A
/// second road would have been a second chance for one of those to be missed,
/// and the missed one is always the surface nobody was looking at.
///
/// What deliberately STAYS is the session record. `pane_sessions` is the
/// vendor's own id for this conversation, and it outlives the process — it is
/// what the 이어서 menu resumes from. An agent that was quit is the most
/// ordinary reason to want that menu, so clearing it here would take the
/// feature away from exactly the person this fix is for. (Orca draws the same
/// line: its `dropAgentStatus` clears five activity maps and does not touch
/// `sleepingAgentSessionsByPaneKey`, store-BgJxB0hr.js:25817-25861.)
pub(super) fn clear_departed_agent(app: &AppHandle, term: TermId) {
    // The report an agent would have sent if it could have. `session` is
    // `None` on purpose — the merge only ever writes a session it is GIVEN, so
    // `None` leaves the conversation record standing.
    let agent = app
        .state::<AppState>()
        .agent_terms()
        .get(&term)
        .copied()
        .and_then(zerocode_core::AgentKind::from_slug);
    let Some(agent) = agent else { return };
    note_pane_state(
        app,
        // 조용한 이탈은 봉투가 없다 — 원장에는 이미 앉은 자리의 상태만
        // Idle로 고쳐진다(새 자리를 지어낼 워크트리가 없으므로).
        None,
        &hooks::PaneHookReport {
            permission_mode: None,
            term,
            agent,
            state: zerocode_core::hook::HookState::Idle,
            session_boundary: false,
            // Neither provenance flag, and `Idle` would refuse both anyway
            // (`hook::done_provenance`). Worth saying rather than leaving to the
            // clamp: an agent whose PROCESS left is not a person interrupting a
            // turn — nobody pressed anything, we noticed an absence. Orca keeps
            // the same line, inferring interrupts from a keystroke intent and
            // never from a process going away.
            interrupted: false,
            // Nobody's helper: this is the WINDOW speaking about a pane whose
            // agent left, so there is no child to attribute it to — and the
            // stash the flag feeds would refuse an `Idle` anyway. Saying it
            // here means the row's stash is dropped along with everything else
            // the departed agent was holding.
            child_attributed: false,
            // 떠난 에이전트의 판에는 끝낼 질문이 없다 — 거절하는 낱말이 곧
            // 기본값이라 여기서도 그것이다.
            submit_shape: zerocode_core::ask::SubmitShape::NotAQuestion,
            // Named for what was observed rather than borrowed from a vendor's
            // vocabulary, so a reader of the event stream can tell this apart
            // from anything an agent said.
            event: "foreground-returned".to_string(),
            session: None,
            resumable: false,
            prompt: None,
            said: None,
            ask: None,
            ask_prompt: None,
            approval: None,
            model: None,
        },
    );
    // And the helpers, which died with the agent that was running them. No
    // `SubagentStop` is coming for these either — the process that would have
    // sent one is the process that just left — so their rows would sit under a
    // calm parent forever, each one claiming to be running.
    clear_pane_subagents(app, term);
}

/// Pay the publishes the state doors owe (t-3098).
///
/// `forget_term_state` retires a helper whose own pane ended in its parent's
/// roster, but it holds only the state and cannot emit; it writes the parent
/// down (`unpublished_rosters`) and the pump — which holds the one
/// `AppHandle` — tells the window here, through the roster's one door, so a
/// parked all-clear replays when that was the last helper. Every round and
/// after the reaper, because the doors that end a pane by hand
/// (`retire_terminal`) run off the pump's clock; a round with nothing owed
/// costs one empty lock. The roster is read after the set's lock is dropped,
/// so the two are never held at once.
pub(super) fn publish_owed_rosters(app: &AppHandle) {
    let owed: Vec<TermId> = {
        let state = app.state::<AppState>();
        let mut owed = state.unpublished_rosters();
        owed.drain().collect()
    };
    for parent in owed {
        let rows = app
            .state::<AppState>()
            .subagents()
            .get(&parent)
            .cloned()
            .unwrap_or_default();
        publish_pane_subagents(app, parent, rows);
    }
}

/// Drop a pane's whole helper roster: the rows, their activity rings, and the
/// empty-list emit that clears every window. One door for the two roads that
/// end a roster wholesale — the departed-agent sweep above, and a session
/// boundary (a new process owns the pane, and the helpers belonged to the
/// conversation that ended).
pub(super) fn clear_pane_subagents(app: &AppHandle, term: TermId) {
    // A parked all-clear goes with the roster that was holding it: the sweep
    // means the agent left, the boundary means a new conversation owns the
    // pane, and replaying a dead turn's Done into either would ring about
    // work nobody is doing.
    app.state::<AppState>().pending_done().remove(&term);
    let held = app.state::<AppState>().subagents().remove(&term);
    if let Some(rows) = held {
        // Their activity rings go with them, by the same door a helper that
        // stopped properly takes: keyed by a card name nothing will ever
        // mention again, which is what makes a leftover one a leak.
        for row in &rows {
            app.state::<AppState>()
                .activities()
                .remove(&hooks::activity_subagent(term, &row.id));
        }
        let _ = app.emit(
            "hook:subagent",
            PaneSubagents {
                term,
                rows: Vec::new(),
            },
        );
    }
}

/// What a pane's agent last asked and last said, for the notification body.
///
/// Read here rather than inside [`ring_now`], because a lane has no `TermId` to
/// look anything up by — its words are its own, and it hands them over.
pub(super) fn pane_words(app: &AppHandle, term: TermId) -> (Option<String>, Option<String>) {
    let state = app.state::<AppState>();
    let states = state.pane_states();
    let held = states.get(&term);
    (
        held.and_then(|one| one.ask.clone()),
        held.and_then(|one| one.said.clone()),
    )
}

/// The ring itself: suppression first, cooldown second, then the OS.
///
/// In that order on purpose — a suppressed ring must not spend the cooldown,
/// or working at the active screen would silence the same worktree's genuine
/// ring five seconds after you look away.
///
/// The words come from the caller. There are two kinds of ringer now — a pane
/// keyed by `TermId` and a lane that has none — and a bell that fetched its own
/// body could only serve the first. One bell, two callers, and the OS call
/// lives here exactly once.
pub(super) struct RingNotice<'a> {
    pub(super) worktree: &'a str,
    pub(super) term: Option<TermId>,
    pub(super) agent: &'a str,
    pub(super) ring: zerocode_core::notify::Ring,
    // Whether a PERSON ended the turn. Rides beside `ring` because the two of
    // them decide one thing — the verb — and the ladder below reads neither
    // until it reaches the words.
    pub(super) interrupted: bool,
    pub(super) ask: Option<String>,
    pub(super) said: Option<String>,
}

/// The OS call, behind a seam: the ladder decides, a host shows. The real host
/// is the Tauri app; a test hands the same call a fake that remembers it.
pub(super) trait NotificationHost {
    fn show(&self, title: &str, body: &str) -> Result<(), String>;
}

impl NotificationHost for AppHandle {
    fn show(&self, title: &str, body: &str) -> Result<(), String> {
        use tauri_plugin_notification::NotificationExt;
        self.notification()
            .builder()
            .title(title)
            .body(body)
            .show()
            .map_err(|error| error.to_string())
    }
}

/// One notice to one host — the only line that speaks to the OS.
pub(super) fn show_notice(
    host: &dyn NotificationHost,
    notice: &zerocode_core::notify::Notice,
) -> Result<(), String> {
    host.show(&notice.title, &notice.body)
}

/// The word a notification calls a worktree by: its last path segment.
pub(super) fn place_of(worktree: &str) -> String {
    Path::new(worktree)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl RingNotice<'_> {
    /// What this ring shows, before any gate: the bell's title shape with the
    /// ring's verb, and the words the caller handed over. One composition for
    /// the ladder and for a test that hands it to a fake host.
    pub(super) fn notice(&self) -> zerocode_core::notify::Notice {
        zerocode_core::notify::notice(
            &place_of(self.worktree),
            self.agent,
            self.ring,
            self.interrupted,
            self.ask.as_deref(),
            self.said.as_deref(),
        )
    }
}

/// The ring a zo pane's `PushNotification` earns (t-2943): the push kind,
/// the model's message as the words, and the pane as the address a click
/// comes back to. A push has no question and reads no interrupt flag.
pub(super) fn zo_push_ring_notice<'a>(
    worktree: &'a str,
    term: TermId,
    push: &ZoPush,
) -> RingNotice<'a> {
    RingNotice {
        worktree,
        term: Some(term),
        agent: AgentKind::Zo.slug(),
        ring: zerocode_core::notify::Ring::Push,
        interrupted: false,
        ask: None,
        said: Some(push.body.clone()),
    }
}

/// A zo pane's `PushNotification` on the window road: the same ladder as
/// every other bell, under the attention kind. A pane with no seat has no
/// worktree, and a ring with no worktree has nowhere to take the person — so,
/// as for the inferred bells, the absence is the honest answer.
pub(super) fn ring_zo_push(app: &AppHandle, term: TermId, push: &ZoPush) {
    let Some(worktree) = seated_worktree(&app.state::<AppState>(), term) else {
        return;
    };
    ring_now(app, zo_push_ring_notice(&worktree, term, push));
}

pub(super) fn ring_now(app: &AppHandle, notice: RingNotice<'_>) {
    // Composed before the ladder so the gates below read only the facts they
    // need — the worktree, the pane, the kind, and (for the seat) who rang
    // and whether a person ended the turn.
    let composed = notice.notice();
    let RingNotice {
        worktree,
        term,
        agent,
        ring,
        interrupted,
        ..
    } = notice;
    if !ring_gates_open(app, ring) {
        return;
    }
    let (active, focused) = watching(app);
    let watched = zerocode_core::notify::suppressed(worktree, &active, focused);
    // The notify seat is asked HERE — after the switches, before the
    // cooldown — at the one point today's table decides "ring or not"
    // (t-6043, `zerocode_core::jev::NOTIFY`). A watched screen is today's
    // own ignore and is not a question; a cooldown is a rate, not a
    // judgment. Under `off`, `shadow`, a timeout or a refusal the call is
    // today's, and the bytes below run exactly as they did before the seat
    // existed; only an `auto` its own evidence raised lets `batch` hold the
    // ring for the person's next hand and `ignore` drop it.
    if !watched {
        let bell = notify_call::Bell {
            worktree,
            term,
            agent,
            ring,
            interrupted,
            notice: &composed,
            focused,
        };
        match notify_call::call_at_the_bell(app, &bell) {
            zerocode_core::notify_call::Call::Interrupt => {}
            zerocode_core::notify_call::Call::Batch => {
                notify_call::hold(app, &bell);
                return;
            }
            zerocode_core::notify_call::Call::Ignore => {
                notify_call::hush(app, term, zerocode_core::notify_call::Call::Ignore);
                return;
            }
        }
    }
    if watched {
        return;
    }
    ring_composed(app, worktree, term, &composed);
}

/// The rings the notify seat held, told once at the person's next hand
/// (t-6043): the folded notice walks the same gates as any bell — the
/// switches, the watched screen, the cooldown — at the first held ring's
/// address, and is not asked of the seat again.
pub(super) fn ring_held(
    app: &AppHandle,
    worktree: &str,
    term: Option<TermId>,
    ring: zerocode_core::notify::Ring,
    folded: &zerocode_core::notify::Notice,
) {
    if !ring_gates_open(app, ring) {
        return;
    }
    let (active, focused) = watching(app);
    if zerocode_core::notify::suppressed(worktree, &active, focused) {
        return;
    }
    ring_composed(app, worktree, term, folded);
}

/// The ladder's first rungs (Orca `notifications.ts:121-131`): the tray's
/// attention dot before any gate, then the master switch and the kind's
/// own — whether a hook rings or a lane rings, the same door. The lane bell
/// going straight past these was the second ill of map P0-14, and a gate
/// standing at each call site was the cause of it.
fn ring_gates_open(app: &AppHandle, ring: zerocode_core::notify::Ring) -> bool {
    let state = app.state::<AppState>();
    // 트레이의 주의 점은 어떤 게이트보다도 먼저다 — Orca 자신의 순서
    // (`notifications.ts:113-119`, "before the cooldown/focus/enabled gates
    // so they can't hold it back"): 알림을 꺼 둔 사람에게도 "무언가 끝났다"
    // 는 점 하나는 남아야 한다. 창 가시성은 `note_activity`가 스스로 본다.
    let source = match ring {
        zerocode_core::notify::Ring::Completion => native_tray::ActivitySource::AgentTaskComplete,
        zerocode_core::notify::Ring::Attention | zerocode_core::notify::Ring::Push => {
            native_tray::ActivitySource::AgentAttention
        }
    };
    state.native_tray().note_activity(app, source);
    // 사다리는 여기 한 곳이다(Orca `notifications.ts:121-131`): 마스터,
    // 종류별, 초점, 쿨다운 — 훅이 울리든 레인이 울리든 같은 문을 지난다.
    // 레인 벨이 이 검사 없이 직행하던 것이 맵 P0-14의 두 번째 병이었고,
    // 게이트가 호출부마다 서 있던 것이 그 병의 원인이었다.
    let preferences = load_settings_for_boot(state.settings())
        .document
        .notifications;
    if !preferences.enabled {
        return false;
    }
    // A push the agent sent on purpose is the agent pulling a person toward
    // something to act on — the attention kind, under the attention switch.
    if matches!(
        ring,
        zerocode_core::notify::Ring::Attention | zerocode_core::notify::Ring::Push
    ) && !preferences.agent_attention
        || matches!(ring, zerocode_core::notify::Ring::Completion) && !preferences.agent_completion
    {
        return false;
    }
    true
}

/// The two facts the watched-screen rule reads: the active worktree and
/// whether the main window has focus.
fn watching(app: &AppHandle) -> (String, bool) {
    let active = app.state::<AppState>().active_root().display().to_string();
    let focused = app
        .get_webview_window("main")
        .and_then(|window| window.is_focused().ok())
        .unwrap_or(false);
    (active, focused)
}

/// The ladder's last rungs: the cooldown, then the OS, then the address a
/// click comes back to.
fn ring_composed(
    app: &AppHandle,
    worktree: &str,
    term: Option<TermId>,
    composed: &zerocode_core::notify::Notice,
) {
    let state = app.state::<AppState>();
    if !state.rings().may_ring(worktree, epoch_ms_now()) {
        return;
    }
    let shown = show_notice(app, composed);
    if shown.is_err() {
        // Once per session, not per ring: a denied permission is one fact.
        static SAID_NO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !SAID_NO.swap(true, std::sync::atomic::Ordering::Relaxed) {
            let _ = app.emit("notify:blocked", ());
        }
    } else {
        // 마지막 벨의 주소 — macOS `Reopen`이 사람을 데려다줄 곳(P0-14 클릭
        // 근사). 발사됐다는 사실 자체가 "그 워크트리를 보고 있지 않았다"는
        // 뜻이다: 활성+초점이었다면 suppressed가 위에서 돌려보냈다.
        *state.last_ring() = Some(LastRing {
            worktree: worktree.to_string(),
            term,
            at: epoch_ms_now(),
        });
    }
}

pub(super) fn note_terminal_bell(app: &AppHandle) {
    app.state::<AppState>()
        .native_tray()
        .note_activity(app, native_tray::ActivitySource::TerminalBell);
}

/// Announce a terminal's bell and title on the round they happened.
///
/// Every shell, read or not: a frame now waits for its screen to come and pull
/// it, and a hidden or minimised window never comes — the tray and the tab
/// badge must not wait with it (design rule 8, I8). The grid answers each fact
/// once ([`zerocode_pty::TerminalGrid::take_news`]), so a bell is counted once
/// however many screens later pull the frame that still carries it.
pub(super) fn announce_term_news(app: &AppHandle, term: TermId, news: zerocode_pty::GridNews) {
    if news.bell {
        note_terminal_bell(app);
        let _ = app.emit("term:bell", TermBell { term });
    }
    if let Some(title) = news.title {
        let _ = app.emit("term:title", TermTitle { term, title });
    }
}

/// The shells at least one window is PREVIEWING, out of what every window said.
///
/// A union: one window previewing a shell is reason enough to send its
/// preview. An empty map means nobody has declared anything yet — a window
/// that has not booted far enough to say — and answers "preview nothing". The
/// reading tier is a different question with a different answer
/// ([`readers_by_term`]: WHO reads each shell), and a caller that unioned the
/// two would have no way left to ask which tier a term belongs to.
pub(super) fn previewed_anywhere(windows: &HashMap<String, HashSet<TermId>>) -> HashSet<TermId> {
    windows.values().flatten().copied().collect()
}

/// The notice that tells one window its terminals have frames to pull.
///
/// Empty on purpose: the frames themselves cross on that window's own pull
/// (`term_pull`), as bytes, when the window is ready for them — this only says
/// "come". Lost, it costs a moment: the reader is told again after
/// [`zerocode_pty::RENOTIFY_AFTER`].
///
/// **Why the label is in the NAME rather than in a filter.** `app.emit` hands
/// a payload to every webview holding a listener for that event, and a plain
/// `listen()` registers with `EventTarget::Any`, which Tauri short-circuits
/// ahead of every filter (`event/listener.rs`, `match_any_or_filter`) — so
/// `emit_to` cannot narrow a broadcast that a window subscribed to by name.
/// What Tauri *does* consult before it evaluates anything is the label's own
/// listener map: `emit_js_filter` looks up `js_listeners[label][event]` and
/// skips a webview that holds no listener for that event AT ALL — no payload,
/// no eval. A name per window is therefore the narrowing, in the one currency
/// the dispatcher reads before it spends anything.
///
/// **Why not a channel.** The frame road WAS a `tauri::ipc::Channel` for one
/// build (v1.3.96-97). A channel numbers its messages, and the webview half
/// holds every later message in `#pending` until the missing index arrives —
/// a stream where one lost message is not one lost frame but **every frame
/// after it, forever**: the screen stops, the echo of a keystroke stops with
/// it, and nothing short of a reload lets go. A notice is delivered on its own
/// and owes nothing to the one before it, and a pull answers for itself.
pub(super) fn dirty_event_for(label: &str) -> String {
    format!("{TERM_DIRTY_EVENT}:{label}")
}

/// The stem every notice's name is built on.
pub(super) const TERM_DIRTY_EVENT: &str = "term:dirty";

/// Which windows asked for each shell, inverted from the per-window sets.
///
/// Delivery needs the addresses: every window that declared a shell is one of
/// its readers (`zerocode_pty::readers`), and no other window is. A shell with
/// no entry is read by nobody — frames stay in its grid, and a screen that
/// starts reading it later is owed a snapshot on its first pull. An empty map
/// means nobody has declared anything yet and reads as "nobody reads
/// anything", which is right: the first thing every surface does on the way to
/// visible is declare itself. Sorted so two rounds over the same declaration
/// keep the same readers in the same order — a board pop-out and the main
/// window must not swap places between rounds for reasons nobody can name.
pub(super) fn readers_by_term(
    windows: &HashMap<String, HashSet<TermId>>,
) -> HashMap<TermId, Vec<String>> {
    let mut readers: HashMap<TermId, Vec<String>> = HashMap::new();
    for (label, terms) in windows {
        for term in terms {
            readers.entry(*term).or_default().push(label.clone());
        }
    }
    for labels in readers.values_mut() {
        labels.sort();
    }
    readers
}

/// Whether a shell that just handed over `moved` bytes still owes this round
/// another pump.
///
/// A short read is the whole answer: [`zerocode_pty::PtyLane::pump`] drains
/// until the channel is empty or the budget is spent, so anything under the
/// budget means the child had nothing more *at that instant*. Only a FULL
/// budget is evidence of a queue behind it, and only then is it worth going
/// back — going back on a short read would spin on an idle shell.
///
/// `ended` stops the asking for the obvious reason, and the deadline stops it
/// for the one that matters: the round has a frame to paint, and a child that
/// can write faster than this machine can parse (`yes`, a runaway log) would
/// otherwise never come up short and would own the loop.
///
/// Pure, and separate from the loop, so the rule can be read and tested
/// without a pty (`a_full_read_is_the_only_reason_to_go_back_for_more`).
pub(super) fn owes_another_pump(
    moved: usize,
    ended: bool,
    now: Instant,
    deadline: Instant,
) -> bool {
    moved >= zerocode_pty::PTY_OUTPUT_BYTE_BUDGET && !ended && now < deadline
}

/// What one pump round has learned about the chase a wake armed
/// ([`CHASE_INTERVAL`]).
///
/// The chase looks for one thing: the child's answer to the write that woke
/// the pump. "The round moved" is not that answer — another shell's output
/// moves a round, and so does the written shell's own output that was on its
/// way before the write — so each shell says, as the round parses it, whether
/// its answer is in and whether a write of its own still waits
/// ([`zerocode_pty::Pumped`]). Pure, so the rule is read and tested without a
/// pty (`a_chase_ends_on_the_answer_to_its_write_not_on_other_output`).
#[derive(Default)]
pub(super) struct ChaseRound {
    /// A shell this round still waits for its child to answer a write young
    /// enough to chase.
    waiting: bool,
}

impl ChaseRound {
    /// One shell as the round parsed it, and the look its readers get: a
    /// screen that is flowing is told too on the round its answer is parsed
    /// ([`zerocode_pty::Look::Chase`]) — whatever round that is, a chase look
    /// or a display beat, because a flowing screen would otherwise take the
    /// answer on its own next display frame and paint it a frame late.
    ///
    /// A write is waited on for one chase's span and no longer. A program that
    /// never answers — a password prompt, a key it ignores — must not keep
    /// every later wake chasing too.
    pub(super) fn saw(&mut self, pumped: zerocode_pty::Pumped, now: Instant) -> zerocode_pty::Look {
        self.waiting |= pumped.unanswered_since.is_some_and(|since| {
            now.saturating_duration_since(since) < CHASE_INTERVAL * CHASE_ROUNDS
        });
        if pumped.answered {
            zerocode_pty::Look::Chase
        } else {
            zerocode_pty::Look::Beat
        }
    }

    /// The chase left after the round. A round that moved anything ends it —
    /// the echo, or the scrolled frame, is out, and the display beat batches
    /// whatever follows — unless a write is still waiting for its answer:
    /// then what moved was something else, and the chase goes on until the
    /// answer or its own rounds run out.
    pub(super) const fn chase_left(&self, chase: u32, moved: bool) -> u32 {
        if moved && !self.waiting { 0 } else { chase }
    }
}

/// Drive every lane — and the shell terminal — forward, forever. One thread
/// for the whole window: a pty read never blocks here (`pump` is
/// non-blocking) and the registry is built to be driven by exactly one
/// cadence owner.
pub(super) fn pump_loop(app: &AppHandle) {
    let local_data_root = app.state::<AppState>().local_data_root().to_path_buf();
    // How many rounds in a row have produced nothing. The cadence drops only
    // after a run of them, so the gap between two keystrokes does not cost a
    // wake-up on the next letter.
    let mut quiet: u32 = 0;
    // How many chase looks the last wake still has to spend (see
    // [`CHASE_INTERVAL`]): counted down by empty rounds, cleared by the first
    // round that produces anything while no write waits for its answer
    // (`ChaseRound`), re-armed in full by every wake.
    let mut chase: u32 = 0;
    // The last minute the schedule was consulted. Once a minute, not once a
    // round: the schedule has minute resolution, and asking sixty times a
    // second would read the automations file sixty times a second to learn
    // nothing new.
    let mut swept: Option<i64> = None;
    // And the last time the terminals were asked who is holding them. Twice a
    // second by the same argument: which process group owns a terminal changes
    // when somebody starts or ends a program, not sixty times a second.
    let mut looked: Option<Instant> = None;
    // And the last time the pump asked after children that have gone quiet
    // WITHOUT closing their output, plus the ones caught looking dead on the
    // previous ask. One entry per shell that has exited, cleared the moment it
    // is reported, so this holds at most the handful of panes dying at once.
    let mut reaped: Option<Instant> = None;
    let mut dying: std::collections::HashSet<TermId> = std::collections::HashSet::new();
    // And the last beat every standing order was given. This is the whole of
    // "automatic": a coordinator writes down what it wants kept running and
    // stops having to be awake to act on it.
    let mut beat: Option<Instant> = None;
    // Where each pane's foreground process stands — a person's pace (pane_cwd_runtime).
    let mut cwd_looked: Option<Instant> = None;
    // The pool snapshot, and the two tables the round's questions are asked
    // of. Declared out here for their CAPACITY and nothing else: all three are
    // emptied again before the loop naps, so a shell that left the pool is
    // never held past the round that read it — a pty master kept alive for an
    // extra display frame is a child that outlives the pane it was.
    let mut entries: Vec<(TermId, terminal_registry::HeldTerminal)> = Vec::new();
    // Which shell the round starts its walk on. The drain below is a bonus the
    // EARLIEST saturated shell tends to collect — it spends the round's budget
    // first — and a pool walked in the same order every round would hand that
    // bonus to the same shell forever. Turning the walk by one each round means
    // every shell leads the walk once per lap. (Nothing else in the round
    // depends on the order: frames, deaths and settled deliveries are each
    // keyed by their own term.)
    let mut turn: usize = 0;
    let mut slot_of: HashMap<TermId, usize> = HashMap::new();
    let mut ended: std::collections::HashSet<TermId> = std::collections::HashSet::new();
    loop {
        // When this round started, for the two clocks that hang off it: how
        // long the drain may run (`DRAIN_BUDGET`) and how much of the display
        // frame is left to sleep at the end. Both were a flat interval before,
        // which made the round's real period `work + PUMP_INTERVAL` — a beat
        // that drifts with the load rather than holding at the display rate.
        let round_began = Instant::now();
        let drain_until = round_began + DRAIN_BUDGET;
        let minute = local_minute_now();
        if swept != Some(minute.epoch_minutes) {
            swept = Some(minute.epoch_minutes);
            tick_automations(app, minute);
        }
        let events = {
            let state = app.state::<AppState>();
            let mut registry = state.registry();
            registry.pump()
        };
        // Noted before the loop consumes them — this round produced work
        // whether or not the vector still exists afterwards.
        let had_events = !events.is_empty();
        for event in events {
            emit_lane_event(app, event);
        }

        // Every shell, not one: a terminal tab is its own process, so the
        // cadence owner has to turn all of them. Collected under the lock and
        // emitted outside it — an emit reaches into the webview, and holding
        // the pool across that would block every terminal command meanwhile.
        // Whether this round asks the second question about a shell (see
        // `REAP_LOOK_EVERY`). Decided before the lock so the clock is read
        // once for the whole pool rather than once per shell.
        let reap_at = Instant::now();
        let reaping = reaped.is_none_or(|last| reap_at.duration_since(last) >= REAP_LOOK_EVERY);
        if reaping {
            reaped = Some(reap_at);
        }
        if beat.is_none_or(|last| reap_at.duration_since(last) >= AUTO_BEAT_EVERY) {
            beat = Some(reap_at);
            beat_standing_orders(app);
        }
        if cwd_looked
            .is_none_or(|last| reap_at.duration_since(last) >= pane_cwd_runtime::CWD_LOOK_EVERY)
        {
            cwd_looked = Some(reap_at);
            pane_cwd_runtime::sweep_pane_cwds_off_the_beat(app);
        }
        let state = app.state::<AppState>();
        // WHO reads each shell, from the windows' own declarations. Taken once
        // and released before any terminal lock: a declaration that lands
        // mid-round is read on the next round, and in between the readers a
        // terminal holds are the ones `set_watched_terms` already declared or
        // forgot under that terminal's own lock.
        let readers = readers_by_term(&state.watched_terms());
        // One clock for every reader's look this round, and what the round
        // learns about the chase shell by shell (`ChaseRound`).
        let looked_at = Instant::now();
        let mut chase_round = ChaseRound::default();
        let (told, news, background_activity, clipboard_writes, gone, settled, pumped_any) = {
            // A SNAPSHOT of the pool, not its lock: the map lock is held for
            // the `Arc` clones and released, and each shell below is parsed
            // under its own terminal lock — so a keystroke waits, at worst,
            // for its own terminal's parse and never for the whole round.
            // The lock order this leans on lives on [`TerminalRegistry`]:
            // `deliveries` is taken before any terminal lock, terminal locks
            // are taken one at a time, and the map lock nests inside
            // `deliveries` but never inside a terminal lock.
            state.terminals().entries_into(&mut entries);
            if !entries.is_empty() {
                let lead = turn % entries.len();
                entries.rotate_left(lead);
                turn = turn.wrapping_add(1);
            }
            // The windows to tell to come and pull — one entry per window per
            // shell that owes it one, folded to one notice a window below.
            let mut told: Vec<std::sync::Arc<str>> = Vec::new();
            let mut news: Vec<(TermId, zerocode_pty::GridNews)> = Vec::new();
            let mut background_activity: Vec<TermId> = Vec::new();
            let mut clipboard_writes: Vec<String> = Vec::new();
            let mut pumped_any = false;
            // The code travels with the id because it can only be asked for
            // here: the entry is dropped a few lines below, and the child goes
            // with it.
            let mut gone: Vec<(TermId, Option<i32>, String)> = Vec::new();
            let mut settled: Vec<(TermId, DeliveryOutcome, Option<String>)> = Vec::new();
            // Held across the walk, exactly as the old single pool lock was:
            // a settled delivery and the next queued prompt must swap under
            // one guard, and a death's cleanup must finish before anybody
            // can address the dead shell again.
            let mut deliveries = state.deliveries();
            for (id, held) in &entries {
                // What the window knows about this pane's LINE, for the
                // delivery waiting on it: whose hand is there, whether a
                // question is parked, which launch it holds. Read before the
                // pane's own lock and only when something is waiting to
                // write, so an idle pane pays nothing and no terminal lock
                // ever nests inside the maps these come from.
                let line = if deliveries.contains_key(id) {
                    prompt_transaction::line_facts(&state, *id)
                } else {
                    zerocode_pty::ready::Line::default()
                };
                let mut pty = lock_pty(held);
                /* Drain this shell, rather than take one budget off it.
                 *
                 * `pump` moves at most [`zerocode_pty::PTY_OUTPUT_BYTE_BUDGET`]
                 * and says how much it moved. A full budget means the child has
                 * more waiting, and the round used to nap on that fact — 256 KiB
                 * per 16 ms is 16 MiB/s, against a parser measured at 80.9 MiB/s
                 * (see [`DRAIN_BUDGET`]). So keep asking while the reads stay
                 * full and the round's drain budget holds.
                 *
                 * `bytes` accumulates across the calls because every reader of
                 * it downstream — `pumped_any`, the background-activity note,
                 * `Observed::wrote` — is asking "did this child say anything
                 * this round", and the answer has to cover the whole round.
                 * `ended` takes the LAST call's word: a child that closed its
                 * output during the drain is ended, and one that has not is not.
                 */
                let mut pumped = pty.pump();
                while owes_another_pump(pumped.bytes, pumped.ended, Instant::now(), drain_until) {
                    let again = pty.pump();
                    // A short read here is the channel running dry mid-drain,
                    // which is the ordinary way out of this loop.
                    if again.bytes == 0 && !again.ended {
                        break;
                    }
                    pumped.bytes = pumped.bytes.saturating_add(again.bytes);
                    pumped.ended = again.ended;
                    pumped.answered |= again.answered;
                    pumped.unanswered_since = again.unanswered_since;
                }
                pumped_any |= pumped.bytes > 0;
                let look = chase_round.saw(pumped, looked_at);
                // Read off the round just parsed, before the readers look: a
                // frame is taken by a look here or by a window's pull on
                // another thread, so the ready signal a delivery waits for —
                // a glyph that appeared this round — keeps its own record
                // (`take_glyph_drawn`) that no take clears.
                //
                // The glyph a delivery is listening for — Codex says its
                // composer line is drawn by printing `›` on it — is asked
                // for every round and of every shell, delivery or not. The
                // answer is an edge, and taking it is what ends the round: a
                // round left untaken keeps its glyph, and the next delivery
                // to arrive would read that as a program announcing itself
                // to *it*, having in fact drawn nothing since.
                let (seen, clipboard_write) = {
                    let listening_for = deliveries.get(id).and_then(PromptDelivery::marker);
                    let grid = pty.terminal_mut().grid_mut();
                    let drawn = grid.take_glyph_drawn(listening_for);
                    (
                        Observed {
                            wrote: pumped.bytes > 0,
                            bracketed_paste: grid.bracketed_paste(),
                            cursor_shows: grid.cursor_shows(),
                            marker_written: drawn.anywhere,
                            marker_in_alt: drawn.in_alt_screen,
                            alt_screen: grid.alt_screen(),
                        },
                        grid.take_osc52_clipboard_write(),
                    )
                };
                if let Some(text) = clipboard_write {
                    clipboard_writes.push(text);
                }
                if let Some(delivery) = deliveries.get_mut(id) {
                    // Human input records its marker before releasing this
                    // same pty lock. Refresh after acquiring it, so a key or
                    // IME commit that won the lock cannot be missed.
                    let line = prompt_transaction::refresh_hand(*id, line);
                    // The one seam every prompt producer's write crosses:
                    // the delivery is turned against the grid's facts AND
                    // the line's, and what it withholds it withholds here,
                    // at the write (`prompt_transaction`). Queue rejection
                    // settles immediately, including while the child lives.
                    if let prompt_transaction::Turned::Settled(outcome) = prompt_transaction::turn(
                        *id,
                        delivery,
                        seen,
                        line,
                        Instant::now(),
                        |bytes| pty.write_input(bytes).is_ok(),
                    ) {
                        // The words travel with a failure: the person told
                        // to paste them has to have them.
                        let words = prompt_transaction::words_to_hand_back(delivery, outcome);
                        settled.push((*id, outcome, words));
                    }
                }
                /* The screen pulls; the pump only looks (design §4.1).
                 *
                 * The bell and the title are announced the round they happen,
                 * off every shell, read or not — a hidden window never pulls
                 * and the tray must not wait for it. Then the readers this
                 * shell's declaration names are brought in line and looked
                 * at: a delta is taken here only for a reader nobody expects,
                 * and that reader is told to come. A reader already on its way
                 * takes its own frame when it arrives, so a screen that paints
                 * slower than this loop turns is handed ONE frame per paint,
                 * folded by the grid, instead of a queue of stale ones — except
                 * on the round that parsed its shell's answer to a write, which
                 * tells it too (`look`). */
                let labels = readers.get(id).map(Vec::as_slice).unwrap_or_default();
                let (frame_readers, grid) = pty.terminal_mut().readers_mut();
                let announced = grid.take_news();
                if !announced.is_empty() {
                    news.push((*id, announced));
                }
                frame_readers.reconcile(labels);
                frame_readers.look(grid, looked_at, look, &mut told);
                if labels.is_empty() && pumped.bytes > 0 {
                    background_activity.push(*id);
                }
                if pumped.ended {
                    // Two different questions to the operating system: the
                    // output side has closed, and the child has been reaped.
                    // They answer a moment apart, so a code that is not there
                    // yet is `None` — never a zero, which would read as a
                    // clean exit in the history for the rest of the file's
                    // life.
                    let code = pty
                        .try_wait()
                        .ok()
                        .flatten()
                        .and_then(|code| i32::try_from(code).ok());
                    gone.push((*id, code, pty.terminal().grid().visible_text()));
                    dying.remove(id);
                } else if reaping {
                    // The OTHER question, and until now nobody asked it: the
                    // child has EXITED. `has_ended` reports that the output
                    // side closed, which the lane's own documentation is at
                    // pains to say is a different fact — and the two come
                    // apart exactly when the child's pty stays referenced
                    // after it dies. Then no EOF ever arrives, this loop
                    // never learns of the death, `term:exited` never reaches
                    // the window, and the pane stands EMPTY for as long as
                    // the window is open. The only thing that used to break
                    // the spell was typing into it: a delivery's failed write
                    // is what set `ended` on the roads where it was ever set
                    // at all, so a pane nobody typed into stayed forever.
                    // (2026-08-19: terms 4 and 6, blank on screen, no process
                    // behind either, three pty masters still held.)
                    //
                    // Confirmed on a SECOND look rather than the first. A
                    // child can exit with bytes still travelling, and every
                    // round between the two looks pumps them into the grid —
                    // so the last thing a program said is on screen before
                    // its pane is taken away, which is the whole reason to
                    // wait a beat.
                    let code = pty
                        .try_wait()
                        .ok()
                        .flatten()
                        .and_then(|code| i32::try_from(code).ok());
                    if code.is_some() {
                        if dying.remove(id) {
                            gone.push((*id, code, pty.terminal().grid().visible_text()));
                        } else {
                            dying.insert(*id);
                        }
                    } else {
                        dying.remove(id);
                    }
                }
            }
            // Dropped under the same lock that decided they were finished, so
            // nothing can start a second delivery for a shell whose first one
            // is still in this vector.
            for (id, outcome, _) in &settled {
                // A withheld write is news for the black box: the reader
                // looking for why a line never went in finds the guard's own
                // sentence, once, beside the term it was about.
                if let Some(line) = prompt_transaction::withheld_line(*id, *outcome) {
                    note_window_event(state.local_data_root(), &line);
                }
                // The line is free: the next parked prompt takes it NOW,
                // under the same lock, on a fresh clock
                // (`prompt_transaction::settle`).
                let mut waiters = state.delivery_waiters();
                let mut queue = state.prompt_queue();
                prompt_transaction::settle(
                    *id,
                    *outcome,
                    &mut deliveries,
                    &mut waiters,
                    &mut queue,
                    Instant::now(),
                );
            }
            for (id, _, _) in &gone {
                deliveries.remove(id);
                if let Some(waiting) = state.delivery_waiters().remove(id) {
                    let _ = waiting.send(DeliveryOutcome::TimedOut);
                }
                // A dead shell answers no more prompts; what was parked for
                // it goes too, or the map holds text addressed to nobody.
                state.prompt_queue().remove(id);
            }
            // Where each shell sits in the snapshot. Both questions below —
            // which entry a death carries the evidence of, and whether a frame
            // is still addressed to the shell it was read from — used to walk
            // the whole pool once per frame, so a window whose panes were all
            // printing paid for every pane in every one of them. Filled only
            // when there is something to ask about: a round where nobody
            // printed and nobody died asks nothing and touches neither table.
            if !gone.is_empty() || !news.is_empty() || !background_activity.is_empty() {
                slot_of.extend(
                    entries
                        .iter()
                        .enumerate()
                        .map(|(slot, (id, _))| (*id, slot)),
                );
                ended.extend(gone.iter().map(|(id, _, _)| *id));
            }
            // The shell exited (the person typed `exit`), so the entry goes —
            // the next open on that surface spawns a fresh one. Removed by
            // the OBSERVED entry, never by id alone: this round worked from a
            // snapshot, and between snapshot and removal the id can be
            // retired and reopened (the float keeps its id), so a bare
            // remove could take a live replacement with a dead shell's
            // evidence. Still under the `deliveries` guard, so a prompt
            // taking that guard after a death always finds the terminal
            // already gone.
            for (id, _, _) in &gone {
                if let Some((_, held)) = slot_of.get(id).map(|slot| &entries[*slot]) {
                    state.terminals().remove_exact(*id, held);
                }
            }
            // News lands only on the shell it was read from. The pool can
            // retire and even replace an id while this round is parsing, and
            // a bell from the old shell would badge a replacement that never
            // rang. A shell that ended THIS round still announces its last
            // news — its removal above was our own. (A notice needs no such
            // guard: it carries nothing, and the pull it asks for reads the
            // shell that is there when it comes.)
            let still_current = |term: TermId| {
                ended.contains(&term)
                    || slot_of
                        .get(&term)
                        .map(|slot| &entries[*slot])
                        .is_some_and(|(_, held)| state.terminals().still_holds(term, held))
            };
            news.retain(|(term, _)| still_current(*term));
            background_activity.retain(|term| still_current(*term));
            // Emptied here rather than on the way in, so nothing in this round
            // outlives the round: the handles go back to the pool's own count
            // the moment the answers have been taken, and only the capacity
            // waits for the next one.
            entries.clear();
            slot_of.clear();
            ended.clear();
            (
                told,
                news,
                background_activity,
                clipboard_writes,
                gone,
                settled,
                pumped_any,
            )
        };
        // Counted before the notices go out, because telling a window to come
        // is work this round did — a round that had something to show is not
        // quiet.
        let moved = had_events
            || pumped_any
            || !told.is_empty()
            || !news.is_empty()
            || !clipboard_writes.is_empty()
            || !gone.is_empty()
            || !settled.is_empty();
        // Chase rounds are not evidence of quiet: they are 2ms peeks inside
        // what would have been ONE display-rate round, and letting eight of
        // them count would spend most of QUIET_ROUNDS on a single unanswered
        // keystroke.
        quiet = if moved {
            0
        } else if chase > 0 {
            quiet
        } else {
            quiet.saturating_add(1)
        };
        // The chase caught what it was chasing — unless a write still waits
        // for its answer (`ChaseRound::chase_left`).
        chase = chase_round.chase_left(chase, moved);

        for text in clipboard_writes {
            let _ = app.emit_to(
                MAIN_WINDOW_LABEL,
                TERMINAL_CLIPBOARD_WRITE_EVENT,
                ClipboardWritePayload { text },
            );
        }

        // A shell ending is the END of whatever run opened it, and until now
        // the ledger kept only beginnings — a history that could say a job
        // fired and never whether it finished. This is the one moment the
        // window can witness that: the pty is ours, and nothing else reports
        // it. (Orca's own window cannot: the completion its runs are marked
        // with comes from the headless path's `waitForTerminal`, and the
        // interactive launch hands `completion` nothing at all, so a run
        // dispatched into a terminal there stays `dispatched` forever.)
        //
        // Guarded on `gone` rather than run every round: this is the render
        // path, and reading a file sixty times a second to learn that nothing
        // ended would be sixty reads a second. A shell exiting is rare. The
        // write is guarded a second time, on whether any row was actually
        // stamped, because most terminals in this window are somebody's own
        // and closing a tab must not rewrite the history file.
        //
        // Before the window is told, so the repaint that `term:exited` sets
        // off reads a ledger that already knows how the run finished.
        for (term, code, _) in &gone {
            crumbs::record("pane", format_args!("exit term={term} code={code:?}"));
        }
        if !gone.is_empty() {
            let ended_at = now_epoch_ms();
            for (term, code, _) in &gone {
                note_window_event(
                    &local_data_root,
                    &format!("term {term} ended code {code:?}"),
                );
            }
            let _store = automation_store_domain();
            let mut history = stored_automation_runs(&local_data_root);
            let mut stamped = false;
            for (term, code, _) in &gone {
                let launch = crate::cmd::terminal::launch_of(&state, *term);
                stamped |= mark_ended(&mut history, *term, launch, ended_at, *code);
            }
            if stamped {
                let forgotten = prune_final_runs(&mut history, RUNS_KEPT_PER_AUTOMATION);
                if write_automation_runs(&local_data_root, &history).is_ok() {
                    forget_run_evidence(&local_data_root, &forgotten);
                }
            }
        }

        // The shells nobody is reading, noted once each for the black box:
        // every grid still parsed every byte, so a reveal is owed a snapshot of
        // the exact current screen, and nothing about them crosses but news.
        let newly_background = unwatched_noted()
            .lock()
            .map(|mut noted| {
                noted.retain(|term| !readers.contains_key(term));
                background_activity
                    .into_iter()
                    .filter(|term| noted.insert(*term))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for term in newly_background {
            note_window_event(
                &local_data_root,
                &format!("output for term {term} stops at the pump (unwatched)"),
            );
        }
        for (term, said) in news {
            announce_term_news(app, term, said);
        }
        // One notice a window, however many of its shells have frames: its
        // pull answers for all of them at once.
        let mut told = told;
        told.sort_unstable();
        told.dedup();
        for label in told {
            let _ = app.emit(&dirty_event_for(&label), ());
        }

        // The second tier: the tail of every shell the board is showing.
        //
        // "지금 에이전트가 떠도 화면을 볼수없음" was the gate above, measured:
        // four agents alive and the black box saying `watch main: [3]`,
        // `[32]`, `[33]`, `[2]` — ONE shell declared at a time, because the
        // stage unhides one tab's panes and that is all "reading" has ever
        // meant. Every other agent's frames stopped at that gate. The full
        // tier still cannot be handed to eight cards at display rate; a few
        // rows four times a second can.
        //
        // Outside the notices on purpose, and this is the half a version
        // inside the old frame loop got wrong: a board is mostly agents that
        // are NOT printing — the idle column, the ones waiting on a person —
        // and a shell with no frame this round would never reach a branch
        // nested in them. Those cards would stand empty forever, which is the
        // very complaint this tier answers.
        //
        // A SNAPSHOT off the grid, never a delta. A delta is cleared as it is
        // taken, so a throttle that dropped deltas would leave a card
        // permanently missing whatever it declined to send; the grid still
        // holds the whole truth and this reads the last few rows of it.
        let previewing = previewed_anywhere(&app.state::<AppState>().previewed_terms());
        if !previewing.is_empty() {
            let now = epoch_ms_now();
            for term in previewing {
                // The full tier wins. A screen taking a delta and a snapshot
                // by turns tears between the two pictures. (The readers this
                // round already knows ARE the full tier — no second lock.)
                if readers.contains_key(&term) {
                    continue;
                }
                let due = previewed_last_sent()
                    .lock()
                    .map(|sent| {
                        sent.get(&term)
                            .is_none_or(|at| now.saturating_sub(*at) >= TERM_PREVIEW_INTERVAL_MS)
                    })
                    .unwrap_or(false);
                if !due {
                    continue;
                }
                let rows = app.state::<AppState>().terminals().handle(term).map(|pty| {
                    lock_pty(&pty)
                        .terminal()
                        .grid()
                        .preview_rows(TERM_PREVIEW_ROWS)
                });
                if let Some(rows) = rows {
                    previewed_last_sent()
                        .lock()
                        .map(|mut sent| sent.insert(term, now))
                        .ok();
                    let _ = app.emit("term:preview", TermPreview { term, rows });
                }
            }
        }
        for (term, _, screen) in gone {
            // The shell ended on its own, so everything held about it is now
            // about nothing. This is the road the window's own close does NOT
            // take — `dropTab` deliberately leaves ending the process to
            // `closeTab`, so a person typing `exit`, an agent finishing its
            // session or a child that crashed never reached `close_term` at
            // all, and eight maps kept their entry for the life of the
            // window. Forgotten BEFORE the event goes out, so a surface that
            // repaints on hearing it reads the state this shell is actually
            // absent from.
            forget_term_state(
                &app.state::<AppState>(),
                term,
                TermLedgerSettlement::Recorded(Some(screen)),
            );
            let _ = app.emit("term:exited", TermGone { term });
        }
        // What the forget door owed the window for those endings — and for
        // the ones the commands settled off this clock (t-3098).
        publish_owed_rosters(app);
        // And the ending nothing reports: an agent that left a terminal its
        // shell outlived. Asked on a gate of its own rather than on `quiet`,
        // because a pane with a lying badge is exactly a pane where nothing is
        // moving — hanging this on the busy cadence would answer only for the
        // agents that are still working.
        //
        // AFTER the reaper above, so a shell that ended this very round has
        // already been forgotten and is not asked about a terminal that is no
        // longer in the pool.
        let now = Instant::now();
        if looked.is_none_or(|last| now.duration_since(last) >= FOREGROUND_LOOK_EVERY) {
            looked = Some(now);
            sweep_departed_agents(app);
            // And the same look the other way: an agent that ARRIVED in a
            // pane nobody launched one in. One gate, two questions, so the
            // kernel is asked about a pane once per turn of this clock.
            sweep_arrived_agents(app);
        }
        // Said out loud, both ways. A prompt that was never delivered is the
        // case a caller most needs to hear about — the agent is sitting there
        // with nothing to do and no error anywhere to explain why.
        for (term, outcome, text) in settled {
            let _ = app.emit(
                "term:prompt",
                PromptSettled {
                    term,
                    delivered: outcome == DeliveryOutcome::Delivered,
                    pasted: outcome.pasted(),
                    why: prompt_transaction::withheld(outcome).map(|why| why.says()),
                    text,
                },
            );
        }

        // A chase look while a wake stands unanswered, display rate while
        // anything is moving, a slow poll once everything has stopped — and
        // interruptible any way, so a keystroke does not wait out the nap it
        // arrived in.
        let period = if chase > 0 {
            CHASE_INTERVAL
        } else if quiet >= QUIET_ROUNDS {
            IDLE_INTERVAL
        } else {
            PUMP_INTERVAL
        };
        /* Sleep the REST of the beat, not the whole beat again.
         *
         * A flat nap makes the round's period `work + beat`: a round that spent
         * 8 ms draining and 1 ms emitting then slept 16 ms, so the display beat
         * a person actually got was 25 ms — 40 Hz, not 62.5 — and it moved with
         * the load, which is what a hand reads as uneven rather than slow. The
         * beat is a PERIOD, so the nap is what is left of it.
         *
         * Zero when the round already overran, and that is the right answer:
         * the round itself is then the pacing, and there is nothing to wait for.
         * The chase and idle beats are subtracted the same way on purpose — a
         * chase look is 2 ms *from the top of the round*, and a round that took
         * longer than that has already spent the look it was going to make. */
        let hurried = app
            .state::<AppState>()
            .cadence()
            .rest(period.saturating_sub(round_began.elapsed()));
        // Somebody sent something to a child. The wake is raised before the
        // write lands, so the round about to run may still see nothing —
        // counting it as quiet would drop the loop back to the idle interval
        // with the wake already spent, and the echo would arrive into a nap.
        // The chase is armed fresh here for the same reason: the write this
        // wake announces has not been read yet, and the next few looks are
        // the ones that catch it.
        if hurried {
            quiet = 0;
            chase = CHASE_ROUNDS;
        } else {
            // Saturating on purpose: the chase counts down to zero and stays
            // there until the next wake re-arms it.
            chase = chase.saturating_sub(1);
        }
    }
}

/// What the staged session runs on — model and permission mode — straight
/// from `session.info`, defensively picked apart.
#[derive(Serialize)]
pub(super) struct SessionInfo {
    pub(super) model: Option<String>,
    pub(super) permission_mode: Option<String>,
}

/// One row of the source-control panel.
///
/// `staged` and `changed` are decided here rather than in the webview because
/// which column a porcelain letter sits in is a **git fact**, not a rendering
/// choice — ` M` is an edit in the working tree and `M ` the same edit already
/// in the index. A file can be both at once (`MM`), which is exactly why the
/// panel needs two groups and why one row can appear in each.
#[derive(Serialize, Debug, PartialEq)]
pub(super) struct ScmEntry {
    pub(super) path: String,
    /// Both columns, verbatim, for the badge to show.
    pub(super) code: String,
    pub(super) staged: bool,
    pub(super) changed: bool,
    /// Where a rename came from, so a row can name both ends.
    pub(super) origin: Option<String>,
    /// `Both modified`, `Deleted by us`, … for an unmerged path, and nothing
    /// for every other row. The word matters more than the badge does: `DU`
    /// and `UD` are both "conflict" and the work they need is opposite.
    pub(super) conflict: Option<&'static str>,
    /// Lines added and removed against `HEAD` — Orca's `DiffLineCounts`
    /// beside the status letter (SourceControl-xYgEJ0Pk.js:512811). `None`
    /// where numstat has no number: a binary file, an untracked file the
    /// diff has never seen.
    pub(super) added: Option<u64>,
    pub(super) removed: Option<u64>,
    /// Set when this row is a SUBMODULE. `stage_inside` is the one fact the
    /// panel acts on: the parent repository can stage a moved commit pointer
    /// and **cannot** stage file changes living inside the submodule's own
    /// worktree, so a `+` offered there is a button that does nothing
    /// (Orca's `isSubmoduleWorktreeOnlyChange`,
    /// `discard-all-sequence.ts:50-55`).
    pub(super) submodule: Option<ScmSubmodule>,
}

/// The submodule facts a row wears.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) struct ScmSubmodule {
    pub(super) commit_changed: bool,
    pub(super) tracked_changes: bool,
    pub(super) untracked_changes: bool,
    /// Unstaged, and the commit pointer has NOT moved — so everything dirty
    /// about it lives inside, where `git add <submodule>` cannot reach.
    pub(super) stage_inside: bool,
}

impl ScmSubmodule {
    pub(super) fn of(found: zerocode_orchestrator::Submodule, staged: bool) -> Self {
        Self {
            commit_changed: found.commit_changed,
            tracked_changes: found.tracked_changes,
            untracked_changes: found.untracked_changes,
            // 원본은 `area === 'unstaged'`로 묻는다. 우리 행은 두 칸을 한 줄에
            // 들고 다니므로 같은 질문의 우리 철자는 "스테이지된 반쪽이 아니다"다.
            stage_inside: !staged && !found.commit_changed,
        }
    }
}
