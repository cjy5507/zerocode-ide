//! Multi-client **pair sessions** for `zo serve` (track 5): spectators watch
//! a live session and a helm steers it, in real time, over the same socket
//! server the single-client `zo attach` already uses.
//!
//! ## The one shared structure: [`PairHub`]
//!
//! A `zo serve` process owns exactly one [`PairHub`] (an `Arc` clone handed
//! to every connection task). It holds, behind a single **std** `Mutex`:
//!
//! - a per-session **frame channel** ([`SessionChannel`]) with a monotonic
//!   `frame_seq`, a manually-managed `Vec<Subscriber>`, a cached history
//!   snapshot, and the in-flight turn's helm/steering handle;
//! - a per-connection **peer** registry for the roster.
//!
//! The lock is held only for synchronous work (serialize a frame, `try_send` to
//! each subscriber, mutate the registry) — **never across `.await`** — so it is
//! a plain `std::sync::Mutex`, poison-tolerant like the rest of `serve`.
//!
//! ## Why a manual registry, not `tokio::sync::broadcast`
//!
//! A broadcast channel forces one lag policy on everyone. Pair sessions need
//! two: a **spectator** that falls behind is dropped-and-resynced (its slowness
//! must never touch the turn), while the **helm**'s own turn stream stays
//! lossless (it drives the turn's pace, exactly as the single-client path does
//! today). Manual `Vec<Subscriber>` + `try_send` lets each subscriber carry its
//! own `lagged` flag and a private resync affordance.
//!
//! ## Backpressure isolation (the load-bearing invariant)
//!
//! Frame fan-out to spectators uses `try_send`. A full spectator channel drops
//! the frame and marks the subscriber `lagged`; the next time it has room the
//! hub hands it a single `{"type":"resync","next_seq":N}` control frame so the
//! client re-requests a snapshot. The turn future and the helm's stream are on
//! entirely different code paths and are **never** blocked by a slow spectator.
//!
//! ## Subscribe atomicity
//!
//! `subscribe`, `begin_turn`, `end_turn`, `broadcast`, and roster fan-out all
//! take the *same* lock, so a subscribe reads `(snapshot, next_seq, helm)` and
//! registers itself in one critical section. Any frame emitted after it joined
//! carries `frame_seq >= next_seq`; any state folded into `snapshot` predates
//! it. A client that joins mid-turn resumes exactly at `next_seq` with no gap or
//! duplicate.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::mpsc;

use runtime::message_stream::RenderBlock;
use runtime::SteeringQueue;

use crate::serve_auth::ServeCapability;
use crate::serve_protocol::{HistoryEntry, RpcResponse, SubscribeResult};

/// Per-connection outbound line channel capacity. Every socket write — RPC
/// responses, the helm's own turn frames, and fanned-out spectator frames —
/// funnels through one such channel drained by a single writer task, so writes
/// never interleave. 256 is the spectator buffer from the design (§4.3): a
/// spectator this far behind is dropped-and-resynced rather than throttling the
/// turn.
pub(crate) const OUT_CHANNEL_CAP: usize = 256;

static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_ANON: AtomicU64 = AtomicU64::new(1);
static NEXT_TURN_ID: AtomicU64 = AtomicU64::new(1);

/// Allocate a process-unique connection id.
pub(crate) fn next_conn_id() -> u64 {
    NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed)
}

/// Allocate a server-issued turn id, used when the client did not supply one.
pub(crate) fn next_turn_id() -> u64 {
    NEXT_TURN_ID.fetch_add(1, Ordering::Relaxed)
}

/// Default peer label when the connection carries no distinguishing token label
/// (`anon-N`, §4.4).
pub(crate) fn next_anon_label() -> String {
    format!("anon-{}", NEXT_ANON.fetch_add(1, Ordering::Relaxed))
}

/// Fan-out state for a subscriber. A first overflow permanently moves its
/// slot to `ResyncPending`; only a fresh boundary subscribe creates a Live slot.
enum SubscriberState {
    Live,
    ResyncPending,
}

/// One spectator's slot in a session's fan-out registry.
struct Subscriber {
    conn_id: u64,
    tx: mpsc::Sender<Arc<str>>,
    subscription_id: u64,
    /// New attach clients opt into the sticky marker protocol. Legacy clients
    /// keep the original retry-on-next-frame lag behavior.
    resync_v2: bool,
    /// Legacy-only, non-sticky lag state.
    lagged: bool,
    state: SubscriberState,
    /// Deferred marker delivery never blocks broadcast. It is aborted whenever
    /// the slot is replaced or removed; an already-delivered stale marker is
    /// harmless because the client compares `subscription_id`.
    resync_task: Option<tokio::task::JoinHandle<()>>,
}

/// The in-flight turn's ownership + steering handle. Present only while a turn
/// runs; `end_turn` clears it.
struct ActiveTurn {
    turn_id: u64,
    owner_conn_id: u64,
    owner_label: String,
    steering: SteeringQueue,
}

/// Everything the hub tracks for one session id.
struct SessionChannel {
    /// Monotonic, per-session frame counter stamped onto every render frame.
    next_seq: u64,
    subscribers: Vec<Subscriber>,
    /// History projected at the last turn boundary, replayed to a joining
    /// spectator so a mid-turn subscribe has a coherent base to stream onto.
    snapshot: Vec<HistoryEntry>,
    turn: Option<ActiveTurn>,
    /// Monotone subscription identity. Replacing a slot produces a distinct
    /// id, allowing clients to discard stale marker delivery races. `None`
    /// means every u64 identity has been issued and new subscriptions refuse
    /// rather than reusing an identity.
    next_subscription_id: Option<u64>,
}

impl Default for SessionChannel {
    fn default() -> Self {
        Self {
            next_seq: 0,
            subscribers: Vec::new(),
            snapshot: Vec::new(),
            turn: None,
            // Zero is reserved as the attach client's legacy-server sentinel.
            next_subscription_id: Some(1),
        }
    }
}

/// One connected client, for the roster.
struct Peer {
    conn_id: u64,
    label: String,
    capability: ServeCapability,
    /// Session ids this connection is subscribed to (spectating).
    subscribed: Vec<String>,
}

#[derive(Default)]
struct PairState {
    channels: HashMap<String, SessionChannel>,
    peers: HashMap<u64, Peer>,
}

/// The single shared pair-session hub for a `zo serve` process.
#[derive(Clone, Default)]
pub(crate) struct PairHub {
    inner: Arc<Mutex<PairState>>,
}

/// Result of `session.subscribe`: the atomic `(snapshot, next_seq, helm)` read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SubscribeError {
    SubscriptionIdExhausted,
}

pub(crate) struct SubscribeOutcome {
    pub(crate) snapshot: Vec<HistoryEntry>,
    pub(crate) next_seq: u64,
    pub(crate) helm: Option<String>,
    pub(crate) subscription_id: u64,
}

/// Verdict for a `session.steer` request (helm-ownership check).
pub(crate) enum SteerAuth {
    /// The requester may steer; push onto this queue.
    Allowed(SteeringQueue),
    /// No turn is in flight for this session — nothing to steer.
    NoActiveTurn,
    /// A turn is in flight but under a different `turn_id` than the one the
    /// client named (a stale steer for a turn that already ended).
    TurnMismatch,
}

impl PairHub {
    fn lock(&self) -> std::sync::MutexGuard<'_, PairState> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Ensure an authenticated connection appears in rosters at least once.
    /// Existing peers retain their original label and can only be promoted
    /// (`Read` → `Full`), never downgraded by a later read-only request.
    pub(crate) fn ensure_peer(&self, conn_id: u64, label: String, capability: ServeCapability) {
        let mut state = self.lock();
        let changed = match state.peers.get_mut(&conn_id) {
            Some(peer) if capability_rank(capability) > capability_rank(peer.capability) => {
                peer.capability = capability;
                true
            }
            Some(_) => false,
            None => {
                state.peers.insert(
                    conn_id,
                    Peer {
                        conn_id,
                        label,
                        capability,
                        subscribed: Vec::new(),
                    },
                );
                true
            }
        };
        if changed {
            // Every session roster contains every connected peer, including
            // sessions this peer does not itself watch.
            let session_ids: Vec<String> = state.channels.keys().cloned().collect();
            for session_id in session_ids {
                fanout_roster_locked(&mut state, &session_id);
            }
        }
    }

    /// Drop a disconnecting connection: remove its subscriptions from every
    /// channel and its peer entry, then refresh every active roster. Returns
    /// nothing — cancellation of any turn it owned is handled by the turn driver
    /// (the render channel closes on disconnect).
    pub(crate) fn remove_peer(&self, conn_id: u64) {
        let mut state = self.lock();
        state.peers.remove(&conn_id);
        for channel in state.channels.values_mut() {
            retain_dropping_conn(&mut channel.subscribers, conn_id);
        }
        // Like capability upgrades and registration, removal changes the
        // roster of every active session, not only sessions this peer watched.
        let session_ids: Vec<String> = state.channels.keys().cloned().collect();
        for session_id in session_ids {
            fanout_roster_locked(&mut state, &session_id);
        }
    }

    /// Queue the subscribe response and activate the subscriber under one
    /// `PairHub` lock. The caller reserves `permit` before entering this method;
    /// therefore `permit.send` cannot await and the response is ordered before
    /// any subsequently fanned frame on the same connection mpsc.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn subscribe_with_permit(
        &self,
        out_tx: mpsc::Sender<Arc<str>>,
        permit: mpsc::Permit<'_, Arc<str>>,
        session_id: &str,
        conn_id: u64,
        snapshot: Option<Vec<HistoryEntry>>,
        request_id: u64,
        resync_v2: bool,
    ) -> Result<SubscribeOutcome, SubscribeError> {
        let mut state = self.lock();
        let channel = state.channels.entry(session_id.to_string()).or_default();
        if let Some(snapshot) = snapshot {
            channel.snapshot = snapshot;
        }
        let subscription_id = allocate_subscription_id(channel)
            .ok_or(SubscribeError::SubscriptionIdExhausted)?;
        let outcome = SubscribeOutcome {
            snapshot: channel.snapshot.clone(),
            next_seq: channel.next_seq,
            helm: channel.turn.as_ref().map(|turn| turn.owner_label.clone()),
            subscription_id,
        };
        let result = SubscribeResult {
            id: session_id.to_string(),
            history: outcome.snapshot.clone(),
            next_seq: outcome.next_seq,
            helm: outcome.helm.clone(),
            floor: Some(outcome.next_seq),
            subscription_id: Some(outcome.subscription_id),
        };
        // A serializer failure is impossible for this value; retain a valid RPC
        // error line rather than permit a frame to overtake a malformed ACK.
        let line = response_line(&RpcResponse::ok(
            request_id,
            serde_json::to_value(result).unwrap_or(serde_json::Value::Null),
        ));
        permit.send(line);
        push_subscriber(channel, conn_id, out_tx, resync_v2, subscription_id);
        if let Some(peer) = state.peers.get_mut(&conn_id) {
            if !peer.subscribed.iter().any(|s| s == session_id) {
                peer.subscribed.push(session_id.to_string());
            }
        }
        fanout_roster_locked(&mut state, session_id);
        Ok(outcome)
    }

    #[cfg(test)]
    fn subscribe(&self, session_id: &str, conn_id: u64, tx: mpsc::Sender<Arc<str>>) -> SubscribeOutcome {
        let sender = tx.clone();
        let permit = sender.try_reserve().expect("test outbound capacity");
        self.subscribe_with_permit(tx, permit, session_id, conn_id, None, 0, true)
            .expect("test identity space")
    }

    #[cfg(test)]
    fn subscribe_boundary(
        &self,
        session_id: &str,
        conn_id: u64,
        tx: mpsc::Sender<Arc<str>>,
        snapshot: Vec<HistoryEntry>,
    ) -> SubscribeOutcome {
        let sender = tx.clone();
        let permit = sender.try_reserve().expect("test outbound capacity");
        self.subscribe_with_permit(tx, permit, session_id, conn_id, Some(snapshot), 0, true)
            .expect("test identity space")
    }

    #[cfg(test)]
    fn subscribe_legacy(&self, session_id: &str, conn_id: u64, tx: mpsc::Sender<Arc<str>>) {
        let mut state = self.lock();
        let channel = state.channels.entry(session_id.to_string()).or_default();
        let subscription_id = allocate_subscription_id(channel).expect("test identity space");
        push_subscriber(channel, conn_id, tx, false, subscription_id);
        if let Some(peer) = state.peers.get_mut(&conn_id) {
            if !peer.subscribed.iter().any(|s| s == session_id) {
                peer.subscribed.push(session_id.to_string());
            }
        }
        fanout_roster_locked(&mut state, session_id);
    }

    /// Remove a spectator (explicit `session.unsubscribe`). Idempotent.
    pub(crate) fn unsubscribe(&self, session_id: &str, conn_id: u64) {
        let mut state = self.lock();
        if let Some(channel) = state.channels.get_mut(session_id) {
            retain_dropping_conn(&mut channel.subscribers, conn_id);
        }
        if let Some(peer) = state.peers.get_mut(&conn_id) {
            peer.subscribed.retain(|s| s != session_id);
        }
        fanout_roster_locked(&mut state, session_id);
    }

    /// Mark a turn as in flight, recording its helm + steering handle and the
    /// pre-turn history a mid-turn joiner should replay. Runs while the caller
    /// holds the session lock, so no concurrent turn can race this.
    pub(crate) fn begin_turn(
        &self,
        session_id: &str,
        turn_id: u64,
        owner_conn_id: u64,
        owner_label: String,
        steering: SteeringQueue,
        snapshot: Vec<HistoryEntry>,
    ) {
        let mut state = self.lock();
        let channel = state.channels.entry(session_id.to_string()).or_default();
        channel.snapshot = snapshot;
        channel.turn = Some(ActiveTurn {
            turn_id,
            owner_conn_id,
            owner_label,
            steering,
        });
        fanout_roster_locked(&mut state, session_id);
    }

    /// Clear the in-flight turn (only if `turn_id` still matches, so a late
    /// `end_turn` never stomps a newer turn) and fold the post-turn history into
    /// the cached snapshot for the next joiner.
    pub(crate) fn end_turn(&self, session_id: &str, turn_id: u64, snapshot: Vec<HistoryEntry>) {
        let mut state = self.lock();
        if let Some(channel) = state.channels.get_mut(session_id) {
            if channel.turn.as_ref().is_some_and(|turn| turn.turn_id == turn_id) {
                channel.turn = None;
            }
            channel.snapshot = snapshot;
        }
        fanout_roster_locked(&mut state, session_id);
    }

    /// Fan one render frame out to every spectator of `session_id`, stamping a
    /// monotonic `frame_seq`. A spectator whose channel is full is dropped and
    /// marked `lagged`; a spectator that was lagged and now has room is handed a
    /// single `resync` control frame instead of the live frame. The turn and the
    /// helm stream are untouched — this is the backpressure firewall.
    pub(crate) fn broadcast(&self, session_id: &str, block: &RenderBlock) {
        let Some(mut value) = super::render_block_json(block) else {
            return;
        };
        let mut state = self.lock();
        let Some(channel) = state.channels.get_mut(session_id) else {
            return;
        };
        let seq = channel.next_seq;
        channel.next_seq = channel.next_seq.saturating_add(1);
        if let Some(object) = value.as_object_mut() {
            object.insert("frame_seq".to_string(), serde_json::json!(seq));
        }
        let line = frame_line(&value);
        let next_seq = channel.next_seq;
        let helm_conn = channel.turn.as_ref().map(|turn| turn.owner_conn_id);
        channel.subscribers.retain_mut(|sub| {
            if sub.tx.is_closed() {
                abort_resync_task(sub);
                false
            } else {
                true
            }
        });
        for sub in &mut channel.subscribers {
            if Some(sub.conn_id) == helm_conn {
                continue;
            }
            if sub.resync_v2 {
                if matches!(sub.state, SubscriberState::ResyncPending) {
                    continue;
                }
                match sub.tx.try_send(line.clone()) {
                    Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        sub.state = SubscriberState::ResyncPending;
                        sub.resync_task = Some(spawn_resync_delivery(
                            sub.tx.clone(), sub.subscription_id, next_seq,
                        ));
                    }
                }
            } else {
                // Exact pre-v2 recovery: a lagged subscriber retries a single
                // resync on each later frame and immediately becomes Live once
                // it lands. It can never become permanently ResyncPending.
                if sub.lagged {
                    if try_send_resync(&sub.tx, next_seq) {
                        sub.lagged = false;
                    }
                    continue;
                }
                if matches!(sub.tx.try_send(line.clone()), Err(mpsc::error::TrySendError::Full(_))) {
                    sub.lagged = true;
                }
            }
        }
    }

    /// Authorize a `session.steer` and hand back the steering queue to push
    /// onto. The capability gate already rejected non-`Full` connections; this
    /// enforces the turn-scoped rule that there must be an in-flight turn to
    /// steer (and, if the client named a `turn_id`, that it is the live one).
    pub(crate) fn steering_for(&self, session_id: &str, turn_id: Option<u64>) -> SteerAuth {
        let state = self.lock();
        let Some(turn) = state.channels.get(session_id).and_then(|c| c.turn.as_ref()) else {
            return SteerAuth::NoActiveTurn;
        };
        if let Some(requested) = turn_id {
            if requested != turn.turn_id {
                return SteerAuth::TurnMismatch;
            }
        }
        SteerAuth::Allowed(Arc::clone(&turn.steering))
    }

    /// Remove a closed session's channel, clear the session from every peer,
    /// abort pending marker tasks, and refresh all rosters those peers still
    /// watch. Called while the exact `LiveCli` guard remains held by close.
    pub(crate) fn close_session(&self, session_id: &str) {
        let mut state = self.lock();
        let mut affected = std::collections::HashSet::new();
        for peer in state.peers.values_mut() {
            if peer.subscribed.iter().any(|id| id == session_id) {
                peer.subscribed.retain(|id| id != session_id);
                affected.extend(peer.subscribed.iter().cloned());
            }
        }
        if let Some(mut channel) = state.channels.remove(session_id) {
            for subscriber in &mut channel.subscribers {
                abort_resync_task(subscriber);
            }
        }
        for watched in affected {
            fanout_roster_locked(&mut state, &watched);
        }
    }

    /// The roster of a session: the helm label (if a turn is running) and every
    /// connected peer, tagged with whether it is watching this session.
    pub(crate) fn roster(&self, session_id: &str) -> serde_json::Value {
        let state = self.lock();
        roster_value(&state, session_id)
    }

    /// This connection's roster label (`anon-N`). Falls back to `helm` if the
    /// peer entry was somehow already reaped.
    pub(crate) fn peer_label(&self, conn_id: u64) -> String {
        self.lock()
            .peers
            .get(&conn_id)
            .map_or_else(|| "helm".to_string(), |peer| peer.label.clone())
    }

    /// Count of spectators currently watching `session_id` (for the host HUD).
    pub(crate) fn viewer_count(&self, session_id: &str) -> usize {
        self.lock()
            .channels
            .get(session_id)
            .map_or(0, |channel| channel.subscribers.len())
    }

    /// Current next render-frame sequence for a session. This is surfaced at a
    /// completed `run_turn` boundary so clients can discard pre-ack duplicates
    /// while retaining frames emitted after their boundary subscribe.
    pub(crate) fn next_seq(&self, session_id: &str) -> u64 {
        self.lock()
            .channels
            .get(session_id)
            .map_or(0, |channel| channel.next_seq)
    }
}

/// Abort deferred marker delivery before dropping a subscriber slot.
fn abort_resync_task(subscriber: &mut Subscriber) {
    if let Some(task) = subscriber.resync_task.take() {
        task.abort();
    }
}

/// Remove every slot owned by `conn_id`, aborting its deferred work first.
fn retain_dropping_conn(subscribers: &mut Vec<Subscriber>, conn_id: u64) {
    subscribers.retain_mut(|sub| {
        if sub.conn_id == conn_id {
            abort_resync_task(sub);
            false
        } else {
            true
        }
    });
}

/// Replace an older connection slot with a fresh Live subscription identity.
fn allocate_subscription_id(channel: &mut SessionChannel) -> Option<u64> {
    let subscription_id = channel.next_subscription_id?;
    channel.next_subscription_id = subscription_id.checked_add(1);
    Some(subscription_id)
}

fn push_subscriber(
    channel: &mut SessionChannel,
    conn_id: u64,
    tx: mpsc::Sender<Arc<str>>,
    resync_v2: bool,
    subscription_id: u64,
) {
    retain_dropping_conn(&mut channel.subscribers, conn_id);
    channel.subscribers.push(Subscriber {
        conn_id,
        tx,
        subscription_id,
        resync_v2,
        lagged: false,
        state: SubscriberState::Live,
        resync_task: None,
    });
}

/// Serialize a seq-stamped frame value into one `\n`-terminated wire line.
fn frame_line(value: &serde_json::Value) -> Arc<str> {
    let mut line = value.to_string();
    line.push('\n');
    Arc::from(line)
}

/// Wait for room to deliver one marker. This task never changes subscriber
/// state: `ResyncPending` remains until boundary subscribe replaces the slot.
fn spawn_resync_delivery(
    tx: mpsc::Sender<Arc<str>>,
    subscription_id: u64,
    next_seq: u64,
) -> tokio::task::JoinHandle<()> {
    let line = frame_line(&serde_json::json!({
        "type": "marker",
        "subscription_id": subscription_id,
        "next_seq": next_seq,
    }));
    tokio::spawn(async move {
        if let Ok(permit) = tx.reserve().await {
            permit.send(line);
        }
    })
}

/// Legacy non-sticky resync marker (exactly the pre-v2 wire shape).
fn try_send_resync(tx: &mpsc::Sender<Arc<str>>, next_seq: u64) -> bool {
    tx.try_send(frame_line(&serde_json::json!({ "type": "resync", "next_seq": next_seq })))
        .is_ok()
}

/// Serialize a response for direct enqueue through a reserved sender permit.
fn response_line(response: &RpcResponse) -> Arc<str> {
    let mut line = serde_json::to_string(&response).unwrap_or_else(|_| {
        r#"{"jsonrpc":"2.0","id":0,"error":{"code":-32603,"message":"serialize"}}"#.to_string()
    });
    line.push('\n');
    Arc::from(line)
}

/// Build the roster control frame for one session.
fn roster_value(state: &PairState, session_id: &str) -> serde_json::Value {
    let channel = state.channels.get(session_id);
    let helm_conn = channel.and_then(|c| c.turn.as_ref().map(|t| t.owner_conn_id));
    let helm_label = channel.and_then(|c| c.turn.as_ref().map(|t| t.owner_label.clone()));
    let watchers: std::collections::HashSet<u64> = channel
        .map(|c| c.subscribers.iter().map(|s| s.conn_id).collect())
        .unwrap_or_default();
    let mut peers: Vec<serde_json::Value> = state
        .peers
        .values()
        .map(|peer| {
            serde_json::json!({
                "conn_id": peer.conn_id,
                "label": peer.label,
                "capability": capability_label(peer.capability),
                "viewer": watchers.contains(&peer.conn_id),
                "helm": Some(peer.conn_id) == helm_conn,
            })
        })
        .collect();
    peers.sort_by_key(|p| p.get("conn_id").and_then(serde_json::Value::as_u64).unwrap_or(0));
    serde_json::json!({
        "type": "roster",
        "session_id": session_id,
        "helm": helm_label,
        "viewers": watchers.len(),
        "peers": peers,
    })
}

/// Fan the current roster out to every spectator of `session_id`. Best-effort
/// (`try_send`, no lag bookkeeping): the roster is status, not transcript.
fn fanout_roster_locked(state: &mut PairState, session_id: &str) {
    let value = roster_value(state, session_id);
    let Some(channel) = state.channels.get(session_id) else {
        return;
    };
    if channel.subscribers.is_empty() {
        return;
    }
    let line = frame_line(&value);
    for sub in &channel.subscribers {
        let _ = sub.tx.try_send(line.clone());
    }
}

const fn capability_label(capability: ServeCapability) -> &'static str {
    match capability {
        ServeCapability::Read => "read",
        ServeCapability::Full => "full",
    }
}

/// Ordered tier so `ensure_peer` only ever raises a peer's roster
/// capability (Read → Full), never lowers it.
const fn capability_rank(capability: ServeCapability) -> u8 {
    match capability {
        ServeCapability::Read => 0,
        ServeCapability::Full => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::message_stream::{BlockId, RenderBlock};
    use std::sync::Mutex as StdMutex;

    fn text_block(text: &str) -> RenderBlock {
        RenderBlock::TextDelta {
            id: BlockId(1),
            text: text.to_string(),
            done: false,
        }
    }

    fn steering() -> SteeringQueue {
        Arc::new(StdMutex::new(Vec::new()))
    }

    #[test]
    fn response_line_is_valid_json_rpc_error() {
        let line = response_line(&RpcResponse::err(9, -32603, "serialize"));
        let value: serde_json::Value = serde_json::from_str(line.trim_end()).expect("valid JSON");
        assert_eq!(value["error"]["code"], -32603);
    }

    /// A fanned frame carries a monotonically increasing `frame_seq` and reaches
    /// every subscriber identically.
    #[tokio::test]
    async fn broadcast_stamps_seq_and_reaches_all_subscribers() {
        let hub = PairHub::default();
        hub.ensure_peer(1, "anon-1".to_string(), ServeCapability::Read);
        hub.ensure_peer(2, "anon-2".to_string(), ServeCapability::Read);
        let (tx1, mut rx1) = mpsc::channel::<Arc<str>>(OUT_CHANNEL_CAP);
        let (tx2, mut rx2) = mpsc::channel::<Arc<str>>(OUT_CHANNEL_CAP);
        let out1 = hub.subscribe("s", 1, tx1);
        let out2 = hub.subscribe("s", 2, tx2);
        assert_eq!(out1.next_seq, 0);
        assert_eq!(out2.next_seq, 0);
        // Drain the roster frames the subscribes fanned out.
        drain_control(&mut rx1);
        drain_control(&mut rx2);

        hub.broadcast("s", &text_block("a"));
        hub.broadcast("s", &text_block("b"));

        let a1 = next_render(&mut rx1);
        let a2 = next_render(&mut rx2);
        assert_eq!(a1["frame_seq"], 0);
        assert_eq!(a2["frame_seq"], 0);
        assert_eq!(a1["text"], "a");
        let b1 = next_render(&mut rx1);
        assert_eq!(b1["frame_seq"], 1);
        assert_eq!(b1["text"], "b");
        assert_eq!(hub.next_seq("s"), 2);
    }

    /// A slow spectator that overflows is dropped-and-marked-lagged, then handed
    /// exactly one resync marker when room reappears — the fast path is
    /// unaffected.
    #[tokio::test]
    async fn slow_spectator_lags_then_resyncs() {
        let hub = PairHub::default();
        hub.ensure_peer(1, "anon-1".to_string(), ServeCapability::Read);
        // Capacity-1 channel so the second frame overflows.
        let (tx, mut rx) = mpsc::channel::<Arc<str>>(1);
        hub.subscribe("s", 1, tx);
        // The subscribe fanned a roster frame that already fills the cap-1
        // channel; clear it so we can observe the render-frame overflow cleanly.
        let _ = rx.try_recv();

        hub.broadcast("s", &text_block("first")); // fills the channel
        hub.broadcast("s", &text_block("second")); // overflows → lagged

        let first = next_render(&mut rx);
        assert_eq!(first["text"], "first");
        // The first dropped frame schedules its recovery immediately. Draining
        // lets the background task reserve capacity and deliver the marker; no
        // later broadcast is required (important for a final-frame overflow).
        let control = recv_value_async(&mut rx).await;
        assert_eq!(control["type"], "marker");
        assert!(control["subscription_id"].is_u64());
    }

    /// A spectator that is *still* full when its resync marker is due is handed
    /// the marker by a background task once room reappears — the broadcaster
    /// never blocks, and the lag state returns to `LIVE`.
    #[tokio::test]
    async fn lagged_spectator_gets_resync_via_spawned_task_when_still_full() {
        let hub = PairHub::default();
        hub.ensure_peer(1, "anon-1".to_string(), ServeCapability::Read);
        // Capacity-1 channel: one buffered frame keeps it full across the resync.
        let (tx, mut rx) = mpsc::channel::<Arc<str>>(1);
        hub.subscribe("s", 1, tx);
        let _ = rx.try_recv(); // clear the roster frame the subscribe fanned

        hub.broadcast("s", &text_block("first")); // fills the cap-1 channel
        hub.broadcast("s", &text_block("second")); // overflows → LAGGED
                                                    // Still full (holds "first"): the inline resync try_send fails, so the
                                                    // hub hands the marker to a spawned task rather than blocking.
        hub.broadcast("s", &text_block("third"));

        // Drain the one buffered frame; this frees room the spawned task awaits.
        let first = recv_value(&mut rx);
        assert_eq!(first["text"], "first");
        // The background task now delivers exactly the resync marker.
        let control = recv_value_async(&mut rx).await;
        assert_eq!(control["type"], "marker");

        // Pending is sticky: later live frames are dropped until a fresh
        // boundary subscribe allocates a new subscription slot.
        hub.broadcast("s", &text_block("fourth"));
        assert!(rx.try_recv().is_err());
    }

    /// Replacing/removing a slot aborts the deferred marker task, so freeing
    /// the outbound channel afterwards cannot leak a stale marker.
    #[tokio::test]
    async fn unsubscribe_aborts_pending_resync_task() {
        let hub = PairHub::default();
        hub.ensure_peer(1, "anon-1".to_string(), ServeCapability::Read);
        let (tx, mut rx) = mpsc::channel::<Arc<str>>(1);
        hub.subscribe("s", 1, tx);
        let _ = rx.try_recv();
        hub.broadcast("s", &text_block("first"));
        hub.broadcast("s", &text_block("overflow"));
        hub.unsubscribe("s", 1);
        let _ = rx.try_recv();
        tokio::task::yield_now().await;
        assert!(rx.try_recv().is_err(), "aborted task must not emit a marker");
    }

    /// A boundary resubscribe allocates a new Live slot, making stale marker
    /// identities distinguishable at the client.
    #[test]
    fn boundary_resubscribe_returns_fresh_subscription_id() {
        let hub = PairHub::default();
        hub.ensure_peer(1, "anon-1".to_string(), ServeCapability::Read);
        let first = hub.subscribe("s", 1, mpsc::channel::<Arc<str>>(OUT_CHANNEL_CAP).0);
        let second = hub.subscribe_boundary(
            "s",
            1,
            mpsc::channel::<Arc<str>>(OUT_CHANNEL_CAP).0,
            Vec::new(),
        );
        assert_ne!(first.subscription_id, second.subscription_id);
    }

    #[tokio::test]
    async fn subscription_id_exhaustion_refuses_to_reuse_max() {
        let hub = PairHub::default();
        hub.ensure_peer(1, "anon-1".to_string(), ServeCapability::Read);
        {
            let mut state = hub.lock();
            state
                .channels
                .entry("s".to_string())
                .or_default()
                .next_subscription_id = Some(u64::MAX);
        }
        let (tx, mut rx) = mpsc::channel::<Arc<str>>(OUT_CHANNEL_CAP);
        let sender = tx.clone();
        let permit = sender.reserve().await.expect("first permit");
        let first = hub
            .subscribe_with_permit(tx, permit, "s", 1, Some(Vec::new()), 1, true)
            .expect("u64::MAX is issued once");
        assert_eq!(first.subscription_id, u64::MAX);
        let _ = rx.recv().await.expect("first ACK");

        let permit = sender.reserve().await.expect("second permit");
        assert!(matches!(
            hub.subscribe_with_permit(sender.clone(), permit, "s", 1, Some(Vec::new()), 2, true),
            Err(SubscribeError::SubscriptionIdExhausted),
        ));
    }

    /// A connection registered at `Read` that later drives a `Full` request is
    /// promoted in the roster, and a redundant/lower request never downgrades
    /// it.
    #[test]
    fn first_subscription_id_is_at_least_one() {
        let hub = PairHub::default();
        hub.ensure_peer(1, "anon-1".to_string(), ServeCapability::Read);
        let outcome = hub.subscribe("s", 1, mpsc::channel::<Arc<str>>(OUT_CHANNEL_CAP).0);
        assert!(outcome.subscription_id >= 1);
    }

    #[test]
    fn register_peer_fans_roster_to_all_channels() {
        let hub = PairHub::default();
        hub.ensure_peer(1, "watcher".to_string(), ServeCapability::Read);
        let (a_tx, mut a_rx) = mpsc::channel::<Arc<str>>(OUT_CHANNEL_CAP);
        let (b_tx, mut b_rx) = mpsc::channel::<Arc<str>>(OUT_CHANNEL_CAP);
        hub.subscribe("a", 1, a_tx);
        hub.subscribe("b", 1, b_tx);
        drain_control(&mut a_rx);
        drain_control(&mut b_rx);

        hub.ensure_peer(2, "arriving".to_string(), ServeCapability::Read);
        for rx in [&mut a_rx, &mut b_rx] {
            let roster = recv_value(rx);
            assert_eq!(roster["type"], "roster");
            assert!(roster["peers"].as_array().unwrap().iter().any(|peer| peer["conn_id"] == 2));
        }
    }

    #[test]
    fn remove_peer_fans_roster_to_all_channels() {
        let hub = PairHub::default();
        hub.ensure_peer(1, "watcher".to_string(), ServeCapability::Read);
        let (a_tx, mut a_rx) = mpsc::channel::<Arc<str>>(OUT_CHANNEL_CAP);
        let (b_tx, mut b_rx) = mpsc::channel::<Arc<str>>(OUT_CHANNEL_CAP);
        hub.subscribe("a", 1, a_tx);
        hub.subscribe("b", 1, b_tx);
        drain_control(&mut a_rx);
        drain_control(&mut b_rx);
        hub.ensure_peer(2, "leaving".to_string(), ServeCapability::Read);
        drain_control(&mut a_rx);
        drain_control(&mut b_rx);

        hub.remove_peer(2);
        for rx in [&mut a_rx, &mut b_rx] {
            let roster = recv_value(rx);
            assert_eq!(roster["type"], "roster");
            assert!(!roster["peers"].as_array().unwrap().iter().any(|peer| peer["conn_id"] == 2));
        }
    }

    #[test]
    fn upgrade_peer_capability_promotes_roster_entry_and_never_downgrades() {
        let hub = PairHub::default();
        hub.ensure_peer(1, "anon-1".to_string(), ServeCapability::Read);
        // The peer watches its own session so the upgrade re-fans that roster.
        hub.subscribe("s", 1, mpsc::channel::<Arc<str>>(OUT_CHANNEL_CAP).0);

        let peer_capability = |hub: &PairHub| {
            hub.roster("s")["peers"]
                .as_array()
                .expect("peers")
                .iter()
                .find(|p| p["conn_id"] == 1)
                .expect("peer 1")["capability"]
                .as_str()
                .expect("capability")
                .to_string()
        };

        assert_eq!(peer_capability(&hub), "read");
        hub.ensure_peer(1, "ignored".to_string(), ServeCapability::Full);
        assert_eq!(peer_capability(&hub), "full", "Read → Full promotes");
        // A redundant lower request must not lower a peer already at Full.
        hub.ensure_peer(1, "ignored".to_string(), ServeCapability::Read);
        assert_eq!(peer_capability(&hub), "full", "never downgrades");
    }

    /// Legacy clients retain the original recover-on-next-frame state: their
    /// lag flag clears after a marker and they receive future frames.
    #[test]
    fn legacy_lag_recovery_never_becomes_sticky() {
        let hub = PairHub::default();
        hub.ensure_peer(1, "legacy".to_string(), ServeCapability::Read);
        let (tx, mut rx) = mpsc::channel::<Arc<str>>(1);
        hub.subscribe_legacy("s", 1, tx);
        let _ = rx.try_recv(); // initial roster
        hub.broadcast("s", &text_block("first"));
        hub.broadcast("s", &text_block("overflow"));
        assert_eq!(next_render(&mut rx)["text"], "first");
        hub.broadcast("s", &text_block("recovery-boundary"));
        assert_eq!(recv_value(&mut rx)["type"], "resync");
        hub.broadcast("s", &text_block("live-again"));
        assert_eq!(next_render(&mut rx)["text"], "live-again");
    }

    /// A reserved ACK is placed before registration; even an immediate frame
    /// and roster fan-out cannot overtake it on the connection mpsc.
    #[tokio::test]
    async fn subscribe_ack_is_queued_before_any_subscriber_frame() {
        let hub = PairHub::default();
        hub.ensure_peer(1, "anon-1".to_string(), ServeCapability::Read);
        let (tx, mut rx) = mpsc::channel::<Arc<str>>(8);
        let permit_sender = tx.clone();
        let permit = permit_sender.reserve().await.expect("open sender");
        hub.subscribe_with_permit(tx, permit, "s", 1, Some(Vec::new()), 77, true)
            .expect("subscribe");
        hub.broadcast("s", &text_block("after-ack"));
        let first = recv_value(&mut rx);
        assert_eq!(first["id"], 77);
        assert_eq!(first["result"]["id"], "s");
        assert_eq!(first["result"]["floor"], 0);
        assert_eq!(next_render(&mut rx)["text"], "after-ack");
    }

    /// Closing a session cleans every peer's subscription list and keeps their
    /// remaining watched rosters usable (no ghost channel recreation).
    #[test]
    fn close_session_cleans_peer_subscriptions() {
        let hub = PairHub::default();
        hub.ensure_peer(1, "anon-1".to_string(), ServeCapability::Read);
        hub.subscribe("closed", 1, mpsc::channel::<Arc<str>>(OUT_CHANNEL_CAP).0);
        hub.subscribe("kept", 1, mpsc::channel::<Arc<str>>(OUT_CHANNEL_CAP).0);
        hub.close_session("closed");
        let state = hub.lock();
        assert!(!state.channels.contains_key("closed"));
        assert_eq!(state.peers[&1].subscribed, vec!["kept".to_string()]);
    }

    /// A peer's capability is visible in rosters of sessions it does not watch.
    #[test]
    fn capability_upgrade_fans_every_active_channel() {
        let hub = PairHub::default();
        hub.ensure_peer(1, "actor".to_string(), ServeCapability::Read);
        hub.ensure_peer(2, "watcher".to_string(), ServeCapability::Read);
        hub.subscribe("a", 2, mpsc::channel::<Arc<str>>(OUT_CHANNEL_CAP).0);
        hub.subscribe("b", 2, mpsc::channel::<Arc<str>>(OUT_CHANNEL_CAP).0);
        hub.ensure_peer(1, "ignored".to_string(), ServeCapability::Full);
        for session in ["a", "b"] {
            let roster = hub.roster(session);
            let peer = roster["peers"]
                .as_array().unwrap().iter()
                .find(|peer| peer["conn_id"] == 1).unwrap();
            assert_eq!(peer["capability"], "full");
        }
    }

    /// Steering is denied when no turn is in flight and allowed (handing back
    /// the live queue) when one is.
    #[test]
    fn steering_requires_an_active_turn() {
        let hub = PairHub::default();
        assert!(matches!(
            hub.steering_for("s", None),
            SteerAuth::NoActiveTurn
        ));
        let queue = steering();
        hub.begin_turn("s", 7, 1, "anon-1".to_string(), Arc::clone(&queue), Vec::new());
        match hub.steering_for("s", Some(7)) {
            SteerAuth::Allowed(handle) => {
                handle.lock().unwrap().push("go left".to_string());
                assert_eq!(queue.lock().unwrap().len(), 1);
            }
            _ => panic!("steer should be allowed for the live turn"),
        }
        assert!(matches!(
            hub.steering_for("s", Some(999)),
            SteerAuth::TurnMismatch
        ));
        hub.end_turn("s", 7, Vec::new());
        assert!(matches!(
            hub.steering_for("s", None),
            SteerAuth::NoActiveTurn
        ));
    }

    /// A mid-turn subscribe reads the pre-turn snapshot and the current seq, so
    /// it resumes exactly where the live stream is without a gap.
    #[test]
    fn mid_turn_subscribe_resumes_at_next_seq() {
        let hub = PairHub::default();
        hub.ensure_peer(1, "anon-1".to_string(), ServeCapability::Read);
        hub.ensure_peer(2, "anon-2".to_string(), ServeCapability::Read);
        let (tx1, _rx1) = mpsc::channel::<Arc<str>>(OUT_CHANNEL_CAP);
        hub.subscribe("s", 1, tx1);
        let snapshot = vec![HistoryEntry {
            role: "user".to_string(),
            text: "earlier".to_string(),
        }];
        hub.begin_turn("s", 1, 1, "anon-1".to_string(), steering(), snapshot.clone());
        hub.broadcast("s", &text_block("a"));
        hub.broadcast("s", &text_block("b"));

        let (tx2, _rx2) = mpsc::channel::<Arc<str>>(OUT_CHANNEL_CAP);
        let joined = hub.subscribe("s", 2, tx2);
        assert_eq!(joined.snapshot, snapshot, "joiner replays pre-turn history");
        assert_eq!(joined.next_seq, 2, "joiner resumes after the two live frames");
        assert_eq!(joined.helm.as_deref(), Some("anon-1"));
    }

    fn drain_control(rx: &mut mpsc::Receiver<Arc<str>>) {
        while rx.try_recv().is_ok() {}
    }

    fn recv_value(rx: &mut mpsc::Receiver<Arc<str>>) -> serde_json::Value {
        let line = rx.try_recv().expect("a queued line");
        serde_json::from_str(line.trim_end()).expect("json line")
    }

    async fn recv_value_async(rx: &mut mpsc::Receiver<Arc<str>>) -> serde_json::Value {
        let line = rx.recv().await.expect("a queued line");
        serde_json::from_str(line.trim_end()).expect("json line")
    }

    /// Pull the next line that is a render frame (skip roster/control frames).
    fn next_render(rx: &mut mpsc::Receiver<Arc<str>>) -> serde_json::Value {
        loop {
            let value = recv_value(rx);
            if value.get("type").and_then(serde_json::Value::as_str) != Some("roster") {
                return value;
            }
        }
    }
}
